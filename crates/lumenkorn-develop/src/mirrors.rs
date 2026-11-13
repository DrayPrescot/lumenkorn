//! Logik, die `src/lib` in TypeScript nachrechnet (Kurve, HSL-Bereiche, Geraderichten,
//! Perspektive), gegen eine gemeinsame Datei `tests/mirrors.json`. Die Oberfläche prüft
//! dieselbe Datei in `tools/mirrors.test.mjs`. Ändert sich die Rechnung hier absichtlich:
//! `LUMENKORN_BLESS=1 cargo test -p lumenkorn-develop mirrors` schreibt die Datei neu,
//! danach die TypeScript-Seite anpassen, bis `npm test` grün ist.

use crate::color::hsl_weights;
use crate::curve::Spline;
use crate::geometry::{Warp, straighten_scale};
use serde_json::{Value, json};

const CURVES: &[&[[f32; 2]]] = &[
    &[[0.0, 0.0], [0.25, 0.15], [0.75, 0.85], [1.0, 1.0]],
    &[[0.0, 0.1], [0.5, 0.5], [1.0, 0.9]],
    // Steiler Anstieg und Plateau: Hier greift die Begrenzung gegen Überschwingen
    &[[0.0, 0.0], [0.1, 0.6], [0.2, 0.62], [1.0, 1.0]],
    &[[0.2, 0.3], [0.6, 0.3], [0.8, 0.9]],
];

/// (Breite, Höhe, Geraderichten, Perspektive vertikal, horizontal)
const WARPS: &[(usize, usize, f32, f32, f32)] = &[
    (6000, 4000, 3.5, 0.0, 0.0),
    (4000, 6000, -12.0, 0.0, 0.0),
    (6000, 4000, 0.0, 40.0, 0.0),
    (6000, 4000, 0.0, 0.0, -25.0),
    (3000, 2000, 7.0, -30.0, 20.0),
];

fn expected() -> Value {
    let xs: Vec<f32> = (0..=20).map(|i| i as f32 / 20.0).collect();
    let curves: Vec<Value> = CURVES
        .iter()
        .map(|pts| {
            let s = Spline::new(pts);
            json!({ "points": pts, "x": xs, "y": xs.iter().map(|&x| s.eval(x)).collect::<Vec<_>>() })
        })
        .collect();
    let hues: Vec<f32> = (0..48).map(|i| i as f32 * 7.5 + 0.3).collect();
    let hsl: Vec<Value> = hues
        .iter()
        .map(|&h| json!({ "hue": h, "weights": hsl_weights(h) }))
        .collect();
    let straighten: Vec<Value> = [
        (6000.0, 4000.0, 3.0),
        (4000.0, 6000.0, -10.0),
        (5000.0, 5000.0, 45.0),
    ]
    .iter()
    .map(|&(w, h, d)| json!({ "w": w, "h": h, "deg": d, "scale": straighten_scale(w, h, d) }))
    .collect();
    let warps: Vec<Value> = WARPS
        .iter()
        .map(|&(w, h, st, v, hz)| {
            let m = Warp::from_angles(w, h, st, v, hz).map(|w| (w.m, w.zoom));
            let (m, zoom) = m.expect("Verzerrung erwartet");
            json!({ "w": w, "h": h, "straighten": st, "vertical": v, "horizontal": hz,
                    "m": m.map(|x| x / m[8]), "zoom": zoom })
        })
        .collect();
    json!({ "curves": curves, "hsl": hsl, "straighten": straighten, "warps": warps })
}

/// Zahlen auf 6 Nachkommastellen (lesbare Datei; f32 rechnet ohnehin nicht genauer)
fn round(v: Value) -> Value {
    match v {
        Value::Number(n) => n
            .as_f64()
            .map(|x| json!((x * 1e6).round() / 1e6))
            .unwrap_or(Value::Number(n)),
        Value::Array(a) => Value::Array(a.into_iter().map(round).collect()),
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, round(v))).collect()),
        v => v,
    }
}

/// Gleich bis auf `tol` bei Zahlen; liefert den Pfad der ersten Abweichung.
fn differs(a: &Value, b: &Value, tol: f64, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            ((x.as_f64()? - y.as_f64()?).abs() > tol).then(|| format!("{path}: {x} statt {y}"))
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x
            .iter()
            .zip(y)
            .enumerate()
            .find_map(|(i, (x, y))| differs(x, y, tol, &format!("{path}[{i}]"))),
        (Value::Object(x), Value::Object(y)) if x.len() == y.len() => {
            x.iter().find_map(|(k, v)| {
                y.get(k).map_or(Some(format!("{path}.{k} fehlt")), |w| {
                    differs(v, w, tol, &format!("{path}.{k}"))
                })
            })
        }
        _ => (a != b).then(|| format!("{path}: {a} statt {b}")),
    }
}

#[test]
fn typescript_mirrors_match_rust() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/mirrors.json");
    let actual = round(expected());
    if std::env::var_os("LUMENKORN_BLESS").is_some() {
        std::fs::write(
            path,
            serde_json::to_string(&actual).unwrap()
                + "
",
        )
        .unwrap();
        return;
    }
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    if let Some(d) = differs(&actual, &stored, 1e-5, "") {
        panic!(
            "Rechnung geändert ({d}): mit LUMENKORN_BLESS=1 neu schreiben und TypeScript anpassen"
        );
    }
}
