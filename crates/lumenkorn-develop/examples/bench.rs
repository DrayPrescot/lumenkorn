//! Misst die Entwicklungspipeline auf CPU und GPU.
//!
//!     cargo run --release -p lumenkorn-develop --example bench
//!     cargo run --release -p lumenkorn-develop --example bench -- 6000 4000   # Exportgröße
//!
//! Ohne echte Grafikkarte: `LUMENKORN_GPU_SOFTWARE=1` (nur zur Kontrolle, nicht schnell).

use lumenkorn_develop::gpu::{self, Rendered};
use lumenkorn_develop::{
    DevelopParams, ImageF32, LocalAdjust, MaskLayer, MaskShape, RenderOptions, Source, SourceKind,
    render_with,
};
use std::time::Instant;

fn main() {
    let args: Vec<usize> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let (w, h) = (
        *args.first().unwrap_or(&2048),
        *args.get(1).unwrap_or(&1365),
    );
    let src = Source {
        kind: SourceKind::Raw,
        full_width: w,
        full_height: h,
        lens: None,
        as_shot_kelvin: None,
        image: ImageF32::from_fn(w, h, |x, y| {
            let n = ((x * 7 + y * 13) % 17) as f32 / 170.0;
            [x as f32 / w as f32 + n, 0.3 + n, y as f32 / h as f32]
        }),
    };
    let layer = |shape| MaskLayer {
        shapes: vec![shape],
        adjust: LocalAdjust {
            exposure: 0.5,
            clarity: 30.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let cases = [
        (
            "Belichtung",
            DevelopParams {
                exposure: 0.5,
                ..Default::default()
            },
        ),
        (
            "Lichter/Tiefen/Klarheit",
            DevelopParams {
                highlights: -50.0,
                shadows: 40.0,
                clarity: 30.0,
                ..Default::default()
            },
        ),
        (
            "+ 3 Maskenebenen + Schärfen",
            DevelopParams {
                highlights: -50.0,
                shadows: 40.0,
                clarity: 30.0,
                sharpen: 50.0,
                masks: (0..3)
                    .map(|i| {
                        layer(MaskShape::Radial {
                            cx: 0.2 + 0.3 * i as f32,
                            cy: 0.5,
                            rx: 0.2,
                            ry: 0.2,
                            angle: 0.0,
                            feather: 0.5,
                            invert: false,
                            subtract: false,
                        })
                    })
                    .collect(),
                ..Default::default()
            },
        ),
    ];
    let opts = RenderOptions::default();
    let g = match gpu::init() {
        Ok(g) => {
            println!("GPU: {} ({}, {})", g.info.name, g.info.backend, g.info.kind);
            Some(g)
        }
        Err(e) => {
            println!("Keine GPU: {e}");
            None
        }
    };
    println!("Bild {w}×{h} ({:.1} MP)\n", (w * h) as f64 / 1e6);
    println!("{:<30} {:>10} {:>10}", "", "CPU", "GPU");
    for (name, p) in &cases {
        // Erster Lauf: Masken berechnen, Puffer anlegen, Bild hochladen
        let _ = render_with(&src, p, &opts);
        let t = Instant::now();
        for _ in 0..3 {
            let _ = render_with(&src, p, &opts);
        }
        let cpu = t.elapsed().as_secs_f64() / 3.0;
        let gpu = g.map(|g| {
            let _ = g.render(&src, p, &opts, true);
            let t = Instant::now();
            for _ in 0..3 {
                match g.render(&src, p, &opts, true) {
                    Ok(Rendered::Rgb8(_)) => {}
                    Ok(_) => unreachable!(),
                    Err(e) => panic!("{e}"),
                }
            }
            t.elapsed().as_secs_f64() / 3.0
        });
        println!(
            "{name:<30} {:>8.0}ms {:>10}",
            cpu * 1000.0,
            gpu.map(|s| format!("{:.0}ms", s * 1000.0))
                .unwrap_or_else(|| "–".into())
        );
    }
}
