//! Vorschaubilder: Aufträge und Zustände.

use super::*;

impl Catalog {
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

    /// Wie [`Self::thumb_job`] für viele Fotos in einer Abfrage. Gelöschte
    /// Fotos fehlen im Ergebnis.
    pub fn thumb_jobs(&self, ids: &[i64]) -> Result<HashMap<i64, ThumbJob>> {
        let ids = serde_json::to_string(ids)?;
        let mut st = self.conn.prepare_cached(&format!(
            "{THUMB_JOB_SELECT} WHERE i.id IN (SELECT value FROM json_each(?1))"
        ))?;
        let rows = st.query_map([ids], thumb_job_from_row)?;
        let mut out = HashMap::new();
        for job in rows {
            let job = job?;
            out.insert(job.id, job);
        }
        Ok(out)
    }

    /// Setzt den Thumbnail-Zustand mehrerer Fotos in einer Transaktion
    /// (`true` = fertig, `false` = fehlgeschlagen).
    pub fn set_thumb_states(&mut self, states: &[(i64, bool)]) -> Result<()> {
        self.with_tx(|tx| {
            let mut st = tx.prepare_cached("UPDATE images SET thumb_state = ?2 WHERE id = ?1")?;
            for (id, ok) in states {
                st.execute(params![id, if *ok { 1 } else { 2 }])?;
            }
            Ok(())
        })
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
            crate::imagemeta::reapply(&self.conn, id)?;
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
}
