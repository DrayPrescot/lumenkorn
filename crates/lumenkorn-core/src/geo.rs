//! Aufnahmeort: Koordinaten und Ortsname je Foto (Migration v21).
//!
//! Koordinaten kommen aus den GPS-Daten der Kamera oder werden vom Nutzer
//! gesetzt (Ortssuche, Karte). Vom Nutzer gesetzte Orte überschreibt kein
//! erneutes Einlesen der Datei. Ortsnamen ermittelt die App (OpenStreetMap)
//! und legt sie hier ab; sie fließen in den Suchindex.

use crate::catalog::{Catalog, flush_search};
use crate::error::Result;
use crate::model::{GeoPoint, ImageQuery, Location};
use rusqlite::{OptionalExtension, params, params_from_iter};

/// Schlüssel für den Ortsnamen-Speicher: Koordinaten auf etwa 1 km gerundet
/// (Fotos am selben Ort teilen sich eine Anfrage).
pub fn name_key(lat: f64, lon: f64) -> String {
    format!("{:.2},{:.2}", lat, lon)
}

impl Catalog {
    /// Aufnahmeort eines Fotos
    pub fn location(&self, id: i64) -> Result<Option<Location>> {
        Ok(self
            .conn
            .query_row(
                "SELECT gps_lat, gps_lon, place, gps_source FROM images
                 WHERE id = ?1 AND gps_lat IS NOT NULL AND gps_lon IS NOT NULL",
                [id],
                |r| {
                    let place: Option<String> = r.get(2)?;
                    Ok(Location {
                        lat: r.get(0)?,
                        lon: r.get(1)?,
                        place: place.filter(|p| !p.is_empty()),
                        manual: r.get::<_, Option<String>>(3)?.as_deref() == Some("manual"),
                    })
                },
            )
            .optional()?)
    }

    /// Ort für mehrere Fotos setzen (`None` entfernt ihn). Gilt als vom
    /// Nutzer gesetzt – erneutes Einlesen der Dateien ändert ihn nicht mehr.
    /// `place`: Ortsname, falls bekannt (sonst ermittelt ihn die App später).
    pub fn set_location(
        &mut self,
        ids: &[i64],
        pos: Option<(f64, f64)>,
        place: Option<&str>,
    ) -> Result<()> {
        if let Some((lat, lon)) = pos
            && !(lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0)
        {
            return Err(crate::Error::Invalid(format!("{lat}, {lon}")));
        }
        self.with_tx(|tx| {
            let mut st = tx.prepare_cached(
                "UPDATE images SET gps_lat = ?2, gps_lon = ?3, place = ?4,
                        gps_source = 'manual', geo_state = 1
                 WHERE id = ?1",
            )?;
            for id in ids {
                st.execute(params![
                    id,
                    pos.map(|p| p.0),
                    pos.map(|p| p.1),
                    pos.and(place)
                ])?;
            }
            Ok(())
        })
    }

    /// Viele Orte auf einmal setzen (übernommene Vorschläge): je Foto
    /// Koordinaten und Ortsname (falls bekannt). Wie [`Catalog::set_location`].
    pub fn set_locations(&mut self, items: &[(i64, f64, f64, Option<String>)]) -> Result<()> {
        if let Some((_, lat, lon, _)) = items.iter().find(|(_, la, lo, _)| {
            !(la.is_finite() && lo.is_finite() && la.abs() <= 90.0 && lo.abs() <= 180.0)
        }) {
            return Err(crate::Error::Invalid(format!("{lat}, {lon}")));
        }
        self.with_tx(|tx| {
            let mut st = tx.prepare_cached(
                "UPDATE images SET gps_lat = ?2, gps_lon = ?3, place = ?4,
                        gps_source = 'manual', geo_state = 1
                 WHERE id = ?1",
            )?;
            for (id, lat, lon, place) in items {
                let place = place.as_deref().filter(|p| !p.is_empty());
                st.execute(params![id, lat, lon, place])?;
            }
            Ok(())
        })
    }

    /// Fotos der Abfrage mit bekanntem Ort (für die Karte)
    pub fn geo_points(&self, q: &ImageQuery) -> Result<Vec<GeoPoint>> {
        let (where_sql, args) = self.where_clause(q)?;
        let mut st = self.conn.prepare(&format!(
            "SELECT i.id, i.gps_lat, i.gps_lon FROM images i
             WHERE {where_sql} AND i.gps_lat IS NOT NULL AND i.gps_lon IS NOT NULL"
        ))?;
        let rows = st.query_map(params_from_iter(args), |r| {
            Ok(GeoPoint {
                id: r.get(0)?,
                lat: r.get(1)?,
                lon: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Fotos, deren GPS-Daten noch nicht gelesen wurden: (ID, Pfad)
    pub fn geo_pending(&self, limit: usize) -> Result<Vec<(i64, String)>> {
        let mut st = self.conn.prepare_cached(
            "SELECT id, path FROM images WHERE geo_state = 0 AND missing = 0 LIMIT ?1",
        )?;
        let rows = st.query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Ergebnis des Nachtragens: GPS aus EXIF (falls vorhanden) übernehmen,
    /// außer der Nutzer hat inzwischen selbst einen Ort gesetzt.
    pub fn set_exif_gps(&mut self, items: &[(i64, Option<(f64, f64)>)]) -> Result<()> {
        self.with_tx(|tx| {
            let mut st = tx.prepare_cached(
                "UPDATE images SET geo_state = 1,
                        gps_lat = CASE WHEN gps_source IS NULL THEN ?2 ELSE gps_lat END,
                        gps_lon = CASE WHEN gps_source IS NULL THEN ?3 ELSE gps_lon END,
                        gps_source = CASE WHEN gps_source IS NULL AND ?2 IS NOT NULL
                                          THEN 'exif' ELSE gps_source END
                 WHERE id = ?1",
            )?;
            for (id, pos) in items {
                st.execute(params![id, pos.map(|p| p.0), pos.map(|p| p.1)])?;
            }
            Ok(())
        })
    }

    /// Fotos mit Koordinaten, aber noch ohne Ortsnamen: (ID, Breite, Länge)
    pub fn place_pending(&self, limit: usize) -> Result<Vec<(i64, f64, f64)>> {
        let mut st = self.conn.prepare_cached(
            "SELECT id, gps_lat, gps_lon FROM images
             WHERE gps_lat IS NOT NULL AND place IS NULL AND missing = 0 LIMIT ?1",
        )?;
        let rows = st.query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Ortsnamen für Fotos ablegen (`""`: keiner gefunden). Nur, wo die
    /// Koordinaten noch dieselben sind – der Nutzer kann den Ort inzwischen
    /// geändert haben.
    pub fn set_place(&mut self, items: &[(i64, f64, f64)], place: &str) -> Result<()> {
        self.with_tx(|tx| {
            let mut st = tx.prepare_cached(
                "UPDATE images SET place = ?4 WHERE id = ?1 AND gps_lat = ?2 AND gps_lon = ?3",
            )?;
            for (id, lat, lon) in items {
                st.execute(params![id, lat, lon, place])?;
            }
            Ok(())
        })
    }

    /// Gemerkter Ortsname zu einer Koordinate (siehe [`name_key`])
    pub fn cached_place(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .prepare_cached("SELECT place FROM geo_names WHERE key = ?1")?
            .query_row([key], |r| r.get(0))
            .optional()?)
    }

    pub fn cache_place(&self, key: &str, place: &str) -> Result<()> {
        self.conn
            .prepare_cached("INSERT OR REPLACE INTO geo_names (key, place) VALUES (?1, ?2)")?
            .execute(params![key, place])?;
        Ok(())
    }

    /// Ortsnamen, die nicht in lateinischer Schrift vorliegen (frühere
    /// Abfragen lieferten etwa für Japan Kanji), verwerfen – sie werden im
    /// Hintergrund neu ermittelt. Liefert die Zahl der betroffenen Namen.
    pub fn reset_non_latin_places(&mut self) -> Result<usize> {
        let mut names: Vec<String> = Vec::new();
        for sql in [
            "SELECT DISTINCT place FROM images WHERE place IS NOT NULL AND place <> ''",
            "SELECT DISTINCT place FROM geo_names WHERE place <> ''",
        ] {
            let mut st = self.conn.prepare(sql)?;
            let rows = st.query_map([], |r| r.get::<_, String>(0))?;
            for n in rows {
                let n = n?;
                if !is_latin(&n) && !names.contains(&n) {
                    names.push(n);
                }
            }
        }
        if names.is_empty() {
            return Ok(0);
        }
        self.with_tx(|tx| {
            for n in &names {
                tx.execute("UPDATE images SET place = NULL WHERE place = ?1", [n])?;
                tx.execute("DELETE FROM geo_names WHERE place = ?1", [n])?;
            }
            Ok(())
        })?;
        Ok(names.len())
    }

    /// Suchindex nach außerhalb von `with_tx` geänderten Ortsnamen nachführen
    pub fn flush_search_index(&self) -> Result<()> {
        flush_search(&self.conn)
    }
}

/// Ist der Text in lateinischer Schrift (auch mit Akzenten wie „Reykjavík“,
/// „Đà Nẵng“)? Ziffern und Satzzeichen zählen nicht. Kanji, Kyrillisch,
/// Arabisch usw. nicht – in der Oberfläche und im Fotoalbum sollen Namen
/// lesbar sein.
pub fn is_latin(s: &str) -> bool {
    s.chars().filter(|c| c.is_alphabetic()).all(|c| {
        matches!(c as u32,
            0x41..=0x5A | 0x61..=0x7A | 0xAA | 0xBA | 0xC0..=0x24F | 0x1E00..=0x1EFF | 0x2C60..=0x2C7F | 0xA720..=0xA7FF)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ImageQuery, Scope};

    #[test]
    fn latin_script_is_recognised() {
        for ok in [
            "München, Bayern",
            "Reykjavík",
            "Đà Nẵng",
            "Kyōto",
            "Saint-Étienne 2",
            "",
        ] {
            assert!(is_latin(ok), "{ok}");
        }
        for bad in ["京都市", "Москва", "Kyoto, 京都府", "القاهرة"] {
            assert!(!is_latin(bad), "{bad}");
        }
    }

    #[test]
    fn non_latin_places_are_looked_up_again() {
        let (_tmp, mut cat, id) = catalog_with_image();
        cat.set_exif_gps(&[(id, Some((35.0, 135.7)))]).unwrap();
        cat.set_place(&[(id, 35.0, 135.7)], "京都市, 京都府, Japan")
            .unwrap();
        cat.cache_place("k", "京都市, 京都府, Japan").unwrap();
        cat.cache_place("m", "München, Bayern").unwrap();
        assert_eq!(cat.reset_non_latin_places().unwrap(), 1);
        assert_eq!(
            cat.place_pending(10).unwrap().len(),
            1,
            "wird neu abgefragt"
        );
        assert_eq!(cat.cached_place("k").unwrap(), None);
        assert_eq!(
            cat.cached_place("m").unwrap().as_deref(),
            Some("München, Bayern")
        );
        assert_eq!(cat.reset_non_latin_places().unwrap(), 0);
    }

    fn catalog_with_image() -> (tempfile::TempDir, Catalog, i64) {
        let tmp = tempfile::tempdir().unwrap();
        let cat = Catalog::open(&tmp.path().join("k.db")).unwrap();
        cat.conn
            .execute_batch(
                "INSERT INTO roots (id, path, added_at) VALUES (1, '/f', 0);
                 INSERT INTO images (id, root_id, path, dir, file_name, ext, kind, size, mtime,
                                     quick_hash, added_at, geo_state)
                 VALUES (1, 1, '/f/a.jpg', '/f', 'a.jpg', 'jpg', 'raster', 1, 0, 'h', 0, 0);",
            )
            .unwrap();
        (tmp, cat, 1)
    }

    #[test]
    fn location_is_set_found_and_searchable() {
        let (_tmp, mut cat, id) = catalog_with_image();
        assert_eq!(cat.location(id).unwrap(), None);
        // EXIF-Nachtrag
        cat.set_exif_gps(&[(id, Some((48.137, 11.575)))]).unwrap();
        let loc = cat.location(id).unwrap().unwrap();
        assert!(!loc.manual && loc.place.is_none());
        assert_eq!(cat.place_pending(10).unwrap().len(), 1);
        cat.set_place(&[(id, 48.137, 11.575)], "München, Bayern, Deutschland")
            .unwrap();
        cat.flush_search_index().unwrap();
        let q = |cat: &Catalog, s: &str| {
            let mut q = ImageQuery::new(Scope::All);
            q.search = Some(s.into());
            cat.query_images(&q).unwrap().len()
        };
        assert_eq!(q(&cat, "münchen"), 1);
        // Vom Nutzer gesetzt: neuer Ort, Name direkt aus der Suche
        cat.set_location(&[id], Some((47.0, 8.0)), Some("Luzern, Schweiz"))
            .unwrap();
        let loc = cat.location(id).unwrap().unwrap();
        assert!(loc.manual && loc.place.as_deref() == Some("Luzern, Schweiz"));
        assert_eq!((q(&cat, "luzern"), q(&cat, "münchen")), (1, 0));
        // EXIF überschreibt den Nutzer nicht
        cat.set_exif_gps(&[(id, Some((1.0, 1.0)))]).unwrap();
        assert_eq!(cat.location(id).unwrap().unwrap().lat, 47.0);
        let pts = cat.geo_points(&ImageQuery::new(Scope::All)).unwrap();
        assert_eq!(pts.len(), 1);
        // Entfernen
        cat.set_location(&[id], None, None).unwrap();
        assert_eq!(cat.location(id).unwrap(), None);
        assert!(
            cat.geo_points(&ImageQuery::new(Scope::All))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn invalid_coordinates_are_rejected() {
        let (_tmp, mut cat, id) = catalog_with_image();
        assert!(cat.set_location(&[id], Some((95.0, 0.0)), None).is_err());
        assert!(
            cat.set_location(&[id], Some((f64::NAN, 0.0)), None)
                .is_err()
        );
    }
}
