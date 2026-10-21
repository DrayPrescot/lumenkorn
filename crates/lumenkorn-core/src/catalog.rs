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

/// Schema-Migrationen. Index + 1 = `user_version` nach Anwendung.
/// Neue Migrationen nur hinten anhängen, bestehende nie verändern.
const MIGRATIONS: &[&str] = &[
    // Kommentare im SQL geben den Stand ihrer Version wieder. (`embeddings`
    // aus v1 ist längst in Gebrauch: `save_ai_result`, `embeddings()`.)
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
    // v6: Sortierung nach Aufnahmedatum über einen Index (Ausdrücke exakt wie
    // in `query_images`). Lohnt nur mit `limit`; ohne (die Oberfläche lädt
    // immer alle Fotos) brachte er bei 50 000 Fotos gemessen nichts.
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
    // v9: RAW-Entwicklung mit RCD-Demosaicing und Eingangsschärfung – die
    // Vorschaubilder bearbeiteter RAWs (aus der Entwicklung gerendert) neu
    // erzeugen. Unbearbeitete nutzen das eingebettete Kamera-JPEG.
    r#"
    UPDATE images SET thumb_state = 0
    WHERE kind = 'raw'
      AND id IN (SELECT image_id FROM edits WHERE is_current = 1);
    "#,
    // v10: Gesichts-IDs nie wiederverwenden (wie v7 für Fotos). Die
    // Gesichtsausschnitte `face/{id}` werden dauerhaft gecacht; nach dem Löschen
    // der neuesten Gesichter (Fotos entfernt, Personen neu erkannt) bekamen neue
    // Gesichter sonst deren IDs und die Oberfläche zeigte den alten Ausschnitt.
    r#"
    CREATE TABLE face_id_floor (v INTEGER NOT NULL);
    INSERT INTO face_id_floor (v) SELECT COALESCE(MAX(id), 0) FROM faces;
    CREATE TRIGGER face_id_floor_del AFTER DELETE ON faces BEGIN
        UPDATE face_id_floor SET v = MAX(v, OLD.id);
    END;
    "#,
    // v11: Leistung (siehe `SEARCH_DIRTY`)
    SEARCH_DIRTY,
    // v12: Presets – benannte Einstellungen, die sich auf beliebige Fotos
    // anwenden lassen. `groups` sagt, welche Teile von `params` dazugehören.
    r#"
    CREATE TABLE IF NOT EXISTS presets (
        id          INTEGER PRIMARY KEY,
        name        TEXT NOT NULL,
        params      TEXT NOT NULL,
        groups      TEXT NOT NULL,
        created_at  INTEGER NOT NULL
    );
    "#,
    // v13: Objektivkorrektur aus den RAW-Daten (standardmäßig an) – die
    // Vorschaubilder bearbeiteter RAWs neu erzeugen.
    r#"
    UPDATE images SET thumb_state = 0
    WHERE kind = 'raw'
      AND id IN (SELECT image_id FROM edits WHERE is_current = 1);
    "#,
    // v14: Personengruppen (Familie, Freunde …) – jede Person in höchstens einer
    r#"
    CREATE TABLE person_groups (
        id          INTEGER PRIMARY KEY,
        name        TEXT NOT NULL COLLATE NOCASE UNIQUE,
        created_at  INTEGER NOT NULL
    );
    ALTER TABLE persons ADD COLUMN group_id INTEGER REFERENCES person_groups(id) ON DELETE SET NULL;
    "#,
    // v15: Lichter, Tiefen, Weiß und Schwarz wirken auf andere Tonbereiche –
    // die Vorschaubilder aller bearbeiteten Fotos neu erzeugen.
    r#"
    UPDATE images SET thumb_state = 0
    WHERE id IN (SELECT image_id FROM edits WHERE is_current = 1);
    "#,
    // v16: Gesättigte Lichter in RAWs werden rekonstruiert (vorher magenta) –
    // die Vorschaubilder bearbeiteter RAWs neu erzeugen.
    r#"
    UPDATE images SET thumb_state = 0
    WHERE kind = 'raw'
      AND id IN (SELECT image_id FROM edits WHERE is_current = 1);
    "#,
    // v17: Kräftigere Grundabstimmung für RAWs (Kontrast, Sättigung,
    // Dynamik) – die Vorschaubilder bearbeiteter RAWs neu erzeugen.
    r#"
    UPDATE images SET thumb_state = 0
    WHERE kind = 'raw'
      AND id IN (SELECT image_id FROM edits WHERE is_current = 1);
    "#,
    // v18: Grundkurve der RAWs an Kamera-JPEGs angeglichen – die
    // Vorschaubilder bearbeiteter RAWs neu erzeugen.
    r#"
    UPDATE images SET thumb_state = 0
    WHERE kind = 'raw'
      AND id IN (SELECT image_id FROM edits WHERE is_current = 1);
    "#,
    // v19: Gesättigte Lichter übernehmen die Farbe der Umgebung (statt
    // gelbgrüner Haut) – die Vorschaubilder bearbeiteter RAWs neu erzeugen.
    r#"
    UPDATE images SET thumb_state = 0
    WHERE kind = 'raw'
      AND id IN (SELECT image_id FROM edits WHERE is_current = 1);
    "#,
    // v20: RAW-Entwicklung wie darktable (Weißpunkt aus rawspeed, inpaint
    // opposed, color calibration, sigmoid) – die Vorschaubilder bearbeiteter
    // RAWs neu erzeugen.
    r#"
    UPDATE images SET thumb_state = 0
    WHERE kind = 'raw'
      AND id IN (SELECT image_id FROM edits WHERE is_current = 1);
    "#,
    // v21: Aufnahmeort (siehe `GEO`)
    GEO,
    // v22: Index für „schon importiert?“ (`has_content`, je Aufnahme beim
    // Import). v11 hatte `images_hash` entfernt; mit der Größe als zweiter
    // Spalte deckt er die ganze Abfrage ab.
    "CREATE INDEX images_hash ON images(quick_hash, size);",
];

/// v21: Aufnahmeort je Foto – Koordinaten (aus EXIF oder vom Nutzer gesetzt)
/// und Ortsname für die Suche. `geo_state = 0`: GPS aus EXIF noch nicht
/// gelesen (Fotos aus der Zeit davor, wird im Hintergrund nachgetragen).
/// `place`: Ortsname, `''` = keiner gefunden (nicht erneut fragen), NULL =
/// noch nicht ermittelt. `geo_names` merkt sich Ortsnamen je Koordinate
/// (gerundet), damit nicht jedes Foto eine Anfrage an OpenStreetMap kostet.
const GEO: &str = r#"
    ALTER TABLE images ADD COLUMN gps_lat REAL;
    ALTER TABLE images ADD COLUMN gps_lon REAL;
    ALTER TABLE images ADD COLUMN gps_source TEXT;   -- 'exif' | 'manual'
    ALTER TABLE images ADD COLUMN place TEXT;
    ALTER TABLE images ADD COLUMN geo_state INTEGER NOT NULL DEFAULT 0;
    CREATE INDEX images_geo_pending ON images(id) WHERE geo_state = 0;
    CREATE INDEX images_place_pending ON images(id) WHERE gps_lat IS NOT NULL AND place IS NULL;
    CREATE INDEX images_geo ON images(gps_lat, gps_lon) WHERE gps_lat IS NOT NULL;
    CREATE TABLE geo_names (key TEXT PRIMARY KEY, place TEXT NOT NULL);

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
           -- Ohne Ort unverändert (der Index bestehender Fotos bleibt gültig)
           || COALESCE(' ' || NULLIF(i.place, ''), '')
    FROM images i;

    DROP TRIGGER search_img_upd;
    CREATE TRIGGER search_img_upd AFTER UPDATE OF file_name, dir, camera, lens, place ON images
    WHEN OLD.file_name IS NOT NEW.file_name OR OLD.dir IS NOT NEW.dir
      OR OLD.camera IS NOT NEW.camera OR OLD.lens IS NOT NEW.lens
      OR OLD.place IS NOT NEW.place BEGIN
        INSERT OR IGNORE INTO search_dirty (id) VALUES (NEW.id);
    END;
"#;

/// v11: Der Suchtext eines Fotos wurde bisher bei jeder einzelnen
/// Schlagwort-/Gesichtszeile neu gebaut – beim Speichern eines KI-Ergebnisses
/// (alte Schlagworte löschen, ~6–10 neue) bis zu elfmal je Foto. Jetzt merken
/// die Trigger nur das Foto vor; `flush_search` baut am Ende der Transaktion
/// jeden Eintrag einmal neu. Gesichts-Trigger feuern nur noch, wenn sich der
/// Suchtext ändern kann (sichtbares Gesicht einer benannten Person).
///
/// Außerdem Indizes: sichtbare Gesichter je Person (Zählen, leere Gruppen,
/// Personenfilter ohne die Embedding-BLOBs zu lesen), vorhandene Ordner je
/// Quellordner (Ordnerbaum, Anzahl je Quellordner, ohne Tabellendurchlauf).
/// `images_captured` und `images_hash` nutzt keine Abfrage (sortiert wird
/// über `COALESCE(captured_at, …)`, passend zu den Indizes aus v6).
const SEARCH_DIRTY: &str = r#"
    CREATE TABLE search_dirty (id INTEGER PRIMARY KEY);

    DROP TRIGGER search_img_ins;
    DROP TRIGGER search_img_upd;
    DROP TRIGGER search_tag_ins;
    DROP TRIGGER search_tag_del;
    DROP TRIGGER search_tag_ren;
    DROP TRIGGER search_face_ins;
    DROP TRIGGER search_face_upd;
    DROP TRIGGER search_face_del;
    DROP TRIGGER search_person_ren;
    -- search_img_del bleibt: Löschen braucht keinen Neuaufbau

    CREATE TRIGGER search_img_ins AFTER INSERT ON images BEGIN
        INSERT OR IGNORE INTO search_dirty (id) VALUES (NEW.id);
    END;
    CREATE TRIGGER search_img_upd AFTER UPDATE OF file_name, dir, camera, lens ON images
    WHEN OLD.file_name IS NOT NEW.file_name OR OLD.dir IS NOT NEW.dir
      OR OLD.camera IS NOT NEW.camera OR OLD.lens IS NOT NEW.lens BEGIN
        INSERT OR IGNORE INTO search_dirty (id) VALUES (NEW.id);
    END;

    CREATE TRIGGER search_tag_ins AFTER INSERT ON image_tags BEGIN
        INSERT OR IGNORE INTO search_dirty (id) VALUES (NEW.image_id);
    END;
    CREATE TRIGGER search_tag_del AFTER DELETE ON image_tags BEGIN
        INSERT OR IGNORE INTO search_dirty (id) VALUES (OLD.image_id);
    END;
    CREATE TRIGGER search_tag_ren AFTER UPDATE OF name ON tags BEGIN
        INSERT OR IGNORE INTO search_dirty (id) SELECT image_id FROM image_tags WHERE tag_id = NEW.id;
    END;

    -- Nur sichtbare Gesichter benannter Personen stehen im Suchtext. Gruppen
    -- ohne Namen (erste Analyse, Umsortieren) lösen daher nichts aus.
    CREATE TRIGGER search_face_ins AFTER INSERT ON faces
    WHEN NEW.hidden = 0 AND (SELECT name FROM persons WHERE id = NEW.person_id) IS NOT NULL BEGIN
        INSERT OR IGNORE INTO search_dirty (id) VALUES (NEW.image_id);
    END;
    CREATE TRIGGER search_face_upd AFTER UPDATE OF person_id, hidden ON faces
    WHEN (OLD.person_id IS NOT NEW.person_id OR OLD.hidden IS NOT NEW.hidden)
     AND ((OLD.hidden = 0 AND (SELECT name FROM persons WHERE id = OLD.person_id) IS NOT NULL)
       OR (NEW.hidden = 0 AND (SELECT name FROM persons WHERE id = NEW.person_id) IS NOT NULL)) BEGIN
        INSERT OR IGNORE INTO search_dirty (id) VALUES (NEW.image_id);
    END;
    CREATE TRIGGER search_face_del AFTER DELETE ON faces
    WHEN OLD.hidden = 0 AND (SELECT name FROM persons WHERE id = OLD.person_id) IS NOT NULL BEGIN
        INSERT OR IGNORE INTO search_dirty (id) VALUES (OLD.image_id);
    END;
    CREATE TRIGGER search_person_ren AFTER UPDATE OF name ON persons BEGIN
        INSERT OR IGNORE INTO search_dirty (id)
            SELECT image_id FROM faces WHERE person_id = NEW.id AND hidden = 0;
    END;
    -- Vor dem Löschen: Danach setzt der Fremdschlüssel person_id auf NULL, und
    -- search_face_upd fände den Namen nicht mehr.
    CREATE TRIGGER search_person_del BEFORE DELETE ON persons WHEN OLD.name IS NOT NULL BEGIN
        INSERT OR IGNORE INTO search_dirty (id)
            SELECT image_id FROM faces WHERE person_id = OLD.id AND hidden = 0;
    END;

    CREATE INDEX faces_visible ON faces(person_id) WHERE hidden = 0;
    -- Ersetzt images_root (root_id). Nicht partiell (WHERE missing = 0): Den
    -- wählte SQLite sonst ohne Statistik für jede Abfrage über alle
    -- vorhandenen Fotos (Zählen 7 → 50 ms, Sortieren nach Bewertung 50 → 100 ms).
    DROP INDEX images_root;
    CREATE INDEX images_root_dir ON images(root_id, missing, dir);
    DROP INDEX images_captured;
    DROP INDEX images_hash;
    -- Statistik nur für die neuen Indizes (~10 ms bei 50 000 Fotos): Ohne sie
    -- hält SQLite images_root_dir für selektiv und liest beim Abgleich eines
    -- Quellordners alle Fotos über den Index (60 → 120 ms) statt am Stück.
    ANALYZE images_root_dir;
    ANALYZE faces_visible;
"#;

/// Suchtexte der vorgemerkten Fotos neu aufbauen (siehe `SEARCH_DIRTY`). Jeder
/// Schreibweg, der Dateinamen, Ordner, Kamera, Objektiv, Schlagworte oder
/// Personen ändert, ruft dies vor dem Commit auf (`with_tx` immer).
/// Außerhalb einer Transaktion läuft es in einer eigenen.
pub(crate) fn flush_search(conn: &Connection) -> Result<()> {
    // Gelöschte Fotos sind schon raus (search_img_del) – etwa nach dem
    // Entfernen eines Quellordners stehen hier sonst Zehntausende.
    let any: bool = conn
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM search_dirty)")?
        .query_row([], |r| r.get(0))?;
    if !any {
        return Ok(());
    }
    // EXISTS statt JOIN: beim JOIN durchlief SQLite (ohne Statistik zur
    // kleinen Tabelle) alle Fotos – 2 ms je Aufruf statt Mikrosekunden.
    let ids: Vec<i64> = conn
        .prepare_cached(
            "SELECT id FROM search_dirty d WHERE EXISTS (SELECT 1 FROM images i WHERE i.id = d.id)",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let tx = conn
        .is_autocommit()
        .then(|| conn.unchecked_transaction())
        .transpose()?;
    {
        let mut del = conn.prepare_cached("DELETE FROM search WHERE rowid = ?1")?;
        let mut ins = conn.prepare_cached(
            "INSERT INTO search (rowid, body) SELECT id, body FROM search_source WHERE id = ?1",
        )?;
        for id in &ids {
            del.execute([id])?;
            ins.execute([id])?;
        }
    }
    conn.execute("DELETE FROM search_dirty", [])?;
    if let Some(tx) = tx {
        tx.commit()?;
    }
    Ok(())
}

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

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        // Von einer neueren Lumenkorn-Version angelegt: Diese Version kennt
        // deren Spalten und Trigger nicht und würde den Katalog beschädigen
        if version > MIGRATIONS.len() as i64 {
            return Err(Error::CatalogTooNew {
                version,
                known: MIGRATIONS.len(),
            });
        }
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
            return Err(Error::Invalid(tr!(
                "{} ist kein Ordner",
                "{} is not a folder",
                path.display()
            )));
        }
        let p = paths::normalize_dir(path);

        // Verschachtelte Quellordner würden Bilder doppelt erfassen.
        for r in self.roots()? {
            if paths::is_within(&p, &r.path) {
                return Err(Error::Invalid(tr!(
                    "{p} liegt bereits in Quellordner {}",
                    "{p} is already inside source folder {}",
                    r.path
                )));
            }
            if paths::is_within(&r.path, &p) {
                return Err(Error::Invalid(tr!(
                    "{p} enthält den bestehenden Quellordner {}. Bitte diesen zuerst entfernen.",
                    "{p} contains the existing source folder {}. Please remove it first.",
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
        flush_search(&self.conn)?;
        // Gruppen, deren Gesichter alle in diesem Ordner lagen
        self.drop_empty_groups(None)?;
        if n == 0 {
            return Err(Error::NotFound(tr!(
                "Quellordner {id}",
                "Source folder {id}"
            )));
        }
        Ok(())
    }

    /// Wie [`Catalog::remove_root`], löscht die Fotos aber in Portionen und
    /// gibt die Katalogsperre dazwischen frei. Bei Katalogen mit Zehntausenden
    /// Fotos hielte das Löschen in einem Zug die Sperre sonst sehr lange.
    /// Der Aufrufer darf die Sperre nicht selbst halten.
    pub fn remove_root_chunked(catalog: &std::sync::Mutex<Catalog>, id: i64) -> Result<()> {
        const CHUNK: i64 = 2000;
        catalog.locked().root(id)?;
        loop {
            let n = catalog.locked().with_tx(|tx| {
                Ok(tx.execute(
                    "DELETE FROM images WHERE id IN
                       (SELECT id FROM images WHERE root_id = ?1 LIMIT ?2)",
                    params![id, CHUNK],
                )?)
            })?;
            if n == 0 {
                break;
            }
        }
        // Rest (z. B. während des Löschens neu gescannt) und der Ordner selbst
        catalog.locked().remove_root(id)
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
            .ok_or_else(|| Error::NotFound(tr!("Quellordner {id}", "Source folder {id}")))
    }

    /// Ordnerbaum eines Quellordners: Ordner mit Bildern und (seit v8) alle
    /// übrigen Ordner von der Platte, auch leere.
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
        self.with_tx(|tx| {
            let mut n = 0;
            for g in gone {
                // Der Ordner selbst und alles darunter (Bereichsabfrage wie beim Ordnerfilter)
                let g = paths::trim_dir(g);
                let (prefix, upper) = subtree_range(&g);
                n += tx.execute(
                    "DELETE FROM folders WHERE root_id = ?1 AND (path = ?2 OR (path >= ?3 AND path < ?4))",
                    params![root_id, g, prefix, upper],
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
        let rows = st.query_map([root_id], known_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Bekannte Bilder unter den gemeldeten Pfaden – die Datei selbst oder
    /// ein Ordner samt Unterordnern – und alle schon fehlenden des
    /// Quellordners (für die Erkennung verschobener Dateien). Für die
    /// Ordnerüberwachung: Bei 100 000 Fotos lud jede Meldung sonst alle.
    pub(crate) fn known_images_under(
        &self,
        root_id: i64,
        paths: &[String],
    ) -> Result<Vec<KnownImage>> {
        // Sehr viele Pfade: ein Durchgang über alle ist dann günstiger
        if paths.len() > 2000 {
            return self.known_images(root_id);
        }
        let mut out: std::collections::BTreeMap<i64, KnownImage> = Default::default();
        let mut missing = self.conn.prepare_cached(
            "SELECT id, path, size, mtime, quick_hash, missing FROM images
             WHERE root_id = ?1 AND missing = 1",
        )?;
        for k in missing.query_map([root_id], known_row)? {
            let k = k?;
            out.insert(k.id, k);
        }
        let mut under = self.conn.prepare_cached(
            "SELECT id, path, size, mtime, quick_hash, missing FROM images
             WHERE root_id = ?1 AND (path = ?2 OR dir = ?2 OR (dir >= ?3 AND dir < ?4))",
        )?;
        for p in paths {
            let (prefix, upper) = subtree_range(p);
            for k in under.query_map(params![root_id, p, prefix, upper], known_row)? {
                let k = k?;
                out.insert(k.id, k);
            }
        }
        Ok(out.into_values().collect())
    }

    /// Führt mehrere Schreibvorgänge in einer Transaktion aus. Der Suchindex
    /// wird am Ende einmal je geändertem Foto nachgeführt.
    pub(crate) fn with_tx<T>(
        &mut self,
        f: impl FnOnce(&rusqlite::Transaction) -> Result<T>,
    ) -> Result<T> {
        let tx = self.conn.transaction()?;
        let out = f(&tx)?;
        flush_search(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Gibt es ein Foto mit diesem Inhalt (Größe und schneller Hash) schon im
    /// Katalog – egal wo? Für den Import („schon importiert“).
    pub fn has_content(&self, quick_hash: &str, size: i64) -> Result<bool> {
        Ok(self
            .conn
            .prepare_cached("SELECT 1 FROM images WHERE quick_hash = ?1 AND size = ?2 LIMIT 1")?
            .exists(params![quick_hash, size])?)
    }

    /// Bild mit genau diesem Pfad, falls schon im Katalog.
    pub(crate) fn image_id_by_path(tx: &rusqlite::Transaction, path: &str) -> Result<Option<i64>> {
        Ok(tx
            .query_row("SELECT id FROM images WHERE path = ?1", [path], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub(crate) fn insert_image(tx: &rusqlite::Transaction, img: &NewImage) -> Result<i64> {
        let m = &img.meta;
        tx.execute(
            "INSERT INTO images (id, root_id, path, dir, file_name, ext, kind, size, mtime, quick_hash,
                                 width, height, orientation, captured_at, camera, lens, iso,
                                 exposure, aperture, focal_length, added_at,
                                 gps_lat, gps_lon, gps_source, geo_state)
             VALUES (MAX(COALESCE((SELECT MAX(id) FROM images), 0),
                         COALESCE((SELECT v FROM image_id_floor), 0)) + 1,
                     ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
                     ?21, ?22, CASE WHEN ?21 IS NULL THEN NULL ELSE 'exif' END, 1)",
            params![
                img.root_id, img.path, img.dir, img.file_name, img.ext, img.kind.as_str(), img.size,
                img.mtime, img.quick_hash, m.width, m.height, m.orientation, m.captured_at, m.camera,
                m.lens, m.iso, m.exposure, m.aperture, m.focal_length, now(),
                m.gps.map(|g| g.0), m.gps.map(|g| g.1)
            ],
        )?;
        Ok(tx.last_insert_rowid())
    }

    /// Datei hat sich geändert: Metadaten neu, Vorschau neu erzeugen. KI und
    /// Gesichtserkennung laufen nur bei geänderter Größe oder Prüfsumme neu.
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
                    missing = 0, thumb_state = 0, geo_state = 1,
                    -- KI und Gesichter nur neu, wenn sich der Inhalt geändert hat (nicht
                    -- bei bloßer Änderungszeit, z. B. `touch` oder Sync-Werkzeug)
                    ai_state = CASE WHEN size IS ?2 AND quick_hash IS ?4 THEN ai_state ELSE 0 END,
                    face_state = CASE WHEN size IS ?2 AND quick_hash IS ?4 THEN face_state ELSE 0 END,
                    -- Vom Nutzer gesetzte Orte bleiben, sonst gilt das neue EXIF
                    place = CASE WHEN gps_source = 'manual' OR (gps_lat IS ?15 AND gps_lon IS ?16)
                                 THEN place ELSE NULL END,
                    gps_lat = CASE WHEN gps_source = 'manual' THEN gps_lat ELSE ?15 END,
                    gps_lon = CASE WHEN gps_source = 'manual' THEN gps_lon ELSE ?16 END,
                    gps_source = CASE WHEN gps_source = 'manual' THEN gps_source
                                      WHEN ?15 IS NULL THEN NULL ELSE 'exif' END
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
                m.focal_length,
                m.gps.map(|g| g.0),
                m.gps.map(|g| g.1)
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
                    EXISTS (SELECT 1 FROM edits e WHERE e.image_id = i.id AND e.is_current = 1),
                    i.gps_lat, i.gps_lon
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

    /// Nur die IDs der Treffer (gleiche Filter wie [`Catalog::query_images`],
    /// ohne Sortierung und Limit) – günstig für Mengenprüfungen.
    pub fn query_image_ids(&self, q: &ImageQuery) -> Result<HashSet<i64>> {
        let (where_sql, args) = self.where_clause(q)?;
        let mut st = self
            .conn
            .prepare(&format!("SELECT i.id FROM images i WHERE {where_sql}"))?;
        let rows = st.query_map(params_from_iter(args), |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub(crate) fn count_images(&self, scope: &Scope) -> Result<i64> {
        let (where_sql, args) = self.where_clause(&ImageQuery::new(scope.clone()))?;
        Ok(self.conn.query_row(
            &format!("SELECT COUNT(*) FROM images i WHERE {where_sql}"),
            params_from_iter(args),
            |r| r.get(0),
        )?)
    }

    pub(crate) fn where_clause(&self, q: &ImageQuery) -> Result<(String, Vec<Value>)> {
        let mut parts = vec!["i.missing = 0".to_string()];
        let mut args: Vec<Value> = Vec::new();

        match &q.scope {
            Scope::All => {}
            Scope::Folder { path, recursive } => {
                let p = paths::trim_dir(path);
                parts.push(dir_filter(&p, *recursive, &mut args));
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
                // Ordner vorab lesen und je Ordner die Bereichsabfrage des
                // Ordnerfilters: nutzt den Index auf `dir` statt substr() für
                // jedes Foto. Überlappende Ordner liefern dank OR jedes Bild einmal.
                let folders: Vec<(String, bool)> = self
                    .conn
                    .prepare_cached(
                        "SELECT path, recursive FROM project_folders WHERE project_id = ?1",
                    )?
                    .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<std::result::Result<_, _>>()?;
                let ors: Vec<String> = folders
                    .iter()
                    .map(|(p, rec)| dir_filter(&paths::trim_dir(p), *rec, &mut args))
                    .collect();
                parts.push(if ors.is_empty() {
                    "0".into()
                } else {
                    format!("({})", ors.join(" OR "))
                });
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

        // Jedes Suchwort muss im Suchtext vorkommen (Dateiname, Ordner, Kamera,
        // Objektiv, Schlagworte, Personennamen – siehe `SEARCH_INDEX`).
        // Volltextindex: Wörter ab drei Zeichen über Trigramme (Index),
        // kürzere per LIKE auf denselben Einträgen. Alle Wörter müssen passen.
        let words: Vec<&str> = q
            .search
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .collect();
        if !words.is_empty() {
            // Absicherung: Vorgemerktes zuerst indexieren (normalerweise leer,
            // jeder Schreibweg leert es selbst)
            flush_search(&self.conn)?;
        }
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
                        location: None,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(tr!("Bild {id}", "Photo {id}")))?;
        d.tags = self.image_tags(id)?;
        d.location = self.location(id)?;
        Ok(d)
    }

    pub fn set_rating(&self, ids: &[i64], rating: i64) -> Result<()> {
        if !(0..=5).contains(&rating) {
            return Err(Error::Invalid(
                tr_s(
                    "Bewertung muss zwischen 0 und 5 liegen",
                    "Rating must be between 0 and 5",
                )
                .into(),
            ));
        }
        self.update_many(ids, "rating", rating)
    }

    pub fn set_flag(&self, ids: &[i64], flag: i64) -> Result<()> {
        if !(-1..=1).contains(&flag) {
            return Err(Error::Invalid(
                tr_s(
                    "Markierung muss -1, 0 oder 1 sein",
                    "Flag must be -1, 0 or 1",
                )
                .into(),
            ));
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
        let (faces, persons) = self.with_tx(|tx| {
            let mut faces = Vec::new();
            // Nur Gruppen dieser Gesichter können leer werden
            let mut persons = Vec::new();
            {
                let mut st = tx.prepare("SELECT id, person_id FROM faces WHERE image_id = ?1")?;
                for id in ids {
                    for r in st.query_map([id], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
                    })? {
                        let (face, person) = r?;
                        faces.push(face);
                        persons.extend(person);
                    }
                }
            }
            for id in ids {
                tx.execute("DELETE FROM images WHERE id = ?1", [id])?;
            }
            Ok((faces, persons))
        })?;
        self.drop_empty_groups(Some(&persons))?;
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

    pub(crate) fn pending_thumbs(&self, limit: usize) -> Result<Vec<ThumbJob>> {
        let mut st = self.conn.prepare(&format!(
            "{THUMB_JOB_SELECT} WHERE i.thumb_state = 0 AND i.missing = 0 ORDER BY i.id LIMIT ?1"
        ))?;
        let rows = st.query_map([limit as i64], thumb_job_from_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Ortsnamen (Spalte `place`) der Fotos; Fotos ohne oder mit leerem Namen
    /// fehlen im Ergebnis.
    /// Alle aktuellen Bearbeitungen vorhandener Fotos – für „Auto nach deinem
    /// Stil“.
    pub fn edited_overview(&self) -> Result<Vec<EditedPhoto>> {
        let mut st = self.conn.prepare(
            "SELECT e.image_id, e.updated_at, i.camera, i.kind = 'raw'
             FROM edits e JOIN images i ON i.id = e.image_id
             WHERE e.is_current = 1 AND i.missing = 0",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn places(&self, ids: &[i64]) -> Result<HashMap<i64, String>> {
        let ids = serde_json::to_string(ids)?;
        let mut st = self.conn.prepare(
            "SELECT id, place FROM images
             WHERE id IN (SELECT value FROM json_each(?1)) AND place IS NOT NULL AND place <> ''",
        )?;
        let rows = st.query_map([ids], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn thumb_job(&self, id: i64) -> Result<ThumbJob> {
        self.conn
            .prepare_cached(&format!("{THUMB_JOB_SELECT} WHERE i.id = ?1"))?
            .query_row([id], thumb_job_from_row)
            .optional()?
            .ok_or_else(|| Error::NotFound(tr!("Bild {id}", "Photo {id}")))
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
            .ok_or_else(|| Error::NotFound(tr!("Bild {id}", "Photo {id}")))
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
            flush_search(&self.conn)?; // Kamera und Objektiv sind durchsuchbar
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
        let name = project_name(name)?;
        let t = now();
        self.conn.execute(
            "INSERT INTO projects (name, description, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![name, description, t],
        )?;
        self.project(self.conn.last_insert_rowid())
    }

    pub fn update_project(&self, id: i64, name: &str, description: &str) -> Result<Project> {
        let name = project_name(name)?;
        let n = self.conn.execute(
            "UPDATE projects SET name = ?2, description = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, name, description, now()],
        )?;
        if n == 0 {
            return Err(Error::NotFound(tr!("Projekt {id}", "Project {id}")));
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
            .ok_or_else(|| Error::NotFound(tr!("Projekt {id}", "Project {id}")))?;

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
            return Err(Error::Invalid(
                tr_s(
                    "Schlagwort darf nicht leer sein",
                    "Keyword must not be empty",
                )
                .into(),
            ));
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
        flush_search(&self.conn)?;
        Ok(())
    }

    pub fn remove_tag(&self, image_id: i64, name: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM image_tags WHERE image_id = ?1 AND tag_id = (SELECT id FROM tags WHERE name = ?2)",
            params![image_id, name],
        )?;
        flush_search(&self.conn)?;
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
        let cur = self.current_edit(image_id)?.ok_or_else(|| {
            Error::Invalid(tr_s("Bild hat noch keine Bearbeitung", "Photo has no edits yet").into())
        })?;
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
    // Presets
    // ------------------------------------------------------------------

    /// Alle Presets, alphabetisch.
    pub fn presets(&self) -> Result<Vec<Preset>> {
        let mut st = self
            .conn
            .prepare("SELECT id, name, params, groups, created_at FROM presets")?;
        let rows = st.query_map([], |r| {
            let params: String = r.get(2)?;
            let groups: String = r.get(3)?;
            Ok(Preset {
                id: r.get(0)?,
                name: r.get(1)?,
                params: serde_json::from_str::<serde_json::Value>(&params)
                    .map(|v| DevelopParams::from_json(&v))
                    .unwrap_or_default(),
                groups: serde_json::from_str(&groups).unwrap_or_default(),
                created_at: r.get(4)?,
            })
        })?;
        let mut out: Vec<Preset> = rows.collect::<std::result::Result<_, _>>()?;
        out.sort_by_key(|p| p.name.to_lowercase());
        Ok(out)
    }

    /// Legt ein Preset an. Fotospezifisches (KI-Rauschreduzierung,
    /// KI-Reparaturen, KI-Masken) gehört nie dazu.
    pub fn create_preset(
        &self,
        name: &str,
        params: &DevelopParams,
        groups: TransferGroups,
    ) -> Result<i64> {
        let p = DevelopParams::default().transfer_from(&params.sanitized(), groups);
        self.conn.execute(
            "INSERT INTO presets (name, params, groups, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                name,
                serde_json::to_string(&p)?,
                serde_json::to_string(&groups)?,
                now()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_preset(&self, id: i64, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE presets SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        Ok(())
    }

    pub fn delete_preset(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM presets WHERE id = ?1", [id])?;
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
        lat: r.get(12)?,
        lon: r.get(13)?,
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

#[cfg(test)]
mod content_update_tests {
    use super::*;

    fn changed(id_path: &str, root_id: i64, mtime: i64, hash: &str) -> NewImage {
        NewImage {
            root_id,
            path: id_path.into(),
            dir: "/tmp/lumenkorn-test".into(),
            file_name: "bild0.jpg".into(),
            ext: "jpg".into(),
            kind: ImageKind::Raster,
            size: 1,
            mtime,
            quick_hash: hash.into(),
            meta: Default::default(),
        }
    }

    #[test]
    fn only_changed_content_resets_ai_and_faces() {
        let mut cat = Catalog::open_in_memory().unwrap();
        let ids = tests_support::insert_dummy_images(&mut cat, 1);
        let root_id: i64 = cat
            .conn
            .query_row("SELECT root_id FROM images WHERE id = ?1", [ids[0]], |r| {
                r.get(0)
            })
            .unwrap();
        let path = "/tmp/lumenkorn-test/bild0.jpg";
        let states = |cat: &Catalog| -> (i64, i64, i64) {
            cat.conn
                .query_row(
                    "SELECT ai_state, face_state, thumb_state FROM images WHERE id = ?1",
                    [ids[0]],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap()
        };
        cat.conn
            .execute("UPDATE images SET ai_state = 1, face_state = 1", [])
            .unwrap();

        // Nur die Änderungszeit: KI und Gesichter bleiben fertig
        let img = changed(path, root_id, 99, "h0");
        cat.with_tx(|tx| Catalog::update_image_content(tx, ids[0], &img))
            .unwrap();
        assert_eq!(states(&cat), (1, 1, 0));

        // Anderer Hash: alles neu
        let img = changed(path, root_id, 100, "andere");
        cat.with_tx(|tx| Catalog::update_image_content(tx, ids[0], &img))
            .unwrap();
        assert_eq!(states(&cat), (0, 0, 0));
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

    #[test]
    fn places_returns_only_non_empty_names() {
        let mut cat = Catalog::open_in_memory().unwrap();
        let ids = tests_support::insert_dummy_images(&mut cat, 3);
        cat.conn
            .execute(
                "UPDATE images SET place = 'Lissabon' WHERE id = ?1",
                [ids[0]],
            )
            .unwrap();
        cat.conn
            .execute("UPDATE images SET place = '' WHERE id = ?1", [ids[1]])
            .unwrap();
        let p = cat.places(&ids).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[&ids[0]], "Lissabon");
        assert!(cat.places(&[]).unwrap().is_empty());
    }
}

#[cfg(test)]
mod search_tests {
    use super::tests_support::exact;
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

    /// Jeder Schreibweg führt den Suchindex selbst nach (v11: Trigger merken
    /// nur vor, `flush_search` baut auf).
    #[test]
    fn every_write_path_flushes() {
        let mut cat = Catalog::open_in_memory().unwrap();
        let ids = tests_support::insert_dummy_images(&mut cat, 4);
        exact(&cat, "einfügen");
        let (a, b, c) = (ids[0], ids[1], ids[2]);
        cat.add_tag(a, "Strand", TagSource::Manual, None).unwrap();
        exact(&cat, "add_tag");
        cat.remove_tag(a, "Strand").unwrap();
        exact(&cat, "remove_tag");
        let tags = [("meer".to_string(), 0.4), ("strand".to_string(), 0.3)];
        cat.save_ai_result(b, "m", &[0, 1], &tags).unwrap();
        cat.save_ai_result(b, "m", &[0, 1], &tags[..1]).unwrap();
        exact(&cat, "save_ai_result");

        // Personen
        let e = |k: usize| {
            let mut v = vec![0.0f32; 8];
            v[k] = 1.0;
            crate::people::NewFace {
                rect: [0.1, 0.1, 0.2, 0.2],
                score: 0.9,
                embedding: v,
            }
        };
        let mut idx = cat.people_index().unwrap();
        cat.save_faces(a, &[e(0)], &mut idx).unwrap();
        cat.save_faces(b, &[e(0), e(1)], &mut idx).unwrap();
        cat.save_faces(c, &[e(1)], &mut idx).unwrap();
        exact(&cat, "save_faces");
        let mut cache = Some(idx);
        let g0 = cat.image_faces(a).unwrap()[0].person_id.unwrap();
        let grace = cat.name_person(g0, "Grace", &mut cache).unwrap();
        exact(&cat, "name_person");
        cat.save_faces(a, &[e(0)], cache.as_mut().unwrap()).unwrap();
        exact(&cat, "save_faces (benannt)");
        let fc = cat.image_faces(c).unwrap()[0].id;
        cat.assign_face(fc, "Hopper", &mut cache).unwrap();
        exact(&cat, "assign_face");
        cat.reassign_faces(&[fc], Some("Ada"), &mut cache).unwrap();
        exact(&cat, "reassign_faces");
        cat.reassign_faces(&[fc], None, &mut cache).unwrap();
        exact(&cat, "reassign_faces (lösen)");
        let ada = cat.person_by_name("Ada").unwrap().unwrap();
        cat.name_person(ada, "grace", &mut cache).unwrap();
        exact(&cat, "name_person (zusammenführen)");
        let fa = cat.image_faces(a).unwrap()[0].id;
        cat.reject_face(fa).unwrap();
        exact(&cat, "reject_face");
        let fb = cat.person_faces(grace).unwrap()[0].id;
        cat.hide_face(fb).unwrap();
        exact(&cat, "hide_face");
        cat.assign_face(fb, "Grace", &mut cache).unwrap();
        cat.delete_person(grace).unwrap();
        exact(&cat, "delete_person");

        // Aufnahmedaten übernehmen, verschieben, entfernen
        cat.conn
            .execute("UPDATE images SET camera = 'Nikon Z6' WHERE id = ?1", [a])
            .unwrap();
        flush_search(&cat.conn).unwrap();
        let path: String = cat
            .conn
            .query_row("SELECT path FROM images WHERE id = ?1", [b], |r| r.get(0))
            .unwrap();
        cat.adopt_capture_info(&path, a).unwrap();
        exact(&cat, "adopt_capture_info");
        let root = cat.image_root(a).unwrap();
        cat.with_tx(|tx| {
            Catalog::move_image(
                tx,
                c,
                &NewImage {
                    root_id: root,
                    path: "/x/Lofoten/neu.jpg".into(),
                    dir: "/x/Lofoten".into(),
                    file_name: "neu.jpg".into(),
                    ext: "jpg".into(),
                    kind: ImageKind::Raster,
                    size: 1,
                    mtime: 1,
                    quick_hash: "h".into(),
                    meta: Default::default(),
                },
            )
        })
        .unwrap();
        exact(&cat, "move_image");
        cat.forget_images(&[a]).unwrap();
        exact(&cat, "forget_images");
        assert_eq!(hits(&cat, "z6"), vec![b]);
        assert_eq!(hits(&cat, "neu"), vec![c]);
        cat.remove_root(root).unwrap();
        exact(&cat, "remove_root");
        let persons: i64 = cat
            .conn
            .query_row("SELECT COUNT(*) FROM persons WHERE name IS NULL", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(persons, 0, "leere Gruppen entfernt");
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

#[cfg(test)]
mod migration_tests {
    use super::*;

    /// Stand vor v22 nachstellen: der Index für `has_content` fehlt wieder. Vor
    /// `UNDO_V11` einsetzen, das den alten `images_hash` neu anlegt.
    const UNDO_V22: &str = "DROP INDEX images_hash;";

    /// Stand vor v21 nachstellen: Ortsspalten weg, Suchansicht und Trigger
    /// wie vorher (Teil von `UNDO_V11`, das weiter zurückgeht).
    const UNDO_V21: &str = "DROP INDEX images_geo_pending; DROP INDEX images_place_pending;
        DROP INDEX images_geo; DROP TABLE geo_names;
        DROP TRIGGER search_img_upd; DROP VIEW search_source;
        ALTER TABLE images DROP COLUMN gps_lat; ALTER TABLE images DROP COLUMN gps_lon;
        ALTER TABLE images DROP COLUMN gps_source; ALTER TABLE images DROP COLUMN place;
        ALTER TABLE images DROP COLUMN geo_state;
        CREATE VIEW search_source(id, body) AS
        SELECT i.id,
               i.file_name
               || ' ' || COALESCE((SELECT substr(i.dir, length(r.path)
                                       + CASE WHEN substr(r.path, -1) IN ('/', '\\') THEN 1 ELSE 2 END)
                                   FROM roots r WHERE r.id = i.root_id), '')
               || ' ' || COALESCE(i.camera, '') || ' ' || COALESCE(i.lens, '')
               || ' ' || COALESCE((SELECT group_concat(t.name, ' ') FROM image_tags it
                                   JOIN tags t ON t.id = it.tag_id WHERE it.image_id = i.id), '')
               || ' ' || COALESCE((SELECT group_concat(p.name, ' ') FROM faces f
                                   JOIN persons p ON p.id = f.person_id
                                   WHERE f.image_id = i.id AND f.hidden = 0 AND p.name IS NOT NULL), '')
        FROM images i;
        CREATE TRIGGER search_img_upd AFTER UPDATE OF file_name, dir, camera, lens ON images BEGIN
            INSERT OR IGNORE INTO search_dirty (id) VALUES (NEW.id);
        END;";

    /// Stand vor v11 nachstellen (die Trigger ersetzt v11 ohnehin).
    const UNDO_V11: &str = "ALTER TABLE persons DROP COLUMN group_id; DROP TABLE person_groups;
        DROP TABLE IF EXISTS presets;
        DROP TRIGGER search_person_del; DROP TABLE search_dirty;
        DROP INDEX faces_visible; DROP INDEX images_root_dir;
        CREATE INDEX images_root ON images(root_id);
        CREATE INDEX images_hash ON images(quick_hash);
        CREATE INDEX images_captured ON images(captured_at);";

    /// v9: Vorschaubilder bearbeiteter RAWs werden neu erzeugt, andere nicht.
    #[test]
    fn new_raw_pipeline_refreshes_edited_raw_thumbnails() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().join("Fotos");
        std::fs::create_dir_all(&root_dir).unwrap();
        let db = tmp.path().join("k.db");
        let ids = {
            let mut cat = Catalog::open(&db).unwrap();
            let root = cat.add_root(&root_dir).unwrap();
            let mut add = |name: &str, kind| {
                let dir = root.path.clone();
                cat.with_tx(|tx| {
                    Catalog::insert_image(
                        tx,
                        &NewImage {
                            root_id: root.id,
                            path: format!("{dir}{}{name}", std::path::MAIN_SEPARATOR),
                            dir: dir.clone(),
                            file_name: name.into(),
                            ext: name.rsplit('.').next().unwrap().into(),
                            kind,
                            size: 1,
                            mtime: 0,
                            quick_hash: name.into(),
                            meta: Default::default(),
                        },
                    )
                })
                .unwrap()
            };
            let raw_edited = add("a.nef", crate::formats::ImageKind::Raw);
            let raw_plain = add("b.nef", crate::formats::ImageKind::Raw);
            let jpg_edited = add("c.jpg", crate::formats::ImageKind::Raster);
            let p = DevelopParams {
                exposure: 0.5,
                ..Default::default()
            };
            cat.save_develop(&[raw_edited, jpg_edited], &p).unwrap();
            cat.conn
                .execute_batch(&format!(
                    // Stand vor v9 (und damit auch vor v10, v11) nachstellen
                    "UPDATE images SET thumb_state = 1; {UNDO_V22} {UNDO_V21} {UNDO_V11}
                     DROP TRIGGER face_id_floor_del; DROP TABLE face_id_floor;
                     PRAGMA user_version = 8;"
                ))
                .unwrap();
            [raw_edited, raw_plain, jpg_edited]
        };
        let cat = Catalog::open(&db).unwrap();
        let state = |id: i64| -> i64 {
            cat.conn
                .query_row("SELECT thumb_state FROM images WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        // Bearbeitete RAWs (v9, v13) und – seit den neuen Tonwertreglern (v15) –
        // alle bearbeiteten Fotos; unbearbeitete bleiben
        assert_eq!(ids.map(state), [0, 1, 0]);
    }

    /// Katalog einer neueren Version: eigener Fehler (die App bietet dann das
    /// Update an), und der Katalog bleibt unverändert.
    #[test]
    fn newer_catalog_is_reported_and_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("k.db");
        let newer = MIGRATIONS.len() as i64 + 1;
        Connection::open(&db)
            .unwrap()
            .pragma_update(None, "user_version", newer)
            .unwrap();
        match Catalog::open(&db) {
            Err(Error::CatalogTooNew { version, known }) => {
                assert_eq!((version, known), (newer, MIGRATIONS.len()));
            }
            Err(e) => panic!("falscher Fehler: {e}"),
            Ok(_) => panic!("neuerer Katalog wurde geöffnet"),
        }
        let conn = Connection::open(&db).unwrap();
        let v: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        let tables: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
            .unwrap();
        assert_eq!((v, tables), (newer, 0));
    }

    /// v11: Ein Katalog von v10 (alte Trigger) behält seinen Suchindex, die
    /// neuen Trigger und Indizes greifen.
    #[test]
    fn v11_keeps_search_index() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("k.db");
        {
            let conn = Connection::open(&db).unwrap();
            conn.pragma_update(None, "foreign_keys", "ON").unwrap();
            for sql in &MIGRATIONS[..10] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 10).unwrap();
            conn.execute_batch(
                "INSERT INTO roots (id, path, added_at) VALUES (1, '/f', 0);
                 INSERT INTO images (id, root_id, path, dir, file_name, ext, kind, size, mtime,
                                     quick_hash, camera, added_at)
                 VALUES (1, 1, '/f/Urlaub/a.jpg', '/f/Urlaub', 'a.jpg', 'jpg', 'raster', 1, 0, 'h', 'Z6', 0),
                        (2, 1, '/f/b.jpg', '/f', 'b.jpg', 'jpg', 'raster', 1, 0, 'h', NULL, 0);
                 INSERT INTO tags (id, name) VALUES (1, 'Strand');
                 INSERT INTO image_tags VALUES (1, 1, 'manual', NULL);
                 INSERT INTO persons (id, name, created_at) VALUES (1, 'Grace', 0), (2, NULL, 0);
                 INSERT INTO faces (id, image_id, x, y, w, h, score, embedding, person_id)
                 VALUES (1, 2, 0, 0, 1, 1, 1, x'00', 1), (2, 1, 0, 0, 1, 1, 1, x'00', 2);",
            )
            .unwrap();
        }
        let cat = Catalog::open(&db).unwrap();
        tests_support::exact(&cat, "nach Migration");
        let names: Vec<String> = cat
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name IN ('images', 'faces')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert!(names.iter().any(|n| n == "faces_visible"), "{names:?}");
        assert!(names.iter().any(|n| n == "images_root_dir"), "{names:?}");
        // images_hash gibt es seit v22 wieder, mit (quick_hash, size)
        assert!(names.iter().any(|n| n == "images_hash"), "{names:?}");
        for gone in ["images_captured", "images_root"] {
            assert!(!names.iter().any(|n| n == gone), "{gone}: {names:?}");
        }
        let body = |id: i64| -> String {
            cat.conn
                .query_row("SELECT body FROM search WHERE rowid = ?1", [id], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        assert!(body(1).contains("Urlaub") && body(1).contains("Strand") && body(1).contains("Z6"));
        assert!(body(2).contains("Grace"));
        // Umbenennen über die neuen Trigger
        cat.conn
            .execute("UPDATE persons SET name = 'Ada' WHERE id = 1", [])
            .unwrap();
        flush_search(&cat.conn).unwrap();
        tests_support::exact(&cat, "Umbenennen");
        assert!(body(2).contains("Ada"));
    }

    /// v22: `has_content` nutzt den Index statt die Tabelle zu durchlaufen.
    #[test]
    fn has_content_uses_index() {
        let cat = Catalog::open_in_memory().unwrap();
        let plan: String = cat
            .conn
            .query_row(
                "EXPLAIN QUERY PLAN SELECT 1 FROM images WHERE quick_hash = ?1 AND size = ?2 LIMIT 1",
                params!["h", 1],
                |r| r.get(3),
            )
            .unwrap();
        assert!(plan.contains("images_hash"), "{plan}");
    }

    /// v10: Gesichts-IDs werden nach dem Löschen der neuesten Gesichter nicht
    /// erneut vergeben – auch nicht für einen Katalog, der vor v10 entstand.
    #[test]
    fn face_ids_are_never_reused() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("k.db");
        let face = |e: f32| crate::people::NewFace {
            rect: [0.1, 0.1, 0.2, 0.2],
            score: 0.9,
            embedding: vec![e, 1.0, 0.0, 0.0],
        };
        let (imgs, old_max) = {
            let mut cat = Catalog::open(&db).unwrap();
            let imgs = tests_support::insert_dummy_images(&mut cat, 2);
            let mut idx = cat.people_index().unwrap();
            cat.save_faces(imgs[0], &[face(0.0)], &mut idx).unwrap();
            let ids = cat
                .save_faces(imgs[1], &[face(0.1), face(0.2)], &mut idx)
                .unwrap();
            // Zustand vor v10 nachstellen: höchste ID bereits im Bestand
            cat.conn
                .execute_batch(&format!(
                    "{UNDO_V22} {UNDO_V21} {UNDO_V11} DROP TRIGGER face_id_floor_del; DROP TABLE face_id_floor;
                     PRAGMA user_version = 9;"
                ))
                .unwrap();
            (imgs, *ids.iter().max().unwrap())
        };
        let mut cat = Catalog::open(&db).unwrap();
        // Neueste Gesichter löschen (wie beim Entfernen des Fotos) …
        cat.conn
            .execute("DELETE FROM faces WHERE image_id = ?1", [imgs[1]])
            .unwrap();
        // … und neu erkennen: neue IDs liegen über allen je vergebenen
        let mut idx = cat.people_index().unwrap();
        let new = cat
            .save_faces(imgs[1], &[face(0.3), face(0.4)], &mut idx)
            .unwrap();
        assert!(new.iter().all(|&id| id > old_max), "{new:?} <= {old_max}");
        // Erneutes Ersetzen (save_faces löscht vorher) vergibt wieder neue IDs
        let again = cat.save_faces(imgs[1], &[face(0.5)], &mut idx).unwrap();
        assert!(again[0] > *new.iter().max().unwrap(), "{again:?} / {new:?}");
    }
}

#[cfg(test)]
mod project_tests {
    use super::*;

    /// Projektordner als Bereichsabfrage: Unterordner ja, Namensvettern
    /// („a“ vs. „ab“) nein, Laufwerkswurzeln („/“, „C:\“) umfassen alles.
    #[test]
    fn project_folders_use_ranges_and_drive_roots() {
        let mut cat = Catalog::open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let root = cat.add_root(tmp.path()).unwrap();
        let r = if cfg!(windows) { r"C:\" } else { "/" };
        let dirs = [
            format!("{r}a"),
            format!("{r}a{SEP}b"),
            format!("{r}ab"),
            format!("{r}a{SEP}b{SEP}c"),
        ];
        cat.with_tx(|tx| {
            for (i, d) in dirs.iter().enumerate() {
                Catalog::insert_image(
                    tx,
                    &NewImage {
                        root_id: root.id,
                        path: format!("{d}{SEP}{i}.jpg"),
                        dir: d.clone(),
                        file_name: format!("{i}.jpg"),
                        ext: "jpg".into(),
                        kind: ImageKind::Raster,
                        size: 1,
                        mtime: 0,
                        quick_hash: "h".into(),
                        meta: Default::default(),
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
        let count = |folders: &[(&str, bool)]| {
            cat.conn.execute("DELETE FROM project_folders", []).unwrap();
            for (p, rec) in folders {
                cat.conn
                    .execute(
                        "INSERT INTO project_folders (project_id, path, recursive) VALUES (1, ?1, ?2)",
                        params![p, rec],
                    )
                    .unwrap();
            }
            let n = cat.count_images(&Scope::Project { id: 1 }).unwrap();
            let list = cat
                .query_images(&ImageQuery::new(Scope::Project { id: 1 }))
                .unwrap();
            assert_eq!(list.len() as i64, n);
            n
        };
        cat.create_project("p", "").unwrap();
        let a = dirs[0].as_str();
        let ab = format!("{a}{SEP}b");
        assert_eq!(count(&[]), 0);
        assert_eq!(count(&[(r, true)]), 4, "Laufwerkswurzel");
        assert_eq!(count(&[(r, false)]), 0);
        assert_eq!(count(&[(a, true)]), 3, "a, a/b, a/b/c – nicht ab");
        assert_eq!(count(&[(a, false)]), 1);
        assert_eq!(count(&[(a, true), (&ab, true)]), 3, "überlappend: einmal");
        assert_eq!(count(&[(a, false), (&ab, false)]), 2);
        assert_eq!(count(&[(&dirs[2], true), (&ab, false)]), 2);
    }

    #[test]
    fn presets_keep_only_their_groups_and_nothing_photo_specific() {
        let cat = Catalog::open_in_memory().unwrap();
        let p = DevelopParams::from_json(&serde_json::json!({
            "exposure": 1.0, "saturation": -100.0, "rotate": 1,
            "denoise": { "id": "7-dn-1", "amount": 80.0 }
        }));
        let g = TransferGroups {
            basic: false,
            color: true,
            ..TransferGroups::default()
        };
        cat.create_preset("schwarzweiß", &p, g).unwrap();
        cat.create_preset("Alles", &p, TransferGroups::default())
            .unwrap();
        let list = cat.presets().unwrap();
        assert_eq!(
            list.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["Alles", "schwarzweiß"],
            "alphabetisch, ohne Groß/klein"
        );
        let bw = &list[1];
        assert_eq!(bw.groups, g);
        assert_eq!(bw.params.saturation, -100.0);
        assert_eq!(
            bw.params.exposure, 0.0,
            "nicht gewählte Gruppen bleiben neutral"
        );
        assert!(bw.params.denoise.is_none() && bw.params.rotate == 0);
        cat.rename_preset(bw.id, "SW").unwrap();
        cat.delete_preset(list[0].id).unwrap();
        let list = cat.presets().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "SW");
    }
}
