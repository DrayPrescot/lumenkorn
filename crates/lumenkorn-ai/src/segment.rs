//! KI-Masken: Motiv (IS-Net), Objekt per Klick (Segment Anything) und Himmel.
//!
//! Alle Masken entstehen als Graustufenbild im Seitenverhältnis des Fotos
//! (längste Kante höchstens [`MASK_EDGE`]) und werden über normierte
//! Koordinaten in jeder Auflösung genutzt.

use crate::accel::Model;
use crate::error::{Error, Result};
use image::imageops::FilterType;
use image::{DynamicImage, GrayImage, Luma, RgbImage};
use lumenkorn_develop::{i18n::tr_s, tr};
use ort::value::Tensor;
use std::collections::VecDeque;
use std::path::Path;

/// Längste Kante gespeicherter Masken.
pub const MASK_EDGE: u32 = 1024;

fn open_session(path: &Path) -> Result<Model> {
    if !crate::runtime::is_loaded() {
        return Err(Error::RuntimeMissing);
    }
    if !path.is_file() {
        return Err(Error::ModelMissing(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        ));
    }
    crate::accel::session(path)
}

fn mask_size(w: u32, h: u32) -> (u32, u32) {
    let s = (MASK_EDGE as f32 / w.max(h) as f32).min(1.0);
    (
        ((w as f32 * s).round() as u32).max(1),
        ((h as f32 * s).round() as u32).max(1),
    )
}

// --- Motiv ----------------------------------------------------------------

/// Motiv-Erkennung mit IS-Net (DIS, „isnet-general-use“): 1024×1024 RGB rein,
/// Wahrscheinlichkeitskarte raus.
pub struct SubjectModel {
    session: Model,
    input: String,
    output: String,
}

impl SubjectModel {
    pub fn load(path: &Path) -> Result<Self> {
        let mut session = open_session(path)?;
        let s = session.inspect();
        let input = s
            .inputs()
            .first()
            .map(|i| i.name().to_string())
            .ok_or_else(|| Error::ModelFormat(tr_s("kein Eingang", "no input").into()))?;
        let output = s
            .outputs()
            .first()
            .map(|o| o.name().to_string())
            .ok_or_else(|| Error::ModelFormat(tr_s("kein Ausgang", "no output").into()))?;
        Ok(Self {
            session,
            input,
            output,
        })
    }

    pub fn predict(&self, img: &DynamicImage) -> Result<GrayImage> {
        const S: u32 = 1024;
        let rgb = img.resize_exact(S, S, FilterType::Lanczos3).to_rgb8();
        // Vorverarbeitung wie im Referenzcode: durch das Maximum teilen, −0,5.
        let max = rgb.as_raw().iter().copied().max().unwrap_or(255).max(1) as f32;
        let n = (S * S) as usize;
        let mut data = vec![0f32; 3 * n];
        for (i, p) in rgb.pixels().enumerate() {
            for c in 0..3 {
                data[c * n + i] = p[c] as f32 / max - 0.5;
            }
        }
        let input = Tensor::from_array(([1usize, 3, S as usize, S as usize], data))?;
        let mut session = self.session.lock();
        let outputs = session.run(vec![(self.input.as_str(), input.into_dyn())])?;
        let (_, pred) = outputs[self.output.as_str()].try_extract_tensor::<f32>()?;
        let (mi, ma) = pred
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        let range = (ma - mi).max(1e-6);
        let raw = GrayImage::from_fn(S, S, |x, y| {
            Luma([(((pred[(y * S + x) as usize] - mi) / range) * 255.0) as u8])
        });
        let (w, h) = mask_size(img.width(), img.height());
        Ok(image::imageops::resize(&raw, w, h, FilterType::Triangle))
    }
}

// --- Segment Anything -----------------------------------------------------

/// Bild-Embedding von SAM. Teuer (Sekunden) – einmal pro Bild berechnen und für
/// alle Klicks wiederverwenden.
pub struct SamEmbedding {
    data: Vec<f32>,
    /// Bildgröße, für die das Embedding berechnet wurde
    width: u32,
    height: u32,
    /// Skalierung auf die 1024er-Eingabe
    scale: f32,
}

pub struct Sam {
    encoder: Model,
    decoder: Model,
}

#[derive(Debug, Clone, Copy)]
pub struct Point {
    /// normiert 0…1
    pub x: f32,
    pub y: f32,
    pub positive: bool,
}

impl Sam {
    pub fn load(encoder: &Path, decoder: &Path) -> Result<Self> {
        Ok(Self {
            encoder: open_session(encoder)?,
            decoder: open_session(decoder)?,
        })
    }

    pub fn embed(&self, img: &DynamicImage) -> Result<SamEmbedding> {
        const S: u32 = 1024;
        let (w, h) = mask_size(img.width(), img.height());
        let scale = S as f32 / w.max(h) as f32;
        let (nw, nh) = (
            ((w as f32 * scale).round() as u32).min(S),
            ((h as f32 * scale).round() as u32).min(S),
        );
        let rgb = img.resize_exact(nw, nh, FilterType::Triangle).to_rgb8();
        const MEAN: [f32; 3] = [123.675, 116.28, 103.53];
        const STD: [f32; 3] = [58.395, 57.12, 57.375];
        let n = (S * S) as usize;
        // Rechts und unten mit Nullen auffüllen (entspricht dem Mittelwert nach Normierung).
        let mut data = vec![0f32; 3 * n];
        for (x, y, p) in rgb.enumerate_pixels() {
            let i = (y * S + x) as usize;
            for c in 0..3 {
                data[c * n + i] = (p[c] as f32 - MEAN[c]) / STD[c];
            }
        }
        let input = Tensor::from_array(([1usize, 3, S as usize, S as usize], data))?;
        let mut enc = self.encoder.lock();
        let name = enc.inputs()[0].name().to_string();
        let out_name = enc.outputs()[0].name().to_string();
        let outputs = enc.run(vec![(name.as_str(), input.into_dyn())])?;
        let (_, emb) = outputs[out_name.as_str()].try_extract_tensor::<f32>()?;
        Ok(SamEmbedding {
            data: emb.to_vec(),
            width: w,
            height: h,
            scale,
        })
    }

    /// Maske für die Klickpunkte. Liefert die Maske mit der höchsten vom Modell
    /// geschätzten Qualität.
    pub fn predict(&self, emb: &SamEmbedding, points: &[Point]) -> Result<GrayImage> {
        if points.is_empty() {
            return Err(Error::ModelFormat(tr_s("keine Punkte", "no points").into()));
        }
        let mut coords = Vec::with_capacity((points.len() + 1) * 2);
        let mut labels = Vec::with_capacity(points.len() + 1);
        for p in points {
            coords.push(p.x * emb.width as f32 * emb.scale);
            coords.push(p.y * emb.height as f32 * emb.scale);
            labels.push(if p.positive { 1.0f32 } else { 0.0 });
        }
        // Füllpunkt, wie es der ONNX-Export ohne Rahmen erwartet
        coords.extend([0.0, 0.0]);
        labels.push(-1.0);
        let n = labels.len();

        let inputs = vec![
            (
                "image_embeddings",
                Tensor::from_array(([1usize, 256, 64, 64], emb.data.clone()))?.into_dyn(),
            ),
            (
                "point_coords",
                Tensor::from_array(([1usize, n, 2], coords))?.into_dyn(),
            ),
            (
                "point_labels",
                Tensor::from_array(([1usize, n], labels))?.into_dyn(),
            ),
            (
                "mask_input",
                Tensor::from_array(([1usize, 1, 256, 256], vec![0f32; 256 * 256]))?.into_dyn(),
            ),
            (
                "has_mask_input",
                Tensor::from_array(([1usize], vec![0f32]))?.into_dyn(),
            ),
            (
                "orig_im_size",
                Tensor::from_array(([2usize], vec![emb.height as f32, emb.width as f32]))?
                    .into_dyn(),
            ),
        ];
        let mut dec = self.decoder.lock();
        let outputs = dec.run(inputs)?;
        let (shape, masks) = outputs["masks"].try_extract_tensor::<f32>()?;
        let (_, iou) = outputs["iou_predictions"].try_extract_tensor::<f32>()?;
        let count = shape.get(1).copied().unwrap_or(1).max(1) as usize;
        let best = (0..count.min(iou.len()))
            .max_by(|a, b| iou[*a].total_cmp(&iou[*b]))
            .unwrap_or(0);
        let (w, h) = (emb.width, emb.height);
        let plane = (w * h) as usize;
        if masks.len() < (best + 1) * plane {
            return Err(Error::ModelFormat(tr!(
                "unerwartete Maskenform {shape:?}",
                "unexpected mask shape {shape:?}"
            )));
        }
        let m = &masks[best * plane..(best + 1) * plane];
        // Logits → weiche, aber schmale Kante um 0; Reste weit außerhalb auf 0
        let mut out = GrayImage::from_fn(w, h, |x, y| {
            let v = m[(y * w + x) as usize];
            let a = 1.0 / (1.0 + (-v * 4.0).exp());
            Luma([if a < 0.03 { 0 } else { (a * 255.0) as u8 }])
        });
        fix_border(&mut out);
        Ok(out)
    }
}

// --- Himmel ---------------------------------------------------------------

/// Himmel finden: Bereichswachstum von hellen, glatten Flächen am oberen Rand,
/// eingeschlossene Löcher (Sonne, Mond) werden gefüllt. Steht SAM zur Verfügung,
/// verfeinert es die Kante – die Heuristik liefert ihm die Klickpunkte.
pub fn sky_mask(img: &DynamicImage, sam: Option<(&Sam, &SamEmbedding)>) -> Result<GrayImage> {
    const WORK: u32 = 256;
    let small = img.resize(WORK, WORK, FilterType::Triangle).to_rgb8();
    let region = sky_region(&small);
    let (sw, sh) = small.dimensions();
    let area = region.iter().filter(|v| **v).count() as f32 / (sw * sh) as f32;
    let (w, h) = mask_size(img.width(), img.height());
    if area < 0.02 {
        return Ok(GrayImage::new(w, h));
    }

    let heuristic = GrayImage::from_fn(sw, sh, |x, y| {
        Luma([if region[(y * sw + x) as usize] {
            255
        } else {
            0
        }])
    });

    if let Some((sam, emb)) = sam {
        let points = sky_prompts(&region, sw, sh);
        if !points.is_empty()
            && let Ok(m) = sam.predict(emb, &points)
        {
            let refined = image::imageops::resize(&m, sw, sh, FilterType::Triangle);
            let iou = iou(&refined, &heuristic);
            log::info!("Himmel: SAM-Übereinstimmung mit Heuristik {iou:.2}");
            if iou > 0.4 {
                // Eingeschlossene Löcher (Sonne, Mond) wie bei der Heuristik füllen
                let bin: Vec<bool> = refined.pixels().map(|p| p[0] > 127).collect();
                let filled = fill_holes(&bin, sw as usize, sh as usize);
                let holes = GrayImage::from_fn(sw, sh, |x, y| {
                    let i = (y * sw + x) as usize;
                    Luma([if filled[i] && !bin[i] { 255 } else { 0 }])
                });
                let holes = image::imageops::resize(
                    &image::imageops::blur(&holes, 0.8),
                    w,
                    h,
                    FilterType::Triangle,
                );
                let mut out = image::imageops::resize(&m, w, h, FilterType::Triangle);
                for (o, hpx) in out.pixels_mut().zip(holes.pixels()) {
                    o[0] = o[0].max(hpx[0]);
                }
                return Ok(out);
            }
        }
    }
    // Leicht weichzeichnen, damit die Kante nicht treppig wirkt.
    let soft = image::imageops::blur(&heuristic, 1.0);
    Ok(image::imageops::resize(&soft, w, h, FilterType::Triangle))
}

fn sky_region(img: &RgbImage) -> Vec<bool> {
    let (w, h) = img.dimensions();
    let (w, h) = (w as usize, h as usize);
    let px = |x: usize, y: usize| {
        let p = img.get_pixel(x as u32, y as u32);
        [
            p[0] as f32 / 255.0,
            p[1] as f32 / 255.0,
            p[2] as f32 / 255.0,
        ]
    };
    let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let lum: Vec<f32> = (0..w * h).map(|i| luma(px(i % w, i / w))).collect();
    let grad = |x: usize, y: usize| {
        let l = |dx: isize, dy: isize| {
            let xx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
            let yy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
            lum[yy * w + xx]
        };
        ((l(1, 0) - l(-1, 0)).powi(2) + (l(0, 1) - l(0, -1)).powi(2)).sqrt()
    };
    let mut sorted = lum.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let median = sorted[sorted.len() / 2];

    let candidate = |x: usize, y: usize| {
        let c = px(x, y);
        let green = c[1] > c[0] * 1.15 && c[1] > c[2] * 1.15;
        // Dunkle, kräftig rote/braune Flächen sind praktisch nie Himmel.
        let earthy = c[0] > c[2] * 1.4 && lum[y * w + x] < 0.45;
        lum[y * w + x] >= (median * 0.9).min(0.6) && grad(x, y) < 0.08 && !green && !earthy
    };

    let mut region = vec![false; w * h];
    let mut queue = VecDeque::new();
    for x in 0..w {
        for y in 0..(h / 12).max(1) {
            if candidate(x, y) && !region[y * w + x] {
                region[y * w + x] = true;
                queue.push_back((x, y));
            }
        }
    }
    while let Some((x, y)) = queue.pop_front() {
        let c = px(x, y);
        for (dx, dy) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (x as isize + dx, y as isize + dy);
            if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            if region[ny * w + nx] {
                continue;
            }
            let n = px(nx, ny);
            let diff =
                ((n[0] - c[0]).powi(2) + (n[1] - c[1]).powi(2) + (n[2] - c[2]).powi(2)).sqrt();
            if diff < 0.06 && grad(nx, ny) < 0.1 && lum[ny * w + nx] > median * 0.6 {
                region[ny * w + nx] = true;
                queue.push_back((nx, ny));
            }
        }
    }

    fill_holes(&region, w, h)
}

/// Löcher füllen: alles, was nicht zur Fläche gehört und keinen Weg zum
/// seitlichen oder unteren Rand hat, liegt im Himmel (Sonne, Mond, Vögel).
fn fill_holes(region: &[bool], w: usize, h: usize) -> Vec<bool> {
    let mut outside = vec![false; w * h];
    let mut q = VecDeque::new();
    for x in 0..w {
        q.push_back((x, h - 1));
    }
    for y in 0..h {
        q.push_back((0, y));
        q.push_back((w - 1, y));
    }
    while let Some((x, y)) = q.pop_front() {
        let i = y * w + x;
        if region[i] || outside[i] {
            continue;
        }
        outside[i] = true;
        if x > 0 {
            q.push_back((x - 1, y));
        }
        if x + 1 < w {
            q.push_back((x + 1, y));
        }
        if y > 0 {
            q.push_back((x, y - 1));
        }
        if y + 1 < h {
            q.push_back((x, y + 1));
        }
    }
    (0..w * h).map(|i| region[i] || !outside[i]).collect()
}

/// SAM unterschätzt Masken in den äußersten Pixelreihen; ohne Korrektur bleibt
/// am Bildrand eine helle Linie stehen. Die Randpixel übernehmen den Wert von
/// drei Pixeln weiter innen, falls der größer ist.
fn fix_border(m: &mut GrayImage) {
    let (w, h) = m.dimensions();
    if w < 8 || h < 8 {
        return;
    }
    for x in 0..w {
        for (edge, inner) in [
            (0, 3),
            (1, 3),
            (2, 3),
            (h - 1, h - 4),
            (h - 2, h - 4),
            (h - 3, h - 4),
        ] {
            let v = m.get_pixel(x, inner)[0].max(m.get_pixel(x, edge)[0]);
            m.put_pixel(x, edge, Luma([v]));
        }
    }
    for y in 0..h {
        for (edge, inner) in [
            (0, 3),
            (1, 3),
            (2, 3),
            (w - 1, w - 4),
            (w - 2, w - 4),
            (w - 3, w - 4),
        ] {
            let v = m.get_pixel(inner, y)[0].max(m.get_pixel(edge, y)[0]);
            m.put_pixel(edge, y, Luma([v]));
        }
    }
}

/// Klickpunkte für SAM: je Bilddrittel ein Punkt im Himmel, dazu Gegenpunkte unten.
fn sky_prompts(region: &[bool], w: u32, h: u32) -> Vec<Point> {
    let (w, h) = (w as usize, h as usize);
    let mut pts = Vec::new();
    for band in 0..3 {
        let x = w * (2 * band + 1) / 6;
        // höchster zusammenhängender Himmelspunkt in dieser Spalte, etwas nach unten versetzt
        let col: Vec<usize> = (0..h).filter(|y| region[y * w + x]).collect();
        if col.len() > h / 20 {
            let y = col[col.len() / 3];
            pts.push(Point {
                x: x as f32 / w as f32,
                y: y as f32 / h as f32,
                positive: true,
            });
        }
    }
    for x in [w / 3, 2 * w / 3] {
        if let Some(y) = (h * 3 / 4..h).rev().find(|y| !region[y * w + x]) {
            pts.push(Point {
                x: x as f32 / w as f32,
                y: y as f32 / h as f32,
                positive: false,
            });
        }
    }
    if pts.iter().any(|p| p.positive) {
        pts
    } else {
        vec![]
    }
}

fn iou(a: &GrayImage, b: &GrayImage) -> f32 {
    let (mut inter, mut union) = (0u32, 0u32);
    for (p, q) in a.pixels().zip(b.pixels()) {
        let (x, y) = (p[0] > 127, q[0] > 127);
        inter += (x && y) as u32;
        union += (x || y) as u32;
    }
    if union == 0 {
        0.0
    } else {
        inter as f32 / union as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    /// Himmel mit Verlauf und Sonne über einer dunklen Bergkette.
    fn landscape() -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(600, 400, |x, y| {
            let ridge = 220.0 + 40.0 * ((x as f32) / 60.0).sin();
            let sun = ((x as f32 - 450.0).powi(2) + (y as f32 - 80.0).powi(2)).sqrt() < 25.0;
            if (y as f32) > ridge {
                Rgb([25, 22, 30])
            } else if sun {
                Rgb([250, 240, 210])
            } else {
                let t = y as f32 / ridge;
                Rgb([
                    (110.0 + 60.0 * t) as u8,
                    (150.0 + 40.0 * t) as u8,
                    (220.0 - 20.0 * t) as u8,
                ])
            }
        }))
    }

    #[test]
    fn finds_sky_including_sun_and_excludes_mountains() {
        let m = sky_mask(&landscape(), None).unwrap();
        let (w, h) = m.dimensions();
        let at = |x: f32, y: f32| m.get_pixel((x * w as f32) as u32, (y * h as f32) as u32)[0];
        assert!(at(0.2, 0.1) > 200, "Himmel oben links");
        assert!(at(0.75, 0.2) > 200, "Sonne gehört zum Himmel");
        assert!(at(0.5, 0.9) < 30, "Berge unten nicht");
    }

    #[test]
    fn no_sky_in_a_closeup() {
        // Grob strukturierte Oberfläche (z. B. Holz, Stoff) in Blöcken, damit die
        // Struktur auch nach dem Verkleinern erhalten bleibt.
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(600, 400, |x, y| {
            let n = ((x / 8) * 7919 + (y / 8) * 104729) % 97;
            Rgb([(90 + n) as u8, (60 + n / 2) as u8, 40])
        }));
        let m = sky_mask(&img, None).unwrap();
        assert!(m.pixels().all(|p| p[0] == 0));
    }
}
