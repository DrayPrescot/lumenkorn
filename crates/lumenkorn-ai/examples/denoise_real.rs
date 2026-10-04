//! Rauschreduzierung an einem echten (RAW-)Foto ausprobieren und vergleichen:
//!
//!     LUMENKORN_TEST_ORT=/pfad/libonnxruntime.so cargo run --release -p lumenkorn-ai \
//!         --example denoise_real -- <drunet.onnx> <foto> <ausgabe.jpg> [stärke …]
//!
//! Schneidet einen Ausschnitt aus der Bildmitte, entrauscht ihn mit den
//! angegebenen Stärken (Reglerwerte 0–100, Standard: 0 50 100) und legt
//! Original und Ergebnisse entwickelt nebeneinander.

use lumenkorn_ai::denoise::{Denoiser, Rgb, strength_factor};
use lumenkorn_ai::runtime;
use lumenkorn_develop::{DevelopParams, ImageF32, Source, render};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn main() {
    let lib = std::env::var("LUMENKORN_TEST_ORT").expect("LUMENKORN_TEST_ORT fehlt");
    let mut args = std::env::args().skip(1);
    let model = PathBuf::from(args.next().expect("Modell fehlt"));
    let input = PathBuf::from(args.next().expect("Foto fehlt"));
    let output = PathBuf::from(args.next().expect("Ausgabe fehlt"));
    let levels: Vec<f32> = {
        let v: Vec<f32> = args.filter_map(|a| a.parse().ok()).collect();
        if v.is_empty() {
            vec![0.0, 50.0, 100.0]
        } else {
            v
        }
    };
    runtime::init(std::path::Path::new(&lib)).unwrap();
    let d = Denoiser::load(&model).unwrap();

    let raw = !matches!(
        input
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "jpg" | "jpeg" | "png" | "tif" | "tiff"
    );
    let src = Source::load(&input, raw, None, None).unwrap();
    let (cw, ch) = (640usize, 480usize);
    let (x0, y0) = ((src.image.width - cw) / 2, (src.image.height - ch) / 2);
    let crop = ImageF32::from_fn(cw, ch, |x, y| src.image.get(x0 + x, y0 + y));

    let params: DevelopParams = std::env::var("PARAMS")
        .ok()
        .map(|j| serde_json::from_str(&j).unwrap())
        .unwrap_or_default();
    let develop = |img: &ImageF32| {
        let s = Source {
            kind: src.kind,
            image: img.clone(),
            full_width: img.width,
            full_height: img.height,
        };
        render(&s, &params)
    };

    {
        // Geschätzte Rauschkurve (σ×255 im kodierten Raum je Helligkeitsstufe)
        let gain = 1.0
            / crop
                .data
                .iter()
                .map(|p| p[0].max(p[1]).max(p[2]))
                .fold(1.0f32, f32::max);
        let enc: Vec<[f32; 3]> = crop
            .data
            .iter()
            .map(|p| {
                p.map(|v| {
                    let v = (v * gain).clamp(0.0, 1.0);
                    if v <= 0.0031308 {
                        v * 12.92
                    } else {
                        1.055 * v.powf(1.0 / 2.4) - 0.055
                    }
                })
            })
            .collect();
        let c = lumenkorn_ai::denoise::estimate_noise(&enc, cw, ch);
        eprintln!(
            "Schätzung σ×255: {:?}",
            c.map(|v| (v * 2550.0).round() / 10.0)
        );
        // Direkt gemessen: Abweichung vom 5×5-Mittel (enthält auch korreliertes Rauschen)
        let mut bins = vec![Vec::new(); 8];
        for y in 2..ch - 2 {
            for x in 2..cw - 2 {
                for c in 0..3 {
                    let mut m = 0.0;
                    for dy in -2i32..=2 {
                        for dx in -2i32..=2 {
                            m += enc[(y as i32 + dy) as usize * cw + (x as i32 + dx) as usize][c];
                        }
                    }
                    let d = enc[y * cw + x][c] - m / 25.0;
                    let b = ((enc[y * cw + x][1] * 8.0) as usize).min(7);
                    bins[b].push(d.abs());
                }
            }
        }
        let direct: Vec<f32> = bins
            .iter_mut()
            .map(|v| {
                if v.len() < 500 {
                    return f32::NAN;
                }
                let mid = v.len() / 2;
                let m = *v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1;
                (1.4826 * m * 255.0 * 10.0 * 25.0f32.sqrt() / 24.0f32.sqrt()).round() / 10.0
            })
            .collect();
        eprintln!("Direkt σ×255 (inkl. korreliert, grob): {direct:?}");
    }
    let mut panels = vec![develop(&crop)];
    let cancel = AtomicBool::new(false);
    for &l in &levels {
        let t = std::time::Instant::now();
        let out = d
            .denoise(
                &Rgb {
                    width: cw,
                    height: ch,
                    data: &crop.data,
                },
                strength_factor(l),
                &|_, _| {},
                &cancel,
            )
            .unwrap();
        eprintln!("Stärke {l}: {:.1?}", t.elapsed());
        panels.push(develop(&ImageF32 {
            width: cw,
            height: ch,
            data: out,
        }));
    }
    // Nebeneinander, als JPEG
    let n = panels.len();
    let mut canvas = image::RgbImage::new((cw * n) as u32, ch as u32);
    for (k, p) in panels.iter().enumerate() {
        for y in 0..ch {
            for x in 0..cw {
                let v = p.get(x, y).map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                canvas.put_pixel((k * cw + x) as u32, y as u32, image::Rgb(v));
            }
        }
    }
    canvas.save(&output).unwrap();
    eprintln!("gespeichert: {}", output.display());
}
