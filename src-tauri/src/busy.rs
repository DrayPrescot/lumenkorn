//! „Läuft gerade“-Markierungen, die sich selbst zurücksetzen – auch wenn die
//! Arbeit mit einem Panic abbricht oder früh mit einem Fehler endet. Sonst
//! bliebe z. B. „Es läuft bereits eine Reparatur“ bis zum Neustart stehen.

use std::sync::atomic::{AtomicBool, Ordering};

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

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
