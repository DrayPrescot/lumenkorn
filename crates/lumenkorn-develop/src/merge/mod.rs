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
/// neu geladen. 3 GB, unter Linux und Windows höchstens die Hälfte des gerade
/// verfügbaren Speichers – auf kleinen Rechnern lieber neu dekodieren als
/// auslagern. (Ohne zusätzliche Abhängigkeit; macOS behält die 3 GB.)
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

/// Verfügbarer Arbeitsspeicher: `MemAvailable` aus /proc/meminfo (Linux).
#[cfg(not(windows))]
fn available_bytes() -> Option<usize> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = s.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb: usize = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// Verfügbarer Arbeitsspeicher laut `GlobalMemoryStatusEx` (Windows).
#[cfg(windows)]
fn available_bytes() -> Option<usize> {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }
    let mut m = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    // SAFETY: Zeiger auf eine gültige, richtig ausgerichtete Struktur mit
    // gesetzter Länge, wie die Windows-API es verlangt.
    let ok = unsafe { GlobalMemoryStatusEx(&mut m) } != 0;
    ok.then(|| usize::try_from(m.avail_phys).unwrap_or(usize::MAX))
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

#[cfg(test)]
mod tests {
    #[test]
    fn available_memory_is_known() {
        // Unter Linux und Windows muss die Abfrage funktionieren
        if cfg!(any(windows, target_os = "linux")) {
            let a = super::available_bytes().expect("Speicher abfragbar");
            assert!(a > 64 << 20, "{a}");
        }
        let k = super::keep_bytes();
        assert!((256 << 20..=3 << 30).contains(&k));
    }
}
