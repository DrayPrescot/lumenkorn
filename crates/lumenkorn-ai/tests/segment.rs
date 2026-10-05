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
    let sky = sky_mask(&land, Some((&sam, &emb))).unwrap();
    let (w, h) = sky.dimensions();
    let at = |x: f32, y: f32| sky.get_pixel((x * w as f32) as u32, (y * h as f32) as u32)[0];
    assert!(at(0.5, 0.1) > 200 && at(0.5, 0.95) < 30);
}
