//! Tests mit echten RAW-Dateien. Ordner per Umgebungsvariable angeben:
//!
//!     LUMENKORN_TEST_RAW=/pfad/zu/raws cargo test -p lumenkorn-develop
//!
//! Ohne Variable werden die Tests übersprungen.

use lumenkorn_develop::*;
use std::path::PathBuf;

fn raws() -> Vec<PathBuf> {
    let Ok(dir) = std::env::var("LUMENKORN_TEST_RAW") else {
        return vec![];
    };
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .is_some_and(|e| !e.eq_ignore_ascii_case("jpg"))
        })
        .collect();
    v.sort();
    v
}

#[test]
fn develops_real_raws() {
    for path in raws() {
        let t = std::time::Instant::now();
        let src = Source::load(&path, true, None, Some(2048))
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let load = t.elapsed();
        assert_eq!(src.kind, SourceKind::Raw);
        // Proxy höchstens 2048 Pixel; kleinere Sensoren (z. B. Fuji F700) bleiben
        let full = src.full_width.max(src.full_height);
        assert_eq!(src.image.long_edge(), full.min(2048), "{}", path.display());

        let t = std::time::Instant::now();
        let out = render(
            &src,
            &DevelopParams {
                exposure: 0.3,
                shadows: 30.0,
                clarity: 20.0,
                ..Default::default()
            },
        );
        let render_time = t.elapsed();

        let mean: f32 = out
            .data
            .iter()
            .map(|c| (c[0] + c[1] + c[2]) / 3.0)
            .sum::<f32>()
            / out.data.len() as f32;
        assert!(mean > 0.02 && mean < 0.98, "unplausible Helligkeit {mean}");
        eprintln!(
            "{}: {}×{} (voll {}×{}), laden {load:.2?}, entwickeln {render_time:.2?}, Mittel {mean:.3}",
            path.file_name().unwrap().to_string_lossy(),
            out.width,
            out.height,
            src.full_width,
            src.full_height
        );

        if let Ok(out_dir) = std::env::var("LUMENKORN_TEST_OUT") {
            let jpg = encode_jpeg_for_test(&out);
            std::fs::write(
                PathBuf::from(out_dir).join(format!(
                    "{}.jpg",
                    path.file_stem().unwrap().to_string_lossy()
                )),
                jpg,
            )
            .unwrap();
        }
    }
}

fn encode_jpeg_for_test(img: &ImageF32) -> Vec<u8> {
    output::encode_jpeg(img, 88).unwrap()
}
