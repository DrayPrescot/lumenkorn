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

pub(crate) fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(Error::Params("Abgebrochen".into()))
    } else {
        Ok(())
    }
}
