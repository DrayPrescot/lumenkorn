//! Auto-Tonwerte für mehrere Dateien ausgeben:
//!
//!     cargo run --release -p lumenkorn-develop --example auto_probe -- <RAW>…
use lumenkorn_develop::*;
fn stats(img: &ImageF32) -> String {
    let mut y: Vec<f32> = img
        .data
        .iter()
        .map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2])
        .collect();
    y.sort_by(f32::total_cmp);
    let mut m: Vec<f32> = img.data.iter().map(|p| p[0].max(p[1]).max(p[2])).collect();
    m.sort_by(f32::total_cmp);
    let q = |v: &Vec<f32>, f: f32| v[((v.len() - 1) as f32 * f) as usize];
    let mean = y.iter().sum::<f32>() / y.len() as f32;
    let sd = (y.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / y.len() as f32).sqrt();
    format!(
        "p0.5 {:.3} p5 {:.3} p50 {:.3} p95 {:.3} max99.5 {:.3} sd {:.3}",
        q(&y, 0.005),
        q(&y, 0.05),
        q(&y, 0.5),
        q(&y, 0.95),
        q(&m, 0.995),
        sd
    )
}
fn main() -> Result<()> {
    for path in std::env::args().skip(1) {
        let p = std::path::Path::new(&path);
        let src = Source::load(p, true, None, Some(1024))?.downscaled(320);
        let a = auto::auto_tone(&src, &DevelopParams::default());
        let name = p.file_name().unwrap().to_string_lossy();
        println!(
            "{name:28} B {:+.2} K {:+3.0} L {:+3.0} T {:+3.0} W {:+3.0} S {:+3.0} D {:+3.0}",
            a.exposure, a.contrast, a.highlights, a.shadows, a.whites, a.blacks, a.vibrance
        );
        let base = DevelopParams {
            exposure: a.exposure,
            ..Default::default()
        };
        println!("   nur Belichtung: {}", stats(&render(&src, &base)));
        let full = DevelopParams {
            exposure: a.exposure,
            contrast: a.contrast,
            highlights: a.highlights,
            shadows: a.shadows,
            whites: a.whites,
            blacks: a.blacks,
            vibrance: a.vibrance,
            ..Default::default()
        };
        println!("   Auto komplett:  {}", stats(&render(&src, &full)));
    }
    Ok(())
}
