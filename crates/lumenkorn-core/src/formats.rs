//! Welche Dateien als Bilder gelten.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageKind {
    /// Direkt dekodierbare Formate (JPEG, PNG, TIFF, WebP).
    Raster,
    /// Kamera-RAW. Für Vorschauen wird das eingebettete JPEG genutzt,
    /// die eigentliche Entwicklung folgt später über LibRaw/rawler.
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
const RASTER: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "webp", "exr"];

const RAW: &[&str] = &[
    "3fr", "arw", "cr2", "cr3", "crw", "dng", "erf", "iiq", "kdc", "mef", "mos", "mrw", "nef",
    "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf", "srw", "x3f",
];

/// Liefert den Bildtyp oder `None`, wenn die Datei kein unterstütztes Bild ist.
pub fn classify(path: &Path) -> Option<(String, ImageKind)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if RAW.contains(&ext.as_str()) {
        Some((ext, ImageKind::Raw))
    } else if RASTER.contains(&ext.as_str()) {
        Some((ext, ImageKind::Raster))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
