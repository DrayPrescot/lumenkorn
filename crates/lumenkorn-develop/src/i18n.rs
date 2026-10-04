//! Sprache für Meldungen an die Oberfläche (Fehler, Hinweise). Die Oberfläche
//! setzt sie beim Start; Protokollmeldungen bleiben deutsch.
//!
//! Beispiel (nur übersetzt, nicht ausgeführt: die Sprache ist global, und
//! Doctests laufen parallel in einem Prozess – geprüft wird im Unit-Test):
//! ```no_run
//! use lumenkorn_develop::{i18n, tr};
//! let n = 3;
//! i18n::set_english(true);
//! assert_eq!(tr!("{n} Fotos", "{n} photos"), "3 photos");
//! i18n::set_english(false);
//! assert_eq!(i18n::tr_s("Abgebrochen", "Canceled"), "Abgebrochen");
//! ```

use std::sync::atomic::{AtomicBool, Ordering};

static ENGLISH: AtomicBool = AtomicBool::new(false);

pub fn set_english(on: bool) {
    ENGLISH.store(on, Ordering::Relaxed);
}

pub fn is_english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

/// Fester Text in der aktuellen Sprache (z. B. in `#[error(...)]`).
pub fn tr_s(de: &'static str, en: &'static str) -> &'static str {
    if is_english() { en } else { de }
}

/// Formatierter Text in der aktuellen Sprache: `tr!("deutsch {x}", "english {x}")`.
/// Argumente wie bei `format!` – implizit aus Variablen oder explizit danach.
#[macro_export]
macro_rules! tr {
    ($de:literal, $en:literal $(,)?) => {
        if $crate::i18n::is_english() { format!($en) } else { format!($de) }
    };
    ($de:literal, $en:literal, $($arg:tt)+) => {
        if $crate::i18n::is_english() { format!($en, $($arg)+) } else { format!($de, $($arg)+) }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Einziger Test, der die (prozessweite) Sprache umschaltet – andere Tests
    /// verlassen sich auf Deutsch, daher am Ende sofort zurück.
    #[test]
    fn error_message_follows_language() {
        let e = || crate::Error::Params(tr_s("Reparatur beschädigt", "repair corrupted").into());
        set_english(true);
        let en = (e().to_string(), tr!("{} Fotos", "{} photos", 2));
        set_english(false);
        let de = (e().to_string(), tr!("{} Fotos", "{} photos", 2));
        assert_eq!(en.0, "Invalid parameters: repair corrupted");
        assert_eq!(en.1, "2 photos");
        assert_eq!(de.0, "Ungültige Parameter: Reparatur beschädigt");
        assert_eq!(de.1, "2 Fotos");
    }
}
