//! Datentypen, die zwischen Kern und Oberfläche ausgetauscht werden.

use crate::formats::ImageKind;
use serde::{Deserialize, Serialize};

/// Ein vom Nutzer hinzugefügter Quellordner. Bilder bleiben, wo sie sind.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Root {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub image_count: i64,
    pub last_scan: Option<i64>,
}

/// Knoten im Ordnerbaum eines Quellordners.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FolderNode {
    pub path: String,
    pub name: String,
    /// Bilder direkt in diesem Ordner.
    pub count: i64,
    /// Bilder inklusive aller Unterordner.
    pub total: i64,
    pub children: Vec<FolderNode>,
}

/// Ein Projekt fasst beliebig viele Ordner (auch aus verschiedenen
/// Quellordnern oder Laufwerken) zu einer virtuellen Sammlung zusammen.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub folders: Vec<ProjectFolder>,
    pub image_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFolder {
    pub path: String,
    pub recursive: bool,
}

/// Kompakte Bildinfo für das Raster.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageSummary {
    pub id: i64,
    pub path: String,
    pub file_name: String,
    pub kind: ImageKind,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub captured_at: Option<String>,
    pub rating: i64,
    pub flag: i64,
    pub thumb_ready: bool,
    pub missing: bool,
    /// Hat eine aktuelle Bearbeitung.
    pub edited: bool,
}

/// Vollständige Bildinfo für die Detailansicht.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImageDetail {
    pub id: i64,
    pub path: String,
    pub dir: String,
    pub file_name: String,
    pub ext: String,
    pub size: i64,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub captured_at: Option<String>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<i64>,
    pub exposure: Option<String>,
    pub aperture: Option<f64>,
    pub focal_length: Option<f64>,
    pub rating: i64,
    pub flag: i64,
    pub tags: Vec<ImageTag>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageTag {
    pub name: String,
    pub source: TagSource,
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TagSource {
    Manual,
    Ai,
}

impl TagSource {
    pub fn as_str(self) -> &'static str {
        match self {
            TagSource::Manual => "manual",
            TagSource::Ai => "ai",
        }
    }
}

/// Welche Bilder angezeigt werden sollen.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Scope {
    All,
    Folder {
        path: String,
        recursive: bool,
    },
    Project {
        id: i64,
    },
    /// Fotos, auf denen eine Person (oder Gruppe) erkannt wurde
    Person {
        id: i64,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SortOrder {
    #[default]
    CapturedAsc,
    CapturedDesc,
    NameAsc,
    RatingDesc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageQuery {
    pub scope: Scope,
    #[serde(default)]
    pub sort: SortOrder,
    /// Freitext über Dateiname, Kamera, Objektiv und Schlagworte.
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub min_rating: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
    /// Nur diese Fotos (z. B. Treffer der Inhaltssuche)
    #[serde(skip)]
    pub ids: Option<Vec<i64>>,
}

impl ImageQuery {
    pub fn new(scope: Scope) -> Self {
        Self {
            scope,
            sort: SortOrder::default(),
            search: None,
            ids: None,
            min_rating: None,
            limit: None,
            offset: None,
        }
    }
}

/// Ergebnis eines Ordner-Scans.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub root_id: i64,
    pub added: usize,
    pub updated: usize,
    /// Umbenannt/verschoben erkannt – Bearbeitungen bleiben erhalten.
    pub moved: usize,
    pub missing: usize,
    pub unchanged: usize,
    /// Ordnerstruktur hat sich geändert (neue, leere oder entfernte Ordner)
    #[serde(default)]
    pub folders_changed: bool,
    /// Bilddateien in nicht unterstützten Formaten (z. B. HEIC), je Endung
    #[serde(default)]
    pub unsupported: Vec<(String, usize)>,
    /// Nicht lesbare Ordner/Dateien (z. B. fehlende Berechtigung)
    #[serde(default)]
    pub unreadable: usize,
    /// Vom Nutzer ausgelöst („Neu einlesen“): Ergebnis immer melden.
    #[serde(default)]
    pub requested: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub root_id: i64,
    pub phase: ScanPhase,
    pub done: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ScanPhase {
    Walking,
    Reading,
    Writing,
}

/// Eine gespeicherte Bearbeitung. `params` ist der vollständige,
/// non-destruktive Parametersatz der Entwicklungspipeline als JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    pub id: i64,
    pub image_id: i64,
    pub name: String,
    pub params: serde_json::Value,
    pub is_current: bool,
    pub created_at: i64,
    pub updated_at: i64,
}
