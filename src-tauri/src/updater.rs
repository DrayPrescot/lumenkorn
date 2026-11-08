//! Updates von GitHub: Beim Start fragt die Oberfläche einmal nach einer
//! neueren Version (`latest.json` des neuesten Releases, signiert mit dem
//! Schlüssel des Projekts). Der Nutzer entscheidet, ob installiert wird;
//! lehnt er ab, bieten die Einstellungen das gefundene Update weiter an, und
//! beim nächsten Start kommt die Frage wieder.
//!
//! Nur Desktop – auf Android aktualisiert der App-Store bzw. das APK.

use crate::AppState;
use lumenkorn_core::Error;
use lumenkorn_core::LockExt;
use serde::Serialize;
use tauri::AppHandle;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current: String,
    /// Text des Releases (Markdown)
    pub notes: Option<String>,
    pub date: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProgress {
    done: u64,
    total: Option<u64>,
    /// Herunterladen fertig, jetzt wird installiert
    installing: bool,
}

/// Das zuletzt gefundene Update – `update_install` installiert genau dieses.
#[derive(Default)]
pub struct UpdaterState {
    #[cfg(desktop)]
    pending: std::sync::Mutex<Option<tauri_plugin_updater::Update>>,
}

/// So lange darf die Prüfung dauern (eine kleine JSON-Datei). Ohne Grenze hing
/// sie bei einer Störung auf GitHub minutenlang, ohne dass etwas zu sehen war.
#[cfg(desktop)]
const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Neuere Version auf GitHub? Beim Start (`manual` aus) bleiben Fehler still,
/// offline gibt es eben kein Update. Auf Wunsch (Einstellungen) wird ein
/// Fehler gemeldet, statt „auf dem neuesten Stand“ vorzutäuschen. Ein früher
/// gefundenes Update bleibt dann installierbar.
#[tauri::command]
pub async fn update_check(
    app: AppHandle,
    manual: Option<bool>,
) -> Result<Option<UpdateInfo>, Error> {
    #[cfg(desktop)]
    {
        use tauri::Manager;
        use tauri_plugin_updater::UpdaterExt;
        let manual = manual.unwrap_or(false);
        let failed = |e: String| {
            if manual {
                Err(Error::Invalid(lumenkorn_develop::tr!(
                    "GitHub ist gerade nicht erreichbar ({e}). Bitte später noch einmal versuchen.",
                    "GitHub cannot be reached right now ({e}). Please try again later."
                )))
            } else {
                Ok(None)
            }
        };
        let updater = match app.updater_builder().timeout(CHECK_TIMEOUT).build() {
            Ok(u) => u,
            Err(e) => {
                log::warn!("Updater nicht verfügbar: {e}");
                return failed(e.to_string());
            }
        };
        let mut update = match updater.check().await {
            Ok(u) => u,
            Err(e) => {
                log::info!("Keine Update-Prüfung möglich: {e}");
                return failed(e.to_string());
            }
        };
        // Die Grenze gilt nur der Prüfung: Der Download (rund 18 MB) darf auf
        // einer langsamen Leitung dauern, wie bisher
        if let Some(u) = update.as_mut() {
            u.timeout = None;
        }
        let info = update.as_ref().map(|u| UpdateInfo {
            version: u.version.clone(),
            current: u.current_version.clone(),
            notes: u.body.clone(),
            date: u.date.map(|d| d.to_string()),
        });
        if let Some(i) = &info {
            log::info!("Update verfügbar: {} → {}", i.current, i.version);
        }
        *app.state::<UpdaterState>().pending.locked() = update;
        Ok(info)
    }
    #[cfg(not(desktop))]
    {
        let _ = (app, manual);
        Ok(None)
    }
}

/// Lädt das gefundene Update, prüft die Signatur, installiert und startet
/// neu. Fortschritt kommt als `update-progress`. Unter Windows beendet das
/// Installationsprogramm die App selbst und startet sie danach wieder.
#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<(), Error> {
    #[cfg(desktop)]
    {
        use tauri::{Emitter, Manager};
        let update = app
            .state::<UpdaterState>()
            .pending
            .locked()
            .take()
            .ok_or_else(|| {
                Error::Invalid(
                    lumenkorn_develop::i18n::tr_s(
                        "Kein Update gefunden – bitte die App neu starten.",
                        "No update found – please restart the app.",
                    )
                    .into(),
                )
            })?;
        let mut done = 0u64;
        let events = app.clone();
        let finished = app.clone();
        // Für einen erneuten Versuch behalten, falls der Download scheitert
        let retry = update.clone();
        update
            .download_and_install(
                move |chunk, total| {
                    done += chunk as u64;
                    let _ = events.emit(
                        "update-progress",
                        UpdateProgress {
                            done,
                            total,
                            installing: false,
                        },
                    );
                },
                move || {
                    let _ = finished.emit(
                        "update-progress",
                        UpdateProgress {
                            done: 0,
                            total: None,
                            installing: true,
                        },
                    );
                },
            )
            .await
            .map_err(|e| {
                *app.state::<UpdaterState>().pending.locked() = Some(retry);
                Error::Invalid(e.to_string())
            })?;
        // Laufende KI-Arbeit beenden wie beim Neustart aus dem KI-Dialog;
        // ohne Katalog (neuere Version) gibt es keine
        if app.try_state::<AppState>().is_some() {
            crate::ai::app_restart(app);
        } else {
            crate::exit::restart(&app);
        }
        Ok(())
    }
    #[cfg(not(desktop))]
    {
        let _ = app;
        Ok(())
    }
}
