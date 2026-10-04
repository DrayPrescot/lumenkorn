//! Befehle, die das Frontend per `invoke` aufruft.

use crate::AppState;
use crate::worker::Job;
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
}

#[tauri::command]
pub fn app_info(state: State<AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        data_dir: state.data_dir.display().to_string(),
    }
}

// --- Quellordner -----------------------------------------------------------

#[tauri::command(async)]
pub fn list_roots(state: State<AppState>) -> CmdResult<Vec<Root>> {
    state.catalog.lock().unwrap().roots()
}

#[tauri::command(async)]
pub fn add_root(state: State<AppState>, path: String) -> CmdResult<Root> {
    let path = PathBuf::from(path);
    let root = state.catalog.lock().unwrap().add_root(&path)?;
    if let Some(w) = state.watcher.lock().unwrap().as_mut()
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
    let cat = state.catalog.lock().unwrap();
    let root = cat.root(id)?;
    cat.remove_root(id)?;
    if let Some(w) = state.watcher.lock().unwrap().as_mut() {
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
    state.catalog.lock().unwrap().folder_tree(root_id)
}

// --- Bilder -----------------------------------------------------------------

#[tauri::command(async)]
pub fn query_images(state: State<AppState>, query: ImageQuery) -> CmdResult<Vec<ImageSummary>> {
    state.catalog.lock().unwrap().query_images(&query)
}

#[tauri::command(async)]
pub fn image_detail(state: State<AppState>, id: i64) -> CmdResult<ImageDetail> {
    state.catalog.lock().unwrap().image(id)
}

#[tauri::command(async)]
pub fn set_rating(state: State<AppState>, ids: Vec<i64>, rating: i64) -> CmdResult<()> {
    state.catalog.lock().unwrap().set_rating(&ids, rating)
}

#[tauri::command(async)]
pub fn set_flag(state: State<AppState>, ids: Vec<i64>, flag: i64) -> CmdResult<()> {
    state.catalog.lock().unwrap().set_flag(&ids, flag)
}

#[tauri::command(async)]
pub fn add_tag(state: State<AppState>, image_id: i64, name: String) -> CmdResult<Vec<ImageTag>> {
    let cat = state.catalog.lock().unwrap();
    cat.add_tag(image_id, &name, TagSource::Manual, None)?;
    cat.image_tags(image_id)
}

#[tauri::command(async)]
pub fn remove_tag(state: State<AppState>, image_id: i64, name: String) -> CmdResult<Vec<ImageTag>> {
    let cat = state.catalog.lock().unwrap();
    cat.remove_tag(image_id, &name)?;
    cat.image_tags(image_id)
}

// --- Projekte ---------------------------------------------------------------

#[tauri::command(async)]
pub fn list_projects(state: State<AppState>) -> CmdResult<Vec<Project>> {
    state.catalog.lock().unwrap().projects()
}

#[tauri::command(async)]
pub fn create_project(
    state: State<AppState>,
    name: String,
    description: Option<String>,
) -> CmdResult<Project> {
    state
        .catalog
        .lock()
        .unwrap()
        .create_project(&name, description.as_deref().unwrap_or(""))
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
        .lock()
        .unwrap()
        .update_project(id, &name, &description)
}

#[tauri::command(async)]
pub fn delete_project(state: State<AppState>, id: i64) -> CmdResult<()> {
    state.catalog.lock().unwrap().delete_project(id)
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
        .lock()
        .unwrap()
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
        .lock()
        .unwrap()
        .remove_project_folder(project_id, &path)
}
