//! Import von einer Speicherkarte: einlesen, Vorschauen, kopieren mit
//! Umbenennen (siehe `lumenkorn_core::import`).

use crate::worker::Job;
use crate::{AppState, blocking};
use lumenkorn_core::import::{self, Candidate, ImportItem, ImportReport};
use lumenkorn_core::{Error, LockExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Manager};

/// Kantenlänge der Vorschauen im Import-Dialog
const PREVIEW_EDGE: u32 = 240;
const SETTINGS_FILE: &str = "import.json";

#[derive(Default)]
pub struct ImportState {
    session: Mutex<Option<Arc<Session>>>,
    pub(crate) running: AtomicBool,
    cancel: AtomicBool,
}

/// Zuletzt eingelesene Quelle
struct Session {
    cands: Vec<Candidate>,
    counters: Vec<String>,
    previews: Mutex<HashMap<usize, Arc<Vec<u8>>>>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ImportSettings {
    pub source: Option<String>,
    pub target: Option<String>,
    #[serde(default)]
    pub delete_after: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDefaults {
    settings: ImportSettings,
    /// Erkannte Speicherkarten (Ordner `DCIM` auf Wechseldatenträgern)
    cards: Vec<String>,
}

fn settings_path(state: &AppState) -> PathBuf {
    state.data_dir.join(SETTINGS_FILE)
}

fn load_settings(state: &AppState) -> ImportSettings {
    std::fs::read_to_string(settings_path(state))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Zuletzt benutzte Quelle und Ziel (Ziel ersatzweise der erste
/// Quellordner) und eingesteckte Speicherkarten.
#[tauri::command]
pub async fn import_defaults(app: AppHandle) -> Result<ImportDefaults, Error> {
    blocking(app, |state| {
        let mut settings = load_settings(state);
        if settings.target.is_none() {
            settings.target = state
                .catalog
                .locked()
                .roots()?
                .first()
                .map(|r| r.path.clone());
        }
        Ok(ImportDefaults {
            settings,
            cards: card_folders(),
        })
    })
    .await
}

/// Ordner `DCIM` auf Wechseldatenträgern (Windows: Laufwerkstyp „removable“ –
/// Netzlaufwerke werden nicht angefasst, `exists()` könnte dort hängen).
#[cfg(windows)]
fn card_folders() -> Vec<String> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLogicalDrives() -> u32;
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    const DRIVE_REMOVABLE: u32 = 2;
    // SAFETY: Keine Zeiger; liefert eine Bitmaske der Laufwerke.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .filter_map(|i| {
            let root = format!("{}:\\", (b'A' + i) as char);
            let wide: Vec<u16> = root.encode_utf16().chain([0]).collect();
            // SAFETY: nullterminierte UTF-16-Zeichenkette, gültig während des Aufrufs
            let removable = unsafe { GetDriveTypeW(wide.as_ptr()) } == DRIVE_REMOVABLE;
            let dcim = Path::new(&root).join("DCIM");
            (removable && dcim.is_dir()).then(|| dcim.display().to_string())
        })
        .collect()
}

/// Ordner `DCIM` unter den Einhängepunkten für Wechselmedien.
#[cfg(not(windows))]
fn card_folders() -> Vec<String> {
    let user = std::env::var("USER").unwrap_or_default();
    let bases = [
        PathBuf::from("/media").join(&user),
        PathBuf::from("/run/media").join(&user),
        PathBuf::from("/Volumes"),
    ];
    bases
        .iter()
        .filter_map(|b| std::fs::read_dir(b).ok())
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path().join("DCIM"))
        .filter(|p| p.is_dir())
        .map(|p| p.display().to_string())
        .collect()
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ScanProgress {
    found: usize,
}

/// Liest die Aufnahmen unter `source` ein (nach Aufnahmezeit sortiert).
#[tauri::command]
pub async fn import_scan(app: AppHandle, source: String) -> Result<Vec<ImportItem>, Error> {
    let events = app.clone();
    blocking(app, move |state| {
        let source = PathBuf::from(source);
        let last = std::sync::atomic::AtomicUsize::new(0);
        let cands = import::scan(&source, &state.catalog, &|n| {
            // Höchstens alle 25 Aufnahmen melden
            if n / 25 != last.swap(n, Ordering::Relaxed) / 25 {
                let _ = events.emit("import-scan-progress", ScanProgress { found: n });
            }
        })?;
        let counters = import::counters(&cands);
        let items = cands
            .iter()
            .zip(&counters)
            .map(|(c, n)| c.item(n))
            .collect();
        *state.import.session.lock().unwrap() = Some(Arc::new(Session {
            cands,
            counters,
            previews: Mutex::default(),
        }));
        Ok(items)
    })
    .await
}

fn session(state: &AppState) -> Result<Arc<Session>, Error> {
    state.import.session.lock().unwrap().clone().ok_or_else(|| {
        Error::Invalid(
            lumenkorn_develop::i18n::tr_s("Keine Quelle eingelesen", "No source has been read")
                .into(),
        )
    })
}

/// Vorschau einer eingelesenen Aufnahme (JPEG)
#[tauri::command]
pub async fn import_preview(app: AppHandle, id: usize) -> Result<Response, Error> {
    blocking(app, move |state| {
        let s = session(state)?;
        if let Some(b) = s.previews.lock().unwrap().get(&id) {
            return Ok(Response::new(b.as_ref().clone()));
        }
        let c = s
            .cands
            .get(id)
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        let bytes = Arc::new(lumenkorn_core::thumbs::file_preview(
            c.main(),
            c.kind,
            c.orientation,
            PREVIEW_EDGE,
        )?);
        s.previews.lock().unwrap().insert(id, bytes.clone());
        Ok(Response::new(bytes.as_ref().clone()))
    })
    .await
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ImportDone {
    report: Option<ImportReport>,
    error: Option<String>,
    /// Das Ziel lag in keinem Quellordner und wurde als neuer aufgenommen
    root_added: bool,
    /// Das Ziel liegt außerhalb der Bibliothek und ließ sich nicht aufnehmen
    outside_library: bool,
}

/// Importiert die Aufnahmen `ids` nach `target` (siehe `lumenkorn_core::import`).
#[tauri::command]
pub async fn import_start(
    app: AppHandle,
    ids: Vec<usize>,
    target: String,
    event: String,
    delete_after: bool,
) -> Result<(), Error> {
    let state = app.state::<AppState>();
    let s = session(&state)?;
    let target = PathBuf::from(target.trim());
    if target.as_os_str().is_empty() || !target.is_absolute() {
        return Err(Error::Invalid(
            lumenkorn_develop::i18n::tr_s(
                "Bitte einen Zielordner wählen.",
                "Please choose a target folder.",
            )
            .into(),
        ));
    }
    let Some(running) = crate::busy::claim_in(&app, |s| &s.import.running) else {
        return Err(Error::Invalid(
            lumenkorn_develop::i18n::tr_s(
                "Es läuft bereits ein Import.",
                "An import is already running.",
            )
            .into(),
        ));
    };
    state.import.cancel.store(false, Ordering::SeqCst);
    // Einstellungen für das nächste Mal
    let mut settings = load_settings(&state);
    settings.source = s
        .cands
        .first()
        .and_then(|c| c.main().parent())
        .map(|p| p.display().to_string())
        .or(settings.source);
    settings.target = Some(target.display().to_string());
    settings.delete_after = delete_after;
    if let Ok(json) = serde_json::to_string_pretty(&settings) {
        let _ = std::fs::write(settings_path(&state), json);
    }
    let app2 = app.clone();
    std::thread::Builder::new()
        .name("lumenkorn-import".into())
        .spawn(move || {
            let _running = running;
            let state = app2.state::<AppState>();
            let picked: Vec<(&Candidate, String)> = ids
                .iter()
                .filter_map(|&i| Some((s.cands.get(i)?, s.counters.get(i)?.clone())))
                .collect();
            let last = Mutex::new(std::time::Instant::now());
            let report = import::run(
                &picked,
                &target,
                &event,
                delete_after,
                &state.import.cancel,
                &|p| {
                    // Fortschritt höchstens alle 100 ms (und am Ende jeder Aufnahme)
                    let mut l = last.lock().unwrap();
                    if l.elapsed().as_millis() >= 100 || p.done == p.total {
                        *l = std::time::Instant::now();
                        let _ = app2.emit("import-progress", p.clone());
                    }
                },
            );
            log::info!(
                "Import: {} Aufnahmen ({} Dateien, {} MB) nach {}, {} schon vorhanden, {} gelöscht, {} Fehler",
                report.imported,
                report.files,
                report.bytes / 1_000_000,
                target.display(),
                report.existing,
                report.deleted,
                report.failed.len()
            );
            let (root_added, outside_library) = register(&state, &target, &report.dirs);
            // Vorschauen der Karte nicht weiter vorhalten
            *state.import.session.lock().unwrap() = None;
            let _ = app2.emit(
                "import-done",
                ImportDone {
                    report: Some(report),
                    error: None,
                    root_added,
                    outside_library,
                },
            );
        })
        .map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(())
}

/// Sorgt dafür, dass die importierten Ordner im Katalog erscheinen: im
/// vorhandenen Quellordner gezielt einlesen, sonst das Ziel als neuen
/// Quellordner aufnehmen. Liefert (neu aufgenommen, außerhalb der Bibliothek).
fn register(state: &AppState, target: &Path, dirs: &[PathBuf]) -> (bool, bool) {
    let normalized = PathBuf::from(lumenkorn_core::paths::normalize_dir(target));
    let roots = state.catalog.locked().roots().unwrap_or_default();
    if let Some(r) = roots.iter().find(|r| normalized.starts_with(&r.path)) {
        let paths = dirs
            .iter()
            .map(|d| PathBuf::from(lumenkorn_core::paths::normalize_dir(d)));
        state.worker.submit_changes(r.id, paths);
        return (false, false);
    }
    if dirs.is_empty() {
        return (false, false);
    }
    match state.catalog.locked().add_root(target) {
        Ok(root) => {
            if let Some(w) = state.watcher.lock().unwrap().as_mut()
                && let Err(e) = w.watch(Path::new(&root.path))
            {
                log::warn!("Kann {} nicht überwachen: {e}", root.path);
            }
            state.worker.submit(Job::Scan(root.id));
            (true, false)
        }
        Err(e) => {
            // Z. B. ein Ordner, der vorhandene Quellordner enthält
            log::warn!(
                "Importziel {} nicht als Quellordner aufgenommen: {e}",
                target.display()
            );
            (false, true)
        }
    }
}

#[tauri::command]
pub fn import_cancel(app: AppHandle) {
    app.state::<AppState>()
        .import
        .cancel
        .store(true, Ordering::SeqCst);
}
