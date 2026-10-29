//! KI: Embeddings und automatische Schlagworte.

use super::*;

impl Catalog {
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

    /// Embeddings nach `after` (Bild-ID), aufsteigend, höchstens `limit` –
    /// zum seitenweisen Durchgehen aller analysierten Fotos.
    pub fn embeddings_after(
        &self,
        model: &str,
        after: i64,
        limit: usize,
    ) -> Result<Vec<(i64, Vec<u8>)>> {
        let mut st = self.conn.prepare_cached(
            "SELECT e.image_id, e.vector FROM embeddings e JOIN images i ON i.id = e.image_id
             WHERE e.model = ?1 AND e.image_id > ?2 AND i.missing = 0
             ORDER BY e.image_id LIMIT ?3",
        )?;
        let rows = st.query_map(params![model, after, limit as i64], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Fortschritt beim seitenweisen Durchgehen mit [`Self::embeddings_after`]:
    /// (bis einschließlich `after` erledigt, alle)
    pub fn embeddings_progress(&self, model: &str, after: i64) -> Result<(i64, i64)> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(e.image_id <= ?2), 0), COUNT(*) FROM embeddings e
             JOIN images i ON i.id = e.image_id WHERE e.model = ?1 AND i.missing = 0",
            params![model, after],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }

    /// KI-Schlagworte ergänzen, ohne die vorhandenen zu ersetzen (z. B. neu
    /// hinzugekommene Tierarten für schon analysierte Fotos).
    pub fn add_ai_tags(&mut self, items: &[(i64, Vec<(String, f64)>)]) -> Result<()> {
        self.with_tx(|tx| {
            for (image_id, tags) in items {
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
            }
            Ok(())
        })
    }

    /// KI-Schlagworte mit diesen Namen (`names`, z. B. alle Tierarten) durch
    /// neue ersetzen – die übrigen KI- und alle eigenen Schlagworte bleiben.
    pub fn replace_ai_tags_named(
        &mut self,
        items: &[(i64, Vec<(String, f64)>)],
        names: &[&str],
    ) -> Result<()> {
        self.with_tx(|tx| {
            let mut ids: Vec<i64> = Vec::new();
            {
                let mut st = tx.prepare_cached("SELECT id FROM tags WHERE name = ?1")?;
                for n in names {
                    if let Some(id) = st.query_row([n], |r| r.get(0)).optional()? {
                        ids.push(id);
                    }
                }
            }
            // IDs sind Zahlen aus der Datenbank – direkt in die Abfrage
            let list = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
            for (image_id, tags) in items {
                if !list.is_empty() {
                    tx.execute(
                        &format!(
                            "DELETE FROM image_tags WHERE image_id = ?1 AND source = 'ai' AND tag_id IN ({list})"
                        ),
                        [image_id],
                    )?;
                }
                for (name, confidence) in tags {
                    tx.execute("INSERT INTO tags (name) VALUES (?1) ON CONFLICT (name) DO NOTHING", [name])?;
                    tx.execute(
                        "INSERT INTO image_tags (image_id, tag_id, source, confidence)
                         SELECT ?1, id, 'ai', ?3 FROM tags WHERE name = ?2
                         ON CONFLICT (image_id, tag_id) DO NOTHING",
                        params![image_id, name, confidence],
                    )?;
                }
            }
            Ok(())
        })
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
}
