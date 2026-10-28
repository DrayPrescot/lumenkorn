//! Schema-Migrationen und ihre Anwendung.

use super::*;

/// Schema-Migrationen. Index + 1 = `user_version` nach Anwendung.
/// Neue Migrationen nur hinten anhängen, bestehende nie verändern.
pub(super) const MIGRATIONS: &[&str] = &[
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
    // v4: Volltextindex für die Suche (siehe `SEARCH_INDEX`; seit v11 halten
    // Trigger ihn nicht mehr aktuell, sondern markieren nur – `flush_search`)
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
    // v23: EXIF aus RW2 und ORF wurde nicht gelesen (fehlende Ausrichtung,
    // Datum, Kamera). Ungültige Änderungszeit: Der nächste Abgleich liest die
    // Metadaten neu, Bewertungen und Bearbeitungen bleiben.
    "UPDATE images SET mtime = -1 WHERE lower(ext) IN ('rw2', 'orf');",
    // v24: Vom Nutzer bearbeitete Metadaten (siehe `IMAGE_META`)
    IMAGE_META,
    // v25: RW2 bekommt die Verzeichnungskorrektur aus den Kameradaten (statt
    // keiner bzw. Lensfun) – Vorschaubilder neu erzeugen
    "UPDATE images SET thumb_state = 0 WHERE lower(ext) = 'rw2';",
    // v26: Exportierte Dateien (`exports.rs`), etwa zum Hochladen der
    // Exporte eines Projekts
    "CREATE TABLE exports (
        path        TEXT PRIMARY KEY,
        image_id    INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
        exported_at INTEGER NOT NULL
    );
    CREATE INDEX exports_image ON exports(image_id);",
    // v27: Je Ordner, ob seine Unterordner zur Bibliothek gehören
    // (`scanner::set_subfolder_rules`); ohne Eintrag gilt der Standard aus
    // den Einstellungen
    "CREATE TABLE folder_options (
        path        TEXT PRIMARY KEY,
        subfolders  INTEGER NOT NULL
    );",
    // v28: `has_image_path` (Export überschreibt nie ein Foto) vergleicht
    // ohne Groß-/Kleinschreibung; der Index auf `path` ist binär und hilft
    // dabei nicht – ohne diesen lief je Zieldatei ein Durchgang über alle Fotos
    "CREATE INDEX images_path_nocase ON images(path COLLATE NOCASE);",
];

/// v24: Metadaten, die der Nutzer geändert hat (`imagemeta.rs`). Ein Eintrag
/// besteht nur, solange mindestens ein Feld überschrieben ist. NULL heißt
/// „nicht überschrieben“. Titel, Beschreibung, Autor und Urheberrecht gibt es
/// nur hier; die übrigen Felder stehen zusätzlich in `images` (damit Suche,
/// Sortierung und Statistik sie sofort nutzen), `f_*` merkt sich den Wert
/// aus der Datei, um ihn beim Zurücksetzen wiederherzustellen. Ein erneutes
/// Einlesen der Datei aktualisiert `f_*` und lässt die Überschreibung gelten.
/// Titel, Beschreibung und Autor sind durchsuchbar.
pub(super) const IMAGE_META: &str = r#"
    CREATE TABLE image_meta (
        image_id        INTEGER PRIMARY KEY REFERENCES images(id) ON DELETE CASCADE,
        title           TEXT,
        description     TEXT,
        creator         TEXT,
        copyright       TEXT,
        captured_at     TEXT,
        camera          TEXT,
        lens            TEXT,
        iso             INTEGER,
        exposure        TEXT,
        aperture        REAL,
        focal_length    REAL,
        f_captured_at   TEXT,
        f_camera        TEXT,
        f_lens          TEXT,
        f_iso           INTEGER,
        f_exposure      TEXT,
        f_aperture      REAL,
        f_focal_length  REAL
    );

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
           || COALESCE(' ' || NULLIF(i.place, ''), '')
           || COALESCE((SELECT ' ' || COALESCE(m.title, '') || ' ' || COALESCE(m.description, '')
                               || ' ' || COALESCE(m.creator, '')
                        FROM image_meta m WHERE m.image_id = i.id), '')
    FROM images i;

    CREATE TRIGGER search_meta_ins AFTER INSERT ON image_meta BEGIN
        INSERT OR IGNORE INTO search_dirty (id) VALUES (NEW.image_id);
    END;
    CREATE TRIGGER search_meta_upd AFTER UPDATE OF title, description, creator ON image_meta BEGIN
        INSERT OR IGNORE INTO search_dirty (id) VALUES (NEW.image_id);
    END;
    CREATE TRIGGER search_meta_del AFTER DELETE ON image_meta BEGIN
        INSERT OR IGNORE INTO search_dirty (id) VALUES (OLD.image_id);
    END;
"#;

/// v21: Aufnahmeort je Foto – Koordinaten (aus EXIF oder vom Nutzer gesetzt)
/// und Ortsname für die Suche. `geo_state = 0`: GPS aus EXIF noch nicht
/// gelesen (Fotos aus der Zeit davor, wird im Hintergrund nachgetragen).
/// `place`: Ortsname, `''` = keiner gefunden (nicht erneut fragen), NULL =
/// noch nicht ermittelt. `geo_names` merkt sich Ortsnamen je Koordinate
/// (gerundet), damit nicht jedes Foto eine Anfrage an OpenStreetMap kostet.
pub(super) const GEO: &str = r#"
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
pub(super) const SEARCH_DIRTY: &str = r#"
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
/// (auch Umlaute) und Akzente spielen keine Rolle.
///
/// Ursprünglich (v4) hielten Trigger den Index bei jeder Änderung sofort
/// aktuell. Seit Migration v11 (`SEARCH_DIRTY`) markieren sie Fotos nur noch
/// als veraltet (Tabelle `search_dirty`); neu aufgebaut wird der Eintrag erst
/// von `flush_search`. Code, der Dateiname, Ordner, Kamera, Objektiv,
/// Schlagworte oder Personennamen ändert, muss `flush_search` danach aufrufen
/// (`with_tx` tut es am Ende der Transaktion), sonst findet die Suche die
/// Änderung nicht.
pub(super) const SEARCH_INDEX: &str = r#"
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

impl Catalog {
    pub(super) fn migrate(&mut self) -> Result<()> {
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
}
