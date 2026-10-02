//! Quellbild laden: RAW entwickeln bzw. Rasterbild linearisieren.

use crate::color::srgb_decode_lut_u16;
use crate::error::{Error, Result};
use crate::geometry::apply_exif_orientation;
use crate::image_f32::ImageF32;
use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// Szenenbezogene Sensordaten mit Reserven über Weiß (> 1.0 möglich).
    Raw,
    /// Bereits für die Anzeige gerenderte Datei (JPEG, PNG, TIFF).
    Raster,
}

/// Aufrecht stehendes, lineares Rec.709-Bild als Ausgangspunkt jeder Entwicklung.
#[derive(Clone)]
pub struct Source {
    pub kind: SourceKind,
    pub image: ImageF32,
    /// Abmessungen in voller Auflösung (nach Orientierung).
    pub full_width: usize,
    pub full_height: usize,
}

impl Source {
    /// Lädt eine Datei. `max_edge` begrenzt die längste Kante (Proxy für die
    /// Bearbeitung); `None` lädt in voller Auflösung (Export).
    ///
    /// `orientation` ist die EXIF-Orientierung aus dem Katalog; fehlt sie bei
    /// einem RAW, wird die des RAW-Decoders verwendet.
    pub fn load(
        path: &Path,
        raw: bool,
        orientation: Option<u16>,
        max_edge: Option<usize>,
    ) -> Result<Source> {
        // EXR (Ergebnis von HDR/Panorama/Focus Stacking): linear und
        // szenenbezogen – entwickelt wird es wie ein RAW (Basiskurve, Lichterreserve).
        if let (true, Some(m)) = (crate::exr_io::is_exr(path), max_edge) {
            // Verkleinert und zwischengespeichert (große Panoramen nur einmal voll lesen)
            let (img, scene) = crate::exr_io::read_downscaled(path, m)?;
            let o = orientation.unwrap_or(1);
            let (w, h) = crate::exr_io::dimensions(path).unwrap_or((img.width, img.height));
            let (full_width, full_height) = if (5..=8).contains(&o) { (h, w) } else { (w, h) };
            return Ok(Source {
                kind: if scene {
                    SourceKind::Raw
                } else {
                    SourceKind::Raster
                },
                image: apply_exif_orientation(img, o),
                full_width,
                full_height,
            });
        }
        let (kind, image) = if crate::exr_io::is_exr(path) {
            let (img, scene) = crate::exr_io::read(path)?;
            let kind = if scene {
                SourceKind::Raw
            } else {
                SourceKind::Raster
            };
            (kind, apply_exif_orientation(img, orientation.unwrap_or(1)))
        } else if raw {
            let (img, raw_orientation) = decode_raw(path)?;
            let o = orientation.unwrap_or(raw_orientation);
            (SourceKind::Raw, apply_exif_orientation(img, o))
        } else {
            let img = decode_raster(path)?;
            (
                SourceKind::Raster,
                apply_exif_orientation(img, orientation.unwrap_or(1)),
            )
        };
        let (full_width, full_height) = (image.width, image.height);
        let image = match max_edge {
            Some(m) => image.downscale_to(m),
            None => image,
        };
        Ok(Source {
            kind,
            image,
            full_width,
            full_height,
        })
    }

    /// Kleinere Kopie, z. B. für Thumbnails aus einem bereits geladenen Proxy.
    pub fn downscaled(&self, max_edge: usize) -> Source {
        // Feldweise statt `..self.clone()` – das kopierte sonst erst das
        // ganze Bild (bei 60 MP 720 MB), nur um es gleich zu verwerfen.
        Source {
            kind: self.kind,
            image: self.image.downscale_to(max_edge),
            full_width: self.full_width,
            full_height: self.full_height,
        }
    }
}

fn decode_raster(path: &Path) -> Result<ImageF32> {
    let img = image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()?;
    let lut = srgb_decode_lut_u16();
    let sixteen = matches!(
        img.color(),
        image::ColorType::Rgb16
            | image::ColorType::Rgba16
            | image::ColorType::L16
            | image::ColorType::La16
    );
    let (w, h) = (img.width() as usize, img.height() as usize);
    let data = if sixteen {
        let rgb = img.to_rgb16();
        rgb.pixels()
            .map(|p| [lut[p[0] as usize], lut[p[1] as usize], lut[p[2] as usize]])
            .collect()
    } else {
        let rgb = img.to_rgb8();
        rgb.pixels()
            .map(|p| {
                [
                    lut[p[0] as usize * 257],
                    lut[p[1] as usize * 257],
                    lut[p[2] as usize * 257],
                ]
            })
            .collect()
    };
    Ok(ImageF32 {
        width: w,
        height: h,
        data,
    })
}

/// Entwickelt ein RAW bis zum linearen Rec.709-Bild mit Kamera-Weißabgleich.
fn decode_raw(path: &Path) -> Result<(ImageF32, u16)> {
    let raw = rawler::decode_file(path)?;
    let orientation = raw.orientation.to_u16();

    // Alles außer der sRGB-Gammakurve – die Tonwertabbildung macht unsere Pipeline.
    let develop = RawDevelop::new_with(&[
        ProcessingStep::Rescale,
        ProcessingStep::Demosaic,
        ProcessingStep::FujiRotate,
        ProcessingStep::CropActiveArea,
        ProcessingStep::WhiteBalance,
        ProcessingStep::Calibrate,
        ProcessingStep::CropDefault,
    ]);
    let out = develop.develop_intermediate(&raw)?;
    let img = match out {
        Intermediate::ThreeColor(px) => ImageF32 {
            width: px.width,
            height: px.height,
            data: px.data.iter().map(|p| sanitize(*p)).collect(),
        },
        Intermediate::Monochrome(px) => ImageF32 {
            width: px.width,
            height: px.height,
            data: px.data.iter().map(|v| sanitize([*v, *v, *v])).collect(),
        },
        Intermediate::FourColor(_) => {
            return Err(Error::Raw(
                "Vierfarb-Sensor nach der Kalibrierung nicht unterstützt".into(),
            ));
        }
    };
    if img.width == 0 || img.height == 0 {
        return Err(Error::Raw("leeres Bild".into()));
    }
    Ok((img, orientation))
}

#[inline]
fn sanitize(p: [f32; 3]) -> [f32; 3] {
    let f = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
    [f(p[0]), f(p[1]), f(p[2])]
}
