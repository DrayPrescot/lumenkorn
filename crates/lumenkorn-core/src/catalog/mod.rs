//! Der Katalog: eine SQLite-Datenbank mit Quellordnern, Bildern, Projekten,
//! Schlagworten und Bearbeitungen. Bilddateien selbst werden nie verändert.

use crate::LockExt;
use crate::error::{Error, Result};
use crate::formats::ImageKind;
use crate::metadata::Metadata;
use crate::model::*;
use crate::paths::{self, SEP};
use lumenkorn_develop::{DevelopParams, TransferGroups};
use lumenkorn_develop::{i18n::tr_s, tr};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

mod ai;
mod edits;
mod images;
mod migrations;
mod projects;
mod roots;
mod scan;
mod tags;
#[cfg(test)]
mod tests;
mod thumbs;

pub(crate) use migrations::flush_search;

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub struct Catalog {
    pub(crate) conn: Connection,
    /// Zählt Änderungen an Personen/Gruppen, die einen Personen-Index im
    /// Speicher ungültig machen (gelöschte Gruppen, Umbenennen, Löschen).
    pub(crate) people_gen: std::cell::Cell<u64>,
}

/// Angaben eines Fotos für die Stapel-Umbenennung.
#[derive(Debug, Clone)]
pub struct RenameInfo {
    pub id: i64,
    pub path: String,
    pub dir: String,
    pub file_name: String,
    pub captured_at: Option<String>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    /// Änderungszeit der Datei (Sekunden seit 1970)
    pub mtime: i64,
    pub missing: bool,
}

/// Bekannter Datensatz, wie ihn der Scanner zum Abgleich braucht.
#[derive(Debug, Clone)]
pub(crate) struct KnownImage {
    pub id: i64,
    pub path: String,
    pub size: i64,
    pub mtime: i64,
    pub quick_hash: String,
    pub missing: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct NewImage {
    pub root_id: i64,
    pub path: String,
    pub dir: String,
    pub file_name: String,
    pub ext: String,
    pub kind: ImageKind,
    pub size: i64,
    pub mtime: i64,
    pub quick_hash: String,
    pub meta: Metadata,
}

/// Bearbeitetes Foto: (Foto, Zeitpunkt der letzten Änderung, Kamera, RAW?)
pub type EditedPhoto = (i64, i64, Option<String>, bool);

/// Auftrag für den Vorschau-Generator.
#[derive(Debug, Clone)]
pub struct ThumbJob {
    pub id: i64,
    pub path: String,
    pub kind: ImageKind,
    pub orientation: Option<i64>,
    /// Aktuelle Bearbeitung, falls vorhanden – das Thumbnail zeigt sie.
    pub edit: Option<serde_json::Value>,
    /// Längere Kante des Originals laut Metadaten
    pub long_edge: Option<u32>,
}

impl Catalog {
    /// Öffnet (oder erstellt) den Katalog und spielt Migrationen ein.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // Größerer Seiten-Cache (Standard ~2 MB) und Speicherabbildung: Embeddings
        // und Gesichter (zig MB) lesen sich so deutlich schneller.
        conn.pragma_update(None, "cache_size", -32_000)?; // KiB
        conn.pragma_update(None, "mmap_size", 256i64 << 20)?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        Self::init(conn)
    }

    /// Katalog im Arbeitsspeicher, für Tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut cat = Catalog {
            conn,
            people_gen: Default::default(),
        };
        cat.migrate()?;
        // Reste nach einem Absturz zwischen Änderung und Neuaufbau
        flush_search(&cat.conn)?;
        Ok(cat)
    }
}

const THUMB_JOB_SELECT: &str = "SELECT i.id, i.path, i.kind, i.orientation, e.params,
            MAX(COALESCE(i.width, 0), COALESCE(i.height, 0))
     FROM images i LEFT JOIN edits e ON e.image_id = i.id AND e.is_current = 1";

fn thumb_job_from_row(r: &Row) -> rusqlite::Result<ThumbJob> {
    let edit: Option<String> = r.get(4)?;
    Ok(ThumbJob {
        id: r.get(0)?,
        path: r.get(1)?,
        kind: ImageKind::parse(&r.get::<_, String>(2)?),
        orientation: r.get(3)?,
        edit: edit.and_then(|s| serde_json::from_str(&s).ok()),
        long_edge: r.get::<_, i64>(5).ok().filter(|e| *e > 0).map(|e| e as u32),
    })
}

fn root_from_row(r: &Row) -> rusqlite::Result<Root> {
    let path: String = r.get(1)?;
    let name = Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    Ok(Root {
        id: r.get(0)?,
        path,
        name,
        last_scan: r.get(2)?,
        image_count: r.get(3)?,
    })
}

fn summary_from_row(r: &Row) -> rusqlite::Result<ImageSummary> {
    Ok(ImageSummary {
        id: r.get(0)?,
        path: r.get(1)?,
        file_name: r.get(2)?,
        kind: ImageKind::parse(&r.get::<_, String>(3)?),
        width: r.get(4)?,
        height: r.get(5)?,
        captured_at: r.get(6)?,
        rating: r.get(7)?,
        flag: r.get(8)?,
        thumb_ready: r.get::<_, i64>(9)? == 1,
        missing: r.get(10)?,
        edited: r.get(11)?,
        lat: r.get(12)?,
        lon: r.get(13)?,
        aspect: r.get(14)?,
    })
}

/// Seitenverhältnis der aktuellen Bearbeitung: eine ungerade Zahl von
/// Vierteldrehungen tauscht Breite und Höhe, der Zuschnitt ist normiert auf
/// das gedrehte Bild. Für Raster und Filmstreifen.
const EDIT_ASPECT: &str = "(SELECT
        (CASE WHEN json_extract(e.params, '$.rotate') % 2 = 1 THEN i.height ELSE i.width END)
        * COALESCE(json_extract(e.params, '$.crop.w'), 1.0)
        / ((CASE WHEN json_extract(e.params, '$.rotate') % 2 = 1 THEN i.width ELSE i.height END)
           * COALESCE(json_extract(e.params, '$.crop.h'), 1.0))
     FROM edits e
     WHERE e.image_id = i.id AND e.is_current = 1 AND i.width > 0 AND i.height > 0)";

fn edit_from_row(r: &Row) -> rusqlite::Result<Edit> {
    let json: String = r.get(3)?;
    Ok(Edit {
        id: r.get(0)?,
        image_id: r.get(1)?,
        name: r.get(2)?,
        params: serde_json::from_str(&json).unwrap_or(serde_json::Value::Null),
        is_current: r.get(4)?,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
    })
}

/// Bedingung für Fotos in Ordner `p` (normiert), mit Unterordnern als
/// Bereichsabfrage statt substr(): nutzt den Index auf `dir`. Alle Pfade mit
/// Präfix „…/Ordner/“ liegen zwischen dem Präfix und dem Präfix mit um eins
/// erhöhtem Trennzeichen. Laufwerkswurzeln („/“, „C:\“) enden schon auf den
/// Trenner – `child_prefix` hängt dann keinen zweiten an.
fn known_row(r: &Row) -> rusqlite::Result<KnownImage> {
    Ok(KnownImage {
        id: r.get(0)?,
        path: r.get(1)?,
        size: r.get(2)?,
        mtime: r.get(3)?,
        quick_hash: r.get(4)?,
        missing: r.get(5)?,
    })
}

fn dir_filter(p: &str, recursive: bool, args: &mut Vec<Value>) -> String {
    if !recursive {
        args.push(Value::Text(p.to_string()));
        return "i.dir = ?".into();
    }
    let (prefix, upper) = subtree_range(p);
    args.extend([
        Value::Text(p.to_string()),
        Value::Text(prefix),
        Value::Text(upper),
    ]);
    "(i.dir = ? OR (i.dir >= ? AND i.dir < ?))".into()
}

/// Grenzen `[prefix, upper)` für alle Pfade unterhalb von `dir`: `upper` ist
/// `prefix` mit dem Trenner um eins erhöht. So nutzt die Abfrage den Index
/// auf dem Pfad als Bereich, statt jede Zeile per `LIKE`/`substr` zu prüfen.
fn subtree_range(dir: &str) -> (String, String) {
    let prefix = paths::child_prefix(dir);
    let mut upper = prefix.clone();
    let last = upper.pop().unwrap_or('/');
    upper.push(char::from_u32(last as u32 + 1).unwrap_or(char::MAX));
    (prefix, upper)
}

/// Projektname ohne Leerraum am Rand; leer ist ungültig.
fn project_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Invalid(
            tr_s(
                "Projektname darf nicht leer sein",
                "Project name must not be empty",
            )
            .into(),
        ));
    }
    Ok(name)
}

fn escape_like(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('%', r"\%")
        .replace('_', r"\_")
}

fn build_tree(path: &str, name: &str, nodes: &BTreeMap<String, i64>) -> FolderNode {
    let prefix = paths::child_prefix(path);
    // Direkte Kinder: Einträge unter `prefix` ohne weiteren Trenner.
    let mut children: Vec<FolderNode> = nodes
        .range(prefix.clone()..)
        .take_while(|(k, _)| k.starts_with(&prefix))
        .filter(|(k, _)| !k[prefix.len()..].contains(SEP))
        .map(|(k, _)| build_tree(k, &k[prefix.len()..], nodes))
        .collect();
    children.sort_by_key(|c| c.name.to_lowercase());
    let count = nodes.get(path).copied().unwrap_or(0);
    let total = count + children.iter().map(|c| c.total).sum::<i64>();
    FolderNode {
        path: path.to_string(),
        name: name.to_string(),
        count,
        total,
        children,
    }
}

/// Hilfen für Tests anderer Module.
#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    /// Suchindex exakt wie `search_source`, nichts mehr vorgemerkt – ohne dass
    /// eine Suche (die vorsorglich nachführt) dazwischen lief.
    pub fn exact(cat: &Catalog, step: &str) {
        let q = |sql: &str| -> i64 { cat.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(
            q("SELECT COUNT(*) FROM search_dirty"),
            0,
            "{step}: vorgemerkt"
        );
        assert_eq!(
            q(
                "SELECT COUNT(*) FROM search_source s LEFT JOIN search x ON x.rowid = s.id
               WHERE x.body IS NOT s.body"
            ),
            0,
            "{step}: veraltet"
        );
        assert_eq!(
            q("SELECT COUNT(*) FROM search WHERE rowid NOT IN (SELECT id FROM images)"),
            0,
            "{step}: verwaist"
        );
    }

    /// Legt `n` Bilder mit fertigem Thumbnail an und liefert ihre IDs.
    pub fn insert_dummy_images(cat: &mut Catalog, n: usize) -> Vec<i64> {
        let dir = std::env::temp_dir().join(format!("lumenkorn-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = cat.add_root(&dir).unwrap();
        cat.with_tx(|tx| {
            let mut ids = Vec::new();
            for i in 0..n {
                let img = NewImage {
                    root_id: root.id,
                    path: format!("/tmp/lumenkorn-test/bild{i}.jpg"),
                    dir: "/tmp/lumenkorn-test".into(),
                    file_name: format!("bild{i}.jpg"),
                    ext: "jpg".into(),
                    kind: ImageKind::Raster,
                    size: 1,
                    mtime: 0,
                    quick_hash: format!("h{i}"),
                    meta: Default::default(),
                };
                ids.push(Catalog::insert_image(tx, &img)?);
            }
            tx.execute("UPDATE images SET thumb_state = 1", [])?;
            Ok(ids)
        })
        .unwrap()
    }
}
