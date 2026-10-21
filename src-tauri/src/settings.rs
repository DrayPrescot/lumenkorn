//! Allgemeine Einstellungen der App (`einstellungen.json` im Datenordner).
//! KI, Import und MCP haben eigene Dateien; hier steht, was keinem Bereich
//! allein gehört – derzeit, ob Karte und Ortsnamen OpenStreetMap nutzen.

use crate::AppState;
use lumenkorn_core::Error;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, State};

const FILE: &str = "einstellungen.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    /// Karte, Ortssuche und Ortsnamen über OpenStreetMap. Aus: Es geht nichts
    /// an OpenStreetMap; GPS aus den Fotos und vorhandene Ortsnamen bleiben.
    pub map_enabled: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self { map_enabled: true }
    }
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// Beim Start: Einstellungen lesen und wirksam machen.
pub fn load(data_dir: &Path) -> AppSettings {
    let s: AppSettings = std::fs::read(path(data_dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    crate::geo::set_enabled(s.map_enabled);
    s
}

#[tauri::command]
pub fn app_settings(state: State<AppState>) -> AppSettings {
    load(&state.data_dir)
}

#[tauri::command]
pub fn app_set_settings(
    app: AppHandle,
    state: State<AppState>,
    settings: AppSettings,
) -> Result<(), Error> {
    std::fs::write(
        path(&state.data_dir),
        serde_json::to_string_pretty(&settings)?,
    )?;
    crate::geo::set_enabled(settings.map_enabled);
    if settings.map_enabled {
        // Ausstehende Ortsnamen jetzt nachholen
        crate::geo::kick(&app);
    }
    Ok(())
}
