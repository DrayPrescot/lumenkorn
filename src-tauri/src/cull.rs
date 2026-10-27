//! Aussortieren: abgelehnte Fotos wirklich von der Festplatte löschen.
//!
//! Gelöscht wird nur, was beim Löschen noch als abgelehnt markiert ist –
//! die Prüfung passiert hier im Backend, nicht nur in der Oberfläche.
//! Standard ist der Papierkorb des Betriebssystems; endgültiges Löschen
//! muss ausdrücklich gewählt werden (auf Android gibt es keinen Papierkorb).

use crate::AppState;
use lumenkorn_core::Error;
use lumenkorn_core::LockExt;
use serde::Serialize;
use std::path::Path;
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
    let files = state.catalog.locked().rejected_files(&ids)?;
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
    let files = state.catalog.locked().rejected_files(&ids)?;
    let mut deleted = Vec::new();
    let mut bytes = 0;
    let mut failed = Vec::new();
    for (id, path, size) in &files {
        let p = Path::new(path);
        match remove(p, to_trash) {
            Ok(()) => {
                deleted.push(*id);
                bytes += size;
                for sidecar in lumenkorn_core::xmp::sidecars(p) {
                    if let Err(e) = remove(&sidecar, to_trash) {
                        log::warn!("XMP {} nicht gelöscht: {e}", sidecar.display());
                    }
                }
            }
            // Schon weg (z. B. von außen gelöscht): trotzdem aus dem Katalog nehmen
            Err(e) if already_gone(p) => {
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
    let faces = state.catalog.locked().forget_images(&deleted)?;
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

/// Ist die Datei sicher nicht mehr da? Nur wenn ihr Ordner erreichbar ist –
/// fehlt eine Netzwerkfreigabe oder externe Platte, meldet `exists()` auch
/// `false`; das Foto darf dann nicht samt Bearbeitungen verschwinden.
fn already_gone(path: &Path) -> bool {
    let dir_there = path.parent().is_some_and(|d| d.is_dir());
    dir_there
        && std::fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}

fn remove(path: &Path, to_trash: bool) -> Result<(), String> {
    if to_trash {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        return trash::delete(path).map_err(|e| e.to_string());
    }
    std::fs::remove_file(path).map_err(|e| e.to_string())
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
    let Ok(still_used) = state.catalog.locked().live_files() else {
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
    fn missing_folder_is_not_already_gone() {
        let dir = std::env::temp_dir().join(format!("lw-gone-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(already_gone(&dir.join("weg.jpg")), "Ordner da, Datei nicht");
        std::fs::write(dir.join("da.jpg"), b"x").unwrap();
        assert!(!already_gone(&dir.join("da.jpg")));
        // Ordner nicht erreichbar (z. B. NAS getrennt)
        assert!(!already_gone(&dir.join("fehlt").join("x.jpg")));
        let _ = std::fs::remove_dir_all(&dir);
    }

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
            let mut v: Vec<String> = lumenkorn_core::xmp::sidecars(&dir.join(p))
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
