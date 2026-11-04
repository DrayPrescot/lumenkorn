//! Speichergrenzen beim Lesen fremder Dateien. Eine fehlgeschlagene Anforderung
//! beendet den Prozess, und `catch_unwind` hilft dagegen nicht – hier wird deshalb
//! gemessen, wie viel wirklich angefordert wird (eigene Testdatei wegen des
//! eigenen Allokators).

use lumenkorn_develop::raster;
use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct Counting;
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn add(n: usize) {
    let now = CURRENT.fetch_add(n, Relaxed) + n;
    PEAK.fetch_max(now, Relaxed);
}

// SAFETY: reicht alles unverändert an den Systemallokator weiter und zählt nur mit.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        add(l.size());
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        add(l.size());
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        CURRENT.fetch_sub(l.size(), Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        CURRENT.fetch_sub(l.size(), Relaxed);
        add(n);
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Der Spitzenwert gilt für den ganzen Prozess: nacheinander messen
static SERIAL: Mutex<()> = Mutex::new(());

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for b in parts.iter().flat_map(|p| p.iter()) {
        c ^= u32::from(*b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// 8×8-PNG mit einem Farbprofil, das auf `inflated` Bytes aufgeht
fn png_with_profile(path: &Path, inflated: usize) {
    let mut o = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&8u32.to_be_bytes());
    ihdr.extend_from_slice(&8u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut o, b"IHDR", &ihdr);
    let mut icc = b"x\0\0".to_vec();
    icc.extend_from_slice(&zlib(&vec![0u8; inflated]));
    chunk(&mut o, b"iCCP", &icc);
    chunk(&mut o, b"IDAT", &zlib(&[0u8; (1 + 8 * 3) * 8]));
    chunk(&mut o, b"IEND", &[]);
    std::fs::write(path, o).unwrap();
}

/// Ohne Grenze wuchs ein 1-MB-PNG so auf 1 GB, ein 16-MB-PNG wäre bei 16 GB.
#[test]
fn compressed_png_profile_cannot_fill_the_memory() {
    let _g = SERIAL.lock().unwrap();
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("bombe.png");
    png_with_profile(&p, 400 << 20);
    assert!(std::fs::metadata(&p).unwrap().len() < 4 << 20);

    PEAK.store(CURRENT.load(Relaxed), Relaxed);
    let before = CURRENT.load(Relaxed);
    let opened = raster::open(&p);
    let peak = PEAK.load(Relaxed) - before;
    assert!(peak < 200 << 20, "Spitze {} MB", peak >> 20);
    // Ob das Bild trotzdem aufgeht, ist nicht wichtig – nur, dass es glimpflich endet
    drop(opened);

    PEAK.store(CURRENT.load(Relaxed), Relaxed);
    let before = CURRENT.load(Relaxed);
    let size = raster::dimensions(&p);
    let peak = PEAK.load(Relaxed) - before;
    assert!(peak < 200 << 20, "Spitze {} MB", peak >> 20);
    drop(size);
}

/// Die Grenze gilt den Zusatzdaten: Große Bilder gehen weiter auf.
#[test]
fn large_png_still_opens() {
    let _g = SERIAL.lock().unwrap();
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("gross.png");
    // 75 MB Pixel, also über der Grenze für Zusatzdaten
    let (w, h) = (5000u32, 5000u32);
    image::RgbImage::from_pixel(w, h, image::Rgb([20, 40, 60]))
        .save(&p)
        .unwrap();
    assert_eq!(raster::dimensions(&p).unwrap(), (w, h));
    let img = raster::open(&p).unwrap();
    assert_eq!((img.width(), img.height()), (w, h));
    assert_eq!(img.to_rgb8().get_pixel(4999, 4999).0, [20, 40, 60]);
}

/// Ein normales Farbprofil bleibt lesbar.
#[test]
fn ordinary_png_profile_is_fine() {
    let _g = SERIAL.lock().unwrap();
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("profil.png");
    png_with_profile(&p, 3 << 20);
    assert_eq!(raster::dimensions(&p).unwrap(), (8, 8));
    assert_eq!(raster::open(&p).unwrap().width(), 8);
}
