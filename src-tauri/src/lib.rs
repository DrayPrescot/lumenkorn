//! Tauri-Shell: verbindet den plattformunabhängigen Kern mit der Oberfläche.

mod ai;
mod commands;
mod cull;
mod develop;
mod guard;
mod logfile;
mod memory;
mod merge;
mod people;
mod protocol;
mod repair;
mod worker;

use lumenkorn_core::Catalog;
use lumenkorn_core::maskstore::MaskFiles;
use lumenkorn_core::thumbs::ThumbCache;
use lumenkorn_core::watcher::FolderWatcher;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::Manager;
use worker::{Job, Worker};

pub struct AppState {
    pub catalog: Arc<Mutex<Catalog>>,
    pub cache: ThumbCache,
    pub worker: Worker,
    pub watcher: Mutex<Option<FolderWatcher>>,
    pub data_dir: PathBuf,
    pub develop: develop::DevelopCache,
    pub ai: ai::AiState,
    pub people: people::PeopleState,
    pub repair: repair::RepairState,
    /// Flicken der KI-Reparaturen
    pub repairs: Arc<lumenkorn_develop::repair::RepairFiles>,
    pub masks: Arc<MaskFiles>,
    /// Entrauschte Zwischenstufen
    pub denoised_dir: PathBuf,
    /// Wann KI-Modelle zuletzt gebraucht wurden (zum Entladen)
    pub usage: memory::Usage,
    pub merge: merge::MergeState,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logfile::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("lw", protocol::handle)
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            // Bis Version 0.1 hieß das Projekt „Lichtwerk“ (Kennung de.lichtwerk.app):
            // Katalog, Masken und Vorschaubilder von dort übernehmen.
            adopt_previous_dir(&data_dir);
            if let Ok(c) = app.path().app_cache_dir() {
                adopt_previous_dir(&c);
            }
            std::fs::create_dir_all(&data_dir)?;
            logfile::open(&data_dir);
            let catalog = Arc::new(Mutex::new(Catalog::open(&data_dir.join("katalog.db"))?));
            let masks = Arc::new(MaskFiles::new(data_dir.join("masken")));
            let repairs = Arc::new(lumenkorn_develop::repair::RepairFiles::new(
                data_dir.join("reparaturen"),
            ));
            let cache = ThumbCache::new(
                app.path()
                    .app_cache_dir()
                    .unwrap_or_else(|_| data_dir.join("cache")),
            )
            .with_masks(masks.clone())
            .with_repairs(repairs.clone())
            .with_denoise_dir(data_dir.join("entrauscht"));
            log::info!("Katalog: {}", data_dir.join("katalog.db").display());

            // Eigene Ordner nie als Fotos aufnehmen (siehe scanner::set_excluded_dirs)
            lumenkorn_core::scanner::set_excluded_dirs(vec![
                data_dir.clone(),
                cache.base().to_path_buf(),
            ]);
            match catalog.lock().unwrap().retry_failed_thumbs() {
                Ok(n) if n > 0 => {
                    log::info!("{n} fehlgeschlagene Thumbnails werden erneut versucht")
                }
                Ok(_) => {}
                Err(e) => log::warn!("Thumbnails zurücksetzen: {e}"),
            }
            let worker = Worker::start(app.handle().clone(), catalog.clone(), cache.clone());
            let watcher = start_watcher(catalog.clone(), worker.clone(), data_dir.clone());

            // Alle Quellordner beim Start abgleichen – Änderungen, während die App
            // geschlossen war, werden so erkannt.
            let roots = catalog.lock().unwrap().roots()?;
            let watcher = watcher.map(|mut w| {
                for r in &roots {
                    if let Err(e) = w.watch(std::path::Path::new(&r.path)) {
                        log::warn!("Kann {} nicht überwachen: {e}", r.path);
                    }
                }
                w
            });
            for r in &roots {
                worker.submit(Job::Scan(r.id));
            }
            worker.submit(Job::Thumbs);

            let ai_state = ai::AiState::new(&data_dir);
            let denoised_dir = data_dir.join("entrauscht");
            app.manage(AppState {
                catalog,
                cache,
                worker,
                watcher: Mutex::new(watcher),
                data_dir,
                develop: develop::DevelopCache::default(),
                ai: ai_state,
                people: people::PeopleState::default(),
                repair: repair::RepairState::default(),
                repairs,
                masks,
                denoised_dir,
                usage: memory::Usage::default(),
                merge: merge::MergeState::default(),
            });
            memory::start_janitor(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::list_roots,
            commands::add_root,
            commands::remove_root,
            cull::rejected_summary,
            merge::merge_start,
            merge::merge_cancel,
            cull::delete_rejected,
            memory::develop_release,
            commands::rescan_root,
            commands::folder_tree,
            commands::query_images,
            commands::image_detail,
            commands::set_rating,
            commands::set_flag,
            commands::add_tag,
            commands::remove_tag,
            commands::list_projects,
            commands::create_project,
            commands::update_project,
            commands::delete_project,
            commands::add_project_folder,
            commands::remove_project_folder,
            develop::develop_load,
            develop::develop_sample_color,
            develop::develop_render,
            develop::develop_render_detail,
            develop::develop_detect_tilt,
            develop::develop_save,
            develop::develop_transfer,
            develop::develop_params,
            develop::develop_snapshot,
            develop::develop_restore,
            develop::develop_delete_snapshot,
            develop::export_images,
            ai::ai_status,
            ai::ai_set_settings,
            ai::ai_download,
            ai::ai_cancel_download,
            ai::ai_benchmark,
            ai::app_restart,
            ai::ai_dismiss_notice,
            people::people_status,
            people::people_list,
            people::person_faces,
            people::image_faces,
            people::person_rename,
            people::face_assign,
            people::face_confirm,
            people::face_reject,
            people::face_hide,
            people::person_delete,
            people::people_rescan,
            repair::repair_compute,
            repair::repair_cancel,
            ai::ai_retag_all,
            ai::semantic_search,
            ai::mask_subject,
            ai::mask_sky,
            ai::mask_object,
            ai::denoise_start,
            ai::denoise_cancel,
        ])
        .build(tauri::generate_context!())
        .expect("Lumenkorn konnte nicht gestartet werden")
        .run(|app, event| {
            // Normales Beenden ist kein Absturz – auch nicht mitten in einer Analyse.
            if let tauri::RunEvent::Exit = event
                && let Some(state) = app.try_state::<AppState>()
            {
                ai::clear_crash_markers(&state);
                log::info!("Lumenkorn beendet");
            }
        });
}

fn start_watcher(
    catalog: Arc<Mutex<Catalog>>,
    worker: Worker,
    data_dir: PathBuf,
) -> Option<FolderWatcher> {
    let res = FolderWatcher::new(move |changes| {
        let roots = match catalog.lock() {
            Ok(c) => c.roots().unwrap_or_default(),
            Err(_) => return,
        };
        // Ereignisse verloren: alle Quellordner vollständig abgleichen
        if changes.rescan {
            for r in &roots {
                worker.submit(Job::Scan(r.id));
            }
            return;
        }
        // Eigene Dateien (Katalog, Cache) ignorieren; die Pfade nach
        // Quellordner sortiert nur dort abgleichen, wo sich etwas tat.
        let mut by_root: std::collections::HashMap<i64, Vec<PathBuf>> = Default::default();
        for p in changes.paths {
            if p.starts_with(&data_dir) || lumenkorn_core::scanner::is_excluded(&p) {
                continue;
            }
            if let Some(r) = roots.iter().find(|r| p.starts_with(&r.path)) {
                by_root.entry(r.id).or_default().push(p);
            }
        }
        for (id, paths) in by_root {
            worker.submit_changes(id, paths);
        }
    });
    match res {
        Ok(w) => Some(w),
        Err(e) => {
            log::warn!("Dateiüberwachung nicht verfügbar: {e}");
            None
        }
    }
}

/// Kennung der App vor der Umbenennung in Lumenkorn
const PREVIOUS_IDENTIFIER: &str = "de.lichtwerk.app";

/// Übernimmt einen Ordner der früheren App-Kennung (gleicher übergeordneter
/// Ordner), falls es ihn gibt: Ist der neue Ordner noch nicht da, wird der alte
/// umbenannt, sonst werden fehlende Einträge einzeln verschoben (der neue kann
/// schon die Daten der Web-Ansicht enthalten).
fn adopt_previous_dir(new: &std::path::Path) {
    let Some(parent) = new.parent() else { return };
    let old = parent.join(PREVIOUS_IDENTIFIER);
    if !old.is_dir() || old == new {
        return;
    }
    if !new.exists() {
        match std::fs::rename(&old, new) {
            Ok(()) => log::info!("Daten von {} übernommen", old.display()),
            Err(e) => log::warn!("Kann {} nicht übernehmen: {e}", old.display()),
        }
        return;
    }
    let Ok(entries) = std::fs::read_dir(&old) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        // Daten der Web-Ansicht gehören zur jeweiligen Kennung
        if name == "EBWebView" {
            continue;
        }
        let target = new.join(&name);
        if !target.exists()
            && let Err(err) = std::fs::rename(e.path(), &target)
        {
            log::warn!("Kann {} nicht übernehmen: {err}", e.path().display());
        }
    }
}
