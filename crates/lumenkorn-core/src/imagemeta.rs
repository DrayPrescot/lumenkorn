//! Vom Nutzer bearbeitete Metadaten eines Fotos (Migration v24).
//!
//! Titel, Beschreibung, Autor und Urheberrecht gibt es nur hier. Aufnahmezeit,
//! Kamera, Objektiv, Brennweite, Blende, Belichtungszeit und ISO stehen
//! zusätzlich in `images` (Suche, Sortierung, Statistik, Filter nutzen sie
//! von dort). Dorthin wird eine Überschreibung ebenfalls geschrieben; der Wert
//! aus der Datei bleibt in `image_meta.f_*`, damit das Zurücksetzen ihn
//! wiederherstellt. Liest der Scanner die Datei neu, bleibt die
//! Überschreibung bestehen (`reapply`).
//!
//! Die Bilddateien werden nie verändert; für andere Programme gibt es eine
//! XMP-Begleitdatei (`xmp.rs`), siehe [`MetaPatch::xmp_changes`].

use crate::catalog::Catalog;
use crate::error::{Error, Result};
use crate::geotime::parse_time;
use crate::timeshift::format_time;
use crate::xmp::{self, Content, XmpChange};
use lumenkorn_develop::tr;
use rusqlite::{Connection, OptionalExtension, ToSql, params};
use serde::{Deserialize, Serialize};

/// Änderung eines Feldes
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Edit<T> {
    /// Nicht anfassen
    #[default]
    Keep,
    /// Auf diesen Wert setzen
    Set(T),
    /// Überschreibung aufheben (Wert aus der Datei)
    Reset,
}

impl<T> Edit<T> {
    fn is_keep(&self) -> bool {
        matches!(self, Edit::Keep)
    }
}

/// Änderungen an den Metadaten eines Fotos
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MetaPatch {
    pub title: Edit<String>,
    pub description: Edit<String>,
    pub creator: Edit<String>,
    pub copyright: Edit<String>,
    /// `JJJJ-MM-TTThh:mm:ss`
    pub captured_at: Edit<String>,
    pub camera: Edit<String>,
    pub lens: Edit<String>,
    pub iso: Edit<i64>,
    /// „1/250“, „2“ oder „0.5“ (Sekunden)
    pub exposure: Edit<String>,
    pub aperture: Edit<f64>,
    pub focal_length: Edit<f64>,
}

fn invalid(what: &str, v: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("{what}: {v}"))
}

/// Text bereinigen: leer heißt „Überschreibung aufheben“
fn text(e: &Edit<String>) -> Edit<String> {
    match e {
        Edit::Set(s) if s.trim().is_empty() => Edit::Reset,
        Edit::Set(s) => Edit::Set(s.trim().to_string()),
        other => other.clone(),
    }
}

/// Belichtungszeit aus Eingabe wie „1/250“, „2“, „0,5“ oder „2 s“ in Sekunden
pub fn parse_exposure(s: &str) -> Option<f64> {
    let t = s
        .trim()
        .trim_end_matches(['s', 'S'])
        .trim()
        .replace(',', ".");
    let secs = match t.split_once('/') {
        Some((a, b)) => {
            let (a, b): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
            if b == 0.0 {
                return None;
            }
            a / b
        }
        None => t.parse().ok()?,
    };
    (secs.is_finite() && secs > 0.0 && secs <= 100_000.0).then_some(secs)
}

/// Belichtungszeit so geschrieben, wie der Katalog sie aus EXIF ablegt
pub fn format_exposure(secs: f64) -> String {
    if secs < 1.0 {
        format!("1/{}", (1.0 / secs).round().max(1.0) as i64)
    } else {
        format!("{secs}s")
    }
}

/// Zahl als Bruch („2.8“ → „28/10“) für XMP
pub fn rational(v: f64) -> String {
    for d in [1i64, 10, 100, 1000, 10_000] {
        let n = v * d as f64;
        if (n - n.round()).abs() < 1e-6 {
            return format!("{}/{d}", n.round() as i64);
        }
    }
    format!("{}/10000", (v * 10_000.0).round() as i64)
}

impl MetaPatch {
    /// Keine Änderung enthalten?
    pub fn is_empty(&self) -> bool {
        self.title.is_keep()
            && self.description.is_keep()
            && self.creator.is_keep()
            && self.copyright.is_keep()
            && self.captured_at.is_keep()
            && self.camera.is_keep()
            && self.lens.is_keep()
            && self.iso.is_keep()
            && self.exposure.is_keep()
            && self.aperture.is_keep()
            && self.focal_length.is_keep()
    }

    /// Prüft die Werte und bringt sie in die Form des Katalogs (Zeit
    /// `JJJJ-MM-TTThh:mm:ss`, Belichtungszeit wie aus EXIF, leere Texte als
    /// Zurücksetzen).
    pub fn normalized(&self) -> Result<MetaPatch> {
        let captured_at = match &self.captured_at {
            Edit::Set(s) => {
                let secs = parse_time(s).ok_or_else(|| invalid("captured_at", s))?;
                Edit::Set(format_time(secs))
            }
            other => other.clone(),
        };
        let iso = match self.iso {
            Edit::Set(v) if !(1..=100_000_000).contains(&v) => return Err(invalid("ISO", v)),
            ref other => other.clone(),
        };
        let exposure = match &self.exposure {
            Edit::Set(s) if s.trim().is_empty() => Edit::Reset,
            Edit::Set(s) => Edit::Set(format_exposure(
                parse_exposure(s).ok_or_else(|| invalid("exposure", s))?,
            )),
            other => other.clone(),
        };
        let positive = |e: &Edit<f64>, what: &str, max: f64| -> Result<Edit<f64>> {
            match e {
                Edit::Set(v) if !(v.is_finite() && *v > 0.0 && *v <= max) => Err(invalid(what, v)),
                other => Ok(other.clone()),
            }
        };
        Ok(MetaPatch {
            title: text(&self.title),
            description: text(&self.description),
            creator: text(&self.creator),
            copyright: text(&self.copyright),
            captured_at,
            camera: text(&self.camera),
            lens: text(&self.lens),
            iso,
            exposure,
            aperture: positive(&self.aperture, "aperture", 1000.0)?,
            focal_length: positive(&self.focal_length, "focal_length", 100_000.0)?,
        })
    }

    /// Entsprechende Änderungen an der XMP-Begleitdatei (Patch muss
    /// [`normalized`](Self::normalized) sein). Nur geänderte Felder.
    pub fn xmp_changes(&self) -> Vec<XmpChange> {
        let mut out = Vec::new();
        fn push<T>(
            out: &mut Vec<XmpChange>,
            e: &Edit<T>,
            names: &[xmp::PropName],
            make: impl Fn(&T) -> Content,
        ) {
            let value = match e {
                Edit::Keep => return,
                Edit::Set(v) => Some(make(v)),
                Edit::Reset => None,
            };
            for &name in names {
                out.push(XmpChange {
                    name,
                    value: value.clone(),
                });
            }
        }
        push(&mut out, &self.title, &[xmp::DC_TITLE], |v| {
            Content::Alt(v.clone())
        });
        push(&mut out, &self.description, &[xmp::DC_DESCRIPTION], |v| {
            Content::Alt(v.clone())
        });
        push(&mut out, &self.creator, &[xmp::DC_CREATOR], |v| {
            Content::Seq(v.clone())
        });
        push(&mut out, &self.copyright, &[xmp::DC_RIGHTS], |v| {
            Content::Alt(v.clone())
        });
        push(
            &mut out,
            &self.captured_at,
            &[
                xmp::EXIF_DATE_ORIGINAL,
                xmp::XMP_CREATE_DATE,
                xmp::PS_DATE_CREATED,
            ],
            |v| Content::Plain(v.clone()),
        );
        // `tiff:Make` bleibt, wie es ist: Der Katalog kennt Marke und Modell
        // nur als einen Text, der sich nicht verlässlich trennen lässt
        push(&mut out, &self.camera, &[xmp::TIFF_MODEL], |v| {
            Content::Plain(v.clone())
        });
        push(
            &mut out,
            &self.lens,
            &[xmp::EXIF_EX_LENS, xmp::AUX_LENS],
            |v| Content::Plain(v.clone()),
        );
        push(
            &mut out,
            &self.focal_length,
            &[xmp::EXIF_FOCAL_LENGTH],
            |v| Content::Plain(rational(*v)),
        );
        push(&mut out, &self.aperture, &[xmp::EXIF_F_NUMBER], |v| {
            Content::Plain(rational(*v))
        });
        push(&mut out, &self.exposure, &[xmp::EXIF_EXPOSURE_TIME], |v| {
            Content::Plain(match v.strip_prefix("1/") {
                Some(d) if d.parse::<i64>().is_ok() => v.clone(),
                _ => rational(parse_exposure(v).unwrap_or(0.0)),
            })
        });
        push(&mut out, &self.iso, &[xmp::EXIF_ISO], |v| {
            Content::Seq(v.to_string())
        });
        out
    }
}

/// Ein Feld: gültiger Wert, Wert aus der Datei und ob überschrieben
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Field<T> {
    /// Gültiger Wert (Überschreibung, sonst Datei)
    pub value: Option<T>,
    /// Wert in der Datei (bei reinen Katalogfeldern wie dem Titel leer)
    pub file: Option<T>,
    pub overridden: bool,
}

/// Bearbeitbare Metadaten eines Fotos
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageMeta {
    pub id: i64,
    pub title: Field<String>,
    pub description: Field<String>,
    pub creator: Field<String>,
    pub copyright: Field<String>,
    pub captured_at: Field<String>,
    pub camera: Field<String>,
    pub lens: Field<String>,
    pub iso: Field<i64>,
    pub exposure: Field<String>,
    pub aperture: Field<f64>,
    pub focal_length: Field<f64>,
}

/// Foto, dessen Metadaten geändert werden sollen
#[derive(Debug, Clone)]
pub struct MetaTarget {
    pub id: i64,
    pub path: String,
    /// Gültige Aufnahmezeit
    pub captured_at: Option<String>,
}

/// Überschreibungen nach einem erneuten Einlesen der Datei wieder anwenden:
/// `images` trägt dann die Werte der Datei; diese werden nach `f_*`
/// übernommen, dann gewinnt die Überschreibung.
pub(crate) fn reapply(conn: &Connection, id: i64) -> Result<()> {
    let n = conn.execute(
        "UPDATE image_meta SET
            f_captured_at = (SELECT captured_at FROM images WHERE id = image_id),
            f_camera = (SELECT camera FROM images WHERE id = image_id),
            f_lens = (SELECT lens FROM images WHERE id = image_id),
            f_iso = (SELECT iso FROM images WHERE id = image_id),
            f_exposure = (SELECT exposure FROM images WHERE id = image_id),
            f_aperture = (SELECT aperture FROM images WHERE id = image_id),
            f_focal_length = (SELECT focal_length FROM images WHERE id = image_id)
         WHERE image_id = ?1",
        [id],
    )?;
    if n > 0 {
        conn.execute(
            "UPDATE images SET
                captured_at = COALESCE((SELECT captured_at FROM image_meta WHERE image_id = ?1), captured_at),
                camera = COALESCE((SELECT camera FROM image_meta WHERE image_id = ?1), camera),
                lens = COALESCE((SELECT lens FROM image_meta WHERE image_id = ?1), lens),
                iso = COALESCE((SELECT iso FROM image_meta WHERE image_id = ?1), iso),
                exposure = COALESCE((SELECT exposure FROM image_meta WHERE image_id = ?1), exposure),
                aperture = COALESCE((SELECT aperture FROM image_meta WHERE image_id = ?1), aperture),
                focal_length = COALESCE((SELECT focal_length FROM image_meta WHERE image_id = ?1), focal_length)
             WHERE id = ?1",
            [id],
        )?;
    }
    Ok(())
}

/// Spalte in `image_meta` (und, wenn `backed`, in `images`) ändern
fn apply_field<T: ToSql>(
    conn: &Connection,
    id: i64,
    col: &str,
    backed: bool,
    edit: &Edit<T>,
) -> Result<()> {
    match edit {
        Edit::Keep => {}
        Edit::Set(v) => {
            conn.execute(
                &format!("UPDATE image_meta SET {col} = ?1 WHERE image_id = ?2"),
                params![v, id],
            )?;
            if backed {
                conn.execute(
                    &format!("UPDATE images SET {col} = ?1 WHERE id = ?2"),
                    params![v, id],
                )?;
            }
        }
        Edit::Reset => {
            if backed {
                conn.execute(
                    &format!(
                        "UPDATE images SET {col} = (SELECT f_{col} FROM image_meta WHERE image_id = ?1)
                         WHERE id = ?1 AND EXISTS (SELECT 1 FROM image_meta WHERE image_id = ?1)"
                    ),
                    [id],
                )?;
            }
            conn.execute(
                &format!("UPDATE image_meta SET {col} = NULL WHERE image_id = ?1"),
                [id],
            )?;
        }
    }
    Ok(())
}

impl Catalog {
    /// Metadaten eines Fotos mit Wert aus der Datei und Überschreibung
    pub fn image_meta(&self, id: i64) -> Result<ImageMeta> {
        self.conn
            .query_row(
                "SELECT i.captured_at, i.camera, i.lens, i.iso, i.exposure, i.aperture, i.focal_length,
                        m.title, m.description, m.creator, m.copyright,
                        m.captured_at, m.camera, m.lens, m.iso, m.exposure, m.aperture, m.focal_length,
                        m.f_captured_at, m.f_camera, m.f_lens, m.f_iso, m.f_exposure, m.f_aperture,
                        m.f_focal_length
                 FROM images i LEFT JOIN image_meta m ON m.image_id = i.id
                 WHERE i.id = ?1",
                [id],
                |r| {
                    fn field<T: rusqlite::types::FromSql>(
                        r: &rusqlite::Row,
                        cur: usize,
                        over: usize,
                        file: usize,
                    ) -> rusqlite::Result<Field<T>> {
                        let value: Option<T> = r.get(cur)?;
                        let o: Option<T> = r.get(over)?;
                        let overridden = o.is_some();
                        Ok(Field {
                            file: if overridden { r.get(file)? } else { None },
                            value,
                            overridden,
                        })
                    }
                    // Wert aus der Datei: ohne Überschreibung der aktuelle
                    fn backed<T: rusqlite::types::FromSql + Clone>(
                        r: &rusqlite::Row,
                        cur: usize,
                        over: usize,
                        file: usize,
                    ) -> rusqlite::Result<Field<T>> {
                        let mut f = field::<T>(r, cur, over, file)?;
                        if !f.overridden {
                            f.file = f.value.clone();
                        }
                        Ok(f)
                    }
                    fn only<T: rusqlite::types::FromSql>(
                        r: &rusqlite::Row,
                        col: usize,
                    ) -> rusqlite::Result<Field<T>> {
                        let v: Option<T> = r.get(col)?;
                        Ok(Field {
                            overridden: v.is_some(),
                            value: v,
                            file: None,
                        })
                    }
                    Ok(ImageMeta {
                        id,
                        title: only(r, 7)?,
                        description: only(r, 8)?,
                        creator: only(r, 9)?,
                        copyright: only(r, 10)?,
                        captured_at: backed(r, 0, 11, 18)?,
                        camera: backed(r, 1, 12, 19)?,
                        lens: backed(r, 2, 13, 20)?,
                        iso: backed(r, 3, 14, 21)?,
                        exposure: backed(r, 4, 15, 22)?,
                        aperture: backed(r, 5, 16, 23)?,
                        focal_length: backed(r, 6, 17, 24)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(tr!("Bild {id}", "Photo {id}")))
    }

    /// Pfad und gültige Aufnahmezeit der Fotos (in der Reihenfolge von
    /// `ids`; unbekannte fehlen)
    pub fn meta_targets(&self, ids: &[i64]) -> Result<Vec<MetaTarget>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT path, captured_at FROM images WHERE id = ?1")?;
        let mut out = Vec::with_capacity(ids.len());
        for &id in ids {
            if let Some((path, captured_at)) = st
                .query_row([id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?
            {
                out.push(MetaTarget {
                    id,
                    path,
                    captured_at,
                });
            }
        }
        Ok(out)
    }

    /// Änderungen für mehrere Fotos in einer Transaktion speichern. Die
    /// Patches müssen [`MetaPatch::normalized`] sein.
    pub fn apply_meta(&mut self, items: &[(i64, MetaPatch)]) -> Result<()> {
        self.with_tx(|tx| {
            for (id, patch) in items {
                if patch.is_empty() {
                    continue;
                }
                tx.execute(
                    "INSERT OR IGNORE INTO image_meta
                        (image_id, f_captured_at, f_camera, f_lens, f_iso, f_exposure,
                         f_aperture, f_focal_length)
                     SELECT id, captured_at, camera, lens, iso, exposure, aperture, focal_length
                     FROM images WHERE id = ?1",
                    [id],
                )?;
                apply_field(tx, *id, "title", false, &patch.title)?;
                apply_field(tx, *id, "description", false, &patch.description)?;
                apply_field(tx, *id, "creator", false, &patch.creator)?;
                apply_field(tx, *id, "copyright", false, &patch.copyright)?;
                apply_field(tx, *id, "captured_at", true, &patch.captured_at)?;
                apply_field(tx, *id, "camera", true, &patch.camera)?;
                apply_field(tx, *id, "lens", true, &patch.lens)?;
                apply_field(tx, *id, "iso", true, &patch.iso)?;
                apply_field(tx, *id, "exposure", true, &patch.exposure)?;
                apply_field(tx, *id, "aperture", true, &patch.aperture)?;
                apply_field(tx, *id, "focal_length", true, &patch.focal_length)?;
                // Ohne Überschreibung bleibt kein Eintrag
                tx.execute(
                    "DELETE FROM image_meta WHERE image_id = ?1
                       AND title IS NULL AND description IS NULL AND creator IS NULL
                       AND copyright IS NULL AND captured_at IS NULL AND camera IS NULL
                       AND lens IS NULL AND iso IS NULL AND exposure IS NULL
                       AND aperture IS NULL AND focal_length IS NULL",
                    [id],
                )?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{NewImage, tests_support};
    use crate::formats::ImageKind;
    use crate::metadata::Metadata;
    use crate::model::{ImageQuery, Scope, SortOrder};

    fn set(s: &str) -> Edit<String> {
        Edit::Set(s.into())
    }

    fn search(cat: &Catalog, text: &str) -> usize {
        let q = ImageQuery {
            scope: Scope::All,
            sort: SortOrder::default(),
            search: Some(text.into()),
            min_rating: None,
            exts: None,
            edited: None,
            limit: None,
            offset: None,
            ids: None,
        };
        cat.query_images(&q).unwrap().len()
    }

    #[test]
    fn exposure_parsing_and_format() {
        assert_eq!(parse_exposure("1/250"), Some(1.0 / 250.0));
        assert_eq!(parse_exposure("2"), Some(2.0));
        assert_eq!(parse_exposure(" 0,5 s"), Some(0.5));
        assert_eq!(parse_exposure("1/0"), None);
        assert_eq!(parse_exposure("abc"), None);
        assert_eq!(parse_exposure("-1"), None);
        assert_eq!(format_exposure(1.0 / 250.0), "1/250");
        assert_eq!(format_exposure(2.0), "2s");
        assert_eq!(rational(2.8), "28/10");
        assert_eq!(rational(50.0), "50/1");
    }

    #[test]
    fn patch_validation() {
        let p = MetaPatch {
            exposure: set("1/250"),
            captured_at: set("2024-06-01 14:03:22"),
            title: set("   "),
            ..Default::default()
        };
        let n = p.normalized().unwrap();
        assert_eq!(n.exposure, set("1/250"));
        assert_eq!(n.captured_at, set("2024-06-01T14:03:22"));
        assert_eq!(n.title, Edit::Reset);
        for bad in [
            MetaPatch {
                iso: Edit::Set(0),
                ..Default::default()
            },
            MetaPatch {
                aperture: Edit::Set(-1.0),
                ..Default::default()
            },
            MetaPatch {
                focal_length: Edit::Set(f64::NAN),
                ..Default::default()
            },
            MetaPatch {
                exposure: set("schnell"),
                ..Default::default()
            },
            MetaPatch {
                captured_at: set("gestern"),
                ..Default::default()
            },
        ] {
            assert!(bad.normalized().is_err());
        }
        // JSON aus dem Frontend
        let j: MetaPatch =
            serde_json::from_str(r#"{"title":{"set":"X"},"iso":"reset","lens":"keep"}"#).unwrap();
        assert_eq!(j.title, set("X"));
        assert_eq!(j.iso, Edit::Reset);
        assert_eq!(j.camera, Edit::Keep);
    }

    #[test]
    fn xmp_changes_for_fields() {
        let p = MetaPatch {
            aperture: Edit::Set(2.8),
            exposure: set("1/250"),
            iso: Edit::Reset,
            lens: set("FE 50"),
            ..Default::default()
        };
        let c = p.xmp_changes();
        let find = |n: xmp::PropName| c.iter().find(|x| x.name == n).unwrap().value.clone();
        assert_eq!(
            find(xmp::EXIF_F_NUMBER),
            Some(Content::Plain("28/10".into()))
        );
        assert_eq!(
            find(xmp::EXIF_EXPOSURE_TIME),
            Some(Content::Plain("1/250".into()))
        );
        assert_eq!(find(xmp::EXIF_ISO), None);
        assert_eq!(find(xmp::AUX_LENS), Some(Content::Plain("FE 50".into())));
        assert!(!c.iter().any(|x| x.name == xmp::DC_TITLE));
    }

    #[test]
    fn override_reset_rescan_and_search() {
        let mut cat = Catalog::open_in_memory().unwrap();
        let ids = tests_support::insert_dummy_images(&mut cat, 2);
        let id = ids[0];
        cat.conn
            .execute(
                "UPDATE images SET camera = 'Canon R5', captured_at = '2020-01-01T10:00:00',
                        iso = 100, aperture = 4.0 WHERE id = ?1",
                [id],
            )
            .unwrap();

        let patch = MetaPatch {
            title: set("Sonnenuntergang am Meer"),
            creator: set("Jörg"),
            camera: set("Leica Q3"),
            iso: Edit::Set(800),
            captured_at: set("2024-06-01T14:03:22"),
            ..Default::default()
        };
        cat.apply_meta(&[(id, patch.normalized().unwrap())])
            .unwrap();

        let m = cat.image_meta(id).unwrap();
        assert_eq!(m.title.value.as_deref(), Some("Sonnenuntergang am Meer"));
        assert!(m.title.overridden);
        assert_eq!(m.camera.value.as_deref(), Some("Leica Q3"));
        assert_eq!(m.camera.file.as_deref(), Some("Canon R5"));
        assert!(m.camera.overridden);
        assert_eq!(m.iso.value, Some(800));
        assert_eq!(m.iso.file, Some(100));
        assert!(!m.aperture.overridden);
        assert_eq!(m.aperture.file, Some(4.0));
        let detail = cat.image(id).unwrap();
        assert_eq!(detail.camera.as_deref(), Some("Leica Q3"));
        assert_eq!(detail.captured_at.as_deref(), Some("2024-06-01T14:03:22"));

        // Suche: Titel, Autor, neue Kamera; die alte nicht mehr
        assert_eq!(search(&cat, "Sonnenuntergang"), 1);
        assert_eq!(search(&cat, "Jörg"), 1);
        assert_eq!(search(&cat, "Leica"), 1);
        assert_eq!(search(&cat, "Canon"), 0);
        tests_support::exact(&cat, "nach Bearbeitung");

        // Datei wird neu eingelesen: Überschreibung bleibt, Dateiwert neu
        let root_id: i64 = cat
            .conn
            .query_row("SELECT root_id FROM images WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        let img = NewImage {
            root_id,
            path: "/tmp/lumenkorn-test/bild0.jpg".into(),
            dir: "/tmp/lumenkorn-test".into(),
            file_name: "bild0.jpg".into(),
            ext: "jpg".into(),
            kind: ImageKind::Raster,
            size: 2,
            mtime: 5,
            quick_hash: "neu".into(),
            meta: Metadata {
                camera: Some("Sony A7".into()),
                iso: Some(200),
                captured_at: Some("2021-02-02T02:02:02".into()),
                ..Default::default()
            },
        };
        cat.with_tx(|tx| Catalog::update_image_content(tx, id, &img))
            .unwrap();
        let m = cat.image_meta(id).unwrap();
        assert_eq!(m.camera.value.as_deref(), Some("Leica Q3"));
        assert_eq!(m.camera.file.as_deref(), Some("Sony A7"));
        assert_eq!(m.iso.value, Some(800));
        // nicht überschriebenes Feld folgt der Datei
        assert_eq!(m.aperture.value, None);
        tests_support::exact(&cat, "nach Neueinlesen");

        // Zurücksetzen: Wert der Datei kommt zurück, Eintrag verschwindet
        let reset = MetaPatch {
            title: Edit::Reset,
            creator: Edit::Reset,
            camera: Edit::Reset,
            iso: Edit::Reset,
            captured_at: Edit::Reset,
            ..Default::default()
        };
        cat.apply_meta(&[(id, reset)]).unwrap();
        let m = cat.image_meta(id).unwrap();
        assert_eq!(m.camera.value.as_deref(), Some("Sony A7"));
        assert_eq!(m.iso.value, Some(200));
        assert_eq!(m.captured_at.value.as_deref(), Some("2021-02-02T02:02:02"));
        assert!(!m.camera.overridden && m.title.value.is_none());
        let rows: i64 = cat
            .conn
            .query_row("SELECT COUNT(*) FROM image_meta", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
        assert_eq!(search(&cat, "Sonnenuntergang"), 0);
        assert_eq!(search(&cat, "Sony"), 1);
        tests_support::exact(&cat, "nach Zurücksetzen");

        // Verbundenes Bild übernimmt Aufnahmedaten, Überschreibungen gewinnen
        cat.apply_meta(&[(
            ids[1],
            MetaPatch {
                lens: set("Mein Objektiv"),
                ..Default::default()
            },
        )])
        .unwrap();
        let p1: String = cat
            .conn
            .query_row("SELECT path FROM images WHERE id = ?1", [ids[1]], |r| {
                r.get(0)
            })
            .unwrap();
        cat.adopt_capture_info(&p1, id).unwrap();
        let m = cat.image_meta(ids[1]).unwrap();
        assert_eq!(m.lens.value.as_deref(), Some("Mein Objektiv"));
        assert_eq!(m.camera.value.as_deref(), Some("Sony A7"));
    }

    #[test]
    fn deleting_photo_removes_meta() {
        let mut cat = Catalog::open_in_memory().unwrap();
        let ids = tests_support::insert_dummy_images(&mut cat, 1);
        cat.apply_meta(&[(
            ids[0],
            MetaPatch {
                title: set("T"),
                ..Default::default()
            },
        )])
        .unwrap();
        cat.conn.execute("DELETE FROM images", []).unwrap();
        let rows: i64 = cat
            .conn
            .query_row("SELECT COUNT(*) FROM image_meta", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }
}
