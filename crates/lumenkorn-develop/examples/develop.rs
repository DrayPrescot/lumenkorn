//! Entwickelt eine Datei ohne Oberfläche:
//!
//!     cargo run -p lumenkorn-develop --example develop -- <Eingabe> <Ausgabe.jpg> ['{"exposure":0.5}']
//!
//! Praktisch, um Pipeline-Änderungen an echten Bildern zu prüfen.

use lumenkorn_develop::output::{ExportFormat, ExportOptions};
use lumenkorn_develop::*;
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let input = PathBuf::from(args.next().expect("Eingabedatei fehlt"));
    let output = PathBuf::from(args.next().expect("Ausgabedatei fehlt"));
    let params: DevelopParams = args
        .next()
        .map(|j| serde_json::from_str(&j).expect("Parameter sind kein gültiges JSON"))
        .unwrap_or_default();

    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let raw = !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "tif" | "tiff" | "webp"
    );

    let t = Instant::now();
    let src = Source::load(&input, raw, None, Some(2048))?;
    println!(
        "geladen in {:.2?} ({}×{})",
        t.elapsed(),
        src.full_width,
        src.full_height
    );

    let t = Instant::now();
    let img = render(&src, &params);
    println!("entwickelt in {:.2?}", t.elapsed());

    output::write(
        &img,
        &output,
        &ExportOptions {
            format: ExportFormat::Jpeg,
            formats: Vec::new(),
            quality: 88,
            long_edge: None,
        },
    )?;
    Ok(())
}
