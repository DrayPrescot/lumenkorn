//! RCD-Demosaicing (Ratio Corrected Demosaicing, Luis Sanz Rodríguez) für
//! Bayer-Sensoren – das Standardverfahren von RawTherapee und darktable.
//!
//! Gegenüber PPG (rawler) bleiben feine Strukturen schärfer und es entstehen
//! weniger Labyrinth- und Reißverschluss-Muster an Kanten. Gerechnet wird in
//! Kacheln (parallel, wenig Speicher). Am Bildrand wird das Mosaik gespiegelt –
//! um eine gerade Zahl Pixel, damit das Farbmuster erhalten bleibt.

use rayon::prelude::*;

/// Kachelgröße (Innenbereich) und Rand. Beide gerade, damit die Bayer-Phase
/// in der Kachel der im Bild entspricht.
const TILE: usize = 240;
const BORDER: usize = 10;
const EPS: f32 = 1e-5;
const EPSSQ: f32 = 1e-10;

/// Bayer-Mosaik → RGB.
///
/// `mosaic` hat die Breite `width`; demosaikiert wird der Ausschnitt
/// `(x0, y0, w, h)`. `pattern[y & 1][x & 1]` ist die Farbe (0 = R, 1 = G,
/// 2 = B) an Position `(x, y)` relativ zum Ausschnitt.
pub fn rcd(
    mosaic: &[f32],
    width: usize,
    (x0, y0, w, h): (usize, usize, usize, usize),
    pattern: [[usize; 2]; 2],
) -> Vec<[f32; 3]> {
    assert!(w > BORDER && h > BORDER, "Bild zu klein für RCD");
    // Spiegeln ohne Phasenwechsel: −1 → 1, w → w − 2
    let reflect = |v: isize, n: usize| -> usize {
        let n = n as isize;
        let mut v = v;
        loop {
            if v < 0 {
                v = -v;
            } else if v >= n {
                v = 2 * (n - 1) - v;
            } else {
                return v as usize;
            }
        }
    };
    let at = |x: isize, y: isize| mosaic[(y0 + reflect(y, h)) * width + x0 + reflect(x, w)];

    let mut out = vec![[0f32; 3]; w * h];
    let tiles_x = w.div_ceil(TILE);
    // Je Kachelzeile ein Streifen der Ausgabe – Kacheln einer Zeile nacheinander
    out.par_chunks_mut(TILE * w)
        .enumerate()
        .for_each(|(ty, band)| {
            let mut t = Tile::new();
            for tx in 0..tiles_x {
                let (ox, oy) = (tx * TILE, ty * TILE);
                t.load(|x, y| {
                    at(
                        ox as isize + x as isize - BORDER as isize,
                        oy as isize + y as isize - BORDER as isize,
                    )
                });
                t.run(pattern);
                let rows = band.len() / w;
                let cols = TILE.min(w - ox);
                for r in 0..rows {
                    for c in 0..cols {
                        let i = (r + BORDER) * SIZE + c + BORDER;
                        band[r * w + ox + c] = [t.rgb[0][i], t.rgb[1][i], t.rgb[2][i]];
                    }
                }
            }
        });
    out
}

const SIZE: usize = TILE + 2 * BORDER;
const W1: usize = SIZE;
const W2: usize = 2 * SIZE;
const W3: usize = 3 * SIZE;
const W4: usize = 4 * SIZE;

struct Tile {
    cfa: Vec<f32>,
    rgb: [Vec<f32>; 3],
    /// Vertikal/horizontal-Entscheidung (0 = vertikal, 1 = horizontal)
    vh: Vec<f32>,
    /// Diagonal-Entscheidung P/Q
    pq: Vec<f32>,
    /// Tiefpass an R/B-Positionen
    lpf: Vec<f32>,
    buf_a: Vec<f32>,
    buf_b: Vec<f32>,
}

impl Tile {
    fn new() -> Self {
        let z = || vec![0f32; SIZE * SIZE];
        Tile {
            cfa: z(),
            rgb: [z(), z(), z()],
            vh: z(),
            pq: z(),
            lpf: z(),
            buf_a: z(),
            buf_b: z(),
        }
    }

    fn load(&mut self, get: impl Fn(usize, usize) -> f32) {
        for y in 0..SIZE {
            for x in 0..SIZE {
                self.cfa[y * SIZE + x] = get(x, y);
            }
        }
    }

    fn run(&mut self, pattern: [[usize; 2]; 2]) {
        let fc = |x: usize, y: usize| pattern[y & 1][x & 1];
        let Tile {
            cfa,
            rgb,
            vh,
            pq,
            lpf,
            buf_a,
            buf_b,
        } = self;
        let cfa = &cfa[..];

        for v in rgb.iter_mut() {
            v.fill(0.0);
        }
        for y in 0..SIZE {
            for x in 0..SIZE {
                let i = y * SIZE + x;
                rgb[fc(x, y)][i] = cfa[i];
            }
        }

        // 1.1: quadrierter Hochpass vertikal (a) und horizontal (b)
        let hp = |c: &[f32], i: usize, s: usize| {
            let v = (c[i - 3 * s] - c[i - s] - c[i + s] + c[i + 3 * s])
                - 3.0 * (c[i - 2 * s] + c[i + 2 * s])
                + 6.0 * c[i];
            v * v
        };
        for y in 3..SIZE - 3 {
            for x in 3..SIZE - 3 {
                let i = y * SIZE + x;
                buf_a[i] = hp(cfa, i, W1);
                buf_b[i] = hp(cfa, i, 1);
            }
        }
        // 1.2: Richtungsstärke
        for y in 4..SIZE - 4 {
            for x in 4..SIZE - 4 {
                let i = y * SIZE + x;
                let v = (buf_a[i - W1] + buf_a[i] + buf_a[i + W1]).max(EPSSQ);
                let h = (buf_b[i - 1] + buf_b[i] + buf_b[i + 1]).max(EPSSQ);
                vh[i] = v / (v + h);
            }
        }

        // 2: Tiefpass an R/B-Positionen
        for y in 2..SIZE - 2 {
            let start = 2 + (fc(0, y) & 1);
            for x in (start..SIZE - 2).step_by(2) {
                let i = y * SIZE + x;
                lpf[i] = cfa[i]
                    + 0.5 * (cfa[i - W1] + cfa[i + W1] + cfa[i - 1] + cfa[i + 1])
                    + 0.25
                        * (cfa[i - W1 - 1] + cfa[i - W1 + 1] + cfa[i + W1 - 1] + cfa[i + W1 + 1]);
            }
        }

        let disc = |d: &[f32], i: usize| {
            let central = d[i];
            let around = 0.25 * (d[i - W1 - 1] + d[i - W1 + 1] + d[i + W1 - 1] + d[i + W1 + 1]);
            if (0.5 - central).abs() < (0.5 - around).abs() {
                around
            } else {
                central
            }
        };

        // 3: Grün an R/B-Positionen
        for y in 4..SIZE - 4 {
            let start = 4 + (fc(0, y) & 1);
            for x in (start..SIZE - 4).step_by(2) {
                let i = y * SIZE + x;
                let vh_disc = disc(vh, i);
                let c = cfa;
                let n_grad = EPS
                    + (c[i - W1] - c[i + W1]).abs()
                    + (c[i] - c[i - W2]).abs()
                    + (c[i - W1] - c[i - W3]).abs()
                    + (c[i - W2] - c[i - W4]).abs();
                let s_grad = EPS
                    + (c[i - W1] - c[i + W1]).abs()
                    + (c[i] - c[i + W2]).abs()
                    + (c[i + W1] - c[i + W3]).abs()
                    + (c[i + W2] - c[i + W4]).abs();
                let w_grad = EPS
                    + (c[i - 1] - c[i + 1]).abs()
                    + (c[i] - c[i - 2]).abs()
                    + (c[i - 1] - c[i - 3]).abs()
                    + (c[i - 2] - c[i - 4]).abs();
                let e_grad = EPS
                    + (c[i - 1] - c[i + 1]).abs()
                    + (c[i] - c[i + 2]).abs()
                    + (c[i + 1] - c[i + 3]).abs()
                    + (c[i + 2] - c[i + 4]).abs();
                let l = lpf[i];
                let est = |nb: f32, lo: f32| nb * (1.0 + (l - lo) / (EPS + l + lo));
                let n_est = est(c[i - W1], lpf[i - W2]);
                let s_est = est(c[i + W1], lpf[i + W2]);
                let w_est = est(c[i - 1], lpf[i - 2]);
                let e_est = est(c[i + 1], lpf[i + 2]);
                let v_est = (s_grad * n_est + n_grad * s_est) / (n_grad + s_grad);
                let h_est = (w_grad * e_est + e_grad * w_est) / (e_grad + w_grad);
                rgb[1][i] = vh_disc * h_est + (1.0 - vh_disc) * v_est;
            }
        }

        // 4.1: Diagonal-Entscheidung (a = P, b = Q)
        for y in 3..SIZE - 3 {
            for x in 3..SIZE - 3 {
                let i = y * SIZE + x;
                let p = (cfa[i - W3 - 3] - cfa[i - W1 - 1] - cfa[i + W1 + 1] + cfa[i + W3 + 3])
                    - 3.0 * (cfa[i - W2 - 2] + cfa[i + W2 + 2])
                    + 6.0 * cfa[i];
                let q = (cfa[i - W3 + 3] - cfa[i - W1 + 1] - cfa[i + W1 - 1] + cfa[i + W3 - 3])
                    - 3.0 * (cfa[i - W2 + 2] + cfa[i + W2 - 2])
                    + 6.0 * cfa[i];
                buf_a[i] = p * p;
                buf_b[i] = q * q;
            }
        }
        for y in 4..SIZE - 4 {
            for x in 4..SIZE - 4 {
                let i = y * SIZE + x;
                let p = (buf_a[i - W1 - 1] + buf_a[i] + buf_a[i + W1 + 1]).max(EPSSQ);
                let q = (buf_b[i - W1 + 1] + buf_b[i] + buf_b[i + W1 - 1]).max(EPSSQ);
                pq[i] = p / (p + q);
            }
        }

        // 4.2: R an B- und B an R-Positionen
        for y in 4..SIZE - 4 {
            let start = 4 + (fc(0, y) & 1);
            for x in (start..SIZE - 4).step_by(2) {
                let i = y * SIZE + x;
                let c = 2 - fc(x, y);
                let pq_disc = disc(pq, i);
                let (rc, g) = (&rgb[c], &rgb[1]);
                let nw_grad = EPS
                    + (rc[i - W1 - 1] - rc[i + W1 + 1]).abs()
                    + (rc[i - W1 - 1] - rc[i - W3 - 3]).abs()
                    + (g[i] - g[i - W2 - 2]).abs();
                let ne_grad = EPS
                    + (rc[i - W1 + 1] - rc[i + W1 - 1]).abs()
                    + (rc[i - W1 + 1] - rc[i - W3 + 3]).abs()
                    + (g[i] - g[i - W2 + 2]).abs();
                let sw_grad = EPS
                    + (rc[i - W1 + 1] - rc[i + W1 - 1]).abs()
                    + (rc[i + W1 - 1] - rc[i + W3 - 3]).abs()
                    + (g[i] - g[i + W2 - 2]).abs();
                let se_grad = EPS
                    + (rc[i - W1 - 1] - rc[i + W1 + 1]).abs()
                    + (rc[i + W1 + 1] - rc[i + W3 + 3]).abs()
                    + (g[i] - g[i + W2 + 2]).abs();
                let nw = rc[i - W1 - 1] - g[i - W1 - 1];
                let ne = rc[i - W1 + 1] - g[i - W1 + 1];
                let sw = rc[i + W1 - 1] - g[i + W1 - 1];
                let se = rc[i + W1 + 1] - g[i + W1 + 1];
                let p_est = (nw_grad * se + se_grad * nw) / (nw_grad + se_grad);
                let q_est = (ne_grad * sw + sw_grad * ne) / (ne_grad + sw_grad);
                let v = g[i] + pq_disc * q_est + (1.0 - pq_disc) * p_est;
                rgb[c][i] = v;
            }
        }

        // 4.3: R und B an Grün-Positionen
        for y in 4..SIZE - 4 {
            // Grün in Spalte 0 → gerade Spalten, sonst ungerade
            let start = 4 + (fc(0, y) != 1) as usize;
            for x in (start..SIZE - 4).step_by(2) {
                let i = y * SIZE + x;
                let vh_disc = disc(vh, i);
                let g = &rgb[1];
                let g0 = g[i];
                let n1 = EPS + (g0 - g[i - W2]).abs();
                let s1 = EPS + (g0 - g[i + W2]).abs();
                let w1 = EPS + (g0 - g[i - 2]).abs();
                let e1 = EPS + (g0 - g[i + 2]).abs();
                let (gn, gs, gw, ge) = (g[i - W1], g[i + W1], g[i - 1], g[i + 1]);
                let mut res = [0f32; 2];
                for (k, c) in [0usize, 2].into_iter().enumerate() {
                    let r = &rgb[c];
                    let sn = (r[i - W1] - r[i + W1]).abs();
                    let ew = (r[i - 1] - r[i + 1]).abs();
                    let n_grad = n1 + sn + (r[i - W1] - r[i - W3]).abs();
                    let s_grad = s1 + sn + (r[i + W1] - r[i + W3]).abs();
                    let w_grad = w1 + ew + (r[i - 1] - r[i - 3]).abs();
                    let e_grad = e1 + ew + (r[i + 1] - r[i + 3]).abs();
                    let n_est = r[i - W1] - gn;
                    let s_est = r[i + W1] - gs;
                    let w_est = r[i - 1] - gw;
                    let e_est = r[i + 1] - ge;
                    let v_est = (n_grad * s_est + s_grad * n_est) / (n_grad + s_grad);
                    let h_est = (e_grad * w_est + w_grad * e_est) / (e_grad + w_grad);
                    res[k] = g0 + vh_disc * h_est + (1.0 - vh_disc) * v_est;
                }
                rgb[0][i] = res[0];
                rgb[2][i] = res[1];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rawler::imgop::sensor::Demosaic;

    const PATTERNS: [(&str, [[usize; 2]; 2]); 4] = [
        ("RGGB", [[0, 1], [1, 2]]),
        ("BGGR", [[2, 1], [1, 0]]),
        ("GRBG", [[1, 0], [2, 1]]),
        ("GBRG", [[1, 2], [0, 1]]),
    ];

    fn mosaic(img: &[[f32; 3]], w: usize, pattern: [[usize; 2]; 2]) -> Vec<f32> {
        img.iter()
            .enumerate()
            .map(|(i, p)| p[pattern[(i / w) & 1][(i % w) & 1]])
            .collect()
    }

    /// Detailreiches Testbild: Farbverläufe, schräge Kanten, feine Streifen,
    /// Ringe mit steigender Frequenz.
    fn synthetic(w: usize, h: usize) -> Vec<[f32; 3]> {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let base = [0.2 + 0.5 * x / w as f32, 0.3, 0.6 - 0.4 * y / h as f32];
                let edge = if (x * 0.8 + y * 0.35) % 37.0 < 18.0 {
                    1.0
                } else {
                    0.35
                };
                let r = ((x - w as f32 / 2.0).powi(2) + (y - h as f32 / 2.0).powi(2)).sqrt();
                let ring = 0.75 + 0.25 * (r * r / 900.0).cos();
                // feine, kontrastreiche Struktur nahe der Auflösungsgrenze
                let stripes = 0.65 + 0.35 * ((x + 0.3 * y) * 1.4).sin();
                base.map(|v| (v * edge * ring * stripes).clamp(0.0, 1.0))
            })
            .collect()
    }

    fn psnr(a: &[[f32; 3]], b: &[[f32; 3]]) -> f64 {
        let se: f64 = a
            .iter()
            .zip(b)
            .flat_map(|(p, q)| (0..3).map(move |c| ((p[c] - q[c]) as f64).powi(2)))
            .sum();
        10.0 * (1.0 / (se / (a.len() * 3) as f64)).log10()
    }

    #[test]
    fn flat_colors_survive_every_bayer_phase_and_border() {
        // Ungerade Größe und mehrere Kacheln, damit Ränder und Kachelgrenzen dabei sind
        let (w, h) = (TILE + 37, 2 * TILE + 13);
        let color = [0.62, 0.31, 0.12];
        let img = vec![color; w * h];
        for (name, pattern) in PATTERNS {
            let out = rcd(&mosaic(&img, w, pattern), w, (0, 0, w, h), pattern);
            let max = out
                .iter()
                .flat_map(|p| (0..3).map(move |c| (p[c] - color[c]).abs()))
                .fold(0f32, f32::max);
            assert!(max < 1e-4, "{name}: Abweichung {max}");
        }
    }

    #[test]
    fn region_of_interest_inside_a_larger_mosaic() {
        // Ausschnitt mit ungeradem Versatz: Muster relativ zum Ausschnitt
        let (fw, fh) = (300, 280);
        let full = synthetic(fw, fh);
        let pattern_full = [[0, 1], [1, 2]];
        let m = mosaic(&full, fw, pattern_full);
        let (x0, y0, w, h) = (3, 5, 250, 240);
        let pattern = [
            [
                pattern_full[y0 & 1][x0 & 1],
                pattern_full[y0 & 1][(x0 + 1) & 1],
            ],
            [
                pattern_full[(y0 + 1) & 1][x0 & 1],
                pattern_full[(y0 + 1) & 1][(x0 + 1) & 1],
            ],
        ];
        let out = rcd(&m, fw, (x0, y0, w, h), pattern);
        let truth: Vec<[f32; 3]> = (0..w * h)
            .map(|i| full[(y0 + i / w) * fw + x0 + i % w])
            .collect();
        assert!(psnr(&out, &truth) > 30.0, "{}", psnr(&out, &truth));
    }

    #[test]
    fn sharper_than_ppg() {
        let (w, h) = (400, 300);
        let truth = synthetic(w, h);
        for (name, pattern) in PATTERNS {
            let m = mosaic(&truth, w, pattern);
            let ours = rcd(&m, w, (0, 0, w, h), pattern);
            let pix = rawler::pixarray::PixF32::new_with(m, w, h);
            let ppg = rawler::imgop::sensor::bayer::ppg::PPGDemosaic::new().demosaic(
                &pix,
                &rawler::CFA::new(name),
                &rawler::cfa::PlaneColor::new("RGB"),
                pix.rect(),
            );
            // Rand ausnehmen (PPG rechnet dort nur bilinear)
            let inner = |v: &[[f32; 3]]| -> Vec<[f32; 3]> {
                (4..h - 4)
                    .flat_map(|y| (4..w - 4).map(move |x| y * w + x))
                    .map(|i| v[i])
                    .collect()
            };
            let (a, b) = (
                psnr(&inner(&ours), &inner(&truth)),
                psnr(&inner(&ppg.data), &inner(&truth)),
            );
            eprintln!("{name}: RCD {a:.2} dB, PPG {b:.2} dB");
            assert!(a > b + 1.0, "{name}: RCD {a:.2} dB, PPG {b:.2} dB");
            assert!(a > 30.0, "{name}: RCD {a:.2} dB");
        }
    }
}
