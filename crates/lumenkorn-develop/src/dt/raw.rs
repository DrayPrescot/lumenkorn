//! RAW-Entwicklung wie darktable 5.0 bis einschließlich color calibration;
//! sigmoid folgt in der Entwicklung (`pipeline.rs`, GPU).
//!
//! rawprepare (Weißpunkt aus rawspeed) → temperature (Kamera-Weißabgleich,
//! „as shot to reference“) → highlights (inpaint opposed) → demosaic (RCD,
//! X-Trans: Markesteijn 1 Durchgang) → exposure (+0,7 EV abzüglich der
//! Belichtungskorrektur der Kamera, Schwarz −2⁻¹²) → colorin (Kameramatrix,
//! Rec.2020) → color calibration (CAT16 von der Lichtart der Kamera nach D50).
//! Das Ergebnis wird in lineares sRGB umgerechnet (ohne Beschneiden).

use super::color::{self, M3};
use crate::error::Result;
use crate::image_f32::ImageF32;
use rayon::prelude::*;

/// exposure: Standard im szenenbezogenen Ablauf
const SCENE_EXPOSURE: f32 = 0.7;
/// exposure: Schwarzpunkt im szenenbezogenen Ablauf
const SCENE_BLACK: f32 = -0.000_244_140_63;

/// Daten aus EXIF, die darktable verwendet
#[derive(Default, Clone, Copy)]
pub(crate) struct Exif {
    pub exposure_bias: Option<f32>,
    pub iso: Option<u32>,
}

impl Exif {
    /// Aus den bereits gelesenen RAW-Metadaten (rawler).
    pub(crate) fn from_metadata(md: &rawler::decoders::RawMetadata) -> Exif {
        let e = &md.exif;
        Exif {
            exposure_bias: e
                .exposure_bias
                .filter(|r| r.d != 0)
                .map(|r| r.n as f32 / r.d as f32),
            iso: e.iso_speed_ratings.map(u32::from).or(e.iso_speed),
        }
    }
}

/// Kameramatrix XYZ → Kamera (D65, wie darktables Adobe-Matrizen)
fn xyz_to_cam(raw: &rawler::RawImage) -> Option<M3> {
    use rawler::imgop::xyz::Illuminant;
    let m = raw.color_matrix.get(&Illuminant::D65)?;
    if m.len() < 9 || m.iter().all(|v| *v == 0.0) {
        return None;
    }
    Some(std::array::from_fn(|i| {
        std::array::from_fn(|j| m[i * 3 + j] as f64)
    }))
}

/// Ist die darktable-Entwicklung für dieses RAW möglich? (RGB-Bayer oder
/// X-Trans mit D65-Kameramatrix; Sonderfälle übernimmt rawler.)
pub(crate) fn applies(raw: &rawler::RawImage) -> bool {
    use rawler::imgop::sensor::SensorType;
    use rawler::rawimage::RawPhotometricInterpretation;
    let RawPhotometricInterpretation::Cfa(config) = &raw.photometric else {
        return false;
    };
    if raw.cpp != 1 || !config.cfa.is_rgb() || raw.fuji_rotation_width.is_some() {
        return false;
    }
    if config.sensor != SensorType::Bayer && config.sensor != SensorType::Xtrans {
        return false;
    }
    let roi = raw.active_area.unwrap_or(rawler::imgop::Rect::new(
        rawler::imgop::Point::default(),
        rawler::imgop::Dim2::new(raw.width, raw.height),
    ));
    if roi.width() < 32 || roi.height() < 32 {
        return false;
    }
    // Bereiche aus dem Decoder nicht blind übernehmen: sie müssen im Bild
    // liegen und die Daten groß genug sein, sonst übernimmt rawler.
    let inside = |r: &rawler::imgop::Rect, w: usize, h: usize| {
        r.p.x.checked_add(r.width()).is_some_and(|x| x <= w)
            && r.p.y.checked_add(r.height()).is_some_and(|y| y <= h)
    };
    let len = match &raw.data {
        rawler::rawimage::RawImageData::Integer(v) => v.len(),
        rawler::rawimage::RawImageData::Float(v) => v.len(),
    };
    if raw.width.checked_mul(raw.height).is_none_or(|n| len < n)
        || !inside(&roi, raw.width, raw.height)
    {
        return false;
    }
    if let Some(crop) = &raw.crop_area {
        let ok = match &raw.active_area {
            // Zuschnitt wird relativ zum aktiven Bereich angewandt
            Some(a) => {
                crop.p.x >= a.p.x
                    && crop.p.y >= a.p.y
                    && inside(
                        &rawler::imgop::Rect::new(
                            rawler::imgop::Point::new(crop.p.x - a.p.x, crop.p.y - a.p.y),
                            crop.d,
                        ),
                        a.width(),
                        a.height(),
                    )
            }
            None => inside(crop, raw.width, raw.height),
        };
        if !ok || crop.width() == 0 || crop.height() == 0 {
            return false;
        }
    }
    let cfa = config.cfa.shift(roi.p.x, roi.p.y);
    let colors_ok = (0..6).all(|r| (0..6).all(|c| cfa.color_at(r, c) <= 2));
    colors_ok
        && xyz_to_cam(raw).is_some_and(|m| {
            // Matrix muss sich invertieren lassen, Weißabgleich und Primärfarben
            // müssen endlich bleiben (Zeilensumme 0 würde NaN ergeben)
            color::inv(&m).is_some()
                && color::camera_to_xyz(&m)
                    .is_some_and(|c| c.iter().flatten().all(|v| v.is_finite()))
                && color::d65_coeffs(&m)
                    .iter()
                    .all(|v| v.is_finite() && *v > 0.0)
        })
}

/// Entwickelt bis zum szenenbezogenen linearen sRGB (siehe Modulbeschreibung).
/// Nur aufrufen, wenn [`applies`].
pub(crate) fn develop(mut raw: rawler::RawImage, exif: Exif) -> Result<ImageF32> {
    use rawler::imgop::sensor::SensorType;
    use rawler::rawimage::{RawImageData, RawPhotometricInterpretation, WhiteLevel};

    let xyz_to_cam = xyz_to_cam(&raw)
        .ok_or_else(|| crate::error::Error::Raw("Kameramatrix unbrauchbar".into()))?;

    // --- rawprepare: Weißpunkt wie darktable (rawspeed)
    let (makes, models) = (
        [raw.camera.clean_make.as_str(), raw.camera.make.as_str()],
        [raw.camera.clean_model.as_str(), raw.camera.model.as_str()],
    );
    if let Some((_, white)) =
        super::cameras::sensor_levels(&makes, &models, &raw.camera.mode, exif.iso)
    {
        raw.whitelevel = WhiteLevel::new(vec![white]);
    }
    raw.apply_scaling()?;
    let data = match std::mem::replace(&mut raw.data, RawImageData::Float(Vec::new())) {
        RawImageData::Float(v) => v,
        other => other.as_f32().into_owned(),
    };
    let RawPhotometricInterpretation::Cfa(config) = &raw.photometric else {
        unreachable!("von applies geprüft");
    };
    let roi = raw.active_area.unwrap_or(rawler::imgop::Rect::new(
        rawler::imgop::Point::default(),
        rawler::imgop::Dim2::new(raw.width, raw.height),
    ));
    let (x0, y0, w, h) = (roi.p.x, roi.p.y, roi.width(), roi.height());
    let cfa = config.cfa.shift(x0, y0);
    let color = |row: usize, col: usize| cfa.color_at(row, col);

    // --- temperature: Kamera-Weißabgleich (Grün = 1), D65-Koeffizienten
    let d65 = color::d65_coeffs(&xyz_to_cam).map(|v| v as f32);
    let as_shot = if raw.wb_coeffs[..3].iter().all(|c| c.is_finite() && *c > 0.0) {
        let g = raw.wb_coeffs[1];
        [raw.wb_coeffs[0] / g, 1.0, raw.wb_coeffs[2] / g]
    } else {
        d65
    };
    let correction = [
        d65[0] / as_shot[0],
        d65[1] / as_shot[1],
        d65[2] / as_shot[2],
    ];

    // Aktiven Bereich ausschneiden und Weißabgleich anwenden
    let mut m: Vec<f32> = vec![0.0; w * h];
    m.par_chunks_mut(w).enumerate().for_each(|(row, out)| {
        let src = &data[(y0 + row) * raw.width + x0..][..w];
        for (col, (o, v)) in out.iter_mut().zip(src).enumerate() {
            *o = v * as_shot[color(row, col)];
        }
    });
    drop(data);

    // --- highlights: inpaint opposed
    super::opposed::process_opposed(&mut m, w, h, color, as_shot, correction);

    // --- demosaic
    let rgb: Vec<[f32; 3]> = if config.sensor == SensorType::Bayer {
        let pattern = [[color(0, 0), color(0, 1)], [color(1, 0), color(1, 1)]];
        let filters = super::rcd::filters_from_pattern(pattern);
        super::rcd::rcd_demosaic(&m, w, h, filters)
    } else {
        use rawler::imgop::sensor::Demosaic;
        let px = rawler::pixarray::PixF32::new_with(m, w, h);
        let out =
            rawler::imgop::sensor::xtrans::markesteijn::XTransMarkesteijnDemosaic::new_pass_1()
                .demosaic(&px, &cfa, &config.colors, px.rect());
        out.data
    };

    // --- exposure, colorin, color calibration, nach sRGB
    let bias = exif
        .exposure_bias
        .filter(|b| b.is_finite() && (-5.0..=5.0).contains(b))
        .unwrap_or(0.0);
    let exposure = SCENE_EXPOSURE - bias;
    let scale = 1.0 / ((-exposure).exp2() - SCENE_BLACK);
    let cam_to_xyz = color::camera_to_xyz(&xyz_to_cam)
        .ok_or_else(|| crate::error::Error::Raw("Kameramatrix unbrauchbar".into()))?;
    let xyz_to_work = color::inv(&color::rec2020_to_xyz()).expect("Rec.2020");
    let corr: M3 = [
        [correction[0] as f64, 0.0, 0.0],
        [0.0, correction[1] as f64, 0.0],
        [0.0, 0.0, correction[2] as f64],
    ];
    let cam_to_work = color::to_f32(&color::mul(&xyz_to_work, &color::mul(&cam_to_xyz, &corr)));
    let work_to_srgb = color::to_f32(&color::mul(
        &color::inv(&color::srgb_to_xyz()).expect("sRGB"),
        &color::rec2020_to_xyz(),
    ));
    let illuminant = color::illuminant_from_coeffs(&xyz_to_cam, as_shot)
        .map(|(x, y)| color::calibration_illuminant(x, y))
        .unwrap_or(color::D50_XY);
    let cal = color::Calibration::new(illuminant);
    let mut rgb = rgb;
    rgb.par_iter_mut().for_each(|p| {
        let e = p.map(|v| (v - SCENE_BLACK) * scale);
        let work = color::apply(&cam_to_work, e);
        let work = color::calibrate_pixel(&cal, work);
        let out = color::apply(&work_to_srgb, work);
        *p = out.map(|v| if v.is_finite() { v } else { 0.0 });
    });

    // Standard-Zuschnitt, angepasst an den bereits abgeschnittenen aktiven Bereich
    let mut img = rawler::pixarray::Color2D::new_with(rgb, w, h);
    if let Some(mut crop) = raw.crop_area.or(raw.active_area) {
        if let Some(active) = &raw.active_area {
            crop = crop.adapt(active);
        }
        if crop.d != img.dim() {
            img = img.crop(crop);
        }
    }
    Ok(ImageF32 {
        width: img.width,
        height: img.height,
        data: img.data,
    })
}
