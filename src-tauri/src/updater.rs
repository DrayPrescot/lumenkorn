//! Updates von GitHub: Beim Start fragt die Oberfläche einmal nach einer
//! neueren Version (`latest.json` des neuesten Releases, signiert mit dem
//! Schlüssel des Projekts). Der Nutzer entscheidet, ob installiert wird;
//! lehnt er ab, kommt die Frage beim nächsten Start wieder.
//!
//! Nur Desktop – auf Android aktualisiert der App-Store bzw. das APK.

use crate::AppState;
use lumenkorn_core::Error;
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

/// Neuere Version auf GitHub? Fehler (offline, GitHub nicht erreichbar)
/// bleiben still – dann gibt es eben kein Update.
#[tauri::command]
pub async fn update_check(app: AppHandle) -> Result<Option<UpdateInfo>, Error> {
    #[cfg(desktop)]
    {
        use tauri::Manager;
        use tauri_plugin_updater::UpdaterExt;
        let updater = match app.updater() {
            Ok(u) => u,
            Err(e) => {
                log::warn!("Updater nicht verfügbar: {e}");
                return Ok(None);
            }
        };
        let update = match updater.check().await {
            Ok(u) => u,
            Err(e) => {
                log::info!("Keine Update-Prüfung möglich: {e}");
                return Ok(None);
            }
        };
        let info = update.as_ref().map(|u| UpdateInfo {
            version: u.version.clone(),
            current: u.current_version.clone(),
            notes: u.body.clone(),
            date: u.date.map(|d| d.to_string()),
        });
        if let Some(i) = &info {
            log::info!("Update verfügbar: {} → {}", i.current, i.version);
        }
        *app.state::<UpdaterState>().pending.lock().unwrap() = update;
        Ok(info)
    }
    #[cfg(not(desktop))]
    {
        let _ = app;
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
            .lock()
            .unwrap()
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
                *app.state::<UpdaterState>().pending.lock().unwrap() = Some(retry);
                Error::Invalid(e.to_string())
            })?;
        // Laufende KI-Arbeit beenden wie beim Neustart aus dem KI-Dialog;
        // ohne Katalog (neuere Version) gibt es keine
        if app.try_state::<AppState>().is_some() {
            crate::ai::app_restart(app);
        } else {
            app.restart();
        }
        Ok(())
    }
    #[cfg(not(desktop))]
    {
        let _ = app;
        Ok(())
    }
}
