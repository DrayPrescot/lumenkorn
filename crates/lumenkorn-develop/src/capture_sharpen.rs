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

use crate::color::luma;
use crate::image_f32::ImageF32;
use rayon::prelude::*;
use std::sync::OnceLock;

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
    static LUT: OnceLock<PowLut> = OnceLock::new();
    if y <= 0.0 {
        0.0
    } else {
        LUT.get_or_init(|| PowLut::new(1.0 / 2.2)).eval(y)
    }
}
#[inline]
fn decode(l: f32) -> f32 {
    static LUT: OnceLock<PowLut> = OnceLock::new();
    if l <= 0.0 {
        0.0
    } else {
        LUT.get_or_init(|| PowLut::new(2.2)).eval(l)
    }
}

/// `x^p` über Tabellen statt `powf`: Zwei `powf` je Pixel in Sensorauflösung
/// kosteten bei jedem RAW-Öffnen rund eine halbe Sekunde (24 MP).
///
/// Zerlegt x = m · 2^e (m in 1…2): x^p = m^p · 2^(e·p). Der zweite Faktor
/// kommt exakt aus einer Tabelle je Exponent, der erste linear interpoliert aus
/// einer Tabelle über die Mantisse. Relativer Fehler < 2e-6 (Test) – im
/// gesamten Wertebereich, also auch für RAW-Lichter weit über 1.
struct PowLut {
    p: f32,
    mant: Vec<f32>,
    exp: Vec<f32>,
}

/// Stützstellen über die Mantisse (Zweierpotenz: Index direkt aus den Bits)
const MANT_BITS: u32 = 10;

impl PowLut {
    fn new(p: f32) -> Self {
        let n = 1usize << MANT_BITS;
        let pd = p as f64;
        let mant = (0..=n)
            .map(|i| (1.0 + i as f64 / n as f64).powf(pd) as f32)
            .collect();
        let exp = (0..256)
            .map(|e| 2f64.powf((e as f64 - 127.0) * pd) as f32)
            .collect();
        Self { p, mant, exp }
    }

    #[inline]
    fn eval(&self, x: f32) -> f32 {
        let bits = x.to_bits();
        let e = (bits >> 23) & 0xff;
        // Subnormal, unendlich oder NaN: selten – exakt rechnen
        if e == 0 || e == 0xff {
            return x.powf(self.p);
        }
        let m = bits & 0x7f_ffff;
        let i = (m >> (23 - MANT_BITS)) as usize;
        let t = (m & ((1 << (23 - MANT_BITS)) - 1)) as f32 / (1 << (23 - MANT_BITS)) as f32;
        let (a, b) = (self.mant[i], self.mant[i + 1]);
        (a + (b - a) * t) * self.exp[e as usize]
    }
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
    // Ränder durch Wiederholen des Randpixels (wie früher die Index-Klemme),
    // innen ohne Klemme und Iterator – das ließ sich nicht vektorisieren und
    // kostete bei 24 MP über 300 ms. Gleiche Summationsreihenfolge, also
    // bitgleiches Ergebnis.
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each_init(
        || vec![0f32; w + 4],
        |pad, (y, row)| {
            let src = &l[y * w..(y + 1) * w];
            pad[2..w + 2].copy_from_slice(src);
            pad[..2].fill(src[0]);
            pad[w + 2..].fill(src[w - 1]);
            for (x, o) in row.iter_mut().enumerate() {
                let p = &pad[x..x + 5];
                *o = k[0] * p[0] + k[1] * p[1] + k[2] * p[2] + k[3] * p[3] + k[4] * p[4];
            }
        },
    );
    let mut blur = vec![0f32; w * h];
    blur.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let r = |j: isize| {
            let yy = (y as isize + j).clamp(0, h as isize - 1) as usize;
            &tmp[yy * w..(yy + 1) * w]
        };
        let (r0, r1, r2, r3, r4) = (r(-2), r(-1), r(0), r(1), r(2));
        for (x, o) in row.iter_mut().enumerate() {
            *o = k[0] * r0[x] + k[1] * r1[x] + k[2] * r2[x] + k[3] * r3[x] + k[4] * r4[x];
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
        let r = |j: isize| {
            let yy = (y as isize + j).clamp(0, h as isize - 1) as usize;
            &l[yy * w..(yy + 1) * w]
        };
        let rows = [r(-1), r(0), r(1)];
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
            let (xl, xr) = (x.saturating_sub(1), (x + 1).min(w - 1));
            for rr in rows {
                for n in [rr[xl], rr[x], rr[xr]] {
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

    /// Bisherige Fassung (Klemme je Zugriff) als Referenz: die schnelle muss
    /// bitgleich rechnen.
    fn apply_reference(img: &mut ImageF32, amount: f32, overshoot: f32) {
        let (w, h) = (img.width, img.height);
        let l: Vec<f32> = img.data.iter().map(|p| encode(luma(*p))).collect();
        let k = {
            let k: Vec<f32> = (-2..=2)
                .map(|i| (-(i * i) as f32 / (2.0 * SIGMA * SIGMA)).exp())
                .collect();
            let s: f32 = k.iter().sum();
            k.into_iter().map(|v| v / s).collect::<Vec<_>>()
        };
        let clampi = |v: isize, n: usize| v.clamp(0, n as isize - 1) as usize;
        let mut tmp = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                tmp[y * w + x] = (0..5)
                    .map(|j| k[j] * l[y * w + clampi(x as isize + j as isize - 2, w)])
                    .sum();
            }
        }
        let mut blur = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                blur[y * w + x] = (0..5)
                    .map(|j| k[j] * tmp[clampi(y as isize + j as isize - 2, h) * w + x])
                    .sum();
            }
        }
        let threshold = {
            let mut s: Vec<f32> = (0..w * h)
                .step_by(7)
                .map(|i| (l[i] - blur[i]).abs())
                .collect();
            let mid = s.len() / 2;
            let mad = *s.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1;
            (NOISE_FACTOR * 1.4826 * mad).max(MIN_THRESHOLD)
        };
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let d = l[i] - blur[i];
                let a = d.abs();
                let gain = if a <= threshold {
                    (a / threshold).powi(2)
                } else {
                    1.0
                };
                let mut v = l[i] + amount * gain * d;
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
                let px = &mut img.data[i];
                let y0 = luma(*px);
                if y0 > 1e-6 {
                    let s = decode(v) / y0;
                    *px = px.map(|c| c * s);
                } else {
                    let add = decode(v);
                    *px = px.map(|c| c + add);
                }
            }
        }
    }

    #[test]
    fn fast_blur_matches_reference_exactly() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = move || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 40) as f32 / (1u64 << 24) as f32
        };
        for (w, h) in [(8, 8), (37, 19), (64, 9)] {
            let data: Vec<[f32; 3]> = (0..w * h)
                .map(|i| {
                    // Kanten, Rauschen, Schwarz und Lichter über 1
                    let base = if i % w < w / 2 { 0.05 } else { 1.7 };
                    let z = if i % 11 == 0 { 0.0 } else { 1.0 };
                    [0, 1, 2].map(|_| (base * (0.8 + 0.4 * rnd())) * z)
                })
                .collect();
            let img = ImageF32 {
                width: w,
                height: h,
                data,
            };
            let mut a = img.clone();
            let mut b = img;
            apply_with(&mut a, AMOUNT, OVERSHOOT);
            apply_reference(&mut b, AMOUNT, OVERSHOOT);
            for (p, q) in a.data.iter().zip(&b.data) {
                assert_eq!(p.map(f32::to_bits), q.map(f32::to_bits), "{w}×{h}");
            }
        }
    }

    #[test]
    fn pow_lut_is_accurate() {
        let check = |f: fn(f32) -> f32, p: f64| {
            let mut worst = 0f64;
            // Logarithmisch von sehr dunkel bis weit über Weiß, dazu 0…1 dicht
            let xs = (0..200_000)
                .map(|i| 10f64.powf(-12.0 + i as f64 * 16.0 / 200_000.0) as f32)
                .chain((1..100_000).map(|i| i as f32 / 100_000.0));
            for x in xs {
                let want = (x as f64).powf(p);
                let got = f(x) as f64;
                worst = worst.max(((got - want) / want).abs());
            }
            worst
        };
        let (e, d) = (check(encode, 1.0 / 2.2), check(decode, 2.2));
        assert!(e < 2e-6 && d < 2e-6, "relativer Fehler: {e}, {d}");
        assert_eq!(encode(0.0), 0.0);
        assert_eq!(decode(-1.0), 0.0);
        assert!(decode(f32::MAX).is_infinite() && encode(f32::INFINITY).is_infinite());
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
