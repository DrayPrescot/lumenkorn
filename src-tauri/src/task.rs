//! Hintergrundarbeiten, die höchstens einmal gleichzeitig laufen: anmelden, abbrechen,
//! im Speicher-Sparmodus begrenzen, Paniken abfangen. Alle stehen in [`Tasks`] (im
//! `AppState`), damit an einer Stelle sichtbar ist, was im Hintergrund laufen kann.
//!
//! Eine Anmeldung ([`Claim`]) gibt die Aufgabe beim Drop wieder frei – auch wenn die
//! Arbeit mit einem Panic abbricht, früh mit einem Fehler endet oder der Thread gar
//! nicht startet. Sonst bliebe „Es läuft bereits …“ bis zum Neustart stehen.

use crate::AppState;
use lumenkorn_core::Error;
use lumenkorn_develop::i18n::tr_s;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tauri::{AppHandle, Manager};

/// Eine Hintergrundarbeit: läuft sie, und soll sie abbrechen?
pub struct Task {
    running: AtomicBool,
    cancel: AtomicBool,
    /// Meldung, wenn sie schon läuft (deutsch, englisch)
    busy: (&'static str, &'static str),
}

impl Task {
    const fn new(de: &'static str, en: &'static str) -> Self {
        Self {
            running: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            busy: (de, en),
        }
    }

    /// Abbruch erbitten; die Arbeit fragt [`Task::cancelled`] ab.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Abbruch zurücknehmen, wenn eine Aufgabe mehrere Teile nacheinander abarbeitet
    /// und ein Abbruch nur den laufenden Teil betrifft (KI-Downloads).
    pub fn uncancel(&self) {
        self.cancel.store(false, Ordering::SeqCst);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Die Abbruchmarke für Funktionen der Crates, die ein `&AtomicBool` abfragen.
    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.cancel
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Belegen, wenn frei; setzt den Abbruch zurück.
    fn try_start(&self) -> Result<(), Error> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(Error::Busy(tr_s(self.busy.0, self.busy.1).into()));
        }
        self.cancel.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn finish(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

/// Alle Hintergrundarbeiten, die nur einmal gleichzeitig laufen.
pub struct Tasks {
    /// Export, Album-Export und Übernahme ins Original (schreiben alle in Ordner des
    /// Nutzers). Zwei Exporte gleichzeitig verdoppelten den Speicherbedarf, und das
    /// Aufräumen am Ende des ersten gäbe Puffer des zweiten frei. Übernehmen und
    /// Wiederherstellen gleichzeitig könnten eine Sicherung löschen, während das
    /// andere die Datei ohne neue Sicherung ersetzt.
    pub export: Task,
    /// HDR, Panorama, Focus Stacking
    pub merge: Task,
    /// Import von Speicherkarte oder Ordner
    pub import: Task,
    /// KI-Rauschreduzierung
    pub denoise: Task,
    /// KI-Reparatur (Entfernen)
    pub repair: Task,
    /// Hochladen zu Nextcloud
    pub upload: Task,
    /// KI-Komponenten herunterladen (weitere Wünsche reihen sich in die Warteschlange
    /// ein, statt abgewiesen zu werden). Solange es läuft, ruhen Analyse und
    /// Gesichtererkennung.
    pub download: Task,
}

impl Default for Tasks {
    fn default() -> Self {
        Self {
            export: Task::new(
                "Es läuft bereits ein Export oder eine Übernahme ins Original – bitte warten, bis er fertig ist.",
                "An export or an apply to originals is already running – please wait until it has finished.",
            ),
            merge: Task::new(
                "Es werden bereits Fotos zusammengefügt.",
                "Photos are already being merged.",
            ),
            import: Task::new(
                "Es läuft bereits ein Import.",
                "An import is already running.",
            ),
            denoise: Task::new(
                "Es wird bereits ein Foto entrauscht.",
                "A photo is already being denoised.",
            ),
            repair: Task::new(
                "Es läuft bereits eine Reparatur.",
                "A repair is already running.",
            ),
            upload: Task::new(
                "Es läuft bereits ein Hochladen.",
                "An upload is already running.",
            ),
            download: Task::new(
                "Es läuft bereits ein Download.",
                "A download is already running.",
            ),
        }
    }
}

/// Welche Aufgabe in [`Tasks`] (als Funktion, damit die Anmeldung ohne Leihgabe des
/// Zustands in einen eigenen Thread wandern kann).
pub type Which = fn(&Tasks) -> &Task;

/// Angemeldete Arbeit; gibt die Aufgabe beim Drop frei.
pub struct Claim {
    app: AppHandle,
    which: Which,
    full_res: Option<FullRes>,
}

/// Anmelden. Fehler [`Error::Busy`], wenn die Aufgabe schon läuft.
pub fn claim(app: &AppHandle, which: Which) -> Result<Claim, Error> {
    which(&app.state::<AppState>().tasks).try_start()?;
    Ok(Claim {
        app: app.clone(),
        which,
        full_res: None,
    })
}

impl Claim {
    /// Zusätzlich als Arbeit in voller Auflösung anmelden (im Sparmodus nur eine
    /// gleichzeitig). Danach wird der Speicher beim Freigeben ans Betriebssystem
    /// zurückgegeben.
    pub fn full_res(mut self) -> Result<Self, Error> {
        self.full_res = Some(claim_full_res(lumenkorn_develop::low_memory()).ok_or_else(|| {
            Error::Busy(
                tr_s(
                    "Im Speicher-Sparmodus läuft immer nur eine Arbeit in voller Auflösung (Export, Entrauschen, Reparatur, Zusammenfügen) – bitte warten, bis die laufende fertig ist.",
                    "In memory saver mode only one full-resolution task runs at a time (export, denoise, repair, merge) – please wait until the current one has finished.",
                )
                .into(),
            )
        })?);
        Ok(self)
    }

    pub fn task(&self) -> &Task {
        (self.which)(&self.app.state::<AppState>().inner().tasks)
    }

    /// In einem eigenen Thread `lumenkorn-<name>` ausführen; `f` bekommt die
    /// Anmeldung und gibt sie spätestens am Ende frei. Wer danach noch „fertig“
    /// meldet, gibt sie vorher frei (`drop`), sonst scheitert ein sofortiger
    /// Neustart an „läuft bereits“. Startet der Thread nicht, wird sie sofort frei.
    pub fn spawn(
        self,
        name: &str,
        f: impl FnOnce(&AppHandle, Claim) + Send + 'static,
    ) -> Result<(), Error> {
        std::thread::Builder::new()
            .name(format!("lumenkorn-{name}"))
            .spawn(move || {
                let app = self.app.clone();
                f(&app, self);
            })?;
        Ok(())
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.task().finish();
        if self.full_res.take().is_some() {
            // Große Zwischenpuffer dem Betriebssystem zurückgeben
            crate::memory::trim_heap();
        }
    }
}

/// Laufende Arbeiten, die Fotos in voller Auflösung laden (ein RAW belegt dabei bis
/// ~1,5 GB). Jede hat ihre eigene [`Task`] und läuft höchstens einmal; diese Zählung
/// begrenzt sie im Speicher-Sparmodus gemeinsam auf eine.
static FULL_RES: AtomicUsize = AtomicUsize::new(0);

/// Angemeldete Arbeit in voller Auflösung; meldet sich beim Drop ab.
struct FullRes(());

impl Drop for FullRes {
    fn drop(&mut self) {
        FULL_RES.fetch_sub(1, Ordering::SeqCst);
    }
}

fn claim_full_res(low_memory: bool) -> Option<FullRes> {
    if low_memory {
        FULL_RES
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .ok()?;
    } else {
        FULL_RES.fetch_add(1, Ordering::SeqCst);
    }
    Some(FullRes(()))
}

/// Läuft gerade eine Arbeit in voller Auflösung? (Hintergrundarbeit wartet dann)
pub fn full_res_running() -> bool {
    FULL_RES.load(Ordering::SeqCst) > 0
}

/// `f` ausführen; eine Panik (z. B. ein Dekoder an einer kaputten Datei oder die
/// GPU-Pipeline) wird zum Fehler [`Internal`], statt die Arbeit zu beenden. Die
/// Einzelheiten stehen über den Panic-Hook im Protokoll.
pub fn catch_panic<T, E: From<Internal>>(f: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err(Internal.into()))
}

/// Eine Panik, abgefangen von [`catch_panic`].
pub struct Internal;

impl std::fmt::Display for Internal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(tr_s(
            "Interner Fehler (Details im Protokoll)",
            "Internal error (details in the log)",
        ))
    }
}

impl From<Internal> for Error {
    fn from(e: Internal) -> Self {
        Error::Invalid(e.to_string())
    }
}

impl From<Internal> for String {
    fn from(e: Internal) -> Self {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_runs_once_and_start_resets_cancel() {
        let t = Task::new("läuft", "running");
        t.cancel();
        t.try_start().expect("frei");
        assert!(t.is_running());
        assert!(!t.cancelled(), "ein neuer Lauf beginnt ohne alten Abbruch");
        assert!(matches!(t.try_start(), Err(Error::Busy(_))));
        t.cancel();
        assert!(t.cancelled());
        t.finish();
        assert!(!t.is_running());
        t.try_start().expect("wieder frei");
    }

    #[test]
    fn full_res_limit_in_memory_saver_mode() {
        let a = claim_full_res(true).expect("erste");
        assert!(claim_full_res(true).is_none(), "Sparmodus: nur eine");
        let b = claim_full_res(false).expect("ohne Sparmodus mehrere");
        assert!(full_res_running());
        drop((a, b));
        assert!(!full_res_running());
        assert!(claim_full_res(true).is_some());
    }

    #[test]
    fn panic_becomes_an_error() {
        let r: Result<(), Error> = catch_panic(|| panic!("Dekoder kaputt"));
        assert!(r.is_err());
        let r: Result<u8, String> = catch_panic(|| Ok(3));
        assert_eq!(r, Ok(3));
    }
}
