//! Hintergrundarbeit: Ordner scannen und Thumbnails erzeugen, ohne die
//! Oberfläche zu blockieren. Fortschritt wird als Tauri-Event gemeldet.

use lumenkorn_core::Catalog;
use lumenkorn_core::scanner::{scan_paths, scan_root};
use lumenkorn_core::thumbs::{ThumbCache, generate_some};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Job {
    /// Quellordner vollständig abgleichen
    Scan(i64),
    /// Nur die von der Dateiüberwachung gemeldeten Pfade abgleichen
    /// (gesammelt in `Worker::changes`)
    Changes(i64),
    Thumbs,
    /// Ein Stapel KI-Analyse; reiht sich selbst neu ein, solange Bilder fehlen.
    /// So kommen Scans zwischendurch zum Zug.
    Ai,
    /// Ein Stapel Gesichtserkennung; reiht sich selbst neu ein.
    Faces,
}

#[derive(Clone)]
pub struct Worker {
    tx: Sender<Job>,
    /// Bereits eingereihte Aufträge – doppelte werden verworfen.
    queued: Arc<Mutex<HashSet<Job>>>,
    /// Gemeldete Pfade je Quellordner, bis `Job::Changes` sie abholt
    changes: Arc<Mutex<HashMap<i64, HashSet<PathBuf>>>>,
}

/// Ab so vielen gemeldeten Pfaden ist ein vollständiger Abgleich einfacher.
const MAX_CHANGED_PATHS: usize = 20_000;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ThumbsReady {
    ids: Vec<i64>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct JobError {
    message: String,
}

impl Worker {
    pub fn start(app: AppHandle, catalog: Arc<Mutex<Catalog>>, cache: ThumbCache) -> Self {
        let (tx, rx) = channel::<Job>();
        let queued: Arc<Mutex<HashSet<Job>>> = Arc::default();
        let worker = Worker {
            tx,
            queued: queued.clone(),
            changes: Arc::default(),
        };
        let me = worker.clone();

        std::thread::Builder::new()
            .name("lumenkorn-worker".into())
            .stack_size(16 << 20)
            .spawn(move || {
                // Der Worker startet, bevor der App-Zustand registriert ist –
                // KI-Aufträge brauchen ihn aber.
                while app.try_state::<crate::AppState>().is_none() {
                    std::thread::sleep(Duration::from_millis(20));
                }
                for job in rx {
                    queued
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&job);
                    // Ein Fehler in einem Auftrag darf den Worker nicht beenden.
                    let run = std::panic::AssertUnwindSafe(|| match job {
                        Job::Scan(root_id) => {
                            let progress = |p| {
                                let _ = app.emit("scan-progress", p);
                            };
                            match scan_root(&catalog, root_id, &progress) {
                                Ok(report) => {
                                    log::info!("Scan: {report:?}");
                                    let _ = app.emit("library-changed", report);
                                    me.submit(Job::Thumbs);
                                }
                                Err(e) => {
                                    log::warn!("Scan {root_id} fehlgeschlagen: {e}");
                                    let _ = app.emit(
                                        "job-error",
                                        JobError {
                                            message: e.to_string(),
                                        },
                                    );
                                }
                            }
                        }
                        Job::Changes(root_id) => {
                            let paths: Vec<PathBuf> = me
                                .changes
                                .lock()
                                .unwrap()
                                .remove(&root_id)
                                .map(|s| s.into_iter().collect())
                                .unwrap_or_default();
                            if paths.is_empty() {
                                return;
                            }
                            let progress = |p| {
                                let _ = app.emit("scan-progress", p);
                            };
                            match scan_paths(&catalog, root_id, &paths, &progress) {
                                Ok(report) => {
                                    let changed = report.added
                                        + report.updated
                                        + report.moved
                                        + report.missing;
                                    log::info!("Änderungen ({} Pfade): {report:?}", paths.len());
                                    let _ = app.emit("library-changed", report);
                                    if changed > 0 {
                                        me.submit(Job::Thumbs);
                                    }
                                }
                                Err(e) => {
                                    // Lieber vollständig abgleichen als etwas verpassen
                                    log::warn!("Abgleich der Änderungen fehlgeschlagen: {e}");
                                    me.submit(Job::Scan(root_id));
                                }
                            }
                        }
                        Job::Thumbs => {
                            // Fertige Thumbnails gebündelt melden (max. alle 250 ms).
                            let batch = Mutex::new((Vec::new(), Instant::now()));
                            let flush = |force: bool| {
                                let mut b = batch.lock().unwrap();
                                if !b.0.is_empty()
                                    && (force || b.1.elapsed() > Duration::from_millis(250))
                                {
                                    let ids = std::mem::take(&mut b.0);
                                    let _ = app.emit("thumbs-ready", ThumbsReady { ids });
                                    b.1 = Instant::now();
                                }
                            };
                            let on_done = |id: i64, _ok: bool| {
                                batch.lock().unwrap().0.push(id);
                                flush(false);
                            };
                            // In Portionen: dazwischen kommen Ordneränderungen und Scans dran
                            let more = match generate_some(&catalog, &cache, &on_done, 256) {
                                Ok((_, more)) => more,
                                Err(e) => {
                                    log::warn!("Thumbnails: {e}");
                                    false
                                }
                            };
                            flush(true);
                            me.submit(if more { Job::Thumbs } else { Job::Ai });
                        }
                        Job::Ai => match crate::ai::run_batch(&app) {
                            Ok(true) => me.submit(Job::Ai),
                            // Schlagworte fertig → Gesichter
                            Ok(false) => me.submit(Job::Faces),
                            Err(e) => {
                                log::warn!("KI-Analyse: {e}");
                                me.submit(Job::Faces);
                            }
                        },
                        Job::Faces => match crate::people::run_batch(&app) {
                            Ok(true) => me.submit(Job::Faces),
                            Ok(false) => {}
                            Err(e) => log::warn!("Personenerkennung: {e}"),
                        },
                    });
                    if std::panic::catch_unwind(run).is_err() {
                        log::error!("Auftrag {job:?} abgebrochen (Panik, siehe oben)");
                    }
                }
            })
            .expect("Worker-Thread konnte nicht gestartet werden");

        worker
    }

    /// Von der Dateiüberwachung gemeldete Pfade eines Quellordners abgleichen.
    pub fn submit_changes(&self, root_id: i64, paths: impl IntoIterator<Item = PathBuf>) {
        let too_many = {
            let mut c = self.changes.lock().unwrap();
            let set = c.entry(root_id).or_default();
            set.extend(paths);
            set.len() > MAX_CHANGED_PATHS
        };
        if too_many {
            self.changes.lock().unwrap().remove(&root_id);
            self.submit(Job::Scan(root_id));
        } else {
            self.submit(Job::Changes(root_id));
        }
    }

    pub fn submit(&self, job: Job) {
        if self.queued.lock().unwrap().insert(job) {
            let _ = self.tx.send(job);
        }
    }
}
