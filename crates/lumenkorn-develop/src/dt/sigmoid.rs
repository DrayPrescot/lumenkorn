//! Tonwertabbildung „sigmoid“, übertragen aus darktable 5.0 `src/iop/sigmoid.c`
//! (Copyright (C) 2020-2024 darktable developers, GPL-3.0-or-later).
//!
//! Standardwerte wie darktables automatisch angewendete Voreinstellung:
//! Kontrast 1,5, Schiefe 0, Weiß 100 %, Schwarz 0,0152 %, „je Kanal“ mit 100 %
//! Farbton-Erhalt, Grundfarben = Arbeitsfarbraum (Rec.2020) ohne Einzug und
//! Drehung – die Umrechnung in den Darstellungsraum ist dann die Einheit.

/// `MIDDLE_GREY`
const MIDDLE_GREY: f32 = 0.1845;
pub(crate) const CONTRAST: f32 = 1.5;
pub(crate) const SKEW: f32 = 0.0;
pub(crate) const WHITE_TARGET: f32 = 100.0;
pub(crate) const BLACK_TARGET: f32 = 0.0152;
pub(crate) const HUE_PRESERVATION: f32 = 100.0;

/// Kurvenparameter aus `commit_params`
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sigmoid {
    pub white_target: f32,
    pub paper_exposure: f32,
    pub film_fog: f32,
    pub film_power: f32,
    pub paper_power: f32,
    pub hue_preservation: f32,
}

#[inline]
fn loglogistic(
    value: f32,
    magnitude: f32,
    paper_exp: f32,
    film_fog: f32,
    film_power: f32,
    paper_power: f32,
) -> f32 {
    let clamped = value.max(0.0);
    let film_response = (film_fog + clamped).powf(film_power);
    let ratio = film_response / (paper_exp + film_response);
    // Ohne Schiefe ist der Exponent genau 1 (`x^1 == x` exakt) – spart je
    // Kanal ein `powf`, bei jeder RAW-Vorschau Millionen davon
    let paper_response = magnitude
        * if paper_power == 1.0 {
            ratio
        } else {
            ratio.powf(paper_power)
        };
    if paper_response.is_nan() {
        magnitude
    } else {
        paper_response
    }
}

impl Sigmoid {
    /// Standardeinstellung von darktable
    pub(crate) fn default_params() -> Sigmoid {
        Sigmoid::new(CONTRAST, SKEW, WHITE_TARGET, BLACK_TARGET, HUE_PRESERVATION)
    }

    /// `commit_params`
    pub(crate) fn new(
        contrast: f32,
        skew: f32,
        display_white: f32,
        display_black: f32,
        hue: f32,
    ) -> Sigmoid {
        let ref_film_power = contrast;
        let ref_paper_power = 1.0;
        let ref_magnitude = 1.0;
        let ref_film_fog = 0.0;
        let ref_paper_exposure = (ref_film_fog + MIDDLE_GREY).powf(ref_film_power)
            * ((ref_magnitude / MIDDLE_GREY) - 1.0);
        let delta = 1e-6f32;
        let ref_slope = (loglogistic(
            MIDDLE_GREY + delta,
            ref_magnitude,
            ref_paper_exposure,
            ref_film_fog,
            ref_film_power,
            ref_paper_power,
        ) - loglogistic(
            MIDDLE_GREY - delta,
            ref_magnitude,
            ref_paper_exposure,
            ref_film_fog,
            ref_film_power,
            ref_paper_power,
        )) / 2.0
            / delta;

        let paper_power = 5f32.powf(-skew);

        let temp_film_power = 1.0;
        let temp_white_target = 0.01 * display_white;
        let temp_white_grey_relation =
            (temp_white_target / MIDDLE_GREY).powf(1.0 / paper_power) - 1.0;
        let temp_paper_exposure = MIDDLE_GREY.powf(temp_film_power) * temp_white_grey_relation;
        let temp_slope = (loglogistic(
            MIDDLE_GREY + delta,
            temp_white_target,
            temp_paper_exposure,
            ref_film_fog,
            temp_film_power,
            paper_power,
        ) - loglogistic(
            MIDDLE_GREY - delta,
            temp_white_target,
            temp_paper_exposure,
            ref_film_fog,
            temp_film_power,
            paper_power,
        )) / 2.0
            / delta;

        let film_power = ref_slope / temp_slope;
        let white_target = 0.01 * display_white;
        let black_target = 0.01 * display_black;
        let white_grey_relation = (white_target / MIDDLE_GREY).powf(1.0 / paper_power) - 1.0;
        let white_black_relation = (black_target / white_target).powf(-1.0 / paper_power) - 1.0;
        let film_fog = MIDDLE_GREY * white_grey_relation.powf(1.0 / film_power)
            / (white_black_relation.powf(1.0 / film_power)
                - white_grey_relation.powf(1.0 / film_power));
        let paper_exposure = (film_fog + MIDDLE_GREY).powf(film_power) * white_grey_relation;
        Sigmoid {
            white_target,
            paper_exposure,
            film_fog,
            film_power,
            paper_power,
            hue_preservation: (0.01 * hue).clamp(0.0, 1.0),
        }
    }

    #[inline]
    fn curve(&self, v: f32) -> f32 {
        loglogistic(
            v,
            self.white_target,
            self.paper_exposure,
            self.film_fog,
            self.film_power,
            self.paper_power,
        )
    }

    /// Ein Pixel im Arbeitsfarbraum (`process_loglogistic_per_channel`)
    #[inline]
    pub(crate) fn apply(&self, pix: [f32; 3]) -> [f32; 3] {
        let rgb = desaturate_negative_values(pix);
        let per_channel = rgb.map(|v| self.curve(v));
        let order = channel_order(rgb);
        preserve_hue_and_energy(rgb, per_channel, order, self.hue_preservation)
    }
}

/// `_desaturate_negative_values`
#[inline]
fn desaturate_negative_values(p: [f32; 3]) -> [f32; 3] {
    let avg = ((p[0] + p[1] + p[2]) / 3.0).max(0.0);
    let min = p[0].min(p[1]).min(p[2]);
    let f = if min < 0.0 { -avg / (min - avg) } else { 1.0 };
    p.map(|v| avg + f * (v - avg))
}

/// (min, mid, max) wie `_pixel_channel_order`
#[inline]
fn channel_order(p: [f32; 3]) -> (usize, usize, usize) {
    if p[0] >= p[1] {
        if p[1] > p[2] {
            (2, 1, 0)
        } else if p[2] > p[0] {
            (1, 0, 2)
        } else if p[2] > p[1] {
            (1, 2, 0)
        } else {
            (2, 1, 0)
        }
    } else if p[0] >= p[2] {
        (2, 0, 1)
    } else if p[2] > p[1] {
        (0, 1, 2)
    } else {
        (0, 2, 1)
    }
}

/// `_preserve_hue_and_energy`
#[inline]
fn preserve_hue_and_energy(
    pix_in: [f32; 3],
    per_channel: [f32; 3],
    (min, mid, max): (usize, usize, usize),
    hue_preservation: f32,
) -> [f32; 3] {
    let chroma = pix_in[max] - pix_in[min];
    let midscale = if chroma != 0.0 {
        (pix_in[mid] - pix_in[min]) / chroma
    } else {
        0.0
    };
    let full_hue_correction = per_channel[min] + (per_channel[max] - per_channel[min]) * midscale;
    let naive_hue_mid =
        (1.0 - hue_preservation) * per_channel[mid] + hue_preservation * full_hue_correction;

    let per_channel_energy = per_channel[0] + per_channel[1] + per_channel[2];
    let naive_hue_energy = per_channel[min] + naive_hue_mid + per_channel[max];
    let min_plus_mid = pix_in[min] + pix_in[mid];
    let blend = if min_plus_mid != 0.0 {
        2.0 * pix_in[min] / min_plus_mid
    } else {
        0.0
    };
    let energy_target = blend * per_channel_energy + (1.0 - blend) * naive_hue_energy;

    let mut out = [0f32; 3];
    if naive_hue_mid <= per_channel[mid] {
        let corrected_mid = ((1.0 - hue_preservation) * per_channel[mid]
            + hue_preservation
                * (midscale * per_channel[max]
                    + (1.0 - midscale) * (energy_target - per_channel[max])))
            / (1.0 + hue_preservation * (1.0 - midscale));
        out[min] = energy_target - per_channel[max] - corrected_mid;
        out[mid] = corrected_mid;
        out[max] = per_channel[max];
    } else {
        let corrected_mid = ((1.0 - hue_preservation) * per_channel[mid]
            + hue_preservation
                * (per_channel[min] * (1.0 - midscale)
                    + midscale * (energy_target - per_channel[min])))
            / (1.0 + hue_preservation * midscale);
        out[min] = per_channel[min];
        out[mid] = corrected_mid;
        out[max] = energy_target - per_channel[min] - corrected_mid;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_maps_to_middle_grey_and_white_is_never_reached() {
        let s = Sigmoid::default_params();
        let g = s.apply([MIDDLE_GREY; 3]);
        assert!(g.iter().all(|v| (v - MIDDLE_GREY).abs() < 1e-3), "{g:?}");
        let w = s.apply([1000.0; 3]);
        assert!(w[0] < 1.0 && w[0] > 0.99, "{w:?}");
        let b = s.apply([0.0; 3]);
        assert!(b[0] >= 0.0 && b[0] < 0.001, "{b:?}");
        // Monoton
        let mut last = 0.0;
        for i in 0..200 {
            let v = s.apply([i as f32 * 0.05; 3])[0];
            assert!(v >= last);
            last = v;
        }
    }

    #[test]
    fn hue_is_kept_for_bright_colors() {
        // Verhältnis (mid − min) / (max − min) bleibt bei 100 % Farbton-Erhalt
        let s = Sigmoid::default_params();
        let p = [4.0f32, 2.0, 1.0];
        let o = s.apply(p);
        let r_in = (p[1] - p[2]) / (p[0] - p[2]);
        let r_out = (o[1] - o[2]) / (o[0] - o[2]);
        assert!((r_in - r_out).abs() < 1e-3, "{o:?}");
    }
}
