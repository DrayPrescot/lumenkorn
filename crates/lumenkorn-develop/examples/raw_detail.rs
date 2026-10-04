//! 1:1-Ausschnitte eines RAWs mit verschiedenen Verfahren zum Vergleichen:
//! PPG (bisher), RCD, RCD + Eingangsschärfung – jeweils mit Standard-Entwicklung.
//!
//!     cargo run --release -p lumenkorn-develop --example raw_detail -- <raw> <ausgabe-präfix>
//!
//! Wählt den Ausschnitt mit den meisten Details und gibt seine Lage aus.

use lumenkorn_develop::source::{DemosaicMethod, Source, SourceKind, decode_raw_with};
use lumenkorn_develop::{DevelopParams, ImageF32, render};
use std::path::Path;

const CW: usize = 360;
const CH: usize = 240;

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("RAW fehlt");
    let prefix = args.next().expect("Präfix fehlt");
    let (ppg, _) = decode_raw_with(Path::new(&input), DemosaicMethod::Ppg).unwrap();
    let (rcd, _) = decode_raw_with(Path::new(&input), DemosaicMethod::Rcd).unwrap();
    let mut cs = rcd.clone();
    let t = std::time::Instant::now();
    lumenkorn_develop::capture_sharpen::apply(&mut cs);
    eprintln!(
        "Eingangsschärfung: {:.0?} für {}×{}",
        t.elapsed(),
        cs.width,
        cs.height
    );
    // Varianten zum Abstimmen: VARIANTS=1.2/0.02,1.6/0.03
    let variants: Vec<(String, ImageF32)> = std::env::var("VARIANTS")
        .unwrap_or_default()
        .split(',')
        .filter(|v| !v.is_empty())
        .map(|v| {
            let (a, o) = v.split_once('/').unwrap();
            let mut img = rcd.clone();
            lumenkorn_develop::capture_sharpen::apply_with(
                &mut img,
                a.parse().unwrap(),
                o.parse().unwrap(),
            );
            (format!("a{a}_o{o}"), img)
        })
        .collect();

    // Ausschnitt mit der größten Gradientenenergie (Grünkanal)
    let (w, h) = (rcd.width, rcd.height);
    let mut best = (0.0f64, 0, 0);
    for by in (0..h - CH).step_by(CH / 2) {
        for bx in (0..w - CW).step_by(CW / 2) {
            let mut e = 0f64;
            for y in (by + 1..by + CH).step_by(2) {
                for x in (bx + 1..bx + CW).step_by(2) {
                    let g = rcd.get(x, y)[1].sqrt();
                    e += ((g - rcd.get(x - 1, y)[1].sqrt()).abs()
                        + (g - rcd.get(x, y - 1)[1].sqrt()).abs()) as f64;
                }
            }
            if e > best.0 {
                best = (e, bx, by);
            }
        }
    }
    let (_, mut x0, mut y0) = best;
    // Optional: Lage als Bruchteil vorgeben (`AT=0.5,0.3` = Mitte des Ausschnitts)
    if let Ok(at) = std::env::var("AT") {
        let f: Vec<f32> = at.split(',').map(|v| v.parse().unwrap()).collect();
        x0 = ((f[0] * w as f32) as usize)
            .saturating_sub(CW / 2)
            .min(w - CW);
        y0 = ((f[1] * h as f32) as usize)
            .saturating_sub(CH / 2)
            .min(h - CH);
    }
    println!("{} {} {} {}", x0, y0, w, h);
    let params: DevelopParams = std::env::var("PARAMS")
        .ok()
        .map(|j| serde_json::from_str(&j).unwrap())
        .unwrap_or_default();
    let mut all: Vec<(String, &ImageF32)> = vec![
        ("ppg".into(), &ppg),
        ("rcd".into(), &rcd),
        ("rcd_cs".into(), &cs),
    ];
    all.extend(variants.iter().map(|(n, i)| (n.clone(), i)));
    for (name, img) in all {
        // Ganzes Bild entwickeln (lokale Operatoren brauchen den Zusammenhang),
        // dann ausschneiden
        let src = Source {
            kind: SourceKind::Raw,
            image: img.clone(),
            full_width: w,
            full_height: h,
        };
        let out: ImageF32 = render(&src, &params);
        let mut canvas = image::RgbImage::new(CW as u32, CH as u32);
        for y in 0..CH {
            for x in 0..CW {
                let p = out.get(x0 + x, y0 + y);
                canvas.put_pixel(
                    x as u32,
                    y as u32,
                    image::Rgb(p.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)),
                );
            }
        }
        canvas.save(format!("{prefix}_{name}.png")).unwrap();
    }
}
