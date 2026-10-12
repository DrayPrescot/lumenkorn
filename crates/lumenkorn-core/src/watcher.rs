//! Überwacht die Quellordner und meldet Änderungen gebündelt.

use crate::error::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, Instant};

/// Ruhezeit nach der letzten Änderung, bevor gemeldet wird.
const QUIET: Duration = Duration::from_millis(1500);
/// Spätestens so lange nach der ersten Änderung wird gemeldet, auch wenn
/// es nie ruhig wird (Tethering, Cloud-Sync, Logdatei im Ordner) – sonst
/// erschienen neue Fotos nie und die Liste wüchse ohne Grenze.
const MAX_WAIT: Duration = Duration::from_secs(15);

/// Gebündelte Änderungen.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Changes {
    /// Geänderte, neue, gelöschte oder umbenannte Dateien und Ordner
    pub paths: Vec<PathBuf>,
    /// Das System hat Ereignisse verloren (Pufferüberlauf o. Ä.): Alles
    /// muss vollständig abgeglichen werden.
    pub rescan: bool,
}

enum Msg {
    Paths(Vec<PathBuf>),
    Rescan,
}

/// Hält die Überwachung am Leben, solange das Objekt existiert.
pub struct FolderWatcher {
    watcher: RecommendedWatcher,
    watched: HashSet<PathBuf>,
}

impl FolderWatcher {
    /// `on_change` erhält die geänderten Pfade, frühestens 1,5 s nach der
    /// letzten Änderung – so führt das Kopieren von 500 Bildern zu einem
    /// Abgleich.
    ///
    /// Reine Lesezugriffe (Öffnen, Schließen ohne Schreiben) werden ignoriert,
    /// sonst würde jeder Scan und jedes Thumbnail einen neuen Scan auslösen.
    pub fn new(on_change: impl Fn(Changes) + Send + 'static) -> Result<Self> {
        Self::with_timing(QUIET, MAX_WAIT, on_change)
    }

    fn with_timing(
        quiet: Duration,
        max_wait: Duration,
        on_change: impl Fn(Changes) + Send + 'static,
    ) -> Result<Self> {
        let (tx, rx) = channel::<Msg>();

        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| match res {
            Ok(ev) if ev.need_rescan() => {
                let _ = tx.send(Msg::Rescan);
            }
            Ok(ev) if is_relevant(&ev.kind) => {
                let paths: Vec<PathBuf> = ev
                    .paths
                    .into_iter()
                    .filter(|p| !is_dir_content_change(&ev.kind, p))
                    .collect();
                if !paths.is_empty() {
                    let _ = tx.send(Msg::Paths(paths));
                }
            }
            Ok(_) => {}
            Err(e) => {
                // Verlorene Ereignisse: lieber einmal alles abgleichen
                log::warn!("Dateiüberwachung: {e} – gleiche vollständig ab");
                let _ = tx.send(Msg::Rescan);
            }
        })?;

        std::thread::Builder::new()
            .name("lumenkorn-watch".into())
            .spawn(move || {
                let mut pending: HashSet<PathBuf> = HashSet::new();
                let mut rescan = false;
                // Zeitpunkt der ersten noch nicht gemeldeten Änderung
                let mut since: Option<Instant> = None;
                loop {
                    let wait =
                        since.map_or(quiet, |t| quiet.min(max_wait.saturating_sub(t.elapsed())));
                    let quiet_now = match rx.recv_timeout(wait) {
                        Ok(Msg::Paths(paths)) => {
                            pending.extend(paths);
                            false
                        }
                        Ok(Msg::Rescan) => {
                            rescan = true;
                            false
                        }
                        Err(RecvTimeoutError::Timeout) => true,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    if pending.is_empty() && !rescan {
                        continue;
                    }
                    let first = *since.get_or_insert_with(Instant::now);
                    if quiet_now || first.elapsed() >= max_wait {
                        since = None;
                        on_change(Changes {
                            paths: pending.drain().collect(),
                            rescan: std::mem::take(&mut rescan),
                        });
                    }
                }
            })?;

        Ok(Self {
            watcher,
            watched: HashSet::new(),
        })
    }

    pub fn watch(&mut self, dir: &Path) -> Result<()> {
        if self.watched.insert(dir.to_path_buf()) {
            self.watcher.watch(dir, RecursiveMode::Recursive)?;
        }
        Ok(())
    }

    pub fn unwatch(&mut self, dir: &Path) {
        if self.watched.remove(dir) {
            let _ = self.watcher.unwatch(dir);
        }
    }
}

/// Windows meldet beim Anlegen einer Datei auch ihren Ordner als „geändert“
/// (Änderungszeit). Den ganzen Ordner deshalb neu einzulesen, wäre genau der
/// Aufwand, den die Überwachung vermeiden soll – die Datei selbst wird ja
/// gemeldet. Neue, gelöschte und umbenannte Ordner bleiben relevant.
fn is_dir_content_change(kind: &EventKind, path: &Path) -> bool {
    use notify::event::ModifyKind;
    matches!(
        kind,
        EventKind::Modify(ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Other)
    ) && path.is_dir()
}

fn is_relevant(kind: &EventKind) -> bool {
    use notify::event::{AccessKind, AccessMode};
    match kind {
        EventKind::Create(_) | EventKind::Remove(_) => true,
        // Metadatenänderungen (z. B. Zugriffszeit) lösen keinen Scan aus.
        EventKind::Modify(m) => !matches!(m, notify::event::ModifyKind::Metadata(_)),
        // Fertig geschriebene Datei (z. B. Kopiervorgang abgeschlossen)
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        EventKind::Any | EventKind::Other => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn reports_writes_but_not_reads() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("vorhanden.jpg"), b"x").unwrap();
        let hits: Arc<Mutex<Vec<Vec<PathBuf>>>> = Arc::default();
        let h = hits.clone();
        let mut w = FolderWatcher::new(move |c| h.lock().unwrap().push(c.paths)).unwrap();
        w.watch(d.path()).unwrap();

        // Nur lesen: darf nichts melden
        let _ = std::fs::read(d.path().join("vorhanden.jpg")).unwrap();
        std::thread::sleep(QUIET * 2);
        assert!(
            hits.lock().unwrap().is_empty(),
            "Lesezugriff hat Scan ausgelöst"
        );

        // Schreiben: wird gebündelt gemeldet
        std::fs::write(d.path().join("neu.jpg"), b"y").unwrap();
        std::thread::sleep(QUIET * 2);
        let got = hits.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert!(got[0].iter().any(|p| p.ends_with("neu.jpg")));
    }

    #[test]
    fn reports_during_constant_writing() {
        let d = tempfile::tempdir().unwrap();
        let hits: Arc<Mutex<usize>> = Arc::default();
        let h = hits.clone();
        let quiet = Duration::from_millis(400);
        let mut w = FolderWatcher::with_timing(quiet, Duration::from_millis(1000), move |_| {
            *h.lock().unwrap() += 1
        })
        .unwrap();
        w.watch(d.path()).unwrap();
        // Alle 100 ms schreiben – nie 400 ms Ruhe
        for i in 0..30 {
            std::fs::write(d.path().join("log.txt"), format!("{i}")).unwrap();
            std::thread::sleep(Duration::from_millis(100));
        }
        let during = *hits.lock().unwrap();
        assert!(during >= 2, "nur {during} Meldungen während des Schreibens");
    }
}
