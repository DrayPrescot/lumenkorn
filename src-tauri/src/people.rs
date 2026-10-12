//! Personenerkennung in der App: Gesichter im Hintergrund suchen, Personen
//! verwalten, Gesichtsvorschauen liefern.

use crate::AppState;
use crate::ai::{analysis_guard, err, lazy_model};
use lumenkorn_ai::faces::{self, FaceDetector, FaceEmbedder};
use lumenkorn_ai::models;
use lumenkorn_core::Error;
use lumenkorn_core::LockExt;
use lumenkorn_core::people::{FaceInfo, NewFace, PeopleIndex, Person, PersonGroup};
use lumenkorn_core::thumbs::decode_for_analysis;
use lumenkorn_develop::i18n::tr_s;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

/// Bilder pro Stapel – danach kommen andere Aufträge (Scan, Thumbnails) zum Zug.
const BATCH: usize = 8;
/// Längste Kante des untersuchten Bildes
const ANALYSIS_EDGE: u32 = 1600;
/// Kantenlänge der Gesichtsvorschau
const FACE_THUMB: u32 = 160;

#[derive(Default)]
pub struct PeopleState {
    models: Mutex<Option<Arc<(FaceDetector, FaceEmbedder)>>>,
    index: Mutex<Option<PeopleIndex>>,
    load_error: Mutex<Option<String>>,
}

impl PeopleState {
    /// Zentroide verwerfen – nach Änderungen durch den Nutzer neu aufbauen.
    pub(crate) fn invalidate(&self) {
        *self.index.lock().unwrap() = None;
    }
}

pub(crate) fn thumb_dir(state: &AppState) -> PathBuf {
    state.data_dir.join(crate::dirs::FACES)
}

/// Entlädt die Gesichtsmodelle, wenn sie gerade nicht benutzt werden.
pub(crate) fn release_models(state: &AppState) -> bool {
    crate::ai::drop_if_unused(&state.people.models)
}

fn face_models(state: &AppState) -> Result<Arc<(FaceDetector, FaceEmbedder)>, Error> {
    state.usage.touch(crate::memory::Slot::Background);
    lazy_model(
        state,
        &state.people.models,
        tr_s("YuNet/SFace (Personen)", "YuNet/SFace (people)"),
        || {
            let dir = models::faces_dir(&state.data_dir);
            Ok((
                FaceDetector::load(&dir.join(faces::DETECTOR_FILE)).map_err(err)?,
                FaceEmbedder::load(&dir.join(faces::EMBEDDER_FILE)).map_err(err)?,
            ))
        },
    )
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FacesProgress {
    done: i64,
    total: i64,
    faces: i64,
    ids: Vec<i64>,
}

/// Sucht Gesichter in einem Stapel Bilder. `Ok(true)`, wenn noch weitere ausstehen.
pub fn run_batch(app: &AppHandle) -> Result<bool, Error> {
    let state = app.state::<AppState>();
    if !state.ai.settings.lock().unwrap().faces
        || state.ai.downloading.load(Ordering::Relaxed)
        || !models::status(&state.data_dir).faces_ready
        || state.people.load_error.lock().unwrap().is_some()
    {
        return Ok(false);
    }
    let ids = state.catalog.locked().pending_faces(BATCH)?;
    if ids.is_empty() {
        return Ok(false);
    }
    let m = match face_models(&state) {
        Ok(m) => m,
        Err(e) => {
            *state.people.load_error.lock().unwrap() = Some(e.to_string());
            return Err(e);
        }
    };
    let (det, emb) = (&m.0, &m.1);
    let tdir = thumb_dir(&state);
    std::fs::create_dir_all(&tdir)?;

    // Dekodieren (RAW-Vorschau, 1600 px) parallel für den ganzen Stapel – die
    // Erkennung selbst läuft danach Bild für Bild auf dem Modell.
    let jobs: Vec<_> = {
        let cat = state.catalog.locked();
        ids.iter().map(|id| (*id, cat.thumb_job(*id))).collect()
    };
    let decoded: Vec<(i64, Result<image::RgbImage, String>)> = {
        use rayon::prelude::*;
        jobs.into_par_iter()
            .map(|(id, job)| {
                let img = job
                    .map_err(|e| e.to_string())
                    .and_then(|j| decode_for_analysis(&j, ANALYSIS_EDGE).map_err(|e| e.to_string()))
                    .map(|i| i.to_rgb8());
                (id, img)
            })
            .collect()
    };
    for (id, img) in decoded {
        let img = match img {
            Ok(i) => i,
            Err(e) => {
                log::warn!("Gesichter {id}: {e}");
                state.catalog.locked().set_faces_failed(id)?;
                continue;
            }
        };
        let (w, h) = (img.width() as f32, img.height() as f32);
        // Fehler bei einem Bild betreffen nur dieses – sonst stünde es bei
        // jedem Durchlauf wieder vorn und hielte die ganze Erkennung auf.
        let found = {
            let _g = analysis_guard(&state, tr_s("Personenerkennung", "Face recognition"));
            det.detect(&img).map_err(err).and_then(|faces| {
                faces
                    .into_iter()
                    .map(|f| emb.embed(&img, &f).map_err(err).map(|e| (f, e)))
                    .collect::<Result<Vec<_>, _>>()
            })
        };
        let found = match found {
            Ok(f) => f,
            Err(e) => {
                log::warn!("Gesichter {id}: {e}");
                state.catalog.locked().set_faces_failed(id)?;
                continue;
            }
        };
        let new: Vec<NewFace> = found
            .iter()
            .map(|(f, e)| NewFace {
                rect: [f.bbox[0] / w, f.bbox[1] / h, f.bbox[2] / w, f.bbox[3] / h],
                score: f.score,
                embedding: e.clone(),
            })
            .collect();
        let face_ids = {
            let mut cat = state.catalog.locked();
            let mut idx = state.people.index.lock().unwrap();
            if idx.as_ref().is_none_or(|i| !cat.people_index_current(i)) {
                *idx = Some(cat.people_index()?);
            }
            match cat.save_faces(id, &new, idx.as_mut().unwrap()) {
                Ok(ids) => ids,
                Err(e) => {
                    // Der Index wurde womöglich schon ergänzt – neu aufbauen
                    *idx = None;
                    log::warn!("Gesichter {id} speichern: {e}");
                    cat.set_faces_failed(id)?;
                    continue;
                }
            }
        };
        // Vorschaubilder gleich mit erzeugen – das Bild ist schon dekodiert.
        for (fid, (f, _)) in face_ids.iter().zip(&found) {
            let t = faces::thumbnail(&img, f, FACE_THUMB);
            let _ = t.save_with_format(tdir.join(format!("{fid}.jpg")), image::ImageFormat::Jpeg);
        }
    }

    let (done, total, faces) = state.catalog.locked().face_counts()?;
    let _ = app.emit(
        "faces-progress",
        FacesProgress {
            done,
            total,
            faces,
            ids,
        },
    );
    Ok(true)
}

/// Vorschau eines Gesichts (für das lw://-Schema). Fehlt sie, wird sie erzeugt.
pub fn face_thumb(state: &AppState, face_id: i64) -> Option<Vec<u8>> {
    let path = thumb_dir(state).join(format!("{face_id}.jpg"));
    if let Ok(b) = std::fs::read(&path) {
        return Some(b);
    }
    let (image_id, rect) = {
        let cat = state.catalog.locked();
        let f = cat.face(face_id).ok()?;
        (f.image_id, f.rect)
    };
    let job = state.catalog.locked().thumb_job(image_id).ok()?;
    let img = decode_for_analysis(&job, ANALYSIS_EDGE).ok()?.to_rgb8();
    let (w, h) = (img.width() as f32, img.height() as f32);
    let face = faces::Face {
        bbox: [
            rect[0] as f32 * w,
            rect[1] as f32 * h,
            rect[2] as f32 * w,
            rect[3] as f32 * h,
        ],
        landmarks: [[0.0; 2]; 5],
        score: 1.0,
    };
    let t = faces::thumbnail(&img, &face, FACE_THUMB);
    let _ = std::fs::create_dir_all(thumb_dir(state));
    t.save_with_format(&path, image::ImageFormat::Jpeg).ok()?;
    std::fs::read(&path).ok()
}

// --- Befehle -------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeopleStatus {
    ready: bool,
    download_size: u64,
    enabled: bool,
    analyzed: i64,
    total: i64,
    faces: i64,
    error: Option<String>,
}

#[tauri::command(async)]
pub fn people_status(state: State<AppState>) -> Result<PeopleStatus, Error> {
    let files = models::status(&state.data_dir);
    let (analyzed, total, faces) = state.catalog.locked().face_counts()?;
    Ok(PeopleStatus {
        ready: files.faces_ready,
        download_size: files.faces_download_size,
        enabled: state.ai.settings.lock().unwrap().faces,
        analyzed,
        total,
        faces,
        error: state.people.load_error.lock().unwrap().clone(),
    })
}

#[tauri::command(async)]
pub fn people_list(state: State<AppState>) -> Result<Vec<Person>, Error> {
    state.catalog.locked().list_people()
}

#[tauri::command(async)]
pub fn person_groups(state: State<AppState>) -> Result<Vec<PersonGroup>, Error> {
    state.catalog.locked().person_groups()
}

/// Neue Gruppe (oder die vorhandene gleichen Namens); mit `persons` gleich
/// diese Personen hineinlegen. Liefert die ID der Gruppe.
#[tauri::command(async)]
pub fn person_group_create(
    state: State<AppState>,
    name: String,
    persons: Vec<i64>,
) -> Result<i64, Error> {
    let cat = state.catalog.locked();
    let id = cat.create_person_group(&name)?;
    if !persons.is_empty() {
        cat.set_person_group(&persons, Some(id))?;
    }
    Ok(id)
}

#[tauri::command(async)]
pub fn person_group_rename(state: State<AppState>, id: i64, name: String) -> Result<i64, Error> {
    state.catalog.locked().rename_person_group(id, &name)
}

#[tauri::command(async)]
pub fn person_group_delete(state: State<AppState>, id: i64) -> Result<(), Error> {
    state.catalog.locked().delete_person_group(id)
}

#[tauri::command(async)]
pub fn person_set_group(
    state: State<AppState>,
    persons: Vec<i64>,
    group: Option<i64>,
) -> Result<(), Error> {
    state.catalog.locked().set_person_group(&persons, group)
}

#[tauri::command(async)]
pub fn person_faces(state: State<AppState>, id: i64) -> Result<Vec<FaceInfo>, Error> {
    state.catalog.locked().person_faces(id)
}

#[tauri::command(async)]
pub fn image_faces(state: State<AppState>, id: i64) -> Result<Vec<FaceInfo>, Error> {
    state.catalog.locked().image_faces(id)
}

#[tauri::command(async)]
pub fn person_rename(state: State<AppState>, id: i64, name: String) -> Result<i64, Error> {
    // Den Index der Analyse weiterreichen: Er wird nachgeführt statt bei jedem
    // Klick neu aufgebaut (bei Fehlern verworfen). Sperren in derselben
    // Reihenfolge wie `run_batch`: Katalog, dann Index.
    let mut cat = state.catalog.locked();
    cat.name_person(id, &name, &mut state.people.index.lock().unwrap())
}

#[tauri::command(async)]
pub fn face_assign(state: State<AppState>, face_id: i64, name: String) -> Result<i64, Error> {
    let mut cat = state.catalog.locked();
    cat.assign_face(face_id, &name, &mut state.people.index.lock().unwrap())
}

#[tauri::command(async)]
pub fn faces_of_images(state: State<AppState>, ids: Vec<i64>) -> Result<Vec<FaceInfo>, Error> {
    state.catalog.locked().faces_of_images(&ids)
}

/// Mehrere Gesichter (z. B. aus allen ausgewählten Fotos) einer Person
/// zuordnen oder – ohne Namen – aus ihrer bisherigen Person herausnehmen.
#[tauri::command(async)]
pub fn faces_reassign(
    state: State<AppState>,
    face_ids: Vec<i64>,
    name: Option<String>,
) -> Result<Option<i64>, Error> {
    let mut cat = state.catalog.locked();
    cat.reassign_faces(
        &face_ids,
        name.as_deref(),
        &mut state.people.index.lock().unwrap(),
    )
}

/// Mehrere Vorschläge auf einmal bestätigen.
#[tauri::command(async)]
pub fn faces_confirm(state: State<AppState>, face_ids: Vec<i64>) -> Result<usize, Error> {
    let n = state.catalog.locked().confirm_faces(&face_ids)?;
    state.people.invalidate();
    Ok(n)
}

#[tauri::command(async)]
pub fn face_confirm(state: State<AppState>, face_id: i64) -> Result<(), Error> {
    state.catalog.locked().confirm_face(face_id)?;
    state.people.invalidate();
    Ok(())
}

#[tauri::command(async)]
pub fn face_reject(state: State<AppState>, face_id: i64) -> Result<(), Error> {
    state.catalog.locked().reject_face(face_id)?;
    state.people.invalidate();
    Ok(())
}

#[tauri::command(async)]
pub fn face_hide(state: State<AppState>, face_id: i64) -> Result<(), Error> {
    state.catalog.locked().hide_face(face_id)?;
    state.people.invalidate();
    Ok(())
}

#[tauri::command(async)]
pub fn person_delete(state: State<AppState>, id: i64) -> Result<(), Error> {
    state.catalog.locked().delete_person(id)?;
    state.people.invalidate();
    Ok(())
}

/// Alle Fotos erneut nach Gesichtern absuchen. Namen bleiben erhalten,
/// Zuordnungen werden neu vorgeschlagen.
#[tauri::command(async)]
pub fn people_rescan(state: State<AppState>) -> Result<(), Error> {
    state.catalog.locked().reset_faces()?;
    state.people.invalidate();
    *state.people.load_error.lock().unwrap() = None;
    let _ = std::fs::remove_dir_all(thumb_dir(&state));
    state.worker.submit(crate::worker::Job::Faces);
    Ok(())
}
