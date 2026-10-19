//! Gradationskurve (wie darktables „RGB-Kurve“, Lightrooms Punktkurve).
//!
//! Stützpunkte werden mit einem monotonen kubischen Spline verbunden
//! (Fritsch–Carlson, wie darktables „monoton“): Die Kurve schwingt zwischen
//! den Punkten nicht über, steigt also nie, wo die Punkte fallen, und
//! umgekehrt. Vor dem ersten und nach dem letzten Punkt bleibt sie waagerecht.
//!
//! CPU und GPU werten dieselbe Tabelle aus ([`CURVE_LUT`] Werte je Kurve,
//! dazwischen linear) – so stimmen beide Wege überein.

use crate::color::luma;
use crate::params::ToneCurve;

/// Stützwerte je Kurve in der Tabelle
pub const CURVE_LUT: usize = 1024;

/// Tabelle für [`curve_px`] und den GPU-Shader: RGB, Rot, Grün, Blau
/// hintereinander, je [`CURVE_LUT`] Werte.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CurveLut {
    pub data: Vec<f32>,
    /// RGB-Kurve: [`MODE_OFF`], [`MODE_EACH`] oder [`MODE_PRESERVE`]
    pub mode: u32,
}

/// RGB-Kurve leer: nur die Kanalkurven wirken
pub(crate) const MODE_OFF: u32 = 0;
/// RGB-Kurve auf jeden Kanal (wie Lightroom)
pub(crate) const MODE_EACH: u32 = 1;
/// RGB-Kurve auf die Helligkeit, Farbton und Sättigung bleiben (wie darktable)
pub(crate) const MODE_PRESERVE: u32 = 2;

/// `None`, wenn die Kurven nichts ändern.
pub(crate) fn curve_setup(c: &ToneCurve) -> Option<CurveLut> {
    if c.is_neutral() {
        return None;
    }
    let mut data = Vec::with_capacity(4 * CURVE_LUT);
    for pts in [&c.rgb, &c.red, &c.green, &c.blue] {
        let f = Spline::new(pts);
        data.extend((0..CURVE_LUT).map(|i| f.eval(i as f32 / (CURVE_LUT - 1) as f32)));
    }
    let mode = if c.rgb.is_empty() {
        MODE_OFF
    } else if c.preserve_colors {
        MODE_PRESERVE
    } else {
        MODE_EACH
    };
    Some(CurveLut { data, mode })
}

/// Wert der Kurve `k` (0 = RGB, 1–3 = Rot, Grün, Blau) an der Stelle `v`.
#[inline]
fn lookup(lut: &[f32], k: usize, v: f32) -> f32 {
    let t = v.clamp(0.0, 1.0) * (CURVE_LUT - 1) as f32;
    let i = (t as usize).min(CURVE_LUT - 2);
    let f = t - i as f32;
    let a = lut[k * CURVE_LUT + i];
    let b = lut[k * CURVE_LUT + i + 1];
    a + (b - a) * f
}

/// Gradationskurve für ein Pixel (sRGB-kodiert, 0…1).
#[inline]
pub(crate) fn curve_px(c: [f32; 3], l: &CurveLut) -> [f32; 3] {
    let c = match l.mode {
        MODE_PRESERVE => {
            let y = luma(c);
            crate::pipeline::apply_luma(c, y, lookup(&l.data, 0, y))
        }
        MODE_EACH => c.map(|v| lookup(&l.data, 0, v)),
        _ => c,
    };
    [
        lookup(&l.data, 1, c[0]),
        lookup(&l.data, 2, c[1]),
        lookup(&l.data, 3, c[2]),
    ]
}

/// Monotoner kubischer Hermite-Spline durch die Stützpunkte (nach x sortiert,
/// siehe `ToneCurve::sanitized`). Ohne Punkte: die Gerade y = x.
struct Spline {
    x: Vec<f32>,
    y: Vec<f32>,
    /// Steigungen in den Punkten
    m: Vec<f32>,
}

impl Spline {
    fn new(points: &[[f32; 2]]) -> Self {
        if points.len() < 2 {
            return Spline {
                x: vec![0.0, 1.0],
                y: vec![0.0, 1.0],
                m: vec![1.0, 1.0],
            };
        }
        let x: Vec<f32> = points.iter().map(|p| p[0]).collect();
        let y: Vec<f32> = points.iter().map(|p| p[1]).collect();
        let n = x.len();
        // Sekanten der Abschnitte
        let d: Vec<f32> = (0..n - 1)
            .map(|k| (y[k + 1] - y[k]) / (x[k + 1] - x[k]))
            .collect();
        let mut m = vec![0f32; n];
        m[0] = d[0];
        m[n - 1] = d[n - 2];
        for k in 1..n - 1 {
            m[k] = if d[k - 1] * d[k] <= 0.0 {
                0.0
            } else {
                (d[k - 1] + d[k]) / 2.0
            };
        }
        // Fritsch–Carlson: Steigungen begrenzen, damit jeder Abschnitt monoton bleibt
        for k in 0..n - 1 {
            if d[k] == 0.0 {
                m[k] = 0.0;
                m[k + 1] = 0.0;
                continue;
            }
            let a = m[k] / d[k];
            let b = m[k + 1] / d[k];
            let s = a * a + b * b;
            if s > 9.0 {
                let t = 3.0 / s.sqrt();
                m[k] = t * a * d[k];
                m[k + 1] = t * b * d[k];
            }
        }
        Spline { x, y, m }
    }

    fn eval(&self, v: f32) -> f32 {
        let n = self.x.len();
        if v <= self.x[0] {
            return self.y[0];
        }
        if v >= self.x[n - 1] {
            return self.y[n - 1];
        }
        let k = self.x.partition_point(|&x| x <= v).clamp(1, n - 1) - 1;
        let h = self.x[k + 1] - self.x[k];
        let t = (v - self.x[k]) / h;
        let (t2, t3) = (t * t, t * t * t);
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        (h00 * self.y[k] + h10 * h * self.m[k] + h01 * self.y[k + 1] + h11 * h * self.m[k + 1])
            .clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(rgb: &[[f32; 2]]) -> ToneCurve {
        ToneCurve {
            rgb: rgb.to_vec(),
            ..ToneCurve::default()
        }
    }

    #[test]
    fn spline_passes_points_and_stays_monotone() {
        let pts = [[0.0, 0.0], [0.25, 0.15], [0.75, 0.85], [1.0, 1.0]];
        let s = Spline::new(&pts);
        for p in pts {
            assert!((s.eval(p[0]) - p[1]).abs() < 1e-6, "{p:?}");
        }
        let mut last = -1.0;
        for i in 0..=1000 {
            let v = s.eval(i as f32 / 1000.0);
            assert!(v >= last - 1e-6, "fällt bei {i}");
            last = v;
        }
        // Ohne Punkte: Identität
        let id = Spline::new(&[]);
        assert!((id.eval(0.37) - 0.37).abs() < 1e-6);
    }

    #[test]
    fn steep_points_do_not_overshoot() {
        // Treppe: dazwischen waagerecht, kein Überschwingen über 0,5 / unter 0
        let s = Spline::new(&[[0.0, 0.0], [0.4, 0.0], [0.6, 0.5], [1.0, 0.5]]);
        for i in 0..=100 {
            let v = s.eval(i as f32 / 100.0);
            assert!((0.0..=0.5 + 1e-6).contains(&v), "{i}: {v}");
        }
        assert_eq!(s.eval(0.2), 0.0);
        assert_eq!(s.eval(0.8), 0.5);
    }

    #[test]
    fn neutral_curve_has_no_setup_and_lut_is_exact_enough() {
        assert!(curve_setup(&ToneCurve::default()).is_none());
        // Aufhellen: Mitte von 0,5 auf 0,6
        let l = curve_setup(&curve(&[[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]])).unwrap();
        let s = Spline::new(&[[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]]);
        for i in 0..=997 {
            let v = i as f32 / 997.0;
            assert!((lookup(&l.data, 0, v) - s.eval(v)).abs() < 2e-4, "{v}");
        }
        // Kanalkurven unverändert
        assert!((lookup(&l.data, 1, 0.3) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn preserve_colors_keeps_ratios_per_channel_shifts_them() {
        let c = curve(&[[0.0, 0.0], [0.5, 0.65], [1.0, 1.0]]);
        let px = [0.6, 0.4, 0.2];
        let keep = curve_px(px, &curve_setup(&c).unwrap());
        // Gleiches Verhältnis der Kanäle, heller
        assert!((keep[0] / keep[2] - 3.0).abs() < 1e-3, "{keep:?}");
        assert!(luma(keep) > luma(px));
        let each = curve_px(
            px,
            &curve_setup(&ToneCurve {
                preserve_colors: false,
                ..c
            })
            .unwrap(),
        );
        assert!((each[0] / each[2] - 3.0).abs() > 0.1, "{each:?}");
        // Rotkurve allein
        let red = ToneCurve {
            red: vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]],
            ..ToneCurve::default()
        };
        let o = curve_px([0.5, 0.5, 0.5], &curve_setup(&red).unwrap());
        assert!(
            o[0] > 0.65 && (o[1] - 0.5).abs() < 1e-4 && (o[2] - 0.5).abs() < 1e-4,
            "{o:?}"
        );
    }
}
