//! Tauri-Shell: verbindet den plattformunabhängigen Kern mit der Oberfläche.

mod ai;
mod album;
mod commands;
mod cull;
mod cull_assist;
mod develop;
mod docs;
mod exit;
mod fonts;
mod geo;
mod guard;
mod import;
mod logfile;
mod mcp;
mod memory;
mod merge;
mod meta;
mod nextcloud;
mod originals;
mod people;
mod protocol;
mod rename;
mod repair;
mod settings;
mod slideshow;
mod style;
mod task;
mod updater;
mod worker;

use lumenkorn_core::Catalog;
use lumenkorn_core::Error;
use lumenkorn_core::LockExt;
use lumenkorn_core::maskstore::MaskFiles;
use lumenkorn_core::thumbs::ThumbCache;
use lumenkorn_core::watcher::FolderWatcher;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use worker::{Job, Worker};

/// Unterordner im Datenordner. An einer Stelle, damit Anlegen, Aufräumen
/// (`memory::sweep_data`) und Löschen (`cull::cleanup`) nie auseinanderlaufen.
pub(crate) mod dirs {
    pub const MASKS: &str = "masken";
    pub const DENOISED: &str = "entrauscht";
    pub const REPAIRS: &str = "reparaturen";
    pub const FACES: &str = "gesichter";
    /// Alben und Diashows
    pub const DOCS: &str = "dokumente";
    /// Zusatzdateien von Bearbeitungen; Dateinamen beginnen mit `<Bild-ID>-`.
    pub const EDIT_FILES: [&str; 3] = [MASKS, DENOISED, REPAIRS];
}

pub struct AppState {
    pub docs: lumenkorn_core::docstore::DocStore,
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
    pub import: import::ImportState,
    pub mcp: mcp::McpState,
    /// „Auto nach deinem Stil“
    pub style: style::StyleState,
    /// Hintergrundarbeiten, die nur einmal gleichzeitig laufen
    pub tasks: task::Tasks,
}

/// Der Katalog stammt von einer neueren Version: Die App startet ohne
/// Katalog (kein `AppState`) und bietet nur das Update an.
pub struct CatalogTooNew(pub String);

/// Datenordner: `LUMENKORN_DATA_DIR` für Tests und Dev-Builds, die den
/// echten Katalog nicht anfassen sollen, sonst der übliche App-Ordner.
fn data_dir_override() -> Option<PathBuf> {
    std::env::var_os("LUMENKORN_DATA_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// `lumenkorn --mcp`: stdio-Brücke für KI-Agenten (siehe `mcp`)
pub fn run_mcp_stdio() {
    mcp::run_stdio();
}

/// Datenordner bestimmen, ggf. den alten „Lichtwerk“-Ordner übernehmen, Sprache laden
fn prepare_data_dir(
    app: &tauri::App,
) -> Result<(std::path::PathBuf, Option<std::path::PathBuf>), Box<dyn std::error::Error>> {
    let custom = data_dir_override();
    let data_dir = match &custom {
        Some(d) => d.clone(),
        None => app.path().app_data_dir()?,
    };
    // Bis Version 0.1 hieß das Projekt „Lichtwerk“ (Kennung de.lichtwerk.app):
    // Katalog, Masken und Vorschaubilder von dort übernehmen.
    if custom.is_none() {
        adopt_previous_dir(&data_dir);
        if let Ok(c) = app.path().app_cache_dir() {
            adopt_previous_dir(&c);
        }
    }
    std::fs::create_dir_all(&data_dir)?;
    logfile::open(&data_dir);
    // Zuletzt gewählte Sprache, bis die Oberfläche sie meldet
    if let Ok(l) = std::fs::read_to_string(language_file(&data_dir)) {
        lumenkorn_develop::i18n::set_english(l.trim() == "en");
    }
    Ok((data_dir, custom))
}

/// Start der App: Katalog, Zustand und Hintergrunddienste einrichten
fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    app.manage(updater::UpdaterState::default());
    let (data_dir, custom) = prepare_data_dir(app)?;
    let catalog_path = data_dir.join("katalog.db");
    settings::prepare_first_start(&data_dir, &catalog_path);
    let catalog = match Catalog::open(&catalog_path) {
        Ok(c) => Arc::new(Mutex::new(c)),
        // Neuere Version: nicht beenden, sondern das Update anbieten
        Err(e @ lumenkorn_core::Error::CatalogTooNew { .. }) => {
            log::warn!("{e}");
            app.manage(CatalogTooNew(e.to_string()));
            return Ok(());
        }
        Err(e) => {
            // Sonst endete die App ohne sichtbaren Grund
            use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
            app.dialog()
                .message(e.to_string())
                .title("Lumenkorn")
                .kind(MessageDialogKind::Error)
                .blocking_show();
            return Err(e.into());
        }
    };
    let masks = Arc::new(MaskFiles::new(data_dir.join(dirs::MASKS)));
    let repairs = Arc::new(lumenkorn_develop::repair::RepairFiles::new(
        data_dir.join(dirs::REPAIRS),
    ));
    let denoised_dir = data_dir.join(dirs::DENOISED);
    let cache = ThumbCache::new(match &custom {
        Some(d) => d.join("cache"),
        None => app
            .path()
            .app_cache_dir()
            .unwrap_or_else(|_| data_dir.join("cache")),
    })
    .with_masks(masks.clone())
    .with_repairs(repairs.clone())
    .with_denoise_dir(denoised_dir.clone());
    log::info!("Katalog: {}", catalog_path.display());
    // Allgemeine Einstellungen wirksam machen (Karte, Speicher-Sparmodus) –
    // vor dem Worker, der gleich Thumbnails entwickelt
    settings::load(&data_dir);

    // Eigene Ordner nie als Fotos aufnehmen (siehe scanner::set_excluded_dirs)
    lumenkorn_core::scanner::set_excluded_dirs(vec![data_dir.clone(), cache.base().to_path_buf()]);
    match catalog.locked().retry_failed_thumbs() {
        Ok(n) if n > 0 => {
            log::info!("{n} fehlgeschlagene Thumbnails werden erneut versucht")
        }
        Ok(_) => {}
        Err(e) => log::warn!("Thumbnails zurücksetzen: {e}"),
    }
    let worker = Worker::start(app.handle().clone(), catalog.clone(), cache.clone());
    let watcher = start_watcher(catalog.clone(), worker.clone(), data_dir.clone());

    // „Unterordner einbeziehen“ vor dem ersten Scan
    settings::apply_folder_rules(&data_dir, &catalog.locked());
    // Alle Quellordner beim Start abgleichen – Änderungen, während die App
    // geschlossen war, werden so erkannt.
    let roots = catalog.locked().roots()?;
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
    let style_state = style::StyleState::new(&data_dir);
    app.manage(AppState {
        docs: lumenkorn_core::docstore::DocStore::new(data_dir.join(dirs::DOCS)),
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
        import: import::ImportState::default(),
        mcp: mcp::McpState::default(),
        style: style_state,
        tasks: task::Tasks::default(),
    });
    style::start(app.handle());
    mcp::start_if_enabled(app.handle());
    memory::start_janitor(app.handle().clone());
    // GPS aus EXIF nachtragen, fehlende Ortsnamen ermitteln
    geo::kick(app.handle());
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logfile::init();

    let builder = tauri::Builder::default();
    // Nur eine Instanz (muss als erstes Plugin kommen): Ein zweiter Start
    // öffnete denselben Katalog parallel und scannte doppelt – stattdessen
    // das vorhandene Fenster nach vorn holen
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
        }
    }));
    let builder = builder.plugin(tauri_plugin_dialog::init());
    // Updates von GitHub (nur Desktop)
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    builder
        .register_asynchronous_uri_scheme_protocol("lw", protocol::handle)
        .setup(setup)
        .invoke_handler(tauri::generate_handler![
            // Nach Modul sortiert; neuer Befehl: hier, in api.ts und in mock.ts
            ai::ai_benchmark,
            ai::ai_cancel_download,
            ai::ai_dismiss_notice,
            ai::ai_download,
            ai::ai_retag_all,
            ai::ai_set_settings,
            ai::ai_status,
            ai::app_restart,
            ai::denoise_cancel,
            ai::denoise_start,
            ai::mask_animal,
            ai::mask_object,
            ai::mask_person,
            ai::mask_sky,
            ai::mask_subject,
            ai::semantic_search,
            album::export::album_export,
            album::export::album_export_cancel,
            album::select::album_places,
            album::select::album_suggest,
            album::select::album_suggest_cancel,
            commands::add_project_folder,
            commands::add_root,
            commands::add_tag,
            commands::app_exit,
            commands::app_info,
            commands::create_project,
            commands::delete_project,
            commands::file_types,
            commands::folder_options,
            commands::folder_set_subfolders,
            commands::folder_tree,
            commands::image_detail,
            commands::list_projects,
            commands::list_roots,
            commands::open_url,
            commands::photo_stats,
            commands::query_images,
            commands::remove_project_folder,
            commands::remove_root,
            commands::remove_tag,
            commands::rescan_root,
            commands::set_flag,
            commands::set_language,
            commands::set_rating,
            commands::update_project,
            cull::delete_rejected,
            cull::rejected_summary,
            cull_assist::cull_learn,
            cull_assist::cull_reset,
            cull_assist::cull_status,
            cull_assist::cull_suggest,
            cull_assist::cull_suggest_cancel,
            develop::develop_auto,
            develop::develop_auto_wb,
            develop::develop_delete_snapshot,
            develop::develop_detect_tilt,
            develop::develop_load,
            develop::develop_params,
            develop::develop_render,
            develop::develop_render_detail,
            develop::develop_restore,
            develop::develop_sample_color,
            develop::develop_save,
            develop::develop_snapshot,
            develop::develop_transfer,
            develop::export_conflicts,
            develop::export_images,
            develop::preset_create,
            develop::preset_delete,
            develop::preset_list,
            develop::preset_rename,
            docs::doc_delete,
            docs::doc_get,
            docs::doc_projects,
            docs::doc_put,
            fonts::system_fonts,
            geo::geo_apply,
            geo::geo_clear_location,
            geo::geo_search,
            geo::geo_set_location,
            geo::geo_time_suggest,
            import::import_cancel,
            import::import_close,
            import::import_defaults,
            import::import_estimate_shift,
            import::import_preview,
            import::import_scan,
            import::import_set_target,
            import::import_start,
            mcp::mcp_new_token,
            mcp::mcp_set_enabled,
            mcp::mcp_status,
            memory::develop_release,
            merge::merge_cancel,
            merge::merge_start,
            meta::meta_get_many,
            meta::meta_set,
            meta::meta_shift_time,
            nextcloud::nc_login_cancel,
            nextcloud::nc_login_start,
            nextcloud::nc_logout,
            nextcloud::nc_project_info,
            nextcloud::nc_set_base_folder,
            nextcloud::nc_shared_projects,
            nextcloud::nc_status,
            nextcloud::nc_upload_cancel,
            nextcloud::nc_upload_project,
            originals::bake_originals,
            originals::bake_summary,
            originals::delete_originals,
            originals::originals_summary,
            originals::restore_originals,
            people::face_assign,
            people::face_confirm,
            people::face_hide,
            people::face_reject,
            people::faces_confirm,
            people::faces_of_images,
            people::faces_reassign,
            people::image_faces,
            people::people_list,
            people::people_rescan,
            people::people_status,
            people::person_delete,
            people::person_faces,
            people::person_group_create,
            people::person_group_delete,
            people::person_group_rename,
            people::person_groups,
            people::person_rename,
            people::person_set_group,
            rename::rename_apply,
            rename::rename_preview,
            repair::repair_cancel,
            repair::repair_compute,
            settings::app_set_settings,
            settings::app_settings,
            settings::memory_status,
            settings::preview_cache_usage,
            slideshow::slideshow_music,
            slideshow::slideshow_video_close,
            slideshow::slideshow_video_open,
            slideshow::slideshow_video_write,
            style::style_reset,
            style::style_status,
            updater::update_check,
            updater::update_install,
        ])
        .build(tauri::generate_context!())
        .expect("Lumenkorn konnte nicht gestartet werden")
        .run(|app, event| {
            // Normales Beenden ist kein Absturz – auch nicht mitten in einer Analyse.
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app.try_state::<AppState>() {
                    ai::clear_crash_markers(&state);
                }
                log::info!("Lumenkorn beendet");
                exit::finish(app);
            }
        });
}

fn start_watcher(
    catalog: Arc<Mutex<Catalog>>,
    worker: Worker,
    data_dir: PathBuf,
) -> Option<FolderWatcher> {
    let res = FolderWatcher::new(move |changes| {
        let roots = catalog.locked().roots().unwrap_or_default();
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

/// Führt `f` im Blocking-Pool aus – für Befehle, die dekodieren, rechnen oder
/// länger auf den Katalog warten und so keinen async-Worker blockieren sollen.
pub(crate) async fn blocking<T: Send + 'static>(
    app: AppHandle,
    f: impl FnOnce(&AppState) -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    tauri::async_runtime::spawn_blocking(move || f(&app.state::<AppState>()))
        .await
        .map_err(|e| {
            Error::Invalid(lumenkorn_develop::tr!(
                "Hintergrundaufgabe abgebrochen: {e}",
                "Background task aborted: {e}"
            ))
        })?
}

/// Längere Leseabfrage über eine eigene Leseverbindung (`Catalog::reader`), damit
/// sie die Katalogsperre nicht hält, die sich die Oberfläche teilt. Lässt sich
/// keine öffnen, liest sie wie bisher unter der Sperre.
pub(crate) fn read_catalog<T>(
    state: &AppState,
    f: impl Fn(&Catalog) -> Result<T, Error>,
) -> Result<T, Error> {
    let reader = state.catalog.locked().reader();
    match reader {
        Ok(r) => f(&r),
        Err(e) => {
            log::warn!("Leseverbindung nicht geöffnet: {e}");
            f(&state.catalog.locked())
        }
    }
}

/// Datei mit der zuletzt gewählten Sprache („de“ oder „en“).
pub(crate) fn language_file(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("sprache")
}

/// Kennung der App vor der Umbenennung in Lumenkorn (bis Version 0.1).
/// Die Übernahme (`adopt_previous_dir`) kostet je Start nur zwei `stat`-Aufrufe
/// und kann entfallen, sobald keine Installationen von vor 0.1 mehr zu
/// erwarten sind.
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
