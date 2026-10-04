//! Focus Stacking: aus einer Reihe mit verschobener Schärfeebene ein
//! durchgehend scharfes Bild.
//!
//! 1. Referenz ist die mittlere Aufnahme; alle anderen werden per Homografie
//!    auf sie ausgerichtet (gleicht Verwackeln und das „Atmen“ des Objektivs,
//!    also die leichte Maßstabsänderung beim Fokussieren, aus).
//! 2. Jedes Bild wird in eine Laplace-Pyramide zerlegt (Details je
//!    Größenordnung). Je Pyramidenstufe und Pixel gewinnt das Bild mit dem
//!    stärksten (leicht geglätteten) Detail – das ist das an dieser Stelle
//!    scharfe. Die gröbste Stufe wird gemittelt, damit keine Helligkeitssprünge
//!    entstehen.
//! 3. Die Bilder werden nacheinander eingearbeitet; im Speicher liegen nur die
//!    bisher beste Pyramide und das aktuelle Bild.

use super::align::{self, Features, Mat3};
use super::{Loader, Progress, check};
use crate::error::{Error, Result};
use crate::i18n::tr_s;
use crate::image_f32::ImageF32;
use rayon::prelude::*;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FocusOptions {
    /// Freihand-Reihen und Objektiv-Atmen ausgleichen
    pub align: bool,
}

impl Default for FocusOptions {
    fn default() -> Self {
        Self { align: true }
    }
}

fn luma(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

/// Weichzeichnen mit [1 4 6 4 1]/16 (getrennt) und halbieren.
fn down(img: &ImageF32) -> ImageF32 {
    let (w, h) = (img.width, img.height);
    const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let (ow, oh) = (w.div_ceil(2), h.div_ceil(2));
    // waagerecht (nur jede zweite Spalte gebraucht)
    let mut tmp = vec![[0f32; 3]; ow * h];
    tmp.par_chunks_mut(ow).enumerate().for_each(|(y, row)| {
        let src = &img.data[y * w..(y + 1) * w];
        for (ox, o) in row.iter_mut().enumerate() {
            let x = (ox * 2) as isize;
            let mut s = [0f32; 3];
            for (k, kv) in K.iter().enumerate() {
                let xx = (x + k as isize - 2).clamp(0, w as isize - 1) as usize;
                for c in 0..3 {
                    s[c] += kv * src[xx][c];
                }
            }
            *o = s;
        }
    });
    let mut data = vec![[0f32; 3]; ow * oh];
    data.par_chunks_mut(ow).enumerate().for_each(|(oy, row)| {
        let y = (oy * 2) as isize;
        for (k, kv) in K.iter().enumerate() {
            let yy = (y + k as isize - 2).clamp(0, h as isize - 1) as usize;
            let src = &tmp[yy * ow..(yy + 1) * ow];
            for (o, s) in row.iter_mut().zip(src) {
                for c in 0..3 {
                    o[c] += kv * s[c];
                }
            }
        }
    });
    ImageF32 {
        width: ow,
        height: oh,
        data,
    }
}

/// Bilinear auf `w`×`h` vergrößern (Gegenstück zu `down`).
fn up(img: &ImageF32, w: usize, h: usize) -> ImageF32 {
    let (sw, sh) = (img.width, img.height);
    let mut data = vec![[0f32; 3]; w * h];
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let fy = y as f32 * 0.5;
        let y0 = (fy as usize).min(sh - 1);
        let y1 = (y0 + 1).min(sh - 1);
        let ty = fy - y0 as f32;
        for (x, o) in row.iter_mut().enumerate() {
            let fx = x as f32 * 0.5;
            let x0 = (fx as usize).min(sw - 1);
            let x1 = (x0 + 1).min(sw - 1);
            let tx = fx - x0 as f32;
            let (a, b) = (img.data[y0 * sw + x0], img.data[y0 * sw + x1]);
            let (c, d) = (img.data[y1 * sw + x0], img.data[y1 * sw + x1]);
            for k in 0..3 {
                let top = a[k] + (b[k] - a[k]) * tx;
                let bot = c[k] + (d[k] - c[k]) * tx;
                o[k] = top + (bot - top) * ty;
            }
        }
    });
    ImageF32 {
        width: w,
        height: h,
        data,
    }
}

/// Laplace-Pyramide: Detailstufen fein → grob, zuletzt die Grundstufe.
fn pyramid(img: ImageF32, levels: usize) -> Vec<ImageF32> {
    let mut out = Vec::with_capacity(levels + 1);
    let mut g = img;
    for _ in 0..levels {
        let next = down(&g);
        let u = up(&next, g.width, g.height);
        g.data
            .par_iter_mut()
            .zip(u.data.par_iter())
            .for_each(|(p, q)| {
                for c in 0..3 {
                    p[c] -= q[c];
                }
            });
        out.push(g);
        g = next;
    }
    out.push(g);
    out
}

fn collapse(mut pyr: Vec<ImageF32>) -> ImageF32 {
    let mut g = pyr.pop().expect("Pyramide leer");
    while let Some(mut l) = pyr.pop() {
        let u = up(&g, l.width, l.height);
        l.data
            .par_iter_mut()
            .zip(u.data.par_iter())
            .for_each(|(p, q)| {
                for c in 0..3 {
                    p[c] += q[c];
                }
            });
        g = l;
    }
    g
}

/// Detailstärke je Pixel: |Laplace| der Helligkeit, 3×3 gemittelt
/// (verhindert, dass einzelnes Rauschen gewinnt).
fn activity(l: &ImageF32, mask: &[bool]) -> Vec<f32> {
    let (w, h) = (l.width, l.height);
    let a: Vec<f32> = l.data.par_iter().map(|p| luma(*p).abs()).collect();
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            if !mask[y * w + x] {
                *o = -1.0;
                continue;
            }
            let mut s = 0.0;
            for yy in y.saturating_sub(1)..(y + 2).min(h) {
                for xx in x.saturating_sub(1)..(x + 2).min(w) {
                    s += a[yy * w + xx];
                }
            }
            *o = s;
        }
    });
    out
}

/// Abdeckungsmasken je Pyramidenstufe (nur vollständig abgedeckte Pixel zählen).
fn mask_pyramid(cover: Vec<f32>, w: usize, h: usize, levels: usize) -> Vec<Vec<bool>> {
    let mut out = Vec::with_capacity(levels + 1);
    let mut m = ImageF32 {
        width: w,
        height: h,
        data: cover.into_iter().map(|v| [v; 3]).collect(),
    };
    for _ in 0..=levels {
        out.push(m.data.iter().map(|p| p[0] > 0.999).collect());
        // Rand der Abdeckung je Stufe etwas großzügiger ausschließen
        m = down(&m);
    }
    out
}

pub fn merge(
    n: usize,
    load: Loader,
    opts: FocusOptions,
    progress: Progress,
    cancel: &AtomicBool,
) -> Result<ImageF32> {
    if n < 2 {
        return Err(Error::Params(
            tr_s(
                "Für Focus Stacking mindestens zwei Fotos wählen",
                "Select at least two photos for focus stacking",
            )
            .into(),
        ));
    }
    let r = n / 2;
    // Referenz zuerst, dann die anderen
    let order: Vec<usize> = std::iter::once(r)
        .chain((0..n).filter(|&i| i != r))
        .collect();

    // --- 1. Laden und ausrichten ----------------------------------------------------
    const KEEP_BYTES: usize = 3 << 30;
    let mut full: Vec<Option<ImageF32>> = (0..n).map(|_| None).collect();
    let mut feats: Vec<Option<Features>> = (0..n).map(|_| None).collect();
    let mut size = (0, 0);
    let mut kept = 0;
    for (k, &i) in order.iter().enumerate() {
        check(cancel)?;
        progress(tr_s("Bilder laden", "Loading images"), k, n);
        let img = load(i)?;
        if k == 0 {
            size = (img.width, img.height);
        } else if (img.width, img.height) != size {
            return Err(Error::Params(crate::tr!(
                "Alle Fotos müssen gleich groß sein (Nr. {} ist {}×{}, die Referenz {}×{})",
                "All photos must be the same size (no. {} is {}×{}, the reference {}×{})",
                i + 1,
                img.width,
                img.height,
                size.0,
                size.1
            )));
        }
        if opts.align {
            feats[i] = Some(align::features(&img, 1600, 3000));
        }
        let bytes = img.data.len() * 12;
        if kept + bytes <= KEEP_BYTES {
            kept += bytes;
            full[i] = Some(img);
        }
    }
    // homs[i]: Referenzkoordinaten → Bild i
    let mut homs: Vec<Mat3> = vec![align::IDENTITY; n];
    if opts.align {
        let rf = feats[r].as_ref().unwrap();
        let fitted: Vec<(usize, Option<Mat3>)> = (0..n)
            .into_par_iter()
            .filter(|&i| i != r)
            .map(|i| {
                let fit = align::align(rf, feats[i].as_ref().unwrap(), 25);
                (i, fit.and_then(|f| align::invert(&f.h)))
            })
            .collect();
        for (i, h) in fitted {
            match h {
                Some(h) => homs[i] = h,
                None => log::warn!(
                    "Focus Stacking: Bild {} nicht ausrichtbar, nehme Stativ an",
                    i + 1
                ),
            }
        }
    }
    drop(feats);

    // --- 2. Pyramiden verschmelzen ---------------------------------------------------
    let (w, h) = size;
    let levels = {
        let mut l = 0;
        let mut m = w.min(h);
        while m > 48 && l < 9 {
            m = m.div_ceil(2);
            l += 1;
        }
        l
    };
    let mut fused: Vec<ImageF32> = Vec::new();
    let mut best: Vec<Vec<f32>> = Vec::new();
    let mut base_sum: Vec<[f32; 3]> = Vec::new();
    let mut base_cnt: Vec<f32> = Vec::new();
    let mut reference: Option<ImageF32> = None;
    let mut ref_dense: Option<align::Dense> = None;

    for (k, &i) in order.iter().enumerate() {
        check(cancel)?;
        progress(tr_s("Schärfe zusammenführen", "Merging sharpness"), k, n);
        let img = match full[i].take() {
            Some(img) => img,
            None => load(i)?,
        };
        // Auf die Referenz entzerren; nicht abgedeckte Stellen mit der Referenz
        // füllen (keine künstlichen Kanten) und als ungültig markieren.
        let (img, cover) = if i == r {
            (img, vec![1f32; w * h])
        } else {
            let refimg = reference.as_ref().unwrap();
            // Eckpunkte verschieben sich mit der Unschärfe – pixelgenau nachführen
            let hm = match (&ref_dense, opts.align) {
                (Some(rd), true) => align::refine(rd, &align::dense(&img, 1200), &homs[i]),
                _ => homs[i],
            };
            // Direkt in zwei Puffer (kein Zwischenpuffer aus Paaren)
            let mut data = vec![[0f32; 3]; w * h];
            let mut cover = vec![0f32; w * h];
            data.par_iter_mut()
                .zip(cover.par_iter_mut())
                .enumerate()
                .for_each(|(p, (d, c))| {
                    let (x, y) = (p % w, p / w);
                    let (sx, sy) = align::apply(&hm, x as f64, y as f64);
                    (*d, *c) = match align::sample(&img, sx, sy) {
                        Some(v) => (v, 1.0),
                        None => (refimg.data[p], 0.0),
                    };
                });
            drop(img);
            (
                ImageF32 {
                    width: w,
                    height: h,
                    data,
                },
                cover,
            )
        };
        if i == r {
            if opts.align {
                ref_dense = Some(align::dense(&img, 1200));
            }
            reference = Some(img.clone());
        }
        let masks = mask_pyramid(cover, w, h, levels);
        let pyr = pyramid(img, levels);
        if fused.is_empty() {
            best = pyr[..levels]
                .iter()
                .zip(&masks)
                .map(|(l, m)| activity(l, m))
                .collect();
            base_sum = pyr[levels].data.clone();
            base_cnt = vec![1.0; base_sum.len()];
            fused = pyr;
            continue;
        }
        for (lvl, l) in pyr.into_iter().enumerate() {
            let m = &masks[lvl];
            if lvl == levels {
                base_sum
                    .par_iter_mut()
                    .zip(base_cnt.par_iter_mut())
                    .zip(l.data.par_iter())
                    .zip(m.par_iter())
                    .for_each(|(((s, c), p), ok)| {
                        if *ok {
                            for k in 0..3 {
                                s[k] += p[k];
                            }
                            *c += 1.0;
                        }
                    });
                continue;
            }
            let act = activity(&l, m);
            fused[lvl]
                .data
                .par_iter_mut()
                .zip(best[lvl].par_iter_mut())
                .zip(l.data.par_iter().zip(act.par_iter()))
                .for_each(|((f, b), (p, a))| {
                    if *a > *b {
                        *f = *p;
                        *b = *a;
                    }
                });
        }
    }
    drop(best);
    drop(reference);
    progress(tr_s("Schärfe zusammenführen", "Merging sharpness"), n, n);
    fused[levels].data = base_sum
        .iter()
        .zip(&base_cnt)
        .map(|(s, c)| s.map(|v| v / c))
        .collect();
    let mut out = collapse(fused);
    out.data
        .par_iter_mut()
        .for_each(|p| *p = p.map(|v| v.max(0.0)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::align::tests::{texture, warp};

    fn box_blur(img: &ImageF32, r: usize, passes: usize) -> ImageF32 {
        let mut cur = img.clone();
        for _ in 0..passes {
            let src = cur.clone();
            cur = ImageF32::from_fn(src.width, src.height, |x, y| {
                let mut s = [0f32; 3];
                let mut c = 0.0;
                for yy in y.saturating_sub(r)..(y + r + 1).min(src.height) {
                    for xx in x.saturating_sub(r)..(x + r + 1).min(src.width) {
                        let p = src.get(xx, yy);
                        for k in 0..3 {
                            s[k] += p[k];
                        }
                        c += 1.0;
                    }
                }
                s.map(|v| v / c)
            });
        }
        cur
    }

    #[test]
    fn combines_sharp_regions() {
        let (w, h) = (600usize, 400usize);
        let sharp = texture(w, h, 5);
        let soft = box_blur(&sharp, 3, 2);
        // Drei Aufnahmen: jeweils ein Drittel scharf, Rest unscharf
        let shots: Vec<ImageF32> = (0..3)
            .map(|s| {
                ImageF32::from_fn(w, h, |x, y| {
                    if x * 3 / w == s {
                        sharp.get(x, y)
                    } else {
                        soft.get(x, y)
                    }
                })
            })
            .collect();
        // Die letzte leicht verschoben und vergrößert (Objektiv-Atmen)
        let breath = [[1.01, 0.0, -2.0], [0.0, 1.01, 1.5], [0.0, 0.0, 1.0]];
        let mut shots = shots;
        shots[2] = warp(&shots[2], &align::invert(&breath).unwrap(), w, h);
        let cancel = AtomicBool::new(false);
        let out = merge(
            3,
            &|i| Ok(shots[i].clone()),
            FocusOptions::default(),
            &|_, _, _| {},
            &cancel,
        )
        .unwrap();
        assert_eq!((out.width, out.height), (w, h));
        // Fehler gegenüber der scharfen Vorlage in jedem Drittel (ohne Ränder)
        for s in 0..3 {
            let (x0, x1) = (s * w / 3 + 20, (s + 1) * w / 3 - 20);
            let err = |img: &ImageF32| {
                let mut e = 0.0;
                let mut c = 0.0;
                for y in 20..h - 20 {
                    for x in x0..x1 {
                        e += (luma(img.get(x, y)) - luma(sharp.get(x, y))).abs();
                        c += 1.0;
                    }
                }
                e / c
            };
            let (fused, blurred) = (err(&out), err(&soft));
            // Das verschobene Bild wurde zweimal bilinear umgerechnet (Test + Ausrichtung)
            let limit = if s == 2 { 0.45 } else { 0.15 };
            assert!(
                fused < blurred * limit,
                "Drittel {s}: Fehler {fused:.4} (unscharf {blurred:.4})"
            );
        }
    }
}
