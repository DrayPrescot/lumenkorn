//! „Auto“ im Reiter Basis: stellt Belichtung, Kontrast, Lichter, Tiefen,
//! Weiß, Schwarz und Dynamik so ein, dass das Foto ausgewogen aussieht.
//!
//! Statt fester Formeln wird das Foto klein (wenige hundert Pixel) mit der
//! echten Pipeline entwickelt und gemessen; jeder Regler wird der Reihe nach
//! per Intervallhalbierung auf ein Ziel gebracht. So passt das Ergebnis auch
//! zu Basiskurve, Objektivkorrektur und Zuschnitt, und Vorschau und Export
//! sehen gleich aus. Alle Messungen gelten für die sRGB-kodierte Helligkeit
//! des fertigen Bildes:
//!
//! 1. **Belichtung:** Median der Helligkeit auf [`TARGET_MEDIAN`] – höchstens
//!    so weit, dass nicht mehr als [`MAX_CLIP`] der Pixel ausfressen. Sehr
//!    dunkle Szenen (Nacht, Gegenlicht mit viel Schwarz) werden weniger weit
//!    angehoben, damit sie nicht grau wirken.
//! 2. **Weiß / Schwarz:** hellste Stellen knapp unter Weiß, dunkelste knapp
//!    über Schwarz – der Tonwertumfang wird genutzt, ohne zu beschneiden.
//! 3. **Kontrast:** restliche Streuung der Helligkeit Richtung
//!    [`TARGET_SPREAD`], vorsichtig (flaue Bilder etwas mehr, harte weniger).
//! 4. **Lichter / Tiefen:** helle Flächen zurückholen, bis das 98. Perzentil
//!    Zeichnung hat; dunkle Bereiche aufhellen, bis das 8. Perzentil sichtbar
//!    wird.
//! 5. **Dynamik:** blasse Bilder bekommen etwas mehr Farbe (nie weniger).
//!
//! Masken, Reparaturen und Rauschreduzierung bleiben bei der Messung außen
//! vor; alle übrigen Einstellungen (Weißabgleich, Farbe, Zuschnitt …) gelten.

use crate::color::luma;
use crate::image_f32::ImageF32;
use crate::params::DevelopParams;
use crate::pipeline::{RenderOptions, render_with};
use crate::source::Source;
use rayon::prelude::*;

/// Kantenlänge der Messbilder
const EDGE: usize = 320;
/// Ziel für den Median der Helligkeit (sRGB, 0…1)
const TARGET_MEDIAN: f32 = 0.44;
/// Höchstens so viele Pixel dürfen durch die Belichtung ausfressen
const MAX_CLIP: f32 = 0.005;
/// Ziel für die Standardabweichung der Helligkeit
const TARGET_SPREAD: f32 = 0.18;
/// Hellste Stellen (99,5. Perzentil des hellsten Farbkanals) knapp unter Weiß
const TARGET_WHITE: f32 = 0.93;
/// Dunkelste Stellen (0,5. Perzentil der Helligkeit) knapp über Schwarz
const TARGET_BLACK: f32 = 0.03;
/// Anteil des Wegs zu den Zielen für Weiß und Schwarz
const ENDS_SHARE: f32 = 0.75;

/// Ergebnis: die Werte der automatisch gesetzten Regler.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoTone {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub vibrance: f32,
}

impl AutoTone {
    fn apply(&self, p: &mut DevelopParams) {
        p.exposure = self.exposure;
        p.contrast = self.contrast;
        p.highlights = self.highlights;
        p.shadows = self.shadows;
        p.whites = self.whites;
        p.blacks = self.blacks;
        p.vibrance = self.vibrance;
    }
}

/// Kennzahlen eines entwickelten Bildes.
struct Stats {
    /// sortierte Helligkeiten
    sorted: Vec<f32>,
    /// sortierte Werte des hellsten Kanals (für den Weißpunkt: gesättigte
    /// Farben erreichen in der Helligkeit nie 1)
    sorted_max: Vec<f32>,
    /// Anteil Pixel mit einem Kanal am Anschlag
    clipped: f32,
    mean: f32,
    /// mittlere Sättigung (HSV)
    saturation: f32,
}

impl Stats {
    fn of(img: &ImageF32) -> Self {
        let n = img.data.len().max(1) as f32;
        // Rund 100 Messungen je „Auto“: parallel und instabil sortieren –
        // bei `total_cmp` ist die Reihenfolge gleicher Werte ohnehin egal
        let mut sorted: Vec<f32> = img.data.par_iter().map(|p| luma(*p)).collect();
        sorted.par_sort_unstable_by(f32::total_cmp);
        let mut sorted_max: Vec<f32> = img
            .data
            .par_iter()
            .map(|p| p[0].max(p[1]).max(p[2]))
            .collect();
        sorted_max.par_sort_unstable_by(f32::total_cmp);
        let clipped = img
            .data
            .iter()
            .filter(|p| p.iter().any(|v| *v >= 0.995))
            .count() as f32
            / n;
        let mean = sorted.iter().sum::<f32>() / n;
        let saturation = img
            .data
            .iter()
            .map(|p| {
                let max = p[0].max(p[1]).max(p[2]);
                let min = p[0].min(p[1]).min(p[2]);
                if max > 1e-4 { (max - min) / max } else { 0.0 }
            })
            .sum::<f32>()
            / n;
        Self {
            sorted,
            sorted_max,
            clipped,
            mean,
            saturation,
        }
    }
    fn pct(&self, q: f32) -> f32 {
        pct(&self.sorted, q)
    }
    /// Perzentil des hellsten Kanals
    fn pct_max(&self, q: f32) -> f32 {
        pct(&self.sorted_max, q)
    }
    fn spread(&self) -> f32 {
        let n = self.sorted.len().max(1) as f32;
        (self
            .sorted
            .iter()
            .map(|v| (v - self.mean).powi(2))
            .sum::<f32>()
            / n)
            .sqrt()
    }
}

fn pct(sorted: &[f32], q: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f32 * q).round() as usize]
}

/// Sucht `x` in `[lo, hi]`, so dass `f(x)` das Ziel trifft; `f` steigt mit `x`.
/// Liegt das Ziel außerhalb, wird der passende Rand genommen.
fn solve(
    mut lo: f32,
    mut hi: f32,
    target: f32,
    steps: usize,
    mut f: impl FnMut(f32) -> f32,
) -> f32 {
    if f(lo) >= target {
        return lo;
    }
    if f(hi) <= target {
        return hi;
    }
    for _ in 0..steps {
        let mid = 0.5 * (lo + hi);
        if f(mid) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Berechnet die automatischen Tonwerte für `src` auf Basis der aktuellen
/// Einstellungen `base` (deren Tonregler werden dabei ignoriert).
pub fn auto_tone(src: &Source, base: &DevelopParams) -> AutoTone {
    auto_tone_with_features(src, base).0
}

/// Anzahl der Bildmerkmale aus [`auto_tone_with_features`]
pub const FEATURES: usize = 8;

/// Wie [`auto_tone`], dazu Bildmerkmale des Fotos vor den Tonreglern (für
/// „Auto nach deinem Stil“, siehe `style`): Helligkeit an den Perzentilen
/// 5, 25, 50, 75, 95, Anteil ausgefressener Pixel, mittlere Sättigung,
/// Anteil sehr dunkler Pixel – alles 0…1.
pub fn auto_tone_with_features(src: &Source, base: &DevelopParams) -> (AutoTone, [f32; FEATURES]) {
    let mut small = src.downscaled(EDGE);
    let mut p = base.sanitized();
    // Objektivkorrektur einmal vorab statt in jedem der rund 80 Messdurchläufe
    if p.lens_correction
        && let Some(l) = small.lens.take()
    {
        small.image = l.apply(&small.image);
    }
    p.masks.clear();
    p.repairs.clear();
    p.denoise = None;
    let mut a = AutoTone::default();
    a.apply(&mut p);
    let opts = RenderOptions::default();
    let measure = |a: &AutoTone| {
        let mut q = p.clone();
        a.apply(&mut q);
        Stats::of(&render_with(&small, &q, &opts))
    };

    // 1. Belichtung: Median aufs Ziel, ohne viel auszufressen
    let start = measure(&a);
    // Sehr dunkle Szenen (viel echtes Schwarz) nur teilweise anheben
    let dark_share = start.sorted.iter().take_while(|v| **v < 0.10).count() as f32
        / start.sorted.len().max(1) as f32;
    let features = [
        start.pct(0.05),
        start.pct(0.25),
        start.pct(0.5),
        start.pct(0.75),
        start.pct(0.95),
        start.clipped,
        start.saturation,
        dark_share,
    ];
    // Nachtbild: dunkel im Mittel, aber mit hellen Lichtern – gewollt dunkel
    let night = start.pct(0.5) < 0.12 && start.pct(0.99) > 0.5;
    let dark = if night {
        1.0
    } else {
        (dark_share / 0.5).min(1.0)
    };
    let target = TARGET_MEDIAN - 0.14 * dark;
    // Große helle Flächen (Schnee, Himmel, Gegenlicht): Aufhellen würde sie
    // ausbleichen – dann höchstens leicht heller
    let bright_share = start.sorted.iter().filter(|v| **v > 0.75).count() as f32
        / start.sorted.len().max(1) as f32;
    let ev_median = solve(-3.0, 3.0, target, 9, |ev| {
        measure(&AutoTone { exposure: ev, ..a }).pct(0.5)
    });
    // Was schon ausgefressen ist, bleibt erlaubt – nur kaum mehr dazu
    let base_clip = start.clipped + MAX_CLIP;
    // Belichtung, ab der zu viel ausfrisst (Ausfressen steigt mit der Belichtung)
    let ev_clip = solve(-3.0, 3.0, base_clip, 9, |ev| {
        measure(&AutoTone { exposure: ev, ..a }).clipped
    });
    a.exposure = if ev_median > ev_clip {
        // Dazwischen: die Lichter holt danach der Lichter-Regler zurück
        ev_clip + 0.4 * (ev_median - ev_clip)
    } else {
        ev_median
    };
    // Nie mehr als ±2 Blenden – größere Fehler sind meist Absicht oder Motiv;
    // dunkle Szenen (Nacht) höchstens eine Blende heller
    let max_ev = if bright_share > 0.1 { 0.3 } else { 2.0 - dark };
    a.exposure = (a.exposure.clamp(-2.0, max_ev) / 0.05).round() * 0.05;

    // 2. Weiß und Schwarz: Tonwertumfang nutzen, ohne zu beschneiden. Vor dem
    // Kontrast, weil das Spreizen der Enden die Streuung schon erhöht – sonst
    // landete der Kontrast fast immer am Anschlag.
    // Nur einen Teil des Wegs: flaue Motive (Nebel, Gegenlicht) bleiben etwas
    // weicher, und der Wert richtet sich nach dem Bild statt am Anschlag zu
    // kleben
    let st = measure(&a);
    let (w0, b0) = (st.pct_max(0.995), st.pct(0.005));
    let toward = |from: f32, to: f32| from + ENDS_SHARE * (to - from);
    // Ohne helle Stellen (dunkles Motiv) ist das Ziel unerreichbar – dann
    // nicht bis zum Anschlag, sonst fressen die wenigen Lichter aus
    a.whites = solve(-60.0, 60.0, toward(w0, TARGET_WHITE), 9, |w| {
        measure(&AutoTone { whites: w, ..a }).pct_max(0.995)
    })
    .round();
    a.blacks = solve(-80.0, 40.0, toward(b0, TARGET_BLACK), 9, |b| {
        measure(&AutoTone { blacks: b, ..a }).pct(0.005)
    })
    .round();

    // 3. Kontrast: restliche Streuung in Richtung Ziel, nur teilweise
    let s = measure(&a).spread();
    let wanted = s + 0.5 * (TARGET_SPREAD - s);
    // Weniger Kontrast nur wenig: flaue Bilder wirken schnell grau
    a.contrast = solve(-15.0, 30.0, wanted, 8, |c| {
        measure(&AutoTone { contrast: c, ..a }).spread()
    })
    .round();

    // 4. Lichter: das 98. Perzentil soll Zeichnung haben (unter 0,92)
    let st = measure(&a);
    if st.pct(0.98) > 0.92 || st.clipped > MAX_CLIP {
        // `f` steigt mit dem Regler; gesucht ist der kleinste Eingriff
        a.highlights = solve(-80.0, 0.0, 0.92, 7, |h| {
            measure(&AutoTone { highlights: h, ..a }).pct(0.98)
        })
        .round();
    }
    // Tiefen: das 8. Perzentil soll sichtbar sein (über 0,08)
    let st = measure(&a);
    if st.pct(0.08) < 0.08 {
        a.shadows = solve(0.0, 60.0, 0.08, 7, |s| {
            measure(&AutoTone { shadows: s, ..a }).pct(0.08)
        })
        .round();
    }

    // 5. Dynamik: blasse Bilder etwas farbiger – bei Schwarzweiß gibt es
    // keine Farbe zu messen, dort bleibt der Regler, wie er ist
    a.vibrance = if p.monochrome {
        base.vibrance
    } else {
        let sat = measure(&a).saturation;
        (((0.32 - sat) / 0.32) * 40.0).clamp(0.0, 25.0).round()
    };

    // Feinschliff der Belichtung nach den übrigen Reglern (kleine Korrektur)
    let m = measure(&a).pct(0.5);
    if (m - target).abs() > 0.03 {
        let ev = solve(a.exposure - 0.5, a.exposure + 0.5, target, 6, |ev| {
            measure(&AutoTone { exposure: ev, ..a }).pct(0.5)
        });
        a.exposure = (ev.clamp(-2.0, max_ev) / 0.05).round() * 0.05;
    }
    (a, features)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceKind;

    fn scene(gain: f32, contrast: f32) -> Source {
        // Verlauf mit Himmel, Wiese und Schatten – linear
        let (w, h) = (300usize, 200usize);
        let image = ImageF32::from_fn(w, h, |x, y| {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let base: [f32; 3] = if fy < 0.4 {
                [0.55, 0.65, 0.85]
            } else if fx < 0.3 {
                [0.05, 0.06, 0.05]
            } else {
                [0.2, 0.35, 0.12]
            };
            let tex = 1.0 + 0.15 * ((x * 13 + y * 7) % 11) as f32 / 11.0;
            base.map(|v| ((v * tex - 0.25) * contrast + 0.25).max(0.0) * gain)
        });
        Source {
            kind: SourceKind::Raster,
            full_width: w,
            full_height: h,
            lens: None,
            image,
        }
    }

    fn median_after(src: &Source, a: &AutoTone) -> f32 {
        let mut p = DevelopParams::default();
        a.apply(&mut p);
        Stats::of(&render_with(
            &src.downscaled(EDGE),
            &p,
            &RenderOptions::default(),
        ))
        .pct(0.5)
    }

    #[test]
    fn dark_photo_is_brightened_and_bright_one_darkened() {
        let dark = scene(0.15, 1.0);
        let a = auto_tone(&dark, &DevelopParams::default());
        assert!(a.exposure > 0.8, "{a:?}");
        assert!(
            (median_after(&dark, &a) - TARGET_MEDIAN).abs() < 0.08,
            "{}",
            median_after(&dark, &a)
        );

        let bright = scene(1.6, 1.0);
        let b = auto_tone(&bright, &DevelopParams::default());
        assert!(b.exposure < 0.0 || b.highlights < 0.0, "{b:?}");
    }

    #[test]
    fn flat_photo_gets_contrast_and_color() {
        let flat = scene(0.6, 0.35);
        let a = auto_tone(&flat, &DevelopParams::default());
        assert!(
            a.contrast > 0.0 || a.whites > 0.0 || a.blacks < 0.0,
            "{a:?}"
        );
        assert!(a.vibrance >= 0.0);
    }

    #[test]
    fn well_exposed_photo_keeps_its_exposure() {
        // Mittlere Helligkeit stimmt schon; nur der Tonwertumfang wird genutzt
        let ok = scene(0.55, 1.0);
        let a = auto_tone(&ok, &DevelopParams::default());
        assert!(a.exposure.abs() <= 0.5, "{a:?}");
        assert!(
            a.highlights.abs() <= 20.0 && a.shadows.abs() <= 20.0,
            "{a:?}"
        );
        assert!(
            (median_after(&ok, &a) - TARGET_MEDIAN).abs() < 0.08,
            "{}",
            median_after(&ok, &a)
        );
    }

    #[test]
    fn night_scene_is_not_turned_grey() {
        // Viel echtes Schwarz und ein paar Lichter: höchstens eine Blende heller
        let (w, h) = (300usize, 200usize);
        let image = ImageF32::from_fn(w, h, |x, y| {
            if (x / 20 + y / 20) % 9 == 0 {
                [0.6, 0.5, 0.3]
            } else {
                [0.004, 0.004, 0.006]
            }
        });
        let night = Source {
            kind: SourceKind::Raster,
            full_width: w,
            full_height: h,
            lens: None,
            image,
        };
        let a = auto_tone(&night, &DevelopParams::default());
        assert!(a.exposure <= 1.0, "{a:?}");
    }
}
