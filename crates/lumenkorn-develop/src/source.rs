//! Quellbild laden: RAW entwickeln bzw. Rasterbild linearisieren.

use crate::color::srgb_decode_lut_u16;
use crate::error::{Error, Result};
use crate::geometry::apply_exif_orientation;
use crate::image_f32::ImageF32;
use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
use rayon::prelude::*;
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

/// Demosaicing-Verfahren für Bayer-Sensoren.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemosaicMethod {
    /// PPG wie in rawler – nur zum Vergleich und für Tests
    Ppg,
    /// Ratio Corrected Demosaicing (Standard)
    Rcd,
}

/// Entwickelt ein RAW bis zum linearen Rec.709-Bild mit Kamera-Weißabgleich.
/// Mit Eingangsschärfung (siehe [`crate::capture_sharpen`]).
fn decode_raw(path: &Path) -> Result<(ImageF32, u16)> {
    let (mut img, o) = decode_raw_with(path, DemosaicMethod::Rcd)?;
    crate::capture_sharpen::apply(&mut img);
    Ok((img, o))
}

/// Wie [`Source::load`] für RAWs, aber mit wählbarem Demosaicing (Vergleiche).
pub fn decode_raw_with(path: &Path, method: DemosaicMethod) -> Result<(ImageF32, u16)> {
    let raw = rawler::decode_file(path)?;
    let orientation = raw.orientation.to_u16();
    let img = match own_pipeline(&raw, method)? {
        Some(img) => img,
        None => rawler_pipeline(&raw)?,
    };
    if img.width == 0 || img.height == 0 {
        return Err(Error::Raw("leeres Bild".into()));
    }
    Ok((img, orientation))
}

/// rawler komplett: Sonderfälle (Fuji SuperCCD, Vierfarb-Sensoren, Monochrom)
fn rawler_pipeline(raw: &rawler::RawImage) -> Result<ImageF32> {
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
    let out = develop.develop_intermediate(raw)?;
    Ok(match out {
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
    })
}

/// Eigenes Demosaicing für RGB-Bayer (RCD) und X-Trans (Markesteijn, 3 Durchgänge;
/// rawler nimmt dort nur bilinear). Farbumrechnung und Standard-Zuschnitt wie in
/// rawler (`develop_intermediate`). `None`: Sonderfall, rawler übernimmt.
fn own_pipeline(raw: &rawler::RawImage, method: DemosaicMethod) -> Result<Option<ImageF32>> {
    use rawler::imgop::sensor::{Demosaic, SensorType};
    use rawler::pixarray::{Color2D, PixF32};
    use rawler::rawimage::RawPhotometricInterpretation;

    let RawPhotometricInterpretation::Cfa(config) = &raw.photometric else {
        return Ok(None);
    };
    if raw.cpp != 1 || !config.cfa.is_rgb() || raw.fuji_rotation_width.is_some() {
        return Ok(None);
    }
    let bayer = config.sensor == SensorType::Bayer;
    if !bayer && config.sensor != SensorType::Xtrans {
        return Ok(None);
    }

    let mut raw = raw.clone();
    raw.apply_scaling()?;
    let pixels = PixF32::new_with(raw.data.as_f32().into_owned(), raw.width, raw.height);
    let roi = raw.active_area.unwrap_or(pixels.rect());
    let wb = if raw.wb_coeffs[0].is_nan() {
        [1.0; 4]
    } else {
        raw.wb_coeffs
    };

    let rgb: Color2D<f32, 3> = if bayer && method == DemosaicMethod::Ppg {
        rawler::imgop::sensor::bayer::ppg::PPGDemosaic::new().demosaic(
            &pixels,
            &config.cfa,
            &config.colors,
            roi,
        )
    } else if bayer {
        let cfa = config.cfa.shift(roi.p.x, roi.p.y);
        let pattern = [
            [cfa.color_at(0, 0), cfa.color_at(0, 1)],
            [cfa.color_at(1, 0), cfa.color_at(1, 1)],
        ];
        if pattern.iter().flatten().any(|c| *c > 2) || roi.width() < 32 || roi.height() < 32 {
            return Ok(None);
        }
        // Mit Weißabgleich demosaikieren (graue Kanten = gleiche Kanäle, so
        // arbeiten die Farbdifferenz-Schätzungen am besten), danach zurück –
        // die Farbumrechnung unten wendet ihn wie bei rawler erneut an.
        let mut m = pixels.data;
        let w = raw.width;
        m.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            if y < roi.p.y {
                return;
            }
            for (x, v) in row.iter_mut().enumerate().skip(roi.p.x) {
                *v *= wb[pattern[(y - roi.p.y) & 1][(x - roi.p.x) & 1]];
            }
        });
        let mut out = crate::demosaic::rcd(
            &m,
            w,
            (roi.p.x, roi.p.y, roi.width(), roi.height()),
            pattern,
        );
        out.par_iter_mut().for_each(|p| {
            for c in 0..3 {
                p[c] = p[c].max(0.0) / wb[c];
            }
        });
        Color2D::new_with(out, roi.width(), roi.height())
    } else {
        rawler::imgop::sensor::xtrans::markesteijn::XTransMarkesteijnDemosaic::new_pass_3()
            .demosaic(&pixels, &config.cfa, &config.colors, roi)
    };

    let rgb = calibrate(&raw, &rgb, &wb);

    // Standard-Zuschnitt, angepasst an den bereits abgeschnittenen aktiven Bereich
    let mut img = rgb;
    if let Some(mut crop) = raw.crop_area.or(raw.active_area) {
        if let Some(active) = &raw.active_area {
            crop = crop.adapt(active);
        }
        if crop.d != img.dim() {
            img = img.crop(crop);
        }
    }
    Ok(Some(ImageF32 {
        width: img.width,
        height: img.height,
        data: img.data.iter().map(|p| sanitize(*p)).collect(),
    }))
}

/// Kamera-RGB → lineares Rec.709 mit Weißabgleich, wie rawler (`Calibrate`).
fn calibrate(
    raw: &rawler::RawImage,
    src: &rawler::pixarray::Color2D<f32, 3>,
    wb: &[f32; 4],
) -> rawler::pixarray::Color2D<f32, 3> {
    use rawler::imgop::matrix::{multiply, normalize, pseudo_inverse, transform_1d};
    use rawler::imgop::xyz::{Illuminant, SRGB_TO_XYZ_D65};

    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0f32]];
    let (illu, matrix) = raw
        .color_matrix_find_first([
            Illuminant::D65,
            Illuminant::A,
            Illuminant::B,
            Illuminant::C,
            Illuminant::D50,
            Illuminant::D55,
            Illuminant::D75,
            Illuminant::Daylight,
            Illuminant::Flash,
        ])
        .unwrap_or_else(|| (Illuminant::D65, identity.as_flattened().to_vec()));
    let d65: Vec<f32> = if illu == Illuminant::D65 || matrix.len() != 9 {
        matrix
    } else {
        let m = transform_1d::<3, 3>(&matrix).unwrap_or(identity);
        rawler::imgop::chromatic_adaption::adapt_bradford(&illu, &Illuminant::D65, &m)
            .as_flattened()
            .to_vec()
    };
    let mut xyz2cam = [[0f32; 3]; 4];
    for i in 0..(d65.len() / 3).min(4) {
        for j in 0..3 {
            xyz2cam[i][j] = d65[i * 3 + j];
        }
    }
    let rgb2cam = normalize(multiply(&xyz2cam, &SRGB_TO_XYZ_D65));
    let cam2rgb = pseudo_inverse(rgb2cam);
    let data = src
        .data
        .par_iter()
        .map(|p| {
            let (r, g, b) = (p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]);
            let out = [
                cam2rgb[0][0] * r + cam2rgb[0][1] * g + cam2rgb[0][2] * b,
                cam2rgb[1][0] * r + cam2rgb[1][1] * g + cam2rgb[1][2] * b,
                cam2rgb[2][0] * r + cam2rgb[2][1] * g + cam2rgb[2][2] * b,
            ];
            rawler::imgop::raw::clip_euclidean_norm_avg(&out)
        })
        .collect();
    rawler::pixarray::Color2D::new_with(data, src.width, src.height)
}

#[inline]
fn sanitize(p: [f32; 3]) -> [f32; 3] {
    let f = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
    [f(p[0]), f(p[1]), f(p[2])]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Eigene Pipeline mit rawlers PPG ergibt exakt rawlers Ergebnis (Farb-
    /// umrechnung, Zuschnitt). Braucht echte RAWs: `LUMENKORN_TEST_RAW=a.nef:b.cr2`.
    #[test]
    fn own_pipeline_matches_rawler() {
        let Ok(list) = std::env::var("LUMENKORN_TEST_RAW") else {
            eprintln!("übersprungen: LUMENKORN_TEST_RAW nicht gesetzt");
            return;
        };
        for f in list.split(':').filter(|f| !f.is_empty()) {
            let raw = rawler::decode_file(Path::new(f)).unwrap();
            let theirs = rawler_pipeline(&raw).unwrap();
            let ours = own_pipeline(&raw, DemosaicMethod::Ppg)
                .unwrap()
                .expect("eigene Pipeline");
            assert_eq!(
                (ours.width, ours.height),
                (theirs.width, theirs.height),
                "{f}"
            );
            let max = ours
                .data
                .iter()
                .zip(&theirs.data)
                .flat_map(|(a, b)| (0..3).map(move |c| (a[c] - b[c]).abs()))
                .fold(0f32, f32::max);
            assert!(max < 1e-5, "{f}: Abweichung {max}");
        }
    }
}
