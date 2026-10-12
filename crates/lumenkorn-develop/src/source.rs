//! Quellbild laden: RAW entwickeln bzw. Rasterbild linearisieren.

use crate::error::{Error, Result};
use crate::geometry::apply_exif_orientation;
use crate::i18n::tr_s;
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
    /// Objektivkorrektur aus den Daten der Kamera (nur RAWs, falls vorhanden)
    pub lens: Option<std::sync::Arc<crate::lens::LensProfile>>,
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
                lens: None,
            });
        }
        // Volle Abmessungen (vor Verkleinerung); Rasterbilder setzen sie
        // selbst, weil sehr große schon beim Linearisieren verkleinert werden.
        let mut full = None;
        let mut lens = None;
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
            let size = img_size(&img, o);
            lens = crate::lens::LensProfile::read(path, o, size).map(std::sync::Arc::new);
            full = Some(size);
            // Erst verkleinern, dann aufrichten: Bei Hochformat drehte sonst
            // jede Vorschau das volle Bild (bis 720 MB Kopie)
            let img = match max_edge {
                Some(m) => img.into_downscaled(m),
                None => img,
            };
            (SourceKind::Raw, apply_exif_orientation(img, o))
        } else {
            let img = crate::raster::open(path)?;
            let o = orientation.unwrap_or(1);
            let (w, h) = (img.width() as usize, img.height() as usize);
            full = Some(if (5..=8).contains(&o) { (h, w) } else { (w, h) });
            let img = crate::raster::to_linear(img, max_edge);
            (SourceKind::Raster, apply_exif_orientation(img, o))
        };
        let (full_width, full_height) = full.unwrap_or((image.width, image.height));
        let image = match max_edge {
            Some(m) => image.into_downscaled(m),
            None => image,
        };
        Ok(Source {
            kind,
            image,
            full_width,
            full_height,
            lens,
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
            lens: self.lens.clone(),
        }
    }
}

/// Größe nach dem Aufrichten
fn img_size(img: &ImageF32, o: u16) -> (usize, usize) {
    if (5..=8).contains(&o) {
        (img.height, img.width)
    } else {
        (img.width, img.height)
    }
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
    // Erst prüfen, dann die eigene Pipeline das RawImage übernehmen lassen –
    // so braucht es keine Kopie für den Fall, dass doch rawler rechnen muss.
    let img = if own_pipeline_applies(&raw, method) {
        own_pipeline_owned(raw, method, true)?
    } else {
        rawler_pipeline(&raw)?
    };
    if img.width == 0 || img.height == 0 {
        return Err(Error::Raw(tr_s("leeres Bild", "empty image").into()));
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
                tr_s(
                    "Vierfarb-Sensor nach der Kalibrierung nicht unterstützt",
                    "Four-color sensor after calibration not supported",
                )
                .into(),
            ));
        }
    })
}

/// Bayer-Muster ab dem aktiven Bereich und dieser Bereich (x, y, Breite, Höhe).
fn bayer_layout(
    raw: &rawler::RawImage,
    config: &rawler::rawimage::CFAConfig,
) -> ([[usize; 2]; 2], (usize, usize, usize, usize)) {
    let roi = raw.active_area.unwrap_or(rawler::imgop::Rect::new(
        rawler::imgop::Point::default(),
        rawler::imgop::Dim2::new(raw.width, raw.height),
    ));
    let cfa = config.cfa.shift(roi.p.x, roi.p.y);
    let pattern = [
        [cfa.color_at(0, 0), cfa.color_at(0, 1)],
        [cfa.color_at(1, 0), cfa.color_at(1, 1)],
    ];
    (pattern, (roi.p.x, roi.p.y, roi.width(), roi.height()))
}

/// Kann die eigene Pipeline das RAW rechnen? Sonderfälle (Fuji SuperCCD,
/// Vierfarb-Sensoren, Monochrom, untypische Muster) übernimmt rawler.
/// Alle Ausschlüsse stehen hier, vor jeder Umrechnung der Sensordaten.
fn own_pipeline_applies(raw: &rawler::RawImage, method: DemosaicMethod) -> bool {
    use rawler::imgop::sensor::SensorType;
    use rawler::rawimage::RawPhotometricInterpretation;

    let RawPhotometricInterpretation::Cfa(config) = &raw.photometric else {
        return false;
    };
    if raw.cpp != 1 || !config.cfa.is_rgb() || raw.fuji_rotation_width.is_some() {
        return false;
    }
    let bayer = config.sensor == SensorType::Bayer;
    if !bayer && config.sensor != SensorType::Xtrans {
        return false;
    }
    if bayer && method == DemosaicMethod::Rcd {
        let (pattern, (_, _, w, h)) = bayer_layout(raw, config);
        if pattern.iter().flatten().any(|c| *c > 2) || w < 32 || h < 32 {
            return false;
        }
    }
    true
}

/// Eigene Pipeline auf einer Kopie (Vergleichstest gegen rawler).
#[cfg(test)]
fn own_pipeline(raw: &rawler::RawImage, method: DemosaicMethod) -> Result<Option<ImageF32>> {
    if !own_pipeline_applies(raw, method) {
        return Ok(None);
    }
    // Ohne Lichter-Rekonstruktion: rawler kennt sie nicht, verglichen wird
    // der Rest (Farbumrechnung, Zuschnitt, Demosaicing)
    own_pipeline_owned(raw.clone(), method, false).map(Some)
}

/// Eigenes Demosaicing für RGB-Bayer (RCD) und X-Trans (Markesteijn, 3 Durchgänge;
/// rawler nimmt dort nur bilinear). Farbumrechnung und Standard-Zuschnitt wie in
/// rawler (`develop_intermediate`). Nur aufrufen, wenn [`own_pipeline_applies`].
/// `unclip`: gesättigte Kanäle rekonstruieren (siehe [`unclip`]).
fn own_pipeline_owned(
    mut raw: rawler::RawImage,
    method: DemosaicMethod,
    unclip: bool,
) -> Result<ImageF32> {
    use rawler::imgop::sensor::{Demosaic, SensorType};
    use rawler::pixarray::{Color2D, PixF32};
    use rawler::rawimage::{RawImageData, RawPhotometricInterpretation};

    raw.apply_scaling()?;
    // Die skalierten Werte übernehmen statt kopieren (`as_f32().into_owned()`
    // kopierte bei Float-Daten das ganze Sensorbild noch einmal).
    let data = match std::mem::replace(&mut raw.data, RawImageData::Float(Vec::new())) {
        RawImageData::Float(v) => v,
        other => other.as_f32().into_owned(),
    };
    let pixels = PixF32::new_with(data, raw.width, raw.height);
    let RawPhotometricInterpretation::Cfa(config) = &raw.photometric else {
        unreachable!("von own_pipeline_applies geprüft");
    };
    let bayer = config.sensor == SensorType::Bayer;
    let roi = raw.active_area.unwrap_or(pixels.rect());
    // Fehlt ein Koeffizient (NaN, 0), würde der Kanal ausfallen
    let wb = if raw.wb_coeffs[..3].iter().all(|c| c.is_finite() && *c > 0.0) {
        raw.wb_coeffs
    } else {
        [1.0; 4]
    };

    // Sensordaten nach dem Demosaicing gleich freigeben (Spitzenspeicher)
    let rgb: Color2D<f32, 3> = if bayer && method == DemosaicMethod::Ppg {
        let rgb = rawler::imgop::sensor::bayer::ppg::PPGDemosaic::new().demosaic(
            &pixels,
            &config.cfa,
            &config.colors,
            roi,
        );
        drop(pixels);
        rgb
    } else if bayer {
        let (pattern, _) = bayer_layout(&raw, config);
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
        drop(m);
        out.par_iter_mut().for_each(|p| {
            for c in 0..3 {
                p[c] = p[c].max(0.0) / wb[c];
            }
        });
        Color2D::new_with(out, roi.width(), roi.height())
    } else {
        let rgb =
            rawler::imgop::sensor::xtrans::markesteijn::XTransMarkesteijnDemosaic::new_pass_3()
                .demosaic(&pixels, &config.cfa, &config.colors, roi);
        drop(pixels);
        rgb
    };

    let mut rgb = rgb;
    if unclip {
        reconstruct_highlights(&mut rgb.data, rgb.width, &wb);
    }
    // Liefert bereits bereinigte Werte (`sanitize`) – kein eigener Durchgang
    let rgb = calibrate(&raw, rgb, &wb);

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
    Ok(ImageF32 {
        width: img.width,
        height: img.height,
        data: img.data,
    })
}

/// Kamera-RGB → lineares Rec.709 mit Weißabgleich, wie rawler (`Calibrate`).
fn calibrate(
    raw: &rawler::RawImage,
    mut img: rawler::pixarray::Color2D<f32, 3>,
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
    // An Ort und Stelle: keine zweite Kopie des RGB-Bildes (24 MP: 290 MB)
    img.data.par_iter_mut().for_each(|p| {
        let (r, g, b) = (p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]);
        let out = [
            cam2rgb[0][0] * r + cam2rgb[0][1] * g + cam2rgb[0][2] * b,
            cam2rgb[1][0] * r + cam2rgb[1][1] * g + cam2rgb[1][2] * b,
            cam2rgb[2][0] * r + cam2rgb[2][1] * g + cam2rgb[2][2] * b,
        ];
        *p = sanitize(rawler::imgop::raw::clip_euclidean_norm_avg(&out));
    });
    img
}

/// Ab hier (Anteil des Weißpunkts nach `apply_scaling`, hellster Kanal) gilt
/// ein Pixel als gesättigt; bis `CLIP_AT` weicher Übergang. Sony sättigt etwas
/// über dem angegebenen Weißpunkt (14-Bit-Maximum, ~1,07) – alles ab 1,0 ist
/// Sättigung.
const CLIP_FROM: f32 = 0.94;
const CLIP_AT: f32 = 1.0;
/// Zellen über die längste Kante, in denen die Farbe der Umgebung gesammelt
/// wird (Zellgröße also relativ zum Bild – Vorschau und Export gleich)
const CLIP_CELLS: usize = 300;
/// Nach so vielen Zellen Abstand zur nächsten nicht gesättigten Stelle ist die
/// übertragene Farbe ganz neutral: Das Innere großer ausgefressener Flächen
/// (weiße Wand) bleibt weiß, statt Farbe von weit her zu übernehmen.
const CLIP_REACH: f32 = 10.0;
/// Nur fast gesättigte Pixel (Anteil des Weißpunkts) tragen zur Farbe bei: Das
/// ist meist der Rand desselben Objekts (Haut neben dem Sonnenfleck) – nicht
/// ein dunkleres daneben (Dach, rosa Papier vor einer weißen Wand).
const CLIP_SOURCE_MIN: f32 = 0.8;

/// Helligkeit im Kamera-RGB (nach Weißabgleich)
#[inline]
fn cam_luma(v: [f32; 3]) -> f32 {
    0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2]
}

#[inline]
fn clip_weight(p: &[f32; 3]) -> f32 {
    crate::color::smoothstep(CLIP_FROM, CLIP_AT, p[0].max(p[1]).max(p[2]))
}

/// Gesättigte Lichter rekonstruieren (Kamera-RGB vor dem Weißabgleich).
///
/// Ein Kanal am Sensor-Maximum kennt seinen wahren Wert nicht. Der
/// Weißabgleich hebt Rot und Blau (Sony etwa ×2,4 und ×1,6) über ein
/// abgeschnittenes Grün – ausgefressene weiße Flächen wurden magenta.
///
/// - Teilweise gesättigt (Haut in der Sonne: Grün voll, Rot und Blau nicht):
///   Der gesättigte Kanal wird aus den gemessenen anderen geschätzt, über das
///   Farbverhältnis der fast gesättigten Umgebung („Farbübertragung“ wie in
///   RawTherapee und darktable). Haut bleibt hautfarben.
/// - Alle drei Kanäle gesättigt: Es gibt keine Farbinformation – neutral bei
///   der Helligkeit des hellsten Kanals, wie die Kamera ausgefressene Flächen
///   zeichnet.
///   Eine weiße Wand bleibt weiß, statt Farbe vom Nachbarobjekt zu borgen.
fn reconstruct_highlights(data: &mut [[f32; 3]], w: usize, wb: &[f32; 4]) {
    if w == 0 || !data.par_iter().any(|p| clip_weight(p) > 0.0) {
        return;
    }
    let h = data.len() / w;
    let cell = (w.max(h) / CLIP_CELLS).max(4);
    let (gw, gh) = (w.div_ceil(cell), h.div_ceil(cell));
    let wbv = |p: &[f32; 3]| [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]];
    // Farbe (Verhältnis zur Helligkeit) der hellen, nicht gesättigten Pixel je
    // Zelle, nach Helligkeit² gewichtet – die hellsten liegen am Rand der Lichter
    let cells: Vec<[f32; 4]> = (0..gh)
        .into_par_iter()
        .flat_map_iter(|gy| {
            let mut acc = vec![[0f32; 4]; gw];
            for y in gy * cell..((gy + 1) * cell).min(h) {
                for (x, p) in data[y * w..(y + 1) * w].iter().enumerate() {
                    if clip_weight(p) > 0.0 || p[0].max(p[1]).max(p[2]) < CLIP_SOURCE_MIN {
                        continue;
                    }
                    let v = wbv(p);
                    let l = cam_luma(v);
                    let a = &mut acc[x / cell];
                    for c in 0..3 {
                        a[c] += v[c] * l; // Gewicht l² · Verhältnis v/l
                    }
                    a[3] += l * l;
                }
            }
            acc
        })
        .collect();
    // (Farbe, Abstand in Zellen zur nächsten Zelle mit eigener Farbe)
    let mut grid: Vec<Option<([f32; 3], f32)>> = cells
        .iter()
        .map(|a| (a[3] > 0.0).then(|| ([a[0] / a[3], a[1] / a[3], a[2] / a[3]], 0.0)))
        .collect();
    // Ganz gesättigte Zellen von außen nach innen mit dem Mittel der
    // Nachbarn füllen
    const NEIGHBORS: [(isize, isize); 8] = [
        (-1, 0),
        (1, 0),
        (0, -1),
        (0, 1),
        (-1, -1),
        (1, -1),
        (-1, 1),
        (1, 1),
    ];
    loop {
        let snapshot = grid.clone();
        let mut changed = false;
        for gy in 0..gh {
            for gx in 0..gw {
                if snapshot[gy * gw + gx].is_some() {
                    continue;
                }
                let mut sum = [0f32; 3];
                let mut dist = f32::MAX;
                let mut n = 0.0;
                for (dx, dy) in NEIGHBORS {
                    let (nx, ny) = (gx as isize + dx, gy as isize + dy);
                    if nx < 0 || ny < 0 || nx >= gw as isize || ny >= gh as isize {
                        continue;
                    }
                    if let Some((r, d)) = snapshot[ny as usize * gw + nx as usize] {
                        for c in 0..3 {
                            sum[c] += r[c];
                        }
                        dist = dist.min(d);
                        n += 1.0;
                    }
                }
                if n > 0.0 {
                    grid[gy * gw + gx] = Some((sum.map(|v| v / n), dist + 1.0));
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    // Mit dem Abstand neutraler (Verhältnis 1 : 1 : 1); ohne jede Umgebung
    // (ganzes Bild gesättigt) ganz neutral
    let grid: Vec<[f32; 3]> = grid
        .into_iter()
        .map(|g| match g {
            Some((r, d)) => {
                let t = (d / CLIP_REACH).min(1.0);
                r.map(|v| v + (1.0 - v) * t)
            }
            None => [1.0; 3],
        })
        .collect();
    let at = |gx: usize, gy: usize| grid[gy.min(gh - 1) * gw + gx.min(gw - 1)];
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let fy = ((y as f32 + 0.5) / cell as f32 - 0.5).max(0.0);
        let (y0, ty) = (fy as usize, fy.fract());
        for (x, p) in row.iter_mut().enumerate() {
            if clip_weight(p) <= 0.0 {
                continue;
            }
            let fx = ((x as f32 + 0.5) / cell as f32 - 0.5).max(0.0);
            let (x0, tx) = (fx as usize, fx.fract());
            let r: [f32; 3] = std::array::from_fn(|c| {
                let top = at(x0, y0)[c] + (at(x0 + 1, y0)[c] - at(x0, y0)[c]) * tx;
                let bot = at(x0, y0 + 1)[c] + (at(x0 + 1, y0 + 1)[c] - at(x0, y0 + 1)[c]) * tx;
                top + (bot - top) * ty
            });
            let v = wbv(p);
            let wc = p.map(|x| crate::color::smoothstep(CLIP_FROM, CLIP_AT, x));
            // Gesättigte Kanäle aus den nicht gesättigten schätzen – über das
            // Farbverhältnis der Umgebung. Die nicht gesättigten sind echte
            // Messwerte und bleiben; am Rand der Lichter fallen Schätzung und
            // Messung zusammen (keine Sprünge).
            let mut out = v;
            let free: f32 = wc.iter().map(|w| 1.0 - w).sum();
            if free > 1e-3 {
                for c in 0..3 {
                    if wc[c] <= 0.0 {
                        continue;
                    }
                    let est = (0..3)
                        .filter(|&j| j != c)
                        .map(|j| (1.0 - wc[j]) * v[j] * r[c] / r[j].max(1e-6))
                        .sum::<f32>()
                        / (free - (1.0 - wc[c])).max(1e-3);
                    // Gesättigt heißt: mindestens so hell wie gemessen
                    out[c] = v[c] + (est.max(v[c]) - v[c]) * wc[c];
                }
            }
            // Alle drei Kanäle gesättigt: keine Farbinformation – neutral (wie
            // die Kamera ausgefressene Flächen zeigt), und mindestens so hell
            // wie der hellste Kanal. Weiß sättigt nacheinander Grün, Blau, Rot;
            // so schließt die Fläche ohne Sprung an die Zone an, in der nur
            // Rot noch gemessen ist.
            let all = wc[0].min(wc[1]).min(wc[2]);
            if all > 0.0 {
                let m = out[0].max(out[1]).max(out[2]);
                out = out.map(|x| x + (m - x) * all);
            }
            for c in 0..3 {
                p[c] = out[c] / wb[c];
            }
        }
    });
}

#[inline]
fn sanitize(p: [f32; 3]) -> [f32; 3] {
    let f = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
    [f(p[0]), f(p[1]), f(p[2])]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipped_highlights_take_the_color_of_their_surroundings() {
        // Sony: Rot ×2,43, Blau ×1,56
        let wb = [2.43, 1.0, 1.56, f32::NAN];
        let (w, h) = (96usize, 96usize);
        // Fleck von 24 px (6 Zellen) – wie ein Sonnenfleck auf der Wange
        let in_spot = |x: usize, y: usize| (36..60).contains(&x) && (36..60).contains(&y);
        // Haut (Kamera-RGB nach Weißabgleich etwa 1 : 0,72 : 0,55), in der
        // Mitte ein Sonnenfleck mit gesättigtem Grün und Rot
        let skin = [1.0f32, 0.72, 0.55];
        let mut data: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                // Ringsum helle Haut knapp unter der Sättigung (Grün ~86 %),
                // im Fleck nur Grün gesättigt (Rot ~66 %, Blau ~56 %)
                let k = if in_spot(x, y) { 1.6 } else { 1.2 };
                std::array::from_fn(|c| (skin[c] * k / wb[c]).min(1.05))
            })
            .collect();
        let before = data.clone();
        reconstruct_highlights(&mut data, w, &wb);
        let v = |p: [f32; 3]| [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]];
        let c = v(data[48 * w + 48]);
        // Farbton der Haut ringsum (G/R 0,72, B/R 0,55) statt magenta (G/R
        // 0,66 gemessen) oder gelb (G/R ~1); zur Mitte hin etwas neutraler
        let (gr, br) = (c[1] / c[0], c[2] / c[0]);
        assert!(gr > 0.70 && gr < 0.85 && (br - 0.55).abs() < 0.01, "{c:?}");
        // Gemessene Kanäle (Rot, Blau) bleiben, Grün wird nur heller
        let (m, b) = (data[48 * w + 48], before[48 * w + 48]);
        assert!(m[0] == b[0] && m[2] == b[2] && m[1] >= b[1], "{m:?} {b:?}");
        // Nicht gesättigte Pixel bleiben unverändert
        assert_eq!(data[2 * w + 2], before[2 * w + 2]);

        // Großer gesättigter Bereich mit farbigem Rand: innen neutral
        let (bw, bh) = (400usize, 400usize);
        let mut big: Vec<[f32; 3]> = (0..bw * bh)
            .map(|i| {
                if i % bw < 8 {
                    [0.9 / wb[0], 0.3, 0.3 / wb[2]] // kräftig rot, hell
                } else {
                    [1.069; 3]
                }
            })
            .collect();
        reconstruct_highlights(&mut big, bw, &wb);
        let c = v(big[200 * bw + 300]);
        assert!(
            (c[0] - c[1]).abs() < 1e-3 && (c[1] - c[2]).abs() < 1e-3,
            "{c:?}"
        );

        // Ganz gesättigt ohne farbige Umgebung (weiße Wand): neutral statt magenta
        let mut wall = vec![[1.069f32; 3]; 32 * 32];
        reconstruct_highlights(&mut wall, 32, &wb);
        let c = v(wall[16 * 32 + 16]);
        assert!(
            (c[0] - c[1]).abs() < 1e-3 && (c[1] - c[2]).abs() < 1e-3,
            "{c:?}"
        );
    }

    /// Eigene Pipeline mit rawlers PPG ergibt exakt rawlers Ergebnis (Farb-
    /// umrechnung, Zuschnitt), RCD weicht nur im Detail ab. Braucht echte RAWs:
    /// `LUMENKORN_TEST_RAW=/ordner/mit/raws` (wie `tests/raw.rs`; die CI lädt
    /// Beispiel-RAWs von sechs Kameras).
    #[test]
    fn own_pipeline_matches_rawler() {
        let Ok(dir) = std::env::var("LUMENKORN_TEST_RAW") else {
            eprintln!("übersprungen: LUMENKORN_TEST_RAW nicht gesetzt");
            return;
        };
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .is_some_and(|e| !e.eq_ignore_ascii_case("jpg"))
            })
            .collect();
        files.sort();
        for f in files {
            let f = f.display().to_string();
            let raw = rawler::decode_file(Path::new(&f)).unwrap();
            let theirs = rawler_pipeline(&raw).unwrap();
            let Some(ours) = own_pipeline(&raw, DemosaicMethod::Ppg).unwrap() else {
                // Sonderfälle (Fuji SuperCCD, Vierfarb-Sensoren) rechnet rawler
                eprintln!("{f}: Sonderfall, rawler-Pipeline");
                continue;
            };
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

            // RCD: gleiche Abmessungen und Farben wie rawler, Unterschiede nur im
            // Detail (Kanten, Rauschen) – fällt die Ähnlichkeit, stimmt das
            // Farbmuster oder die Weißabgleich-Zuordnung nicht mehr.
            let rcd = own_pipeline(&raw, DemosaicMethod::Rcd)
                .unwrap()
                .expect("eigene Pipeline");
            assert_eq!(
                (rcd.width, rcd.height),
                (theirs.width, theirs.height),
                "{f}"
            );
            let enc = |v: f32| v.clamp(0.0, 1.0).sqrt();
            let mse = rcd
                .data
                .iter()
                .zip(&theirs.data)
                .flat_map(|(a, b)| (0..3).map(move |c| ((enc(a[c]) - enc(b[c])) as f64).powi(2)))
                .sum::<f64>()
                / (rcd.data.len() * 3) as f64;
            let psnr = 10.0 * (1.0 / mse.max(1e-12)).log10();
            eprintln!("{f}: RCD gegenüber PPG {psnr:.1} dB");
            assert!(psnr > 30.0, "{f}: RCD weicht zu stark ab ({psnr:.1} dB)");
        }
    }
}
