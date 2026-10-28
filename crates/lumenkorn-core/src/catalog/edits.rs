//! Bearbeitungen, Schnappschüsse und Presets.

use super::*;

impl Catalog {
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
}
