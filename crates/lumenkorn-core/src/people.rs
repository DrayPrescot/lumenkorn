//! Personen: erkannte Gesichter speichern, zu Gruppen zusammenfassen,
//! benennen, bestätigen und ablehnen.
//!
//! Jedes Gesicht gehört höchstens einer Person. Personen ohne Namen sind
//! automatisch gebildete Gruppen ähnlicher Gesichter. Neue Gesichter werden
//! mit dem Mittelwert (Zentroid) jeder Person verglichen:
//!
//! 1. ähnlich genug zu einer **benannten** Person → als Vorschlag zuordnen
//!    (`confirmed = 0`; der Nutzer bestätigt oder lehnt ab),
//! 2. sonst ähnlich genug zu einer **unbenannten Gruppe** → dazu,
//! 3. sonst neue Gruppe.
//!
//! Benannte Personen werden über ihre bestätigten Gesichter beschrieben, damit
//! falsche Vorschläge die Person nicht schleichend verändern.

use crate::catalog::{Catalog, now};
use crate::error::{Error, Result};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::collections::HashMap;

/// Ab dieser Kosinus-Ähnlichkeit zum Zentroid gehört ein Gesicht zu einer Person
/// bzw. Gruppe (siehe `lumenkorn_ai::faces::GROUP`).
pub const GROUP: f32 = 0.5;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub id: i64,
    /// `None`: unbenannte Gruppe
    pub name: Option<String>,
    pub face_count: i64,
    /// davon nur vorgeschlagen (noch nicht bestätigt)
    pub suggested: i64,
    pub image_count: i64,
    /// Gesicht für das Vorschaubild
    pub cover_face: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceInfo {
    pub id: i64,
    pub image_id: i64,
    /// x, y, Breite, Höhe – normiert auf das unbearbeitete Bild
    pub rect: [f64; 4],
    pub score: f64,
    pub person_id: Option<i64>,
    pub person_name: Option<String>,
    pub confirmed: bool,
}

/// Ein neu gefundenes Gesicht.
pub struct NewFace {
    pub rect: [f32; 4],
    pub score: f32,
    /// normiert (Länge 1)
    pub embedding: Vec<f32>,
}

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn from_blob(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn normalized(v: &[f32]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter().map(|x| x / n).collect()
}

#[derive(Default)]
struct Cluster {
    named: bool,
    /// Summe aller Gesichter bzw. nur der bestätigten (benannte Personen)
    sum: Vec<f32>,
    count: usize,
    confirmed_sum: Vec<f32>,
    confirmed: usize,
    /// Normierter Zentroid, bei jedem `add` aktualisiert – beim Zuordnen wird
    /// er für jede Gruppe gebraucht, Neuberechnen wäre O(Gesichter × Gruppen).
    cent: Vec<f32>,
}

impl Cluster {
    fn add(&mut self, e: &[f32], confirmed: bool) {
        if self.sum.is_empty() {
            self.sum = vec![0.0; e.len()];
            self.confirmed_sum = vec![0.0; e.len()];
        }
        for (s, x) in self.sum.iter_mut().zip(e) {
            *s += x;
        }
        self.count += 1;
        if confirmed {
            for (s, x) in self.confirmed_sum.iter_mut().zip(e) {
                *s += x;
            }
            self.confirmed += 1;
        }
        self.cent = self.centroid().unwrap_or_default();
    }

    fn centroid(&self) -> Option<Vec<f32>> {
        if self.named && self.confirmed > 0 {
            Some(normalized(&self.confirmed_sum))
        } else if self.count > 0 {
            Some(normalized(&self.sum))
        } else {
            None
        }
    }
}

/// Zentroide aller Personen und Gruppen, im Speicher gehalten.
#[derive(Default)]
pub struct PeopleIndex {
    clusters: HashMap<i64, Cluster>,
    /// Stand des Katalogs beim Aufbau (`Catalog::people_gen`)
    generation: u64,
}

impl PeopleIndex {
    /// Beste Person: zuerst benannte, dann Gruppen; `None` unter der Schwelle.
    fn best(&self, e: &[f32], exclude: &[i64]) -> Option<i64> {
        let mut best: [(Option<i64>, f32); 2] = [(None, GROUP), (None, GROUP)];
        for (id, c) in &self.clusters {
            if exclude.contains(id) {
                continue;
            }
            if c.cent.is_empty() {
                continue;
            }
            let s = cosine(e, &c.cent);
            let slot = &mut best[usize::from(!c.named)];
            if s >= slot.1 {
                *slot = (Some(*id), s);
            }
        }
        best[0].0.or(best[1].0)
    }

    fn add(&mut self, person: i64, named: bool, e: &[f32], confirmed: bool) {
        let c = self.clusters.entry(person).or_default();
        c.named |= named;
        c.add(e, confirmed);
    }
}

impl Catalog {
    // --- Analyse -------------------------------------------------------------

    /// Bilder, deren Gesichter noch gesucht werden müssen.
    pub fn pending_faces(&self, limit: usize) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare(
            "SELECT id FROM images WHERE face_state = 0 AND thumb_state = 1 AND missing = 0
             ORDER BY id LIMIT ?1",
        )?;
        let rows = st.query_map([limit as i64], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// (analysiert, gesamt, Gesichter)
    pub fn face_counts(&self) -> Result<(i64, i64, i64)> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(face_state != 0), 0), COUNT(*),
                    (SELECT COUNT(*) FROM faces WHERE hidden = 0)
             FROM images WHERE missing = 0",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?)
    }

    pub fn set_faces_failed(&self, image_id: i64) -> Result<()> {
        self.conn
            .execute("UPDATE images SET face_state = 2 WHERE id = ?1", [image_id])?;
        Ok(())
    }

    /// Zentroide aus dem Katalog aufbauen (einmal; danach laufend ergänzt).
    pub fn people_index(&self) -> Result<PeopleIndex> {
        let mut idx = PeopleIndex {
            generation: self.people_gen.get(),
            ..Default::default()
        };
        let mut st = self.conn.prepare(
            "SELECT f.person_id, p.name IS NOT NULL, f.embedding, f.confirmed
             FROM faces f JOIN persons p ON p.id = f.person_id WHERE f.hidden = 0",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, bool>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, bool>(3)?,
            ))
        })?;
        for row in rows {
            let (pid, named, blob, confirmed) = row?;
            idx.add(pid, named, &from_blob(&blob), confirmed);
        }
        Ok(idx)
    }

    /// Passt ein Index noch zum Katalog? Sonst neu aufbauen: Er könnte auf
    /// gelöschte Gruppen verweisen (Fremdschlüsselfehler beim Speichern).
    pub fn people_index_current(&self, idx: &PeopleIndex) -> bool {
        idx.generation == self.people_gen.get()
    }

    /// Index ungültig machen (z. B. nach einem Fehler mitten im Speichern).
    pub fn invalidate_people_index(&self) {
        self.people_gen.set(self.people_gen.get() + 1);
    }

    /// Speichert die Gesichter eines Bildes (ersetzt frühere) und ordnet sie
    /// Personen bzw. Gruppen zu. Liefert die neuen Gesichts-IDs.
    pub fn save_faces(
        &mut self,
        image_id: i64,
        faces: &[NewFace],
        index: &mut PeopleIndex,
    ) -> Result<Vec<i64>> {
        let ids = self.with_tx(|tx| {
            tx.execute("DELETE FROM faces WHERE image_id = ?1", [image_id])?;
            let mut ids = Vec::with_capacity(faces.len());
            // Eine Person erscheint höchstens einmal pro Foto
            let mut used: Vec<i64> = Vec::new();
            // Sicherste Gesichter zuerst zuordnen
            let mut order: Vec<usize> = (0..faces.len()).collect();
            order.sort_by(|a, b| faces[*b].score.total_cmp(&faces[*a].score));
            let mut assigned = vec![0i64; faces.len()];
            for &k in &order {
                let f = &faces[k];
                let person = match index.best(&f.embedding, &used) {
                    Some(p) => p,
                    None => {
                        tx.execute(
                            "INSERT INTO persons (name, created_at) VALUES (NULL, ?1)",
                            [now()],
                        )?;
                        tx.last_insert_rowid()
                    }
                };
                used.push(person);
                assigned[k] = person;
                let named = index.clusters.get(&person).is_some_and(|c| c.named);
                index.add(person, named, &f.embedding, false);
            }
            for (f, person) in faces.iter().zip(&assigned) {
                tx.execute(
                    "INSERT INTO faces (image_id, x, y, w, h, score, embedding, person_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        image_id,
                        f.rect[0] as f64,
                        f.rect[1] as f64,
                        f.rect[2] as f64,
                        f.rect[3] as f64,
                        f.score as f64,
                        to_blob(&f.embedding),
                        person
                    ],
                )?;
                ids.push(tx.last_insert_rowid());
            }
            tx.execute("UPDATE images SET face_state = 1 WHERE id = ?1", [image_id])?;
            Ok(ids)
        })?;
        self.drop_empty_groups()?;
        Ok(ids)
    }

    // --- Abfragen ------------------------------------------------------------

    /// Benannte Personen (alphabetisch), dann Gruppen (größte zuerst).
    pub fn list_people(&self) -> Result<Vec<Person>> {
        let mut st = self.conn.prepare(
            "SELECT p.id, p.name,
                    COUNT(f.id), COALESCE(SUM(f.confirmed = 0), 0), COUNT(DISTINCT f.image_id),
                    (SELECT f2.id FROM faces f2 WHERE f2.person_id = p.id AND f2.hidden = 0
                     ORDER BY f2.confirmed DESC, f2.w * f2.h * f2.score DESC LIMIT 1)
             FROM persons p JOIN faces f ON f.person_id = p.id AND f.hidden = 0
             GROUP BY p.id
             ORDER BY p.name IS NULL, p.name COLLATE NOCASE, COUNT(f.id) DESC",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Person {
                id: r.get(0)?,
                name: r.get(1)?,
                face_count: r.get(2)?,
                suggested: r.get(3)?,
                image_count: r.get(4)?,
                cover_face: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn faces_where(&self, cond: &str, arg: i64) -> Result<Vec<FaceInfo>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT f.id, f.image_id, f.x, f.y, f.w, f.h, f.score, f.person_id, p.name, f.confirmed
             FROM faces f LEFT JOIN persons p ON p.id = f.person_id
             WHERE f.hidden = 0 AND {cond}
             ORDER BY f.confirmed DESC, f.image_id, f.x"
        ))?;
        let rows = st.query_map([arg], |r| {
            Ok(FaceInfo {
                id: r.get(0)?,
                image_id: r.get(1)?,
                rect: [r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?],
                score: r.get(6)?,
                person_id: r.get(7)?,
                person_name: r.get(8)?,
                confirmed: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn face(&self, face_id: i64) -> Result<FaceInfo> {
        // auch ausgeblendete Gesichter (für Vorschaubilder)
        let mut st = self.conn.prepare(
            "SELECT f.id, f.image_id, f.x, f.y, f.w, f.h, f.score, f.person_id, p.name, f.confirmed
             FROM faces f LEFT JOIN persons p ON p.id = f.person_id WHERE f.id = ?1",
        )?;
        st.query_row([face_id], |r| {
            Ok(FaceInfo {
                id: r.get(0)?,
                image_id: r.get(1)?,
                rect: [r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?],
                score: r.get(6)?,
                person_id: r.get(7)?,
                person_name: r.get(8)?,
                confirmed: r.get(9)?,
            })
        })
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("Gesicht {face_id}")))
    }

    /// Alle Fotos erneut analysieren lassen. Benannte Personen bleiben; ihre
    /// Gesichter werden beim erneuten Durchlauf wieder vorgeschlagen.
    pub fn reset_faces(&self) -> Result<()> {
        self.conn.execute("UPDATE images SET face_state = 0", [])?;
        Ok(())
    }

    pub fn image_faces(&self, image_id: i64) -> Result<Vec<FaceInfo>> {
        self.faces_where("f.image_id = ?1", image_id)
    }

    pub fn person_faces(&self, person_id: i64) -> Result<Vec<FaceInfo>> {
        self.faces_where("f.person_id = ?1", person_id)
    }

    /// Sichtbare Gesichter mehrerer Fotos (für Änderungen an einer Auswahl).
    pub fn faces_of_images(&self, image_ids: &[i64]) -> Result<Vec<FaceInfo>> {
        let ids = serde_json::to_string(image_ids)?;
        let mut st = self.conn.prepare(
            "SELECT f.id, f.image_id, f.x, f.y, f.w, f.h, f.score, f.person_id, p.name, f.confirmed
             FROM faces f LEFT JOIN persons p ON p.id = f.person_id
             WHERE f.hidden = 0 AND f.image_id IN (SELECT value FROM json_each(?1))
             ORDER BY f.image_id, f.x",
        )?;
        let rows = st.query_map([ids], |r| {
            Ok(FaceInfo {
                id: r.get(0)?,
                image_id: r.get(1)?,
                rect: [r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?],
                score: r.get(6)?,
                person_id: r.get(7)?,
                person_name: r.get(8)?,
                confirmed: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Mehrere Gesichter auf einmal ändern: mit `name` dieser Person zuordnen
    /// (bestätigt; neu angelegt, falls es sie nicht gibt), ohne `name` die
    /// Zuordnung lösen und der bisherigen Person nicht wieder vorschlagen.
    /// Liefert die Ziel-Person.
    pub fn reassign_faces(&mut self, face_ids: &[i64], name: Option<&str>) -> Result<Option<i64>> {
        if face_ids.is_empty() {
            return Ok(None);
        }
        let ids = serde_json::to_string(face_ids)?;
        let person = match name.map(str::trim) {
            Some("") => return Err(Error::Invalid("Bitte einen Namen eingeben".into())),
            Some(name) => Some(match self.person_by_name(name)? {
                Some(id) => id,
                None => {
                    self.conn.execute(
                        "INSERT INTO persons (name, created_at) VALUES (?1, ?2)",
                        params![name, now()],
                    )?;
                    self.conn.last_insert_rowid()
                }
            }),
            None => None,
        };
        self.with_tx(|tx| {
            match person {
                Some(p) => tx.execute(
                    "UPDATE faces SET person_id = ?2, confirmed = 1, rejected_person = NULL, hidden = 0
                     WHERE id IN (SELECT value FROM json_each(?1))",
                    params![ids, p],
                )?,
                None => tx.execute(
                    "UPDATE faces SET rejected_person = person_id, person_id = NULL, confirmed = 0
                     WHERE id IN (SELECT value FROM json_each(?1)) AND person_id IS NOT NULL",
                    params![ids],
                )?,
            };
            Ok(())
        })?;
        self.invalidate_people_index();
        self.drop_empty_groups()?;
        if let Some(p) = person {
            self.suggest_for(p)?;
        }
        Ok(person)
    }

    // --- Bearbeiten ------------------------------------------------------------

    fn person_by_name(&self, name: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM persons WHERE name = ?1 COLLATE NOCASE",
                [name],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub(crate) fn drop_empty_groups(&self) -> Result<()> {
        let n = self.conn.execute(
            "DELETE FROM persons WHERE name IS NULL AND NOT EXISTS
               (SELECT 1 FROM faces f WHERE f.person_id = persons.id AND f.hidden = 0)",
            [],
        )?;
        if n > 0 {
            self.invalidate_people_index();
        }
        Ok(())
    }

    /// Person bzw. Gruppe benennen. Gibt es den Namen schon, werden beide
    /// zusammengeführt. Die Gesichter gelten danach als bestätigt.
    /// Liefert die ID der (ggf. zusammengeführten) Person.
    pub fn name_person(&mut self, person_id: i64, name: &str) -> Result<i64> {
        self.invalidate_people_index();
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Invalid("Bitte einen Namen eingeben".into()));
        }
        let existing = self.person_by_name(name)?.filter(|id| *id != person_id);
        // Eine Gruppe benennen bestätigt ihre Gesichter. Eine schon benannte
        // Person nur umbenennen (z. B. „Max“ → „Max Mustermann“) lässt offene
        // Vorschläge offen – sie sind weiterhin zu prüfen.
        let had_name: bool = self.conn.query_row(
            "SELECT name IS NOT NULL FROM persons WHERE id = ?1",
            [person_id],
            |r| r.get(0),
        )?;
        let confirm = if had_name { "confirmed" } else { "1" };
        let target = self.with_tx(|tx| match existing {
            Some(other) => {
                tx.execute(
                    &format!("UPDATE faces SET person_id = ?1, confirmed = {confirm} WHERE person_id = ?2"),
                    params![other, person_id],
                )?;
                tx.execute("DELETE FROM persons WHERE id = ?1", [person_id])?;
                Ok(other)
            }
            None => {
                tx.execute(
                    "UPDATE persons SET name = ?2 WHERE id = ?1",
                    params![person_id, name],
                )?;
                tx.execute(
                    &format!("UPDATE faces SET confirmed = {confirm} WHERE person_id = ?1"),
                    [person_id],
                )?;
                Ok(person_id)
            }
        })?;
        self.suggest_for(target)?;
        Ok(target)
    }

    /// Ein einzelnes Gesicht einer Person zuordnen (neu angelegt, falls nötig).
    pub fn assign_face(&mut self, face_id: i64, name: &str) -> Result<i64> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Invalid("Bitte einen Namen eingeben".into()));
        }
        let person = match self.person_by_name(name)? {
            Some(id) => id,
            None => {
                self.conn.execute(
                    "INSERT INTO persons (name, created_at) VALUES (?1, ?2)",
                    params![name, now()],
                )?;
                self.conn.last_insert_rowid()
            }
        };
        self.conn.execute(
            "UPDATE faces SET person_id = ?2, confirmed = 1, rejected_person = NULL, hidden = 0
             WHERE id = ?1",
            params![face_id, person],
        )?;
        self.drop_empty_groups()?;
        self.suggest_for(person)?;
        Ok(person)
    }

    pub fn confirm_face(&self, face_id: i64) -> Result<()> {
        self.conn
            .execute("UPDATE faces SET confirmed = 1 WHERE id = ?1", [face_id])?;
        Ok(())
    }

    /// „Nicht diese Person“: Zuordnung lösen und nicht erneut vorschlagen.
    pub fn reject_face(&self, face_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE faces SET rejected_person = person_id, person_id = NULL, confirmed = 0
             WHERE id = ?1",
            [face_id],
        )?;
        self.drop_empty_groups()?;
        Ok(())
    }

    /// Gesicht ausblenden (kein Gesicht, unwichtig).
    pub fn hide_face(&self, face_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE faces SET hidden = 1, person_id = NULL, confirmed = 0 WHERE id = ?1",
            [face_id],
        )?;
        self.drop_empty_groups()?;
        Ok(())
    }

    /// Person löschen – die Gesichter bleiben, sind aber keiner Person mehr zugeordnet.
    pub fn delete_person(&self, person_id: i64) -> Result<()> {
        self.invalidate_people_index();
        self.conn.execute(
            "UPDATE faces SET person_id = NULL, confirmed = 0 WHERE person_id = ?1",
            [person_id],
        )?;
        self.conn
            .execute("DELETE FROM persons WHERE id = ?1", [person_id])?;
        Ok(())
    }

    /// Nach dem Benennen: ähnliche unbenannte Gruppen und lose Gesichter als
    /// Vorschläge zur Person holen. Abgelehnte Gesichter bleiben außen vor.
    fn suggest_for(&mut self, person: i64) -> Result<()> {
        let idx = self.people_index()?;
        let Some(target) = idx.clusters.get(&person).and_then(Cluster::centroid) else {
            return Ok(());
        };
        let groups: Vec<i64> = idx
            .clusters
            .iter()
            .filter(|(id, c)| **id != person && !c.named)
            .filter(|(_, c)| !c.cent.is_empty() && cosine(&c.cent, &target) >= GROUP)
            .map(|(id, _)| *id)
            .collect();
        let loose: Vec<i64> = {
            let mut st = self.conn.prepare(
                "SELECT id, embedding FROM faces
                 WHERE person_id IS NULL AND hidden = 0 AND COALESCE(rejected_person, -1) != ?1",
            )?;
            let rows = st.query_map([person], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
            })?;
            let mut v = Vec::new();
            for row in rows {
                let (id, blob) = row?;
                if cosine(&from_blob(&blob), &target) >= GROUP {
                    v.push(id);
                }
            }
            v
        };
        self.with_tx(|tx| {
            for g in &groups {
                tx.execute(
                    "UPDATE faces SET person_id = ?1, confirmed = 0
                     WHERE person_id = ?2 AND COALESCE(rejected_person, -1) != ?1
                       AND image_id NOT IN (SELECT image_id FROM faces WHERE person_id = ?1)",
                    params![person, g],
                )?;
            }
            for f in &loose {
                tx.execute(
                    "UPDATE faces SET person_id = ?1, confirmed = 0 WHERE id = ?2
                       AND image_id NOT IN (SELECT image_id FROM faces WHERE person_id = ?1)",
                    params![person, f],
                )?;
            }
            Ok(())
        })?;
        self.drop_empty_groups()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ImageQuery, Scope};

    /// Einheitsvektor nahe an einer „Identität“ `k` mit kleiner Abweichung.
    fn face(k: usize, jitter: f32) -> Vec<f32> {
        let mut v = vec![0.0; 16];
        v[k] = 1.0;
        v[(k + 7) % 16] = jitter;
        normalized(&v)
    }

    fn catalog_with_images(n: usize) -> (Catalog, Vec<i64>) {
        let mut cat = Catalog::open_in_memory().unwrap();
        let ids = crate::catalog::tests_support::insert_dummy_images(&mut cat, n);
        (cat, ids)
    }

    fn add(cat: &mut Catalog, idx: &mut PeopleIndex, image: i64, e: Vec<f32>) -> i64 {
        cat.save_faces(
            image,
            &[NewFace {
                rect: [0.1, 0.1, 0.2, 0.2],
                score: 0.9,
                embedding: e,
            }],
            idx,
        )
        .unwrap()[0]
    }

    #[test]
    fn groups_names_suggests_and_filters() {
        let (mut cat, imgs) = catalog_with_images(5);
        let mut idx = cat.people_index().unwrap();
        // Anna (Identität 0) auf drei Fotos, Ben (Identität 1) auf einem
        let a1 = add(&mut cat, &mut idx, imgs[0], face(0, 0.1));
        let _a2 = add(&mut cat, &mut idx, imgs[1], face(0, 0.2));
        let b1 = add(&mut cat, &mut idx, imgs[2], face(1, 0.1));
        let people = cat.list_people().unwrap();
        assert_eq!(people.len(), 2, "zwei Gruppen: {people:?}");
        let anna_group = cat.image_faces(imgs[0]).unwrap()[0].person_id.unwrap();
        assert_eq!(
            people
                .iter()
                .find(|p| p.id == anna_group)
                .unwrap()
                .face_count,
            2
        );

        // Benennen bestätigt die Gruppe
        let anna = cat.name_person(anna_group, "Anna").unwrap();
        assert!(cat.person_faces(anna).unwrap().iter().all(|f| f.confirmed));

        // Neues Foto von Anna wird ihr vorgeschlagen (nicht bestätigt)
        let mut idx = cat.people_index().unwrap();
        let a3 = add(&mut cat, &mut idx, imgs[3], face(0, 0.15));
        let f3 = &cat.image_faces(imgs[3]).unwrap()[0];
        assert_eq!(f3.person_name.as_deref(), Some("Anna"));
        assert!(!f3.confirmed);

        // Ablehnen löst die Zuordnung und schlägt sie nicht erneut vor
        cat.reject_face(a3).unwrap();
        assert!(cat.image_faces(imgs[3]).unwrap()[0].person_id.is_none());
        cat.name_person(anna, "Anna").unwrap();
        assert!(cat.image_faces(imgs[3]).unwrap()[0].person_id.is_none());

        // Einzelnes Gesicht von Hand zuordnen; gleicher Name führt zusammen
        cat.assign_face(b1, "Ben").unwrap();
        let ben = cat.image_faces(imgs[2]).unwrap()[0].person_id.unwrap();
        let mut idx = cat.people_index().unwrap();
        add(&mut cat, &mut idx, imgs[4], face(5, 0.0));
        let g = cat.image_faces(imgs[4]).unwrap()[0].person_id.unwrap();
        assert_eq!(
            cat.name_person(g, "ben").unwrap(),
            ben,
            "Name ohne Groß/klein → zusammengeführt"
        );

        // Filter und Suche
        let anna_imgs = cat
            .query_images(&ImageQuery::new(Scope::Person { id: anna }))
            .unwrap();
        assert_eq!(anna_imgs.len(), 2);
        let mut q = ImageQuery::new(Scope::All);
        q.search = Some("anna".into());
        assert_eq!(cat.query_images(&q).unwrap().len(), 2);

        // Ausblenden und Löschen
        cat.hide_face(a1).unwrap();
        assert_eq!(
            cat.query_images(&ImageQuery::new(Scope::Person { id: anna }))
                .unwrap()
                .len(),
            1
        );
        cat.delete_person(ben).unwrap();
        assert!(cat.list_people().unwrap().iter().all(|p| p.id != ben));
        let (done, total, faces) = cat.face_counts().unwrap();
        assert_eq!((done, total), (5, 5));
        assert_eq!(faces, 4);
    }

    #[test]
    fn one_face_per_person_and_photo() {
        let (mut cat, imgs) = catalog_with_images(1);
        let mut idx = cat.people_index().unwrap();
        // Zwei sehr ähnliche Gesichter auf demselben Foto (z. B. Zwillinge)
        let nf = |e| NewFace {
            rect: [0.1, 0.1, 0.2, 0.2],
            score: 0.9,
            embedding: e,
        };
        cat.save_faces(imgs[0], &[nf(face(0, 0.1)), nf(face(0, 0.12))], &mut idx)
            .unwrap();
        let f = cat.image_faces(imgs[0]).unwrap();
        assert_eq!(f.len(), 2);
        assert_ne!(f[0].person_id, f[1].person_id);
    }

    /// Mehrere Fotos auf einmal einer anderen Person zuordnen bzw. aus einer
    /// Person herausnehmen – alle, nicht nur das erste.
    #[test]
    fn reassigns_faces_of_several_photos() {
        let (mut cat, imgs) = catalog_with_images(4);
        let mut idx = cat.people_index().unwrap();
        for &i in &imgs {
            add(&mut cat, &mut idx, i, face(0, 0.1));
        }
        let group = cat.image_faces(imgs[0]).unwrap()[0].person_id.unwrap();
        let anna = cat.name_person(group, "Anna").unwrap();
        assert_eq!(cat.faces_of_images(&imgs).unwrap().len(), 4);

        // Drei der vier Fotos zeigen in Wahrheit Ben
        let wrong: Vec<i64> = cat
            .faces_of_images(&imgs[1..])
            .unwrap()
            .iter()
            .map(|f| f.id)
            .collect();
        assert_eq!(wrong.len(), 3);
        let ben = cat.reassign_faces(&wrong, Some("Ben")).unwrap().unwrap();
        let anna_imgs = cat
            .query_images(&ImageQuery::new(Scope::Person { id: anna }))
            .unwrap();
        assert_eq!(anna_imgs.len(), 1, "nur noch das erste Foto bei Anna");
        let ben_faces = cat.person_faces(ben).unwrap();
        assert_eq!(ben_faces.len(), 3);
        assert!(ben_faces.iter().all(|f| f.confirmed));

        // Aus Ben wieder herausnehmen: alle drei, und nicht erneut vorgeschlagen
        cat.reassign_faces(&wrong, None).unwrap();
        assert!(cat.person_faces(ben).unwrap().is_empty());
        cat.name_person(anna, "Anna").unwrap();
        let back: Vec<_> = cat.faces_of_images(&imgs[1..]).unwrap();
        assert!(back.iter().all(|f| f.person_id != Some(ben)));
        // Leerer Name wird abgelehnt
        assert!(cat.reassign_faces(&wrong, Some("  ")).is_err());
    }

    /// Umbenennen einer schon benannten Person behält offene Vorschläge;
    /// ein vorhandener Name führt zusammen.
    #[test]
    fn renames_named_person() {
        let (mut cat, imgs) = catalog_with_images(3);
        let mut idx = cat.people_index().unwrap();
        add(&mut cat, &mut idx, imgs[0], face(0, 0.1));
        let group = cat.image_faces(imgs[0]).unwrap()[0].person_id.unwrap();
        let max = cat.name_person(group, "Max").unwrap();
        // Neues Foto wird Max nur vorgeschlagen
        let mut idx = cat.people_index().unwrap();
        add(&mut cat, &mut idx, imgs[1], face(0, 0.12));
        assert!(!cat.image_faces(imgs[1]).unwrap()[0].confirmed);

        assert_eq!(
            cat.name_person(max, "Max Mustermann").unwrap(),
            max,
            "gleiche Person"
        );
        let faces = cat.person_faces(max).unwrap();
        assert!(
            faces
                .iter()
                .all(|f| f.person_name.as_deref() == Some("Max Mustermann"))
        );
        assert_eq!(
            faces.iter().filter(|f| f.confirmed).count(),
            1,
            "Vorschlag bleibt offen"
        );

        // Umbenennen auf einen vorhandenen Namen führt zusammen
        let mut idx = cat.people_index().unwrap();
        add(&mut cat, &mut idx, imgs[2], face(3, 0.0));
        let g2 = cat.image_faces(imgs[2]).unwrap()[0].person_id.unwrap();
        let erika = cat.name_person(g2, "Erika").unwrap();
        assert_eq!(cat.name_person(erika, "max mustermann").unwrap(), max);
        assert_eq!(cat.person_faces(max).unwrap().len(), 3);
    }
}
