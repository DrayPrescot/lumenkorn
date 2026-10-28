//! Schreibzugriffe für den Scanner und das Umbenennen.

use super::*;

impl Catalog {
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
        // Vom Nutzer geänderte Metadaten gelten weiter
        crate::imagemeta::reapply(tx, id)?;
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

    /// Angaben zu Fotos, die umbenannt werden sollen (in der Reihenfolge von
    /// `ids`; unbekannte IDs fehlen im Ergebnis).
    pub fn rename_infos(&self, ids: &[i64]) -> Result<Vec<RenameInfo>> {
        let mut st = self.conn.prepare_cached(
            "SELECT id, path, dir, file_name, captured_at, camera, lens, mtime, missing
             FROM images WHERE id = ?1",
        )?;
        let mut out = Vec::new();
        for id in ids {
            if let Some(r) = st
                .query_row([id], |r| {
                    Ok(RenameInfo {
                        id: r.get(0)?,
                        path: r.get(1)?,
                        dir: r.get(2)?,
                        file_name: r.get(3)?,
                        captured_at: r.get(4)?,
                        camera: r.get(5)?,
                        lens: r.get(6)?,
                        mtime: r.get(7)?,
                        missing: r.get(8)?,
                    })
                })
                .optional()?
            {
                out.push(r);
            }
        }
        Ok(out)
    }

    /// Nach dem Umbenennen der Dateien: neuer Pfad und Dateiname je Foto
    /// (`(id, path, file_name)`). Ordner und Endung bleiben. Alles in einer
    /// Transaktion; ID, Bewertung, Schlagworte, Bearbeitungen und Gesichter
    /// bleiben beim Foto, der Suchindex wird nachgeführt.
    pub fn rename_images(&mut self, items: &[(i64, String, String)]) -> Result<()> {
        self.with_tx(|tx| {
            // `path` ist eindeutig: bei Tausch zweier Namen kurz Platzhalter
            for (id, _, _) in items {
                tx.execute(
                    "UPDATE images SET path = '?rename' || id WHERE id = ?1",
                    [id],
                )?;
            }
            for (id, path, name) in items {
                tx.execute(
                    "UPDATE images SET path = ?2, file_name = ?3, missing = 0 WHERE id = ?1",
                    params![id, path, name],
                )?;
            }
            Ok(())
        })
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
}
