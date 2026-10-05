//! Test mit den echten DRUNet-Gewichten:
//!
//!     LUMENKORN_TEST_ORT=/pfad/libonnxruntime.so LUMENKORN_TEST_DENOISE=/pfad cargo test -p lumenkorn-ai --test denoise
//!
//! `LUMENKORN_TEST_DENOISE` enthält drunet_color.pth (KAIR-Release) und ein
//! sauberes Testbild clean.png.

use lumenkorn_ai::denoise::{Denoiser, Rgb, build_drunet_onnx};
use lumenkorn_ai::runtime;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn srgb_to_lin(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn lin_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

#[test]
fn builds_drunet_and_removes_noise() {
    let (Ok(lib), Ok(dir)) = (
        std::env::var("LUMENKORN_TEST_ORT"),
        std::env::var("LUMENKORN_TEST_DENOISE"),
    ) else {
        eprintln!("übersprungen: LUMENKORN_TEST_ORT/LUMENKORN_TEST_DENOISE fehlen");
        return;
    };
    let dir = PathBuf::from(dir);
    runtime::init(std::path::Path::new(&lib)).unwrap();

    let t = std::time::Instant::now();
    let onnx = build_drunet_onnx(&std::fs::read(dir.join("drunet_color.pth")).unwrap()).unwrap();
    eprintln!(
        "ONNX gebaut in {:.2?}, {} MB",
        t.elapsed(),
        onnx.len() / 1_000_000
    );
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("drunet.onnx");
    std::fs::write(&path, onnx).unwrap();
    let d = Denoiser::load(&path).unwrap();

    // Sauberes Bild + Gaußrauschen σ = 20/255 im sRGB-Raum
    let clean = image::open(dir.join("clean.png")).unwrap().to_rgb8();
    let (w, h) = (clean.width() as usize, clean.height() as usize);
    let mut seed = 7u64;
    let mut gauss = || {
        let mut u = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 11) as f32 / (1u64 << 53) as f32).max(1e-7)
        };
        let (a, b) = (u(), u());
        (-2.0 * a.ln()).sqrt() * (2.0 * std::f32::consts::PI * b).cos()
    };
    let sigma = 20.0 / 255.0;
    let clean_s: Vec<[f32; 3]> = clean
        .pixels()
        .map(|p| p.0.map(|v| v as f32 / 255.0))
        .collect();
    let noisy_s: Vec<[f32; 3]> = clean_s
        .iter()
        .map(|p| p.map(|v| (v + sigma * gauss()).clamp(0.0, 1.0)))
        .collect();
    let noisy_lin: Vec<[f32; 3]> = noisy_s.iter().map(|p| p.map(srgb_to_lin)).collect();

    let t = std::time::Instant::now();
    let out = d
        .denoise(
            &Rgb {
                width: w,
                height: h,
                data: &noisy_lin,
            },
            1.0,
            &|a, b| eprint!("\r{a}/{b} Kacheln "),
            &AtomicBool::new(false),
        )
        .unwrap();
    eprintln!("\nentrauscht {w}×{h} in {:.2?}", t.elapsed());

    let psnr = |a: &[[f32; 3]]| {
        let mse: f32 = a
            .iter()
            .zip(&clean_s)
            .map(|(x, y)| (0..3).map(|c| (x[c] - y[c]).powi(2)).sum::<f32>())
            .sum::<f32>()
            / (3 * a.len()) as f32;
        10.0 * (1.0 / mse).log10()
    };
    let out_s: Vec<[f32; 3]> = out.iter().map(|p| p.map(lin_to_srgb)).collect();
    let (before, after) = (psnr(&noisy_s), psnr(&out_s));
    eprintln!("PSNR {before:.2} dB → {after:.2} dB");
    assert!(
        after > before + 5.0,
        "zu wenig Verbesserung: {before:.2} → {after:.2}"
    );

    if let Ok(o) = std::env::var("LUMENKORN_TEST_OUT") {
        let mut img = image::RgbImage::new(w as u32 * 2, h as u32);
        for (i, (n, d)) in noisy_s.iter().zip(&out_s).enumerate() {
            let (x, y) = ((i % w) as u32, (i / w) as u32);
            img.put_pixel(x, y, image::Rgb(n.map(|v| (v * 255.0) as u8)));
            img.put_pixel(x + w as u32, y, image::Rgb(d.map(|v| (v * 255.0) as u8)));
        }
        img.save(PathBuf::from(o).join("denoise_vergleich.png"))
            .unwrap();
    }
}
