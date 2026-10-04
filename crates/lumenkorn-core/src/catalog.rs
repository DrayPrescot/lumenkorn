//! Der Katalog: eine SQLite-Datenbank mit Quellordnern, Bildern, Projekten,
//! Schlagworten und Bearbeitungen. Bilddateien selbst werden nie verändert.

use crate::error::{Error, Result};
use crate::formats::ImageKind;
use crate::metadata::Metadata;
use crate::model::*;
use crate::paths::{self, SEP};
use lumenkorn_develop::DevelopParams;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Schema-Migrationen. Index + 1 = `user_version` nach Anwendung.
/// Neue Migrationen nur hinten anhängen, bestehende nie verändern.
const MIGRATIONS: &[&str] = &[
    // v1: Grundschema
    r#"
    CREATE TABLE roots (
        id          INTEGER PRIMARY KEY,
        path        TEXT NOT NULL UNIQUE,
        added_at    INTEGER NOT NULL,
        last_scan   INTEGER
    );

    CREATE TABLE images (
        id           INTEGER PRIMARY KEY,
        root_id      INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
        path         TEXT NOT NULL UNIQUE,
        dir          TEXT NOT NULL,
        file_name    TEXT NOT NULL,
        ext          TEXT NOT NULL,
        kind         TEXT NOT NULL,
        size         INTEGER NOT NULL,
        mtime        INTEGER NOT NULL,
        quick_hash   TEXT NOT NULL,
        width        INTEGER,
        height       INTEGER,
        orientation  INTEGER,
        captured_at  TEXT,
        camera       TEXT,
        lens         TEXT,
        iso          INTEGER,
        exposure     TEXT,
        aperture     REAL,
        focal_length REAL,
        rating       INTEGER NOT NULL DEFAULT 0,
        flag         INTEGER NOT NULL DEFAULT 0,   -- -1 abgelehnt, 0 keine, 1 markiert
        color_label  TEXT,
        missing      INTEGER NOT NULL DEFAULT 0,
        thumb_state  INTEGER NOT NULL DEFAULT 0,   -- 0 ausstehend, 1 fertig, 2 fehlgeschlagen
        ai_state     INTEGER NOT NULL DEFAULT 0,   -- 0 ausstehend, 1 verschlagwortet, 2 fehlgeschlagen
        added_at     INTEGER NOT NULL
    );
    CREATE INDEX images_root ON images(root_id);
    CREATE INDEX images_dir  ON images(dir);
    CREATE INDEX images_hash ON images(quick_hash);
    CREATE INDEX images_captured ON images(captured_at);
    CREATE INDEX images_thumb ON images(thumb_state) WHERE thumb_state = 0;

    CREATE TABLE projects (
        id          INTEGER PRIMARY KEY,
        name        TEXT NOT NULL,
        description TEXT NOT NULL DEFAULT '',
        created_at  INTEGER NOT NULL,
        updated_at  INTEGER NOT NULL
    );

    -- Ein Projekt verweist auf Ordner per Pfad, nicht auf Bilder. Neue Bilder
    -- in diesen Ordnern gehören damit automatisch zum Projekt.
    CREATE TABLE project_folders (
        project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        path        TEXT NOT NULL,
        recursive   INTEGER NOT NULL DEFAULT 1,
        PRIMARY KEY (project_id, path)
    );

    CREATE TABLE tags (
        id         INTEGER PRIMARY KEY,
        name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
        parent_id  INTEGER REFERENCES tags(id) ON DELETE SET NULL
    );

    CREATE TABLE image_tags (
        image_id   INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
        tag_id     INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
        source     TEXT NOT NULL CHECK (source IN ('manual', 'ai')),
        confidence REAL,
        PRIMARY KEY (image_id, tag_id)
    );
    CREATE INDEX image_tags_tag ON image_tags(tag_id);

    -- Non-destruktive Bearbeitungen: Parametersatz als JSON. Genau eine
    -- Bearbeitung pro Bild ist „aktuell“, weitere sind Snapshots.
    CREATE TABLE edits (
        id          INTEGER PRIMARY KEY,
        image_id    INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
        name        TEXT NOT NULL,
        params      TEXT NOT NULL,
        is_current  INTEGER NOT NULL DEFAULT 0,
        created_at  INTEGER NOT NULL,
        updated_at  INTEGER NOT NULL
    );
    CREATE INDEX edits_image ON edits(image_id);
    CREATE UNIQUE INDEX edits_current ON edits(image_id) WHERE is_current = 1;

    -- Für die semantische Suche mit CLIP/SigLIP (folgt später).
    CREATE TABLE embeddings (
        image_id  INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
        model     TEXT NOT NULL,
        vector    BLOB NOT NULL,
        PRIMARY KEY (image_id, model)
    );
    "#,
    // v2: Personenerkennung
    r#"
    ALTER TABLE images ADD COLUMN face_state INTEGER NOT NULL DEFAULT 0; -- 0 ausstehend, 1 fertig, 2 fehlgeschlagen

    -- Person oder (ohne Namen) automatisch gebildete Gruppe ähnlicher Gesichter
    CREATE TABLE persons (
        id          INTEGER PRIMARY KEY,
        name        TEXT COLLATE NOCASE,
        created_at  INTEGER NOT NULL
    );
    CREATE UNIQUE INDEX persons_name ON persons(name) WHERE name IS NOT NULL;

    CREATE TABLE faces (
        id              INTEGER PRIMARY KEY,
        image_id        INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
        -- normiert auf das unbearbeitete, aufrecht gedrehte Bild
        x REAL NOT NULL, y REAL NOT NULL, w REAL NOT NULL, h REAL NOT NULL,
        score           REAL NOT NULL,
        embedding       BLOB NOT NULL,
        person_id       INTEGER REFERENCES persons(id) ON DELETE SET NULL,
        confirmed       INTEGER NOT NULL DEFAULT 0, -- vom Nutzer zugeordnet/bestätigt
        rejected_person INTEGER,                    -- „nicht diese Person“: nicht erneut vorschlagen
        hidden          INTEGER NOT NULL DEFAULT 0  -- ignoriert (kein Gesicht, unwichtig)
    );
    CREATE INDEX faces_image ON faces(image_id);
    CREATE INDEX faces_person ON faces(person_id);
    "#,
    // v3: Warteschlangen der Hintergrundanalyse ohne Tabellendurchlauf
    r#"
    CREATE INDEX images_ai_pending ON images(id) WHERE ai_state = 0;
    CREATE INDEX images_faces_pending ON images(id) WHERE face_state = 0;
    "#,
    // v4: Volltextindex für die Suche (siehe `SEARCH_INDEX`)
    SEARCH_INDEX,
    // v5: Ordnernamen bei Laufwerken als Quellordner (`E:\`) nicht kürzen
    r#"
    DROP VIEW search_source;
    CREATE VIEW search_source(id, body) AS
    SELECT i.id,
           i.file_name
           || ' ' || COALESCE((SELECT substr(i.dir, length(r.path)
                                   + CASE WHEN substr(r.path, -1) IN ('/', '\') THEN 1 ELSE 2 END)
                               FROM roots r WHERE r.id = i.root_id), '')
           || ' ' || COALESCE(i.camera, '') || ' ' || COALESCE(i.lens, '')
           || ' ' || COALESCE((SELECT group_concat(t.name, ' ') FROM image_tags it
                               JOIN tags t ON t.id = it.tag_id WHERE it.image_id = i.id), '')
           || ' ' || COALESCE((SELECT group_concat(p.name, ' ') FROM faces f
                               JOIN persons p ON p.id = f.person_id
                               WHERE f.image_id = i.id AND f.hidden = 0 AND p.name IS NOT NULL), '')
    FROM images i;
    DELETE FROM search;
    INSERT INTO search(rowid, body) SELECT id, body FROM search_source;
    "#,
    // v6: Sortierung nach Aufnahmedatum über einen Index statt jedes Mal alle
    // Fotos zu sortieren (Ausdrücke exakt wie in `query_images`)
    r#"
    CREATE INDEX images_sort_captured_asc ON images(COALESCE(captured_at, '9999'), file_name);
    CREATE INDEX images_sort_captured_desc ON images(COALESCE(captured_at, '0000'), file_name);
    "#,
    // v7: IDs gelöschter Fotos nie wiederverwenden. Ohne AUTOINCREMENT vergibt
    // SQLite nach dem Löschen der neuesten Fotos deren IDs erneut – Browser-
    // und Vollbild-Cache zeigten dann das gelöschte Bild unter der neuen ID.
    r#"
    CREATE TABLE image_id_floor (v INTEGER NOT NULL);
    INSERT INTO image_id_floor (v) SELECT COALESCE(MAX(id), 0) FROM images;
    CREATE TRIGGER image_id_floor_del AFTER DELETE ON images BEGIN
        UPDATE image_id_floor SET v = MAX(v, OLD.id);
    END;
    "#,
    // v8: Ordner auf der Platte – auch ohne (unterstützte) Bilder im Ordnerbaum
    r#"
    CREATE TABLE folders (
        root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
        path    TEXT NOT NULL,
        PRIMARY KEY (root_id, path)
    );
    "#,
];

/// Volltextindex der Suche: je Foto ein Eintrag aus Dateiname, Ordner (relativ
/// zum Quellordner), Kamera, Objektiv, Schlagworten und Personennamen.
///
/// Trigramme statt Wörter: So findet „sonnen“ wie bisher „Sonnenuntergang“,
/// aber über den Index statt durch Lesen jeder Zeile. Groß-/Kleinschreibung
/// (auch Umlaute) und Akzente spielen keine Rolle. Trigger halten den Index
/// bei jeder Änderung aktuell – kein Code muss daran denken.
const SEARCH_INDEX: &str = r#"
    CREATE VIRTUAL TABLE search USING fts5(body, tokenize = 'trigram remove_diacritics 1');

    CREATE VIEW search_source(id, body) AS
    SELECT i.id,
           i.file_name
           || ' ' || COALESCE((SELECT substr(i.dir, length(r.path) + 2) FROM roots r WHERE r.id = i.root_id), '')
           || ' ' || COALESCE(i.camera, '') || ' ' || COALESCE(i.lens, '')
           || ' ' || COALESCE((SELECT group_concat(t.name, ' ') FROM image_tags it
                               JOIN tags t ON t.id = it.tag_id WHERE it.image_id = i.id), '')
           || ' ' || COALESCE((SELECT group_concat(p.name, ' ') FROM faces f
                               JOIN persons p ON p.id = f.person_id
                               WHERE f.image_id = i.id AND f.hidden = 0 AND p.name IS NOT NULL), '')
    FROM images i;

    INSERT INTO search(rowid, body) SELECT id, body FROM search_source;

    -- Bilder
    CREATE TRIGGER search_img_ins AFTER INSERT ON images BEGIN
        INSERT INTO search(rowid, body) SELECT id, body FROM search_source WHERE id = NEW.id;
    END;
    CREATE TRIGGER search_img_upd AFTER UPDATE OF file_name, dir, camera, lens ON images BEGIN
        DELETE FROM search WHERE rowid = NEW.id;
        INSERT INTO search(rowid, body) SELECT id, body FROM search_source WHERE id = NEW.id;
    END;
    CREATE TRIGGER search_img_del AFTER DELETE ON images BEGIN
        DELETE FROM search WHERE rowid = OLD.id;
    END;

    -- Schlagworte
    CREATE TRIGGER search_tag_ins AFTER INSERT ON image_tags BEGIN
        DELETE FROM search WHERE rowid = NEW.image_id;
        INSERT INTO search(rowid, body) SELECT id, body FROM search_source WHERE id = NEW.image_id;
    END;
    CREATE TRIGGER search_tag_del AFTER DELETE ON image_tags BEGIN
        DELETE FROM search WHERE rowid = OLD.image_id;
        INSERT INTO search(rowid, body) SELECT id, body FROM search_source WHERE id = OLD.image_id;
    END;
    CREATE TRIGGER search_tag_ren AFTER UPDATE OF name ON tags BEGIN
        DELETE FROM search WHERE rowid IN (SELECT image_id FROM image_tags WHERE tag_id = NEW.id);
        INSERT INTO search(rowid, body) SELECT id, body FROM search_source
            WHERE id IN (SELECT image_id FROM image_tags WHERE tag_id = NEW.id);
    END;

    -- Personen
    CREATE TRIGGER search_face_ins AFTER INSERT ON faces WHEN NEW.person_id IS NOT NULL BEGIN
        DELETE FROM search WHERE rowid = NEW.image_id;
        INSERT INTO search(rowid, body) SELECT id, body FROM search_source WHERE id = NEW.image_id;
    END;
    CREATE TRIGGER search_face_upd AFTER UPDATE OF person_id, hidden ON faces BEGIN
        DELETE FROM search WHERE rowid = NEW.image_id;
        INSERT INTO search(rowid, body) SELECT id, body FROM search_source WHERE id = NEW.image_id;
    END;
    CREATE TRIGGER search_face_del AFTER DELETE ON faces WHEN OLD.person_id IS NOT NULL BEGIN
        DELETE FROM search WHERE rowid = OLD.image_id;
        INSERT INTO search(rowid, body) SELECT id, body FROM search_source WHERE id = OLD.image_id;
    END;
    CREATE TRIGGER search_person_ren AFTER UPDATE OF name ON persons BEGIN
        DELETE FROM search WHERE rowid IN (SELECT image_id FROM faces WHERE person_id = NEW.id);
        INSERT INTO search(rowid, body) SELECT id, body FROM search_source
            WHERE id IN (SELECT DISTINCT image_id FROM faces WHERE person_id = NEW.id);
    END;
"#;

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
        Ok(cat)
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Quellordner
    // ------------------------------------------------------------------

    pub fn add_root(&self, path: &Path) -> Result<Root> {
        if !path.is_dir() {
            return Err(Error::Invalid(format!(
                "{} ist kein Ordner",
                path.display()
            )));
        }
        let p = paths::normalize_dir(path);

        // Verschachtelte Quellordner würden Bilder doppelt erfassen.
        for r in self.roots()? {
            if paths::is_within(&p, &r.path) {
                return Err(Error::Invalid(format!(
                    "{p} liegt bereits in Quellordner {}",
                    r.path
                )));
            }
            if paths::is_within(&r.path, &p) {
                return Err(Error::Invalid(format!(
                    "{p} enthält den bestehenden Quellordner {}. Bitte diesen zuerst entfernen.",
                    r.path
                )));
            }
        }

        self.conn.execute(
            "INSERT INTO roots (path, added_at) VALUES (?1, ?2)",
            params![p, now()],
        )?;
        self.root(self.conn.last_insert_rowid())
    }

    /// Entfernt den Quellordner aus dem Katalog. Die Dateien bleiben unangetastet.
    pub fn remove_root(&self, id: i64) -> Result<()> {
        let n = self.conn.execute("DELETE FROM roots WHERE id = ?1", [id])?;
        if n == 0 {
            return Err(Error::NotFound(format!("Quellordner {id}")));
        }
        Ok(())
    }

    pub fn roots(&self) -> Result<Vec<Root>> {
        let mut st = self.conn.prepare(
            "SELECT r.id, r.path, r.last_scan,
                    (SELECT COUNT(*) FROM images i WHERE i.root_id = r.id AND i.missing = 0)
             FROM roots r ORDER BY r.path",
        )?;
        let rows = st.query_map([], root_from_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn root(&self, id: i64) -> Result<Root> {
        self.conn
            .query_row(
                "SELECT r.id, r.path, r.last_scan,
                        (SELECT COUNT(*) FROM images i WHERE i.root_id = r.id AND i.missing = 0)
                 FROM roots r WHERE r.id = ?1",
                [id],
                root_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("Quellordner {id}")))
    }

    /// Ordnerbaum eines Quellordners, aufgebaut aus den Ordnern mit Bildern.
    pub fn folder_tree(&self, root_id: i64) -> Result<FolderNode> {
        let root = self.root(root_id)?;
        let mut st = self.conn.prepare(
            "SELECT dir, COUNT(*) FROM images WHERE root_id = ?1 AND missing = 0 GROUP BY dir",
        )?;
        let counts: Vec<(String, i64)> = st
            .query_map([root_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;

        // Pfad -> direkte Anzahl. Zwischenordner ohne Bilder werden mit 0 ergänzt.
        let mut nodes: BTreeMap<String, i64> = BTreeMap::new();
        nodes.insert(root.path.clone(), 0);
        // Ordner von der Platte (auch leere) mit 0 Fotos
        let folders: Vec<String> = self
            .conn
            .prepare_cached("SELECT path FROM folders WHERE root_id = ?1")?
            .query_map([root_id], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let counts = counts
            .into_iter()
            .chain(folders.into_iter().map(|f| (f, 0)));
        for (dir, n) in counts {
            if !paths::is_within(&dir, &root.path) {
                continue;
            }
            *nodes.entry(dir.clone()).or_default() += n;
            let mut cur = Path::new(&dir)
                .parent()
                .map(|p| p.to_string_lossy().into_owned());
            while let Some(p) = cur {
                if !paths::is_within(&p, &root.path) || nodes.contains_key(&p) {
                    break;
                }
                nodes.insert(p.clone(), 0);
                cur = Path::new(&p)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned());
            }
        }
        Ok(build_tree(&root.path, &root.name, &nodes))
    }

    /// Ordner eines Quellordners nach einem vollständigen Scan ersetzen.
    /// `true`, wenn sich etwas geändert hat.
    pub(crate) fn set_folders(&mut self, root_id: i64, dirs: &[String]) -> Result<bool> {
        let before: std::collections::HashSet<String> = self
            .conn
            .prepare("SELECT path FROM folders WHERE root_id = ?1")?
            .query_map([root_id], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let after: std::collections::HashSet<String> = dirs.iter().cloned().collect();
        if before == after {
            return Ok(false);
        }
        self.with_tx(|tx| {
            tx.execute("DELETE FROM folders WHERE root_id = ?1", [root_id])?;
            let mut st =
                tx.prepare("INSERT OR IGNORE INTO folders (root_id, path) VALUES (?1, ?2)")?;
            for d in &after {
                st.execute(params![root_id, d])?;
            }
            Ok(())
        })?;
        Ok(true)
    }

    /// Ordner nach gemeldeten Änderungen: `added` neu bzw. noch vorhanden,
    /// `gone` samt allem darunter entfernt. `true`, wenn sich etwas geändert hat.
    pub(crate) fn update_folders(
        &mut self,
        root_id: i64,
        added: &[String],
        gone: &[String],
    ) -> Result<bool> {
        if added.is_empty() && gone.is_empty() {
            return Ok(false);
        }
        let sep = SEP.to_string();
        self.with_tx(|tx| {
            let mut n = 0;
            for g in gone {
                // Der Ordner selbst und alles darunter (Bereichsabfrage wie beim Ordnerfilter)
                let prefix = format!("{}{sep}", paths::trim_dir(g));
                let upper = format!("{}{}", paths::trim_dir(g), char::from_u32(SEP as u32 + 1).unwrap_or('~'));
                n += tx.execute(
                    "DELETE FROM folders WHERE root_id = ?1 AND (path = ?2 OR (path >= ?3 AND path < ?4))",
                    params![root_id, paths::trim_dir(g), prefix, upper],
                )?;
            }
            let mut st = tx.prepare("INSERT OR IGNORE INTO folders (root_id, path) VALUES (?1, ?2)")?;
            for d in added {
                n += st.execute(params![root_id, d])?;
            }
            Ok(n > 0)
        })
    }

    // ------------------------------------------------------------------
    // Bilder – Schreibzugriffe für den Scanner
    // ------------------------------------------------------------------

    pub(crate) fn known_images(&self, root_id: i64) -> Result<Vec<KnownImage>> {
        let mut st = self.conn.prepare(
            "SELECT id, path, size, mtime, quick_hash, missing FROM images WHERE root_id = ?1",
        )?;
        let rows = st.query_map([root_id], |r| {
            Ok(KnownImage {
                id: r.get(0)?,
                path: r.get(1)?,
                size: r.get(2)?,
                mtime: r.get(3)?,
                quick_hash: r.get(4)?,
                missing: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Führt mehrere Schreibvorgänge in einer Transaktion aus.
    pub(crate) fn with_tx<T>(
        &mut self,
        f: impl FnOnce(&rusqlite::Transaction) -> Result<T>,
    ) -> Result<T> {
        let tx = self.conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    pub(crate) fn insert_image(tx: &rusqlite::Transaction, img: &NewImage) -> Result<i64> {
        let m = &img.meta;
        tx.execute(
            "INSERT INTO images (id, root_id, path, dir, file_name, ext, kind, size, mtime, quick_hash,
                                 width, height, orientation, captured_at, camera, lens, iso,
                                 exposure, aperture, focal_length, added_at)
             VALUES (MAX(COALESCE((SELECT MAX(id) FROM images), 0),
                         COALESCE((SELECT v FROM image_id_floor), 0)) + 1,
                     ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
            params![
                img.root_id, img.path, img.dir, img.file_name, img.ext, img.kind.as_str(), img.size,
                img.mtime, img.quick_hash, m.width, m.height, m.orientation, m.captured_at, m.camera,
                m.lens, m.iso, m.exposure, m.aperture, m.focal_length, now()
            ],
        )?;
        Ok(tx.last_insert_rowid())
    }

    /// Datei hat sich inhaltlich geändert: Metadaten neu, Vorschau neu erzeugen.
    pub(crate) fn update_image_content(
        tx: &rusqlite::Transaction,
        id: i64,
        img: &NewImage,
    ) -> Result<()> {
        let m = &img.meta;
        tx.execute(
            "UPDATE images SET size = ?2, mtime = ?3, quick_hash = ?4, width = ?5, height = ?6,
                    orientation = ?7, captured_at = ?8, camera = ?9, lens = ?10, iso = ?11,
                    exposure = ?12, aperture = ?13, focal_length = ?14,
                    missing = 0, thumb_state = 0, ai_state = 0, face_state = 0
             WHERE id = ?1",
            params![
                id,
                img.size,
                img.mtime,
                img.quick_hash,
                m.width,
                m.height,
                m.orientation,
                m.captured_at,
                m.camera,
                m.lens,
                m.iso,
                m.exposure,
                m.aperture,
                m.focal_length
            ],
        )?;
        Ok(())
    }

    /// Datei wurde verschoben oder umbenannt. ID, Bewertung, Schlagworte und
    /// Bearbeitungen bleiben erhalten.
    pub(crate) fn move_image(tx: &rusqlite::Transaction, id: i64, img: &NewImage) -> Result<()> {
        tx.execute(
            "UPDATE images SET path = ?2, dir = ?3, file_name = ?4, ext = ?5, mtime = ?6, missing = 0
             WHERE id = ?1",
            params![id, img.path, img.dir, img.file_name, img.ext, img.mtime],
        )?;
        Ok(())
    }

    pub(crate) fn set_missing(tx: &rusqlite::Transaction, id: i64, missing: bool) -> Result<()> {
        tx.execute(
            "UPDATE images SET missing = ?2 WHERE id = ?1",
            params![id, missing],
        )?;
        Ok(())
    }

    pub(crate) fn mark_scanned(tx: &rusqlite::Transaction, root_id: i64) -> Result<()> {
        tx.execute(
            "UPDATE roots SET last_scan = ?2 WHERE id = ?1",
            params![root_id, now()],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Bilder – Abfragen
    // ------------------------------------------------------------------

    pub fn query_images(&self, q: &ImageQuery) -> Result<Vec<ImageSummary>> {
        let (where_sql, mut args) = self.where_clause(q)?;
        let order = match q.sort {
            SortOrder::CapturedAsc => "COALESCE(i.captured_at, '9999') ASC, i.file_name ASC",
            SortOrder::CapturedDesc => "COALESCE(i.captured_at, '0000') DESC, i.file_name DESC",
            SortOrder::NameAsc => "i.file_name COLLATE NOCASE ASC, i.path ASC",
            SortOrder::RatingDesc => "i.rating DESC, COALESCE(i.captured_at, '9999') ASC",
        };
        let mut sql = format!(
            "SELECT i.id, i.path, i.file_name, i.kind, i.width, i.height, i.captured_at,
                    i.rating, i.flag, i.thumb_state, i.missing,
                    EXISTS (SELECT 1 FROM edits e WHERE e.image_id = i.id AND e.is_current = 1)
             FROM images i WHERE {where_sql} ORDER BY {order}"
        );
        if let Some(limit) = q.limit {
            sql.push_str(" LIMIT ? OFFSET ?");
            args.push(Value::Integer(limit));
            args.push(Value::Integer(q.offset.unwrap_or(0)));
        }
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map(params_from_iter(args), summary_from_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn count_images(&self, scope: &Scope) -> Result<i64> {
        let (where_sql, args) = self.where_clause(&ImageQuery::new(scope.clone()))?;
        Ok(self.conn.query_row(
            &format!("SELECT COUNT(*) FROM images i WHERE {where_sql}"),
            params_from_iter(args),
            |r| r.get(0),
        )?)
    }

    fn where_clause(&self, q: &ImageQuery) -> Result<(String, Vec<Value>)> {
        let mut parts = vec!["i.missing = 0".to_string()];
        let mut args: Vec<Value> = Vec::new();
        let sep = SEP.to_string();

        match &q.scope {
            Scope::All => {}
            Scope::Folder { path, recursive } => {
                let p = paths::trim_dir(path);
                if *recursive {
                    // Bereichsabfrage statt substr(): nutzt den Index auf `dir`.
                    // Alle Pfade mit Präfix „…/Ordner/“ liegen zwischen dem Präfix
                    // und dem Präfix mit um eins erhöhtem Trennzeichen.
                    parts.push("(i.dir = ? OR (i.dir >= ? AND i.dir < ?))".into());
                    let prefix = paths::child_prefix(&p);
                    let mut upper = prefix.clone();
                    let last = upper.pop().unwrap_or('/');
                    upper.push(char::from_u32(last as u32 + 1).unwrap_or(char::MAX));
                    args.extend([Value::Text(p), Value::Text(prefix), Value::Text(upper)]);
                } else {
                    parts.push("i.dir = ?".into());
                    args.push(Value::Text(p));
                }
            }
            Scope::Person { id } => {
                parts.push(
                    // Von den Gesichtern der Person ausgehen (Index faces_person)
                    "i.id IN (SELECT f.image_id FROM faces f WHERE f.person_id = ? AND f.hidden = 0)"
                        .into(),
                );
                args.push(Value::Integer(*id));
            }
            Scope::Project { id } => {
                // Bilder aus überlappenden Ordnern erscheinen dank EXISTS nur einmal.
                parts.push(
                    "EXISTS (SELECT 1 FROM project_folders pf WHERE pf.project_id = ? AND (
                        i.dir = pf.path OR (pf.recursive = 1 AND substr(i.dir, 1, length(pf.path) + 1)
                            = CASE WHEN substr(pf.path, -1) = ? THEN pf.path ELSE pf.path || ? END)))"
                        .into(),
                );
                args.extend([
                    Value::Integer(*id),
                    Value::Text(sep.clone()),
                    Value::Text(sep),
                ]);
            }
        }

        if let Some(min) = q.min_rating.filter(|m| *m > 0) {
            parts.push("i.rating >= ?".into());
            args.push(Value::Integer(min));
        }
        if let Some(ids) = &q.ids {
            parts.push("i.id IN (SELECT value FROM json_each(?))".into());
            args.push(Value::Text(
                serde_json::to_string(ids).unwrap_or_else(|_| "[]".into()),
            ));
        }

        // Jedes Suchwort muss in Dateiname, Kamera, Objektiv, einem Schlagwort
        // oder dem Namen einer abgebildeten Person vorkommen.
        // Volltextindex: Wörter ab drei Zeichen über Trigramme (Index),
        // kürzere per LIKE auf denselben Einträgen. Alle Wörter müssen passen.
        let words: Vec<&str> = q
            .search
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .collect();
        let (long, short): (Vec<&str>, Vec<&str>) =
            words.into_iter().partition(|w| w.chars().count() >= 3);
        if !long.is_empty() {
            // Jedes Wort als Phrase in Anführungszeichen: keine Operatoren
            let fts = long
                .iter()
                .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" AND ");
            parts.push("i.id IN (SELECT rowid FROM search WHERE search MATCH ?)".into());
            args.push(Value::Text(fts));
        }
        for w in short {
            parts.push(r"i.id IN (SELECT rowid FROM search WHERE body LIKE ? ESCAPE '\')".into());
            args.push(Value::Text(format!("%{}%", escape_like(w))));
        }
        Ok((parts.join(" AND "), args))
    }

    pub fn image(&self, id: i64) -> Result<ImageDetail> {
        let mut d = self
            .conn
            .query_row(
                "SELECT id, path, dir, file_name, ext, size, width, height, captured_at, camera, lens,
                        iso, exposure, aperture, focal_length, rating, flag
                 FROM images WHERE id = ?1",
                [id],
                |r| {
                    Ok(ImageDetail {
                        id: r.get(0)?,
                        path: r.get(1)?,
                        dir: r.get(2)?,
                        file_name: r.get(3)?,
                        ext: r.get(4)?,
                        size: r.get(5)?,
                        width: r.get(6)?,
                        height: r.get(7)?,
                        captured_at: r.get(8)?,
                        camera: r.get(9)?,
                        lens: r.get(10)?,
                        iso: r.get(11)?,
                        exposure: r.get(12)?,
                        aperture: r.get(13)?,
                        focal_length: r.get(14)?,
                        rating: r.get(15)?,
                        flag: r.get(16)?,
                        tags: vec![],
                    })
                },
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("Bild {id}")))?;
        d.tags = self.image_tags(id)?;
        Ok(d)
    }

    pub fn set_rating(&self, ids: &[i64], rating: i64) -> Result<()> {
        if !(0..=5).contains(&rating) {
            return Err(Error::Invalid(
                "Bewertung muss zwischen 0 und 5 liegen".into(),
            ));
        }
        self.update_many(ids, "rating", rating)
    }

    pub fn set_flag(&self, ids: &[i64], flag: i64) -> Result<()> {
        if !(-1..=1).contains(&flag) {
            return Err(Error::Invalid("Markierung muss -1, 0 oder 1 sein".into()));
        }
        self.update_many(ids, "flag", flag)
    }

    /// Abgelehnte Bilder aus `ids` (Datei vorhanden, Markierung noch −1):
    /// (ID, Pfad, Größe in Bytes). Andere IDs werden stillschweigend übergangen –
    /// gelöscht wird nur, was zum Zeitpunkt des Löschens wirklich abgelehnt ist.
    pub fn rejected_files(&self, ids: &[i64]) -> Result<Vec<(i64, String, i64)>> {
        let mut st = self.conn.prepare(
            "SELECT id, path, size FROM images WHERE id = ?1 AND flag = -1 AND missing = 0",
        )?;
        let mut out = Vec::new();
        for id in ids {
            if let Some(r) = st
                .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .optional()?
            {
                out.push(r);
            }
        }
        Ok(out)
    }

    /// Entfernt Bilder aus dem Katalog (samt Bearbeitungen, Schlagworten,
    /// Gesichtern). Liefert die IDs der entfernten Gesichter, damit deren
    /// Ausschnitte aufgeräumt werden können.
    pub fn forget_images(&mut self, ids: &[i64]) -> Result<Vec<i64>> {
        let faces = self.with_tx(|tx| {
            let mut faces = Vec::new();
            {
                let mut st = tx.prepare("SELECT id FROM faces WHERE image_id = ?1")?;
                for id in ids {
                    for r in st.query_map([id], |r| r.get::<_, i64>(0))? {
                        faces.push(r?);
                    }
                }
            }
            for id in ids {
                tx.execute("DELETE FROM images WHERE id = ?1", [id])?;
            }
            Ok(faces)
        })?;
        self.drop_empty_groups()?;
        Ok(faces)
    }

    fn update_many(&self, ids: &[i64], column: &str, value: i64) -> Result<()> {
        // Eine Transaktion statt eines Commits je Bild (Strg+A, 50 000 Fotos)
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut st =
                tx.prepare_cached(&format!("UPDATE images SET {column} = ?2 WHERE id = ?1"))?;
            for id in ids {
                st.execute(params![id, value])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Vorschauen
    // ------------------------------------------------------------------

    pub fn pending_thumbs(&self, limit: usize) -> Result<Vec<ThumbJob>> {
        let mut st = self.conn.prepare(&format!(
            "{THUMB_JOB_SELECT} WHERE i.thumb_state = 0 AND i.missing = 0 ORDER BY i.id LIMIT ?1"
        ))?;
        let rows = st.query_map([limit as i64], thumb_job_from_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn thumb_job(&self, id: i64) -> Result<ThumbJob> {
        self.conn
            .prepare_cached(&format!("{THUMB_JOB_SELECT} WHERE i.id = ?1"))?
            .query_row([id], thumb_job_from_row)
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("Bild {id}")))
    }

    /// Fehlgeschlagene Thumbnails erneut versuchen (z. B. nach kurzer
    /// NAS-Unterbrechung). Ohne Thumbnail übergehen Verschlagwortung und
    /// Gesichtserkennung ein Bild sonst für immer. Liefert die Anzahl.
    pub fn retry_failed_thumbs(&self) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE images SET thumb_state = 0 WHERE thumb_state = 2 AND missing = 0",
            [],
        )?)
    }

    /// Quellordner eines Fotos.
    pub fn image_root(&self, id: i64) -> Result<i64> {
        self.conn
            .prepare_cached("SELECT root_id FROM images WHERE id = ?1")?
            .query_row([id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("Bild {id}")))
    }

    /// Für ein zusammengefügtes Bild (HDR, Panorama, Focus Stacking): Aufnahme-
    /// daten vom Referenzfoto `from` übernehmen – die EXR-Datei hat keine.
    /// Liefert die ID des Bildes unter `path`, falls es im Katalog ist.
    pub fn adopt_capture_info(&self, path: &str, from: i64) -> Result<Option<i64>> {
        let id: Option<i64> = self
            .conn
            .query_row("SELECT id FROM images WHERE path = ?1", [path], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(id) = id {
            self.conn.execute(
                "UPDATE images SET (captured_at, camera, lens, iso, exposure, aperture, focal_length) =
                   (SELECT captured_at, camera, lens, iso, exposure, aperture, focal_length
                      FROM images WHERE id = ?2)
                 WHERE id = ?1",
                params![id, from],
            )?;
        }
        Ok(id)
    }

    pub fn set_thumb_state(&self, id: i64, ok: bool) -> Result<()> {
        self.conn
            .prepare_cached("UPDATE images SET thumb_state = ?2 WHERE id = ?1")?
            .execute(params![id, if ok { 1 } else { 2 }])?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Projekte
    // ------------------------------------------------------------------

    pub fn create_project(&self, name: &str, description: &str) -> Result<Project> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Invalid("Projektname darf nicht leer sein".into()));
        }
        let t = now();
        self.conn.execute(
            "INSERT INTO projects (name, description, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![name, description, t],
        )?;
        self.project(self.conn.last_insert_rowid())
    }

    pub fn update_project(&self, id: i64, name: &str, description: &str) -> Result<Project> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Invalid("Projektname darf nicht leer sein".into()));
        }
        let n = self.conn.execute(
            "UPDATE projects SET name = ?2, description = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, name, description, now()],
        )?;
        if n == 0 {
            return Err(Error::NotFound(format!("Projekt {id}")));
        }
        self.project(id)
    }

    pub fn delete_project(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM projects WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Fügt einem Projekt einen Ordner hinzu. Der Ordner kann in einem beliebigen
    /// Quellordner liegen; mehrere Ordner aus verschiedenen Orten sind erlaubt.
    pub fn add_project_folder(
        &self,
        project_id: i64,
        path: &Path,
        recursive: bool,
    ) -> Result<Project> {
        let p = paths::normalize_dir(path);
        self.project(project_id)?;
        self.conn.execute(
            "INSERT INTO project_folders (project_id, path, recursive) VALUES (?1, ?2, ?3)
             ON CONFLICT (project_id, path) DO UPDATE SET recursive = excluded.recursive",
            params![project_id, p, recursive],
        )?;
        self.touch_project(project_id)?;
        self.project(project_id)
    }

    pub fn remove_project_folder(&self, project_id: i64, path: &str) -> Result<Project> {
        self.conn.execute(
            "DELETE FROM project_folders WHERE project_id = ?1 AND path = ?2",
            params![project_id, path],
        )?;
        self.touch_project(project_id)?;
        self.project(project_id)
    }

    fn touch_project(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET updated_at = ?2 WHERE id = ?1",
            params![id, now()],
        )?;
        Ok(())
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        let ids: Vec<i64> = {
            let mut st = self
                .conn
                .prepare("SELECT id FROM projects ORDER BY name COLLATE NOCASE")?;
            st.query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        ids.into_iter().map(|id| self.project(id)).collect()
    }

    pub fn project(&self, id: i64) -> Result<Project> {
        let mut p = self
            .conn
            .query_row(
                "SELECT id, name, description, created_at, updated_at FROM projects WHERE id = ?1",
                [id],
                |r| {
                    Ok(Project {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        description: r.get(2)?,
                        folders: vec![],
                        image_count: 0,
                        created_at: r.get(3)?,
                        updated_at: r.get(4)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("Projekt {id}")))?;

        let mut st = self.conn.prepare(
            "SELECT path, recursive FROM project_folders WHERE project_id = ?1 ORDER BY path",
        )?;
        p.folders = st
            .query_map([id], |r| {
                Ok(ProjectFolder {
                    path: r.get(0)?,
                    recursive: r.get(1)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        p.image_count = self.count_images(&Scope::Project { id })?;
        Ok(p)
    }

    // ------------------------------------------------------------------
    // Schlagworte
    // ------------------------------------------------------------------

    /// Setzt ein Schlagwort. Ein manuelles Schlagwort wird nie durch ein
    /// KI-Schlagwort überschrieben.
    pub fn add_tag(
        &self,
        image_id: i64,
        name: &str,
        source: TagSource,
        confidence: Option<f64>,
    ) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Invalid("Schlagwort darf nicht leer sein".into()));
        }
        self.conn.execute(
            "INSERT INTO tags (name) VALUES (?1) ON CONFLICT (name) DO NOTHING",
            [name],
        )?;
        let tag_id: i64 =
            self.conn
                .query_row("SELECT id FROM tags WHERE name = ?1", [name], |r| r.get(0))?;
        self.conn.execute(
            "INSERT INTO image_tags (image_id, tag_id, source, confidence) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (image_id, tag_id) DO UPDATE SET
                 source = CASE WHEN image_tags.source = 'manual' THEN 'manual' ELSE excluded.source END,
                 confidence = CASE WHEN image_tags.source = 'manual' THEN image_tags.confidence
                                   ELSE excluded.confidence END",
            params![image_id, tag_id, source.as_str(), confidence],
        )?;
        Ok(())
    }

    pub fn remove_tag(&self, image_id: i64, name: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM image_tags WHERE image_id = ?1 AND tag_id = (SELECT id FROM tags WHERE name = ?2)",
            params![image_id, name],
        )?;
        Ok(())
    }

    pub fn image_tags(&self, image_id: i64) -> Result<Vec<ImageTag>> {
        let mut st = self.conn.prepare(
            "SELECT t.name, it.source, it.confidence FROM image_tags it JOIN tags t ON t.id = it.tag_id
             WHERE it.image_id = ?1 ORDER BY it.source DESC, COALESCE(it.confidence, 1) DESC, t.name",
        )?;
        let rows = st.query_map([image_id], |r| {
            let src: String = r.get(1)?;
            Ok(ImageTag {
                name: r.get(0)?,
                source: if src == "ai" {
                    TagSource::Ai
                } else {
                    TagSource::Manual
                },
                confidence: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    // ------------------------------------------------------------------
    // Bearbeitungen (non-destruktiv)
    // ------------------------------------------------------------------

    pub fn current_edit(&self, image_id: i64) -> Result<Option<Edit>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, image_id, name, params, is_current, created_at, updated_at
                 FROM edits WHERE image_id = ?1 AND is_current = 1",
                [image_id],
                edit_from_row,
            )
            .optional()?)
    }

    /// Legt einen benannten Snapshot des aktuellen Zustands an.
    pub fn create_snapshot(&self, image_id: i64, name: &str) -> Result<Edit> {
        let cur = self
            .current_edit(image_id)?
            .ok_or_else(|| Error::Invalid("Bild hat noch keine Bearbeitung".into()))?;
        let t = now();
        self.conn.execute(
            "INSERT INTO edits (image_id, name, params, is_current, created_at, updated_at)
             VALUES (?1, ?2, ?3, 0, ?4, ?4)",
            params![image_id, name, serde_json::to_string(&cur.params)?, t],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(self.conn.query_row(
            "SELECT id, image_id, name, params, is_current, created_at, updated_at FROM edits WHERE id = ?1",
            [id],
            edit_from_row,
        )?)
    }

    pub fn edits(&self, image_id: i64) -> Result<Vec<Edit>> {
        let mut st = self.conn.prepare(
            "SELECT id, image_id, name, params, is_current, created_at, updated_at
             FROM edits WHERE image_id = ?1 ORDER BY is_current DESC, created_at DESC, id DESC",
        )?;
        let rows = st.query_map([image_id], edit_from_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Aktuelle Entwicklungsparameter eines Bildes (Standard, wenn unbearbeitet).
    pub fn develop_params(&self, image_id: i64) -> Result<DevelopParams> {
        Ok(self
            .current_edit(image_id)?
            .map(|e| DevelopParams::from_json(&e.params))
            .unwrap_or_default())
    }

    /// Speichert Entwicklungsparameter für ein oder mehrere Bilder und lässt die
    /// Thumbnails neu erzeugen. Standardwerte entfernen die Bearbeitung, damit
    /// „bearbeitet“ im Raster wirklich etwas bedeutet. Snapshots bleiben erhalten.
    pub fn save_develop(&mut self, image_ids: &[i64], params: &DevelopParams) -> Result<()> {
        let p = params.sanitized();
        let json = serde_json::to_string(&p)?;
        let default = p.is_default();
        let t = now();
        self.with_tx(|tx| {
            for id in image_ids {
                if default {
                    tx.execute("DELETE FROM edits WHERE image_id = ?1 AND is_current = 1", [id])?;
                } else {
                    let n = tx.execute(
                        "UPDATE edits SET params = ?2, updated_at = ?3 WHERE image_id = ?1 AND is_current = 1",
                        params![id, json, t],
                    )?;
                    if n == 0 {
                        tx.execute(
                            "INSERT INTO edits (image_id, name, params, is_current, created_at, updated_at)
                             VALUES (?1, 'Aktuell', ?2, 1, ?3, ?3)",
                            params![id, json, t],
                        )?;
                    }
                }
                tx.execute("UPDATE images SET thumb_state = 0 WHERE id = ?1", [id])?;
            }
            Ok(())
        })
    }

    /// Übernimmt einen Snapshot als aktuelle Bearbeitung.
    pub fn restore_snapshot(&mut self, edit_id: i64) -> Result<DevelopParams> {
        let (image_id, json): (i64, String) = self
            .conn
            .query_row(
                "SELECT image_id, params FROM edits WHERE id = ?1",
                [edit_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("Snapshot {edit_id}")))?;
        let p = DevelopParams::from_json(&serde_json::from_str(&json)?);
        self.save_develop(&[image_id], &p)?;
        Ok(p)
    }

    pub fn delete_snapshot(&self, edit_id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM edits WHERE id = ?1 AND is_current = 0",
            [edit_id],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // KI: Embeddings und automatische Schlagworte
    // ------------------------------------------------------------------

    /// Bilder, die noch analysiert werden müssen (Thumbnail muss bereitstehen).
    pub fn pending_ai(&self, limit: usize) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare(
            "SELECT id FROM images WHERE ai_state = 0 AND thumb_state = 1 AND missing = 0 ORDER BY id LIMIT ?1",
        )?;
        let rows = st.query_map([limit as i64], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// (analysiert, gesamt) – für die Fortschrittsanzeige.
    pub fn ai_counts(&self) -> Result<(i64, i64)> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(ai_state != 0), 0), COUNT(*) FROM images WHERE missing = 0",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }

    /// Speichert das Ergebnis der Analyse. Bisherige KI-Schlagworte des Bildes
    /// werden ersetzt; manuelle bleiben unangetastet.
    pub fn save_ai_result(
        &mut self,
        image_id: i64,
        model: &str,
        embedding: &[u8],
        tags: &[(String, f64)],
    ) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO embeddings (image_id, model, vector) VALUES (?1, ?2, ?3)
                 ON CONFLICT (image_id, model) DO UPDATE SET vector = excluded.vector",
                params![image_id, model, embedding],
            )?;
            tx.execute(
                "DELETE FROM image_tags WHERE image_id = ?1 AND source = 'ai'",
                [image_id],
            )?;
            for (name, confidence) in tags {
                tx.execute(
                    "INSERT INTO tags (name) VALUES (?1) ON CONFLICT (name) DO NOTHING",
                    [name],
                )?;
                tx.execute(
                    "INSERT INTO image_tags (image_id, tag_id, source, confidence)
                     SELECT ?1, id, 'ai', ?3 FROM tags WHERE name = ?2
                     ON CONFLICT (image_id, tag_id) DO NOTHING",
                    params![image_id, name, confidence],
                )?;
            }
            tx.execute("UPDATE images SET ai_state = 1 WHERE id = ?1", [image_id])?;
            Ok(())
        })
    }

    pub fn set_ai_failed(&self, image_id: i64) -> Result<()> {
        self.conn
            .execute("UPDATE images SET ai_state = 2 WHERE id = ?1", [image_id])?;
        Ok(())
    }

    /// Alle Bilder erneut analysieren (z. B. nach einem Modellwechsel).
    pub fn reset_ai(&self) -> Result<()> {
        self.conn.execute("UPDATE images SET ai_state = 0", [])?;
        Ok(())
    }

    pub fn embeddings(&self, model: &str) -> Result<Vec<(i64, Vec<u8>)>> {
        let mut st = self.conn.prepare(
            "SELECT e.image_id, e.vector FROM embeddings e JOIN images i ON i.id = e.image_id
             WHERE e.model = ?1 AND i.missing = 0",
        )?;
        let rows = st.query_map([model], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Bilder in der angegebenen Reihenfolge (z. B. nach Ähnlichkeit sortiert),
    /// beschränkt auf einen Bereich.
    pub fn images_in_order(
        &self,
        ids: &[i64],
        scope: &Scope,
        min_rating: Option<i64>,
    ) -> Result<Vec<ImageSummary>> {
        // Nur die Treffer lesen, nicht alle Fotos des Bereichs (bis 100 000)
        let mut q = ImageQuery::new(scope.clone());
        q.min_rating = min_rating;
        q.ids = Some(ids.to_vec());
        let all = self.query_images(&q)?;
        let by_id: std::collections::HashMap<i64, ImageSummary> =
            all.into_iter().map(|i| (i.id, i)).collect();
        Ok(ids.iter().filter_map(|id| by_id.get(id).cloned()).collect())
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
    })
}

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

#[cfg(test)]
mod cull_tests {
    use super::*;

    #[test]
    fn only_rejected_present_images_are_deletable() {
        let mut cat = Catalog::open_in_memory().unwrap();
        let ids = tests_support::insert_dummy_images(&mut cat, 4);
        cat.set_flag(&ids[..3], -1).unwrap();
        cat.set_flag(&ids[1..2], 1).unwrap(); // wieder angenommen
        cat.conn
            .execute("UPDATE images SET missing = 1 WHERE id = ?1", [ids[2]])
            .unwrap();
        let files = cat.rejected_files(&ids).unwrap();
        assert_eq!(files.iter().map(|f| f.0).collect::<Vec<_>>(), vec![ids[0]]);

        cat.forget_images(&[ids[0]]).unwrap();
        let left: i64 = cat
            .conn
            .query_row("SELECT COUNT(*) FROM images", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 3);
        assert!(cat.rejected_files(&ids).unwrap().is_empty());
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;

    fn hits(cat: &Catalog, text: &str) -> Vec<i64> {
        let mut q = ImageQuery::new(Scope::All);
        q.search = Some(text.into());
        let mut ids: Vec<i64> = cat.query_images(&q).unwrap().iter().map(|i| i.id).collect();
        ids.sort();
        ids
    }

    #[test]
    fn index_follows_every_change() {
        let mut cat = Catalog::open_in_memory().unwrap();
        let ids = tests_support::insert_dummy_images(&mut cat, 3);
        let (a, b, c) = (ids[0], ids[1], ids[2]);

        // Dateiname, Teilwort, kurze Wörter, Sonderzeichen
        assert_eq!(hits(&cat, "bild1"), vec![b]);
        assert_eq!(hits(&cat, "ILD"), ids);
        assert_eq!(hits(&cat, "d2"), vec![c], "zwei Zeichen: über LIKE");
        assert!(
            hits(&cat, "\"; DROP").is_empty(),
            "Anführungszeichen sind kein Operator"
        );

        // Schlagworte: Umlaute ohne Groß-/Kleinschreibung, Akzente egal
        cat.add_tag(a, "Äpfel am Café", TagSource::Manual, None)
            .unwrap();
        assert_eq!(hits(&cat, "äpfel"), vec![a]);
        assert_eq!(hits(&cat, "cafe"), vec![a]);
        assert_eq!(
            hits(&cat, "äpfel bild0"),
            vec![a],
            "alle Wörter müssen passen"
        );
        assert!(hits(&cat, "äpfel bild1").is_empty());
        cat.remove_tag(a, "Äpfel am Café").unwrap();
        assert!(hits(&cat, "äpfel").is_empty());

        // Personen: Name, Umbenennen, Ausblenden
        cat.conn
            .execute_batch(&format!(
                "INSERT INTO persons (id, name, created_at) VALUES (7, 'Grace Hopper', 0);
                 INSERT INTO faces (id, image_id, x, y, w, h, score, embedding, person_id)
                 VALUES (70, {b}, 0, 0, 1, 1, 1, x'00', 7);"
            ))
            .unwrap();
        assert_eq!(hits(&cat, "hopper"), vec![b]);
        cat.conn
            .execute("UPDATE persons SET name = 'Ada Lovelace' WHERE id = 7", [])
            .unwrap();
        assert!(hits(&cat, "hopper").is_empty());
        assert_eq!(hits(&cat, "lovelace"), vec![b]);
        cat.hide_face(70).unwrap();
        assert!(hits(&cat, "lovelace").is_empty());

        // Umbenennen/Verschieben ändert Dateiname und Ordner
        cat.conn
            .execute(
                "UPDATE images SET file_name = 'urlaub.jpg', dir = '/x/Lofoten 2025' WHERE id = ?1",
                [c],
            )
            .unwrap();
        assert_eq!(hits(&cat, "urlaub"), vec![c]);
        assert!(hits(&cat, "bild2").is_empty());

        // Gelöschtes Bild verschwindet aus dem Index
        cat.forget_images(&[a]).unwrap();
        let n: i64 = cat
            .conn
            .query_row("SELECT COUNT(*) FROM search", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn folder_names_are_searchable() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().join("Fotos");
        std::fs::create_dir_all(root_dir.join("Lofoten")).unwrap();
        let mut cat = Catalog::open_in_memory().unwrap();
        let root = cat.add_root(&root_dir).unwrap();
        // Wie der Scanner: Pfade vom normierten Quellordner aus bilden. (Unter
        // Windows kann der Temp-Pfad Kurznamen wie „RUNNER~1“ enthalten, die
        // `canonicalize` auflöst.)
        let sep = std::path::MAIN_SEPARATOR;
        let dir = format!("{}{sep}Lofoten", root.path);
        cat.with_tx(|tx| {
            Catalog::insert_image(
                tx,
                &NewImage {
                    root_id: root.id,
                    path: format!("{dir}{sep}a.jpg"),
                    dir: dir.clone(),
                    file_name: "a.jpg".into(),
                    ext: "jpg".into(),
                    kind: crate::formats::ImageKind::Raster,
                    size: 1,
                    mtime: 0,
                    quick_hash: "h".into(),
                    meta: Default::default(),
                },
            )
        })
        .unwrap();
        assert_eq!(hits(&cat, "lofoten").len(), 1);
        // Quellordner mit Trenner am Ende (Laufwerk „E:\“, „/“): nichts abschneiden
        cat.conn
            .execute(
                "UPDATE roots SET path = ?1",
                [format!("{}{sep}", root.path)],
            )
            .unwrap();
        cat.conn
            .execute_batch(
                "DELETE FROM search; INSERT INTO search(rowid, body) SELECT id, body FROM search_source;",
            )
            .unwrap();
        let body: String = cat
            .conn
            .query_row("SELECT body FROM search", [], |r| r.get(0))
            .unwrap();
        assert!(body.contains(" Lofoten "), "{body}");
        // Der Pfad des Quellordners selbst gehört nicht zum Suchtext
        let tmp_name = tmp
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(hits(&cat, &tmp_name).is_empty());
    }
}
