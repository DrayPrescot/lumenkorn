//! Aussortieren: abgelehnte Fotos wirklich von der Festplatte löschen.
//!
//! Gelöscht wird nur, was beim Löschen noch als abgelehnt markiert ist –
//! die Prüfung passiert hier im Backend, nicht nur in der Oberfläche.
//! Standard ist der Papierkorb des Betriebssystems; endgültiges Löschen
//! muss ausdrücklich gewählt werden (auf Android gibt es keinen Papierkorb).

use crate::AppState;
use lumenkorn_core::Error;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectedSummary {
    count: usize,
    bytes: i64,
    trash_supported: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    path: String,
    error: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteReport {
    deleted: Vec<i64>,
    bytes: i64,
    failed: Vec<Failure>,
    to_trash: bool,
}

pub fn trash_supported() -> bool {
    cfg!(not(any(target_os = "android", target_os = "ios")))
}

/// Wie viele der übergebenen Bilder würden tatsächlich gelöscht?
#[tauri::command(async)]
pub fn rejected_summary(state: State<AppState>, ids: Vec<i64>) -> Result<RejectedSummary, Error> {
    let files = state.catalog.lock().unwrap().rejected_files(&ids)?;
    Ok(RejectedSummary {
        count: files.len(),
        bytes: files.iter().map(|f| f.2).sum(),
        trash_supported: trash_supported(),
    })
}

#[tauri::command]
pub async fn delete_rejected(
    app: tauri::AppHandle,
    ids: Vec<i64>,
    to_trash: bool,
) -> Result<DeleteReport, Error> {
    // Papierkorb und Dateisystem können dauern: nicht auf der async-Laufzeit
    crate::blocking(app, move |state| {
        delete_rejected_blocking(state, ids, to_trash)
    })
    .await
}

fn delete_rejected_blocking(
    state: &AppState,
    ids: Vec<i64>,
    to_trash: bool,
) -> Result<DeleteReport, Error> {
    if to_trash && !trash_supported() {
        return Err(Error::Invalid(
            lumenkorn_develop::i18n::tr_s(
                "Auf diesem System gibt es keinen Papierkorb.",
                "There is no trash on this system.",
            )
            .into(),
        ));
    }
    // Liste unter Sperre holen, Dateien ohne Sperre löschen (Papierkorb kann dauern)
    let files = state.catalog.lock().unwrap().rejected_files(&ids)?;
    let mut deleted = Vec::new();
    let mut bytes = 0;
    let mut failed = Vec::new();
    for (id, path, size) in &files {
        let p = Path::new(path);
        match remove(p, to_trash) {
            Ok(()) => {
                deleted.push(*id);
                bytes += size;
                for sidecar in sidecars(p) {
                    if let Err(e) = remove(&sidecar, to_trash) {
                        log::warn!("XMP {} nicht gelöscht: {e}", sidecar.display());
                    }
                }
            }
            // Schon weg (z. B. von außen gelöscht): trotzdem aus dem Katalog nehmen
            Err(e) if !p.exists() => {
                log::info!("{path} war bereits gelöscht ({e})");
                deleted.push(*id);
            }
            Err(e) => {
                log::warn!("{path} nicht gelöscht: {e}");
                failed.push(Failure {
                    path: path.clone(),
                    error: e,
                });
            }
        }
    }
    let faces = state.catalog.lock().unwrap().forget_images(&deleted)?;
    crate::ai::forget_in_index(state, Some(&deleted));
    cleanup(state, &deleted, &faces);
    log::info!(
        "{} abgelehnte Fotos gelöscht ({}, {} MB), {} Fehler",
        deleted.len(),
        if to_trash { "Papierkorb" } else { "endgültig" },
        bytes / 1_000_000,
        failed.len()
    );
    Ok(DeleteReport {
        deleted,
        bytes,
        failed,
        to_trash,
    })
}

fn remove(path: &Path, to_trash: bool) -> Result<(), String> {
    if to_trash {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        return trash::delete(path).map_err(|e| e.to_string());
    }
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

/// XMP-Begleitdateien eines Fotos: `foto.cr3.xmp` gehört immer dazu,
/// `foto.xmp` nur, wenn kein anderes Foto gleichen Namens daneben liegt
/// (RAW+JPEG-Paare teilen sich diese Datei).
fn sidecars(photo: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(name) = photo.file_name().and_then(|n| n.to_str()) else {
        return out;
    };
    let dir = photo.parent().unwrap_or(Path::new("."));
    let stem = photo.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let mut seen = HashSet::new();
    let entries: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    let is_xmp = |n: &str| n.to_ascii_lowercase().ends_with(".xmp");
    let sibling = entries.iter().any(|n| {
        !is_xmp(n)
            && n != name
            && Path::new(n)
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s == stem)
    });
    for n in &entries {
        if !is_xmp(n) {
            continue;
        }
        let base = &n[..n.len() - 4];
        if base == name || (!sibling && base == stem) {
            // Groß-/Kleinschreibung: dieselbe Datei nur einmal
            if seen.insert(n.to_ascii_lowercase()) {
                out.push(dir.join(n));
            }
        }
    }
    out
}

/// Zwischenspeicher und Zusatzdateien gelöschter Bilder entfernen.
/// Masken, Entrauschungen und Reparaturen tragen die Bild-ID als Präfix.
fn cleanup(state: &AppState, ids: &[i64], faces: &[i64]) {
    if ids.is_empty() {
        return;
    }
    for id in ids {
        state.cache.remove(*id);
        state.develop.forget(*id);
    }
    let prefixes: Vec<String> = ids.iter().map(|id| format!("{id}-")).collect();
    // Was ein anderes Foto noch nutzt, bleibt (ältere Versionen haben beim
    // Einfügen auch Verweise auf fremde Entrauschungen übertragen).
    // Ohne verlässliche Liste lieber nichts löschen – das tägliche Aufräumen
    // erledigt die Reste später
    let Ok(still_used) = state.catalog.lock().unwrap().live_files() else {
        log::warn!("Zusatzdateien gelöschter Fotos bleiben vorerst liegen");
        return;
    };
    for dir in crate::dirs::EDIT_FILES {
        let Ok(rd) = std::fs::read_dir(state.data_dir.join(dir)) else {
            continue;
        };
        for e in rd.flatten() {
            let n = e.file_name();
            let n = n.to_string_lossy();
            let stem = n.rsplit_once('.').map_or(&*n, |(s, _)| s);
            if prefixes.iter().any(|p| n.starts_with(p.as_str())) && !still_used.contains(stem) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let thumbs = crate::people::thumb_dir(state);
    for f in faces {
        let _ = std::fs::remove_file(thumbs.join(format!("{f}.jpg")));
    }
    if !faces.is_empty() {
        state.people.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecars_respect_raw_jpeg_pairs() {
        let dir = std::env::temp_dir().join(format!("lw-cull-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for n in [
            "a.cr3",
            "a.jpg",
            "a.xmp",
            "a.jpg.xmp",
            "b.nef",
            "b.xmp",
            "c.jpg",
        ] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let names = |p: &str| -> Vec<String> {
            let mut v: Vec<String> = sidecars(&dir.join(p))
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        // a.xmp gehört auch zu a.cr3 → bleibt
        assert_eq!(names("a.jpg"), vec!["a.jpg.xmp"]);
        assert_eq!(names("b.nef"), vec!["b.xmp"]);
        assert!(names("c.jpg").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
