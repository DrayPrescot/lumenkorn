//! Ausgabe: JPEG für Vorschau und Export, 16-Bit-TIFF für die Weitergabe an
//! andere Programme.

use crate::error::{Error, Result};
use crate::image_f32::ImageF32;
use image::{ImageBuffer, Rgb, RgbImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::Path;

// Direkt in den fertigen Puffer schreiben: `flat_map_iter().collect()` ist
// nicht indexiert – rayon sammelte Teilstücke und fügte sie mit einer
// weiteren Kopie zusammen (bei 45 MP 135 bzw. 270 MB).
pub fn to_rgb8(img: &ImageF32) -> RgbImage {
    let mut data = vec![0u8; img.data.len() * 3];
    data.par_chunks_mut(3)
        .zip(img.data.par_iter())
        .for_each(|(o, c)| {
            for k in 0..3 {
                o[k] = (c[k].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        });
    ImageBuffer::from_raw(img.width as u32, img.height as u32, data).expect("Puffergröße")
}

pub fn to_rgb16(img: &ImageF32) -> ImageBuffer<Rgb<u16>, Vec<u16>> {
    let mut data = vec![0u16; img.data.len() * 3];
    data.par_chunks_mut(3)
        .zip(img.data.par_iter())
        .for_each(|(o, c)| {
            for k in 0..3 {
                o[k] = (c[k].clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
            }
        });
    ImageBuffer::from_raw(img.width as u32, img.height as u32, data).expect("Puffergröße")
}

pub fn encode_jpeg(img: &ImageF32, quality: u8) -> Result<Vec<u8>> {
    encode_jpeg_rgb8(&to_rgb8(img), quality)
}

/// JPEG aus bereits 8-bittigen Daten (z. B. direkt von der GPU).
///
/// Ab Qualität 90 mit voller Farbauflösung (4:4:4, für Exporte), darunter
/// mit halbierter (4:2:0) wie bei Kamera-JPEGs – fürs Auge gleich, Vorschauen
/// werden so ein Drittel kleiner.
pub fn encode_jpeg_rgb8(rgb: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let (w, h) = rgb.dimensions();
    if w > u16::MAX as u32 || h > u16::MAX as u32 {
        return Err(Error::Params(crate::tr!(
            "{w}×{h} ist zu groß für JPEG (max. 65535)",
            "{w}×{h} is too large for JPEG (max. 65535)"
        )));
    }
    let mut out = Vec::with_capacity(rgb.as_raw().len() / 12);
    let mut enc = jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100));
    enc.set_sampling_factor(if quality >= 90 {
        jpeg_encoder::SamplingFactor::F_1_1
    } else {
        jpeg_encoder::SamplingFactor::F_2_2
    });
    enc.encode(
        rgb.as_raw(),
        w as u16,
        h as u16,
        jpeg_encoder::ColorType::Rgb,
    )?;
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Jpeg,
    Tiff16,
    Png,
    /// Rohdaten des Originals als DNG (nur RAWs, ohne Bearbeitung), siehe [`write_dng`]
    Dng,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Tiff16 => "tif",
            ExportFormat::Png => "png",
            ExportFormat::Dng => "dng",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    pub format: ExportFormat,
    /// Mehrere Formate auf einmal (z. B. DNG und JPEG); leer: nur `format`
    #[serde(default)]
    pub formats: Vec<ExportFormat>,
    /// 1…100, nur für JPEG
    #[serde(default = "default_quality")]
    pub quality: u8,
    /// Längste Kante in Pixeln; `None` = volle Auflösung
    #[serde(default)]
    pub long_edge: Option<usize>,
}

fn default_quality() -> u8 {
    90
}

impl ExportOptions {
    /// Zu schreibende Formate, ohne Doppelte
    pub fn all_formats(&self) -> Vec<ExportFormat> {
        let mut out = Vec::new();
        let list = if self.formats.is_empty() {
            std::slice::from_ref(&self.format)
        } else {
            &self.formats[..]
        };
        for f in list {
            if !out.contains(f) {
                out.push(*f);
            }
        }
        out
    }
}

/// Schreibt das entwickelte Bild. Schreibt zuerst in eine temporäre Datei, damit
/// nie eine halbe Datei am Ziel liegt.
pub fn write(img: &ImageF32, path: &Path, opts: &ExportOptions) -> Result<()> {
    // Ohne Verkleinern keine Kopie (bei 60 MP 720 MB)
    let img: std::borrow::Cow<ImageF32> = match opts.long_edge {
        Some(m) if m > 0 && m < img.long_edge() => std::borrow::Cow::Owned(img.downscale_to(m)),
        _ => std::borrow::Cow::Borrowed(img),
    };
    let tmp = path.with_extension(format!("{}.tmp", opts.format.extension()));
    // PNG/TIFF direkt in die Datei kodieren statt erst in den Speicher
    // (bei einem 150-MP-Panorama als 16-Bit-TIFF ~900 MB weniger)
    let file = |p: &Path| -> Result<std::io::BufWriter<std::fs::File>> {
        Ok(std::io::BufWriter::with_capacity(
            1 << 20,
            std::fs::File::create(p)?,
        ))
    };
    let res = (|| -> Result<()> {
        match opts.format {
            ExportFormat::Jpeg => {
                std::fs::write(&tmp, encode_jpeg(&img, opts.quality.clamp(1, 100))?)?
            }
            ExportFormat::Png => {
                let mut f = file(&tmp)?;
                to_rgb8(&img).write_to(&mut f, image::ImageFormat::Png)?;
                std::io::Write::flush(&mut f)?;
            }
            ExportFormat::Tiff16 => {
                let mut f = file(&tmp)?;
                to_rgb16(&img).write_to(&mut f, image::ImageFormat::Tiff)?;
                std::io::Write::flush(&mut f)?;
            }
            ExportFormat::Dng => {
                return Err(Error::Params(
                    "DNG entsteht aus dem Original, nicht aus dem entwickelten Bild".into(),
                ));
            }
        }
        Ok(())
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Rohdaten eines RAWs verlustfrei als DNG, mit Vorschaubild, aber ohne die
/// Bearbeitung – zum Archivieren oder für andere Programme. `orientation`
/// ist die EXIF-Ausrichtung aus dem Katalog; nicht jeder Decoder kennt sie
/// (Panasonic RW2), das DNG soll trotzdem richtig herum stehen.
pub fn write_dng(raw: &Path, path: &Path, orientation: Option<u16>) -> Result<()> {
    let params = rawler::dng::convert::ConvertParams::default()
        .with_embedded(false)
        .with_software(format!("Lumenkorn {}", env!("CARGO_PKG_VERSION")));
    let mut buf = std::io::Cursor::new(Vec::new());
    rawler::dng::convert::convert_raw_file(raw, &mut buf, &params)?;
    let mut data = buf.into_inner();
    if let Some(o) = orientation.filter(|o| (1..=8).contains(o)) {
        set_tiff_orientation(&mut data, o);
    }
    let tmp = path.with_extension("dng.tmp");
    if let Err(e) = std::fs::write(&tmp, &data) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Überschreibt die Ausrichtung (Tag 0x0112) in IFD0 einer TIFF-Datei. Der
/// DNG-Schreiber legt den Eintrag immer an; fehlt er doch, bleibt alles, wie
/// es ist.
fn set_tiff_orientation(d: &mut [u8], o: u16) -> bool {
    let le = match d.get(0..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return false,
    };
    let u16_at = |d: &[u8], i: usize| -> Option<u16> {
        let b: [u8; 2] = d.get(i..i + 2)?.try_into().ok()?;
        Some(if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let Some(ifd) = d
        .get(4..8)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(|b| if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) } as usize)
    else {
        return false;
    };
    let n = u16_at(d, ifd).unwrap_or(0) as usize;
    for i in 0..n {
        let e = ifd + 2 + i * 12;
        if u16_at(d, e) == Some(0x0112) && u16_at(d, e + 2) == Some(3) {
            let v = if le { o.to_le_bytes() } else { o.to_be_bytes() };
            if let Some(slot) = d.get_mut(e + 8..e + 10) {
                slot.copy_from_slice(&v);
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_is_patched_in_ifd0() {
        // TIFF (LE) mit IFD0 bei 8: ein Eintrag Orientation = 1
        let mut d = b"II*\0".to_vec();
        d.extend(8u32.to_le_bytes());
        d.extend(1u16.to_le_bytes());
        d.extend(0x0112u16.to_le_bytes());
        d.extend(3u16.to_le_bytes());
        d.extend(1u32.to_le_bytes());
        d.extend([1, 0, 0, 0]);
        d.extend(0u32.to_le_bytes());
        assert!(set_tiff_orientation(&mut d, 6));
        assert_eq!(&d[18..20], &[6, 0]);
        assert!(!set_tiff_orientation(&mut b"kein tiff".to_vec(), 6));
    }

    /// Echte RAWs aus `LUMENKORN_TEST_RAW`: DNG lässt sich wieder öffnen
    #[test]
    fn dng_from_real_raws() {
        let Ok(dir) = std::env::var("LUMENKORN_TEST_RAW") else {
            return;
        };
        let out = std::env::temp_dir().join(format!("lw-dng-{}", std::process::id()));
        std::fs::create_dir_all(&out).unwrap();
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
            if ![
                "rw2", "nef", "arw", "cr2", "cr3", "orf", "raf", "pef", "dng",
            ]
            .contains(&ext.to_ascii_lowercase().as_str())
            {
                continue;
            }
            let target = out.join(p.file_stem().unwrap()).with_extension("dng");
            write_dng(&p, &target, Some(6)).unwrap();
            let raw = rawler::decode_file(&target).unwrap();
            assert!(raw.width > 0, "{}", p.display());
            assert_eq!(raw.orientation.to_u16(), 6, "{}", p.display());
        }
        std::fs::remove_dir_all(&out).unwrap();
    }

    #[test]
    fn writes_all_formats() {
        let d = std::env::temp_dir().join(format!("lw-export-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let img = ImageF32::from_fn(300, 200, |x, y| [x as f32 / 300.0, y as f32 / 200.0, 0.5]);
        for (format, name) in [
            (ExportFormat::Jpeg, "a.jpg"),
            (ExportFormat::Tiff16, "a.tif"),
            (ExportFormat::Png, "a.png"),
        ] {
            let p = d.join(name);
            write(
                &img,
                &p,
                &ExportOptions {
                    format,
                    formats: Vec::new(),
                    quality: 90,
                    long_edge: Some(150),
                },
            )
            .unwrap();
            let back = image::open(&p).unwrap();
            assert_eq!((back.width(), back.height()), (150, 100), "{name}");
            if format == ExportFormat::Tiff16 {
                assert_eq!(back.color(), image::ColorType::Rgb16);
            }
        }
        // keine temporären Reste
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 3);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
