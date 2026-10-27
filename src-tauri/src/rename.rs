//! Stapel-Umbenennung: Vorschau und Ausführen (Logik in
//! `lumenkorn_core::rename`).

use crate::{AppState, blocking};
use lumenkorn_core::rename::{self, Item, Options};
use lumenkorn_core::{Error, LockExt};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameReport {
    /// Umbenannte Fotos
    renamed: Vec<Renamed>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Renamed {
    id: i64,
    path: String,
    file_name: String,
}

fn plan_for(state: &AppState, ids: &[i64], options: &Options) -> Result<rename::Plan, Error> {
    let infos = state.catalog.locked().rename_infos(ids)?;
    Ok(rename::plan(&infos, options))
}

/// Neue Namen und Konflikte, ohne etwas zu ändern.
#[tauri::command]
pub async fn rename_preview(
    app: tauri::AppHandle,
    ids: Vec<i64>,
    options: Options,
) -> Result<Vec<Item>, Error> {
    blocking(app, move |state| Ok(plan_for(state, &ids, &options)?.items)).await
}

/// Benennt Dateien und Begleitdateien um und führt den Katalog nach.
/// Die Prüfung wird hier wiederholt; bei einem Konflikt passiert nichts.
#[tauri::command]
pub async fn rename_apply(
    app: tauri::AppHandle,
    ids: Vec<i64>,
    options: Options,
) -> Result<RenameReport, Error> {
    blocking(app, move |state| {
        let plan = plan_for(state, &ids, &options)?;
        if plan.has_conflict() {
            return Err(Error::Invalid(
                lumenkorn_develop::i18n::tr_s(
                    "Es gibt noch Konflikte bei den neuen Namen.",
                    "There are still conflicts with the new names.",
                )
                .into(),
            ));
        }
        // Dateien ohne Katalogsperre umbenennen (Netzlaufwerke können dauern)
        rename::execute(&plan.moves).map_err(Error::Invalid)?;
        // Der Katalog folgt sofort, damit die Dateiüberwachung die Fotos
        // unter dem neuen Namen schon kennt und keine Dubletten anlegt.
        // Scheitert das, werden die Dateien zurückbenannt.
        let saved = state.catalog.locked().rename_images(&plan.changes);
        if let Err(e) = saved {
            let back: Vec<_> = plan
                .moves
                .iter()
                .map(|m| rename::Move {
                    src: m.dst.clone(),
                    dst: m.src.clone(),
                })
                .collect();
            if let Err(e2) = rename::execute(&back) {
                log::error!("Umbenennen zurücknehmen fehlgeschlagen: {e2}");
            }
            return Err(e);
        }
        // Der Entwicklungs-Zwischenspeicher merkt sich den Dateipfad
        for (id, _, _) in &plan.changes {
            state.develop.forget(*id);
        }
        let renamed: Vec<Renamed> = plan
            .changes
            .into_iter()
            .map(|(id, path, file_name)| Renamed {
                id,
                path,
                file_name,
            })
            .collect();
        log::info!("{} Fotos umbenannt", renamed.len());
        Ok(RenameReport { renamed })
    })
    .await
}
