//! Bearbeitung ins Original übernehmen, Original wiederherstellen und
//! gesicherte Originale löschen.
//!
//! Beim Übernehmen wird die Datei des Fotos durch die entwickelte Fassung
//! ersetzt. Das bisherige Original bleibt im Ordner „Original“ daneben
//! (`scanner::original_dir_for`): als harter Link auf dieselbe Datei, wo das
//! nicht geht als Kopie. Dann wird der Katalog umgestellt und erst danach
//! ersetzt ein einziges Umbenennen die Datei des Fotos: Sie ist zu keinem
//! Zeitpunkt weg, und bricht das Programm mittendrin ab, wirkt die Bearbeitung
//! nie doppelt (sie steht dann im Schnappschuss). Nur JPEG, TIFF und PNG,
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

/// Übernehmen, Wiederherstellen und Löschen gesicherter Originale schließen
/// einander und den Export aus (dieselbe Markierung wie der Export: beide
/// entwickeln in voller Auflösung). Liefen Übernehmen und Wiederherstellen
/// gleichzeitig, könnte das eine die Sicherung löschen, während das andere die
/// Datei ohne neue Sicherung ersetzt.
fn claim_files(app: &AppHandle) -> Result<crate::task::Claim, Error> {
    crate::task::claim(app, |t| &t.export)
}

/// Übernimmt die Bearbeitung der Fotos `ids` in ihre Dateien, im Hintergrund.
/// Fortschritt per `bake-progress`. Fotos, die nicht gehen (RAW, unbearbeitet
/// …), werden ausgelassen.
#[tauri::command(async)]
pub fn bake_originals(app: AppHandle, ids: Vec<i64>) -> Result<(), Error> {
    claim_files(&app)?
        .full_res()?
        .spawn("bake", move |app, _claim| {
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
                let res = crate::task::catch_panic(|| bake_one(&state, *id, snapshot));
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
        })
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
        .is_some_and(|s| backup_of(state, s).is_file());
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
    // 1. Die neue Fassung in eine Zwischendatei neben dem Foto
    let staging = lumenkorn_core::scanner::bake_staging_path(&photo);
    if let Err(e) = output::write(&img, &staging, &opts, Some(&meta)) {
        let _ = std::fs::remove_file(&staging);
        return Err(e.to_string());
    }
    drop(img);
    let discard = |backup: Option<&Path>| {
        let _ = std::fs::remove_file(&staging);
        if let Some(b) = backup {
            let _ = std::fs::remove_file(b);
        }
    };
    // 2. Das bisherige Original sichern (eine vorhandene Datei nie ersetzen)
    if let Some(b) = &backup {
        let saved = if b.exists() {
            Err(tr!("{} gibt es schon", "{} already exists", b.display()))
        } else {
            link_or_copy(&photo, b)
        };
        if let Err(e) = saved {
            discard(None);
            return Err(tr!(
                "Original nicht gesichert: {e}",
                "Original not saved: {e}"
            ));
        }
    }
    // 3. Katalog umstellen, bevor die Datei ersetzt wird
    let backup_str = backup.as_ref().map(|b| b.to_string_lossy().into_owned());
    let done = state
        .catalog
        .locked()
        .bake_done(id, backup_str.as_deref(), snapshot);
    let snapshot_id = match done {
        Ok(s) => s,
        Err(err) => {
            discard(backup.as_deref());
            return Err(err.to_string());
        }
    };
    // 4. Ein Umbenennen ersetzt das Foto
    if let Err(e) = std::fs::rename(&staging, &photo) {
        let undone = state
            .catalog
            .locked()
            .bake_undo(id, snapshot_id, backup.is_some());
        if let Err(u) = undone {
            log::error!("Übernahme von {id} nicht zurückgenommen: {u}");
        }
        discard(backup.as_deref());
        return Err(e.to_string());
    }
    refresh(state, id, &photo);
    Ok(true)
}

/// Wo das gesicherte Original jetzt liegt. Steht es nicht mehr unter dem
/// eingetragenen Pfad, weil der Ordner umbenannt oder verschoben wurde (das Foto
/// führt der Katalog dann schon unter dem neuen Pfad), wird es im Ordner gleichen
/// Namens neben dem Foto gesucht und der Eintrag nachgeführt.
fn backup_of(state: &AppState, s: &SavedOriginal) -> PathBuf {
    let stored = PathBuf::from(&s.backup);
    if stored.is_file() {
        return stored;
    }
    let (Some(dir), Some(file), Some(parent)) = (
        stored.parent().and_then(|d| d.file_name()),
        stored.file_name(),
        Path::new(&s.photo).parent(),
    ) else {
        return stored;
    };
    let here = parent.join(dir);
    let found = here.join(file);
    if !(lumenkorn_core::scanner::is_original_dir(&here) && found.is_file()) {
        return stored;
    }
    if let Err(e) = state
        .catalog
        .locked()
        .set_original_path(s.image_id, &found.to_string_lossy())
    {
        log::warn!(
            "Pfad des Originals von {} nicht nachgeführt: {e}",
            s.image_id
        );
    }
    found
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

/// Ersetzt `photo` durch das, was `write` in eine Zwischendatei daneben schreibt:
/// ein einziges Umbenennen, das Foto ist also zu keinem Zeitpunkt weg. Scheitert
/// ein Schritt, bleibt es unverändert.
fn replace_file(
    photo: &Path,
    write: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let staging = lumenkorn_core::scanner::bake_staging_path(photo);
    let res =
        write(&staging).and_then(|()| std::fs::rename(&staging, photo).map_err(|e| e.to_string()));
    if res.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    res
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
    let running = claim_files(&app)?;
    let emitter = app.clone();
    let report = crate::blocking(app, move |state| {
        let _running = running;
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
    let backup = &backup_of(state, s);
    let photo = Path::new(&s.photo);
    if !backup.is_file() {
        return Err(tr!(
            "Gesichertes Original fehlt: {}",
            "Saved original is missing: {}",
            backup.display()
        ));
    }
    let _critical = crate::exit::critical();
    replace_file(photo, |staging| link_or_copy(backup, staging))?;
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
fn deletable(backup: &Path, photo: &Path) -> Result<u64, String> {
    let meta = std::fs::metadata(backup).map_err(|e| e.to_string())?;
    if !photo.is_file() {
        return Err(tr_s(
            "Das Foto selbst fehlt – das Original ist die einzige Datei.",
            "The photo itself is missing – the original is the only file.",
        )
        .into());
    }
    if same_file::is_same_file(backup, photo).unwrap_or(true) {
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
    let sizes: Vec<u64> = saved
        .iter()
        .filter_map(|s| deletable(&backup_of(&state, s), Path::new(&s.photo)).ok())
        .collect();
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
    let running = claim_files(&app)?;
    let emitter = app.clone();
    let report = crate::blocking(app, move |state| {
        let _running = running;
        let saved = state.catalog.locked().saved_originals(&ids)?;
        let mut deleted = Vec::new();
        let mut bytes = 0;
        let mut failed = Vec::new();
        let mut dirs = std::collections::BTreeSet::new();
        for s in &saved {
            let backup = &backup_of(state, s);
            let removed = match deletable(backup, Path::new(&s.photo)) {
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
    fn replace_keeps_the_photo_until_the_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let photo = tmp.path().join("IMG_1.jpg");
        let dir = tmp.path().join("Original");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(&photo, b"original").unwrap();
        let backup = free_name(&dir, &photo).unwrap();
        assert_eq!(backup, dir.join("IMG_1.jpg"));
        link_or_copy(&photo, &backup).unwrap();
        replace_file(&photo, |s| {
            assert!(photo.is_file(), "Foto noch da, während geschrieben wird");
            assert_eq!(s.extension().unwrap(), "tmp", "Zwischendatei ist kein Foto");
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

    /// Scheitert das Schreiben, bleibt alles, wie es war
    #[test]
    fn failed_replace_leaves_everything_as_it_was() {
        let tmp = tempfile::tempdir().unwrap();
        let photo = tmp.path().join("IMG_1.jpg");
        std::fs::write(&photo, b"original").unwrap();
        let err = replace_file(&photo, |s| {
            std::fs::write(s, b"halb").unwrap();
            Err("kaputt".into())
        });
        assert!(err.is_err());
        assert_eq!(std::fs::read(&photo).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
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
        assert_eq!(deletable(&backup, &photo), Ok(8));
        // Eingetragene Sicherung ist das Foto selbst
        assert!(deletable(&photo, &photo).is_err());
        // Foto fehlt: Das Original ist die einzige Datei
        std::fs::remove_file(&photo).unwrap();
        assert!(deletable(&backup, &photo).is_err());
    }
}
