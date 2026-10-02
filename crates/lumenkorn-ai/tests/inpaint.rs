//! KI-Reparatur mit dem echten LaMa-Modell:
//!
//!     LUMENKORN_TEST_ORT=/pfad/libonnxruntime.so LUMENKORN_TEST_LAMA=/pfad/lama.onnx \
//!     LUMENKORN_TEST_IMAGE=/pfad/foto.jpg cargo test --release -p lumenkorn-ai --test inpaint

use lumenkorn_ai::inpaint::Inpainter;
use lumenkorn_ai::runtime;
use lumenkorn_develop::ImageF32;
use std::sync::atomic::AtomicBool;

fn lin(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

#[test]
fn removes_painted_defects() {
    let (Ok(lib), Ok(model), Ok(photo)) = (
        std::env::var("LUMENKORN_TEST_ORT"),
        std::env::var("LUMENKORN_TEST_LAMA"),
        std::env::var("LUMENKORN_TEST_IMAGE"),
    ) else {
        return;
    };
    runtime::init(std::path::Path::new(&lib)).unwrap();
    let inp = Inpainter::load(std::path::Path::new(&model)).unwrap();
    // Großes Bild (doppelte Auflösung), damit die Ausschnitt-Logik greift
    let src = image::open(&photo).unwrap().to_rgb8();
    let src = image::imageops::resize(
        &src,
        src.width() * 2,
        src.height() * 2,
        image::imageops::FilterType::Triangle,
    );
    let (w, h) = (src.width() as usize, src.height() as usize);
    let clean = ImageF32::from_fn(w, h, |x, y| src.get_pixel(x as u32, y as u32).0.map(lin));

    // Störungen: weißer Fleck und schwarzer Strich
    let mut alpha = vec![0f32; w * h];
    let mut broken = clean.clone();
    let mut paint = |x: usize, y: usize, c: [f32; 3]| {
        broken.data[y * w + x] = c;
        alpha[y * w + x] = 1.0;
    };
    let (cx, cy, r) = (w / 4, h / 3, w / 40);
    for y in cy - r..cy + r {
        for x in cx - r..cx + r {
            if (x as i64 - cx as i64).pow(2) + (y as i64 - cy as i64).pow(2) < (r * r) as i64 {
                paint(x, y, [1.0; 3]);
            }
        }
    }
    for i in 0..w / 3 {
        for t in 0..4 {
            paint(w / 2 + i, h / 2 + i / 3 + t, [0.0; 3]);
        }
    }

    let t = std::time::Instant::now();
    let patch = inp
        .repair(
            &broken,
            &alpha,
            &|a, b| eprint!("\rBereich {a}/{b} "),
            &AtomicBool::new(false),
        )
        .unwrap();
    eprintln!(
        "\n{} Bereiche repariert in {:.2?} ({w}×{h})",
        patch.parts.len(),
        t.elapsed()
    );
    assert_eq!(patch.parts.len(), 2);
    let mut fixed = broken.clone();
    patch.apply(&mut fixed);

    // Fehler im Schadensbereich: vorher (kaputt) gegen nachher (repariert)
    let err = |img: &ImageF32| {
        let (mut s, mut n) = (0f64, 0f64);
        for (i, a) in alpha.iter().enumerate() {
            if *a > 0.0 {
                for c in 0..3 {
                    s += ((img.data[i][c] - clean.data[i][c]) as f64).abs();
                }
                n += 3.0;
            }
        }
        s / n
    };
    let (before, after) = (err(&broken), err(&fixed));
    eprintln!("mittlere Abweichung zum Original im Schaden: {before:.4} → {after:.4}");
    assert!(
        after < before * 0.3,
        "zu wenig repariert: {before} → {after}"
    );
    // Außerhalb der (erweiterten) Auswahl bleibt alles unverändert
    let untouched =
        fixed.get(5, 5) == broken.get(5, 5) && fixed.get(w - 5, h - 5) == broken.get(w - 5, h - 5);
    assert!(untouched);

    if let Ok(o) = std::env::var("LUMENKORN_TEST_OUT") {
        let enc = |v: f32| {
            (if v <= 0.0031308 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            } * 255.0)
                .clamp(0.0, 255.0) as u8
        };
        let mut out = image::RgbImage::new(w as u32 * 2, h as u32);
        for y in 0..h {
            for x in 0..w {
                out.put_pixel(x as u32, y as u32, image::Rgb(broken.get(x, y).map(enc)));
                out.put_pixel(
                    (x + w) as u32,
                    y as u32,
                    image::Rgb(fixed.get(x, y).map(enc)),
                );
            }
        }
        out.save(std::path::Path::new(&o).join("reparatur.jpg"))
            .unwrap();
    }
}
