//! Arbeitsspeicher aufräumen.
//!
//! Rust hat keinen Garbage Collector: Speicher wird frei, sobald der letzte
//! Besitzer verschwindet. Was lange lebt, sind deshalb nur bewusst gehaltene
//! Zwischenspeicher – KI-Modelle, dekodierte Bilder, GPU-Puffer. Dieses Modul
//! legt fest, wann sie losgelassen werden:
//!
//! - Beim Verlassen des Entwickeln-Modus: große Bildpuffer (volle Auflösung,
//!   entrauschte Fassungen, Masken, GPU-Puffer) sofort.
//! - KI-Modelle nach einer Weile ohne Nutzung (Hausmeister-Thread). Ein
//!   Modell, das gerade rechnet, wird nie entladen.
//! - Zwischengespeicherte Vollbilder (Zoom, Vergleich) ebenso.
//! - Verwaiste Dateien im Datenordner: kurz nach dem Start, dann täglich.

use crate::AppState;
use lumenkorn_core::LockExt;
use lumenkorn_core::housekeeping::{self, Swept};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// Masken-, Entrausch- und Reparaturmodelle (nur im Entwickeln gebraucht)
const INTERACTIVE_IDLE: Duration = Duration::from_secs(3 * 60);
/// CLIP und Gesichtsmodelle (Hintergrundanalyse)
const BACKGROUND_IDLE: Duration = Duration::from_secs(5 * 60);
/// Vollbilder im Speicher (`protocol::release_full`) – Zoom/Vergleich vorbei
const FULL_IDLE: Duration = Duration::from_secs(3 * 60);
const TICK: Duration = Duration::from_secs(30);
/// Erstes Aufräumen des Datenordners nach dem Start, danach täglich
const SWEEP_FIRST: Duration = Duration::from_secs(90);
const SWEEP_EVERY: Duration = Duration::from_secs(24 * 3600);

#[derive(Clone, Copy)]
pub enum Slot {
    Interactive = 0,
    Background = 1,
}

/// Zeitpunkt der letzten Nutzung je Modellgruppe.
pub struct Usage {
    start: Instant,
    last: [AtomicU64; 2],
}

impl Default for Usage {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            last: [AtomicU64::new(0), AtomicU64::new(0)],
        }
    }
}

impl Usage {
    pub fn touch(&self, slot: Slot) {
        let now = self.start.elapsed().as_secs();
        self.last[slot as usize].store(now, Ordering::Relaxed);
    }

    fn idle(&self, slot: Slot) -> Duration {
        let last = self.last[slot as usize].load(Ordering::Relaxed);
        Duration::from_secs(self.start.elapsed().as_secs().saturating_sub(last))
    }
}

/// Große Puffer der Bildbearbeitung freigeben (beim Verlassen des Entwickelns).
pub fn release_develop(state: &AppState) {
    state.develop.release_large();
    state.masks.clear_cache();
    state.repairs.clear_cache();
    lumenkorn_develop::release_caches();
    trim_heap();
}

#[tauri::command]
pub async fn develop_release(app: AppHandle) -> Result<(), ()> {
    // Kann auf eine laufende RAW-Dekodierung warten: nicht auf der async-Laufzeit
    let _ = crate::blocking(app, |state| {
        release_develop(state);
        log::debug!("Entwickeln verlassen: Bildpuffer freigegeben");
        Ok(())
    })
    .await;
    Ok(())
}

/// Startet den Hausmeister: Er entlädt ungenutzte KI-Modelle und Vollbilder,
/// räumt kurz nach dem Start und dann täglich den Datenordner auf
/// (`sweep_data`) und optimiert dabei die Datenbank.
pub fn start_janitor(app: AppHandle) {
    std::thread::Builder::new()
        .name("hausmeister".into())
        .spawn(move || {
            let mut next_sweep = Instant::now() + SWEEP_FIRST;
            loop {
                std::thread::sleep(TICK);
                // Ein Panic in einem Durchlauf darf den Hausmeister nicht beenden
                let round = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let state = app.state::<AppState>();
                    if Instant::now() >= next_sweep {
                        next_sweep = Instant::now() + SWEEP_EVERY;
                        match sweep_data(&state) {
                            // Katalog geändert (z. B. Abmessungen): Oberfläche lädt neu
                            Ok((_, true)) => {
                                let _ = app.emit("catalog-updated", ());
                            }
                            Ok(_) => {}
                            Err(e) => log::warn!("Aufräumen des Datenordners: {e}"),
                        }
                    }
                    let mut freed = Vec::new();
                    if state.usage.idle(Slot::Interactive) >= INTERACTIVE_IDLE {
                        freed.extend(crate::ai::release_models(&state, true, false));
                        if crate::repair::release_model(&state) {
                            freed.push("LaMa");
                        }
                    }
                    if state.usage.idle(Slot::Background) >= BACKGROUND_IDLE {
                        freed.extend(crate::ai::release_models(&state, false, true));
                        if crate::people::release_models(&state) {
                            freed.push("Gesichter");
                        }
                    }
                    let full = crate::protocol::release_full(FULL_IDLE);
                    if full {
                        log::info!("Ungenutzte Vollbilder freigegeben");
                    }
                    if !freed.is_empty() {
                        log::info!("Ungenutzte KI-Modelle entladen: {}", freed.join(", "));
                    }
                    if full || !freed.is_empty() {
                        trim_heap();
                    }
                }));
                if round.is_err() {
                    log::error!("Hausmeister: Durchlauf abgestürzt, läuft weiter");
                }
            }
        })
        .expect("Hausmeister-Thread");
}

/// Löscht verwaiste Dateien im Datenordner (siehe `housekeeping`) und
/// aktualisiert die Statistiken der Datenbank.
/// Liefert zusätzlich, ob Katalogeinträge geändert wurden.
pub fn sweep_data(state: &AppState) -> Result<(Swept, bool), lumenkorn_core::Error> {
    // Abgelehnte löschen räumt selbst auf; hier geht es um Reste aus
    // Neuberechnungen, entfernten Ordnern und gelöschten Masken.
    const EDIT_FILES: Duration = Duration::from_secs(24 * 3600);
    const DERIVED: Duration = Duration::from_secs(3600);
    let t = Instant::now();
    // Unter der Sperre wird nur eingesammelt bzw. kurz geschrieben, das
    // Parsen der Bearbeitungen geschieht ohne Sperre.
    let rows = state.catalog.locked().edit_rows()?;
    let plan = housekeeping::plan_foreign_refs(rows)?;
    let fixed = state.catalog.locked().apply_edit_fixes(&plan)?;
    if fixed > 0 {
        log::info!("{fixed} Bearbeitungen mit Verweisen auf fremde Fotos bereinigt");
        state.worker.submit(crate::worker::Job::Thumbs);
    }
    let dims = housekeeping::fix_raw_dims(&state.catalog)?;
    if dims > 0 {
        log::info!("Abmessungen von {dims} RAWs korrigiert");
    }
    let live = state.catalog.locked().live_raw()?.resolve();
    let keep_file = |stem: &str| live.files.contains(stem);
    let mut swept = Swept::default();
    for dir in crate::dirs::EDIT_FILES {
        swept += housekeeping::sweep_dir(&state.data_dir.join(dir), EDIT_FILES, keep_file);
    }
    swept += housekeeping::sweep_dir(&crate::people::thumb_dir(state), DERIVED, |stem| {
        stem.parse::<i64>().is_ok_and(|id| live.faces.contains(&id))
    });
    swept += housekeeping::sweep_image_cache(state.cache.base(), &live.images, DERIVED);
    state.catalog.locked().optimize()?;
    if swept.files > 0 {
        log::info!(
            "Datenordner aufgeräumt: {} verwaiste Dateien, {:.1} MB, in {:.1?}",
            swept.files,
            swept.bytes as f64 / 1e6,
            t.elapsed()
        );
    }
    Ok((swept, fixed > 0 || dims > 0))
}

/// Gibt freien Heap an das Betriebssystem zurück. Unter Linux (glibc) behält
/// `free` große Bereiche sonst oft für später; Windows gibt sie selbst zurück.
pub(crate) fn trim_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        malloc_trim(0);
    }
}
