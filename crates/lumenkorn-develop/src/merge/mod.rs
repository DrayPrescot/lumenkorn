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
fn keep_bytes() -> usize {
    const MAX: usize = 3 << 30;
    available_bytes().map_or(MAX, |a| (a / 2).clamp(256 << 20, MAX))
}

/// Vollbilder, die zwischen den Arbeitsschritten im Speicher bleiben (HDR,
/// Panorama). Was nicht ins Budget [`keep_bytes`] passt, wird bei Bedarf neu
/// geladen.
pub(crate) struct FrameCache<'a> {
    load: Loader<'a>,
    full: Vec<Option<ImageF32>>,
    kept: usize,
    budget: usize,
}

impl<'a> FrameCache<'a> {
    pub(crate) fn new(load: Loader<'a>) -> Self {
        Self {
            load,
            full: Vec::new(),
            kept: 0,
            budget: keep_bytes(),
        }
    }

    /// Nimmt das nächste Bild (Ladereihenfolge) auf, wenn es noch ins Budget passt.
    pub(crate) fn offer(&mut self, img: ImageF32) {
        let bytes = img.data.len() * 12;
        if self.kept + bytes <= self.budget {
            self.kept += bytes;
            self.full.push(Some(img));
        } else {
            self.full.push(None);
        }
    }

    /// Macht Platz für `bytes` (z. B. den Ergebnispuffer): gibt Bilder von
    /// hinten frei, bis beides ins Budget passt.
    pub(crate) fn reserve(&mut self, bytes: usize) {
        for slot in self.full.iter_mut().rev() {
            if self.kept + bytes <= self.budget {
                break;
            }
            if let Some(img) = slot.take() {
                self.kept -= img.data.len() * 12;
            }
        }
    }

    /// Bild `i` herausnehmen (gibt den Platz frei) oder neu laden.
    pub(crate) fn take(&mut self, i: usize) -> Result<ImageF32> {
        match self.full[i].take() {
            Some(img) => Ok(img),
            None => (self.load)(i),
        }
    }

    /// Legt ein schon geladenes Bild für später zurück (außerhalb des Budgets –
    /// nur für Bilder, die gleich noch einmal gebraucht werden).
    pub(crate) fn put(&mut self, i: usize, img: ImageF32) {
        self.full[i] = Some(img);
    }
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
