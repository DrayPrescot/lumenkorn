//! Eingangsschärfung für RAWs („Capture Sharpening“).
//!
//! Optischer Tiefpass, Objektiv und Demosaicing machen jedes RAW etwas weich;
//! Lightroom und andere Entwickler schärfen RAWs deshalb standardmäßig nach. Hier
//! geschieht das einmal in Sensorauflösung direkt nach dem Demosaicing – vor
//! jeder Verkleinerung, damit Vorschau, Zoom und Export gleich aussehen.
//!
//! Geschärft wird nur die Helligkeit in wahrnehmungsnaher Kodierung (Farben
//! bleiben unberührt), mit kleinem Radius, sanfter Schwelle gegen Rauschen und
//! Begrenzung auf die Werte der Nachbarn (keine hellen oder dunklen Säume).

use crate::image_f32::ImageF32;
use rayon::prelude::*;

/// Gauß-Radius (σ in Pixeln): typische Unschärfe von Tiefpassfilter + Demosaicing
const SIGMA: f32 = 0.75;
/// Stärke der Anhebung
const AMOUNT: f32 = 2.0;
/// Mindestschwelle (Kodierungseinheiten 0..1); darüber hinaus richtet sie sich
/// nach dem gemessenen Rauschen, damit hohe ISO-Werte nicht körniger werden
const MIN_THRESHOLD: f32 = 0.002;
/// Schwelle in Vielfachen der geschätzten Rauschstärke
const NOISE_FACTOR: f32 = 2.5;
/// Erlaubtes Überschwingen über das lokale Minimum/Maximum hinaus
const OVERSHOOT: f32 = 0.025;

/// Wahrnehmungsnahe Kodierung der Helligkeit (Gamma 1/2,2, linear nahe 0)
#[inline]
fn encode(y: f32) -> f32 {
    if y <= 0.0 { 0.0 } else { y.powf(1.0 / 2.2) }
}
#[inline]
fn decode(l: f32) -> f32 {
    if l <= 0.0 { 0.0 } else { l.powf(2.2) }
}

#[inline]
fn luma(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

/// Schärft ein lineares RGB-Bild in Sensorauflösung.
pub fn apply(img: &mut ImageF32) {
    apply_with(img, AMOUNT, OVERSHOOT);
}

/// Wie [`apply`] mit eigener Stärke und Überschwing-Reserve (Vergleiche).
pub fn apply_with(img: &mut ImageF32, amount: f32, overshoot: f32) {
    let (w, h) = (img.width, img.height);
    if w < 8 || h < 8 {
        return;
    }
    let l: Vec<f32> = img.data.par_iter().map(|p| encode(luma(*p))).collect();

    // Separabler Gauß (Radius 2)
    let k = {
        let k: Vec<f32> = (-2..=2)
            .map(|i| (-(i * i) as f32 / (2.0 * SIGMA * SIGMA)).exp())
            .collect();
        let s: f32 = k.iter().sum();
        k.into_iter().map(|v| v / s).collect::<Vec<_>>()
    };
    let clampi = |v: isize, n: usize| v.clamp(0, n as isize - 1) as usize;
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src = &l[y * w..(y + 1) * w];
        for (x, o) in row.iter_mut().enumerate() {
            *o = (0..5)
                .map(|j| k[j] * src[clampi(x as isize + j as isize - 2, w)])
                .sum();
        }
    });
    let mut blur = vec![0f32; w * h];
    blur.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            *o = (0..5)
                .map(|j| k[j] * tmp[clampi(y as isize + j as isize - 2, h) * w + x])
                .sum();
        }
    });
    drop(tmp);

    // Rauschstärke der Detailschicht: robuste Streuung (MAD) über eine Stichprobe
    let threshold = {
        let mut s: Vec<f32> = (0..w * h)
            .step_by(7)
            .map(|i| (l[i] - blur[i]).abs())
            .collect();
        let mid = s.len() / 2;
        let mad = *s.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1;
        (NOISE_FACTOR * 1.4826 * mad).max(MIN_THRESHOLD)
    };

    img.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let d = l[i] - blur[i];
            // Sanfte Schwelle: kleine Abweichungen (Rauschen) kaum, große voll
            let a = d.abs();
            let gain = if a <= threshold {
                (a / threshold).powi(2)
            } else {
                1.0
            };
            let mut v = l[i] + amount * gain * d;
            // Keine Säume: auf die 3×3-Nachbarschaft begrenzen
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for dy in -1..=1isize {
                let yy = clampi(y as isize + dy, h);
                for dx in -1..=1isize {
                    let n = l[yy * w + clampi(x as isize + dx, w)];
                    lo = lo.min(n);
                    hi = hi.max(n);
                }
            }
            v = v.clamp(lo - overshoot, hi + overshoot).max(0.0);
            let y0 = luma(*px);
            if y0 > 1e-6 {
                let s = decode(v) / y0;
                *px = px.map(|c| c * s);
            } else {
                let add = decode(v);
                *px = px.map(|c| c + add);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: usize, h: usize, f: impl Fn(usize, usize) -> [f32; 3] + Sync) -> ImageF32 {
        ImageF32::from_fn(w, h, f)
    }

    #[test]
    fn flat_areas_and_colors_stay_unchanged() {
        let mut a = img(32, 32, |_, _| [0.4, 0.2, 0.1]);
        apply(&mut a);
        for p in &a.data {
            for (c, v) in p.iter().zip([0.4, 0.2, 0.1]) {
                assert!((c - v).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn edges_get_steeper_without_halos() {
        // Weiche Kante (wie nach Tiefpass + Demosaicing)
        let soft = |x: usize| {
            let t = ((x as f32 - 16.0) / 1.5).tanh() * 0.5 + 0.5;
            0.05 + 0.4 * t
        };
        let mut a = img(32, 8, |x, _| [soft(x); 3]);
        let before: Vec<f32> = (0..32).map(|x| a.get(x, 4)[1]).collect();
        apply(&mut a);
        let after: Vec<f32> = (0..32).map(|x| a.get(x, 4)[1]).collect();
        let slope = |v: &[f32]| v[17] - v[15];
        assert!(
            slope(&after) > slope(&before) * 1.15,
            "{before:?} → {after:?}"
        );
        // Kein Überschwingen über die Plateaus hinaus (bis auf die kleine Reserve)
        let (lo, hi) = (before[0], before[31]);
        let enc_tol = |v: f32| decode(encode(v) + OVERSHOOT);
        assert!(
            after
                .iter()
                .all(|v| *v <= enc_tol(hi) && *v >= decode(encode(lo) - OVERSHOOT))
        );
    }

    #[test]
    fn faint_noise_is_left_alone() {
        // Rauschen weit unter der Schwelle
        let mut a = img(32, 32, |x, y| {
            let n = (((x * 7 + y * 13) % 5) as f32 - 2.0) * 0.0004;
            [0.18 + n; 3]
        });
        let before = a.data.clone();
        apply(&mut a);
        let max = a
            .data
            .iter()
            .zip(&before)
            .map(|(p, q)| (p[1] - q[1]).abs())
            .fold(0f32, f32::max);
        assert!(max < 0.0002, "{max}");
    }
}
