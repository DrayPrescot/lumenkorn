//! Alben und Diashows speichern (siehe `lumenkorn_core::docstore`).

use crate::AppState;
use lumenkorn_core::docstore::DocKind;
use lumenkorn_core::{Error, Scope};
use tauri::State;

#[tauri::command(async)]
pub fn doc_get(
    state: State<AppState>,
    kind: DocKind,
    scope: Scope,
) -> Result<Option<String>, Error> {
    state.docs.get(kind, &scope)
}

#[tauri::command(async)]
pub fn doc_put(
    state: State<AppState>,
    kind: DocKind,
    scope: Scope,
    json: String,
) -> Result<(), Error> {
    state.docs.put(kind, &scope, &json)
}

#[tauri::command(async)]
pub fn doc_delete(state: State<AppState>, kind: DocKind, scope: Scope) -> Result<(), Error> {
    state.docs.delete(kind, &scope)
}

/// Projekte mit einem Dokument dieser Art (Symbole in der Seitenleiste)
#[tauri::command(async)]
pub fn doc_projects(state: State<AppState>, kind: DocKind) -> Vec<i64> {
    state.docs.projects(kind)
}
