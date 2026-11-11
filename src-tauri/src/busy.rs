//! „Läuft gerade“-Markierungen, die sich selbst zurücksetzen – auch wenn die
//! Arbeit mit einem Panic abbricht oder früh mit einem Fehler endet. Sonst
//! bliebe z. B. „Es läuft bereits eine Reparatur“ bis zum Neustart stehen.

use crate::AppState;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tauri::{AppHandle, Manager};

/// Führt `f` beim Verlassen des Gültigkeitsbereichs aus (Drop), genau einmal.
pub struct Defer<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Defer<F> {
    pub fn new(f: F) -> Self {
        Self(Some(f))
    }
}

impl<F: FnOnce()> Drop for Defer<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

/// Setzt `flag`, wenn es frei ist; das Ergebnis gibt es beim Drop wieder frei.
/// `None`, wenn bereits jemand arbeitet.
pub fn claim(flag: &AtomicBool) -> Option<Defer<impl FnOnce() + '_>> {
    (!flag.swap(true, Ordering::SeqCst))
        .then(|| Defer::new(move || flag.store(false, Ordering::SeqCst)))
}

/// Wie [`claim`] für Markierungen im `AppState`, deren Wächter in einen
/// eigenen Thread wandert (dort gibt es keine Leihgabe des Zustands).
pub fn claim_in(
    app: &AppHandle,
    flag: fn(&AppState) -> &AtomicBool,
) -> Option<Defer<impl FnOnce() + Send + 'static>> {
    if flag(&app.state::<AppState>()).swap(true, Ordering::SeqCst) {
        return None;
    }
    let app = app.clone();
    Some(Defer::new(move || {
        flag(&app.state::<AppState>()).store(false, Ordering::SeqCst)
    }))
}

/// Laufende Arbeiten, die Fotos in voller Auflösung laden (Export, Übernahme ins
/// Original, Zusammenfügen, Entrauschen, Reparatur; ein RAW belegt dabei bis
/// ~1,5 GB). Jede hat ihre eigene Markierung und läuft höchstens einmal; diese
/// Zählung begrenzt sie im Speicher-Sparmodus gemeinsam auf eine.
static FULL_RES: AtomicUsize = AtomicUsize::new(0);

/// Angemeldete Arbeit in voller Auflösung; meldet sich beim Drop ab.
pub struct FullRes(());

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

/// Arbeit in voller Auflösung anmelden. Im Speicher-Sparmodus ein Fehler, wenn
/// schon eine läuft.
pub fn full_res() -> Result<FullRes, lumenkorn_core::Error> {
    claim_full_res(lumenkorn_develop::low_memory()).ok_or_else(|| {
        lumenkorn_core::Error::Invalid(
            lumenkorn_develop::i18n::tr_s(
                "Im Speicher-Sparmodus läuft immer nur eine Arbeit in voller Auflösung (Export, Entrauschen, Reparatur, Zusammenfügen) – bitte warten, bis die laufende fertig ist.",
                "In memory saver mode only one full-resolution task runs at a time (export, denoise, repair, merge) – please wait until the current one has finished.",
            )
            .into(),
        )
    })
}

/// Läuft gerade eine Arbeit in voller Auflösung? (Hintergrundarbeit wartet dann)
pub fn full_res_running() -> bool {
    FULL_RES.load(Ordering::SeqCst) > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

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
    fn flag_is_released_after_panic() {
        let flag = AtomicBool::new(false);
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _busy = claim(&flag).unwrap();
            assert!(flag.load(Ordering::SeqCst));
            // Zweiter Versuch, während die Arbeit läuft
            assert!(claim(&flag).is_none());
            panic!("Modell kaputt");
        }));
        assert!(res.is_err());
        assert!(!flag.load(Ordering::SeqCst));
        assert!(claim(&flag).is_some());
    }

    #[test]
    fn deferred_reset_runs_on_panic_and_on_early_return() {
        let running: Mutex<Option<i64>> = Mutex::new(Some(7));
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _reset = Defer::new(|| *running.lock().unwrap() = None);
            panic!("Entrauschen abgestürzt");
        }));
        assert!(res.is_err());
        assert_eq!(*running.lock().unwrap(), None);

        *running.lock().unwrap() = Some(8);
        let work = || -> Result<(), &'static str> {
            let _reset = Defer::new(|| *running.lock().unwrap() = None);
            Err("früher Fehler")?;
            Ok(())
        };
        assert!(work().is_err());
        assert_eq!(*running.lock().unwrap(), None);
    }

    #[test]
    fn dropping_unspawned_work_releases() {
        // Wird der Thread nicht gestartet, verfällt die Closure samt Wächter.
        let flag = AtomicBool::new(false);
        let busy = claim(&flag).unwrap();
        let job = move || drop(busy);
        drop(job);
        assert!(!flag.load(Ordering::SeqCst));
    }
}
