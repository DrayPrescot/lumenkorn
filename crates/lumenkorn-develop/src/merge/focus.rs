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
//! 3. Die Bilder werden nacheinander je einmal geladen, ausgerichtet,
//!    eingearbeitet und wieder freigegeben; im Speicher liegen nur die bisher
//!    beste Pyramide, die Referenz (füllt nicht abgedeckte Ränder) und das
//!    aktuelle Bild.

use super::align::{self, Features};
use super::{Loader, Progress, check};
use crate::color::luma;
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
/// (verhindert, dass einzelnes Rauschen gewinnt). `mask`: nur abgedeckte
/// Pixel zählen, die übrigen bekommen −1 (`None`: alle abgedeckt).
fn activity(l: &ImageF32, mask: Option<&[bool]>) -> Vec<f32> {
    let (w, h) = (l.width, l.height);
    let a: Vec<f32> = l.data.par_iter().map(|p| luma(*p).abs()).collect();
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            if mask.is_some_and(|m| !m[y * w + x]) {
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

/// `down` für ein einkanaliges Bild (Abdeckungsmaske). Gleiche
/// Rechenreihenfolge, also dieselben Werte wie ein Kanal von `down` – aber
/// ein Drittel des Speichers.
fn down1(img: &[f32], w: usize, h: usize) -> Vec<f32> {
    const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let (ow, oh) = (w.div_ceil(2), h.div_ceil(2));
    let mut tmp = vec![0f32; ow * h];
    tmp.par_chunks_mut(ow).enumerate().for_each(|(y, row)| {
        let src = &img[y * w..(y + 1) * w];
        for (ox, o) in row.iter_mut().enumerate() {
            let x = (ox * 2) as isize;
            let mut s = 0f32;
            for (k, kv) in K.iter().enumerate() {
                let xx = (x + k as isize - 2).clamp(0, w as isize - 1) as usize;
                s += kv * src[xx];
            }
            *o = s;
        }
    });
    let mut data = vec![0f32; ow * oh];
    data.par_chunks_mut(ow).enumerate().for_each(|(oy, row)| {
        let y = (oy * 2) as isize;
        for (k, kv) in K.iter().enumerate() {
            let yy = (y + k as isize - 2).clamp(0, h as isize - 1) as usize;
            let src = &tmp[yy * ow..(yy + 1) * ow];
            for (o, s) in row.iter_mut().zip(src) {
                *o += kv * s;
            }
        }
    });
    data
}

/// Abdeckungsmasken je Pyramidenstufe (nur vollständig abgedeckte Pixel zählen).
fn mask_pyramid(cover: Vec<f32>, w: usize, h: usize, levels: usize) -> Vec<Vec<bool>> {
    let mut out = Vec::with_capacity(levels + 1);
    let (mut m, mut mw, mut mh) = (cover, w, h);
    for lvl in 0..=levels {
        out.push(m.iter().map(|v| *v > 0.999).collect());
        if lvl < levels {
            // Rand der Abdeckung je Stufe etwas großzügiger ausschließen
            m = down1(&m, mw, mh);
            (mw, mh) = (mw.div_ceil(2), mh.div_ceil(2));
        }
    }
    out
}

/// Bisher bestes Ergebnis: verschmolzene Pyramide, Detailstärke des jeweils
/// gewählten Pixels und Summe/Anzahl der Grundstufe.
struct Stack {
    fused: Vec<ImageF32>,
    best: Vec<Vec<f32>>,
    base_sum: Vec<[f32; 3]>,
    base_cnt: Vec<f32>,
}

impl Stack {
    /// Beginnt mit der (voll abgedeckten) Referenz.
    fn new(img: ImageF32, levels: usize) -> Self {
        let pyr = pyramid(img, levels);
        let best = pyr[..levels].iter().map(|l| activity(l, None)).collect();
        let base_sum = pyr[levels].data.clone();
        let base_cnt = vec![1.0; base_sum.len()];
        Self {
            fused: pyr,
            best,
            base_sum,
            base_cnt,
        }
    }

    /// Arbeitet ein (ausgerichtetes) Bild ein; `masks` je Stufe, `None` = voll abgedeckt.
    fn add(&mut self, img: ImageF32, masks: Option<&[Vec<bool>]>) {
        let levels = self.fused.len() - 1;
        let pyr = pyramid(img, levels);
        for (lvl, l) in pyr.into_iter().enumerate() {
            let m = masks.map(|m| m[lvl].as_slice());
            if lvl == levels {
                self.base_sum
                    .par_iter_mut()
                    .zip(self.base_cnt.par_iter_mut())
                    .zip(l.data.par_iter())
                    .enumerate()
                    .for_each(|(i, ((s, c), p))| {
                        if m.is_none_or(|m| m[i]) {
                            for k in 0..3 {
                                s[k] += p[k];
                            }
                            *c += 1.0;
                        }
                    });
                continue;
            }
            let act = activity(&l, m);
            self.fused[lvl]
                .data
                .par_iter_mut()
                .zip(self.best[lvl].par_iter_mut())
                .zip(l.data.par_iter().zip(act.par_iter()))
                .for_each(|((f, b), (p, a))| {
                    if *a > *b {
                        *f = *p;
                        *b = *a;
                    }
                });
        }
    }

    fn finish(self) -> ImageF32 {
        let Self {
            mut fused,
            best,
            base_sum,
            base_cnt,
        } = self;
        drop(best);
        let levels = fused.len() - 1;
        fused[levels].data = base_sum
            .iter()
            .zip(&base_cnt)
            .map(|(s, c)| s.map(|v| v / c))
            .collect();
        let mut out = collapse(fused);
        out.data
            .par_iter_mut()
            .for_each(|p| *p = p.map(|v| v.max(0.0)));
        out
    }
}

/// Kanten für die Ausrichtung: Merkmale (1600) und Nachführung (1200/600).
const FEATURE_EDGE: usize = 1600;
const DENSE_EDGE: usize = 1200;

/// Merkmale und Nachführungs-Pyramide eines Vollbilds. (Beides aus einer
/// gemeinsamen Zwischenstufe zu verkleinern spart nur ~0,1 s je 24-MP-Bild,
/// verschiebt die Ausrichtung aber messbar – daher direkt aus dem Vollbild.)
fn align_data(img: &ImageF32) -> (Features, align::Dense) {
    (
        align::features(img, FEATURE_EDGE, 3000),
        align::dense(img, DENSE_EDGE),
    )
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
    let step = tr_s("Schärfe zusammenführen", "Merging sharpness");

    // --- Referenz -----------------------------------------------------------------
    check(cancel)?;
    progress(step, 0, n);
    let refimg = load(r)?;
    let (w, h) = (refimg.width, refimg.height);
    // Halbieren, bis die kürzere Seite höchstens 48 px misst (Grundstufe ohne
    // nennenswerte Details), höchstens 9 Stufen – das reicht bis ~25 000 px.
    let levels = {
        let mut l = 0;
        let mut m = w.min(h);
        while m > 48 && l < 9 {
            m = m.div_ceil(2);
            l += 1;
        }
        l
    };
    let ref_align = opts.align.then(|| align_data(&refimg));
    // Die Referenz füllt, was ein entzerrtes Bild nicht abdeckt – nur beim
    // Ausrichten nötig (sonst deckt jedes Bild alles ab). Halbe Genauigkeit
    // genügt für diese ohnehin als ungültig markierten Ränder und spart die
    // Hälfte (bei 60 MP 360 statt 720 MB neben Pyramide und Bildern).
    let reference: Option<Vec<[half::f16; 3]>> = opts.align.then(|| {
        refimg
            .data
            .par_iter()
            .map(|p| p.map(half::f16::from_f32))
            .collect()
    });
    let mut stack = Stack::new(refimg, levels);

    // --- Übrige Bilder: je eines laden, ausrichten, einarbeiten, freigeben ----------
    for (k, i) in (0..n).filter(|&i| i != r).enumerate() {
        check(cancel)?;
        progress(step, k + 1, n);
        let img = load(i)?;
        if (img.width, img.height) != (w, h) {
            return Err(Error::Params(crate::tr!(
                "Alle Fotos müssen gleich groß sein (Nr. {} ist {}×{}, die Referenz {}×{})",
                "All photos must be the same size (no. {} is {}×{}, the reference {}×{})",
                i + 1,
                img.width,
                img.height,
                w,
                h
            )));
        }
        // Referenzkoordinaten → Bild i
        let hm = match &ref_align {
            Some((rf, rd)) => {
                let (fi, di) = align_data(&img);
                let h0 = match align::align(rf, &fi, 25).and_then(|f| align::invert(&f.h)) {
                    Some(h0) => h0,
                    None => {
                        log::warn!(
                            "Focus Stacking: Bild {} nicht ausrichtbar, nehme Stativ an",
                            i + 1
                        );
                        align::IDENTITY
                    }
                };
                // Eckpunkte verschieben sich mit der Unschärfe – pixelgenau nachführen
                align::refine(rd, &di, &h0)
            }
            None => align::IDENTITY,
        };
        if hm == align::IDENTITY {
            // Keine Entzerrung (ohne Ausrichtung): Bild direkt, voll abgedeckt
            stack.add(img, None);
            continue;
        }
        // Auf die Referenz entzerren; nicht abgedeckte Stellen mit der Referenz
        // füllen (keine künstlichen Kanten) und als ungültig markieren.
        let refimg = reference.as_ref().expect("Referenz beim Ausrichten");
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
                    None => (refimg[p].map(half::f16::to_f32), 0.0),
                };
            });
        drop(img);
        let masks = mask_pyramid(cover, w, h, levels);
        stack.add(
            ImageF32 {
                width: w,
                height: h,
                data,
            },
            Some(&masks),
        );
    }
    drop(reference);
    progress(step, n, n);
    Ok(stack.finish())
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
