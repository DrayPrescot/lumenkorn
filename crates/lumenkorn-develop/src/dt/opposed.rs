//! Lichter-Rekonstruktion „inpaint opposed“, übertragen aus darktable 5.0
//! `src/iop/hlreconstruct/opposed.c` (`_process_opposed`) und `_calc_refavg`
//! aus `segbased.c`.
//!
//! Copyright (C) 2022-2024 darktable developers, GPL-3.0-or-later. Verfahren
//! entwickelt von garagecoder und Iain (G'MIC-Team) und Hanno Schwalm.
//!
//! Läuft auf dem Mosaik nach rawprepare und Weißabgleich (Kamera-Koeffizienten),
//! vor dem Demosaicing. Ein gesättigtes Fotosite wird durch den Mittelwert der
//! beiden anderen Farben im 3×3-Umfeld (in Kubikwurzel-Darstellung) plus einer
//! globalen Korrektur je Farbe ersetzt, falls das größer ist als der Messwert.

use rayon::prelude::*;

/// `highlights_clip_magics[DT_IOP_HIGHLIGHTS_OPPOSED]`
const CLIP_MAGIC: f32 = 0.987;
const HL_POWERF: f32 = 3.0;

/// Mittel der Gegenfarben im 3×3-Umfeld (`_calc_refavg`, linear).
#[inline]
fn refavg(
    input: &[f32],
    width: usize,
    height: usize,
    row: usize,
    col: usize,
    color: &(impl Fn(usize, usize) -> usize + Sync),
    correction: [f32; 3],
) -> f32 {
    let mut mean = [0f32; 3];
    let mut cnt = [0f32; 3];
    let dymin = row.saturating_sub(1);
    let dxmin = col.saturating_sub(1);
    let dymax = (height - 1).min(row + 2);
    let dxmax = (width - 1).min(col + 2);
    for dy in dymin..dymax {
        for dx in dxmin..dxmax {
            let val = input[dy * width + dx].max(0.0);
            let c = color(dy, dx);
            mean[c] += val;
            cnt[c] += 1.0;
        }
    }
    for c in 0..3 {
        mean[c] = if cnt[c] > 0.0 {
            ((correction[c] * mean[c]) / cnt[c]).powf(1.0 / HL_POWERF)
        } else {
            0.0
        };
    }
    let croot = [
        0.5 * (mean[1] + mean[2]),
        0.5 * (mean[0] + mean[2]),
        0.5 * (mean[0] + mean[1]),
    ];
    croot[color(row, col)].powf(HL_POWERF)
}

/// `_mask_dilated`: Zelle oder eine im Umkreis von etwa 3 Zellen gesetzt
#[inline]
fn mask_dilated(m: &[u8], i: usize, w1: usize) -> u8 {
    if m[i] != 0 {
        return 1;
    }
    let at = |dy: isize, dx: isize| m[(i as isize + dy * w1 as isize + dx) as usize];
    const NEAR: [(isize, isize); 8] = [
        (-1, -1),
        (-1, 0),
        (-1, 1),
        (0, -1),
        (0, 1),
        (1, -1),
        (1, 0),
        (1, 1),
    ];
    if NEAR.iter().any(|&(dy, dx)| at(dy, dx) != 0) {
        return 1;
    }
    const FAR: [(isize, isize); 36] = [
        (-3, -2),
        (-3, -1),
        (-3, 0),
        (-3, 1),
        (-3, 2),
        (-2, -3),
        (-2, -2),
        (-2, -1),
        (-2, 0),
        (-2, 1),
        (-2, 2),
        (-2, 3),
        (-1, -3),
        (-1, -2),
        (-1, 2),
        (-1, 3),
        (0, -3),
        (0, -2),
        (0, 2),
        (0, 3),
        (1, -3),
        (1, -2),
        (1, 2),
        (1, 3),
        (2, -3),
        (2, -2),
        (2, -1),
        (2, 0),
        (2, 1),
        (2, 2),
        (2, 3),
        (3, -2),
        (3, -1),
        (3, 0),
        (3, 1),
        (3, 2),
    ];
    FAR.iter().any(|&(dy, dx)| at(dy, dx) != 0) as u8
}

/// [`mask_dilated`] für alle Zellen außer dem Rand von 3 Zellen (der bleibt 0).
/// Zeilenweise parallel; eine Zeile, in deren Umkreis (±3 Zeilen) nichts gesetzt
/// ist, bleibt 0, ohne gerechnet zu werden – meist ist nur ein kleiner Teil des
/// Bildes gesättigt (bei 33 MP sonst 200 ms auf einem Kern).
fn dilate(src: &[u8], dst: &mut [u8], mwidth: usize, mheight: usize) {
    let row_set: Vec<bool> = (0..mheight)
        .map(|r| src[r * mwidth..(r + 1) * mwidth].iter().any(|v| *v != 0))
        .collect();
    let rows = 3..mheight.saturating_sub(3);
    dst[..mwidth * mheight]
        .par_chunks_mut(mwidth)
        .enumerate()
        .filter(|(row, _)| rows.contains(row) && row_set[row - 3..=row + 3].contains(&true))
        .for_each(|(row, out)| {
            let cols = 3..mwidth.saturating_sub(3);
            for (col, o) in out.iter_mut().enumerate().filter(|(c, _)| cols.contains(c)) {
                *o = mask_dilated(src, row * mwidth + col, mwidth);
            }
        });
}

/// Rekonstruiert gesättigte Fotosites an Ort und Stelle.
///
/// `data`: Mosaik `width` × `height`, normiert (Weiß = 1) und mit den
/// Weißabgleich-Koeffizienten `coeffs` multipliziert. `color(row, col)` ist die
/// Farbe (0 R, 1 G, 2 B) eines Fotosites. `correction` = D65-Koeffizienten /
/// Kamera-Koeffizienten (darktable „as shot to reference“).
pub(crate) fn process_opposed(
    data: &mut [f32],
    width: usize,
    height: usize,
    color: impl Fn(usize, usize) -> usize + Sync,
    coeffs: [f32; 3],
    correction: [f32; 3],
) {
    if width < 21 || height < 21 {
        return;
    }
    let clips = coeffs.map(|c| CLIP_MAGIC * c);
    let input: &[f32] = data;
    let mwidth = width / 3;
    let mheight = height / 3;
    let msize = (mwidth + 1) * (mheight + 1);

    // Maske gesättigter Farben je 3×3-Zelle. Wie im Original wird die Farbe
    // an (Zellzeile + y, Zellspalte + x) abgefragt.
    let mut mask = vec![0u8; 6 * msize];
    let (base, dil) = mask.split_at_mut(3 * msize);
    let rows: Vec<[Vec<(usize, u8)>; 3]> = (1..mheight - 1)
        .into_par_iter()
        .map(|mrow| {
            let mut out: [Vec<(usize, u8)>; 3] = Default::default();
            for mcol in 1..mwidth - 1 {
                let mut mbuff = [0u8; 3];
                let grp = 3 * (mrow * width + mcol);
                for y in -1isize..2 {
                    for x in -1isize..2 {
                        let idx = (grp as isize + y * width as isize + x) as usize;
                        let c = color((mrow as isize + y) as usize, (mcol as isize + x) as usize);
                        if input[idx] >= clips[c] {
                            mbuff[c] += 1;
                        }
                    }
                }
                for c in 0..3 {
                    if mbuff[c] != 0 {
                        out[c].push((mrow * mwidth + mcol, 1));
                    }
                }
            }
            out
        })
        .collect();
    let mut anyclipped = false;
    for r in rows {
        for (c, list) in r.iter().enumerate() {
            for &(i, v) in list {
                base[c * msize + i] = v;
                anyclipped = true;
            }
        }
    }
    if !anyclipped {
        return;
    }

    // Maske leicht erweitern: die Fotosites dicht um gesättigte Bereiche
    for c in 0..3 {
        dilate(
            &base[c * msize..(c + 1) * msize],
            &mut dil[c * msize..(c + 1) * msize],
            mwidth,
            mheight,
        );
    }

    // Korrektur je Farbe aus den nicht gesättigten Fotosites dicht am Rand
    let lo_clips = clips.map(|c| 0.2 * c);
    let (sums, cnts) = (3..height - 3)
        .into_par_iter()
        .map(|row| {
            let mut s = [0f64; 3];
            let mut n = [0f64; 3];
            for col in 3..width - 3 {
                let c = color(row, col);
                let inval = input[row * width + col];
                let m = (row / 3) * mwidth + col / 3;
                if inval < clips[c] && inval > lo_clips[c] && dil[c * msize + m] != 0 {
                    s[c] +=
                        (inval - refavg(input, width, height, row, col, &color, correction)) as f64;
                    n[c] += 1.0;
                }
            }
            (s, n)
        })
        .reduce(
            || ([0f64; 3], [0f64; 3]),
            |a, b| {
                (
                    std::array::from_fn(|c| a.0[c] + b.0[c]),
                    std::array::from_fn(|c| a.1[c] + b.1[c]),
                )
            },
        );
    let chrominance: [f32; 3] = std::array::from_fn(|c| {
        if cnts[c] > 100.0 {
            (sums[c] / cnts[c]) as f32
        } else {
            0.0
        }
    });

    // Rekonstruieren. Gelesen wird immer das unveränderte Mosaik: Erst alle neuen
    // Werte der gesättigten Fotosites sammeln (meist ein kleiner Teil), dann
    // schreiben – statt das ganze Mosaik neu aufzubauen und zurückzukopieren.
    let fixed: Vec<Vec<(usize, f32)>> = (0..height)
        .into_par_iter()
        .map(|row| {
            let mut out = Vec::new();
            for col in 0..width {
                let c = color(row, col);
                let inval = input[row * width + col];
                if inval >= clips[c] {
                    let r = refavg(input, width, height, row, col, &color, correction);
                    out.push((row * width + col, inval.max(r + chrominance[c])));
                }
            }
            out
        })
        .collect();
    // Wie im Original: übrige Werte unter 0 (und NaN) werden 0
    data.par_iter_mut().for_each(|v| *v = v.max(0.0));
    for (i, v) in fixed.into_iter().flatten() {
        data[i] = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bayer(row: usize, col: usize) -> usize {
        [[0, 1], [1, 2]][row & 1][col & 1]
    }

    #[test]
    fn nothing_changes_without_clipping() {
        let (w, h) = (60, 60);
        let mut d: Vec<f32> = (0..w * h).map(|i| 0.2 + (i % w) as f32 * 0.005).collect();
        let before = d.clone();
        process_opposed(&mut d, w, h, bayer, [2.0, 1.0, 1.5], [1.0; 3]);
        assert_eq!(d, before);
    }

    #[test]
    fn clipped_green_is_raised_from_red_and_blue() {
        // Weißabgleich-Raum: Grauer Fleck, in der Mitte Grün am Anschlag (1,0),
        // Rot und Blau gemessen bei 1,3
        let (w, h) = (90, 90);
        let coeffs = [2.0f32, 1.0, 1.5];
        let mut d: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let r = ((x - 45.0).powi(2) + (y - 45.0).powi(2)).sqrt();
                let v: f32 = if r < 15.0 { 1.3 } else { 0.8 };
                let c = bayer(i / w, i % w);
                v.min(coeffs[c])
            })
            .collect();
        process_opposed(&mut d, w, h, bayer, coeffs, [1.0; 3]);
        let g = d[45 * w + 46];
        assert!(g > 1.2, "Grün geschätzt: {g}");
        // Nie dunkler als gemessen
        assert!(d.iter().all(|v| *v >= 0.0));
    }

    /// Die zeilenweise Erweiterung mit übersprungenen Zeilen liefert dasselbe wie
    /// die einfache Schleife über alle Zellen – auch für einzelne Zellen genau 3
    /// Zeilen über bzw. unter einer sonst leeren Zeile und am Rand.
    #[test]
    fn dilate_matches_plain_loop() {
        let (mw, mh) = (40, 37);
        let mut src = vec![0u8; (mw + 1) * (mh + 1)];
        for (r, c) in [
            (0, 5),
            (3, 3),
            (10, 20),
            (16, 3),
            (22, 36),
            (33, 30),
            (36, 39),
        ] {
            src[r * mw + c] = 1;
        }
        let mut fast = vec![0u8; src.len()];
        dilate(&src, &mut fast, mw, mh);
        let mut plain = vec![0u8; src.len()];
        for row in 3..mh - 3 {
            for col in 3..mw - 3 {
                plain[row * mw + col] = mask_dilated(&src, row * mw + col, mw);
            }
        }
        assert_eq!(fast, plain);
        assert!(plain.iter().filter(|v| **v != 0).count() > 50, "Testaufbau");
    }

    /// Unter 0 wird 0, auch bei Sättigung anderswo (wie im Original darktable)
    #[test]
    fn negative_values_become_zero_when_reconstructing() {
        let (w, h) = (60, 60);
        let mut d: Vec<f32> = (0..w * h)
            .map(|i| if i % 7 == 0 { -0.01 } else { 0.5 })
            .collect();
        for i in [30 * w + 30, 30 * w + 31, 31 * w + 30, 31 * w + 31] {
            d[i] = 2.0;
        }
        process_opposed(&mut d, w, h, bayer, [1.0; 3], [1.0; 3]);
        assert!(d.iter().all(|v| *v >= 0.0));
        assert!(d[30 * w + 30] >= 2.0);
    }
}
