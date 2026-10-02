//! Lineare Bilder als OpenEXR: Ergebnisse von HDR, Panorama und Focus
//! Stacking. Halbgleitkomma (16 Bit) mit ZIP-Kompression – verlustfrei für
//! diesen Zweck, Werte über 1,0 bleiben erhalten (Lichterreserve wie bei RAWs).
//! Farbraum: lineares Rec.709/sRGB, wie alle Quellbilder in Lumenkorn.

use crate::error::{Error, Result};
use crate::image_f32::ImageF32;
use exr::prelude::*;
use std::path::Path;

pub fn is_exr(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("exr"))
}

/// Merkt sich in der Datei, ob die Quellen bereits für die Anzeige gerendert
/// waren (JPEG/TIFF) – dann wird das Ergebnis auch so entwickelt, nicht wie ein RAW.
const KIND_ATTR: &str = "lumenkornSource";

/// Schreibt atomar (temporäre Datei, dann umbenennen). `scene_linear`:
/// aus RAWs entstanden (Entwicklung wie ein RAW), sonst wie ein JPEG.
pub fn write(img: &ImageF32, path: &Path, scene_linear: bool) -> Result<()> {
    let w = img.width;
    let data = &img.data;
    let mut attrs = LayerAttributes::named("rgb");
    attrs.other.insert(
        Text::from(KIND_ATTR),
        AttributeValue::Text(Text::from(if scene_linear { "raw" } else { "display" })),
    );
    let layer = Layer::new(
        (img.width, img.height),
        attrs,
        Encoding {
            compression: Compression::ZIP16,
            blocks: Blocks::ScanLines,
            line_order: LineOrder::Increasing,
        },
        SpecificChannels::rgb(move |pos: Vec2<usize>| {
            let p = data[pos.1 * w + pos.0];
            let h = |v: f32| {
                f16::from_f32(if v.is_finite() {
                    v.clamp(0.0, 65000.0)
                } else {
                    0.0
                })
            };
            (h(p[0]), h(p[1]), h(p[2]))
        }),
    );
    let tmp = path.with_extension("exr.tmp");
    Image::from_layer(layer)
        .write()
        .to_file(&tmp)
        .map_err(|e| Error::Params(format!("EXR schreiben: {e}")))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Breite und Höhe aus dem Dateikopf, ohne die Pixel zu lesen.
pub fn dimensions(path: &Path) -> Option<(usize, usize)> {
    let meta = exr::meta::MetaData::read_from_file(path, false).ok()?;
    let h = meta.headers.first()?;
    Some((h.layer_size.0, h.layer_size.1))
}

/// Liest die erste RGB(A)-Ebene als lineares Bild; `true`, wenn es wie ein
/// RAW zu entwickeln ist (Standard für fremde EXR-Dateien).
pub fn read(path: &Path) -> Result<(ImageF32, bool)> {
    let img = read_first_rgba_layer_from_file(
        path,
        |res, _| ImageF32 {
            width: res.0,
            height: res.1,
            data: vec![[0.0; 3]; res.0 * res.1],
        },
        |img: &mut ImageF32, pos, (r, g, b, _a): (f32, f32, f32, f32)| {
            let w = img.width;
            img.data[pos.1 * w + pos.0] =
                [r, g, b].map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
        },
    )
    .map_err(|e| Error::Params(format!("EXR lesen: {e}")))?;
    let scene = !matches!(
        img.layer_data.attributes.other.get(&Text::from(KIND_ATTR)),
        Some(AttributeValue::Text(t)) if t.as_slice() == b"display"
    );
    Ok((img.layer_data.channel_data.pixels, scene))
}

/// Kante der zwischengespeicherten Verkleinerung (= Proxy der Bearbeitung).
pub const PROXY_EDGE: usize = 2048;

type ProxyEntry = (
    std::path::PathBuf,
    Option<std::time::SystemTime>,
    ImageF32,
    bool,
);
static PROXY: std::sync::Mutex<Option<ProxyEntry>> = std::sync::Mutex::new(None);

/// Wie [`read`], aber auf höchstens `max_edge` verkleinert. Thumbnail,
/// Vorschau, Gesichtsanalyse und Bearbeitungs-Proxy fragen dasselbe Bild kurz
/// nacheinander an – ein 150-MP-Panorama wird so nur einmal voll gelesen.
pub fn read_downscaled(path: &Path, max_edge: usize) -> Result<(ImageF32, bool)> {
    if max_edge > PROXY_EDGE {
        let (img, scene) = read(path)?;
        return Ok((img.downscale_to(max_edge), scene));
    }
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    {
        let cache = PROXY.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((p, t, img, scene)) = cache.as_ref()
            && p == path
            && *t == mtime
        {
            return Ok((img.downscale_to(max_edge), *scene));
        }
    }
    let (full, scene) = read(path)?;
    let proxy = full.downscale_to(PROXY_EDGE);
    drop(full);
    let out = proxy.downscale_to(max_edge);
    *PROXY.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((path.to_path_buf(), mtime, proxy, scene));
    Ok((out, scene))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_keeps_highlights() {
        let img = ImageF32::from_fn(64, 32, |x, y| [x as f32 / 8.0, y as f32 / 32.0, 0.25]);
        let p = std::env::temp_dir().join(format!("lw-exr-{}.exr", std::process::id()));
        write(&img, &p, false).unwrap();
        let (back, scene) = read(&p).unwrap();
        assert!(!scene, "Herkunft geht verloren");
        assert_eq!((back.width, back.height), (64, 32));
        let a = back.get(63, 10);
        assert!(
            (a[0] - 63.0 / 8.0).abs() < 0.01,
            "Wert über 1 bleibt: {a:?}"
        );
        assert!((a[1] - 10.0 / 32.0).abs() < 1e-3);
        let _ = std::fs::remove_file(p);
    }
}
