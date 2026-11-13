//! Weißabgleich in Kelvin. Das RAW ist bereits auf die Lichtfarbe der Aufnahme
//! abgeglichen (siehe `dt::raw`); eine andere Farbtemperatur bedeutet: Das
//! Licht war in Wahrheit so – ein Grau unter diesem Licht soll neutral werden.
//! Angepasst wird wie in der Farbwissenschaft üblich nach Bradford, und zwar als
//! volle 3×3-Matrix in linearem sRGB ([`kelvin_matrix`]): Faktoren je Kanal
//! träfen das Grau ebenso, verschöben kräftige Farben bei großen Korrekturen
//! aber anders als eine echte Anpassung. CPU und GPU rechnen mit derselben Matrix.

use crate::color::luma;

/// Bereich des Reglers. Darüber hinaus ändert sich die Lichtfarbe kaum noch.
pub const KELVIN_MIN: f32 = 2000.0;
pub const KELVIN_MAX: f32 = 25000.0;

const BRADFORD: [[f64; 3]; 3] = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];
const BRADFORD_INV: [[f64; 3]; 3] = [
    [0.9869929, -0.1470543, 0.1599627],
    [0.4323053, 0.5183603, 0.0492912],
    [-0.0085287, 0.0400428, 0.9684867],
];
/// XYZ (D65) → lineares sRGB (auch `ColorMatrix1` im DNG-Export)
pub(crate) const XYZ_TO_SRGB: [[f64; 3]; 3] = [
    [3.2404542, -1.5371385, -0.4985314],
    [-0.9692660, 1.8760108, 0.0415560],
    [0.0556434, -0.2040259, 1.0572252],
];
/// Weißpunkt D65 (XYZ, Y = 1)
const D65: [f64; 3] = [0.95047, 1.0, 1.08883];

fn apply(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

fn mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

/// Weißabgleich als Matrix für lineares sRGB (Zeilen): `out = M · in`
pub type WbMatrix = [[f32; 3]; 3];

/// Einheitsmatrix (kein Weißabgleich)
pub const WB_IDENTITY: WbMatrix = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// `m · c`
#[inline]
pub fn apply_wb(m: &WbMatrix, c: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * c[0] + m[0][1] * c[1] + m[0][2] * c[2],
        m[1][0] * c[0] + m[1][1] * c[1] + m[1][2] * c[2],
        m[2][0] * c[0] + m[2][1] * c[1] + m[2][2] * c[2],
    ]
}

/// Lineares sRGB → XYZ (D65), Umkehrung von [`XYZ_TO_SRGB`]
fn srgb_to_xyz() -> [[f64; 3]; 3] {
    let m = XYZ_TO_SRGB;
    let c =
        |r0: usize, r1: usize, c0: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let det = m[0][0] * c(1, 2, 1, 2) - m[0][1] * c(1, 2, 0, 2) + m[0][2] * c(1, 2, 0, 1);
    let adj = [
        [c(1, 2, 1, 2), -c(0, 2, 1, 2), c(0, 1, 1, 2)],
        [-c(1, 2, 0, 2), c(0, 2, 0, 2), -c(0, 1, 0, 2)],
        [c(1, 2, 0, 1), -c(0, 2, 0, 1), c(0, 1, 0, 1)],
    ];
    adj.map(|r| r.map(|v| v / det))
}

/// Weißabgleich als Matrix (lineares sRGB), wenn das Bild für `as_shot` Kelvin
/// abgeglichen ist, das Licht aber `kelvin` hatte: Anpassung nach Bradford von
/// der Farbe, in der ein Grau unter `kelvin` erscheint, nach D65. Graustufen
/// behalten ihre Helligkeit (normiert wie [`kelvin_multipliers`]); ein Grau
/// unter `kelvin` wird neutral, genau wie mit den Faktoren.
pub fn kelvin_matrix(as_shot: f32, kelvin: f32) -> WbMatrix {
    let lms0 = apply(&BRADFORD, xyz(planck_xy(as_shot)));
    let lms = apply(&BRADFORD, xyz(planck_xy(kelvin)));
    // Erscheinendes Weiß (lms · d65 / lms0) → D65: Faktor lms0 / lms je Zapfen
    let gain = [0, 1, 2].map(|k| lms0[k] / lms[k].max(1e-9));
    let diag = [
        [gain[0], 0.0, 0.0],
        [0.0, gain[1], 0.0],
        [0.0, 0.0, gain[2]],
    ];
    let m = mul(
        &mul(&mul(&XYZ_TO_SRGB, &BRADFORD_INV), &diag),
        &mul(&BRADFORD, &srgb_to_xyz()),
    );
    let m: WbMatrix = m.map(|r| r.map(|v| v as f32));
    let y = luma(apply_wb(&m, [1.0, 1.0, 1.0]));
    m.map(|r| r.map(|v| v / y))
}

/// Farbort des Planckschen Strahlers (Näherung nach Kim et al., 1667–25 000 K)
pub fn planck_xy(kelvin: f32) -> (f64, f64) {
    let t = (kelvin as f64).clamp(1667.0, 25000.0);
    let (t2, t3) = (t * t, t * t * t);
    let x = if t <= 4000.0 {
        -0.2661239e9 / t3 - 0.2343589e6 / t2 + 0.8776956e3 / t + 0.179910
    } else {
        -3.0258469e9 / t3 + 2.1070379e6 / t2 + 0.2226347e3 / t + 0.240390
    };
    let (x2, x3) = (x * x, x * x * x);
    let y = if t <= 2222.0 {
        -1.1063814 * x3 - 1.34811020 * x2 + 2.18555832 * x - 0.20219683
    } else if t <= 4000.0 {
        -0.9549476 * x3 - 1.37418593 * x2 + 2.09137015 * x - 0.16748867
    } else {
        3.0817580 * x3 - 5.87338670 * x2 + 3.75112997 * x - 0.37001483
    };
    (x, y)
}

/// Ähnlichste Farbtemperatur eines Farborts (McCamy), im Bereich des Reglers
pub fn xy_to_kelvin(x: f64, y: f64) -> Option<f32> {
    let n = (x - 0.3320) / (0.1858 - y);
    let t = 449.0 * n.powi(3) + 3525.0 * n.powi(2) + 6823.3 * n + 5520.33;
    t.is_finite()
        .then_some((t as f32).clamp(KELVIN_MIN, KELVIN_MAX))
}

fn xyz(xy: (f64, f64)) -> [f64; 3] {
    [xy.0 / xy.1, 1.0, (1.0 - xy.0 - xy.1) / xy.1]
}

/// Faktoren je Kanal (lineares sRGB, Helligkeit unverändert), wenn das Bild
/// für `as_shot` Kelvin abgeglichen ist, das Licht aber `kelvin` hatte.
pub fn kelvin_multipliers(as_shot: f32, kelvin: f32) -> [f32; 3] {
    let lms0 = apply(&BRADFORD, xyz(planck_xy(as_shot)));
    let lms_d65 = apply(&BRADFORD, D65);
    // So erscheint ein Grau unter `kelvin` im auf `as_shot` abgeglichenen Bild
    let lms = apply(&BRADFORD, xyz(planck_xy(kelvin)));
    let adapted = [0, 1, 2].map(|k| lms[k] * lms_d65[k] / lms0[k]);
    let rgb = apply(&XYZ_TO_SRGB, apply(&BRADFORD_INV, adapted));
    let m = rgb.map(|v| 1.0 / v.max(1e-3)).map(|v| v as f32);
    let y = luma(m);
    m.map(|v| v / y)
}

/// Kelvin, bei dem Rot zu Blau im Verhältnis `red_over_blue` verstärkt wird
/// (Umkehrung von [`kelvin_multipliers`] für den automatischen Weißabgleich
/// und ältere Bearbeitungen mit relativer Temperatur).
pub fn kelvin_for_ratio(as_shot: f32, red_over_blue: f32) -> f32 {
    let ratio = |k: f32| {
        let m = kelvin_multipliers(as_shot, k);
        m[0] / m[2]
    };
    // Höhere Temperatur → wärmer → mehr Rot; in Mired gleichmäßig suchen
    let (mut lo, mut hi) = (1e6 / KELVIN_MAX, 1e6 / KELVIN_MIN);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if ratio(1e6 / mid) > red_over_blue {
            // zu warm: niedrigere Temperatur = mehr Mired
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (1e6 / (0.5 * (lo + hi))).clamp(KELVIN_MIN, KELVIN_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_shot_is_neutral() {
        for k in [2500.0, 5500.0, 9000.0] {
            let m = kelvin_multipliers(k, k);
            for c in m {
                assert!((c - 1.0).abs() < 1e-4, "{k}: {m:?}");
            }
        }
    }

    #[test]
    fn lower_kelvin_cools_the_image() {
        // Aufgenommen bei 5500 K, das Licht war aber Glühlampe (3000 K)
        let m = kelvin_multipliers(5500.0, 3000.0);
        assert!(m[2] > m[0], "{m:?}");
        // Spürbar, aber nicht absurd (in sRGB stärker als in Kamerawerten)
        let r = m[2] / m[0];
        assert!((2.0..6.0).contains(&r), "Blau/Rot {r}");
        let warm = kelvin_multipliers(5500.0, 9000.0);
        assert!(warm[0] > warm[2]);
    }

    /// Die Matrix macht dasselbe Grau neutral wie die Faktoren, wirkt auf Farben
    /// aber als echte Anpassung (Nebendiagonale ≠ 0)
    #[test]
    fn kelvin_matrix_neutralizes_the_same_gray() {
        for (k0, k) in [(5500.0, 3000.0), (5500.0, 9000.0), (4200.0, 6500.0)] {
            let f = kelvin_multipliers(k0, k);
            let m = kelvin_matrix(k0, k);
            // Erscheinendes Grau: das, was die Faktoren neutral machen
            let gray = f.map(|v| 1.0 / v);
            let out = apply_wb(&m, gray);
            let spread = out.iter().copied().fold(f32::MIN, f32::max)
                / out.iter().copied().fold(f32::MAX, f32::min);
            assert!(spread < 1.002, "{k0}→{k}: {out:?}");
            assert!(
                m[0][1].abs() > 1e-3 || m[0][2].abs() > 1e-3,
                "keine reine Diagonale"
            );
        }
        let id = kelvin_matrix(5000.0, 5000.0);
        for (r, row) in id.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                let want = if r == c { 1.0 } else { 0.0 };
                assert!((v - want).abs() < 1e-4, "{id:?}");
            }
        }
    }

    #[test]
    fn kelvin_round_trips() {
        assert!(
            (xy_to_kelvin(planck_xy(4000.0).0, planck_xy(4000.0).1).unwrap() - 4000.0).abs() < 60.0
        );
        for k in [2800.0f32, 4500.0, 7500.0] {
            let m = kelvin_multipliers(5200.0, k);
            let back = kelvin_for_ratio(5200.0, m[0] / m[2]);
            assert!((back - k).abs() / k < 0.01, "{k} → {back}");
        }
    }
}
