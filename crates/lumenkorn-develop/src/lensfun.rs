//! Objektivprofile aus der Lensfun-Datenbank (<https://lensfun.github.io/>,
//! Daten unter CC BY-SA 3.0, siehe `data/LICENSE-lensfun-db.txt`).
//!
//! Die Datenbank liegt als kompaktes JSON bei (`tools/lensfun_db.py` erzeugt
//! es aus den XML-Dateien) und wird beim ersten Bedarf gelesen. Die Modelle
//! sind hier unabhängig von der Lensfun-Bibliothek umgesetzt:
//!
//! - **Verzeichnung** (`poly3`, `poly5`, `ptlens`) und **Farbsäume**
//!   (`linear`, `poly3`) in Hugin-Koordinaten: r = 1 ist die halbe kürzere
//!   Seite des Sensors, mit dem kalibriert wurde. Das Ausgabepixel im Abstand
//!   r holt seinen Wert aus `r · P(r)`.
//! - **Vignettierung** (`pa`): r = 1 ist die halbe Diagonale des
//!   Kalibriersensors; das Pixel wird durch `1 + k1 r² + k2 r⁴ + k3 r⁶` geteilt.
//!
//! Ein anderer Sensor (Crop-Faktor) wird über die Abstände in Millimetern auf
//! der Bildebene umgerechnet. Zwischen kalibrierten Brennweiten und Blenden
//! wird linear interpoliert (Blende in Stufen).

use serde::Deserialize;
use std::sync::OnceLock;

/// Diagonale des Kleinbildformats in mm
const FF_DIAGONAL: f64 = 43.266_615;

#[derive(Deserialize)]
struct Db {
    mounts: std::collections::HashMap<String, Vec<String>>,
    cameras: Vec<Camera>,
    lenses: Vec<Lens>,
}

#[derive(Deserialize)]
struct Camera {
    maker: Vec<String>,
    model: Vec<String>,
    mount: String,
    crop: f64,
}

#[derive(Deserialize)]
struct Lens {
    maker: Vec<String>,
    model: Vec<String>,
    mount: Vec<String>,
    crop: f64,
    aspect: f64,
    center: [f64; 2],
    /// [Brennweite, Modell, Koeffizienten …]
    dist: Vec<Vec<serde_json::Value>>,
    tca: Vec<Vec<serde_json::Value>>,
    /// [Brennweite, Blende, Entfernung, k1, k2, k3]
    vig: Vec<[f64; 6]>,
}

fn db() -> Option<&'static Db> {
    static DB: OnceLock<Option<Db>> = OnceLock::new();
    DB.get_or_init(|| {
        let gz: &[u8] = include_bytes!("../data/lensfun.json.gz");
        let mut json = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(gz), &mut json).ok()?;
        match serde_json::from_slice::<Db>(&json) {
            Ok(db) => Some(db),
            Err(e) => {
                log::error!("Lensfun-Datenbank unlesbar: {e}");
                None
            }
        }
    })
    .as_ref()
}

/// Was ein Foto über Kamera und Objektiv verrät.
#[derive(Debug, Clone, Default)]
pub struct Shot {
    pub camera_make: String,
    pub camera_model: String,
    pub lens_make: Option<String>,
    pub lens_model: Option<String>,
    /// mm
    pub focal: Option<f64>,
    /// Blendenzahl
    pub aperture: Option<f64>,
}

/// Wörter und Zahlen eines Objektiv- oder Kameranamens, kleingeschrieben;
/// Buchstaben und Ziffern werden getrennt („EF24-70mm“ → ef, 24, 70, mm).
fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut digit = false;
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        let t = std::mem::take(cur);
        let t = t.trim_end_matches('.').to_string();
        // „f“ und „mm“ sind nur Einheiten
        if !t.is_empty() && t != "f" && t != "mm" {
            out.push(t);
        }
    };
    for ch in s.chars().flat_map(char::to_lowercase) {
        let is_digit = ch.is_ascii_digit() || (ch == '.' && digit);
        if ch.is_alphanumeric() || (ch == '.' && digit) {
            if !cur.is_empty() && is_digit != digit {
                flush(&mut cur, &mut out);
            }
            digit = is_digit;
            cur.push(ch);
        } else {
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    // „1.80“ und „1.8“ gleich behandeln
    out.into_iter()
        .map(|t| match t.parse::<f64>() {
            Ok(v) if t.contains('.') => format!("{v}"),
            _ => t,
        })
        .collect()
}

fn same_name(a: &str, b: &str) -> bool {
    tokens(a) == tokens(b)
}

fn camera<'a>(db: &'a Db, shot: &Shot) -> Option<&'a Camera> {
    let make = tokens(&shot.camera_make);
    let first = make.first().cloned().unwrap_or_default();
    let candidates: Vec<&Camera> = db
        .cameras
        .iter()
        .filter(|c| c.model.iter().any(|m| same_name(m, &shot.camera_model)))
        .collect();
    candidates
        .iter()
        .find(|c| c.maker.iter().any(|m| tokens(m).first() == Some(&first)))
        .or(candidates.first())
        .copied()
}

/// Bestes Objektiv zum EXIF-Namen: alle Wörter und Zahlen des EXIF-Namens
/// müssen im Lensfun-Namen vorkommen (dort steht meist der Hersteller
/// zusätzlich davor); bei mehreren gewinnt der mit den wenigsten
/// zusätzlichen Wörtern, dann der mit passendem Bajonett.
fn lens<'a>(db: &'a Db, shot: &Shot, cam: Option<&Camera>) -> Option<(&'a Lens, &'a str)> {
    let name = shot.lens_model.as_deref()?.trim();
    if name.is_empty() || name.eq_ignore_ascii_case("unknown") {
        return None;
    }
    let mut want = tokens(name);
    // Herstellername im EXIF-Namen ist kein Unterscheidungsmerkmal
    if let Some(m) = &shot.lens_make {
        let mt = tokens(m);
        want.retain(|t| !mt.contains(t));
    }
    if want.iter().all(|t| t.parse::<f64>().is_err()) {
        return None; // ohne Brennweite im Namen zu unsicher
    }
    let mounts: Vec<&str> = cam
        .map(|c| {
            let mut v = vec![c.mount.as_str()];
            if let Some(compat) = db.mounts.get(&c.mount) {
                v.extend(compat.iter().map(String::as_str));
            }
            v
        })
        .unwrap_or_default();
    let mut best: Option<(i64, &Lens, &str)> = None;
    for l in &db.lenses {
        for m in &l.model {
            let have = tokens(m);
            if !want.iter().all(|t| have.contains(t)) {
                continue;
            }
            let maker: Vec<String> = l.maker.iter().flat_map(|m| tokens(m)).collect();
            let extra = have
                .iter()
                .filter(|t| !want.contains(t) && !maker.contains(t))
                .count() as i64;
            let mount_ok =
                mounts.is_empty() || l.mount.iter().any(|lm| mounts.contains(&lm.as_str()));
            let score = extra * 2 + if mount_ok { 0 } else { 5 };
            if best.as_ref().is_none_or(|b| score < b.0) {
                best = Some((score, l, m.as_str()));
            }
        }
    }
    // Brennweite muss in den Bereich der Kalibrierung passen
    let (_, l, n) = best?;
    if let Some(f) = shot.focal {
        let fs: Vec<f64> = l
            .dist
            .iter()
            .chain(&l.tca)
            .filter_map(|e| e.first().and_then(|v| v.as_f64()))
            .chain(l.vig.iter().map(|v| v[0]))
            .collect();
        let (lo, hi) = fs
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        if !fs.is_empty() && (f < lo * 0.8 || f > hi * 1.25) {
            return None;
        }
    }
    Some((l, n))
}

/// Linear zwischen den beiden Einträgen um `f` (sortiert nach Brennweite).
fn around<T>(entries: &[(f64, T)], f: f64) -> Option<(&T, &T, f64)> {
    let first = entries.first()?;
    if f <= first.0 || entries.len() == 1 {
        return Some((&first.1, &first.1, 0.0));
    }
    for w in entries.windows(2) {
        if f <= w[1].0 {
            let t = (f - w[0].0) / (w[1].0 - w[0].0).max(1e-9);
            return Some((&w[0].1, &w[1].1, t));
        }
    }
    let last = entries.last()?;
    Some((&last.1, &last.1, 0.0))
}

fn coeffs(v: &[serde_json::Value]) -> Vec<f64> {
    v.iter().skip(2).filter_map(|x| x.as_f64()).collect()
}

/// Verzeichnungs-Faktor P(r) in Hugin-Einheiten.
fn dist_factor(model: &str, k: &[f64], r: f64) -> f64 {
    let g = |i: usize| k.get(i).copied().unwrap_or(0.0);
    let r2 = r * r;
    match model {
        "poly3" => 1.0 - g(0) + g(0) * r2,
        "poly5" => 1.0 + g(0) * r2 + g(1) * r2 * r2,
        "ptlens" => g(0) * r2 * r + g(1) * r2 + g(2) * r + 1.0 - g(0) - g(1) - g(2),
        _ => 1.0,
    }
}

/// Faktor für Rot und Blau relativ zum Grün; `r` = verzeichneter Radius.
fn tca_factor(model: &str, k: &[f64], r: f64) -> (f64, f64) {
    let g = |i: usize, d: f64| k.get(i).copied().unwrap_or(d);
    match model {
        "linear" => (g(0, 1.0), g(1, 1.0)),
        "poly3" => (
            g(0, 1.0) + g(1, 0.0) * r + g(2, 0.0) * r * r,
            g(3, 1.0) + g(4, 0.0) * r + g(5, 0.0) * r * r,
        ),
        _ => (1.0, 1.0),
    }
}

/// Vignettierung einer Brennweite: (Blende in Stufen, k1…k3), sortiert
type VigByAperture = Vec<(f64, [f64; 3])>;

/// Lensfun-Korrektur, umgerechnet auf Lumenkorns Form (siehe `lens.rs`):
/// Radien normiert auf die halbe Bilddiagonale.
pub(crate) struct Profile {
    pub name: String,
    /// Optische Mitte (normiert, aufrecht ungedrehte Lage wird vom Aufrufer
    /// gedreht)
    pub center: (f32, f32),
    pub knots: Vec<f32>,
    pub factor: Option<[Vec<f32>; 3]>,
    pub chromatic: bool,
    pub vig_gain: Option<Vec<f32>>,
}

/// Sucht das Objektiv und berechnet die Korrektur für Brennweite und Blende
/// des Fotos. `w`, `h`: Bildgröße (unrotiert).
pub(crate) fn profile(shot: &Shot, w: usize, h: usize) -> Option<Profile> {
    const N: usize = 64;
    let db = db()?;
    let cam = camera(db, shot);
    let (lens, name) = lens(db, shot, cam)?;
    let focal = shot.focal.filter(|f| *f > 0.0)?;
    // Sensorgrößen in mm
    let crop = cam.map_or(lens.crop, |c| c.crop);
    let half_diag = FF_DIAGONAL / crop / 2.0;
    let hugin = FF_DIAGONAL / lens.crop / (lens.aspect.max(1.0)).hypot(1.0) / 2.0;
    let half_diag_cal = FF_DIAGONAL / lens.crop / 2.0;
    let knots: Vec<f32> = (0..N).map(|i| i as f32 / (N - 1) as f32).collect();

    // Verzeichnung
    let mut dist: Vec<(f64, (&str, Vec<f64>))> = lens
        .dist
        .iter()
        .filter_map(|e| Some((e.first()?.as_f64()?, (e.get(1)?.as_str()?, coeffs(e)))))
        .collect();
    dist.sort_by(|a, b| a.0.total_cmp(&b.0));
    let d_at = |r: f64| -> f64 {
        match around(&dist, focal) {
            Some((a, b, t)) if a.0 == b.0 => {
                let k: Vec<f64> = a.1.iter().zip(&b.1).map(|(x, y)| x + (y - x) * t).collect();
                dist_factor(a.0, &k, r)
            }
            Some((a, b, t)) => {
                // verschiedene Modelle: Faktoren mischen
                dist_factor(a.0, &a.1, r) * (1.0 - t) + dist_factor(b.0, &b.1, r) * t
            }
            None => 1.0,
        }
    };
    let mut tca: Vec<(f64, (&str, Vec<f64>))> = lens
        .tca
        .iter()
        .filter_map(|e| Some((e.first()?.as_f64()?, (e.get(1)?.as_str()?, coeffs(e)))))
        .collect();
    tca.sort_by(|a, b| a.0.total_cmp(&b.0));
    let t_at = |r: f64| -> (f64, f64) {
        match around(&tca, focal) {
            Some((a, b, t)) => {
                let (ar, ab) = tca_factor(a.0, &a.1, r);
                let (br, bb) = tca_factor(b.0, &b.1, r);
                (ar + (br - ar) * t, ab + (bb - ab) * t)
            }
            None => (1.0, 1.0),
        }
    };
    let factor = (!dist.is_empty() || !tca.is_empty()).then(|| {
        let mut f = [vec![1.0f32; N], vec![1.0f32; N], vec![1.0f32; N]];
        for (i, r) in knots.iter().enumerate() {
            let rh = *r as f64 * half_diag / hugin;
            let p = d_at(rh);
            let (kr, kb) = t_at(rh * p);
            f[0][i] = (p * kr) as f32;
            f[1][i] = p as f32;
            f[2][i] = (p * kb) as f32;
        }
        f
    });

    // Vignettierung: Einträge der größten Entfernung, Blende in Stufen
    let vig_gain = shot.aperture.filter(|a| *a > 0.0).and_then(|ap| {
        let far = lens.vig.iter().map(|v| v[2]).fold(f64::MIN, f64::max);
        let mut by_focal: Vec<(f64, VigByAperture)> = Vec::new();
        for v in lens.vig.iter().filter(|v| v[2] == far) {
            let e = (v[1].log2(), [v[3], v[4], v[5]]);
            match by_focal.iter_mut().find(|(f, _)| *f == v[0]) {
                Some((_, l)) => l.push(e),
                None => by_focal.push((v[0], vec![e])),
            }
        }
        if by_focal.is_empty() {
            return None;
        }
        by_focal.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, l) in by_focal.iter_mut() {
            l.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        let at_focal = |list: &VigByAperture, rv: f64| -> f64 {
            let gain = |k: &[f64; 3]| {
                let r2 = rv * rv;
                1.0 / (1.0 + r2 * (k[0] + r2 * (k[1] + r2 * k[2]))).max(0.1)
            };
            match around(list, ap.log2()) {
                Some((a, b, t)) => gain(a) + (gain(b) - gain(a)) * t,
                None => 1.0,
            }
        };
        Some(
            knots
                .iter()
                .map(|r| {
                    let rv = *r as f64 * half_diag / half_diag_cal;
                    let g = match around(&by_focal, focal) {
                        Some((a, b, t)) => at_focal(a, rv) * (1.0 - t) + at_focal(b, rv) * t,
                        None => 1.0,
                    };
                    g as f32
                })
                .collect::<Vec<f32>>(),
        )
    });
    if factor.is_none() && vig_gain.is_none() {
        return None;
    }
    // Dezentrierung: Einheit = halbe kürzere Bildseite
    let s = w.min(h) as f64 / 2.0;
    let center = (
        (0.5 + lens.center[0] * s / w.max(1) as f64) as f32,
        (0.5 + lens.center[1] * s / h.max(1) as f64) as f32,
    );
    let chromatic = !tca.is_empty();
    let name = lens
        .maker
        .first()
        .filter(|m| !tokens(name).starts_with(&tokens(m)))
        .map_or(name.to_string(), |m| format!("{m} {name}"));
    Some(Profile {
        name,
        center,
        knots,
        factor,
        chromatic,
        vig_gain,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(make: &str, model: &str, lens: &str, focal: f64, ap: f64) -> Shot {
        Shot {
            camera_make: make.into(),
            camera_model: model.into(),
            lens_make: None,
            lens_model: Some(lens.into()),
            focal: Some(focal),
            aperture: Some(ap),
        }
    }

    #[test]
    fn tokens_split_letters_and_digits() {
        assert_eq!(
            tokens("EF24-70mm f/2.8L II USM"),
            ["ef", "24", "70", "2.8", "l", "ii", "usm"]
        );
        assert_eq!(tokens("DT 35mm F1.8 SAM"), ["dt", "35", "1.8", "sam"]);
        assert_eq!(tokens("1.80"), ["1.8"]);
    }

    #[test]
    fn finds_the_sony_dt_35_on_an_alpha_68() {
        let db = db().expect("Datenbank lesbar");
        assert!(db.lenses.len() > 1000 && db.cameras.len() > 500);
        let s = shot("SONY", "ILCA-68", "DT 35mm F1.8 SAM", 35.0, 4.0);
        let cam = camera(db, &s).expect("Kamera");
        assert!((cam.crop - 1.534).abs() < 1e-3);
        let (_, name) = lens(db, &s, Some(cam)).expect("Objektiv");
        assert!(name.contains("DT 35mm f/1.8 SAM"), "{name}");
        let p = profile(&s, 6000, 4000).unwrap();
        // Tonnenförmig wie in den Sony-Daten (−1,75 % an der Ecke)
        let f = p.factor.as_ref().unwrap();
        assert!(
            (f[1][63] / f[1][0] - 0.980).abs() < 0.003,
            "{}",
            f[1][63] / f[1][0]
        );
        // Vignettierung bei f/4: Ecke ~ +13 %
        let g = p.vig_gain.unwrap();
        assert!(
            (g[63] - 1.13).abs() < 0.02 && (g[0] - 1.0).abs() < 1e-6,
            "{}",
            g[63]
        );
    }

    #[test]
    fn unknown_or_vague_lenses_are_not_guessed() {
        let db = db().unwrap();
        assert!(lens(db, &shot("SONY", "ILCA-68", "Unknown", 35.0, 4.0), None).is_none());
        assert!(lens(db, &shot("SONY", "ILCA-68", "Super Lens", 35.0, 4.0), None).is_none());
        // Brennweite weit außerhalb der Kalibrierung
        let s = shot("SONY", "ILCA-68", "DT 35mm F1.8 SAM", 200.0, 4.0);
        assert!(lens(db, &s, None).is_none());
    }

    #[test]
    fn zoom_lenses_interpolate_between_focal_lengths() {
        let db = db().unwrap();
        let s = shot(
            "Canon",
            "Canon EOS 5D Mark II",
            "EF24-105mm f/4L IS USM",
            24.0,
            4.0,
        );
        let (l, name) = lens(db, &s, camera(db, &s)).expect("EF 24-105");
        assert!(name.contains("24-105"), "{name}");
        assert!(l.dist.len() > 2);
        let wide = profile(&s, 5616, 3744).unwrap().factor.unwrap()[1][63];
        let tele = profile(
            &Shot {
                focal: Some(105.0),
                ..s.clone()
            },
            5616,
            3744,
        )
        .unwrap()
        .factor
        .unwrap()[1][63];
        assert!(
            wide < 1.0 && tele > wide,
            "Weitwinkel tonnen-, Tele kissenförmiger: {wide} {tele}"
        );
    }
}
