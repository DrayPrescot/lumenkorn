//! Exportierte Dateien: Der Export trägt jede geschriebene Datei ein, damit
//! sich später etwa alle Exporte eines Projekts hochladen lassen. Exporte von
//! vor dieser Liste finden sich über die markierten Export-Ordner
//! ([`crate::scanner::EXPORT_MARKER`]) in den Ordnern des Projekts.

use crate::catalog::Catalog;
use crate::error::Result;
use crate::model::{ImageQuery, Scope};
use rusqlite::params;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Endungen, die als Export gelten (was Lumenkorn exportiert)
const EXPORT_EXTS: [&str; 6] = ["jpg", "jpeg", "tif", "tiff", "png", "dng"];

fn is_export_file(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXPORT_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Eindeutige Schreibweise eines Pfads, damit dieselbe Datei nicht doppelt
/// zählt (unter Windows etwa kurze 8.3-Namen wie `RUNNER~1` gegenüber dem
/// langen Namen)
fn unique(p: &Path) -> PathBuf {
    PathBuf::from(crate::paths::normalize_dir(p))
}

impl Catalog {
    /// Spalte 0 von `select` (mit Alias `i` für `images`) für die Fotos von
    /// `q`, `extra` wird an die Bedingung angehängt.
    fn paths_where(&self, select: &str, extra: &str, q: &ImageQuery) -> Result<Vec<String>> {
        let (where_sql, args) = self.where_clause(q)?;
        let mut st = self
            .conn
            .prepare(&format!("{select} WHERE {where_sql}{extra}"))?;
        let rows = st.query_map(rusqlite::params_from_iter(args), |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Exportierte Datei von Foto `image_id` merken (ein erneuter Export an
    /// dieselbe Stelle ersetzt den Eintrag).
    pub fn record_export(&self, image_id: i64, path: &Path) -> Result<()> {
        self.conn.execute(
            "INSERT INTO exports (path, image_id, exported_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET image_id = ?2, exported_at = ?3",
            params![path.to_string_lossy(), image_id, crate::catalog::now()],
        )?;
        Ok(())
    }

    /// Alle vorhandenen Export-Dateien eines Projekts, sortiert: die
    /// eingetragenen Exporte seiner Fotos und die Dateien in markierten
    /// Export-Ordnern innerhalb seiner Ordner.
    pub fn project_exports(&self, project_id: i64) -> Result<Vec<PathBuf>> {
        let mut out = BTreeSet::new();
        // Eine Abfrage für alle Fotos des Projekts statt einer je Foto
        let scope = ImageQuery::new(Scope::Project { id: project_id });
        for p in self.paths_where(
            "SELECT e.path FROM exports e JOIN images i ON i.id = e.image_id",
            "",
            &scope,
        )? {
            let p = PathBuf::from(p);
            if p.is_file() {
                out.insert(unique(&p));
            }
        }
        for f in self.project(project_id)?.folders {
            for dir in crate::scanner::export_dirs_in(Path::new(&f.path), f.recursive) {
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    out.extend(
                        rd.flatten()
                            .map(|e| e.path())
                            .filter(|p| p.is_file() && is_export_file(p))
                            .map(|p| unique(&p)),
                    );
                }
            }
        }
        Ok(out.into_iter().collect())
    }

    /// Die Originale aller Fotos eines Projekts (ohne abgelehnte und ohne
    /// fehlende Dateien), sortiert – zum Hochladen statt der Exporte.
    pub fn project_originals(&self, project_id: i64) -> Result<Vec<PathBuf>> {
        // Nur Pfad und Markierung lesen, nicht die ganze Fotoliste
        let scope = ImageQuery::new(Scope::Project { id: project_id });
        let out: BTreeSet<PathBuf> = self
            .paths_where("SELECT i.path FROM images i", " AND i.flag != -1", &scope)?
            .into_iter()
            .map(PathBuf::from)
            .filter(|p| p.is_file())
            .map(|p| unique(&p))
            .collect();
        Ok(out.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_image_files_count() {
        assert!(is_export_file(Path::new("a/B.JPG")));
        assert!(is_export_file(Path::new("a/b.dng")));
        assert!(!is_export_file(Path::new("a/.lumenkorn-export")));
        assert!(!is_export_file(Path::new("a/b.xmp")));
    }
}
