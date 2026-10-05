//! Bilder zueinander ausrichten: Merkmalspunkte finden, zwischen zwei Bildern
//! zuordnen und eine Homografie (projektive Abbildung) robust schätzen.
//!
//! - Merkmale: Harris-Ecken auf einem verkleinerten, belichtungsnormierten
//!   Graubild (Logarithmus der Helligkeit, lokal normiert) – so finden sich
//!   dieselben Punkte auch in unterschiedlich belichteten Aufnahmen (HDR).
//! - Beschreiber: 256-Bit-BRIEF auf einem geglätteten 31×31-Ausschnitt.
//! - Zuordnung: Hamming-Abstand, Verhältnistest und gegenseitig beste Treffer.
//! - Homografie: RANSAC über 4-Punkt-Lösungen, danach Ausgleich über alle
//!   passenden Paare (mit Hartley-Normierung für numerische Stabilität).

use crate::image_f32::ImageF32;
use rayon::prelude::*;

/// 3×3-Matrix, zeilenweise.
pub type Mat3 = [[f64; 3]; 3];

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

pub fn transpose(a: &Mat3) -> Mat3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = a[j][i];
        }
    }
    r
}

pub fn invert(a: &Mat3) -> Option<Mat3> {
    let c =
        |r1: usize, c1: usize, r2: usize, c2: usize| a[r1][c1] * a[r2][c2] - a[r1][c2] * a[r2][c1];
    let det = a[0][0] * c(1, 1, 2, 2) - a[0][1] * c(1, 0, 2, 2) + a[0][2] * c(1, 0, 2, 1);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = [
        [c(1, 1, 2, 2), -c(0, 1, 2, 2), c(0, 1, 1, 2)],
        [-c(1, 0, 2, 2), c(0, 0, 2, 2), -c(0, 0, 1, 2)],
        [c(1, 0, 2, 1), -c(0, 0, 2, 1), c(0, 0, 1, 1)],
    ];
    Some(inv.map(|r| r.map(|v| v / det)))
}

pub fn apply(h: &Mat3, x: f64, y: f64) -> (f64, f64) {
    let w = h[2][0] * x + h[2][1] * y + h[2][2];
    let w = if w.abs() < 1e-12 { 1e-12 } else { w };
    (
        (h[0][0] * x + h[0][1] * y + h[0][2]) / w,
        (h[1][0] * x + h[1][1] * y + h[1][2]) / w,
    )
}

/// Skaliert eine Homografie zwischen verkleinerten Bildern auf volle Größe:
/// `h` bildet Punkte (Faktor `s_from`) auf Punkte (Faktor `s_to`) ab.
pub fn rescale(h: &Mat3, s_from: f64, s_to: f64) -> Mat3 {
    let a = [[s_from, 0.0, 0.0], [0.0, s_from, 0.0], [0.0, 0.0, 1.0]];
    let b = [
        [1.0 / s_to, 0.0, 0.0],
        [0.0, 1.0 / s_to, 0.0],
        [0.0, 0.0, 1.0],
    ];
    mul(&b, &mul(h, &a))
}

/// Löst ein lineares Gleichungssystem (Gauß mit Spaltenpivot). `a` ist n×n.
pub fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..n {
            let f = a[row][col] / a[col][col];
            if f != 0.0 {
                let (top, bottom) = a.split_at_mut(row);
                for (x, y) in bottom[0][col..].iter_mut().zip(&top[col][col..]) {
                    *x -= f * y;
                }
                b[row] -= f * b[col];
            }
        }
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let s: f64 = (i + 1..n).map(|k| a[i][k] * x[k]).sum();
        x[i] = (b[i] - s) / a[i][i];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

// --- Graubild und Merkmale ------------------------------------------------------

/// Einkanaliges Arbeitsbild.
#[derive(Clone)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Gray {
    fn at(&self, x: isize, y: isize) -> f32 {
        let x = x.clamp(0, self.w as isize - 1) as usize;
        let y = y.clamp(0, self.h as isize - 1) as usize;
        self.data[y * self.w + x]
    }
}

/// Belichtungsunabhängiges Graubild für die Merkmalssuche: Logarithmus der
/// Helligkeit, dann auf Mittelwert 0 und Streuung 1 gebracht.
pub fn feature_gray(img: &ImageF32) -> Gray {
    let mut data: Vec<f32> = img
        .data
        .par_iter()
        .map(|p| {
            (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2])
                .max(1e-4)
                .ln()
        })
        .collect();
    let n = data.len().max(1) as f32;
    let mean = data.iter().sum::<f32>() / n;
    let sd = (data.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n)
        .sqrt()
        .max(1e-3);
    data.par_iter_mut().for_each(|v| *v = (*v - mean) / sd);
    Gray {
        w: img.width,
        h: img.height,
        data,
    }
}

/// Trennbarer Gaußfilter.
pub fn blur(g: &Gray, sigma: f32) -> Gray {
    let r = (sigma * 3.0).ceil() as isize;
    let k: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let ks: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / ks).collect();
    let (w, h) = (g.w, g.h);
    let tmp: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            (-r..=r).map(|d| k[(d + r) as usize] * g.at(x + d, y)).sum()
        })
        .collect();
    let t = Gray { w, h, data: tmp };
    let data = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            (-r..=r).map(|d| k[(d + r) as usize] * t.at(x, y + d)).sum()
        })
        .collect();
    Gray { w, h, data }
}

#[derive(Clone, Copy, Debug)]
pub struct Keypoint {
    pub x: f32,
    pub y: f32,
    pub desc: [u64; 4],
}

const PATCH: isize = 15; // halbe Kantenlänge des Beschreiber-Ausschnitts
const BORDER: usize = 18;

/// Feste Punktpaare des BRIEF-Beschreibers (deterministisch).
fn brief_pairs() -> &'static [(i8, i8, i8, i8); 256] {
    static PAIRS: std::sync::OnceLock<[(i8, i8, i8, i8); 256]> = std::sync::OnceLock::new();
    PAIRS.get_or_init(|| {
        let mut state: u32 = 0x9E37_79B9;
        let mut next = || {
            // Annähernd normalverteilt um 0 (Summe von Gleichverteilten)
            let mut s = 0i32;
            for _ in 0..4 {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                s += (state % 15) as i32 - 7;
            }
            (s / 2).clamp(-(PATCH as i32), PATCH as i32) as i8
        };
        let mut p = [(0, 0, 0, 0); 256];
        for e in p.iter_mut() {
            *e = (next(), next(), next(), next());
        }
        p
    })
}

/// Harris-Ecken, gleichmäßig über das Bild verteilt (je Rasterzelle die
/// stärksten), mit BRIEF-Beschreibern.
pub fn detect(g: &Gray, max_points: usize) -> Vec<Keypoint> {
    let (w, h) = (g.w, g.h);
    if w < 2 * BORDER + 4 || h < 2 * BORDER + 4 {
        return vec![];
    }
    let s = blur(g, 1.0);
    // Gradienten und Strukturtensor
    let grads: Vec<(f32, f32)> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            (
                (s.at(x + 1, y) - s.at(x - 1, y)) * 0.5,
                (s.at(x, y + 1) - s.at(x, y - 1)) * 0.5,
            )
        })
        .collect();
    let xx = blur(
        &Gray {
            w,
            h,
            data: grads.iter().map(|g| g.0 * g.0).collect(),
        },
        1.5,
    );
    let yy = blur(
        &Gray {
            w,
            h,
            data: grads.iter().map(|g| g.1 * g.1).collect(),
        },
        1.5,
    );
    let xy = blur(
        &Gray {
            w,
            h,
            data: grads.iter().map(|g| g.0 * g.1).collect(),
        },
        1.5,
    );
    let resp: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (a, b, c) = (xx.data[i], yy.data[i], xy.data[i]);
            a * b - c * c - 0.04 * (a + b) * (a + b)
        })
        .collect();
    let r = Gray { w, h, data: resp };

    // Je Zelle die besten lokalen Maxima
    let cell = ((w * h) as f32 / max_points as f32 * 4.0).sqrt().max(8.0) as usize;
    let per_cell = 4;
    let max_resp = r.data.iter().cloned().fold(0.0f32, f32::max);
    let threshold = max_resp * 1e-4;
    let cells_x = w.div_ceil(cell);
    let cells_y = h.div_ceil(cell);
    let mut pts: Vec<(f32, usize, usize)> = (0..cells_x * cells_y)
        .into_par_iter()
        .flat_map_iter(|c| {
            let (cx, cy) = (c % cells_x, c / cells_x);
            let mut best: Vec<(f32, usize, usize)> = Vec::new();
            for y in (cy * cell).max(BORDER)..((cy + 1) * cell).min(h - BORDER) {
                for x in (cx * cell).max(BORDER)..((cx + 1) * cell).min(w - BORDER) {
                    let v = r.data[y * w + x];
                    if v <= threshold {
                        continue;
                    }
                    let is_max = (-1isize..=1).all(|dy| {
                        (-1isize..=1).all(|dx| {
                            (dx == 0 && dy == 0) || r.at(x as isize + dx, y as isize + dy) < v
                        })
                    });
                    if is_max {
                        best.push((v, x, y));
                    }
                }
            }
            best.sort_by(|a, b| b.0.total_cmp(&a.0));
            best.truncate(per_cell);
            best
        })
        .collect();
    pts.sort_by(|a, b| b.0.total_cmp(&a.0));
    pts.truncate(max_points);

    // Beschreiber auf stärker geglättetem Bild
    let d = blur(g, 2.0);
    let pairs = brief_pairs();
    pts.par_iter()
        .map(|&(_, x, y)| {
            let mut desc = [0u64; 4];
            for (k, &(ax, ay, bx, by)) in pairs.iter().enumerate() {
                let a = d.at(x as isize + ax as isize, y as isize + ay as isize);
                let b = d.at(x as isize + bx as isize, y as isize + by as isize);
                if a < b {
                    desc[k / 64] |= 1 << (k % 64);
                }
            }
            Keypoint {
                x: x as f32,
                y: y as f32,
                desc,
            }
        })
        .collect()
}

fn hamming(a: &[u64; 4], b: &[u64; 4]) -> u32 {
    (0..4).map(|i| (a[i] ^ b[i]).count_ones()).sum()
}

/// Gegenseitig beste Zuordnungen mit Verhältnistest: (Index in a, Index in b).
pub fn match_points(a: &[Keypoint], b: &[Keypoint]) -> Vec<(usize, usize)> {
    let best = |from: &[Keypoint], to: &[Keypoint]| -> Vec<Option<usize>> {
        from.par_iter()
            .map(|p| {
                let (mut b1, mut b2, mut bi) = (u32::MAX, u32::MAX, 0);
                for (j, q) in to.iter().enumerate() {
                    let d = hamming(&p.desc, &q.desc);
                    if d < b1 {
                        b2 = b1;
                        b1 = d;
                        bi = j;
                    } else if d < b2 {
                        b2 = d;
                    }
                }
                (b1 < 64 && (b1 as f32) < 0.8 * b2 as f32).then_some(bi)
            })
            .collect()
    };
    let ab = best(a, b);
    let ba = best(b, a);
    ab.iter()
        .enumerate()
        .filter_map(|(i, j)| j.filter(|&j| ba[j] == Some(i)).map(|j| (i, j)))
        .collect()
}

// --- Homografie -------------------------------------------------------------------

/// Normierung nach Hartley: Schwerpunkt 0, mittlerer Abstand √2.
fn normalizer(pts: &[(f64, f64)]) -> Mat3 {
    let n = pts.len().max(1) as f64;
    let (mx, my) = pts
        .iter()
        .fold((0.0, 0.0), |a, p| (a.0 + p.0 / n, a.1 + p.1 / n));
    let d = pts
        .iter()
        .map(|p| ((p.0 - mx).powi(2) + (p.1 - my).powi(2)).sqrt())
        .sum::<f64>()
        / n;
    let s = if d > 1e-9 {
        std::f64::consts::SQRT_2 / d
    } else {
        1.0
    };
    [[s, 0.0, -s * mx], [0.0, s, -s * my], [0.0, 0.0, 1.0]]
}

/// Homografie aus Punktpaaren (src → dst) per Ausgleichsrechnung (h33 = 1).
pub fn fit_homography(src: &[(f64, f64)], dst: &[(f64, f64)]) -> Option<Mat3> {
    if src.len() < 4 {
        return None;
    }
    let ts = normalizer(src);
    let td = normalizer(dst);
    let mut ata = vec![vec![0.0; 8]; 8];
    let mut atb = vec![0.0; 8];
    for (p, q) in src.iter().zip(dst) {
        let (x, y) = apply(&ts, p.0, p.1);
        let (u, v) = apply(&td, q.0, q.1);
        let rows = [
            ([x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y], u),
            ([0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y], v),
        ];
        for (r, b) in rows {
            for i in 0..8 {
                atb[i] += r[i] * b;
                for j in 0..8 {
                    ata[i][j] += r[i] * r[j];
                }
            }
        }
    }
    let h = solve(ata, atb)?;
    let hn = [[h[0], h[1], h[2]], [h[3], h[4], h[5]], [h[6], h[7], 1.0]];
    let h = mul(&invert(&td)?, &mul(&hn, &ts));
    let s = h[2][2];
    (s.abs() > 1e-12).then(|| h.map(|r| r.map(|v| v / s)))
}

/// Ergebnis einer Ausrichtung.
#[derive(Debug, Clone)]
pub struct Fit {
    /// Bildet Punkte des zweiten Bildes auf das erste ab.
    pub h: Mat3,
    pub inliers: usize,
    pub matches: usize,
}

/// Robuste Homografie b → a (RANSAC). `threshold` in Pixeln.
pub fn ransac(
    a: &[Keypoint],
    b: &[Keypoint],
    matches: &[(usize, usize)],
    threshold: f64,
) -> Option<Fit> {
    if matches.len() < 8 {
        return None;
    }
    let pa: Vec<(f64, f64)> = matches
        .iter()
        .map(|&(i, _)| (a[i].x as f64, a[i].y as f64))
        .collect();
    let pb: Vec<(f64, f64)> = matches
        .iter()
        .map(|&(_, j)| (b[j].x as f64, b[j].y as f64))
        .collect();
    let n = matches.len();
    let t2 = threshold * threshold;
    let count = |h: &Mat3| -> Vec<usize> {
        (0..n)
            .filter(|&i| {
                let (x, y) = apply(h, pb[i].0, pb[i].1);
                (x - pa[i].0).powi(2) + (y - pa[i].1).powi(2) < t2
            })
            .collect()
    };
    // Deterministischer Zufall, damit Ergebnisse reproduzierbar sind
    let trial = |it: usize| -> Option<(usize, Mat3)> {
        let mut s = (it as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
        let mut pick = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s % n as u64) as usize
        };
        let mut idx = [0usize; 4];
        for k in 0..4 {
            let mut v = pick();
            let mut guard = 0;
            while idx[..k].contains(&v) && guard < 20 {
                v = pick();
                guard += 1;
            }
            idx[k] = v;
        }
        let h = fit_homography(
            &idx.iter().map(|&i| pb[i]).collect::<Vec<_>>(),
            &idx.iter().map(|&i| pa[i]).collect::<Vec<_>>(),
        )?;
        Some((count(&h).len(), h))
    };
    // Adaptiv: in Blöcken rechnen und aufhören, sobald mit 99,9 % Sicherheit
    // eine Stichprobe nur aus passenden Paaren dabei war (bei vielen guten
    // Paaren nach wenigen Hundert statt 2000 Versuchen – zählt beim Panorama,
    // wo jedes Bildpaar geprüft wird).
    const MAX_ITER: usize = 2000;
    const BLOCK: usize = 128;
    let mut best: Option<(usize, Mat3)> = None;
    let mut done = 0;
    while done < MAX_ITER {
        let block = (done..done + BLOCK)
            .into_par_iter()
            .filter_map(trial)
            .max_by_key(|(c, _)| *c);
        if let Some(b) = block
            && best.as_ref().is_none_or(|x| b.0 > x.0)
        {
            best = Some(b);
        }
        done += BLOCK;
        let ratio = best.as_ref().map_or(0.0, |b| b.0 as f64 / n as f64);
        let p_good = ratio.powi(4);
        let needed = if p_good >= 1.0 - 1e-12 {
            0.0
        } else if p_good <= 1e-9 {
            f64::INFINITY
        } else {
            (1.0f64 - 0.999).ln() / (1.0 - p_good).ln()
        };
        if done as f64 >= needed {
            break;
        }
    }
    let best = best?;
    // Ausgleich über alle passenden Paare, zweimal (Menge kann wachsen)
    let mut h = best.1;
    let mut inl = count(&h);
    for _ in 0..2 {
        if inl.len() < 4 {
            break;
        }
        // Scheitert der Ausgleich (z. B. Punkte fast auf einer Linie), gilt das bisherige Ergebnis
        let Some(refined) = fit_homography(
            &inl.iter().map(|&i| pb[i]).collect::<Vec<_>>(),
            &inl.iter().map(|&i| pa[i]).collect::<Vec<_>>(),
        ) else {
            break;
        };
        let next = count(&refined);
        if next.len() < inl.len() {
            break;
        }
        h = refined;
        inl = next;
    }
    Some(Fit {
        h,
        inliers: inl.len(),
        matches: n,
    })
}

/// Arbeitsbild für die Ausrichtung: verkleinert, mit Merkmalen.
pub struct Features {
    /// Faktor verkleinert / Original
    pub scale: f64,
    pub w: usize,
    pub h: usize,
    pub points: Vec<Keypoint>,
}

pub fn features(img: &ImageF32, edge: usize, max_points: usize) -> Features {
    let small = img.downscale_to(edge);
    let scale = small.width as f64 / img.width as f64;
    let g = feature_gray(&small);
    Features {
        scale,
        w: small.width,
        h: small.height,
        points: detect(&g, max_points),
    }
}

/// Homografie b → a in Originalpixeln, oder `None`, wenn zu wenig passt.
pub fn align(a: &Features, b: &Features, min_inliers: usize) -> Option<Fit> {
    let m = match_points(&a.points, &b.points);
    let fit = ransac(&a.points, &b.points, &m, 2.5)?;
    if fit.inliers < min_inliers {
        return None;
    }
    Some(Fit {
        h: rescale(&fit.h, b.scale, a.scale),
        ..fit
    })
}

/// Graubild-Pyramide für die pixelgenaue Nachführung (`refine`).
pub struct Dense {
    /// (Graubild, Gradient x, Gradient y, Faktor klein/Original), grob → fein
    levels: Vec<(Gray, Vec<f32>, Vec<f32>, f64)>,
}

/// Arbeitsbilder für `refine`: Kante `edge` und halb so groß, leicht
/// weichgezeichnet (gleicht unterschiedliche Schärfe etwas an).
pub fn dense(img: &ImageF32, edge: usize) -> Dense {
    let levels = [edge / 2, edge]
        .iter()
        .map(|&e| {
            let small = img.downscale_to(e.max(64));
            let scale = small.width as f64 / img.width as f64;
            let g = blur(&feature_gray(&small), 1.2);
            let (w, h) = (g.w, g.h);
            let gx = (0..w * h)
                .into_par_iter()
                .map(|i| {
                    let (x, y) = ((i % w) as isize, (i / w) as isize);
                    (g.at(x + 1, y) - g.at(x - 1, y)) * 0.5
                })
                .collect();
            let gy = (0..w * h)
                .into_par_iter()
                .map(|i| {
                    let (x, y) = ((i % w) as isize, (i / w) as isize);
                    (g.at(x, y + 1) - g.at(x, y - 1)) * 0.5
                })
                .collect();
            (g, gx, gy, scale)
        })
        .collect();
    Dense { levels }
}

fn bilinear(data: &[f32], w: usize, h: usize, x: f64, y: f64) -> Option<f32> {
    if x < 0.0 || y < 0.0 || x > (w - 1) as f64 || y > (h - 1) as f64 {
        return None;
    }
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let top = data[y0 * w + x0] * (1.0 - tx) + data[y0 * w + x1] * tx;
    let bot = data[y1 * w + x0] * (1.0 - tx) + data[y1 * w + x1] * tx;
    Some(top * (1.0 - ty) + bot * ty)
}

/// Verfeinert eine Homografie `h` (Koordinaten in `a` → Koordinaten in `b`,
/// Originalpixel) durch Angleichen der Helligkeitsverläufe (Gauß-Newton,
/// grob → fein). Unempfindlich gegen unterschiedliche Schärfe, anders als
/// Eckpunkte, deren Lage sich beim Weichzeichnen verschiebt.
pub fn refine(a: &Dense, b: &Dense, h: &Mat3) -> Mat3 {
    let mut h = *h;
    for (la, lb) in a.levels.iter().zip(&b.levels) {
        let (ga, _, _, sa) = la;
        let (gb, bx, by, sb) = lb;
        // Normierte Koordinaten je Bild: Mitte 0, halbe Breite 1
        let norm = |g: &Gray, s: f64| -> Mat3 {
            let c = g.w as f64 / 2.0;
            let m = [
                [1.0 / c, 0.0, -(g.w as f64 - 1.0) / 2.0 / c],
                [0.0, 1.0 / c, -(g.h as f64 - 1.0) / 2.0 / c],
                [0.0, 0.0, 1.0],
            ];
            mul(&m, &[[s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, 1.0]])
        };
        let (ta, tb) = (norm(ga, *sa), norm(gb, *sb));
        let Some(ta_inv) = invert(&ta) else { return h };
        let mut hn = mul(&tb, &mul(&h, &ta_inv));
        let z = hn[2][2];
        if !z.is_finite() || z.abs() < 1e-9 {
            return h;
        }
        hn = hn.map(|r| r.map(|v| v / z));
        let cb = gb.w as f64 / 2.0;
        let (ocx, ocy) = ((gb.w as f64 - 1.0) / 2.0, (gb.h as f64 - 1.0) / 2.0);
        let ca = ga.w as f64 / 2.0;
        let (acx, acy) = ((ga.w as f64 - 1.0) / 2.0, (ga.h as f64 - 1.0) / 2.0);
        // Normalgleichungen, Anzahl und mittlere (robuste) Abweichung für `hc`
        let eval = |hc: Mat3| {
            (0..ga.h)
                .into_par_iter()
                .map(|y| {
                    let mut jtj = [[0f64; 8]; 8];
                    let mut jtr = [0f64; 8];
                    let mut cnt = 0usize;
                    let mut cost = 0f64;
                    let yn = (y as f64 - acy) / ca;
                    for x in 0..ga.w {
                        let xn = (x as f64 - acx) / ca;
                        let zz = hc[2][0] * xn + hc[2][1] * yn + 1.0;
                        if zz.abs() < 1e-6 {
                            continue;
                        }
                        let u = (hc[0][0] * xn + hc[0][1] * yn + hc[0][2]) / zz;
                        let v = (hc[1][0] * xn + hc[1][1] * yn + hc[1][2]) / zz;
                        let (px, py) = (u * cb + ocx, v * cb + ocy);
                        let Some(ib) = bilinear(&gb.data, gb.w, gb.h, px, py) else {
                            continue;
                        };
                        let gx = bilinear(bx, gb.w, gb.h, px, py).unwrap_or(0.0) as f64 * cb;
                        let gy = bilinear(by, gb.w, gb.h, px, py).unwrap_or(0.0) as f64 * cb;
                        if gx * gx + gy * gy < 1e-6 {
                            continue;
                        }
                        let r = ib as f64 - ga.data[y * ga.w + x] as f64;
                        let j = [
                            gx * xn / zz,
                            gx * yn / zz,
                            gx / zz,
                            gy * xn / zz,
                            gy * yn / zz,
                            gy / zz,
                            -(gx * u + gy * v) * xn / zz,
                            -(gx * u + gy * v) * yn / zz,
                        ];
                        // Robust (Huber): starke Abweichungen zählen weniger
                        let wgt = if r.abs() > 0.5 { 0.5 / r.abs() } else { 1.0 };
                        cost += if r.abs() > 0.5 {
                            0.5 * (r.abs() - 0.25)
                        } else {
                            0.5 * r * r
                        };
                        for p in 0..8 {
                            jtr[p] += wgt * j[p] * r;
                            for q in p..8 {
                                jtj[p][q] += wgt * j[p] * j[q];
                            }
                        }
                        cnt += 1;
                    }
                    (jtj, jtr, cnt, cost)
                })
                .reduce(
                    || ([[0f64; 8]; 8], [0f64; 8], 0, 0.0),
                    |mut x, y| {
                        for p in 0..8 {
                            x.1[p] += y.1[p];
                            for q in 0..8 {
                                x.0[p][q] += y.0[p][q];
                            }
                        }
                        x.2 += y.2;
                        x.3 += y.3;
                        x
                    },
                )
        };
        // Bestes Ergebnis merken: Bei sich wiederholenden Mustern oder sehr
        // unterschiedlicher Schärfe kann die Iteration abdriften.
        let mut best: Option<(f64, Mat3)> = None;
        for it in 0..=25 {
            let hc = hn;
            let (jtj, jtr, cnt, cost) = eval(hc);
            if cnt < 500 {
                break;
            }
            let mean = cost / cnt as f64;
            if best.as_ref().is_none_or(|b| mean < b.0) {
                best = Some((mean, hc));
            }
            if it == 25 {
                break;
            }
            let mut m: Vec<Vec<f64>> = (0..8)
                .map(|p| {
                    (0..8)
                        .map(|q| if q >= p { jtj[p][q] } else { jtj[q][p] })
                        .collect()
                })
                .collect();
            for (p, row) in m.iter_mut().enumerate() {
                row[p] *= 1.0 + 1e-3;
            }
            let Some(dp) = solve(m, jtr.iter().map(|v| -v).collect()) else {
                break;
            };
            let idx = [
                (0, 0),
                (0, 1),
                (0, 2),
                (1, 0),
                (1, 1),
                (1, 2),
                (2, 0),
                (2, 1),
            ];
            for (k, &(r, c)) in idx.iter().enumerate() {
                hn[r][c] += dp[k];
            }
            if !dp.iter().all(|v| v.is_finite()) {
                break;
            }
            if dp.iter().map(|v| v.abs()).fold(0.0, f64::max) < 1e-6 {
                // Konvergiert: das letzte Ergebnis ist praktisch gleich gut
                best = Some((f64::MIN, hn));
                break;
            }
        }
        // Zu wenig Überdeckung: Ausgangslage behalten
        let Some((_, hn)) = best else { return h };
        let Some(tb_inv) = invert(&tb) else { return h };
        h = mul(&tb_inv, &mul(&hn, &ta));
    }
    h
}

/// Bilineares Abtasten; `None` außerhalb.
#[inline]
pub fn sample(img: &ImageF32, x: f64, y: f64) -> Option<[f32; 3]> {
    let (w, h) = (img.width as f64, img.height as f64);
    if !(x >= 0.0 && y >= 0.0 && x <= w - 1.0 && y <= h - 1.0) {
        return None;
    }
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(img.width - 1), (y0 + 1).min(img.height - 1));
    let (tx, ty) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let p = |xx: usize, yy: usize| img.data[yy * img.width + xx];
    let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
    let mut o = [0f32; 3];
    for k in 0..3 {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bot = c[k] + (d[k] - c[k]) * tx;
        o[k] = top + (bot - top) * ty;
    }
    Some(o)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Strukturreiches Testbild (Rauschen aus Hash + Formen)
    pub(crate) fn texture(w: usize, h: usize, seed: u32) -> ImageF32 {
        let hash = |x: i64, y: i64| -> f32 {
            let mut v =
                (x as u32).wrapping_mul(374761393) ^ (y as u32).wrapping_mul(668265263) ^ seed;
            v = (v ^ (v >> 13)).wrapping_mul(1274126177);
            (v >> 8) as f32 / (1u32 << 24) as f32
        };
        ImageF32::from_fn(w, h, |x, y| {
            // Blöcke verschiedener Größe → Ecken
            let a = hash(x as i64 / 23, y as i64 / 17);
            let b = hash(x as i64 / 7 + 1000, y as i64 / 9);
            let v = 0.05 + 0.6 * a + 0.3 * b;
            [v, v * 0.9, v * 0.8]
        })
    }

    pub(crate) fn warp(img: &ImageF32, h_out_to_in: &Mat3, w: usize, hh: usize) -> ImageF32 {
        ImageF32::from_fn(w, hh, |x, y| {
            let (sx, sy) = apply(h_out_to_in, x as f64, y as f64);
            sample(img, sx, sy).unwrap_or([0.0; 3])
        })
    }

    #[test]
    fn solves_and_inverts() {
        let h = [[1.1, 0.05, 3.0], [-0.02, 0.95, -4.0], [1e-5, -2e-5, 1.0]];
        let i = invert(&h).unwrap();
        let id = mul(&h, &i);
        for r in 0..3 {
            for c in 0..3 {
                assert!((id[r][c] - IDENTITY[r][c]).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn recovers_known_homography_across_exposures() {
        let base = texture(900, 600, 7);
        // b ist a, leicht gedreht/verschoben/skaliert und 2 Blenden dunkler
        let truth: Mat3 = [[0.99, -0.03, 14.0], [0.035, 1.01, -9.0], [2e-6, 1e-6, 1.0]];
        let b = warp(&base, &truth, 900, 600);
        let b = ImageF32 {
            data: b.data.iter().map(|p| p.map(|v| v * 0.25)).collect(),
            ..b
        };
        let fa = features(&base, 900, 1500);
        let fb = features(&b, 900, 1500);
        assert!(
            fa.points.len() > 300,
            "zu wenige Merkmale: {}",
            fa.points.len()
        );
        let fit = align(&fa, &fb, 30).expect("Ausrichtung");
        // fit.h bildet b → a ab; truth bildet b-Koordinaten auf a-Koordinaten ab
        for &(x, y) in &[(100.0, 100.0), (800.0, 500.0), (450.0, 300.0)] {
            let (u, v) = apply(&fit.h, x, y);
            let (tu, tv) = apply(&truth, x, y);
            assert!(
                (u - tu).abs() < 1.0 && (v - tv).abs() < 1.0,
                "({x},{y}) → ({u:.1},{v:.1}) statt ({tu:.1},{tv:.1})"
            );
        }
    }
}
