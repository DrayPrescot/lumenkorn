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
use std::sync::mpsc::{Receiver, sync_channel};
use std::thread::{Scope, ScopedJoinHandle};

/// Fortschritt: (Schritt, erledigt, gesamt)
pub type Progress<'a> = &'a (dyn Fn(&str, usize, usize) + Sync);

/// Ergebnis von Focus Stacking und Panorama.
pub struct Merged {
    pub image: ImageF32,
    /// Fotos (0-basiert, Ladereihenfolge), die sich nicht ausrichten ließen und
    /// ohne Verschiebung bzw. Drehung eingerechnet wurden – das Ergebnis kann
    /// dort Geisterbilder oder Versatz zeigen. Leer: alles ausgerichtet.
    pub unaligned: Vec<usize>,
}

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

    /// Zieht die Größe eines vorab geladenen Bildes vom Budget ab (siehe
    /// [`Prefetch`]): Das Vorlade-Bild liegt neben dem Zwischenspeicher im RAM,
    /// der Spitzenverbrauch soll aber bei `keep_bytes()` bleiben.
    pub(crate) fn reserve_prefetch(&mut self, img: &ImageF32) {
        self.budget = self.budget.saturating_sub(img.data.len() * 12);
    }

    /// Ob Bild `i` im Speicher liegt (sonst muss es geladen werden).
    pub(crate) fn has(&self, i: usize) -> bool {
        self.full.get(i).is_some_and(Option::is_some)
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

/// Lädt die Bilder aus `order` der Reihe nach auf einem Hilfsthread vor, während
/// der Aufrufer das vorige verarbeitet (RAW-Dekodierung ist größtenteils
/// einfädig, die übrigen Kerne liefen sonst leer). Ergebnisse kommen in
/// genau der Reihenfolge von `order`, Ladefehler an der Stelle, an der das
/// Bild gebraucht wird – das Ergebnis ist dasselbe wie beim sequenziellen Laden.
///
/// Der Kanal hat Tiefe 0 (Übergabe): Der Lader hält höchstens ein fertig
/// dekodiertes Bild, das auf Abholung wartet, nie mehrere. Dieses eine Bild
/// ist zusätzlicher Speicher (siehe [`FrameCache::reserve_prefetch`]).
/// Wird `Prefetch` vorzeitig fallengelassen (Fehler, Abbruch), bricht der
/// Lader nach dem laufenden Bild ab; der Scope wartet darauf.
pub(crate) struct Prefetch<'s> {
    rx: Receiver<Result<ImageF32>>,
    _handle: ScopedJoinHandle<'s, ()>,
    cancel: &'s AtomicBool,
}

impl<'s> Prefetch<'s> {
    pub(crate) fn new<'env>(
        scope: &'s Scope<'s, 'env>,
        order: Vec<usize>,
        load: Loader<'env>,
        cancel: &'env AtomicBool,
    ) -> Self
    where
        'env: 's,
    {
        let (tx, rx) = sync_channel(0);
        let handle = scope.spawn(move || {
            for i in order {
                // Nach Abbruch nichts mehr laden; `next` meldet ihn selbst.
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                if tx.send(load(i)).is_err() {
                    return;
                }
            }
        });
        Self {
            rx,
            _handle: handle,
            cancel,
        }
    }

    /// Das nächste vorgeladene Bild (oder dessen Ladefehler).
    pub(crate) fn next(&self) -> Result<ImageF32> {
        match self.rx.recv() {
            Ok(r) => r,
            // Lader ist ohne Bild beendet: nur nach Abbruch vorgesehen
            Err(_) => {
                check(self.cancel)?;
                Err(Error::Params("Vorladen beendet".into()))
            }
        }
    }
}

/// Verfügbarer Arbeitsspeicher: `MemAvailable` aus /proc/meminfo (Linux).
#[cfg(not(windows))]
pub fn available_bytes() -> Option<usize> {
    meminfo("MemAvailable:")
}

/// Eingebauter Arbeitsspeicher: `MemTotal` aus /proc/meminfo (Linux).
#[cfg(not(windows))]
pub fn total_bytes() -> Option<usize> {
    meminfo("MemTotal:")
}

#[cfg(not(windows))]
fn meminfo(key: &str) -> Option<usize> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = s.lines().find(|l| l.starts_with(key))?;
    let kb: usize = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// Verfügbarer Arbeitsspeicher laut `GlobalMemoryStatusEx` (Windows).
#[cfg(windows)]
pub fn available_bytes() -> Option<usize> {
    memory_status().map(|(_, avail)| avail)
}

/// Eingebauter Arbeitsspeicher laut `GlobalMemoryStatusEx` (Windows).
#[cfg(windows)]
pub fn total_bytes() -> Option<usize> {
    memory_status().map(|(total, _)| total)
}

/// (gesamt, verfügbar) in Bytes
#[cfg(windows)]
fn memory_status() -> Option<(usize, usize)> {
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
    let size = |b: u64| usize::try_from(b).unwrap_or(usize::MAX);
    ok.then(|| (size(m.total_phys), size(m.avail_phys)))
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
    use super::*;

    fn fake(i: usize) -> Result<ImageF32> {
        if i == 3 {
            return Err(Error::Params("kaputt".into()));
        }
        Ok(ImageF32 {
            width: i + 1,
            height: 1,
            data: vec![[0.0; 3]; i + 1],
        })
    }

    #[test]
    fn prefetch_keeps_order_and_propagates_errors() {
        let cancel = AtomicBool::new(false);
        let load = |i: usize| fake(i);
        std::thread::scope(|s| {
            let pf = Prefetch::new(s, vec![4, 0, 2, 3, 1], &load, &cancel);
            assert_eq!(pf.next().unwrap().width, 5);
            assert_eq!(pf.next().unwrap().width, 1);
            assert_eq!(pf.next().unwrap().width, 3);
            // Fehler an der Stelle des betroffenen Bildes
            assert!(pf.next().is_err());
            assert_eq!(pf.next().unwrap().width, 2);
        });
    }

    #[test]
    fn prefetch_stops_when_dropped_or_cancelled() {
        let cancel = AtomicBool::new(false);
        let load = |i: usize| fake(i);
        std::thread::scope(|s| {
            let pf = Prefetch::new(s, vec![0, 1, 2], &load, &cancel);
            assert_eq!(pf.next().unwrap().width, 1);
            // vorzeitig fallenlassen: der Scope darf nicht hängen
        });
        let cancel = AtomicBool::new(true);
        std::thread::scope(|s| {
            let pf = Prefetch::new(s, vec![0, 1], &load, &cancel);
            assert!(pf.next().is_err());
        });
    }

    #[test]
    fn available_memory_is_known() {
        // Unter Linux und Windows muss die Abfrage funktionieren
        if cfg!(any(windows, target_os = "linux")) {
            let a = super::available_bytes().expect("Speicher abfragbar");
            assert!(a > 64 << 20, "{a}");
            let total = super::total_bytes().expect("Gesamtspeicher abfragbar");
            assert!(total >= a, "{total} < {a}");
        }
        let k = super::keep_bytes();
        assert!((256 << 20..=3 << 30).contains(&k));
    }
}
