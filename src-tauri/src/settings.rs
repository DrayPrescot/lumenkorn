//! Allgemeine Einstellungen der App (`einstellungen.json` im Datenordner).
//! KI, Import und MCP haben eigene Dateien; hier steht, was keinem Bereich
//! allein gehört: ob Karte und Ortsnamen OpenStreetMap nutzen, der
//! Standard-Unterordner für den Export und der Speicher-Sparmodus.

use crate::AppState;
use lumenkorn_core::{Error, LockExt};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State};

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
    /// Höchstens so viele GB Großvorschauen auf der Platte; ältere (am
    /// längsten nicht angesehene) werden beim Aufräumen gelöscht
    pub preview_cache_gb: u32,
    /// Speicher-Sparmodus (siehe `lumenkorn_develop::set_low_memory`)
    pub memory_saver: MemorySaver,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemorySaver {
    /// An, wenn der Rechner höchstens [`LOW_MEMORY_TOTAL`] hat
    #[default]
    Auto,
    On,
    Off,
}

/// Bis zu so viel eingebautem Arbeitsspeicher schaltet „Automatisch“ den
/// Sparmodus ein. Etwas über 8 GiB, denn Windows meldet bei 8 GB oft weniger
/// (Grafik), manche Rechner aber auch knapp mehr.
const LOW_MEMORY_TOTAL: u64 = 8704 << 20;

impl MemorySaver {
    /// Gilt der Sparmodus bei `total` Bytes Arbeitsspeicher?
    fn active(self, total: Option<u64>) -> bool {
        match self {
            Self::On => true,
            Self::Off => false,
            Self::Auto => total.is_some_and(|t| t <= LOW_MEMORY_TOTAL),
        }
    }
}

fn total_memory() -> Option<u64> {
    lumenkorn_develop::merge::total_bytes().map(|b| b as u64)
}

/// Speicher-Sparmodus nach der Einstellung ein- oder ausschalten.
fn apply_memory_saver(s: &AppSettings) {
    let on = s.memory_saver.active(total_memory());
    if on != lumenkorn_develop::low_memory() {
        log::info!(
            "Speicher-Sparmodus {} ({:?}, {:.1} GB Arbeitsspeicher)",
            if on { "an" } else { "aus" },
            s.memory_saver,
            total_memory().unwrap_or(0) as f64 / (1u64 << 30) as f64
        );
    }
    lumenkorn_develop::set_low_memory(on);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStatus {
    /// Eingebauter Arbeitsspeicher (Bytes), falls ermittelbar
    total: Option<u64>,
    /// Sparmodus gerade wirksam
    low_memory: bool,
}

#[tauri::command]
pub fn memory_status() -> MemoryStatus {
    MemoryStatus {
        total: total_memory(),
        low_memory: lumenkorn_develop::low_memory(),
    }
}

/// Erlaubte Grenze für den Vorschau-Cache (GB)
const PREVIEW_CACHE_GB: std::ops::RangeInclusive<u32> = 1..=1000;

impl AppSettings {
    /// Grenze für den Vorschau-Cache in Bytes
    pub fn preview_cache_bytes(&self) -> u64 {
        u64::from(self.preview_cache_gb) << 30
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            map_enabled: true,
            export_subfolder: lumenkorn_core::scanner::DEFAULT_EXPORT_SUBFOLDER.into(),
            include_subfolders: true,
            preview_cache_gb: 5,
            memory_saver: MemorySaver::Auto,
        }
    }
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// Einstellungen lesen (ungültiger Export-Unterordner: Standard)
pub fn read(data_dir: &Path) -> AppSettings {
    let mut s: AppSettings = lumenkorn_develop::fsutil::load_json_or_default(&path(data_dir));
    s.export_subfolder = lumenkorn_core::scanner::check_export_subfolder(&s.export_subfolder)
        .unwrap_or_else(|_| lumenkorn_core::scanner::DEFAULT_EXPORT_SUBFOLDER.into());
    s.preview_cache_gb = s
        .preview_cache_gb
        .clamp(*PREVIEW_CACHE_GB.start(), *PREVIEW_CACHE_GB.end());
    s
}

/// Großvorschauen auf die eingestellte Grenze bringen.
pub fn trim_previews(state: &AppState) {
    let max = read(&state.data_dir).preview_cache_bytes();
    let s = lumenkorn_core::housekeeping::trim_previews(state.cache.base(), max);
    if s.files > 0 {
        log::info!(
            "Vorschau-Cache begrenzt: {} Vorschauen, {:.1} MB entfernt",
            s.files,
            s.bytes as f64 / 1e6
        );
    }
}

/// Belegter Platz der Großvorschauen (Bytes)
#[tauri::command(async)]
pub fn preview_cache_usage(state: State<AppState>) -> u64 {
    lumenkorn_core::housekeeping::previews_size(state.cache.base())
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
    apply_memory_saver(&s);
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
    let old = read(&state.data_dir);
    let before = old.include_subfolders;
    let mut settings: AppSettings = serde_json::from_value(cur)?;
    settings.preview_cache_gb = settings
        .preview_cache_gb
        .clamp(*PREVIEW_CACHE_GB.start(), *PREVIEW_CACHE_GB.end());
    settings.export_subfolder =
        lumenkorn_core::scanner::check_export_subfolder(&settings.export_subfolder)
            .map_err(Error::Invalid)?;
    lumenkorn_develop::fsutil::save_json(&path(&state.data_dir), &settings)?;
    crate::geo::set_enabled(settings.map_enabled);
    let was_low = lumenkorn_develop::low_memory();
    apply_memory_saver(&settings);
    if lumenkorn_develop::low_memory() && !was_low {
        // Gleich auf das kleinere Maß bringen, nicht erst nach und nach
        let app = app.clone();
        std::thread::spawn(move || crate::memory::release_develop(&app.state::<AppState>()));
    }
    if settings.include_subfolders != before {
        // Neuer Standard: alle Quellordner danach neu abgleichen
        let cat = state.catalog.locked();
        apply_folder_rules(&state.data_dir, &cat);
        for r in cat.roots()? {
            state.worker.submit(crate::worker::Job::Rescan(r.id));
        }
    }
    if settings.preview_cache_gb < old.preview_cache_gb {
        // Kleinere Grenze: gleich aufräumen, nicht erst beim nächsten Durchgang
        let app = app.clone();
        std::thread::spawn(move || trim_previews(&app.state::<AppState>()));
    }
    if settings.map_enabled {
        // Ausstehende Ortsnamen jetzt nachholen
        crate::geo::kick(&app);
    }
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_saver_threshold() {
        const GB: u64 = 1 << 30;
        // 8-GB-Rechner melden unter Windows oft etwas weniger
        assert!(MemorySaver::Auto.active(Some(8 * GB)));
        assert!(MemorySaver::Auto.active(Some(7 * GB + GB / 2)));
        assert!(!MemorySaver::Auto.active(Some(12 * GB)));
        assert!(!MemorySaver::Auto.active(None));
        assert!(MemorySaver::On.active(Some(64 * GB)));
        assert!(!MemorySaver::Off.active(Some(4 * GB)));
    }

    #[test]
    fn old_settings_file_keeps_defaults() {
        // Einstellungen aus früheren Versionen ohne das neue Feld
        let s: AppSettings = serde_json::from_str(r#"{"mapEnabled":false}"#).unwrap();
        assert_eq!(s.memory_saver, MemorySaver::Auto);
        assert!(!s.map_enabled);
    }
}
