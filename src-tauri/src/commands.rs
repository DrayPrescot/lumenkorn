//! Befehle, die das Frontend per `invoke` aufruft.

use crate::AppState;
use crate::worker::Job;
use lumenkorn_core::LockExt;
use lumenkorn_core::*;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::State;

type CmdResult<T> = std::result::Result<T, Error>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: &'static str,
    platform: &'static str,
    data_dir: String,
    /// Gesetzt, wenn der Katalog von einer neueren Version stammt (Meldung)
    catalog_too_new: Option<String>,
}

#[tauri::command]
pub fn app_info(app: tauri::AppHandle) -> AppInfo {
    use tauri::Manager;
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        data_dir: app
            .try_state::<AppState>()
            .map(|s| s.data_dir.display().to_string())
            .unwrap_or_default(),
        catalog_too_new: app.try_state::<crate::CatalogTooNew>().map(|c| c.0.clone()),
    }
}

/// Beendet die App (Hinweis „Katalog von neuerer Version“).
#[tauri::command]
pub fn app_exit(app: tauri::AppHandle) {
    app.exit(0);
}

/// Öffnet eine Webadresse im Standardbrowser (Dialog „Über Lumenkorn“:
/// Quelltext, Lizenzen). Nur https-Adressen ohne Leer- und Steuerzeichen.
#[tauri::command]
pub fn open_url(url: String) -> CmdResult<()> {
    let ok = url.starts_with("https://")
        && url.len() < 1024
        && !url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"');
    if !ok {
        return Err(Error::Invalid(url));
    }
    #[cfg(windows)]
    std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", &url])
        .spawn()?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(&url).spawn()?;
    #[cfg(not(any(windows, target_os = "macos")))]
    std::process::Command::new("xdg-open").arg(&url).spawn()?;
    Ok(())
}

/// Sprache für Meldungen aus dem Hintergrund (Fehler, Hinweise): „de“ oder „en“.
/// Wird gemerkt, damit schon Meldungen beim nächsten Start (Absturzhinweis,
/// Grafikkarte) in dieser Sprache entstehen.
#[tauri::command]
pub fn set_language(state: State<AppState>, lang: String) {
    let en = lang == "en";
    lumenkorn_develop::i18n::set_english(en);
    let _ = std::fs::write(
        crate::language_file(&state.data_dir),
        if en { "en" } else { "de" },
    );
}

// --- Quellordner -----------------------------------------------------------

#[tauri::command(async)]
pub fn list_roots(state: State<AppState>) -> CmdResult<Vec<Root>> {
    state.catalog.locked().roots()
}

#[tauri::command(async)]
pub fn add_root(state: State<AppState>, path: String) -> CmdResult<Root> {
    let path = PathBuf::from(path);
    let root = state.catalog.locked().add_root(&path)?;
    if let Some(w) = state.watcher.locked().as_mut()
        && let Err(e) = w.watch(Path::new(&root.path))
    {
        log::warn!("Kann {} nicht überwachen: {e}", root.path);
    }
    state.worker.submit(Job::Scan(root.id));
    Ok(root)
}

/// Entfernt den Ordner nur aus dem Katalog – Dateien bleiben unangetastet.
#[tauri::command(async)]
pub fn remove_root(state: State<AppState>, id: i64) -> CmdResult<()> {
    let root = state.catalog.locked().root(id)?;
    // In Portionen, die Katalogsperre ist dazwischen frei: Ein Ordner mit
    // Zehntausenden Fotos legt die Oberfläche sonst lange still
    lumenkorn_core::catalog::Catalog::remove_root_chunked(&state.catalog, id)?;
    crate::ai::forget_in_index(&state, None);
    if let Some(w) = state.watcher.locked().as_mut() {
        w.unwatch(Path::new(&root.path));
    }
    Ok(())
}

#[tauri::command]
pub fn rescan_root(state: State<AppState>, id: i64) {
    state.worker.submit(Job::Rescan(id));
}

#[tauri::command(async)]
pub fn folder_tree(state: State<AppState>, root_id: i64) -> CmdResult<FolderNode> {
    state.catalog.locked().folder_tree(root_id)
}

// --- Bilder -----------------------------------------------------------------

#[tauri::command(async)]
pub fn query_images(state: State<AppState>, query: ImageQuery) -> CmdResult<Vec<ImageSummary>> {
    state.catalog.locked().query_images(&query)
}

#[tauri::command(async)]
pub fn image_detail(state: State<AppState>, id: i64) -> CmdResult<ImageDetail> {
    state.catalog.locked().image(id)
}

#[tauri::command(async)]
pub fn set_rating(state: State<AppState>, ids: Vec<i64>, rating: i64) -> CmdResult<()> {
    state.catalog.locked().set_rating(&ids, rating)
}

#[tauri::command(async)]
pub fn set_flag(state: State<AppState>, ids: Vec<i64>, flag: i64) -> CmdResult<()> {
    state.catalog.locked().set_flag(&ids, flag)
}

#[tauri::command(async)]
pub fn add_tag(state: State<AppState>, image_id: i64, name: String) -> CmdResult<Vec<ImageTag>> {
    let cat = state.catalog.locked();
    cat.add_tag(image_id, &name, TagSource::Manual, None)?;
    cat.image_tags(image_id)
}

#[tauri::command(async)]
pub fn remove_tag(state: State<AppState>, image_id: i64, name: String) -> CmdResult<Vec<ImageTag>> {
    let cat = state.catalog.locked();
    cat.remove_tag(image_id, &name)?;
    cat.image_tags(image_id)
}

// --- Projekte ---------------------------------------------------------------

#[tauri::command(async)]
pub fn list_projects(state: State<AppState>) -> CmdResult<Vec<Project>> {
    state.catalog.locked().projects()
}

#[tauri::command(async)]
pub fn create_project(
    state: State<AppState>,
    name: String,
    description: String,
) -> CmdResult<Project> {
    state.catalog.locked().create_project(&name, &description)
}

#[tauri::command(async)]
pub fn update_project(
    state: State<AppState>,
    id: i64,
    name: String,
    description: String,
) -> CmdResult<Project> {
    state
        .catalog
        .locked()
        .update_project(id, &name, &description)
}

#[tauri::command(async)]
pub fn delete_project(state: State<AppState>, id: i64) -> CmdResult<()> {
    state.catalog.locked().delete_project(id)
}

#[tauri::command(async)]
pub fn add_project_folder(
    state: State<AppState>,
    project_id: i64,
    path: String,
    recursive: bool,
) -> CmdResult<Project> {
    state
        .catalog
        .locked()
        .add_project_folder(project_id, Path::new(&path), recursive)
}

#[tauri::command(async)]
pub fn remove_project_folder(
    state: State<AppState>,
    project_id: i64,
    path: String,
) -> CmdResult<Project> {
    state
        .catalog
        .locked()
        .remove_project_folder(project_id, &path)
}
