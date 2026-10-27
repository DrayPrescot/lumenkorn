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
    /// Leicht verlustbehaftetes DNG, bearbeitet oder als Original (siehe
    /// [`ExportOptions::dng_edited`] und [`crate::dng`])
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
    /// DNG mit allen Bearbeitungen; sonst die Daten des Originals ohne
    /// Bearbeitung (zum Weiterentwickeln in anderen Programmen)
    #[serde(default = "yes")]
    pub dng_edited: bool,
    /// Gibt es die Zieldatei schon? Standard: mit „-2“ daneben legen
    #[serde(default)]
    pub on_conflict: OnConflict,
}

/// Umgang mit schon vorhandenen Zieldateien beim Export
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnConflict {
    /// Zusätzlich exportieren: `name-2.jpg`
    #[default]
    Rename,
    /// Vorhandene Datei ersetzen (nie ein Original der Bibliothek)
    Overwrite,
    /// Vorhandene Datei behalten, dieses Format nicht schreiben
    Skip,
}

fn default_quality() -> u8 {
    90
}

fn yes() -> bool {
    true
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
    write_as(img, path, opts, false)
}

/// DNG aus dem unbearbeiteten Original (`scene`: lineares Rec.709 aus
/// [`crate::Source`]).
pub fn write_original_dng(scene: &ImageF32, path: &Path, opts: &ExportOptions) -> Result<()> {
    let opts = ExportOptions {
        format: ExportFormat::Dng,
        ..opts.clone()
    };
    write_as(scene, path, &opts, true)
}

fn write_as(img: &ImageF32, path: &Path, opts: &ExportOptions, scene: bool) -> Result<()> {
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
                let content = if scene {
                    crate::dng::DngContent::Scene(&img)
                } else {
                    crate::dng::DngContent::Developed(&img)
                };
                std::fs::write(&tmp, crate::dng::encode_lossy(content)?)?
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Echte RAWs aus `LUMENKORN_TEST_RAW`: Das Original-DNG liest Lumenkorn
    /// wieder ein, Größe und Farben stimmen
    #[test]
    fn original_dng_from_real_raws() {
        let Ok(dir) = std::env::var("LUMENKORN_TEST_RAW") else {
            return;
        };
        let out = std::env::temp_dir().join(format!("lw-dng-{}", std::process::id()));
        std::fs::create_dir_all(&out).unwrap();
        let mean = |img: &ImageF32| -> [f32; 3] {
            let n = img.data.len() as f32;
            let s = img
                .data
                .iter()
                .fold([0.0f32; 3], |a, c| [0, 1, 2].map(|k| a[k] + c[k].max(0.0)));
            s.map(|v| v / n)
        };
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
            let src = crate::Source::load(&p, true, None, Some(1024)).unwrap();
            let target = out.join(p.file_stem().unwrap()).with_extension("dng");
            let opts = ExportOptions {
                format: ExportFormat::Dng,
                formats: Vec::new(),
                quality: 90,
                long_edge: None,
                dng_edited: false,
                on_conflict: OnConflict::Rename,
            };
            write_original_dng(&src.image, &target, &opts).unwrap();
            let back = crate::Source::load(&target, true, None, None).unwrap();
            assert_eq!(
                (back.image.width, back.image.height),
                (src.image.width, src.image.height),
                "{}",
                p.display()
            );
            // Gleiche Farben; die Helligkeit trägt `BaselineExposure`, das
            // Lumenkorns eigener DNG-Leser (anders als Adobe) nicht anwendet
            let (a, b) = (mean(&src.image), mean(&back.image));
            for k in [0, 2] {
                let (ra, rb) = (a[k] / a[1], b[k] / b[1]);
                assert!((ra - rb).abs() < 0.03, "{}: {a:?} vs {b:?}", p.display());
            }
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
                    dng_edited: true,
                    on_conflict: OnConflict::Rename,
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
