//! Farbumrechnung wie darktable 5.0: Matrixprofile (lcms2 `cmsCreateRGBProfile`,
//! `common/colorspaces.c`), colorin mit Kameramatrix, color calibration mit
//! CAT16 (`iop/channelmixerrgb.c`, `common/chromatic_adaptation.h`,
//! `common/illuminants.h`) und die Umrechnung des Arbeitsfarbraums.
//!
//! Arbeitsfarbraum ist lineares Rec.2020, Verbindungsraum XYZ mit D50 (ICC).
//! Lumenkorn speichert das Ergebnis in linearem Rec.709/sRGB (D65-Weiß = 1,1,1,
//! ohne Beschneiden); sigmoid rechnet wieder in Rec.2020.

// Konstanten wörtlich aus darktable (auch wenn f32 nicht alle Stellen hält)
#![allow(clippy::excessive_precision)]

pub(crate) type M3 = [[f64; 3]; 3];
pub(crate) type M3f = [[f32; 3]; 3];

pub(crate) fn mul(a: &M3, b: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

pub(crate) fn mul_v(m: &M3, v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| (0..3).map(|k| m[i][k] * v[k]).sum())
}

pub(crate) fn inv(m: &M3) -> Option<M3> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    let d = 1.0 / det;
    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * d,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * d,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * d,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * d,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * d,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * d,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * d,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * d,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * d,
        ],
    ])
}

pub(crate) fn to_f32(m: &M3) -> M3f {
    m.map(|r| r.map(|v| v as f32))
}

#[inline]
pub(crate) fn apply(m: &M3f, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

// --- lcms2: Matrixprofile ------------------------------------------------------

/// lcms2 `cmsD50_XYZ`
const LCMS_D50: [f64; 3] = [0.9642, 1.0, 0.8249];
/// lcms2 `LamRiggs` (Bradford) für `_cmsAdaptationMatrix`
const LCMS_BRADFORD: M3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];
/// darktable `D65xyY` (colorspaces.h)
pub(crate) const D65_XY: (f64, f64) = (0.31271, 0.32902);
pub(crate) const REC2020_PRIMARIES: [(f64, f64); 3] =
    [(0.708, 0.292), (0.170, 0.797), (0.131, 0.046)];
pub(crate) const SRGB_PRIMARIES: [(f64, f64); 3] = [(0.64, 0.33), (0.30, 0.60), (0.15, 0.06)];

fn xy_to_xyz(x: f64, y: f64) -> [f64; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// lcms2 `_cmsAdaptationMatrix` mit Bradford
fn lcms_adaptation(from: [f64; 3], to: [f64; 3]) -> M3 {
    let s = mul_v(&LCMS_BRADFORD, from);
    let d = mul_v(&LCMS_BRADFORD, to);
    let cone: M3 = [
        [d[0] / s[0], 0.0, 0.0],
        [0.0, d[1] / s[1], 0.0],
        [0.0, 0.0, d[2] / s[2]],
    ];
    let b_inv = inv(&LCMS_BRADFORD).expect("Bradford invertierbar");
    mul(&b_inv, &mul(&cone, &LCMS_BRADFORD))
}

/// RGB → XYZ (D50) eines Matrixprofils wie lcms2 `cmsCreateRGBProfile`
/// (`_cmsBuildRGB2XYZtransferMatrix`, danach Bradford nach D50).
pub(crate) fn rgb_to_xyz_d50(white: (f64, f64), prim: [(f64, f64); 3]) -> Option<M3> {
    let (xn, yn) = white;
    let p: M3 = [
        [prim[0].0, prim[1].0, prim[2].0],
        [prim[0].1, prim[1].1, prim[2].1],
        [
            1.0 - prim[0].0 - prim[0].1,
            1.0 - prim[1].0 - prim[1].1,
            1.0 - prim[2].0 - prim[2].1,
        ],
    ];
    let w = xy_to_xyz(xn, yn);
    let coef = mul_v(&inv(&p)?, w);
    let r: M3 = std::array::from_fn(|i| std::array::from_fn(|j| p[i][j] * coef[j]));
    Some(mul(&lcms_adaptation(w, LCMS_D50), &r))
}

/// Rec.2020 (Arbeitsfarbraum) → XYZ D50
pub(crate) fn rec2020_to_xyz() -> M3 {
    rgb_to_xyz_d50(D65_XY, REC2020_PRIMARIES).expect("Rec.2020")
}

/// sRGB/Rec.709 (linear) → XYZ D50
pub(crate) fn srgb_to_xyz() -> M3 {
    rgb_to_xyz_d50(D65_XY, SRGB_PRIMARIES).expect("sRGB")
}

/// Lineares sRGB → Rec.2020 und zurück (für sigmoid in der Entwicklung)
pub(crate) fn srgb_rec2020() -> (M3, M3) {
    let to = mul(&inv(&rec2020_to_xyz()).expect("Rec.2020"), &srgb_to_xyz());
    let back = inv(&to).expect("invertierbar");
    (to, back)
}

/// Kamera-RGB → XYZ D50 aus der Adobe-Matrix XYZ → Kamera
/// (`dt_colorspaces_create_xyzimatrix_profile`).
pub(crate) fn camera_to_xyz(xyz_to_cam: &M3) -> Option<M3> {
    let mat = inv(xyz_to_cam)?;
    let mut prim = [(0.0, 0.0); 3];
    for (k, p) in prim.iter_mut().enumerate() {
        let norm = mat[0][k] + mat[1][k] + mat[2][k];
        *p = (mat[0][k] / norm, mat[1][k] / norm);
    }
    // darktable `d65` (colorspaces.c) als xy
    let d65 = [0.95045471, 1.0, 1.08905029];
    let s: f64 = d65.iter().sum();
    rgb_to_xyz_d50((d65[0] / s, d65[1] / s), prim)
}

/// Weißabgleich-Koeffizienten für D65 aus der Kameramatrix
/// (`dt_colorspaces_conversion_matrices_rgb`, auf Grün normiert).
pub(crate) fn d65_coeffs(xyz_to_cam: &M3) -> [f64; 3] {
    // sRGB D65 → XYZ wie dort
    const RGB_TO_XYZ: M3 = [
        [0.412453, 0.357580, 0.180423],
        [0.212671, 0.715160, 0.072169],
        [0.019334, 0.119193, 0.950227],
    ];
    let rgb_to_cam = mul(xyz_to_cam, &RGB_TO_XYZ);
    let m: [f64; 3] = std::array::from_fn(|i| 1.0 / rgb_to_cam[i].iter().sum::<f64>());
    [m[0] / m[1], 1.0, m[2] / m[1]]
}

// --- Lichtart (illuminants.h) ---------------------------------------------------

const XYZ_TO_BRADFORD_LMS: M3f = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];
const BRADFORD_LMS_TO_XYZ: M3f = [
    [0.9870, -0.1471, 0.1600],
    [0.4323, 0.5184, 0.0493],
    [-0.0085, 0.0400, 0.9685],
];
pub(crate) const XYZ_TO_CAT16_LMS: M3f = [
    [0.401288, 0.650173, -0.051461],
    [-0.250268, 1.204414, 0.045854],
    [-0.002079, 0.048952, 0.953127],
];
pub(crate) const CAT16_LMS_TO_XYZ: M3f = [
    [1.862068, -1.011255, 0.149187],
    [0.38752, 0.621447, -0.008974],
    [-0.015841, -0.034123, 1.049964],
];
/// D50 im CAT16-LMS (chromatic_adaptation.h)
pub(crate) const CAT16_D50: [f32; 3] = [0.994535, 1.000997, 0.833036];
/// darktable `D50xyY`
pub(crate) const D50_XY: (f32, f32) = (0.34567, 0.35850);

/// Lichtart (xy) aus den Weißabgleich-Koeffizienten der Kamera
/// (`find_temperature_from_raw_coeffs` → `WB_coeffs_to_illuminant_xy`).
pub(crate) fn illuminant_from_coeffs(xyz_to_cam: &M3, wb: [f32; 3]) -> Option<(f32, f32)> {
    let cam_to_xyz = to_f32(&inv(xyz_to_cam)?);
    // Weiß der Kamera (1, 1, 1) / WB in XYZ
    let v = [1.0 / wb[0], 1.0 / wb[1], 1.0 / wb[2]];
    let xyz = apply(&cam_to_xyz, v);
    // Matrizen sind auf D65 bezogen, die Pipeline auf D50 (lineares Bradford)
    const D65: [f32; 3] = [0.941238, 1.040633, 1.088932];
    const D50: [f32; 3] = [0.996078, 1.020646, 0.818155];
    let lms = apply(&XYZ_TO_BRADFORD_LMS, xyz);
    let lms = [
        D50[0] * lms[0] / D65[0],
        D50[1] * lms[1] / D65[1],
        D50[2] * lms[2] / D65[2],
    ];
    let mut xyz = apply(&BRADFORD_LMS_TO_XYZ, lms);
    xyz[0] /= xyz[1];
    xyz[2] /= xyz[1];
    xyz[1] = 1.0;
    let s = xyz[0] + xyz[1] + xyz[2];
    let (x, y) = (xyz[0] / s, xyz[1] / s);
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

fn xy_to_cct(x: f32, y: f32) -> f32 {
    let n = (x - 0.3366) / (y - 0.1735);
    -949.86315
        + 6253.80338 * (-n / 0.92159).exp()
        + 28.70599 * (-n / 0.20039).exp()
        + 0.00004 * (-n / 0.07125).exp()
}

fn cct_to_xy_daylight(t: f32) -> (f32, f32) {
    let x = if (4000.0..=7000.0).contains(&t) {
        ((-4.6070e9 / t + 2.9678e6) / t + 0.09911e3) / t + 0.244063
    } else if t > 7000.0 && t <= 25000.0 {
        ((-2.0064e9 / t + 1.9018e6) / t + 0.24748e3) / t + 0.237040
    } else {
        0.0
    };
    (x, (-3.0 * x + 2.87) * x - 0.275)
}

fn cct_to_xy_blackbody(t: f32) -> (f32, f32) {
    let x = if (1667.0..=4000.0).contains(&t) {
        ((-0.2661239e9 / t - 0.2343589e6) / t + 0.8776956e3) / t + 0.179910
    } else if t > 4000.0 && t <= 25000.0 {
        ((-3.0258469e9 / t + 2.1070379e6) / t + 0.2226347e3) / t + 0.240390
    } else {
        0.0
    };
    let y = if (1667.0..=2222.0).contains(&t) {
        ((-1.1063814 * x - 1.34811020) * x + 2.18555832) * x - 0.20219683
    } else if t > 2222.0 && t <= 4000.0 {
        ((-0.9549476 * x - 1.37418593) * x + 2.09137015) * x - 0.16748867
    } else if t > 4000.0 && t <= 25000.0 {
        ((3.0817580 * x - 5.87338670) * x + 3.75112997) * x - 0.37001483
    } else {
        0.0
    };
    (x, y)
}

fn cct_reverse_lookup(x: f32, y: f32) -> f32 {
    const T_MIN: f32 = 1667.0;
    const T_MAX: f32 = 25000.0;
    const SAMPLES: usize = 1 << 16;
    let mut best = (f32::MAX, 0.0f32);
    for i in 0..SAMPLES {
        let step = (i as f32 / (SAMPLES - 1) as f32).powf(4.0);
        let t = T_MIN + step * (T_MAX - T_MIN);
        let (xb, yb) = if t >= 4000.0 {
            cct_to_xy_daylight(t)
        } else {
            cct_to_xy_blackbody(t)
        };
        let r = (xb - x).hypot(yb - y);
        if r < best.0 {
            best = (r, t);
        }
    }
    best.1
}

fn xy_to_uv(x: f32, y: f32) -> (f32, f32) {
    let d = 12.0 * y - 1.882 * x + 2.9088;
    ((5.5932 * x + 1.9116 * y) / d, (7.8972 * y) / d)
}

/// Lichtart für color calibration wie `reload_defaults` +
/// `_check_if_close_to_daylight` + `commit_params`: liegt die Lichtart der
/// Kamera nahe an Tageslicht bzw. Planck (Abstand < 0,005 in uv), nimmt
/// darktable das Modell bei der ermittelten Farbtemperatur, sonst xy direkt.
pub(crate) fn calibration_illuminant(x: f32, y: f32) -> (f32, f32) {
    let mut t = xy_to_cct(x, y);
    if t < 3000.0 && t > 1667.0 {
        t = cct_reverse_lookup(x, y);
    }
    let (u, v) = xy_to_uv(x, y);
    let dist = |(xt, yt): (f32, f32)| {
        let (ut, vt) = xy_to_uv(xt, yt);
        (ut - u).hypot(vt - v)
    };
    let delta_daylight = dist(cct_to_xy_daylight(t));
    let delta_bb = dist(cct_to_xy_blackbody(t));
    if delta_bb < 0.005 || delta_daylight < 0.005 {
        // illuminant_to_xy: D fällt auf BB zurück, BB auf die Kamera
        let daylight = delta_bb >= delta_daylight;
        if daylight {
            let (xd, yd) = cct_to_xy_daylight(t);
            if xd != 0.0 && yd != 0.0 {
                return (xd, yd);
            }
        }
        let (xb, yb) = cct_to_xy_blackbody(t);
        if xb != 0.0 && yb != 0.0 {
            return (xb, yb);
        }
    }
    (x, y)
}

// --- color calibration (channelmixerrgb, CAT16, Standardwerte) -------------------

/// Vorberechnete Werte für [`calibrate_pixel`].
pub(crate) struct Calibration {
    /// Arbeits-RGB → CAT16-LMS
    rgb_to_lms: M3f,
    /// XYZ → Arbeits-RGB
    xyz_to_rgb: M3f,
    /// Lichtart im CAT16-LMS
    illuminant: [f32; 3],
}

impl Calibration {
    pub(crate) fn new(illuminant_xy: (f32, f32)) -> Calibration {
        let rgb_to_xyz = rec2020_to_xyz();
        let xyz_to_rgb = inv(&rgb_to_xyz).expect("Rec.2020");
        let (x, y) = illuminant_xy;
        let xyz = [x / y, 1.0, (1.0 - x - y) / y];
        Calibration {
            rgb_to_lms: to_f32(&mul(
                &XYZ_TO_CAT16_LMS.map(|r| r.map(f64::from)),
                &rgb_to_xyz,
            )),
            xyz_to_rgb: to_f32(&xyz_to_rgb),
            illuminant: apply(&XYZ_TO_CAT16_LMS, xyz),
        }
    }
}

const NORM_MIN: f32 = 1.525_878_9e-5;
const INVERSE_SQRT_3: f32 = 0.577_350_26;

#[inline]
fn clipneg(v: [f32; 3]) -> [f32; 3] {
    // fmaxf(NaN, 0) = 0 wie dt_vector_clipneg_nan
    v.map(|c| if c > 0.0 { c } else { 0.0 })
}

/// `_gamut_mapping` mit Kompression 1 (Standard „gamut compression“ 1) und Beschneiden
#[inline]
fn gamut_mapping(xyz: [f32; 3]) -> [f32; 3] {
    const D50_UV: [f32; 2] = [0.209_159_15, 0.488_075_32];
    let sum = xyz[0] + xyz[1] + xyz[2];
    let y_lum = xyz[1];
    let (x, y) = if sum > 0.0 {
        (xyz[0] / sum, xyz[1] / sum)
    } else {
        D50_XY
    };
    let den = -2.0 * x + 12.0 * y + 3.0;
    let mut uv = [4.0 * x / den, 9.0 * y / den];
    let delta = [D50_UV[0] - uv[0], D50_UV[1] - uv[1]];
    let big_delta = y_lum * (delta[0] * delta[0] + delta[1] * delta[1]);
    // Kompression 1: powf(Delta, 1)
    let correction = big_delta;
    for c in 0..2 {
        let tmp = correction.mul_add(delta[c], uv[c]);
        uv[c] = if uv[c] > D50_UV[c] {
            tmp.max(D50_UV[c])
        } else {
            tmp.min(D50_UV[c])
        };
    }
    let den = 6.0 * uv[0] - 16.0 * uv[1] + 12.0;
    let mut xy = [9.0 * uv[0] / den, 4.0 * uv[1] / den];
    // Beschneiden
    xy[0] = xy[0].max(0.0);
    xy[1] = xy[1].max(0.0);
    xy[1] = xy[1].max(NORM_MIN);
    let scale = xy[0] + xy[1];
    if scale >= 1.0 {
        xy[0] /= scale;
        xy[1] /= scale;
    }
    if xy[1] == 0.0 {
        return [0.0; 3];
    }
    [
        y_lum * xy[0] / xy[1],
        y_lum,
        y_lum * (1.0 - xy[0] - xy[1]) / xy[1],
    ]
}

/// `_luma_chroma` (Version 3) mit Sättigung und Helligkeit 0
#[inline]
fn luma_chroma(input: [f32; 3]) -> [f32; 3] {
    let euclid = |v: [f32; 3]| {
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2])
            .sqrt()
            .max(NORM_MIN)
    };
    let mut norm = euclid(input);
    let avg = ((input[0] + input[1] + input[2]) / 3.0).max(NORM_MIN);
    if norm > 0.0 && avg > 0.0 {
        norm *= INVERSE_SQRT_3;
        let mut out = input.map(|v| v / norm);
        // coeff_ratio = 0 → Verhältnisse bleiben (min_ratio wie im Original)
        for o in &mut out {
            let min_ratio = if *o < 0.0 { *o } else { 0.0 };
            *o = (*o).max(min_ratio);
        }
        norm /= euclid(out) * INVERSE_SQRT_3;
        // mix = 0 → Faktor 1
        out.map(|v| v * norm)
    } else {
        input
    }
}

/// color calibration für ein Pixel (Arbeits-RGB → Arbeits-RGB), wie
/// `_loop_switch` mit CAT16, Mischung = Einheit, Beschneiden an.
#[inline]
pub(crate) fn calibrate_pixel(c: &Calibration, rgb: [f32; 3]) -> [f32; 3] {
    let t = clipneg(rgb);
    let lms = apply(&c.rgb_to_lms, t);
    let lms = [
        lms[0] * CAT16_D50[0] / c.illuminant[0],
        lms[1] * CAT16_D50[1] / c.illuminant[1],
        lms[2] * CAT16_D50[2] / c.illuminant[2],
    ];
    let xyz = clipneg(apply(&CAT16_LMS_TO_XYZ, lms));
    let xyz = gamut_mapping(xyz);
    let lms = clipneg(apply(&XYZ_TO_CAT16_LMS, xyz));
    let lms = clipneg(luma_chroma(lms));
    let xyz = clipneg(apply(&CAT16_LMS_TO_XYZ, lms));
    clipneg(apply(&c.xyz_to_rgb, xyz))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_space_maps_white_to_white() {
        // D65-Weiß in sRGB → Rec.2020 bleibt (1, 1, 1)
        let (to, back) = srgb_rec2020();
        let w = mul_v(&to, [1.0, 1.0, 1.0]);
        assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-9), "{w:?}");
        let r = mul_v(&back, mul_v(&to, [0.2, 0.5, 0.9]));
        assert!((r[0] - 0.2).abs() < 1e-12 && (r[2] - 0.9).abs() < 1e-12);
        // Bekannte Matrix sRGB → Rec.2020 (Zeile 1: 0,6274 0,3293 0,0433)
        assert!((to[0][0] - 0.6274).abs() < 2e-3, "{to:?}");
    }

    #[test]
    fn neutral_stays_neutral_in_calibration() {
        // Lichtart D50 = Pipeline: keine Anpassung, Grau bleibt grau
        let c = Calibration::new(D50_XY);
        let g = calibrate_pixel(&c, [0.18, 0.18, 0.18]);
        assert!(g.iter().all(|v| (v - 0.18).abs() < 2e-3), "{g:?}");
    }

    #[test]
    fn daylight_illuminant_is_snapped_to_model() {
        // xy nahe D65 → Tageslicht-Modell bei der Farbtemperatur
        let (x, y) = calibration_illuminant(0.3128, 0.3292);
        assert!((x - 0.3128).abs() < 2e-3 && (y - 0.3292).abs() < 2e-3);
        let t = xy_to_cct(0.3128, 0.3292);
        assert!((t - 6500.0).abs() < 100.0, "{t}");
    }
}
