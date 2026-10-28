//! Schlagworte.

use super::*;

impl Catalog {
    // ------------------------------------------------------------------
    // Schlagworte
    // ------------------------------------------------------------------

    /// Setzt ein Schlagwort. Ein manuelles Schlagwort wird nie durch ein
    /// KI-Schlagwort überschrieben.
    pub fn add_tag(
        &self,
        image_id: i64,
        name: &str,
        source: TagSource,
        confidence: Option<f64>,
    ) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Invalid(
                tr_s(
                    "Schlagwort darf nicht leer sein",
                    "Keyword must not be empty",
                )
                .into(),
            ));
        }
        self.conn.execute(
            "INSERT INTO tags (name) VALUES (?1) ON CONFLICT (name) DO NOTHING",
            [name],
        )?;
        let tag_id: i64 =
            self.conn
                .query_row("SELECT id FROM tags WHERE name = ?1", [name], |r| r.get(0))?;
        self.conn.execute(
            "INSERT INTO image_tags (image_id, tag_id, source, confidence) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (image_id, tag_id) DO UPDATE SET
                 source = CASE WHEN image_tags.source = 'manual' THEN 'manual' ELSE excluded.source END,
                 confidence = CASE WHEN image_tags.source = 'manual' THEN image_tags.confidence
                                   ELSE excluded.confidence END",
            params![image_id, tag_id, source.as_str(), confidence],
        )?;
        flush_search(&self.conn)?;
        Ok(())
    }

    pub fn remove_tag(&self, image_id: i64, name: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM image_tags WHERE image_id = ?1 AND tag_id = (SELECT id FROM tags WHERE name = ?2)",
            params![image_id, name],
        )?;
        flush_search(&self.conn)?;
        Ok(())
    }

    pub fn image_tags(&self, image_id: i64) -> Result<Vec<ImageTag>> {
        let mut st = self.conn.prepare(
            "SELECT t.name, it.source, it.confidence FROM image_tags it JOIN tags t ON t.id = it.tag_id
             WHERE it.image_id = ?1 ORDER BY it.source DESC, COALESCE(it.confidence, 1) DESC, t.name",
        )?;
        let rows = st.query_map([image_id], |r| {
            let src: String = r.get(1)?;
            Ok(ImageTag {
                name: r.get(0)?,
                source: if src == "ai" {
                    TagSource::Ai
                } else {
                    TagSource::Manual
                },
                confidence: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}
