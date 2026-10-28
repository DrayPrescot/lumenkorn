//! Bilder: Abfragen, Bewertung, Markierung, Entfernen.

use super::*;

impl Catalog {
    // ------------------------------------------------------------------
    // Bilder – Abfragen
    // ------------------------------------------------------------------

    /// Liegt unter diesem Pfad ein Foto der Bibliothek? (Groß-/Kleinschreibung
    /// egal – unter Windows ist es dieselbe Datei.) Der Export überschreibt
    /// solche Dateien nie.
    pub fn has_image_path(&self, path: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM images WHERE path = ?1 COLLATE NOCASE)",
            [path],
            |r| r.get(0),
        )?)
    }

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
                    i.gps_lat, i.gps_lon, {EDIT_ASPECT}
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
        if let Some(exts) = q.exts.as_ref().filter(|e| !e.is_empty()) {
            let exts: Vec<String> = exts.iter().map(|e| e.to_ascii_lowercase()).collect();
            parts.push("i.ext IN (SELECT value FROM json_each(?))".into());
            args.push(Value::Text(
                serde_json::to_string(&exts).unwrap_or_else(|_| "[]".into()),
            ));
        }
        if let Some(edited) = q.edited {
            parts.push(format!(
                "{} EXISTS (SELECT 1 FROM edits e WHERE e.image_id = i.id AND e.is_current = 1)",
                if edited { "" } else { "NOT" }
            ));
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
                        ..Default::default()
                    })
                },
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(tr!("Bild {id}", "Photo {id}")))?;
        d.tags = self.image_tags(id)?;
        d.location = self.location(id)?;
        if let Some((title, description, creator, copyright)) = self
            .conn
            .query_row(
                "SELECT title, description, creator, copyright FROM image_meta WHERE image_id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?
        {
            (d.title, d.description, d.creator, d.copyright) =
                (title, description, creator, copyright);
            d.meta_edited = true;
        }
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
            let mut del = tx.prepare_cached("DELETE FROM images WHERE id = ?1")?;
            for id in ids {
                del.execute([id])?;
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

    /// Bilder in der angegebenen Reihenfolge (z. B. nach Ähnlichkeit sortiert),
    /// beschränkt auf einen Bereich.
    pub fn images_in_order(&self, ids: &[i64], filter: &ImageQuery) -> Result<Vec<ImageSummary>> {
        // Nur die Treffer lesen, nicht alle Fotos des Bereichs (bis 100 000).
        // Von `filter` zählen Bereich, Mindestbewertung, Dateitypen und
        // bearbeitet/unbearbeitet.
        let mut q = ImageQuery::new(filter.scope.clone());
        q.min_rating = filter.min_rating;
        q.exts = filter.exts.clone();
        q.edited = filter.edited;
        q.ids = Some(ids.to_vec());
        let all = self.query_images(&q)?;
        let by_id: std::collections::HashMap<i64, ImageSummary> =
            all.into_iter().map(|i| (i.id, i)).collect();
        Ok(ids.iter().filter_map(|id| by_id.get(id).cloned()).collect())
    }
}
