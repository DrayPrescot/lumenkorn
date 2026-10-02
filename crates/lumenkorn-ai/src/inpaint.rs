#![allow(clippy::needless_range_loop)] // Farbkanäle: Index über drei feste Felder ist hier klarer
//! KI-Reparatur mit LaMa („Resolution-robust Large Mask Inpainting“,
//! Apache-2.0, ONNX-Fassung aus dem OpenCV Model Zoo).
//!
//! LaMa hat einen festen Eingang von 512×512. Damit Reparaturen auch in
//! großen Fotos scharf bleiben, wird jeder zusammenhängende Bereich der
//! Auswahl einzeln bearbeitet: ein Ausschnitt mit Umgebung (mindestens
//! 512 px, sonst etwa das 2,2-Fache des Bereichs) wird auf 512 skaliert,
//! gefüllt, zurückskaliert und nur innerhalb der leicht erweiterten, weich
//! auslaufenden Auswahl eingesetzt.

use crate::error::{Error, Result};
use lumenkorn_develop::ImageF32;
use lumenkorn_develop::repair::{Patch, PatchPart};
use ort::session::Session;
use ort::value::Tensor;
use rayon::prelude::*;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub const MODEL_FILE: &str = "lama.onnx";
const S: usize = 512;
/// Rastergröße für die Suche nach zusammenhängenden Bereichen
const CELL: usize = 32;

pub struct Inpainter {
    session: Mutex<Session>,
}

fn srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

impl Inpainter {
    pub fn load(path: &Path) -> Result<Self> {
        if !crate::runtime::is_loaded() {
            return Err(Error::RuntimeMissing);
        }
        if !path.is_file() {
            return Err(Error::ModelMissing(MODEL_FILE.into()));
        }
        Ok(Self {
            session: Mutex::new(crate::accel::session(path)?),
        })
    }

    /// Füllt ein 512×512-Bild (sRGB 0…1, RGB, HWC) innerhalb der Maske (0/1).
    pub fn run_512(&self, rgb: &[[f32; 3]], mask: &[f32]) -> Result<Vec<[f32; 3]>> {
        let n = S * S;
        let mut img = vec![0f32; 3 * n];
        for (i, (p, m)) in rgb.iter().zip(mask).enumerate() {
            let keep = 1.0 - m;
            for c in 0..3 {
                img[c * n + i] = p[c] * keep;
            }
        }
        let image = Tensor::from_array(([1usize, 3, S, S], img))?;
        let mask = Tensor::from_array(([1usize, 1, S, S], mask.to_vec()))?;
        let mut s = self.session.lock().unwrap();
        let out = s.run(vec![("image", image.into_dyn()), ("mask", mask.into_dyn())])?;
        let (_, v) = out[0].try_extract_tensor::<f32>()?;
        if v.len() != 3 * n {
            return Err(Error::ModelFormat(format!(
                "LaMa liefert {} Werte",
                v.len()
            )));
        }
        Ok((0..n)
            .map(|i| [0, 1, 2].map(|c| (v[c * n + i] / 255.0).clamp(0.0, 1.0)))
            .collect())
    }

    /// Repariert die Bereiche der Maske (Deckkraft je Pixel, Größe wie `img`).
    /// `progress(fertig, gesamt)` je Bereich.
    pub fn repair(
        &self,
        img: &ImageF32,
        alpha: &[f32],
        progress: &dyn Fn(usize, usize),
        cancel: &AtomicBool,
    ) -> Result<Patch> {
        let (w, h) = (img.width, img.height);
        let long = w.max(h);
        let sel: Vec<bool> = alpha.par_iter().map(|a| *a > 0.3).collect();
        if !sel.iter().any(|b| *b) {
            return Err(Error::ModelFormat("Die Auswahl ist leer".into()));
        }
        // Etwas erweitern: Schatten und Säume am Rand gehören meist dazu.
        let grow = (long / 400).max(3);
        let dil = dilate(&sel, w, h, grow);
        let feather = (long / 800).max(2);
        let regions = regions(&dil, w, h);

        let mut work = img.clone();
        let mut parts = Vec::new();
        for (k, (x0, y0, x1, y1)) in regions.iter().copied().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            progress(k, regions.len());
            // Ausschnitt mit Umgebung, möglichst quadratisch
            let (bw, bh) = (x1 - x0, y1 - y0);
            let side = ((bw.max(bh) as f32 * 2.2) as usize).max(S);
            let cw = side.min(w);
            let ch = side.min(h);
            let cx = ((x0 + x1) / 2).saturating_sub(cw / 2).min(w - cw);
            let cy = ((y0 + y1) / 2).saturating_sub(ch / 2).min(h - ch);

            // Belichtung so wählen, dass Lichter (RAW > 1) nicht abgeschnitten werden
            let mut lum: Vec<f32> = (cy..cy + ch)
                .step_by(4)
                .flat_map(|y| {
                    let work = &work;
                    (cx..cx + cw)
                        .step_by(4)
                        .map(move |x| work.get(x, y).iter().fold(0f32, |m, v| m.max(*v)))
                })
                .collect();
            let gain = if lum.is_empty() {
                1.0
            } else {
                let i = (lum.len() * 999 / 1000).min(lum.len() - 1);
                let p = *lum.select_nth_unstable_by(i, f32::total_cmp).1;
                1.0 / p.max(1.0)
            };

            // Auf 512×512 bringen (Flächenmittel bzw. bilinear)
            let rgb = resample(
                &|x, y| work.get(cx + x, cy + y).map(|v| srgb(v * gain)),
                cw,
                ch,
                S,
                S,
            );
            let m = resample(
                &|x, y| {
                    let v = dil[(cy + y) * w + cx + x] as u8 as f32;
                    [v, 0.0, 0.0]
                },
                cw,
                ch,
                S,
                S,
            );
            let mask: Vec<f32> = m
                .iter()
                .map(|v| if v[0] > 0.0 { 1.0 } else { 0.0 })
                .collect();
            let out = self.run_512(&rgb, &mask)?;

            // Teil: Bereich plus weicher Rand, Ergebnis zurückskaliert
            let px0 = x0.saturating_sub(feather).max(cx);
            let py0 = y0.saturating_sub(feather).max(cy);
            let px1 = (x1 + feather).min(cx + cw);
            let py1 = (y1 + feather).min(cy + ch);
            let (pw, ph) = (px1 - px0, py1 - py0);
            let soft = blur_region(&dil, w, px0, py0, pw, ph, feather);
            let sx = S as f32 / cw as f32;
            let sy = S as f32 / ch as f32;
            let sample = |x: usize, y: usize| -> [f32; 3] {
                let fx = ((x - cx) as f32 + 0.5) * sx - 0.5;
                let fy = ((y - cy) as f32 + 0.5) * sy - 0.5;
                let fx = fx.clamp(0.0, (S - 1) as f32);
                let fy = fy.clamp(0.0, (S - 1) as f32);
                let (ax, ay) = (fx as usize, fy as usize);
                let (bx, by) = ((ax + 1).min(S - 1), (ay + 1).min(S - 1));
                let (tx, ty) = (fx - ax as f32, fy - ay as f32);
                let g = |i: usize, j: usize| out[j * S + i];
                let mut c = [0f32; 3];
                for ch in 0..3 {
                    let top = g(ax, ay)[ch] + (g(bx, ay)[ch] - g(ax, ay)[ch]) * tx;
                    let bot = g(ax, by)[ch] + (g(bx, by)[ch] - g(ax, by)[ch]) * tx;
                    c[ch] = linear(top + (bot - top) * ty) / gain;
                }
                c
            };
            let rgb_part: Vec<[f32; 3]> = (0..pw * ph)
                .into_par_iter()
                .map(|i| sample(px0 + i % pw, py0 + i / pw))
                .collect();
            let alpha_part: Vec<u8> = soft.iter().map(|a| (a * 255.0).round() as u8).collect();
            // Ins Arbeitsbild übernehmen – spätere Bereiche sehen die Reparatur
            for j in 0..ph {
                for i in 0..pw {
                    let a = alpha_part[j * pw + i] as f32 / 255.0;
                    if a > 0.0 {
                        let o = &mut work.data[(py0 + j) * w + px0 + i];
                        let c = rgb_part[j * pw + i];
                        for ch in 0..3 {
                            o[ch] += (c[ch] - o[ch]) * a;
                        }
                    }
                }
            }
            parts.push(PatchPart {
                x: px0,
                y: py0,
                w: pw,
                h: ph,
                rgb: rgb_part,
                alpha: alpha_part,
            });
        }
        progress(regions.len(), regions.len());
        Ok(Patch {
            width: w,
            height: h,
            parts,
        })
    }
}

/// Quadratische Erweiterung um `r` Pixel (zwei getrennte Durchläufe).
fn dilate(m: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    // waagerecht: Abstand zum nächsten gesetzten Pixel ≤ r
    let horiz: Vec<bool> = m
        .par_chunks(w)
        .flat_map_iter(|row| {
            let mut out = vec![false; w];
            let mut last: Option<usize> = None;
            for x in 0..w {
                if row[x] {
                    last = Some(x);
                }
                out[x] = last.is_some_and(|l| x - l <= r);
            }
            last = None;
            for x in (0..w).rev() {
                if row[x] {
                    last = Some(x);
                }
                out[x] |= last.is_some_and(|l| l - x <= r);
            }
            out
        })
        .collect();
    let mut out = vec![false; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let y0 = y.saturating_sub(r);
        let y1 = (y + r + 1).min(h);
        for (x, o) in row.iter_mut().enumerate() {
            *o = (y0..y1).any(|yy| horiz[yy * w + x]);
        }
    });
    out
}

/// Zusammenhängende Bereiche (über ein grobes Raster) als Pixelrahmen
/// (x0, y0, x1, y1), exklusive Enden.
fn regions(m: &[bool], w: usize, h: usize) -> Vec<(usize, usize, usize, usize)> {
    let (gw, gh) = (w.div_ceil(CELL), h.div_ceil(CELL));
    let occupied: Vec<bool> = (0..gw * gh)
        .into_par_iter()
        .map(|i| {
            let (gx, gy) = (i % gw, i / gw);
            (gy * CELL..((gy + 1) * CELL).min(h))
                .any(|y| m[y * w + gx * CELL..y * w + ((gx + 1) * CELL).min(w)].contains(&true))
        })
        .collect();
    let mut seen = vec![false; gw * gh];
    let mut out = Vec::new();
    for start in 0..gw * gh {
        if !occupied[start] || seen[start] {
            continue;
        }
        let (mut gx0, mut gy0, mut gx1, mut gy1) = (usize::MAX, usize::MAX, 0, 0);
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(c) = stack.pop() {
            let (x, y) = (c % gw, c / gw);
            gx0 = gx0.min(x);
            gy0 = gy0.min(y);
            gx1 = gx1.max(x);
            gy1 = gy1.max(y);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= gw as i64 || ny >= gh as i64 {
                        continue;
                    }
                    let n = ny as usize * gw + nx as usize;
                    if occupied[n] && !seen[n] {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
        // Rahmen auf die tatsächlich gesetzten Pixel verkleinern
        let (cx0, cy0) = (gx0 * CELL, gy0 * CELL);
        let (cx1, cy1) = (((gx1 + 1) * CELL).min(w), ((gy1 + 1) * CELL).min(h));
        let (mut x0, mut y0, mut x1, mut y1) = (cx1, cy1, cx0, cy0);
        for y in cy0..cy1 {
            for x in cx0..cx1 {
                if m[y * w + x] {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        if x0 < x1 && y0 < y1 {
            out.push((x0, y0, x1, y1));
        }
    }
    out
}

/// Weiche Kante: Kastenfilter (zwei Durchläufe) der Auswahl im Teilbereich.
fn blur_region(
    m: &[bool],
    w: usize,
    x0: usize,
    y0: usize,
    pw: usize,
    ph: usize,
    r: usize,
) -> Vec<f32> {
    let mut a: Vec<f32> = (0..pw * ph)
        .map(|i| m[(y0 + i / pw) * w + x0 + i % pw] as u8 as f32)
        .collect();
    let n = (2 * r + 1) as f32;
    for _ in 0..2 {
        let mut t = vec![0f32; pw * ph];
        for y in 0..ph {
            for x in 0..pw {
                let mut s = 0.0;
                for k in x.saturating_sub(r)..(x + r + 1).min(pw) {
                    s += a[y * pw + k];
                }
                t[y * pw + x] = s / n;
            }
        }
        for y in 0..ph {
            for x in 0..pw {
                let mut s = 0.0;
                for k in y.saturating_sub(r)..(y + r + 1).min(ph) {
                    s += t[k * pw + x];
                }
                a[y * pw + x] = s / n;
            }
        }
    }
    // Innerhalb der ursprünglichen Auswahl voll decken
    for (i, v) in a.iter_mut().enumerate() {
        if m[(y0 + i / pw) * w + x0 + i % pw] {
            *v = (*v * 2.0).min(1.0);
        }
    }
    a
}

/// Größenänderung: Flächenmittel beim Verkleinern, bilinear beim Vergrößern.
fn resample(
    get: &(dyn Fn(usize, usize) -> [f32; 3] + Sync),
    sw: usize,
    sh: usize,
    dw: usize,
    dh: usize,
) -> Vec<[f32; 3]> {
    (0..dw * dh)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % dw, i / dw);
            let fx = sw as f32 / dw as f32;
            let fy = sh as f32 / dh as f32;
            if fx >= 1.0 && fy >= 1.0 {
                let xa = (x as f32 * fx) as usize;
                let xb = (((x + 1) as f32 * fx) as usize).clamp(xa + 1, sw);
                let ya = (y as f32 * fy) as usize;
                let yb = (((y + 1) as f32 * fy) as usize).clamp(ya + 1, sh);
                let mut c = [0f32; 3];
                for yy in ya..yb {
                    for xx in xa..xb {
                        let p = get(xx, yy);
                        for k in 0..3 {
                            c[k] += p[k];
                        }
                    }
                }
                let n = ((xb - xa) * (yb - ya)) as f32;
                c.map(|v| v / n)
            } else {
                let sx = ((x as f32 + 0.5) * fx - 0.5).clamp(0.0, (sw - 1) as f32);
                let sy = ((y as f32 + 0.5) * fy - 0.5).clamp(0.0, (sh - 1) as f32);
                let (ax, ay) = (sx as usize, sy as usize);
                let (bx, by) = ((ax + 1).min(sw - 1), (ay + 1).min(sh - 1));
                let (tx, ty) = (sx - ax as f32, sy - ay as f32);
                let (p00, p10, p01, p11) = (get(ax, ay), get(bx, ay), get(ax, by), get(bx, by));
                let mut c = [0f32; 3];
                for k in 0..3 {
                    let top = p00[k] + (p10[k] - p00[k]) * tx;
                    let bot = p01[k] + (p11[k] - p01[k]) * tx;
                    c[k] = top + (bot - top) * ty;
                }
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_separate_regions_and_dilates() {
        let (w, h) = (300, 200);
        let mut m = vec![false; w * h];
        for y in 20..30 {
            for x in 20..40 {
                m[y * w + x] = true;
            }
        }
        for y in 150..160 {
            for x in 250..260 {
                m[y * w + x] = true;
            }
        }
        let r = regions(&m, w, h);
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(r.contains(&(20, 20, 40, 30)));
        let d = dilate(&m, w, h, 3);
        assert!(d[17 * w + 17] && !d[16 * w + 20] && d[32 * w + 42]);
        let soft = blur_region(&d, w, 10, 10, 40, 30, 2);
        assert_eq!(soft[(25 - 10) * 40 + (30 - 10)], 1.0);
        assert!(soft[0] < 0.01);
    }
}
