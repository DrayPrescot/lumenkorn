//! Farb- und Transferfunktionen.

use std::sync::OnceLock;

/// Luminanzgewichte für Rec.709/sRGB-Primärvalenzen.
pub const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

#[inline]
pub fn luma(p: [f32; 3]) -> f32 {
    LUMA[0] * p[0] + LUMA[1] * p[1] + LUMA[2] * p[2]
}

#[inline]
pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
pub fn linear_to_srgb_exact(v: f32) -> f32 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

const ENC_LUT_SIZE: usize = 1 << 14;

fn enc_lut() -> &'static [f32] {
    static LUT: OnceLock<Vec<f32>> = OnceLock::new();
    LUT.get_or_init(|| {
        (0..=ENC_LUT_SIZE)
            .map(|i| linear_to_srgb_exact(i as f32 / ENC_LUT_SIZE as f32))
            .collect()
    })
}

/// sRGB-Kodierung über eine Tabelle mit linearer Interpolation (≈ 30× schneller
/// als `powf`, Abweichung < 1/4000).
#[inline]
pub fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.0 {
        return 0.0;
    }
    if v >= 1.0 {
        return 1.0;
    }
    // Ganz unten ist die Kurve linear – dort ist die Tabelle zu grob.
    if v <= 0.0031308 {
        return v * 12.92;
    }
    let lut = enc_lut();
    let f = v * ENC_LUT_SIZE as f32;
    let i = f as usize;
    let t = f - i as f32;
    lut[i] + (lut[i + 1] - lut[i]) * t
}

/// Tabelle für 8- und 16-Bit-Eingaben.
pub fn srgb_decode_lut_u16() -> &'static [f32] {
    static LUT: OnceLock<Vec<f32>> = OnceLock::new();
    LUT.get_or_init(|| {
        (0..=u16::MAX)
            .map(|i| srgb_to_linear(i as f32 / 65535.0))
            .collect()
    })
}

#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lut_matches_exact() {
        for i in 0..=1000 {
            let v = i as f32 / 1000.0;
            assert!(
                (linear_to_srgb(v) - linear_to_srgb_exact(v)).abs() < 3e-4,
                "{v}"
            );
        }
    }

    #[test]
    fn roundtrip() {
        for i in 0..=255 {
            let v = i as f32 / 255.0;
            assert!((linear_to_srgb(srgb_to_linear(v)) - v).abs() < 5e-4);
        }
    }
}

// --- Oklab und HSL --------------------------------------------------------------

/// Lineares Rec.709/sRGB → Oklab (Björn Ottosson). Wahrnehmungsnah: gleiche
/// Abstände sehen ungefähr gleich verschieden aus.
pub fn oklab(c: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = c.map(|v| v.max(0.0));
    let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// Oklab → lineares Rec.709/sRGB.
pub fn oklab_to_linear([lab_l, a, b]: [f32; 3]) -> [f32; 3] {
    let l = (lab_l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (lab_l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s = (lab_l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

/// Mittelpunkte der HSL-Farbbereiche als Oklch-Farbtonwinkel (Grad):
/// Rot, Orange, Gelb, Grün, Aquamarin, Blau, Lila, Magenta.
pub const HSL_CENTERS: [f32; 8] = [29.0, 60.0, 110.0, 142.0, 195.0, 264.0, 294.0, 328.0];

/// Anteile der Farbbereiche an einem Farbton (Grad): linear zwischen den
/// beiden benachbarten Mittelpunkten, Summe immer 1.
pub fn hsl_weights(hue_deg: f32) -> [f32; 8] {
    let h = hue_deg.rem_euclid(360.0);
    let mut w = [0f32; 8];
    for i in 0..8 {
        let a = HSL_CENTERS[i];
        let mut b = HSL_CENTERS[(i + 1) % 8];
        if b <= a {
            b += 360.0;
        }
        let mut x = h;
        if x < a {
            x += 360.0;
        }
        if x >= a && x < b {
            let t = (x - a) / (b - a);
            w[i] = 1.0 - t;
            w[(i + 1) % 8] = t;
            break;
        }
    }
    w
}

/// HSL-Einstellung je Farbbereich, normiert auf −1…1.
#[derive(Clone, Copy)]
pub struct HslAdjust {
    pub hue: [f32; 8],
    pub sat: [f32; 8],
    pub lum: [f32; 8],
}

/// Wendet HSL auf ein sRGB-kodiertes Pixel an. Gerechnet wird in Oklch:
/// Farbton um bis zu ±30° drehen, Buntheit 0…2-fach, Helligkeit ±50 %.
/// Graue Pixel (kaum Buntheit) bleiben bei Farbton und Luminanz unberührt –
/// sonst würde „Blau dunkler“ auch graue Wolken abdunkeln.
pub fn hsl_pixel(p: [f32; 3], adj: &HslAdjust) -> [f32; 3] {
    let lin = p.map(|v| srgb_to_linear(v.clamp(0.0, 1.0)));
    let [l, a, b] = oklab(lin);
    let c = (a * a + b * b).sqrt();
    if c < 1e-5 {
        return p;
    }
    let hue = b.atan2(a).to_degrees();
    let w = hsl_weights(hue);
    let (mut dh, mut ds, mut dl) = (0f32, 0f32, 0f32);
    for (i, wi) in w.iter().enumerate() {
        dh += wi * adj.hue[i];
        ds += wi * adj.sat[i];
        dl += wi * adj.lum[i];
    }
    let g = smoothstep(0.0, 0.04, c);
    let h2 = (hue + dh * 30.0 * g).to_radians();
    let c2 = c * (1.0 + ds).max(0.0);
    let l2 = l * (1.0 + dl * 0.5 * g);
    let out = oklab_to_linear([l2, c2 * h2.cos(), c2 * h2.sin()]);
    out.map(|v| linear_to_srgb(v.clamp(0.0, 1.0)))
}

#[cfg(test)]
mod hsl_tests {
    use super::*;

    fn enc(c: [f32; 3]) -> [f32; 3] {
        c.map(linear_to_srgb_exact)
    }
    fn zero() -> HslAdjust {
        HslAdjust {
            hue: [0.0; 8],
            sat: [0.0; 8],
            lum: [0.0; 8],
        }
    }

    #[test]
    fn weights_partition_the_hue_circle() {
        for h in (0..360).step_by(7) {
            let w = hsl_weights(h as f32);
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5, "Summe bei {h}°");
        }
        assert!(
            (hsl_weights(HSL_CENTERS[5])[5] - 1.0).abs() < 1e-5,
            "Blau-Mitte"
        );
        let w = hsl_weights(350.0); // zwischen Magenta (328) und Rot (29+360)
        assert!(w[7] > 0.0 && w[0] > 0.0);
    }

    #[test]
    fn hsl_targets_only_the_chosen_colors() {
        let red = enc([0.8, 0.05, 0.03]);
        let blue = enc([0.03, 0.08, 0.7]);
        let gray = enc([0.3, 0.3, 0.3]);

        // Neutral: (fast) unverändert
        let same = hsl_pixel(red, &zero());
        assert!(
            same.iter().zip(red).all(|(a, b)| (a - b).abs() < 2e-3),
            "{same:?}"
        );

        // Rot entsättigen: Rot wird grau, Blau bleibt
        let mut a = zero();
        a.sat[0] = -1.0;
        let r = hsl_pixel(red, &a);
        assert!(
            (r[0] - r[1]).abs() < 0.05 && (r[1] - r[2]).abs() < 0.05,
            "Rot grau: {r:?}"
        );
        let b = hsl_pixel(blue, &a);
        assert!(
            b.iter().zip(blue).all(|(x, y)| (x - y).abs() < 2e-3),
            "Blau unberührt"
        );

        // Blau abdunkeln: Blau dunkler, Grau unverändert
        let mut a = zero();
        a.lum[5] = -1.0;
        assert!(luma(hsl_pixel(blue, &a)) < luma(blue) * 0.7);
        assert_eq!(hsl_pixel(gray, &a), gray);

        // Farbton von Blau Richtung Lila drehen: Rotanteil steigt
        let mut a = zero();
        a.hue[5] = 1.0;
        assert!(hsl_pixel(blue, &a)[0] > blue[0] + 0.05);
    }
}
