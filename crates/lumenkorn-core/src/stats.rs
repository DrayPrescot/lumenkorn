//! Statistik über die Fotos einer Ansicht: Kameras, Objektive, Brennweiten,
//! Blenden, Schlagworte … Es gelten dieselben Filter wie in der Fotoliste
//! (Bereich, Suche, Mindestbewertung, Dateityp).

use crate::catalog::Catalog;
use crate::error::Result;
use crate::model::ImageQuery;
use rusqlite::params_from_iter;
use rusqlite::types::Value;
use serde::{Deserialize, Serialize};

/// Anzahl Fotos je Eintrag (Kamera, Objektiv, Schlagwort …)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LabelCount {
    pub label: String,
    pub count: i64,
}

/// Anzahl Fotos je Zahlenwert (Brennweite, Blende, ISO)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ValueCount {
    pub value: f64,
    pub count: i64,
}

/// Anzahl Fotos je Dateiendung
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileTypeCount {
    /// Endung, klein und ohne Punkt
    pub ext: String,
    /// „raw“ oder „raster“
    pub kind: String,
    pub count: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PhotoStats {
    pub total: i64,
    pub raw: i64,
    pub edited: i64,
    pub with_location: i64,
    /// Erste und letzte Aufnahme (ISO-Zeit)
    pub first: Option<String>,
    pub last: Option<String>,
    /// Fotos mit 0 bis 5 Sternen
    pub ratings: [i64; 6],
    pub cameras: Vec<LabelCount>,
    pub lenses: Vec<LabelCount>,
    /// Auf ganze Millimeter gerundet
    pub focal_lengths: Vec<ValueCount>,
    /// Auf eine Nachkommastelle gerundet
    pub apertures: Vec<ValueCount>,
    pub isos: Vec<ValueCount>,
    /// Wie gespeichert („1/250“, „2“, „0.5“ …)
    pub exposures: Vec<LabelCount>,
    pub file_types: Vec<FileTypeCount>,
    /// „JJJJ-MM“, aufsteigend
    pub months: Vec<LabelCount>,
    /// Aufnahmen je Stunde 0–23
    pub hours: [i64; 24],
    /// Schlagworte (manuell und KI), häufigste zuerst
    pub keywords: Vec<LabelCount>,
    /// Benannte Personen, häufigste zuerst
    pub people: Vec<LabelCount>,
    /// Land (letzter Teil des Ortsnamens), häufigste zuerst
    pub countries: Vec<LabelCount>,
}

/// Höchstens so viele Einträge in langen Listen
const TOP: i64 = 40;

impl Catalog {
    /// Statistik der Fotos, die `q` liefert (Sortierung und Limit egal).
    pub fn photo_stats(&self, q: &ImageQuery) -> Result<PhotoStats> {
        let (where_sql, args) = self.where_clause(q)?;
        let mut s = PhotoStats::default();

        let one = |sql: &str| -> Result<Vec<Vec<Value>>> {
            let sql = sql.replace("{where}", &where_sql);
            let mut st = self.conn.prepare(&sql)?;
            let n = st.column_count();
            let rows = st.query_map(params_from_iter(args.iter()), |r| {
                (0..n).map(|i| r.get::<_, Value>(i)).collect()
            })?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        };
        let int = |v: &Value| match v {
            Value::Integer(i) => *i,
            Value::Real(f) => *f as i64,
            _ => 0,
        };
        let real = |v: &Value| match v {
            Value::Integer(i) => *i as f64,
            Value::Real(f) => *f,
            _ => 0.0,
        };
        let text = |v: &Value| match v {
            Value::Text(t) => t.clone(),
            Value::Integer(i) => i.to_string(),
            Value::Real(f) => f.to_string(),
            _ => String::new(),
        };
        let labels = |rows: Vec<Vec<Value>>| -> Vec<LabelCount> {
            rows.iter()
                .map(|r| LabelCount {
                    label: text(&r[0]),
                    count: int(&r[1]),
                })
                .filter(|c| !c.label.trim().is_empty())
                .collect()
        };
        let values = |rows: Vec<Vec<Value>>| -> Vec<ValueCount> {
            rows.iter()
                .map(|r| ValueCount {
                    value: real(&r[0]),
                    count: int(&r[1]),
                })
                .filter(|c| c.value > 0.0)
                .collect()
        };

        if let Some(r) = one(
            "SELECT COUNT(*),
                    SUM(i.kind = 'raw'),
                    SUM(EXISTS (SELECT 1 FROM edits e WHERE e.image_id = i.id AND e.is_current = 1)),
                    SUM(i.gps_lat IS NOT NULL),
                    MIN(i.captured_at), MAX(i.captured_at)
             FROM images i WHERE {where}",
        )?
        .first()
        {
            s.total = int(&r[0]);
            s.raw = int(&r[1]);
            s.edited = int(&r[2]);
            s.with_location = int(&r[3]);
            s.first = Some(text(&r[4])).filter(|t| !t.is_empty());
            s.last = Some(text(&r[5])).filter(|t| !t.is_empty());
        }
        if s.total == 0 {
            return Ok(s);
        }

        for r in one("SELECT i.rating, COUNT(*) FROM images i WHERE {where} GROUP BY i.rating")? {
            if let Some(slot) = usize::try_from(int(&r[0]))
                .ok()
                .and_then(|k| s.ratings.get_mut(k))
            {
                *slot = int(&r[1]);
            }
        }
        s.cameras = labels(one(&format!(
            "SELECT TRIM(i.camera), COUNT(*) FROM images i
             WHERE {{where}} AND i.camera IS NOT NULL
             GROUP BY TRIM(i.camera) COLLATE NOCASE ORDER BY 2 DESC, 1 LIMIT {TOP}"
        ))?);
        s.lenses = labels(one(&format!(
            "SELECT TRIM(i.lens), COUNT(*) FROM images i
             WHERE {{where}} AND i.lens IS NOT NULL
             GROUP BY TRIM(i.lens) COLLATE NOCASE ORDER BY 2 DESC, 1 LIMIT {TOP}"
        ))?);
        s.focal_lengths = values(one("SELECT ROUND(i.focal_length), COUNT(*) FROM images i
             WHERE {where} AND i.focal_length > 0
             GROUP BY ROUND(i.focal_length) ORDER BY 1")?);
        s.apertures = values(one("SELECT ROUND(i.aperture, 1), COUNT(*) FROM images i
             WHERE {where} AND i.aperture > 0
             GROUP BY ROUND(i.aperture, 1) ORDER BY 1")?);
        s.isos = values(one("SELECT i.iso, COUNT(*) FROM images i
             WHERE {where} AND i.iso > 0 GROUP BY i.iso ORDER BY 1")?);
        s.exposures = labels(one("SELECT i.exposure, COUNT(*) FROM images i
             WHERE {where} AND i.exposure IS NOT NULL GROUP BY i.exposure")?);
        s.file_types = one("SELECT i.ext, MAX(i.kind), COUNT(*) FROM images i
             WHERE {where} GROUP BY i.ext ORDER BY 3 DESC, 1")?
        .iter()
        .map(|r| FileTypeCount {
            ext: text(&r[0]),
            kind: text(&r[1]),
            count: int(&r[2]),
        })
        .collect();
        s.months = labels(one(
            "SELECT substr(i.captured_at, 1, 7), COUNT(*) FROM images i
             WHERE {where} AND length(i.captured_at) >= 7
             GROUP BY 1 ORDER BY 1",
        )?);
        for r in one(
            "SELECT CAST(substr(i.captured_at, 12, 2) AS INTEGER), COUNT(*) FROM images i
             WHERE {where} AND length(i.captured_at) >= 13 GROUP BY 1",
        )? {
            if let Some(slot) = usize::try_from(int(&r[0]))
                .ok()
                .and_then(|h| s.hours.get_mut(h))
            {
                *slot = int(&r[1]);
            }
        }
        s.keywords = labels(one(&format!(
            "SELECT t.name, COUNT(*) FROM image_tags it
             JOIN tags t ON t.id = it.tag_id
             JOIN images i ON i.id = it.image_id
             WHERE {{where}}
             GROUP BY t.id ORDER BY 2 DESC, 1 LIMIT {TOP}"
        ))?);
        s.people = labels(one(&format!(
            "SELECT p.name, COUNT(DISTINCT f.image_id) FROM faces f
             JOIN persons p ON p.id = f.person_id
             JOIN images i ON i.id = f.image_id
             WHERE {{where}} AND p.name IS NOT NULL AND f.hidden = 0
             GROUP BY p.id ORDER BY 2 DESC, 1 LIMIT {TOP}"
        ))?);

        // Land: letzter Teil des Ortsnamens („München, Bayern, Deutschland“)
        let mut countries: std::collections::HashMap<String, i64> = Default::default();
        for r in one("SELECT i.place, COUNT(*) FROM images i
             WHERE {where} AND i.place IS NOT NULL AND i.place <> ''
             GROUP BY i.place")?
        {
            let place = text(&r[0]);
            if let Some(c) = place
                .rsplit(',')
                .next()
                .map(str::trim)
                .filter(|c| !c.is_empty())
            {
                *countries.entry(c.to_string()).or_default() += int(&r[1]);
            }
        }
        let mut countries: Vec<LabelCount> = countries
            .into_iter()
            .map(|(label, count)| LabelCount { label, count })
            .collect();
        countries.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));
        countries.truncate(TOP as usize);
        s.countries = countries;

        Ok(s)
    }

    /// Dateitypen der Fotos von `q`, ohne dessen Dateityp-Filter – für die
    /// Auswahl im Filter, die alle vorhandenen Typen zeigen soll.
    pub fn file_types(&self, q: &ImageQuery) -> Result<Vec<FileTypeCount>> {
        let mut q = q.clone();
        q.exts = None;
        q.ids = None;
        let (where_sql, args) = self.where_clause(&q)?;
        let mut st = self.conn.prepare(&format!(
            "SELECT i.ext, MAX(i.kind), COUNT(*) FROM images i
             WHERE {where_sql} GROUP BY i.ext ORDER BY 3 DESC, 1"
        ))?;
        let rows = st.query_map(params_from_iter(args), |r| {
            Ok(FileTypeCount {
                ext: r.get(0)?,
                kind: r.get(1)?,
                count: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::tests_support::insert_dummy_images;
    use crate::model::{Scope, TagSource};

    #[test]
    fn counts_and_file_type_filter() {
        let mut cat = Catalog::open_in_memory().unwrap();
        let ids = insert_dummy_images(&mut cat, 4);
        let set = |id: i64, sql: &str| {
            cat.conn
                .execute(&format!("UPDATE images SET {sql} WHERE id = ?1"), [id])
                .unwrap();
        };
        set(
            ids[0],
            "ext = 'arw', kind = 'raw', camera = 'ILCE-7M3', lens = 'FE 24-70mm', \
             focal_length = 35.4, aperture = 2.8, iso = 400, exposure = '1/250', \
             captured_at = '2025-06-01T18:30:00', rating = 4, \
             place = 'Ubud, Bali, Indonesien', gps_lat = 1, gps_lon = 1",
        );
        set(
            ids[1],
            "ext = 'arw', kind = 'raw', camera = 'ILCE-7M3', focal_length = 35, \
             aperture = 2.8, captured_at = '2025-06-02T07:05:00', rating = 4",
        );
        set(
            ids[2],
            "ext = 'jpeg', camera = ' Pixel 8 ', captured_at = '2025-07-01T18:00:00'",
        );
        // ids[3]: JPG ohne Aufnahmedaten
        cat.add_tag(ids[0], "Strand", TagSource::Manual, None)
            .unwrap();
        cat.add_tag(ids[1], "Strand", TagSource::Ai, Some(0.8))
            .unwrap();

        let all = ImageQuery::new(Scope::All);
        let s = cat.photo_stats(&all).unwrap();
        assert_eq!((s.total, s.raw, s.with_location), (4, 2, 1));
        assert_eq!(s.ratings, [2, 0, 0, 0, 2, 0]);
        assert_eq!(
            s.cameras,
            [
                LabelCount {
                    label: "ILCE-7M3".into(),
                    count: 2
                },
                LabelCount {
                    label: "Pixel 8".into(),
                    count: 1
                },
            ]
        );
        assert_eq!(
            s.focal_lengths,
            [ValueCount {
                value: 35.0,
                count: 2
            }]
        );
        assert_eq!(
            s.apertures,
            [ValueCount {
                value: 2.8,
                count: 2
            }]
        );
        assert_eq!(s.months.len(), 2);
        assert_eq!((s.hours[18], s.hours[7]), (2, 1));
        assert_eq!(
            s.keywords,
            [LabelCount {
                label: "Strand".into(),
                count: 2
            }]
        );
        assert_eq!(
            s.countries,
            [LabelCount {
                label: "Indonesien".into(),
                count: 1
            }]
        );
        assert_eq!(s.first.as_deref(), Some("2025-06-01T18:30:00"));

        // Dateityp-Filter: in der Liste, in der Statistik, in der Typenauswahl
        let mut raw = all.clone();
        raw.exts = Some(vec!["ARW".into()]);
        assert_eq!(cat.query_images(&raw).unwrap().len(), 2);
        assert_eq!(cat.photo_stats(&raw).unwrap().total, 2);
        let mut jpeg = all.clone();
        jpeg.exts = Some(crate::formats::expand_file_types(&["jpeg"]));
        assert_eq!(cat.query_images(&jpeg).unwrap().len(), 2, "jpg und jpeg");
        let types = cat.file_types(&raw).unwrap();
        assert_eq!(
            types.iter().map(|t| t.count).sum::<i64>(),
            4,
            "ohne eigenen Filter"
        );
        assert_eq!(
            (types[0].ext.as_str(), types[0].kind.as_str()),
            ("arw", "raw")
        );
    }

    #[test]
    fn empty_view() {
        let cat = Catalog::open_in_memory().unwrap();
        let s = cat.photo_stats(&ImageQuery::new(Scope::All)).unwrap();
        assert_eq!(s, PhotoStats::default());
    }
}
