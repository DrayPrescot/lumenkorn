//! Bilder zusammenfügen: HDR aus einer Belichtungsreihe, Panorama, Focus
//! Stacking. Eingaben sind lineare, aufrechte Quellbilder (wie sie
//! `Source::load` liefert), Ergebnis ist wieder ein lineares Bild – gespeichert
//! als EXR und danach wie ein RAW entwickelbar.

pub mod align;
pub mod focus;
pub mod hdr;
pub mod pano;

use crate::error::{Error, Result};
use crate::image_f32::ImageF32;
use std::sync::atomic::{AtomicBool, Ordering};

/// Fortschritt: (Schritt, erledigt, gesamt)
pub type Progress<'a> = &'a (dyn Fn(&str, usize, usize) + Sync);

/// Liefert Eingabebild `i` (linear, aufrecht, volle Auflösung).
pub type Loader<'a> = &'a (dyn Fn(usize) -> Result<ImageF32> + Sync);

/// Obergrenze für zwischengespeicherte Vollbilder (HDR, Panorama): darüber wird
/// neu geladen. 3 GB, auf Linux höchstens die Hälfte des gerade verfügbaren
/// Speichers – auf kleinen Rechnern lieber neu dekodieren als auslagern.
/// (Ohne zusätzliche Abhängigkeit; andere Systeme behalten die 3 GB.)
pub(crate) fn keep_bytes() -> usize {
    const MAX: usize = 3 << 30;
    available_bytes().map_or(MAX, |a| (a / 2).clamp(256 << 20, MAX))
}

/// `MemAvailable` aus /proc/meminfo (nur Linux, sonst `None`).
fn available_bytes() -> Option<usize> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = s.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb: usize = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

pub(crate) fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(Error::Params(
            crate::i18n::tr_s("Abgebrochen", "Canceled").into(),
        ))
    } else {
        Ok(())
    }
}
