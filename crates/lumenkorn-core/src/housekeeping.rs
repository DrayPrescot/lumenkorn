//! Aufräumen des Datenordners: verwaiste Dateien finden und löschen.
//!
//! Berechnete Zwischenstufen (Entrauschungen, KI-Masken, Reparaturen) liegen
//! als Dateien vor, die Bearbeitung verweist nur per ID darauf. Wird eine
//! Entrauschung neu berechnet, eine Maske gelöscht oder ein Quellordner
//! entfernt, bleibt die alte Datei liegen – eine Entrauschung eines 60-MP-Fotos
//! sind ~360 MB. Diese Routine sammelt alle noch referenzierten IDs (aktuelle
//! Bearbeitungen *und* Schnappschüsse) und löscht den Rest.
//!
//! Sicherheitsabstand: Nur Dateien, die älter als `min_age` sind. Eine eben
//! berechnete Maske kann noch auf das (verzögerte) Speichern warten oder im
//! Rückgängig-Verlauf stecken.

use crate::catalog::Catalog;
use crate::error::Result;
use lumenkorn_develop::DevelopParams;
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Was im Katalog noch lebt.
#[derive(Debug, Default)]
pub struct Live {
    pub images: HashSet<i64>,
    pub faces: HashSet<i64>,
    /// IDs von Entrauschungen, Masken und Reparaturen
    pub files: HashSet<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Swept {
    pub files: usize,
    pub bytes: u64,
}

impl std::ops::AddAssign for Swept {
    fn add_assign(&mut self, o: Self) {
        self.files += o.files;
        self.bytes += o.bytes;
    }
}

/// Alle Datei-IDs, auf die eine Bearbeitung verweist.
pub fn referenced(p: &DevelopParams, out: &mut HashSet<String>) {
    if let Some(d) = &p.denoise {
        out.insert(d.id.clone());
    }
    for r in &p.repairs {
        if let Some(id) = &r.result {
            out.insert(id.clone());
        }
    }
    out.extend(p.raster_mask_ids());
}

impl Catalog {
    pub fn live(&self) -> Result<Live> {
        let mut live = Live::default();
        let mut st = self.conn.prepare("SELECT id FROM images")?;
        for r in st.query_map([], |r| r.get(0))? {
            live.images.insert(r?);
        }
        let mut st = self.conn.prepare("SELECT id FROM faces")?;
        for r in st.query_map([], |r| r.get(0))? {
            live.faces.insert(r?);
        }
        // Auch Schnappschüsse: Wiederherstellen braucht ihre Dateien
        let mut st = self.conn.prepare("SELECT params FROM edits")?;
        for r in st.query_map([], |r| r.get::<_, String>(0))? {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&r?) {
                referenced(&DevelopParams::from_json(&v), &mut live.files);
            }
        }
        Ok(live)
    }

    /// Entfernt Verweise auf Dateien anderer Fotos aus allen Bearbeitungen.
    ///
    /// Ältere Versionen haben beim Einfügen von Einstellungen auch die
    /// Entrauschung, Reparaturen und KI-Masken des Ursprungsfotos übertragen –
    /// das Zielfoto zeigte dann fremde Pixel. Alle Datei-IDs beginnen mit der
    /// ID ihres Fotos (`42-dn-…`), daran ist ein fremder Verweis erkennbar.
    /// Liefert die Anzahl bereinigter Bearbeitungen.
    pub fn drop_foreign_refs(&mut self) -> Result<usize> {
        let rows: Vec<(i64, i64, String)> = {
            let mut st = self
                .conn
                .prepare("SELECT id, image_id, params FROM edits")?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        let mut fixed = Vec::new();
        for (edit_id, image_id, json) in rows {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) else {
                continue;
            };
            let mut p = DevelopParams::from_json(&v);
            let own = format!("{image_id}-");
            let foreign = |id: &str| !id.starts_with(&own);
            let foreign_shape = |s: &lumenkorn_develop::MaskShape| matches!(s, lumenkorn_develop::MaskShape::Raster { mask_id, .. } if foreign(mask_id));
            let before = (p.denoise.is_some(), p.masks.len(), p.repairs.len());
            if p.denoise.as_ref().is_some_and(|d| foreign(&d.id)) {
                p.denoise = None;
            }
            p.masks.retain(|m| !m.shapes.iter().any(foreign_shape));
            p.repairs.retain(|r| {
                !r.result.as_deref().is_some_and(foreign) && !r.shapes.iter().any(foreign_shape)
            });
            if before != (p.denoise.is_some(), p.masks.len(), p.repairs.len()) {
                fixed.push((edit_id, image_id, serde_json::to_string(&p)?));
            }
        }
        if fixed.is_empty() {
            return Ok(0);
        }
        self.with_tx(|tx| {
            for (edit_id, image_id, json) in &fixed {
                tx.execute(
                    "UPDATE edits SET params = ?2 WHERE id = ?1",
                    rusqlite::params![edit_id, json],
                )?;
                tx.execute(
                    "UPDATE images SET thumb_state = 0 WHERE id = ?1",
                    [image_id],
                )?;
            }
            Ok(())
        })?;
        Ok(fixed.len())
    }

    /// RAWs, für die beim Einlesen nur die Maße des Mini-Thumbnails
    /// gespeichert wurden (ältere Versionen, z. B. NEF 160×120).
    fn raw_dims_candidates(&self) -> Result<Vec<(i64, String, Option<i64>)>> {
        let mut st = self.conn.prepare(
            "SELECT id, path, orientation FROM images
             WHERE kind = 'raw' AND missing = 0
               AND MAX(COALESCE(width, 0), COALESCE(height, 0)) < 1000",
        )?;
        Ok(st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?)
    }

    /// Statistiken für den Abfrageplaner aktualisieren (günstig, empfohlen
    /// von SQLite für lange laufende Anwendungen).
    pub fn optimize(&self) -> Result<()> {
        // Suchindex: nach vielen kleinen Änderungen die Segmente zusammenführen
        self.conn
            .execute_batch("INSERT INTO search(search) VALUES ('optimize'); PRAGMA optimize;")?;
        Ok(())
    }
}

/// Korrigiert die Abmessungen solcher RAWs (siehe `raw_dims_candidates`).
/// Die Dateien werden ohne Katalogsperre gelesen – bei Tausenden RAWs auf
/// einem NAS dauert das, und die Oberfläche soll derweil bedienbar bleiben.
/// Dateien ohne lesbare Maße (z. B. CR3) werden je Programmstart nur einmal
/// versucht. Liefert die Anzahl korrigierter Fotos.
pub fn fix_raw_dims(catalog: &std::sync::Mutex<Catalog>) -> Result<usize> {
    static TRIED: std::sync::Mutex<Option<std::collections::HashSet<i64>>> =
        std::sync::Mutex::new(None);
    let rows = catalog.lock().unwrap().raw_dims_candidates()?;
    let mut tried = TRIED.lock().unwrap_or_else(|e| e.into_inner());
    let tried = tried.get_or_insert_with(Default::default);
    let mut found = Vec::new();
    for (id, path, orientation) in rows {
        if !tried.insert(id) {
            continue;
        }
        let Some((w, h)) = crate::metadata::tiff_raw_dims(Path::new(&path)) else {
            continue;
        };
        found.push(if matches!(orientation, Some(5..=8)) {
            (id, h, w)
        } else {
            (id, w, h)
        });
    }
    if found.is_empty() {
        return Ok(0);
    }
    let mut cat = catalog.lock().unwrap();
    cat.with_tx(|tx| {
        for (id, w, h) in &found {
            tx.execute(
                "UPDATE images SET width = ?2, height = ?3 WHERE id = ?1",
                rusqlite::params![id, w, h],
            )?;
        }
        Ok(())
    })?;
    Ok(found.len())
}

fn old_enough(meta: &std::fs::Metadata, min_age: Duration) -> bool {
    meta.modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .is_some_and(|age| age >= min_age)
}

/// Löscht in `dir` alle Dateien, deren Name (ohne Endung) `keep` verneint.
/// Unvollständige Schreibvorgänge (`.tmp`) gelten immer als verwaist.
pub fn sweep_dir(dir: &Path, min_age: Duration, keep: impl Fn(&str) -> bool) -> Swept {
    let mut out = Swept::default();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let path = e.path();
        let Ok(meta) = e.metadata() else { continue };
        if !meta.is_file() || !old_enough(&meta, min_age) {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let tmp = path.extension().is_some_and(|x| x == "tmp");
        if (tmp || !keep(stem)) && std::fs::remove_file(&path).is_ok() {
            out.files += 1;
            out.bytes += meta.len();
        }
    }
    out
}

/// Vorschauen (`<cache>/thumbs/2a/42.jpg`) von Bildern, die es nicht mehr gibt.
pub fn sweep_image_cache(base: &Path, images: &HashSet<i64>, min_age: Duration) -> Swept {
    let mut out = Swept::default();
    for variant in ["thumbs", "previews"] {
        let Ok(rd) = std::fs::read_dir(base.join(variant)) else {
            continue;
        };
        for bucket in rd.flatten() {
            if bucket.path().is_dir() {
                out += sweep_dir(&bucket.path(), min_age, |stem| {
                    stem.parse::<i64>().is_ok_and(|id| images.contains(&id))
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweeps_only_unreferenced_files() {
        let dir = std::env::temp_dir().join(format!("lw-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("thumbs/2a")).unwrap();
        for n in ["1-dn-a.lwdn", "1-dn-b.lwdn", "x.tmp"] {
            std::fs::write(dir.join(n), b"1234").unwrap();
        }
        std::fs::write(dir.join("thumbs/2a/42.jpg"), b"x").unwrap();
        std::fs::write(dir.join("thumbs/2a/298.jpg"), b"x").unwrap();

        // Zu jung: nichts passiert
        let keep = |s: &str| s == "1-dn-a";
        assert_eq!(sweep_dir(&dir, Duration::from_secs(3600), keep).files, 0);

        let swept = sweep_dir(&dir, Duration::ZERO, keep);
        assert_eq!(swept, Swept { files: 2, bytes: 8 });
        assert!(dir.join("1-dn-a.lwdn").exists());
        assert!(!dir.join("1-dn-b.lwdn").exists());

        let live: HashSet<i64> = [42].into();
        assert_eq!(sweep_image_cache(&dir, &live, Duration::ZERO).files, 1);
        assert!(dir.join("thumbs/2a/42.jpg").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn collects_ids_from_edits_and_snapshots() {
        let mut cat = Catalog::open_in_memory().unwrap();
        let ids = crate::catalog::tests_support::insert_dummy_images(&mut cat, 2);
        let p = DevelopParams::from_json(&serde_json::json!({
            "denoise": { "id": "1-dn-a", "amount": 80.0 },
            "masks": [{ "id": "m", "shapes": [{ "type": "raster", "maskId": "1-sky-b", "kind": "sky" }] }]
        }));
        cat.save_develop(&[ids[0]], &p).unwrap();
        cat.create_snapshot(ids[0], "s").unwrap();
        // Danach Entrauschung entfernt – die Datei braucht nur noch der Schnappschuss
        cat.save_develop(&[ids[0]], &DevelopParams::default())
            .unwrap();
        let live = cat.live().unwrap();
        assert!(live.files.contains("1-dn-a") && live.files.contains("1-sky-b"));
        assert_eq!(live.images.len(), 2);
    }

    #[test]
    fn drops_references_to_other_photos() {
        let mut cat = Catalog::open_in_memory().unwrap();
        let ids = crate::catalog::tests_support::insert_dummy_images(&mut cat, 2);
        let (a, b) = (ids[0], ids[1]);
        let edit = |owner: i64| {
            DevelopParams::from_json(&serde_json::json!({
                "exposure": 0.5,
                "denoise": { "id": format!("{owner}-dn-x"), "amount": 80.0 },
                "masks": [
                    { "id": "r", "shapes": [{ "type": "radial", "cx": 0.5, "cy": 0.5, "rx": 0.2, "ry": 0.2 }] },
                    { "id": "k", "shapes": [{ "type": "raster", "maskId": format!("{owner}-sky-y"), "kind": "sky" }] }
                ]
            }))
        };
        cat.save_develop(&[a], &edit(a)).unwrap();
        cat.save_develop(&[b], &edit(a)).unwrap(); // alter Fehler: von A eingefügt
        assert_eq!(cat.drop_foreign_refs().unwrap(), 1);
        let pb = cat.develop_params(b).unwrap();
        assert!(pb.denoise.is_none());
        assert_eq!(pb.masks.len(), 1, "Radialmaske bleibt");
        assert_eq!(pb.exposure, 0.5);
        assert!(
            cat.develop_params(a).unwrap().denoise.is_some(),
            "A unverändert"
        );
        assert_eq!(cat.drop_foreign_refs().unwrap(), 0);
    }
}
