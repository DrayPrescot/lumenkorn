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
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Tiff16 => "tif",
            ExportFormat::Png => "png",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    pub format: ExportFormat,
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
