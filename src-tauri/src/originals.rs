//! Bearbeitung ins Original übernehmen, Original wiederherstellen und
//! gesicherte Originale löschen.
//!
//! Beim Übernehmen wird die Datei des Fotos durch die entwickelte Fassung
//! ersetzt. Das bisherige Original bleibt im Ordner „Original“ daneben
//! (`scanner::original_dir_for`): als harter Link auf dieselbe Datei, wo das
//! nicht geht als Kopie. Erst danach ersetzt ein einziges Umbenennen die Datei
//! des Fotos, sie ist also zu keinem Zeitpunkt weg. Nur JPEG, TIFF und PNG,
//! jeweils ins selbe Format und in voller Auflösung; RAW und DNG nie. Die
//! Bearbeitung kommt in einen Schnappschuss und wird zurückgesetzt, damit sie
//! nicht doppelt wirkt. KI-Agenten haben keinen Zugang (kein MCP-Werkzeug).

use crate::AppState;
use crate::worker::Job;
use lumenkorn_core::catalog::{BakeCandidate, SavedOriginal};
use lumenkorn_core::formats::ImageKind;
use lumenkorn_core::{Error, LockExt};
use lumenkorn_develop::i18n::tr_s;
use lumenkorn_develop::output::{self, ExportFormat, ExportOptions};
use lumenkorn_develop::tr;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, State};

/// JPEG-Qualität beim Zurückschreiben: hoch, damit die erneute Kompression
/// kaum auffällt
const JPEG_QUALITY: u8 = 95;

/// Warum ein Foto nicht übernommen wird
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Skip {
    Missing,
    Raw,
    Unsupported,
    Unedited,
}

/// Format, in das sich das Foto zurückschreiben lässt: dasselbe wie das
/// Original. RAW (auch DNG) nie: Die Rohdaten gingen verloren.
fn bake_format(c: &BakeCandidate) -> Result<ExportFormat, Skip> {
    if c.missing {
        return Err(Skip::Missing);
    }
    if c.kind == ImageKind::Raw {
        return Err(Skip::Raw);
    }
    let ext = Path::new(&c.path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let format = match ext.as_str() {
        "jpg" | "jpeg" | "jpe" | "jfif" => ExportFormat::Jpeg,
        "tif" | "tiff" => ExportFormat::Tiff16,
        "png" => ExportFormat::Png,
        _ => return Err(Skip::Unsupported),
    };
    if !c.edited {
        return Err(Skip::Unedited);
    }
    Ok(format)
}

/// Vor dem Übernehmen: Wie viele Fotos gehen, wie viele nicht und warum
#[derive(Serialize, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BakeSummary {
    ready: usize,
    raw: usize,
    unsupported: usize,
    unedited: usize,
    missing: usize,
}

fn summarize(candidates: &[BakeCandidate]) -> BakeSummary {
    let mut s = BakeSummary::default();
    for c in candidates {
        match bake_format(c) {
            Ok(_) => s.ready += 1,
            Err(Skip::Raw) => s.raw += 1,
            Err(Skip::Unsupported) => s.unsupported += 1,
            Err(Skip::Unedited) => s.unedited += 1,
            Err(Skip::Missing) => s.missing += 1,
        }
    }
    s
}

#[tauri::command(async)]
pub fn bake_summary(state: State<AppState>, ids: Vec<i64>) -> Result<BakeSummary, Error> {
    let candidates = state.catalog.locked().bake_candidates(&ids)?;
    Ok(summarize(&candidates))
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BakeProgress {
    done: usize,
    total: usize,
    current: Option<String>,
    failed: Vec<String>,
    /// Übernommene Fotos
    baked: Vec<i64>,
    finished: bool,
}

/// Übernimmt die Bearbeitung der Fotos `ids` in ihre Dateien, im Hintergrund.
/// Fortschritt per `bake-progress`. Fotos, die nicht gehen (RAW, unbearbeitet
/// …), werden ausgelassen.
#[tauri::command(async)]
pub fn bake_originals(app: AppHandle, ids: Vec<i64>) -> Result<(), Error> {
    // Dieselbe Sperre wie der Export: beide entwickeln in voller Auflösung
    let Some(running) = crate::busy::claim_in(&app, |s| &s.develop.exporting) else {
        return Err(Error::Invalid(
            tr_s(
                "Es läuft bereits ein Export – bitte warten, bis er fertig ist.",
                "An export is already running – please wait until it has finished.",
            )
            .into(),
        ));
    };
    std::thread::Builder::new()
        .name("lumenkorn-bake".into())
        .spawn(move || {
            let _running = running;
            let state = app.state::<AppState>();
            let snapshot = tr_s(
                "Vor Übernahme ins Original",
                "Before applying to the original",
            );
            let total = ids.len();
            let mut failed = Vec::new();
            let mut baked = Vec::new();
            for (i, id) in ids.iter().enumerate() {
                let name = state
                    .catalog
                    .locked()
                    .thumb_job(*id)
                    .ok()
                    .and_then(|j| {
                        Path::new(&j.path)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                    })
                    .unwrap_or_else(|| id.to_string());
                let _ = app.emit(
                    "bake-progress",
                    BakeProgress {
                        done: i,
                        total,
                        current: Some(name.clone()),
                        failed: failed.clone(),
                        baked: baked.clone(),
                        finished: false,
                    },
                );
                // Ein Panic (z. B. in der GPU-Pipeline) beendet nicht den ganzen Lauf
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    bake_one(&state, *id, snapshot)
                }))
                .unwrap_or_else(|_| Err(tr_s("Interner Fehler", "Internal error").into()));
                match res {
                    Ok(true) => baked.push(*id),
                    Ok(false) => {}
                    Err(e) => {
                        log::warn!("Übernahme ins Original {id}: {e}");
                        failed.push(format!("{name}: {e}"));
                    }
                }
            }
            lumenkorn_develop::release_large_caches();
            state.worker.submit(Job::Thumbs);
            log::info!(
                "Bearbeitung ins Original übernommen: {} von {total}, {} Fehler",
                baked.len(),
                failed.len()
            );
            let _ = app.emit("catalog-updated", ());
            if !baked.is_empty() {
                let _ = app.emit("develop-external", serde_json::json!({ "ids": baked }));
            }
            let _ = app.emit(
                "bake-progress",
                BakeProgress {
                    done: total,
                    total,
                    current: None,
                    failed,
                    baked,
                    finished: true,
                },
            );
        })?;
    Ok(())
}

/// Ein Foto übernehmen. `Ok(false)`: geht nicht (RAW, unbearbeitet …).
fn bake_one(state: &AppState, id: i64, snapshot: &str) -> Result<bool, String> {
    let e = |e: Error| e.to_string();
    let (candidate, job, params, meta, saved) = {
        let cat = state.catalog.locked();
        let Some(candidate) = cat.bake_candidates(&[id]).map_err(e)?.into_iter().next() else {
            return Ok(false);
        };
        let job = cat.thumb_job(id).map_err(e)?;
        let params = cat.develop_params(id).map_err(e)?;
        // Alle Metadaten des Fotos, anders als beim Export auch der Ort
        let mut meta = cat
            .export_meta(id)
            .inspect_err(|err| log::warn!("Metadaten für {id}: {err}"))
            .unwrap_or_default();
        meta.gps = cat.location(id).ok().flatten().map(|l| (l.lat, l.lon));
        let saved = cat.saved_originals(&[id]).map_err(e)?.into_iter().next();
        (candidate, job, params, meta, saved)
    };
    let Ok(format) = bake_format(&candidate) else {
        return Ok(false);
    };
    let photo = PathBuf::from(&candidate.path);
    let img = crate::develop::develop_for_export(state, &job, &params, None).map_err(e)?;
    // Während des Entwickelns weiter bearbeitet: nicht ersetzen
    if state.catalog.locked().develop_params(id).map_err(e)? != params {
        return Err(tr_s(
            "Wurde währenddessen bearbeitet – bitte noch einmal übernehmen.",
            "Was edited in the meantime – please apply again.",
        )
        .into());
    }
    // Das erste Original bleibt gesichert; sonst wird das jetzige gesichert
    let keep_saved = saved
        .as_ref()
        .is_some_and(|s| Path::new(&s.backup).is_file());
    let backup = if keep_saved {
        None
    } else {
        let dir = lumenkorn_core::scanner::original_dir_for(&photo).map_err(|e| {
            tr!(
                "Ordner für das Original: {e}",
                "Folder for the original: {e}"
            )
        })?;
        Some(free_name(&dir, &photo)?)
    };
    let opts = ExportOptions {
        format,
        formats: Vec::new(),
        quality: JPEG_QUALITY,
        long_edge: None,
        dng_edited: true,
        on_conflict: Default::default(),
    };
    // Datei und Katalog gehören zusammen: das Beenden wartet
    let _critical = crate::exit::critical();
    replace_keeping_backup(&photo, backup.as_deref(), |staging| {
        output::write(&img, staging, &opts, Some(&meta)).map_err(|e| e.to_string())
    })?;
    drop(img);
    let backup_str = backup.as_ref().map(|b| b.to_string_lossy().into_owned());
    if let Err(err) = state
        .catalog
        .locked()
        .bake_done(id, backup_str.as_deref(), snapshot)
    {
        // Katalog nicht geändert: das Original zurücklegen, sonst wirkte die
        // Bearbeitung auf der neuen Datei doppelt
        if let Some(b) = &backup
            && let Err(back) = std::fs::rename(b, &photo)
        {
            log::error!(
                "{} nicht zurückgelegt: {back}; Original liegt in {}",
                photo.display(),
                b.display()
            );
        }
        return Err(err.to_string());
    }
    refresh(state, id, &photo);
    Ok(true)
}

/// Neue Datei im Katalog nachziehen (Maße, Metadaten, Vorschau)
fn refresh(state: &AppState, id: i64, photo: &Path) {
    state.cache.remove(id);
    state.develop.forget(id);
    match state.catalog.locked().image_root(id) {
        Ok(root) => state.worker.submit_changes(root, [photo.to_path_buf()]),
        Err(e) => log::warn!("Foto {id} nicht neu eingelesen: {e}"),
    }
}

/// Freier Name für das Original im Ordner `dir`: wie das Foto, sonst
/// `Name-2` usw. Eine vorhandene Datei wird nie ersetzt.
fn free_name(dir: &Path, photo: &Path) -> Result<PathBuf, String> {
    let stem = photo.file_stem().map(|s| s.to_string_lossy().into_owned());
    let ext = photo.extension().map(|s| s.to_string_lossy().into_owned());
    match (stem, ext) {
        (Some(s), Some(x)) => Ok(crate::develop::unique_path(dir, &s, &x)),
        _ => Err(tr!(
            "Ungültiger Dateiname: {}",
            "Invalid file name: {}",
            photo.display()
        )),
    }
}

/// Ersetzt `photo` durch das, was `write` in eine Zwischendatei schreibt.
/// Vorher bekommt `backup` (sofern angegeben) das bisherige Original: als harter
/// Link (dieselbe Datei, nichts wird kopiert), wo das nicht geht als Kopie. Dann
/// ersetzt ein einziges Umbenennen das Foto, es ist also zu keinem Zeitpunkt weg.
/// Scheitert ein Schritt, bleibt das Foto unverändert und die Sicherung wird
/// wieder entfernt.
fn replace_keeping_backup(
    photo: &Path,
    backup: Option<&Path>,
    write: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    // Der Zwischenname enthält den alten Namen und hat keine Bildendung: Der
    // Scanner übergeht ihn, und man erkennt, wozu er gehört
    let mut staging = photo.as_os_str().to_owned();
    staging.push(".lumenkorn-neu.tmp");
    let staging = PathBuf::from(staging);
    if let Err(e) = write(&staging) {
        let _ = std::fs::remove_file(&staging);
        return Err(e);
    }
    if let Some(b) = backup {
        let saved = if b.exists() {
            Err(tr!("{} gibt es schon", "{} already exists", b.display()))
        } else {
            link_or_copy(photo, b)
        };
        if let Err(e) = saved {
            let _ = std::fs::remove_file(&staging);
            return Err(tr!(
                "Original nicht gesichert: {e}",
                "Original not saved: {e}"
            ));
        }
    }
    if let Err(e) = std::fs::rename(&staging, photo) {
        let _ = std::fs::remove_file(&staging);
        if let Some(b) = backup {
            let _ = std::fs::remove_file(b);
        }
        return Err(e.to_string());
    }
    Ok(())
}

/// `to` wird zu einer zweiten Fassung von `from`: harter Link, sonst eine
/// vollständige Kopie, die auf der Platte steht, bevor es weitergeht
fn link_or_copy(from: &Path, to: &Path) -> Result<(), String> {
    if std::fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    let copied = (|| -> std::io::Result<()> {
        let n = std::fs::copy(from, to)?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(to)?
            .sync_all()?;
        if n != std::fs::metadata(from)?.len() {
            return Err(std::io::Error::other("Kopie unvollständig"));
        }
        Ok(())
    })();
    if let Err(e) = copied {
        let _ = std::fs::remove_file(to);
        return Err(e.to_string());
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginalsReport {
    /// Erledigte Fotos
    done: Vec<i64>,
    failed: Vec<String>,
}

/// Legt die gesicherten Originale der Fotos `ids` zurück und macht die
/// Bearbeitung von davor wieder zur aktuellen. Die übernommene Fassung fällt
/// weg: Sie lässt sich aus Original und Bearbeitung jederzeit neu erzeugen.
#[tauri::command]
pub async fn restore_originals(app: AppHandle, ids: Vec<i64>) -> Result<OriginalsReport, Error> {
    let emitter = app.clone();
    let report = crate::blocking(app, move |state| {
        let saved = state.catalog.locked().saved_originals(&ids)?;
        let snapshot = tr_s(
            "Vor Wiederherstellen des Originals",
            "Before restoring the original",
        );
        let mut done = Vec::new();
        let mut failed = Vec::new();
        for s in &saved {
            match restore_one(state, s, snapshot) {
                Ok(()) => done.push(s.image_id),
                Err(e) => {
                    log::warn!("Original von {} nicht wiederhergestellt: {e}", s.photo);
                    let name = Path::new(&s.photo)
                        .file_name()
                        .map_or_else(|| s.photo.clone(), |n| n.to_string_lossy().into_owned());
                    failed.push(format!("{name}: {e}"));
                }
            }
        }
        state.worker.submit(Job::Thumbs);
        Ok(OriginalsReport { done, failed })
    })
    .await?;
    let _ = emitter.emit("catalog-updated", ());
    if !report.done.is_empty() {
        let _ = emitter.emit(
            "develop-external",
            serde_json::json!({ "ids": report.done }),
        );
    }
    Ok(report)
}

/// Original zurücklegen: erst eine zweite Fassung neben das Foto, die dann mit
/// einem Umbenennen das Foto ersetzt. Die Sicherung im Ordner „Original“ wird
/// erst gelöscht, wenn auch der Katalog umgestellt ist.
fn restore_one(state: &AppState, s: &SavedOriginal, snapshot: &str) -> Result<(), String> {
    let backup = Path::new(&s.backup);
    let photo = Path::new(&s.photo);
    if !backup.is_file() {
        return Err(tr!(
            "Gesichertes Original fehlt: {}",
            "Saved original is missing: {}",
            backup.display()
        ));
    }
    let _critical = crate::exit::critical();
    replace_keeping_backup(photo, None, |staging| link_or_copy(backup, staging))?;
    state
        .catalog
        .locked()
        .restore_done(s.image_id, snapshot)
        .map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::remove_file(backup) {
        log::warn!("{} nicht entfernt: {e}", backup.display());
    }
    if let Some(dir) = backup.parent() {
        lumenkorn_core::scanner::remove_original_dir_if_empty(dir);
    }
    refresh(state, s.image_id, photo);
    Ok(())
}

/// Darf das gesicherte Original gelöscht werden? Nur wenn das Foto noch da ist
/// (sonst wäre es die einzige Datei) und die Sicherung wirklich eine andere
/// Datei ist.
fn deletable(s: &SavedOriginal) -> Result<u64, String> {
    let backup = Path::new(&s.backup);
    let meta = std::fs::metadata(backup).map_err(|e| e.to_string())?;
    if !Path::new(&s.photo).is_file() {
        return Err(tr_s(
            "Das Foto selbst fehlt – das Original ist die einzige Datei.",
            "The photo itself is missing – the original is the only file.",
        )
        .into());
    }
    if same_file::is_same_file(backup, &s.photo).unwrap_or(true) {
        return Err(tr_s(
            "Original und Foto sind dieselbe Datei.",
            "Original and photo are the same file.",
        )
        .into());
    }
    Ok(meta.len())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginalsSummary {
    count: usize,
    bytes: u64,
    trash_supported: bool,
}

/// Wie viele gesicherte Originale der Fotos `ids` würden gelöscht?
#[tauri::command(async)]
pub fn originals_summary(state: State<AppState>, ids: Vec<i64>) -> Result<OriginalsSummary, Error> {
    let saved = state.catalog.locked().saved_originals(&ids)?;
    // Dateien ohne Katalogsperre prüfen
    let sizes: Vec<u64> = saved.iter().filter_map(|s| deletable(s).ok()).collect();
    Ok(OriginalsSummary {
        count: sizes.len(),
        bytes: sizes.iter().sum(),
        trash_supported: crate::cull::trash_supported(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteOriginalsReport {
    deleted: Vec<i64>,
    bytes: u64,
    failed: Vec<String>,
    to_trash: bool,
}

/// Löscht die gesicherten Originale der Fotos `ids` (Papierkorb oder
/// endgültig). Danach lässt sich die Übernahme nicht mehr rückgängig machen.
#[tauri::command]
pub async fn delete_originals(
    app: AppHandle,
    ids: Vec<i64>,
    to_trash: bool,
) -> Result<DeleteOriginalsReport, Error> {
    if to_trash && !crate::cull::trash_supported() {
        return Err(Error::Invalid(
            tr_s(
                "Auf diesem System gibt es keinen Papierkorb.",
                "There is no trash on this system.",
            )
            .into(),
        ));
    }
    let emitter = app.clone();
    let report = crate::blocking(app, move |state| {
        let saved = state.catalog.locked().saved_originals(&ids)?;
        let mut deleted = Vec::new();
        let mut bytes = 0;
        let mut failed = Vec::new();
        let mut dirs = std::collections::BTreeSet::new();
        for s in &saved {
            let backup = Path::new(&s.backup);
            let removed = match deletable(s) {
                Ok(size) => crate::cull::remove(backup, to_trash).map(|()| size),
                // Schon weg (von außen gelöscht): nur aus dem Katalog nehmen
                Err(_) if crate::cull::already_gone(backup) => Ok(0),
                Err(e) => Err(e),
            };
            match removed {
                Ok(size) => {
                    if let Err(e) = state.catalog.locked().forget_original(s.image_id) {
                        failed.push(format!("{}: {e}", backup.display()));
                        continue;
                    }
                    deleted.push(s.image_id);
                    bytes += size;
                    if let Some(d) = backup.parent() {
                        dirs.insert(d.to_path_buf());
                    }
                }
                Err(e) => {
                    log::warn!("{} nicht gelöscht: {e}", backup.display());
                    failed.push(format!("{}: {e}", backup.display()));
                }
            }
        }
        for d in &dirs {
            lumenkorn_core::scanner::remove_original_dir_if_empty(d);
        }
        log::info!(
            "{} gesicherte Originale gelöscht ({}, {} MB), {} Fehler",
            deleted.len(),
            if to_trash { "Papierkorb" } else { "endgültig" },
            bytes / 1_000_000,
            failed.len()
        );
        Ok(DeleteOriginalsReport {
            deleted,
            bytes,
            failed,
            to_trash,
        })
    })
    .await?;
    let _ = emitter.emit("catalog-updated", ());
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(path: &str, kind: ImageKind, edited: bool, missing: bool) -> BakeCandidate {
        BakeCandidate {
            id: 1,
            path: path.into(),
            kind,
            missing,
            edited,
            saved: false,
        }
    }

    #[test]
    fn only_edited_raster_files_in_writable_formats() {
        let ok = |p| bake_format(&candidate(p, ImageKind::Raster, true, false));
        assert_eq!(ok("/a/b.JPG"), Ok(ExportFormat::Jpeg));
        assert_eq!(ok("/a/b.jpeg"), Ok(ExportFormat::Jpeg));
        assert_eq!(ok("/a/b.tif"), Ok(ExportFormat::Tiff16));
        assert_eq!(ok("/a/b.PNG"), Ok(ExportFormat::Png));
        assert_eq!(ok("/a/b.webp"), Err(Skip::Unsupported));
        assert_eq!(ok("/a/b.exr"), Err(Skip::Unsupported));
        assert_eq!(
            bake_format(&candidate("/a/b.dng", ImageKind::Raw, true, false)),
            Err(Skip::Raw)
        );
        assert_eq!(
            bake_format(&candidate("/a/b.jpg", ImageKind::Raster, false, false)),
            Err(Skip::Unedited)
        );
        assert_eq!(
            bake_format(&candidate("/a/b.jpg", ImageKind::Raster, true, true)),
            Err(Skip::Missing)
        );
        let s = summarize(&[
            candidate("/a/1.jpg", ImageKind::Raster, true, false),
            candidate("/a/2.cr3", ImageKind::Raw, true, false),
            candidate("/a/3.jpg", ImageKind::Raster, false, false),
        ]);
        assert_eq!(
            (s.ready, s.raw, s.unedited, s.unsupported, s.missing),
            (1, 1, 1, 0, 0)
        );
    }

    /// Das Foto ist zu keinem Zeitpunkt weg; die Sicherung ist das alte Original
    #[test]
    fn replace_keeps_the_original_as_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let photo = tmp.path().join("IMG_1.jpg");
        let dir = tmp.path().join("Original");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(&photo, b"original").unwrap();
        let backup = free_name(&dir, &photo).unwrap();
        assert_eq!(backup, dir.join("IMG_1.jpg"));
        replace_keeping_backup(&photo, Some(&backup), |s| {
            assert!(photo.is_file(), "Foto noch da, während geschrieben wird");
            std::fs::write(s, b"neu").map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(std::fs::read(&photo).unwrap(), b"neu");
        assert_eq!(std::fs::read(&backup).unwrap(), b"original");
        // Nur Foto und Ordner: keine Zwischendateien
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 2);
        // Zweite Sicherung bekommt einen eigenen Namen
        assert_eq!(free_name(&dir, &photo).unwrap(), dir.join("IMG_1-2.jpg"));
    }

    /// Scheitert das Schreiben oder das Sichern, bleibt alles, wie es war
    #[test]
    fn failed_replace_leaves_everything_as_it_was() {
        let tmp = tempfile::tempdir().unwrap();
        let photo = tmp.path().join("IMG_1.jpg");
        let backup = tmp.path().join("IMG_1-alt.jpg");
        std::fs::write(&photo, b"original").unwrap();
        let err = replace_keeping_backup(&photo, Some(&backup), |s| {
            std::fs::write(s, b"halb").unwrap();
            Err("kaputt".into())
        });
        assert!(err.is_err());
        assert_eq!(std::fs::read(&photo).unwrap(), b"original");
        assert!(!backup.exists());
        // Sicherung gibt es schon: wird nie überschrieben, Foto bleibt
        std::fs::write(&backup, b"andere").unwrap();
        let err = replace_keeping_backup(&photo, Some(&backup), |s| {
            std::fs::write(s, b"neu").map_err(|e| e.to_string())
        });
        assert!(err.is_err());
        assert_eq!(std::fs::read(&photo).unwrap(), b"original");
        assert_eq!(std::fs::read(&backup).unwrap(), b"andere");
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 2);
    }

    /// Gelöscht wird eine Sicherung nur, wenn das Foto noch da ist und sie
    /// eine andere Datei ist
    #[test]
    fn backup_is_deletable_only_beside_its_photo() {
        let tmp = tempfile::tempdir().unwrap();
        let photo = tmp.path().join("IMG_1.jpg");
        let backup = tmp.path().join("Original").join("IMG_1.jpg");
        std::fs::create_dir(backup.parent().unwrap()).unwrap();
        std::fs::write(&photo, b"neu").unwrap();
        std::fs::write(&backup, b"original").unwrap();
        let s = |photo: &Path, backup: &Path| SavedOriginal {
            image_id: 1,
            photo: photo.to_string_lossy().into_owned(),
            backup: backup.to_string_lossy().into_owned(),
            snapshot_id: None,
        };
        assert_eq!(deletable(&s(&photo, &backup)), Ok(8));
        // Eingetragene Sicherung ist das Foto selbst
        assert!(deletable(&s(&photo, &photo)).is_err());
        // Foto fehlt: Das Original ist die einzige Datei
        std::fs::remove_file(&photo).unwrap();
        assert!(deletable(&s(&photo, &backup)).is_err());
    }
}
