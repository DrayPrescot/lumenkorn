//! Fotoalbum: aus einem Projekt (oder Ordner) die besten Fotos zu einem Thema
//! auswählen (`select`) und das in der Oberfläche gestaltete Album als
//! druckfertiges PDF und JPG je Seite speichern (`export`).
//!
//! Die Seitengestaltung selbst (Aufteilung, Tauschen, Verschieben) passiert im
//! Frontend (`src/lib/album.ts`); hierher kommt nur das fertige Ergebnis in
//! Millimetern.

pub mod export;
pub mod select;

use serde::{Deserialize, Serialize};

/// Ein Bildfeld auf einer Seite. Maße in mm, bezogen auf die beschnittene
/// Seite (ohne Beschnittzugabe), Ursprung oben links.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumCell {
    pub id: i64,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Ausschnitt wie CSS `object-position`: 0 = linker/oberer Rand des
    /// Fotos sichtbar, 1 = rechter/unterer, 0.5 = Mitte. Das Foto füllt das
    /// Feld vollständig (`object-fit: cover`).
    #[serde(default = "half")]
    pub ox: f64,
    #[serde(default = "half")]
    pub oy: f64,
}

fn half() -> f64 {
    0.5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumPage {
    pub cells: Vec<AlbumCell>,
    /// Texte und Karte als PNG mit Alpha (Base64, Standard-Alphabet), das die
    /// ganze Seite samt Beschnittzugabe abdeckt. Wird nach den Fotos mit Alpha
    /// über die Seite gelegt. Seiten ohne Fotos, aber mit Ebene, sind erlaubt.
    #[serde(default)]
    pub overlay: Option<String>,
}

/// Das fertig gestaltete Album, wie es gespeichert wird.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumDoc {
    /// Seitenformat (eine Seite, beschnitten) in mm
    pub width_mm: f64,
    pub height_mm: f64,
    /// Beschnittzugabe an jedem Rand in mm. Felder, die an den Seitenrand
    /// stoßen, werden in die Zugabe hinein verlängert.
    pub bleed_mm: f64,
    /// Auflösung der Ausgabe (üblich 300)
    pub dpi: u32,
    /// Hintergrundfarbe der Seiten (sRGB)
    pub background: [u8; 3],
    pub pages: Vec<AlbumPage>,
}
