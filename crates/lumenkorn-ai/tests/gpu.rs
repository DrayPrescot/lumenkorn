//! GPU gegen CPU mit derselben Laufzeitbibliothek (WebGPU-Variante):
//!
//!     LUMENKORN_TEST_ORT_GPU=/pfad/runtime-webgpu/libonnxruntime.so \
//!     LUMENKORN_TEST_DENOISE=/pfad cargo test -p lumenkorn-ai --test gpu
//!
//! Ohne Grafikkarte genügt ein Software-Vulkan (Mesa lavapipe) – dann sehr
//! langsam, aber die Ergebnisse sind dieselben.

use lumenkorn_ai::accel::{self, Backend};
use lumenkorn_ai::denoise::{Denoiser, Rgb, build_drunet_onnx};
use lumenkorn_ai::runtime;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

/// Die GPU-Einstellung ist prozessweit – Tests nacheinander ausführen.
static SERIAL: Mutex<()> = Mutex::new(());

fn gpu_lib() -> Option<String> {
    let lib = std::env::var("LUMENKORN_TEST_ORT_GPU").ok()?;
    runtime::init_with(std::path::Path::new(&lib), Backend::Webgpu).unwrap();
    Some(lib)
}

/// IS-Net („Motiv“) auf GPU und CPU: dieselbe Maske.
#[test]
fn subject_on_gpu_matches_cpu() {
    let (Some(_), Ok(dir)) = (gpu_lib(), std::env::var("LUMENKORN_TEST_SEG")) else {
        eprintln!("übersprungen: LUMENKORN_TEST_ORT_GPU/LUMENKORN_TEST_SEG fehlen");
        return;
    };
    let _g = SERIAL.lock().unwrap();
    let path = PathBuf::from(dir).join("subject.onnx");
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(640, 480, |x, y| {
        let d = ((x as f32 - 400.0).powi(2) + (y as f32 - 240.0).powi(2)).sqrt();
        if d < 110.0 {
            image::Rgb([200, 30, 20])
        } else {
            let n = ((x * 31 + y * 17) % 11) as u8;
            image::Rgb([150 + n, 150 + n, 155 + n])
        }
    }));
    let run = |gpu: bool| {
        accel::set_use_gpu(gpu);
        let m = lumenkorn_ai::segment::SubjectModel::load(&path).unwrap();
        assert_eq!(accel::on_gpu("subject.onnx"), gpu);
        let t = std::time::Instant::now();
        let mask = m.predict(&img).unwrap();
        eprintln!(
            "Motiv {}: {:.2?}",
            if gpu { "GPU" } else { "CPU" },
            t.elapsed()
        );
        mask
    };
    let cpu = run(false);
    let gpu = run(true);
    let max = cpu
        .pixels()
        .zip(gpu.pixels())
        .map(|(a, b)| (a[0] as i32 - b[0] as i32).abs())
        .max()
        .unwrap();
    eprintln!("größte Abweichung der Maske: {max}/255");
    assert!(max <= 2, "Masken weichen ab: {max}");
}

#[test]
fn denoise_on_gpu_matches_cpu() {
    let (Some(_), Ok(dir)) = (gpu_lib(), std::env::var("LUMENKORN_TEST_DENOISE")) else {
        eprintln!("übersprungen: LUMENKORN_TEST_ORT_GPU/LUMENKORN_TEST_DENOISE fehlen");
        return;
    };
    let _g = SERIAL.lock().unwrap();
    let dir = PathBuf::from(dir);
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("drunet.onnx");
    std::fs::write(
        &path,
        build_drunet_onnx(&std::fs::read(dir.join("drunet_color.pth")).unwrap()).unwrap(),
    )
    .unwrap();

    // Klein halten: Software-Vulkan braucht sonst Minuten.
    let clean = image::open(dir.join("clean.png")).unwrap().to_rgb8();
    let (w, h) = (240usize, 176usize);
    let mut seed = 11u64;
    let mut noise = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.12
    };
    let data: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let p = clean
                .get_pixel((i % w) as u32 + 200, (i / w) as u32 + 150)
                .0;
            p.map(|v| ((v as f32 / 255.0).powf(2.2) + noise() * 0.3).max(0.0))
        })
        .collect();
    let img = Rgb {
        width: w,
        height: h,
        data: &data,
    };

    let run = |gpu: bool| {
        accel::set_use_gpu(gpu);
        let d = Denoiser::load(&path).unwrap();
        assert_eq!(d.on_gpu(), gpu, "Modell läuft nicht wie gewünscht");
        let t = std::time::Instant::now();
        let out = d
            .denoise(&img, 1.0, &|_, _| {}, &AtomicBool::new(false))
            .unwrap();
        eprintln!(
            "{} {w}×{h}: {:.2?}",
            if gpu { "GPU" } else { "CPU" },
            t.elapsed()
        );
        out
    };
    let cpu = run(false);
    let gpu = run(true);
    assert!(accel::active().gpu_error.is_none());
    let max = cpu
        .iter()
        .zip(&gpu)
        .flat_map(|(a, b)| (0..3).map(move |c| (a[c] - b[c]).abs()))
        .fold(0f32, f32::max);
    eprintln!("größte Abweichung GPU/CPU: {max:e}");
    assert!(max < 1e-3, "GPU weicht ab: {max}");
    // Wirklich entrauscht und nicht bloß kopiert
    let changed = data
        .iter()
        .zip(&gpu)
        .filter(|(a, b)| (a[0] - b[0]).abs() > 1e-3)
        .count();
    assert!(changed > w * h / 2);
}
