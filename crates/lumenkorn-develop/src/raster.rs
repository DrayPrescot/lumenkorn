//! Rasterbilder (JPEG, PNG, TIFF, WebP) öffnen – auch sehr große.
//!
//! Das `image`-Crate lehnt standardmäßig alles ab, was dekodiert mehr als
//! 512 MB braucht („Memory limit exceeded“); das TIFF-Crate zusätzlich schon
//! ab 256 MB Zwischenpuffer. Davon betroffen sind ganz normale Fotos:
//! Panoramen (30 000 × 14 000 px = 1,26 GB), hochskalierte Bilder,
//! 16-Bit-TIFFs ab etwa 30 MP und 32-Bit-Float-TIFFs aus Astro-Stacking.
//! Hier gilt stattdessen eine eigene, großzügige Obergrenze, und große Bilder
//! werden nacheinander dekodiert, damit parallele Thumbnails nicht gemeinsam
//! den Speicher sprengen (eine fehlgeschlagene Allokation beendet den Prozess).

use crate::color::srgb_decode_lut_u16;
use crate::image_f32::ImageF32;
use image::error::{LimitError, LimitErrorKind};
use image::{DynamicImage, ImageDecoder, ImageError, ImageReader, ImageResult, Limits};
use rayon::prelude::*;
use std::path::Path;
use std::sync::Mutex;

/// Absolute Obergrenzen fürs Dekodieren. Schützt vor defekten oder
/// präparierten Kopfdaten (eine falsche Breite fordert sonst zig GB an; eine
/// fehlgeschlagene Allokation beendet den Prozess, und weil Vorschauen bei
/// jedem Start neu erzeugt werden, gäbe das eine Absturzschleife). Echte Fotos
/// liegen weit darunter (1 Gigapixel RGB8 = 3 GB).
const MAX_BYTES_CAP: u64 = 16 << 30;
const MIN_BYTES_CAP: u64 = 1 << 30;
/// Ohne bekannte Speichergröße (z. B. macOS).
const FALLBACK_BYTES: u64 = 4 << 30;
/// Harte Pixel-Obergrenze, unabhängig vom Speicher.
pub(crate) const MAX_PIXELS: u64 = 1_500_000_000;

/// Höchste erlaubte Größe des dekodierten Bilds: die Hälfte des gerade
/// verfügbaren Arbeitsspeichers, mindestens 1 GiB, höchstens 16 GiB.
pub(crate) fn byte_limit(available: Option<u64>) -> u64 {
    match available {
        Some(a) => (a / 2).clamp(MIN_BYTES_CAP, MAX_BYTES_CAP),
        None => FALLBACK_BYTES,
    }
}

/// Darf ein Bild mit `pixels` Pixeln und `bytes` dekodierter Größe angelegt
/// werden? `available`: verfügbarer Arbeitsspeicher, falls bekannt.
pub(crate) fn decode_allowed(bytes: u64, pixels: u64, available: Option<u64>) -> bool {
    pixels <= MAX_PIXELS && bytes <= byte_limit(available)
}

/// [`decode_allowed`] mit dem jetzt verfügbaren Arbeitsspeicher.
/// `None` bei Überlauf der Maße gilt als nicht erlaubt.
pub(crate) fn dims_allowed(width: usize, height: usize, bytes_per_px: u64) -> bool {
    let Some(pixels) = (width as u64).checked_mul(height as u64) else {
        return false;
    };
    let Some(bytes) = pixels.checked_mul(bytes_per_px) else {
        return false;
    };
    let avail = crate::merge::available_bytes().map(|b| b as u64);
    decode_allowed(bytes, pixels, avail)
}

/// Ab dieser Größe wird nur ein Bild gleichzeitig dekodiert.
const LARGE_BYTES: u64 = 512 << 20;

static LARGE_TURN: Mutex<()> = Mutex::new(());

/// Öffnet ein Rasterbild ohne die Standardgrenzen von `image` (siehe Moduldoc).
pub fn open(path: &Path) -> ImageResult<DynamicImage> {
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    reader.limits(Limits::no_limits());
    let decoder = reader.into_decoder()?;
    let bytes = decoder.total_bytes();
    let (w, h) = decoder.dimensions();
    let pixels = u64::from(w) * u64::from(h);
    let avail = crate::merge::available_bytes().map(|b| b as u64);
    if !decode_allowed(bytes, pixels, avail) {
        return Err(ImageError::Limits(LimitError::from_kind(
            LimitErrorKind::InsufficientMemory,
        )));
    }
    let _turn = (bytes > LARGE_BYTES).then(|| LARGE_TURN.lock().unwrap_or_else(|e| e.into_inner()));
    DynamicImage::from_decoder(decoder)
}

/// Ab dieser Pixelzahl wird für eine verkleinerte Fassung schon beim
/// Linearisieren blockweise gemittelt, statt erst ein Vollbild in Float
/// anzulegen (12 B/px – bei 400 MP fast 5 GB). Darunter bleibt alles exakt wie
/// bisher (Linearisieren, Drehen, dann [`ImageF32::into_downscaled`]).
const PRESHRINK_PIXELS: usize = 100_000_000;

/// Rasterbild → lineares Float-RGB. Sehr große Bilder werden, wenn nur eine
/// Fassung bis `max_edge` gebraucht wird, dabei schon um einen ganzzahligen
/// Faktor in linearem Licht gemittelt (siehe [`PRESHRINK_PIXELS`]); die
/// genaue Endgröße stellt danach wie bei allen Bildern
/// [`ImageF32::into_downscaled`] her (nach dem Drehen, wie bisher).
pub(crate) fn to_linear(img: DynamicImage, max_edge: Option<usize>) -> ImageF32 {
    let lut = srgb_decode_lut_u16();
    let sixteen = matches!(
        img.color(),
        image::ColorType::Rgb16
            | image::ColorType::Rgba16
            | image::ColorType::L16
            | image::ColorType::La16
    );
    let (w, h) = (img.width() as usize, img.height() as usize);
    // Ganzzahliger Vorverkleinerungsfaktor; der Rest geht wie immer über
    // `into_downscaled`, die Kante bleibt dafür ≥ `max_edge`.
    let k = match max_edge {
        Some(m) if m > 0 && w * h > PRESHRINK_PIXELS => (w.max(h) / m).max(1),
        _ => 1,
    };
    if sixteen {
        let raw = img.into_rgb16().into_raw();
        convert(&raw, w, h, k, |v| lut[v as usize])
    } else {
        let raw = img.into_rgb8().into_raw();
        convert(&raw, w, h, k, |v| lut[v as usize * 257])
    }
}

/// Linearisiert interleaved RGB-Samples; mit `k > 1` als Mittel über
/// k×k-Blöcke (Ränder mit den vorhandenen Pixeln).
fn convert<T: Copy + Sync>(
    raw: &[T],
    w: usize,
    h: usize,
    k: usize,
    lin: impl Fn(T) -> f32 + Sync,
) -> ImageF32 {
    if k <= 1 {
        // `into_rgb*` hat den Puffer schon ohne Kopie übernommen, wenn das
        // Bild RGB war; die Umrechnung läuft parallel.
        let data = raw
            .par_chunks_exact(3)
            .map(|p| [lin(p[0]), lin(p[1]), lin(p[2])])
            .collect();
        return ImageF32 {
            width: w,
            height: h,
            data,
        };
    }
    let (ow, oh) = (w.div_ceil(k), h.div_ceil(k));
    let mut data = vec![[0f32; 3]; ow * oh];
    data.par_chunks_mut(ow).enumerate().for_each(|(oy, row)| {
        let y1 = ((oy + 1) * k).min(h);
        for (ox, o) in row.iter_mut().enumerate() {
            let x1 = ((ox + 1) * k).min(w);
            let mut s = [0f64; 3];
            for y in oy * k..y1 {
                let line = &raw[(y * w + ox * k) * 3..(y * w + x1) * 3];
                for p in line.chunks_exact(3) {
                    for c in 0..3 {
                        s[c] += lin(p[c]) as f64;
                    }
                }
            }
            let n = ((y1 - oy * k) * (x1 - ox * k)) as f64;
            *o = s.map(|v| (v / n) as f32);
        }
    });
    ImageF32 {
        width: ow,
        height: oh,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_limits() {
        let gib = 1u64 << 30;
        // Panorama 30 000 x 14 000 RGB8 = 1,26 GB bei normalem Speicher
        let (px, by) = (30_000u64 * 14_000, 30_000u64 * 14_000 * 3);
        assert!(decode_allowed(by, px, Some(16 * gib)));
        assert!(decode_allowed(by, px, Some(8 * gib)));
        // Wenig Speicher: Untergrenze 1 GiB, das Panorama passt nicht mehr
        assert!(!decode_allowed(by, px, Some(gib)));
        assert_eq!(byte_limit(Some(0)), gib);
        assert_eq!(byte_limit(Some(10 * gib)), 5 * gib);
        assert_eq!(byte_limit(Some(1000 * gib)), 16 * gib);
        assert_eq!(byte_limit(None), 4 * gib);
        // Pixelgrenze gilt auch bei viel Speicher
        assert!(!decode_allowed(
            MAX_PIXELS + 1,
            MAX_PIXELS + 1,
            Some(1000 * gib)
        ));
        assert!(decode_allowed(MAX_PIXELS, MAX_PIXELS, Some(1000 * gib)));
        // Überlauf der Maße
        assert!(!dims_allowed(usize::MAX, usize::MAX, 12));
        assert!(dims_allowed(64, 32, 12));
    }

    #[test]
    fn opens_images_above_the_default_limit() {
        // 16-Bit-TIFF mit 600 MB dekodiert: scheiterte am Standardlimit von
        // `image` (512 MB). Gleichförmig, damit die Datei klein bleibt.
        let p = std::env::temp_dir().join(format!("lw-raster-{}.tif", std::process::id()));
        let (w, h) = (12_500u32, 8_000u32);
        image::ImageBuffer::<image::Rgb<u16>, _>::from_pixel(w, h, image::Rgb([1000, 2000, 3000]))
            .save(&p)
            .unwrap();
        assert!(
            ImageReader::open(&p).unwrap().decode().is_err(),
            "Standardlimit greift nicht mehr – Test anpassen"
        );
        let img = open(&p);
        let _ = std::fs::remove_file(&p);
        let img = img.unwrap();
        assert_eq!((img.width(), img.height()), (w, h));
    }

    #[test]
    fn preshrink_averages_in_linear_light() {
        // 4×2-Blöcke aus Schwarz und Weiß → linear 0,5 (nicht sRGB-Mitte)
        let raw: Vec<u8> = (0..4 * 2)
            .flat_map(|i| if i % 2 == 0 { [0u8; 3] } else { [255u8; 3] })
            .collect();
        let lut = srgb_decode_lut_u16();
        let img = convert(&raw, 4, 2, 2, |v| lut[v as usize * 257]);
        assert_eq!((img.width, img.height), (2, 1));
        for p in img.data {
            assert!((p[0] - 0.5).abs() < 1e-6, "{p:?}");
        }
        // Ungerade Ränder: nur vorhandene Pixel zählen
        let raw = vec![255u8; 3 * 3 * 3];
        let img = convert(&raw, 3, 3, 2, |v| lut[v as usize * 257]);
        assert_eq!((img.width, img.height), (2, 2));
        assert!(img.data.iter().all(|p| (p[1] - 1.0).abs() < 1e-6));
    }

    #[test]
    fn small_images_convert_exactly_as_before() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(300, 200, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 77])
        }));
        let lut = srgb_decode_lut_u16();
        let a = to_linear(img.clone(), Some(100));
        assert_eq!(
            (a.width, a.height),
            (300, 200),
            "kleine Bilder: keine Vorverkleinerung"
        );
        let full: Vec<[f32; 3]> = img
            .to_rgb8()
            .pixels()
            .map(|p| p.0.map(|v| lut[v as usize * 257]))
            .collect();
        assert_eq!(a.data, full);
    }
}
