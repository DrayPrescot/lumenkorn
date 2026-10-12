//! RCD-Demosaicing (Ratio Corrected Demosaicing), CPU-Pfad.
//!
//! Aus darktable 5.0 `src/iop/demosaicing/rcd.c` nach Rust übertragen
//! (GPL-3.0-or-later). Originalverfahren RCD von Luis Sanz Rodríguez.
//!
//! Originalkopf der C-Datei:
//!
//! ```text
//! This file is part of darktable,
//! Copyright (C) 2010-2024 darktable developers.
//!
//! darktable is free software: you can redistribute it and/or modify
//! it under the terms of the GNU General Public License as published by
//! the Free Software Foundation, either version 3 of the License, or
//! (at your option) any later version.
//!
//! RATIO CORRECTED DEMOSAICING
//! Luis Sanz Rodríguez (luis.sanz.rodriguez(at)gmail(dot)com)
//! Release 2.3 @ 171125
//! Original code from https://github.com/LuisSR/RCD-Demosaicing
//! Licensed under the GNU GPL version 3
//!
//! The tiling coding was done by Ingo Weyrich (heckflosse67@gmx.de) from rawtherapee
//!
//! Luis Sanz Rodríguez significantly optimised the v 2.3 code and simplified the
//! directional coefficients in an exact, shorter and more performant formula.
//! In cooperation with Ingo Weyrich and Luis Sanz Rodríguez this has been tuned
//! for performance. Hanno Schwalm (hanno@schwalm-bremen.de)
//! ```
//!
//! Schleifenstruktur, Konstanten und Randbehandlung entsprechen dem Original.
//! Unterschiede: OpenMP durch rayon ersetzt (parallel je Kachelzeile), Ausgabe
//! ohne Alpha-Kanal, Indizes mit Bereichsprüfung.

// Noch nicht von der Pipeline benutzt
// Schleifen bewusst wie im C-Original (Zähler, Indizes)
#![allow(clippy::explicit_counter_loop, clippy::needless_range_loop)]

use rayon::prelude::*;

/// Kachelgröße (`DT_RCD_TILESIZE` aus demosaic.c).
const TS: usize = 112;
/// Rand zwischen Kacheln, damit keine Überlappungsfehler entstehen.
const RCD_BORDER: usize = 9;
/// Für die äußersten Kacheln genügt ein kleinerer Rand.
const RCD_MARGIN: usize = 7;
const RCD_TILEVALID: usize = TS - 2 * RCD_BORDER;
const W1: usize = TS;
const W2: usize = 2 * TS;
const W3: usize = 3 * TS;
const W4: usize = 4 * TS;
const T2: usize = TS * TS;
/// Toleranz gegen Division durch null.
const EPS: f32 = 1e-5;
const EPSSQ: f32 = 1e-10;

/// Skalierung `dt_iop_get_processed_maximum(piece)`: darktable nimmt
/// `max(1, processed_maximum[0..3])`. Das Maximum setzen Weißabgleich und
/// Lichter-Rekonstruktion; ohne diese Angabe ist es 1.0 (Standardwert der
/// Pipeline, `processed_maximum = {1,1,1}`). Daten sind hier schon auf 0..1
/// normiert, daher ist 1.0 der passende Standard; die Skalierung ist dann
/// eine Identität, bleibt aber wie im Original erhalten.
const DEFAULT_SCALER: f32 = 1.0;

/// darktables `FC(row, col, filters)`: Farbe (0 = R, 1 = G, 2 = B) an der Stelle.
#[inline(always)]
fn fc(row: usize, col: usize, filters: u32) -> usize {
    ((filters >> ((((row << 1) & 14) + (col & 1)) << 1)) & 3) as usize
}

/// Baut einen darktable-`filters`-Wert aus einem 2x2-Muster:
/// `pattern[y & 1][x & 1]` ist die Farbe (0 = R, 1 = G, 2 = B).
pub(crate) fn filters_from_pattern(pattern: [[usize; 2]; 2]) -> u32 {
    let mut f = 0u32;
    for row in 0..8usize {
        for col in 0..2usize {
            let c = (pattern[row & 1][col & 1] & 3) as u32;
            f |= c << (((row << 1) + col) << 1);
        }
    }
    f
}

#[inline(always)]
fn sqrf(x: f32) -> f32 {
    x * x
}

/// Negative Werte abfangen und normieren (`_safe_in`).
#[inline(always)]
fn safe_in(a: f32, scale: f32) -> f32 {
    a.max(0.0) * scale
}

#[inline(always)]
fn interpolatef(a: f32, b: f32, c: f32) -> f32 {
    a * (b - c) + c
}

/// Im Wesentlichen PPG, nur für den Randbereich (`rcd_ppg_border`).
/// `out` hat `width * height` Pixel.
fn rcd_ppg_border(
    out: &mut [[f32; 3]],
    input: &[f32],
    width: usize,
    height: usize,
    filters: u32,
    margin: usize,
) {
    let border = margin + 3;
    // Rand von etwa 3 Pixeln: Mittelwert der Nachbarn je Farbe
    for j in 0..height {
        let mut i = 0;
        while i < width {
            if i == 3 && j >= 3 && j + 3 < height && width - 3 > i {
                i = width - 3;
            }
            let mut sum = [0f32; 8];
            for y in j.saturating_sub(1)..(j + 2).min(height) {
                for x in i.saturating_sub(1)..(i + 2).min(width) {
                    let f = fc(y, x, filters);
                    sum[f] += input[y * width + x].max(0.0);
                    sum[f + 4] += 1.0;
                }
            }
            let f = fc(j, i, filters);
            for c in 0..3 {
                out[j * width + i][c] = if c != f && sum[c + 4] > 0.0 {
                    sum[c] / sum[c + 4]
                } else {
                    input[j * width + i].max(0.0)
                };
            }
            i += 1;
        }
    }

    // Grün (und native Kanäle) per PPG-Schätzung
    if height > 6 && width > 6 {
        for j in 3..height - 3 {
            let mut i = 3;
            while i < width - 3 {
                if i == border && j >= border && j + border < height && width - border > i {
                    i = width - border;
                }
                let idx = j * width + i;
                let c = fc(j, i, filters);
                let pc = input[idx].max(0.0);
                let mut color = [0f32; 3];
                if c == 0 || c == 2 {
                    color[c] = pc;
                    let g = |o: isize| input[(idx as isize + o) as usize].max(0.0);
                    let w = width as isize;
                    let pym = g(-w);
                    let pym2 = g(-2 * w);
                    let pym3 = g(-3 * w);
                    let pym_big = g(w);
                    let pym_big2 = g(2 * w);
                    let pym_big3 = g(3 * w);
                    let pxm = g(-1);
                    let pxm2 = g(-2);
                    let pxm3 = g(-3);
                    let px_big = g(1);
                    let px_big2 = g(2);
                    let px_big3 = g(3);

                    let guessx = (pxm + pc + px_big) * 2.0 - px_big2 - pxm2;
                    let diffx = ((pxm2 - pc).abs() + (px_big2 - pc).abs() + (pxm - px_big).abs())
                        * 3.0
                        + ((px_big3 - px_big).abs() + (pxm3 - pxm).abs()) * 2.0;
                    let guessy = (pym + pc + pym_big) * 2.0 - pym_big2 - pym2;
                    let diffy = ((pym2 - pc).abs() + (pym_big2 - pc).abs() + (pym - pym_big).abs())
                        * 3.0
                        + ((pym_big3 - pym_big).abs() + (pym3 - pym).abs()) * 2.0;
                    if diffx > diffy {
                        // vertikal schätzen
                        let m = pym.min(pym_big);
                        let mm = pym.max(pym_big);
                        color[1] = (guessy * 0.25).min(mm).max(m);
                    } else {
                        let m = pxm.min(px_big);
                        let mm = pxm.max(px_big);
                        color[1] = (guessx * 0.25).min(mm).max(m);
                    }
                } else {
                    color[1] = pc;
                }
                out[idx] = color;
                i += 1;
            }
        }
    }

    // Für alle Pixel: fehlende Farben aus den Nachbarn
    if height > 2 && width > 2 {
        for j in 1..height - 1 {
            let mut i = 1;
            while i < width - 1 {
                if i == margin && j >= margin && j + margin < height && width - margin > i {
                    i = width - margin;
                }
                let idx = j * width + i;
                let c = fc(j, i, filters);
                let mut color = out[idx];
                if c & 1 == 1 {
                    // Rot und Blau auf Grün: 4er-Nachbarschaft
                    let nt = out[idx - width];
                    let nb = out[idx + width];
                    let nl = out[idx - 1];
                    let nr = out[idx + 1];
                    if fc(j, i + 1, filters) == 0 {
                        // rote Nachbarn in derselben Zeile
                        color[2] = (nt[2] + nb[2] + 2.0 * color[1] - nt[1] - nb[1]) * 0.5;
                        color[0] = (nl[0] + nr[0] + 2.0 * color[1] - nl[1] - nr[1]) * 0.5;
                    } else {
                        // blaue Nachbarn
                        color[0] = (nt[0] + nb[0] + 2.0 * color[1] - nt[1] - nb[1]) * 0.5;
                        color[2] = (nl[2] + nr[2] + 2.0 * color[1] - nl[1] - nr[1]) * 0.5;
                    }
                } else {
                    // 4-Sterne-Nachbarschaft
                    let ntl = out[idx - width - 1];
                    let ntr = out[idx - width + 1];
                    let nbl = out[idx + width - 1];
                    let nbr = out[idx + width + 1];
                    // Rot: Blau füllen (k = 2), Blau: Rot füllen (k = 0)
                    let k = if c == 0 { 2 } else { 0 };
                    let diff1 = (ntl[k] - nbr[k]).abs()
                        + (ntl[1] - color[1]).abs()
                        + (nbr[1] - color[1]).abs();
                    let guess1 = ntl[k] + nbr[k] + 2.0 * color[1] - ntl[1] - nbr[1];
                    let diff2 = (ntr[k] - nbl[k]).abs()
                        + (ntr[1] - color[1]).abs()
                        + (nbl[1] - color[1]).abs();
                    let guess2 = ntr[k] + nbl[k] + 2.0 * color[1] - ntr[1] - nbl[1];
                    color[k] = if diff1 > diff2 {
                        guess2 * 0.5
                    } else if diff1 < diff2 {
                        guess1 * 0.5
                    } else {
                        (guess1 + guess2) * 0.25
                    };
                }
                out[idx] = color;
                i += 1;
            }
        }
    }
}

/// Arbeitspuffer einer Kachel (wie die `dt_alloc_align_float`-Puffer).
struct Scratch {
    vh_dir: Vec<f32>,
    /// zugleich `lpf` (wird nicht überlappend benutzt)
    pq_dir: Vec<f32>,
    cfa: Vec<f32>,
    p_hpf: Vec<f32>,
    q_hpf: Vec<f32>,
    /// drei Ebenen R, G, B hintereinander
    rgb: Vec<f32>,
}

impl Scratch {
    fn new() -> Self {
        Self {
            vh_dir: vec![0.0; T2],
            pq_dir: vec![0.0; T2 / 2],
            cfa: vec![0.0; T2],
            p_hpf: vec![0.0; T2 / 2],
            q_hpf: vec![0.0; T2 / 2],
            rgb: vec![0.0; 3 * T2],
        }
    }
}

/// Eine Kachel berechnen und den gültigen Teil in `band` schreiben.
/// `band` beginnt bei Bildzeile `band_row0`.
#[allow(clippy::too_many_arguments)]
fn process_tile(
    s: &mut Scratch,
    input: &[f32],
    width: usize,
    height: usize,
    filters: u32,
    scaler: f32,
    revscaler: f32,
    (tile_vertical, num_vertical): (usize, usize),
    (tile_horizontal, num_horizontal): (usize, usize),
    band: &mut [[f32; 3]],
    band_row0: usize,
) {
    let row_start = tile_vertical * RCD_TILEVALID;
    let row_end = (row_start + TS).min(height);
    let col_start = tile_horizontal * RCD_TILEVALID;
    let col_end = (col_start + TS).min(width);
    let tile_rows = (row_end - row_start).min(TS);
    let tile_cols = (col_end - col_start).min(TS);

    let Scratch {
        vh_dir,
        pq_dir,
        cfa,
        p_hpf,
        q_hpf,
        rgb,
    } = s;
    // lpf teilt sich den Puffer mit PQ_Dir
    let lpf_is_pq = true;
    debug_assert!(lpf_is_pq);

    if row_start + TS > height || col_start + TS > width {
        // VH_Dir wird nur für (4,4)..(h-4,w-4) gefüllt, gelesen wird ab (3,3):
        // an Teilkacheln den Rand nullen
        vh_dir.fill(0.0);
        rgb.fill(0.0);
    }

    // Schritt 0: Daten füllen, negative Werte abfangen
    for row in row_start..row_end {
        let c0 = fc(row, col_start, filters);
        let c1 = fc(row, col_start + 1, filters);
        let mut indx = (row - row_start) * TS;
        let mut in_indx = row * width + col_start;
        for _ in col_start..col_end {
            let v = safe_in(input[in_indx], revscaler);
            cfa[indx] = v;
            rgb[c0 * T2 + indx] = v;
            rgb[c1 * T2 + indx] = v;
            indx += 1;
            in_indx += 1;
        }
    }

    // SCHRITT 1: vertikale und horizontale Interpolationsrichtung
    // 1.1: Quadrat des Hochpasses der Farbdifferenz
    let mut buffer_v = [[0f32; TS - 8]; 3];
    let vfilt = |cfa: &[f32], indx: usize| -> f32 {
        sqrf(
            (cfa[indx - W3] - cfa[indx - W1] - cfa[indx + W1] + cfa[indx + W3])
                - 3.0 * (cfa[indx - W2] + cfa[indx + W2])
                + 6.0 * cfa[indx],
        )
    };
    for row in 3..(tile_rows.saturating_sub(3)).min(5) {
        let mut indx = row * TS + 4;
        for col in 4..tile_cols.saturating_sub(4) {
            buffer_v[row - 3][col - 4] = vfilt(cfa, indx);
            indx += 1;
        }
    }

    // 1.2: Stärke der vertikalen/horizontalen Unterscheidung
    let mut buffer_h = [0f32; TS];
    // V0, V1, V2 zeigen auf Zeile -1, 0, +1; nach der Zeile wird rotiert
    let (mut v0, mut v1, mut v2) = (0usize, 1usize, 2usize);
    for row in 4..tile_rows.saturating_sub(4) {
        let mut indx = row * TS + 3;
        for col in 3..tile_cols - 3 {
            buffer_h[col - 3] = sqrf(
                (cfa[indx - 3] - cfa[indx - 1] - cfa[indx + 1] + cfa[indx + 3])
                    - 3.0 * (cfa[indx - 2] + cfa[indx + 2])
                    + 6.0 * cfa[indx],
            );
            indx += 1;
        }
        let mut indx = (row + 1) * TS + 4;
        for col in 4..tile_cols - 4 {
            buffer_v[v2][col - 4] = vfilt(cfa, indx);
            indx += 1;
        }
        let mut indx = row * TS + 4;
        for col in 4..tile_cols - 4 {
            let v_stat =
                EPSSQ.max(buffer_v[v0][col - 4] + buffer_v[v1][col - 4] + buffer_v[v2][col - 4]);
            let h_stat = EPSSQ.max(buffer_h[col - 4] + buffer_h[col - 3] + buffer_h[col - 2]);
            vh_dir[indx] = v_stat / (v_stat + h_stat);
            indx += 1;
        }
        // Zeilenzeiger rotieren
        let tmp = v0;
        v0 = v1;
        v1 = v2;
        v2 = tmp;
    }

    // SCHRITT 2: Tiefpass
    // 2.1: Grün, Rot und Blau der Rohdaten einbeziehen
    for row in 2..tile_rows - 2 {
        let mut col = 2 + (fc(row, 0, filters) & 1);
        let mut indx = row * TS + col;
        let mut lp_indx = indx / 2;
        while col < tile_cols - 2 {
            pq_dir[lp_indx] = cfa[indx]
                + 0.5 * (cfa[indx - W1] + cfa[indx + W1] + cfa[indx - 1] + cfa[indx + 1])
                + 0.25
                    * (cfa[indx - W1 - 1]
                        + cfa[indx - W1 + 1]
                        + cfa[indx + W1 - 1]
                        + cfa[indx + W1 + 1]);
            col += 2;
            indx += 2;
            lp_indx += 1;
        }
    }

    // SCHRITT 3: Grünkanal
    // 3.1: Grün an blauen und roten CFA-Positionen
    for row in 4..tile_rows - 4 {
        let mut col = 4 + (fc(row, 0, filters) & 1);
        let mut indx = row * TS + col;
        let mut lpindx = indx / 2;
        while col < tile_cols - 4 {
            let cfai = cfa[indx];

            // Gradienten in den vier Himmelsrichtungen
            let n_grad = EPS
                + (cfa[indx - W1] - cfa[indx + W1]).abs()
                + (cfai - cfa[indx - W2]).abs()
                + (cfa[indx - W1] - cfa[indx - W3]).abs()
                + (cfa[indx - W2] - cfa[indx - W4]).abs();
            let s_grad = EPS
                + (cfa[indx - W1] - cfa[indx + W1]).abs()
                + (cfai - cfa[indx + W2]).abs()
                + (cfa[indx + W1] - cfa[indx + W3]).abs()
                + (cfa[indx + W2] - cfa[indx + W4]).abs();
            let w_grad = EPS
                + (cfa[indx - 1] - cfa[indx + 1]).abs()
                + (cfai - cfa[indx - 2]).abs()
                + (cfa[indx - 1] - cfa[indx - 3]).abs()
                + (cfa[indx - 2] - cfa[indx - 4]).abs();
            let e_grad = EPS
                + (cfa[indx - 1] - cfa[indx + 1]).abs()
                + (cfai - cfa[indx + 2]).abs()
                + (cfa[indx + 1] - cfa[indx + 3]).abs()
                + (cfa[indx + 2] - cfa[indx + 4]).abs();

            // Schätzungen je Richtung
            let lpfi = pq_dir[lpindx];
            let n_est = cfa[indx - W1] * (lpfi + lpfi) / (EPS + lpfi + pq_dir[lpindx - W1]);
            let s_est = cfa[indx + W1] * (lpfi + lpfi) / (EPS + lpfi + pq_dir[lpindx + W1]);
            let w_est = cfa[indx - 1] * (lpfi + lpfi) / (EPS + lpfi + pq_dir[lpindx - 1]);
            let e_est = cfa[indx + 1] * (lpfi + lpfi) / (EPS + lpfi + pq_dir[lpindx + 1]);

            // vertikal und horizontal
            let v_est = (s_grad * n_est + n_grad * s_est) / (n_grad + s_grad);
            let h_est = (w_grad * e_est + e_grad * w_est) / (e_grad + w_grad);

            // G auf B und R: verfeinerte V/H-Unterscheidung
            let vh_central = vh_dir[indx];
            let vh_neigh = 0.25
                * (vh_dir[indx - W1 - 1]
                    + vh_dir[indx - W1 + 1]
                    + vh_dir[indx + W1 - 1]
                    + vh_dir[indx + W1 + 1]);
            let vh_disc = if (0.5 - vh_central).abs() < (0.5 - vh_neigh).abs() {
                vh_neigh
            } else {
                vh_central
            };

            rgb[T2 + indx] = interpolatef(vh_disc, h_est, v_est);

            col += 2;
            indx += 2;
            lpindx += 1;
        }
    }

    // SCHRITT 4: Rot und Blau
    // 4.0: Quadrat des Hochpasses der P/Q-Diagonalen
    for row in 3..tile_rows - 3 {
        let mut col = 3;
        let mut indx = row * TS + col;
        let mut indx2 = indx / 2;
        while col < tile_cols - 3 {
            p_hpf[indx2] = sqrf(
                (cfa[indx - W3 - 3] - cfa[indx - W1 - 1] - cfa[indx + W1 + 1] + cfa[indx + W3 + 3])
                    - 3.0 * (cfa[indx - W2 - 2] + cfa[indx + W2 + 2])
                    + 6.0 * cfa[indx],
            );
            q_hpf[indx2] = sqrf(
                (cfa[indx - W3 + 3] - cfa[indx - W1 + 1] - cfa[indx + W1 - 1] + cfa[indx + W3 - 3])
                    - 3.0 * (cfa[indx - W2 + 2] + cfa[indx + W2 - 2])
                    + 6.0 * cfa[indx],
            );
            col += 2;
            indx += 2;
            indx2 += 1;
        }
    }
    // 4.1: Stärke der P/Q-Unterscheidung
    for row in 4..tile_rows - 4 {
        let mut col = 4 + (fc(row, 0, filters) & 1);
        let indx = row * TS + col;
        let mut indx2 = indx / 2;
        let mut indx3 = (indx - W1 - 1) / 2;
        let mut indx4 = (indx + W1 - 1) / 2;
        while col < tile_cols - 4 {
            let p_stat = EPSSQ.max(p_hpf[indx3] + p_hpf[indx2] + p_hpf[indx4 + 1]);
            let q_stat = EPSSQ.max(q_hpf[indx3 + 1] + q_hpf[indx2] + q_hpf[indx4]);
            pq_dir[indx2] = p_stat / (p_stat + q_stat);
            col += 2;
            indx2 += 1;
            indx3 += 1;
            indx4 += 1;
        }
    }

    // 4.2: Rot und Blau an blauen und roten CFA-Positionen
    for row in 4..tile_rows - 4 {
        let mut col = 4 + (fc(row, 0, filters) & 1);
        let mut indx = row * TS + col;
        let c = 2 - fc(row, col, filters);
        let co = c * T2;
        let mut pqindx = indx / 2;
        let mut pqindx2 = (indx - W1 - 1) / 2;
        let mut pqindx3 = (indx + W1 - 1) / 2;
        while col < tile_cols - 4 {
            // verfeinerte P/Q-Unterscheidung
            let pq_central = pq_dir[pqindx];
            let pq_neigh = 0.25
                * (pq_dir[pqindx2] + pq_dir[pqindx2 + 1] + pq_dir[pqindx3] + pq_dir[pqindx3 + 1]);
            let pq_disc = if (0.5 - pq_central).abs() < (0.5 - pq_neigh).abs() {
                pq_neigh
            } else {
                pq_central
            };

            // Diagonale Gradienten
            let g0 = rgb[T2 + indx];
            let nw_grad = EPS
                + (rgb[co + indx - W1 - 1] - rgb[co + indx + W1 + 1]).abs()
                + (rgb[co + indx - W1 - 1] - rgb[co + indx - W3 - 3]).abs()
                + (g0 - rgb[T2 + indx - W2 - 2]).abs();
            let ne_grad = EPS
                + (rgb[co + indx - W1 + 1] - rgb[co + indx + W1 - 1]).abs()
                + (rgb[co + indx - W1 + 1] - rgb[co + indx - W3 + 3]).abs()
                + (g0 - rgb[T2 + indx - W2 + 2]).abs();
            let sw_grad = EPS
                + (rgb[co + indx - W1 + 1] - rgb[co + indx + W1 - 1]).abs()
                + (rgb[co + indx + W1 - 1] - rgb[co + indx + W3 - 3]).abs()
                + (g0 - rgb[T2 + indx + W2 - 2]).abs();
            let se_grad = EPS
                + (rgb[co + indx - W1 - 1] - rgb[co + indx + W1 + 1]).abs()
                + (rgb[co + indx + W1 + 1] - rgb[co + indx + W3 + 3]).abs()
                + (g0 - rgb[T2 + indx + W2 + 2]).abs();

            // Diagonale Farbdifferenzen
            let nw_est = rgb[co + indx - W1 - 1] - rgb[T2 + indx - W1 - 1];
            let ne_est = rgb[co + indx - W1 + 1] - rgb[T2 + indx - W1 + 1];
            let sw_est = rgb[co + indx + W1 - 1] - rgb[T2 + indx + W1 - 1];
            let se_est = rgb[co + indx + W1 + 1] - rgb[T2 + indx + W1 + 1];

            // P/Q-Schätzungen
            let p_est = (nw_grad * se_est + se_grad * nw_est) / (nw_grad + se_grad);
            let q_est = (ne_grad * sw_est + sw_grad * ne_est) / (ne_grad + sw_grad);

            // R auf B und B auf R
            rgb[co + indx] = g0 + interpolatef(pq_disc, q_est, p_est);

            col += 2;
            indx += 2;
            pqindx += 1;
            pqindx2 += 1;
            pqindx3 += 1;
        }
    }

    // 4.3: Rot und Blau an grünen CFA-Positionen
    for row in 4..tile_rows - 4 {
        let mut col = 4 + (fc(row, 1, filters) & 1);
        let mut indx = row * TS + col;
        while col < tile_cols - 4 {
            // verfeinerte V/H-Unterscheidung
            let vh_central = vh_dir[indx];
            let vh_neigh = 0.25
                * (vh_dir[indx - W1 - 1]
                    + vh_dir[indx - W1 + 1]
                    + vh_dir[indx + W1 - 1]
                    + vh_dir[indx + W1 + 1]);
            let vh_disc = if (0.5 - vh_central).abs() < (0.5 - vh_neigh).abs() {
                vh_neigh
            } else {
                vh_central
            };
            let rgb1 = rgb[T2 + indx];
            let n1 = EPS + (rgb1 - rgb[T2 + indx - W2]).abs();
            let s1 = EPS + (rgb1 - rgb[T2 + indx + W2]).abs();
            let w1 = EPS + (rgb1 - rgb[T2 + indx - 2]).abs();
            let e1 = EPS + (rgb1 - rgb[T2 + indx + 2]).abs();

            let rgb1mw1 = rgb[T2 + indx - W1];
            let rgb1pw1 = rgb[T2 + indx + W1];
            let rgb1m1 = rgb[T2 + indx - 1];
            let rgb1p1 = rgb[T2 + indx + 1];

            for c in [0usize, 2] {
                let co = c * T2;
                let sn_abs = (rgb[co + indx - W1] - rgb[co + indx + W1]).abs();
                let ew_abs = (rgb[co + indx - 1] - rgb[co + indx + 1]).abs();

                // Gradienten
                let n_grad = n1 + sn_abs + (rgb[co + indx - W1] - rgb[co + indx - W3]).abs();
                let s_grad = s1 + sn_abs + (rgb[co + indx + W1] - rgb[co + indx + W3]).abs();
                let w_grad = w1 + ew_abs + (rgb[co + indx - 1] - rgb[co + indx - 3]).abs();
                let e_grad = e1 + ew_abs + (rgb[co + indx + 1] - rgb[co + indx + 3]).abs();

                // Farbdifferenzen
                let n_est = rgb[co + indx - W1] - rgb1mw1;
                let s_est = rgb[co + indx + W1] - rgb1pw1;
                let w_est = rgb[co + indx - 1] - rgb1m1;
                let e_est = rgb[co + indx + 1] - rgb1p1;

                // vertikal und horizontal
                let v_est = (n_grad * s_est + s_grad * n_est) / (n_grad + s_grad);
                let h_est = (e_grad * w_est + w_grad * e_est) / (e_grad + w_grad);

                // R auf G und B auf G
                rgb[co + indx] = rgb1 + interpolatef(vh_disc, h_est, v_est);
            }

            col += 2;
            indx += 2;
        }
    }

    // Für die äußersten Kacheln genügt ein kleinerer Rand
    let first_vertical = row_start
        + if tile_vertical == 0 {
            RCD_MARGIN
        } else {
            RCD_BORDER
        };
    let last_vertical = row_end
        - if tile_vertical == num_vertical - 1 {
            RCD_MARGIN
        } else {
            RCD_BORDER
        };
    let first_horizontal = col_start
        + if tile_horizontal == 0 {
            RCD_MARGIN
        } else {
            RCD_BORDER
        };
    let last_horizontal = col_end
        - if tile_horizontal == num_horizontal - 1 {
            RCD_MARGIN
        } else {
            RCD_BORDER
        };
    for row in first_vertical..last_vertical {
        let mut idx = (row - row_start) * TS + first_horizontal - col_start;
        let mut o_idx = (row - band_row0) * width + first_horizontal;
        for _ in first_horizontal..last_horizontal {
            band[o_idx] = [
                scaler * rgb[idx].max(0.0),
                scaler * rgb[T2 + idx].max(0.0),
                scaler * rgb[2 * T2 + idx].max(0.0),
            ];
            o_idx += 1;
            idx += 1;
        }
    }
}

/// RCD-Demosaicing eines Bayer-Mosaiks (`rcd_demosaic` in darktable).
///
/// `data`: `width * height` Werte, zeilenweise, schon weißabgeglichen und
/// Lichter-rekonstruiert wie die Eingabe von darktables demosaic.
/// `filters`: darktable-CFA-Beschreibung (`dsc.filters`), siehe
/// [`filters_from_pattern`]. Rückgabe: `width * height` RGB-Tripel.
///
/// Die Skalierung (`dt_iop_get_processed_maximum`) beträgt hier
/// [`DEFAULT_SCALER`] = 1.0, weil darktables Standard `processed_maximum = 1`
/// ist.
pub(crate) fn rcd_demosaic(
    data: &[f32],
    width: usize,
    height: usize,
    filters: u32,
) -> Vec<[f32; 3]> {
    rcd_demosaic_scaled(data, width, height, filters, DEFAULT_SCALER)
}

/// Wie [`rcd_demosaic`], mit ausdrücklicher Skalierung (>= 1.0 wie in darktable).
pub(crate) fn rcd_demosaic_scaled(
    data: &[f32],
    width: usize,
    height: usize,
    filters: u32,
    scaler: f32,
) -> Vec<[f32; 3]> {
    assert_eq!(data.len(), width * height, "Mosaikgröße passt nicht");
    let mut out = vec![[0f32; 3]; width * height];
    if width < 2 * RCD_BORDER || height < 2 * RCD_BORDER {
        rcd_ppg_border(&mut out, data, width, height, filters, RCD_BORDER);
        return out;
    }
    rcd_ppg_border(&mut out, data, width, height, filters, RCD_MARGIN);

    let scaler = scaler.max(1.0);
    let revscaler = 1.0 / scaler;

    let num_vertical =
        1 + ((height as isize - 2 * RCD_BORDER as isize - 1) / RCD_TILEVALID as isize) as usize;
    let num_horizontal =
        1 + ((width as isize - 2 * RCD_BORDER as isize - 1) / RCD_TILEVALID as isize) as usize;

    // Die gültigen Zeilen der Kachelzeilen liegen lückenlos hintereinander:
    // Ausgabe in Bänder teilen, je Kachelzeile eines
    let mut bands: Vec<(usize, usize, &mut [[f32; 3]])> = Vec::with_capacity(num_vertical);
    let mut rest: &mut [[f32; 3]] = &mut out[RCD_MARGIN * width..(height - RCD_MARGIN) * width];
    let mut row0 = RCD_MARGIN;
    for tv in 0..num_vertical {
        let row_start = tv * RCD_TILEVALID;
        let row_end = (row_start + TS).min(height);
        let last = row_end
            - if tv == num_vertical - 1 {
                RCD_MARGIN
            } else {
                RCD_BORDER
            };
        let (head, tail) = rest.split_at_mut((last - row0) * width);
        bands.push((tv, row0, head));
        rest = tail;
        row0 = last;
    }

    bands
        .into_par_iter()
        .for_each_init(Scratch::new, |s, (tv, band_row0, band)| {
            for th in 0..num_horizontal {
                process_tile(
                    s,
                    data,
                    width,
                    height,
                    filters,
                    scaler,
                    revscaler,
                    (tv, num_vertical),
                    (th, num_horizontal),
                    band,
                    band_row0,
                );
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RGGB: [[usize; 2]; 2] = [[0, 1], [1, 2]];

    #[test]
    fn filters_roundtrip() {
        for p in [
            [[0, 1], [1, 2]],
            [[2, 1], [1, 0]],
            [[1, 0], [2, 1]],
            [[1, 2], [0, 1]],
        ] {
            let f = filters_from_pattern(p);
            for row in 0..32 {
                for col in 0..32 {
                    assert_eq!(fc(row, col, f), p[row & 1][col & 1]);
                }
            }
        }
    }

    #[test]
    fn flat_gray() {
        let (w, h) = (203, 197);
        let data = vec![0.5f32; w * h];
        let out = rcd_demosaic(&data, w, h, filters_from_pattern(RGGB));
        for (i, p) in out.iter().enumerate() {
            for c in 0..3 {
                assert!(
                    (p[c] - 0.5).abs() < 1e-4,
                    "Pixel ({}, {}) Kanal {c}: {}",
                    i % w,
                    i / w,
                    p[c]
                );
            }
        }
    }

    fn truth(x: usize, y: usize) -> [f32; 3] {
        let (xf, yf) = (x as f32, y as f32);
        [
            0.2 + 0.0015 * xf + 0.0005 * yf,
            0.3 + 0.0004 * xf + 0.0012 * yf,
            0.7 - 0.0008 * xf + 0.0006 * yf,
        ]
    }

    fn mosaic_of(
        w: usize,
        h: usize,
        pattern: [[usize; 2]; 2],
        f: impl Fn(usize, usize) -> [f32; 3],
    ) -> Vec<f32> {
        let mut m = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                m[y * w + x] = f(x, y)[pattern[y & 1][x & 1]];
            }
        }
        m
    }

    #[test]
    fn smooth_gradient() {
        let (w, h) = (300, 260);
        let m = mosaic_of(w, h, RGGB, truth);
        let out = rcd_demosaic(&m, w, h, filters_from_pattern(RGGB));
        for y in 8..h - 8 {
            for x in 8..w - 8 {
                let t = truth(x, y);
                for c in 0..3 {
                    let e = (out[y * w + x][c] - t[c]).abs() / t[c];
                    assert!(
                        e < 0.01,
                        "({x},{y}) Kanal {c}: {} statt {}",
                        out[y * w + x][c],
                        t[c]
                    );
                }
            }
        }
    }

    /// Pseudozufälliger Wert 0..1 aus Gitterkoordinaten
    fn hash(x: i32, y: i32, k: i32) -> f32 {
        let mut n = (x.wrapping_mul(374761393)
            ^ y.wrapping_mul(668265263)
            ^ k.wrapping_mul(1274126177)) as u32;
        n = (n ^ (n >> 13)).wrapping_mul(1274126177);
        ((n ^ (n >> 16)) & 0xffff) as f32 / 65535.0
    }

    /// Weiche Wertestruktur (bilinear interpoliertes Gitterrauschen)
    fn noise(x: f32, y: f32, scale: f32, k: i32) -> f32 {
        let (gx, gy) = (x / scale, y / scale);
        let (x0, y0) = (gx.floor(), gy.floor());
        let (fx, fy) = (gx - x0, gy - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let a = hash(x0, y0, k) * (1.0 - fx) + hash(x0 + 1, y0, k) * fx;
        let b = hash(x0, y0 + 1, k) * (1.0 - fx) + hash(x0 + 1, y0 + 1, k) * fx;
        a * (1.0 - fy) + b * fy
    }

    fn natural(x: usize, y: usize) -> [f32; 3] {
        let (xf, yf) = (x as f32, y as f32);
        let base = 0.1 + 0.6 * noise(xf, yf, 40.0, 1) + 0.2 * noise(xf, yf, 9.0, 2);
        [
            (base * (0.8 + 0.4 * noise(xf, yf, 55.0, 3))).clamp(0.0, 1.0),
            (base * (0.8 + 0.4 * noise(xf, yf, 47.0, 4))).clamp(0.0, 1.0),
            (base * (0.8 + 0.4 * noise(xf, yf, 61.0, 5))).clamp(0.0, 1.0),
        ]
    }

    #[test]
    fn small_images_do_not_hang() {
        for (w, h) in [(4, 4), (10, 7), (17, 40), (40, 17), (18, 18), (19, 25)] {
            let data = vec![0.3f32; w * h];
            let out = rcd_demosaic(&data, w, h, filters_from_pattern(RGGB));
            assert_eq!(out.len(), w * h);
        }
    }

    #[test]
    #[ignore]
    fn timing_33mp() {
        let (w, h) = (7000, 4700);
        let m = mosaic_of(w, h, RGGB, natural);
        let f = filters_from_pattern(RGGB);
        let t = std::time::Instant::now();
        let out = rcd_demosaic(&m, w, h, f);
        println!("33 MP RCD: {:?} ({})", t.elapsed(), out.len());
    }
}
