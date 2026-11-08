//! Bearbeitung ins Original übernommen: Die Datei eines Fotos wurde durch die
//! entwickelte Fassung ersetzt, das bisherige Original liegt im Ordner
//! „Original“ daneben (`scanner::original_dir_for`). Hier steht, wo es liegt und
//! welcher Schnappschuss die Bearbeitung davor hält, damit sich beides
//! zurückholen lässt. Die Dateien selbst bewegt `src-tauri/originals.rs`.

use super::*;

/// Ein Foto, dessen bisheriges Original gesichert ist
#[derive(Debug, Clone, PartialEq)]
pub struct SavedOriginal {
    pub image_id: i64,
    /// Datei des Fotos (die übernommene Fassung)
    pub photo: String,
    /// Gesichertes Original
    pub backup: String,
    /// Schnappschuss mit der Bearbeitung vor der Übernahme (fehlt, wenn ihn
    /// jemand gelöscht hat)
    pub snapshot_id: Option<i64>,
}

/// Was die Übernahme ins Original über ein Foto wissen muss
#[derive(Debug, Clone, PartialEq)]
pub struct BakeCandidate {
    pub id: i64,
    pub path: String,
    pub kind: ImageKind,
    pub missing: bool,
    /// Hat eine aktuelle Bearbeitung
    pub edited: bool,
    /// Ein Original ist schon gesichert
    pub saved: bool,
}

impl Catalog {
    /// Gesicherte Originale der Fotos `ids` (Fotos ohne fehlen)
    pub fn saved_originals(&self, ids: &[i64]) -> Result<Vec<SavedOriginal>> {
        let mut st = self.conn.prepare_cached(
            "SELECT o.image_id, i.path, o.path, o.snapshot_id
             FROM originals o JOIN images i ON i.id = o.image_id
             WHERE o.image_id IN (SELECT value FROM json_each(?1))
             ORDER BY o.image_id",
        )?;
        let rows = st.query_map([serde_json::to_string(ids)?], |r| {
            Ok(SavedOriginal {
                image_id: r.get(0)?,
                photo: r.get(1)?,
                backup: r.get(2)?,
                snapshot_id: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Angaben zu den Fotos `ids` für die Übernahme ins Original, in derselben
    /// Reihenfolge (gelöschte Fotos fehlen)
    pub fn bake_candidates(&self, ids: &[i64]) -> Result<Vec<BakeCandidate>> {
        let mut st = self.conn.prepare_cached(
            "SELECT i.id, i.path, i.kind, i.missing,
                    EXISTS (SELECT 1 FROM edits e WHERE e.image_id = i.id AND e.is_current = 1),
                    EXISTS (SELECT 1 FROM originals o WHERE o.image_id = i.id)
             FROM json_each(?1) j JOIN images i ON i.id = j.value
             ORDER BY j.key",
        )?;
        let rows = st.query_map([serde_json::to_string(ids)?], |r| {
            Ok(BakeCandidate {
                id: r.get(0)?,
                path: r.get(1)?,
                kind: ImageKind::parse(&r.get::<_, String>(2)?),
                missing: r.get(3)?,
                edited: r.get(4)?,
                saved: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Die Datei von Foto `id` ist jetzt die entwickelte Fassung: Die
    /// Bearbeitung kommt in den Schnappschuss `snapshot` und wird zurückgesetzt
    /// (sonst wirkte sie doppelt). `backup`: wohin das bisherige Original kam;
    /// ohne bleibt ein schon gesichertes Original eingetragen.
    pub fn bake_done(&mut self, id: i64, backup: Option<&str>, snapshot: &str) -> Result<()> {
        let t = now();
        self.with_tx(|tx| {
            let n = tx.execute(
                "INSERT INTO edits (image_id, name, params, is_current, created_at, updated_at)
                 SELECT image_id, ?2, params, 0, ?3, ?3 FROM edits
                 WHERE image_id = ?1 AND is_current = 1",
                params![id, snapshot, t],
            )?;
            let snapshot_id = (n == 1).then(|| tx.last_insert_rowid());
            Self::write_develop(tx, id, &DevelopParams::default(), t)?;
            if let Some(b) = backup {
                tx.execute(
                    "INSERT INTO originals (image_id, path, snapshot_id, created_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(image_id) DO UPDATE
                     SET path = ?2, snapshot_id = ?3, created_at = ?4",
                    params![id, b, snapshot_id, t],
                )?;
            }
            Ok(())
        })
    }

    /// Das gesicherte Original von Foto `id` liegt wieder an seinem Platz: Die
    /// Bearbeitung von davor wird wieder die aktuelle. Was seit der Übernahme
    /// bearbeitet wurde, kommt in den Schnappschuss `snapshot`.
    pub fn restore_done(&mut self, id: i64, snapshot: &str) -> Result<()> {
        let t = now();
        self.with_tx(|tx| {
            let saved: Option<i64> = tx
                .query_row(
                    "SELECT snapshot_id FROM originals WHERE image_id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            let before: Option<String> = match saved {
                Some(s) => tx
                    .query_row(
                        "SELECT params FROM edits WHERE id = ?1 AND image_id = ?2 AND is_current = 0",
                        params![s, id],
                        |r| r.get(0),
                    )
                    .optional()?,
                None => None,
            };
            tx.execute(
                "INSERT INTO edits (image_id, name, params, is_current, created_at, updated_at)
                 SELECT image_id, ?2, params, 0, ?3, ?3 FROM edits
                 WHERE image_id = ?1 AND is_current = 1",
                params![id, snapshot, t],
            )?;
            let params = match &before {
                Some(json) => DevelopParams::from_json(&serde_json::from_str(json)?),
                None => DevelopParams::default(),
            };
            Self::write_develop(tx, id, &params, t)?;
            // Der Schnappschuss ist jetzt die aktuelle Bearbeitung
            if let Some(s) = saved {
                tx.execute(
                    "DELETE FROM edits WHERE id = ?1 AND is_current = 0",
                    [s],
                )?;
            }
            tx.execute("DELETE FROM originals WHERE image_id = ?1", [id])?;
            Ok(())
        })
    }

    /// Das gesicherte Original von Foto `id` ist gelöscht: Eintrag und
    /// Schnappschuss von vor der Übernahme entfernen (er gehörte zum Original
    /// und wirkte auf die übernommene Fassung doppelt).
    pub fn forget_original(&mut self, id: i64) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "DELETE FROM edits WHERE is_current = 0 AND id =
                    (SELECT snapshot_id FROM originals WHERE image_id = ?1)",
                [id],
            )?;
            tx.execute("DELETE FROM originals WHERE image_id = ?1", [id])?;
            Ok(())
        })
    }
}
