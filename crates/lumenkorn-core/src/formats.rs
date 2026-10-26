//! Welche Dateien als Bilder gelten.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageKind {
    /// Direkt dekodierbare Formate (JPEG, PNG, TIFF, WebP).
    Raster,
    /// Kamera-RAW. Unbearbeitet zeigen Vorschauen das eingebettete JPEG,
    /// sonst wird es über `lumenkorn_develop::Source` entwickelt.
    Raw,
}

impl ImageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ImageKind::Raster => "raster",
            ImageKind::Raw => "raw",
        }
    }

    pub fn parse(s: &str) -> Self {
        if s == "raw" {
            ImageKind::Raw
        } else {
            ImageKind::Raster
        }
    }
}

/// `exr`: lineare Ergebnisse von HDR, Panorama und Focus Stacking (szenenbezogen
/// wie ein RAW, nicht für die Anzeige gerendert).
const RASTER: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "tif", "tiff", "webp", "exr",
];

/// Bildformate, die (noch) nicht gelesen werden – beim Scan gezählt und
/// gemeldet, damit ein Ordner voller Handy-Fotos (HEIC) nicht „leer“ wirkt.
const UNSUPPORTED: &[&str] = &["heic", "heif", "avif", "jxl", "psd", "gif", "bmp"];

/// Endung, wenn die Datei ein Bild in einem nicht unterstützten Format ist.
pub(crate) fn unsupported_image(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    UNSUPPORTED.contains(&ext.as_str()).then_some(ext)
}

const RAW: &[&str] = &[
    "3fr", "arw", "cr2", "cr3", "crw", "dng", "erf", "iiq", "kdc", "mef", "mos", "mrw", "nef",
    "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf", "srw", "x3f",
];

/// Liefert den Bildtyp oder `None`, wenn die Datei kein unterstütztes Bild ist.
pub(crate) fn classify(path: &Path) -> Option<(String, ImageKind)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if RAW.contains(&ext.as_str()) {
        Some((ext, ImageKind::Raw))
    } else if RASTER.contains(&ext.as_str()) {
        Some((ext, ImageKind::Raster))
    } else {
        None
    }
}

/// Endungen, die als ein Dateityp gelten (Filter „JPEG“ trifft auch .jpg).
const SAME_TYPE: &[&[&str]] = &[&["jpg", "jpeg", "jpe", "jfif"], &["tif", "tiff"]];

/// Dateitypen eines Filters als Endungen: Groß-/Kleinschreibung und Punkt
/// egal, „jpeg“ und „jpg“ sind derselbe Typ, „raw“ steht für alle RAW-Formate.
pub fn expand_file_types<S: AsRef<str>>(types: &[S]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in types {
        let t = t
            .as_ref()
            .trim()
            .trim_start_matches('.')
            .to_ascii_lowercase();
        let group: Vec<&str> = if t == "raw" {
            RAW.to_vec()
        } else if let Some(g) = SAME_TYPE.iter().find(|g| g.contains(&t.as_str())) {
            g.to_vec()
        } else {
            vec![t.as_str()]
        };
        for e in group {
            if !e.is_empty() && !out.iter().any(|o| o == e) {
                out.push(e.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_file_types() {
        assert_eq!(
            expand_file_types(&["JPEG", ".arw", "jpg"]),
            ["jpg", "jpeg", "jpe", "jfif", "arw"]
        );
        assert_eq!(expand_file_types(&["Tif"]), ["tif", "tiff"]);
        assert!(expand_file_types(&["raw"]).contains(&"cr3".to_string()));
        assert!(expand_file_types::<&str>(&[]).is_empty());
    }

    #[test]
    fn classifies_case_insensitively() {
        assert_eq!(classify(Path::new("a/B.NEF")).unwrap().1, ImageKind::Raw);
        assert_eq!(
            classify(Path::new("x.JpG")).unwrap(),
            ("jpg".into(), ImageKind::Raster)
        );
        assert!(classify(Path::new("notes.txt")).is_none());
        assert!(classify(Path::new("noext")).is_none());
    }
}
