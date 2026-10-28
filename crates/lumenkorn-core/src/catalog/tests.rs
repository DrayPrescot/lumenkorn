use super::migrations::*;
use super::*;

#[cfg(test)]
mod content_update_tests {
    use super::*;

    #[test]
    fn rename_swaps_names_and_updates_search() {
        let mut cat = Catalog::open_in_memory().unwrap();
        let ids = tests_support::insert_dummy_images(&mut cat, 2);
        let d = "/tmp/lumenkorn-test";
        cat.rename_images(&[
            (ids[0], format!("{d}/bild1.jpg"), "bild1.jpg".into()),
            (ids[1], format!("{d}/bild0.jpg"), "bild0.jpg".into()),
        ])
        .unwrap();
        let infos = cat.rename_infos(&ids).unwrap();
        assert_eq!(infos[0].file_name, "bild1.jpg");
        assert_eq!(infos[1].path, format!("{d}/bild0.jpg"));
        // Suchindex folgt dem neuen Namen
        let n: i64 = cat
            .conn
            .query_row(
                "SELECT COUNT(*) FROM search WHERE rowid = ?1 AND body LIKE '%bild1.jpg%'",
                [ids[0]],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

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

    /// Raster und Filmstreifen zeigen gedrehte und zugeschnittene Fotos in
    /// ihrer neuen Form
    #[test]
    fn summary_aspect_follows_rotation_and_crop() {
        let mut cat = Catalog::open_in_memory().unwrap();
        let ids = tests_support::insert_dummy_images(&mut cat, 3);
        cat.conn
            .execute("UPDATE images SET width = 6000, height = 4000", [])
            .unwrap();
        let rotated = DevelopParams {
            rotate: 1,
            ..Default::default()
        };
        let cropped = DevelopParams {
            rotate: 3,
            crop: Some(lumenkorn_develop::params::Crop {
                x: 0.0,
                y: 0.0,
                w: 0.5,
                h: 1.0,
            }),
            ..Default::default()
        };
        cat.save_develop(&[ids[1]], &rotated).unwrap();
        cat.save_develop(&[ids[2]], &cropped).unwrap();
        let all = cat.query_images(&ImageQuery::new(Scope::All)).unwrap();
        let aspect = |id: i64| all.iter().find(|s| s.id == id).unwrap().aspect;
        assert_eq!(aspect(ids[0]), None);
        assert!((aspect(ids[1]).unwrap() - 4000.0 / 6000.0).abs() < 1e-6);
        assert!((aspect(ids[2]).unwrap() - 2000.0 / 6000.0).abs() < 1e-6);
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

    /// Nachgetragene KI-Schlagworte (Tierarten) ergänzen nur: vorhandene
    /// KI- und eigene Schlagworte bleiben, die Suche findet die neuen.
    #[test]
    fn added_ai_tags_keep_existing_ones() {
        let mut cat = Catalog::open_in_memory().unwrap();
        let ids = tests_support::insert_dummy_images(&mut cat, 3);
        let (a, b, c) = (ids[0], ids[1], ids[2]);
        cat.save_ai_result(a, "m", &[1, 2, 3, 4], &[("Wiese".into(), 0.5)])
            .unwrap();
        cat.save_ai_result(c, "m", &[5, 6, 7, 8], &[]).unwrap();
        cat.add_tag(a, "Urlaub", TagSource::Manual, None).unwrap();
        // Seitenweise: nur Fotos mit Embedding, aufsteigend, ab `after`
        let page: Vec<i64> = cat
            .embeddings_after("m", 0, 10)
            .unwrap()
            .into_iter()
            .map(|r| r.0)
            .collect();
        assert_eq!(page, vec![a, c]);
        assert_eq!(cat.embeddings_after("m", a, 10).unwrap()[0].0, c);
        assert!(cat.embeddings_after("m", c, 10).unwrap().is_empty());
        assert!(cat.embeddings_after("anders", 0, 10).unwrap().is_empty());
        cat.add_ai_tags(&[(a, vec![("Vogel".into(), 0.9), ("Amsel".into(), 0.4)])])
            .unwrap();
        let mut names: Vec<String> = cat
            .image_tags(a)
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        names.sort();
        assert_eq!(names, ["Amsel", "Urlaub", "Vogel", "Wiese"]);
        assert_eq!(hits(&cat, "Amsel"), vec![a]);
        assert!(cat.image_tags(b).unwrap().is_empty());
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
        cat.add_ai_tags(&[(c, vec![("vogel".to_string(), 0.9)])])
            .unwrap();
        exact(&cat, "add_ai_tags");

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

    /// Stand vor v24 nachstellen: Tabelle und Trigger für bearbeitete
    /// Metadaten weg. Die Suchansicht bleibt als Platzhalter, damit `UNDO_V21`
    /// sie ersetzen kann.
    /// Stand vor v26: keine Liste der Exporte
    const UNDO_V26: &str = "DROP TABLE exports;";
    /// Stand vor v27: keine Einstellungen je Ordner
    const UNDO_V27: &str = "DROP TABLE folder_options;";
    /// Stand vor v28: kein Pfad-Index ohne Groß-/Kleinschreibung
    const UNDO_V28: &str = "DROP INDEX images_path_nocase;";

    const UNDO_V24: &str = "DROP TRIGGER search_meta_ins; DROP TRIGGER search_meta_upd;
        DROP TRIGGER search_meta_del; DROP VIEW search_source; DROP TABLE image_meta;
        CREATE VIEW search_source(id, body) AS SELECT 0, '';";

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
                    "UPDATE images SET thumb_state = 1; {UNDO_V28} {UNDO_V27} {UNDO_V26} {UNDO_V24} {UNDO_V22} {UNDO_V21} {UNDO_V11}
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

    /// Die Prüfung beim Export, ob das Ziel ein Foto der Bibliothek ist, nutzt
    /// einen Index (sonst ein Durchgang über alle Fotos je Zieldatei).
    #[test]
    fn path_lookup_ignoring_case_uses_index() {
        let cat = Catalog::open_in_memory().unwrap();
        // Die Unterabfrage steht in einer eigenen Zeile des Plans
        let plans: Vec<String> = cat
            .conn
            .prepare("EXPLAIN QUERY PLAN SELECT EXISTS (SELECT 1 FROM images WHERE path = ?1 COLLATE NOCASE)")
            .unwrap()
            .query_map(["C:/a.jpg"], |r| r.get(3))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert!(
            plans.iter().any(|p| p.contains("images_path_nocase")),
            "{plans:?}"
        );
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
                    "{UNDO_V28} {UNDO_V27} {UNDO_V26} {UNDO_V24} {UNDO_V22} {UNDO_V21} {UNDO_V11} DROP TRIGGER face_id_floor_del; DROP TABLE face_id_floor;
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
