//! KI-Komponenten herunterladen, der Reihe nach, mit Fortschritt per `ai-download`.

use super::*;

#[derive(Default)]
pub(super) struct Downloads {
    current: Option<Component>,
    queue: std::collections::VecDeque<Component>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct DownloadEvent {
    progress: Option<models::Progress>,
    /// Was gerade geladen wird
    component: Option<Component>,
    /// Was danach noch drankommt
    queued: Vec<Component>,
    /// `component` ist fertig (mit `error` gescheitert)
    finished: bool,
    error: Option<String>,
}

pub(super) fn download_event(
    state: &AppState,
    progress: Option<models::Progress>,
) -> DownloadEvent {
    let d = state.ai.downloads.locked();
    DownloadEvent {
        progress,
        component: d.current,
        queued: d.queue.iter().copied().collect(),
        finished: false,
        error: None,
    }
}

/// Lädt fehlende KI-Komponenten im Hintergrund. Fortschritt per `ai-download`.
/// Läuft schon ein Download, kommt `component` danach dran.
#[tauri::command(async)]
pub fn ai_download(app: AppHandle, component: Option<Component>) -> Result<(), Error> {
    let component = component.unwrap_or(Component::Clip);
    let state = app.state::<AppState>();
    // Einreihen und Starten unter derselben Sperre, unter der der Download-
    // Thread bei leerer Warteschlange endet – so geht kein Wunsch verloren.
    let mut downloads = state.ai.downloads.locked();
    if downloads.current != Some(component) && !downloads.queue.contains(&component) {
        downloads.queue.push_back(component);
    }
    // Setzt `downloading` auch zurück, wenn der Download-Thread panisch endet
    // oder gar nicht erst startet – sonst blieben Analyse und Gesichter-
    // erkennung bis zum Neustart blockiert.
    let Some(done) = crate::busy::claim_in(&app, |s| &s.ai.downloading) else {
        drop(downloads);
        let _ = app.emit("ai-download", download_event(&state, None));
        return Ok(());
    };
    drop(downloads);
    std::thread::Builder::new()
        .name("lumenkorn-download".into())
        .spawn(move || {
            let state = app.state::<AppState>();
            loop {
                let component = {
                    let mut d = state.ai.downloads.locked();
                    let next = d.queue.pop_front();
                    d.current = next;
                    match next {
                        Some(c) => c,
                        None => {
                            drop(done);
                            // Erst jetzt: Solange „lädt herunter“ gilt, brechen
                            // Analyse und Gesichtserkennung sofort ab
                            state.worker.submit(Job::Ai);
                            state.worker.submit(Job::Faces);
                            return;
                        }
                    }
                };
                state.ai.cancel.store(false, Ordering::SeqCst);
                let _ = app.emit("ai-download", download_event(&state, None));
                let error = download_one(&app, &state, component);
                let _ = app.emit(
                    "ai-download",
                    DownloadEvent {
                        finished: true,
                        error,
                        ..download_event(&state, None)
                    },
                );
            }
        })?;
    Ok(())
}

/// Ein Download samt Laden der Modelle; liefert die Fehlermeldung.
pub(super) fn download_one(
    app: &AppHandle,
    state: &AppState,
    component: Component,
) -> Option<String> {
    let progress = |p: models::Progress| {
        let _ = app.emit("ai-download", download_event(state, Some(p)));
    };
    // Ein Panic im Download soll als Fehler enden, nicht als ewiges „läuft“
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        models::download_missing(&state.data_dir, component, &progress, &state.ai.cancel)
    }))
    .unwrap_or_else(|_| {
        Err(lumenkorn_ai::Error::Download(
            tr_s(
                "interner Fehler beim Herunterladen",
                "internal error while downloading",
            )
            .into(),
        ))
    });
    *state.ai.load_error.locked() = None;
    let error = match (res, component) {
        (Ok(()), Component::Clip) => ensure_loaded(state).err().map(|e| e.to_string()),
        (Ok(()), Component::Faces) => {
            let e = ensure_runtime(state).err().map(|e| e.to_string());
            if e.is_none() {
                state.worker.submit(Job::Faces);
            }
            e
        }
        (Ok(()), Component::Masks | Component::Denoise | Component::Repair) => {
            ensure_runtime(state).err().map(|e| e.to_string())
        }
        // Tierarten: schon analysierte Fotos im Hintergrund nachbestimmen
        (Ok(()), Component::Animals) => {
            let e = ensure_runtime(state).err().map(|e| e.to_string());
            if e.is_none() {
                state.worker.submit(Job::Ai);
            }
            e
        }
        // Ist noch keine Laufzeit geladen, wird die neue beim nächsten
        // Bedarf genommen; sonst zeigt der Status „Neustart nötig“.
        (Ok(()), Component::Webgpu) => None,
        (Err(e), _) => Some(e.to_string()),
    };
    if error.is_none() && component == Component::Clip {
        state.worker.submit(Job::Ai);
    }
    error
}

#[tauri::command]
pub fn ai_cancel_download(state: State<AppState>) {
    // Auch alles Eingereihte – „Abbrechen“ meint den ganzen Download
    state.ai.downloads.locked().queue.clear();
    state.ai.cancel.store(true, Ordering::SeqCst);
}
