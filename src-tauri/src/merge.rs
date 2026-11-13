//! Bilder zusammenfügen: HDR aus einer Belichtungsreihe, Panorama, Focus
//! Stacking. Läuft im Hintergrund (Fortschritt über `merge-progress`), das
//! Ergebnis ist eine lineare EXR-Datei neben dem ersten Foto, die gleich in
//! den Katalog aufgenommen und wie ein RAW entwickelt wird.

use crate::AppState;
use crate::worker::Job;
use lumenkorn_core::Error;
use lumenkorn_core::LockExt;
use lumenkorn_core::formats::ImageKind;
use lumenkorn_develop::i18n::tr_s;
use lumenkorn_develop::merge::{focus, hdr, pano};
use lumenkorn_develop::{ImageF32, Source, SourceKind};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeKind {
    Hdr,
    Pano,
    Focus,
}

impl MergeKind {
    fn suffix(self) -> &'static str {
        match self {
            Self::Hdr => "HDR",
            Self::Pano => "Pano",
            Self::Focus => "Fokus",
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MergeOptions {
    hdr: hdr::HdrOptions,
    pano: pano::PanoOptions,
    focus: focus::FocusOptions,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct MergeEvent {
    stage: String,
    done: usize,
    total: usize,
    finished: bool,
    error: Option<String>,
    /// Neues Foto im Katalog
    image_id: Option<i64>,
    file_name: Option<String>,
    /// Fotos (1-basiert, Reihenfolge der Auswahl), die sich nicht ausrichten
    /// ließen; leer: alles ausgerichtet
    unaligned: Vec<usize>,
}

#[tauri::command]
pub fn merge_start(
    app: AppHandle,
    kind: MergeKind,
    ids: Vec<i64>,
    options: Option<MergeOptions>,
) -> Result<(), Error> {
    if ids.len() < 2 {
        return Err(Error::Invalid(
            tr_s(
                "Mindestens zwei Fotos auswählen.",
                "Select at least two photos.",
            )
            .into(),
        ));
    }
    if ids.len() > 200 {
        return Err(Error::Invalid(
            tr_s(
                "Höchstens 200 Fotos auf einmal zusammenfügen.",
                "Merge at most 200 photos at once.",
            )
            .into(),
        ));
    }
    let claim = crate::task::claim(&app, |t| &t.merge)?.full_res()?;
    let options = options.unwrap_or_default();
    claim.spawn("merge", move |app, claim| {
        let state = app.state::<AppState>();
        let emit = |ev: MergeEvent| {
            let _ = app.emit("merge-progress", ev);
        };
        let t = std::time::Instant::now();
        // Auch nach einer Panik (z. B. ein Dekoder an einer kaputten Datei)
        // muss die Funktion wieder frei werden und die Oberfläche es erfahren.
        let res = crate::task::catch_panic(|| run(app, &state, claim.task(), kind, &ids, &options));
        let cancelled = claim.task().cancelled();
        // Vor der Meldung freigeben: Die Oberfläche darf gleich neu starten
        drop(claim);
        // Abbrechen ist kein Fehler: „Abgebrochen“/„Canceled“ melden (die Oberfläche schweigt dann)
        match res {
            Err(_) if cancelled => emit(MergeEvent {
                stage: String::new(),
                done: 0,
                total: 0,
                finished: true,
                error: Some(lumenkorn_develop::i18n::tr_s("Abgebrochen", "Canceled").into()),
                image_id: None,
                file_name: None,
                unaligned: vec![],
            }),
            Ok((id, name, unaligned)) => {
                log::info!(
                    "{kind:?} aus {} Fotos in {:.1?}: {name}",
                    ids.len(),
                    t.elapsed()
                );
                emit(MergeEvent {
                    stage: tr_s("Fertig", "Done").into(),
                    done: 1,
                    total: 1,
                    finished: true,
                    error: None,
                    image_id: id,
                    file_name: Some(name),
                    unaligned: unaligned.iter().map(|i| i + 1).collect(),
                })
            }
            Err(e) => {
                log::warn!("{kind:?} fehlgeschlagen: {e}");
                emit(MergeEvent {
                    stage: String::new(),
                    done: 0,
                    total: 0,
                    finished: true,
                    error: Some(e.to_string()),
                    image_id: None,
                    file_name: None,
                    unaligned: vec![],
                })
            }
        }
    })
}

#[tauri::command]
pub fn merge_cancel(state: State<AppState>) {
    state.tasks.merge.cancel();
}

fn run(
    app: &AppHandle,
    state: &AppState,
    task: &crate::task::Task,
    kind: MergeKind,
    ids: &[i64],
    options: &MergeOptions,
) -> Result<(Option<i64>, String, Vec<usize>), Error> {
    let jobs = {
        let cat = state.catalog.locked();
        ids.iter()
            .map(|id| cat.thumb_job(*id))
            .collect::<Result<Vec<_>, _>>()?
    };
    // Aus RAWs entstanden → wie ein RAW entwickeln, sonst wie ein JPEG
    let scene_linear = AtomicBool::new(true);
    let load = |i: usize| -> lumenkorn_develop::Result<ImageF32> {
        let job = &jobs[i];
        let src = Source::load(
            Path::new(&job.path),
            job.kind == ImageKind::Raw,
            job.orientation.map(|o| o as u16),
            None,
        )?;
        if src.kind == SourceKind::Raster {
            scene_linear.store(false, Ordering::Relaxed);
        }
        // Objektivkorrektur vor dem Ausrichten: Mit Verzeichnung passen die
        // Aufnahmen an den Rändern – wo sie sich überlappen – nicht genau
        // aufeinander, und Nähte im Panorama zeigen dann Versätze. Das
        // Ergebnis (EXR) hat keine Objektivdaten mehr, wird also nicht doppelt
        // korrigiert.
        Ok(match &src.lens {
            Some(lens) => lens.apply(&src.image),
            None => src.image,
        })
    };
    // Höchstens ~10 Meldungen pro Sekunde
    let last = Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(1));
    let progress = |stage: &str, done: usize, total: usize| {
        let mut l = last.locked();
        if done == 0 || done >= total || l.elapsed().as_millis() >= 100 {
            *l = std::time::Instant::now();
            let _ = app.emit(
                "merge-progress",
                MergeEvent {
                    stage: stage.into(),
                    done,
                    total,
                    finished: false,
                    error: None,
                    image_id: None,
                    file_name: None,
                    unaligned: vec![],
                },
            );
        }
    };
    let cancel = task.cancel_flag();
    let n = jobs.len();
    // Referenzfoto (Index in `ids`), dessen Aufnahmedaten übernommen werden:
    // HDR ist auf die mittlere Belichtung skaliert (nicht auf das erste Foto,
    // das z. B. die −2-EV-Aufnahme sein kann), Focus Stacking richtet auf die
    // mittlere Aufnahme aus.
    let (img, reference, unaligned) = match kind {
        MergeKind::Hdr => {
            let m = hdr::merge(n, &load, options.hdr, &progress, cancel)?;
            (m.image, m.reference, m.unaligned)
        }
        MergeKind::Pano => {
            let m = pano::merge_report(n, &load, options.pano, &progress, cancel)?;
            (m.image, 0, m.unaligned)
        }
        MergeKind::Focus => {
            let m = focus::merge_report(n, &load, options.focus, &progress, cancel)?;
            (m.image, n / 2, m.unaligned)
        }
    };
    if task.cancelled() {
        return Err(Error::Invalid(tr_s("abgebrochen", "canceled").into()));
    }

    // Neben das erste Foto schreiben
    progress(tr_s("Speichern", "Saving"), 0, 1);
    let first = Path::new(&jobs[0].path);
    let dir = first.parent().unwrap_or(Path::new("."));
    let stem = first.file_stem().and_then(|s| s.to_str()).unwrap_or("Bild");
    let out: PathBuf =
        crate::develop::unique_path(dir, &format!("{stem}-{}", kind.suffix()), "exr");
    lumenkorn_develop::exr_io::write(&img, &out, scene_linear.load(Ordering::Relaxed))?;
    drop(img);
    let name = out
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    // In den Katalog aufnehmen, Aufnahmedaten vom Referenzfoto übernehmen
    let reference = ids[reference];
    let root_id = state.catalog.locked().image_root(reference)?;
    // Die Ordnerüberwachung meldet die neue Datei ebenfalls – scannt sie
    // gleichzeitig, kann einer der beiden Scans scheitern. Entscheidend ist
    // nur, dass die Datei danach im Katalog steht.
    let report = lumenkorn_core::scanner::scan_paths(
        &state.catalog,
        root_id,
        std::slice::from_ref(&out),
        &|_| {},
    );
    let adopt = || {
        state
            .catalog
            .locked()
            .adopt_capture_info(&out.to_string_lossy(), reference)
    };
    let mut id = adopt()?;
    if id.is_none() {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        id = adopt()?;
    }
    match report {
        Ok(report) => {
            let _ = app.emit("library-changed", report);
        }
        Err(e) => log::warn!("Zusammenfügen: Katalog-Scan von {name}: {e}"),
    }
    state.worker.submit(Job::Thumbs);
    Ok((id, name, unaligned))
}
