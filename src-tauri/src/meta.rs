//! Metadaten bearbeiten: im Katalog und in der XMP-Begleitdatei neben dem
//! Foto (die Bilddatei selbst bleibt unberührt). Lumenkorn legt Dateien in
//! Fotoordnern nur auf Wunsch des Nutzers an – dieser Befehl ist ein solcher.

use crate::AppState;
use lumenkorn_core::LockExt;
use lumenkorn_core::imagemeta::{Edit, ImageMeta, MetaPatch};
use lumenkorn_core::{Error, timeshift, xmp};
use lumenkorn_develop::tr;
use serde::Serialize;
use std::path::Path;
use tauri::{AppHandle, State};

type CmdResult<T> = std::result::Result<T, Error>;

/// Was an den Fotos geändert werden soll
#[allow(clippy::large_enum_variant)]
pub enum Change {
    Patch(MetaPatch),
    /// Aufnahmezeit um so viele Sekunden verschieben
    Shift(i64),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaFailure {
    pub id: i64,
    pub path: String,
    pub error: String,
}

/// Ergebnis: geänderte Fotos und die, bei denen es nicht ging
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaReport {
    pub changed: Vec<i64>,
    pub failed: Vec<MetaFailure>,
}

/// Begleitdatei schreiben, dann Katalog (alle Fotos in einer Transaktion).
/// Schlägt die Begleitdatei fehl, bleibt der Katalog dieses Fotos
/// unverändert, die übrigen Fotos laufen weiter.
pub fn edit_metadata(state: &AppState, ids: &[i64], change: Change) -> CmdResult<MetaReport> {
    let change = match change {
        Change::Patch(p) => Change::Patch(p.normalized()?),
        c => c,
    };
    let targets = state.catalog.locked().meta_targets(ids)?;
    let mut items = Vec::new();
    let mut failed = Vec::new();
    // Dateizugriffe ohne Katalogsperre (Netzlaufwerke können dauern)
    for t in targets {
        let patch = match &change {
            Change::Patch(p) => p.clone(),
            Change::Shift(by) => {
                let shifted = t
                    .captured_at
                    .as_deref()
                    .and_then(|c| timeshift::shift_iso(c, *by));
                match shifted {
                    Some(s) => MetaPatch {
                        captured_at: Edit::Set(s),
                        ..Default::default()
                    },
                    None => {
                        failed.push(MetaFailure {
                            id: t.id,
                            path: t.path,
                            error: tr!(
                                "Keine gültige Aufnahmezeit zum Verschieben",
                                "No valid capture time to shift"
                            ),
                        });
                        continue;
                    }
                }
            }
        };
        match xmp::update_sidecar(Path::new(&t.path), &patch.xmp_changes()) {
            Ok(_) => items.push((t.id, patch)),
            Err(e) => {
                log::warn!("Metadaten von {}: {e}", t.path);
                failed.push(MetaFailure {
                    id: t.id,
                    path: t.path,
                    error: e.to_string(),
                });
            }
        }
    }
    state.catalog.locked().apply_meta(&items)?;
    Ok(MetaReport {
        changed: items.into_iter().map(|(id, _)| id).collect(),
        failed,
    })
}

/// Metadaten mehrerer Fotos (für das Bearbeiten einer Auswahl)
#[tauri::command(async)]
pub fn meta_get_many(state: State<AppState>, ids: Vec<i64>) -> CmdResult<Vec<ImageMeta>> {
    let cat = state.catalog.locked();
    ids.iter().map(|&id| cat.image_meta(id)).collect()
}

/// Metadaten mehrerer Fotos ändern (siehe [`MetaPatch`]).
#[tauri::command]
pub async fn meta_set(app: AppHandle, ids: Vec<i64>, patch: MetaPatch) -> CmdResult<MetaReport> {
    crate::blocking(app, move |state| {
        edit_metadata(state, &ids, Change::Patch(patch))
    })
    .await
}

/// Aufnahmezeit mehrerer Fotos um `seconds` verschieben (negativ: zurück).
#[tauri::command]
pub async fn meta_shift_time(app: AppHandle, ids: Vec<i64>, seconds: i64) -> CmdResult<MetaReport> {
    crate::blocking(app, move |state| {
        edit_metadata(state, &ids, Change::Shift(seconds))
    })
    .await
}
