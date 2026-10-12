//! Tests mit echten Segmentierungsmodellen:
//!
//!     LUMENKORN_TEST_ORT=/pfad/libonnxruntime.so LUMENKORN_TEST_SEG=/pfad cargo test -p lumenkorn-ai --test segment
//!
//! `LUMENKORN_TEST_SEG` enthält subject.onnx (isnet-general-use),
//! sam-encoder.onnx und sam-decoder.onnx (vit_b, quantisiert).

use image::{DynamicImage, Rgb, RgbImage};
use lumenkorn_ai::runtime;
use lumenkorn_ai::segment::{Point, Sam, SubjectModel, sky_mask};
use std::path::PathBuf;

fn setup() -> Option<PathBuf> {
    let lib = std::env::var("LUMENKORN_TEST_ORT").ok()?;
    let dir = PathBuf::from(std::env::var("LUMENKORN_TEST_SEG").ok()?);
    runtime::init(std::path::Path::new(&lib)).unwrap();
    Some(dir)
}

/// Roter Ball auf grauem, leicht strukturiertem Grund
fn ball() -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(640, 480, |x, y| {
        let d = ((x as f32 - 400.0).powi(2) + (y as f32 - 240.0).powi(2)).sqrt();
        if d < 110.0 {
            let shade = (1.0 - d / 220.0) * 255.0;
            Rgb([shade as u8, (shade * 0.15) as u8, (shade * 0.1) as u8])
        } else {
            let n = ((x * 31 + y * 17) % 11) as u8;
            Rgb([150 + n, 150 + n, 155 + n])
        }
    }))
}

#[test]
fn subject_finds_the_ball() {
    let Some(dir) = setup() else {
        eprintln!("übersprungen: LUMENKORN_TEST_ORT/LUMENKORN_TEST_SEG fehlen");
        return;
    };
    let m = SubjectModel::load(&dir.join("subject.onnx")).unwrap();
    let t = std::time::Instant::now();
    let mask = m.predict(&ball()).unwrap();
    eprintln!(
        "Motiv in {:.2?}, {}×{}",
        t.elapsed(),
        mask.width(),
        mask.height()
    );
    let (w, h) = mask.dimensions();
    let at = |x: f32, y: f32| mask.get_pixel((x * w as f32) as u32, (y * h as f32) as u32)[0];
    assert!(
        at(400.0 / 640.0, 0.5) > 128,
        "Ballmitte: {}",
        at(0.625, 0.5)
    );
    assert!(at(0.05, 0.05) < 64, "Hintergrund: {}", at(0.05, 0.05));
}

#[test]
fn sam_segments_clicked_object_and_refines_sky() {
    let Some(dir) = setup() else {
        eprintln!("übersprungen: LUMENKORN_TEST_ORT/LUMENKORN_TEST_SEG fehlen");
        return;
    };
    let sam = Sam::load(&dir.join("sam-encoder.onnx"), &dir.join("sam-decoder.onnx")).unwrap();
    let img = ball();
    let t = std::time::Instant::now();
    let emb = sam.embed(&img).unwrap();
    eprintln!("SAM-Embedding in {:.2?}", t.elapsed());

    let t = std::time::Instant::now();
    let mask = sam
        .predict(
            &emb,
            &[Point {
                x: 400.0 / 640.0,
                y: 0.5,
                positive: true,
            }],
        )
        .unwrap();
    eprintln!("SAM-Maske in {:.2?}", t.elapsed());
    let (w, h) = mask.dimensions();
    let at = |x: f32, y: f32| mask.get_pixel((x * w as f32) as u32, (y * h as f32) as u32)[0];
    assert!(at(0.625, 0.5) > 200, "geklickter Ball");
    assert!(at(0.1, 0.1) < 30, "nicht der Hintergrund");
    let area = mask.pixels().filter(|p| p[0] > 127).count() as f32 / (w * h) as f32;
    let expected = std::f32::consts::PI * 110.0 * 110.0 / (640.0 * 480.0);
    assert!(
        (area - expected).abs() < 0.05,
        "Fläche {area:.3} statt ≈ {expected:.3}"
    );

    // Himmel mit SAM-Verfeinerung auf einer Landschaft
    let land = DynamicImage::ImageRgb8(RgbImage::from_fn(600, 400, |x, y| {
        let ridge = 220.0 + 40.0 * ((x as f32) / 60.0).sin();
        if (y as f32) > ridge {
            Rgb([30, 40, 25])
        } else {
            let t = y as f32 / ridge;
            Rgb([
                (110.0 + 60.0 * t) as u8,
                (150.0 + 40.0 * t) as u8,
                (225.0 - 20.0 * t) as u8,
            ])
        }
    }));
    let emb = sam.embed(&land).unwrap();
    let sky = sky_mask(&land, None, Some((&sam, &emb))).unwrap();
    let (w, h) = sky.dimensions();
    let at = |x: f32, y: f32| sky.get_pixel((x * w as f32) as u32, (y * h as f32) as u32)[0];
    assert!(at(0.5, 0.1) > 200 && at(0.5, 0.95) < 30);
}

/// Der ONNX-Export hat den Zuschnitt für 3:2 quer festgeschrieben – im
/// Hochformat lag die Maske früher gestaucht und nach unten links verschoben.
#[test]
fn sam_mask_fits_portrait_images() {
    let Some(dir) = setup() else {
        eprintln!("übersprungen: LUMENKORN_TEST_ORT/LUMENKORN_TEST_SEG fehlen");
        return;
    };
    let sam = Sam::load(&dir.join("sam-encoder.onnx"), &dir.join("sam-decoder.onnx")).unwrap();
    // Roter Ball oben rechts in einem Hochformat
    let (cx, cy, r) = (300.0, 220.0, 90.0);
    let img = DynamicImage::ImageRgb8(RgbImage::from_fn(400, 800, |x, y| {
        let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
        if d < r {
            Rgb([220, 30, 20])
        } else {
            let n = ((x * 31 + y * 17) % 11) as u8;
            Rgb([150 + n, 150 + n, 155 + n])
        }
    }));
    let emb = sam.embed(&img).unwrap();
    let check = |mask: image::GrayImage, what: &str| {
        let (w, h) = mask.dimensions();
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for (x, y, p) in mask.enumerate_pixels() {
            if p[0] > 127 {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
        // Rahmen der Maske in Fotopixeln gegen den Ball
        let s = 400.0 / w as f32;
        let got = [x0, y0, x1 + 1, y1 + 1].map(|v| v as f32 * s);
        let want = [cx - r, cy - r, cx + r, cy + r];
        for (g, e) in got.iter().zip(want) {
            assert!((g - e).abs() < 12.0, "{what}: {got:?} statt {want:?}");
        }
    };
    let click = Point {
        x: cx / 400.0,
        y: cy / 800.0,
        positive: true,
    };
    check(sam.predict(&emb, &[click]).unwrap(), "Klick");
    let rect = [
        (cx - r - 20.0) / 400.0,
        (cy - r - 20.0) / 800.0,
        (cx + r + 20.0) / 400.0,
        (cy + r + 20.0) / 800.0,
    ];
    check(sam.predict_box(&emb, rect).unwrap(), "Rahmen");
}
