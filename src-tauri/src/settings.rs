//! Allgemeine Einstellungen der App (`einstellungen.json` im Datenordner).
//! KI, Import und MCP haben eigene Dateien; hier steht, was keinem Bereich
//! allein gehört: ob Karte und Ortsnamen OpenStreetMap nutzen, und der
//! Standard-Unterordner für den Export.

use crate::AppState;
use lumenkorn_core::{Error, LockExt};
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
    /// Exportziel ohne gewählten Ordner: relativ zum Ordner des Originals
    /// (`Export`, `Export/JPEG`, `../Exporte`, `.`)
    pub export_subfolder: String,
    /// Unterordner in die Bibliothek aufnehmen – Standard für Ordner ohne
    /// eigene Einstellung (Rechtsklick im Ordnerbaum)
    pub include_subfolders: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            map_enabled: true,
            export_subfolder: lumenkorn_core::scanner::DEFAULT_EXPORT_SUBFOLDER.into(),
            include_subfolders: true,
        }
    }
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// Einstellungen lesen (ungültiger Export-Unterordner: Standard)
pub fn read(data_dir: &Path) -> AppSettings {
    let mut s: AppSettings = std::fs::read(path(data_dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    s.export_subfolder = lumenkorn_core::scanner::check_export_subfolder(&s.export_subfolder)
        .unwrap_or_else(|_| lumenkorn_core::scanner::DEFAULT_EXPORT_SUBFOLDER.into());
    s
}

/// „Unterordner einbeziehen“ an den Scanner geben: Standard aus den
/// Einstellungen, Abweichungen je Ordner aus dem Katalog.
pub fn apply_folder_rules(data_dir: &Path, catalog: &lumenkorn_core::Catalog) {
    let folders = catalog.folder_options().unwrap_or_else(|e| {
        log::warn!("Ordner-Einstellungen nicht lesbar: {e}");
        Vec::new()
    });
    lumenkorn_core::scanner::set_subfolder_rules(read(data_dir).include_subfolders, &folders);
}

/// Beim Start: Einstellungen lesen und wirksam machen.
pub fn load(data_dir: &Path) -> AppSettings {
    let s = read(data_dir);
    crate::geo::set_enabled(s.map_enabled);
    s
}

#[tauri::command]
pub fn app_settings(state: State<AppState>) -> AppSettings {
    read(&state.data_dir)
}

/// Ändert die übergebenen Einstellungen; nicht genannte bleiben, wie sie sind.
/// Liefert den neuen Stand (Export-Unterordner bereinigt).
#[tauri::command]
pub fn app_set_settings(
    app: AppHandle,
    state: State<AppState>,
    settings: serde_json::Value,
) -> Result<AppSettings, Error> {
    let mut cur = serde_json::to_value(read(&state.data_dir))?;
    if let (Some(c), Some(p)) = (cur.as_object_mut(), settings.as_object()) {
        for (k, v) in p {
            c.insert(k.clone(), v.clone());
        }
    }
    let before = read(&state.data_dir).include_subfolders;
    let mut settings: AppSettings = serde_json::from_value(cur)?;
    settings.export_subfolder =
        lumenkorn_core::scanner::check_export_subfolder(&settings.export_subfolder)
            .map_err(Error::Invalid)?;
    std::fs::write(
        path(&state.data_dir),
        serde_json::to_string_pretty(&settings)?,
    )?;
    crate::geo::set_enabled(settings.map_enabled);
    if settings.include_subfolders != before {
        // Neuer Standard: alle Quellordner danach neu abgleichen
        let cat = state.catalog.locked();
        apply_folder_rules(&state.data_dir, &cat);
        for r in cat.roots()? {
            state.worker.submit(crate::worker::Job::Rescan(r.id));
        }
    }
    if settings.map_enabled {
        // Ausstehende Ortsnamen jetzt nachholen
        crate::geo::kick(&app);
    }
    Ok(settings)
}
