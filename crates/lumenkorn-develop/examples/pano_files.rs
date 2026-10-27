//! Panorama aus echten Aufnahmen, wie in der App (zum Prüfen der Nähte):
//!
//!     cargo run --release -p lumenkorn-develop --example pano_files -- <Ausgabe.jpg> <Megapixel> <Fotos …>
//!
//! `LUMENKORN_ORIENTATION=8` dreht alle Fotos wie das EXIF-Feld (Hochformat).
//!
//! Schreibt das Ergebnis entwickelt (Standardeinstellungen) als JPEG.

use lumenkorn_develop::merge::pano;
use lumenkorn_develop::output::{ExportFormat, ExportOptions};
use lumenkorn_develop::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [out, mp, files @ ..] = args.as_slice() else {
        eprintln!("Aufruf: pano_files <Ausgabe.jpg> <Megapixel> <Fotos …>");
        std::process::exit(2);
    };
    let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
    let raw = |p: &Path| {
        !matches!(
            p.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("jpg" | "jpeg" | "png" | "tif" | "tiff" | "webp")
        )
    };
    let orientation: Option<u16> = std::env::var("LUMENKORN_ORIENTATION")
        .ok()
        .and_then(|v| v.parse().ok());
    let load = |i: usize| -> Result<ImageF32> {
        // Wie in der App: Objektivkorrektur vor dem Ausrichten
        let src = Source::load(&files[i], raw(&files[i]), orientation, None)?;
        Ok(match &src.lens {
            Some(lens) => lens.apply(&src.image),
            None => src.image,
        })
    };
    let opts = pano::PanoOptions {
        max_megapixels: mp.parse().unwrap_or(40.0),
        ..Default::default()
    };
    let t = Instant::now();
    let cancel = AtomicBool::new(false);
    let img = pano::merge(
        files.len(),
        &load,
        opts,
        &|stage, done, total| eprintln!("{stage} {done}/{total}"),
        &cancel,
    )?;
    println!("{}×{} in {:.1?}", img.width, img.height, t.elapsed());
    let exr = Path::new(out).with_extension("exr");
    exr_io::write(&img, &exr, true)?;
    let src = Source::load(&exr, false, None, None)?;
    output::write(
        &render(&src, &DevelopParams::default()),
        Path::new(out),
        &ExportOptions {
            format: ExportFormat::Jpeg,
            formats: Vec::new(),
            dng_edited: true,
            on_conflict: Default::default(),
            quality: 90,
            long_edge: None,
        },
        None,
    )?;
    std::fs::remove_file(&exr).ok();
    println!("gespeichert: {out}");
    Ok(())
}
