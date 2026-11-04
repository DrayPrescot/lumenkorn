//! Alben und Diashows auf der Platte (`dokumente/` im Datenordner), je Dokument
//! eine JSON-Datei. Früher lagen sie im `localStorage` der WebView: Der ist
//! klein, ein voller Speicher ging ohne Meldung verloren, und beim Löschen der
//! WebView-Daten wäre die Arbeit weg.

use crate::error::Result;
use crate::model::Scope;
use lumenkorn_develop::fsutil;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DocKind {
    Album,
    Slideshow,
}

impl DocKind {
    fn name(self) -> &'static str {
        match self {
            DocKind::Album => "album",
            DocKind::Slideshow => "diashow",
        }
    }
}

pub struct DocStore {
    dir: PathBuf,
}

impl DocStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, kind: DocKind, scope: &Scope) -> Result<PathBuf> {
        // Ordnerpfade taugen nicht als Dateiname, deshalb ihre Prüfsumme.
        // Projekte bleiben lesbar: Die Seitenleiste listet sie anhand der Namen.
        let tail = match scope {
            Scope::Project { id } => format!("projekt-{id}"),
            other => {
                let json = serde_json::to_string(other)?;
                let hash = blake3::hash(json.as_bytes()).to_hex();
                format!("bereich-{}", &hash[..32])
            }
        };
        Ok(self.dir.join(format!("{}-{tail}.json", kind.name())))
    }

    /// Inhalt des Dokuments; `None`, wenn es keins gibt.
    pub fn get(&self, kind: DocKind, scope: &Scope) -> Result<Option<String>> {
        match std::fs::read_to_string(self.path(kind, scope)?) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn put(&self, kind: DocKind, scope: &Scope, json: &str) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        fsutil::write_atomic(&self.path(kind, scope)?, json.as_bytes())?;
        Ok(())
    }

    pub fn delete(&self, kind: DocKind, scope: &Scope) -> Result<()> {
        match std::fs::remove_file(self.path(kind, scope)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Projekte, für die es ein Dokument dieser Art gibt
    pub fn projects(&self, kind: DocKind) -> Vec<i64> {
        let prefix = format!("{}-projekt-", kind.name());
        let Ok(dir) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        dir.flatten()
            .filter_map(|e| {
                let name = e.file_name();
                let id = name
                    .to_str()?
                    .strip_prefix(&prefix)?
                    .strip_suffix(".json")?
                    .parse()
                    .ok()?;
                Some(id)
            })
            .collect()
    }

    /// Dokumente eines gelöschten Projekts entfernen.
    pub fn delete_project(&self, id: i64) {
        for kind in [DocKind::Album, DocKind::Slideshow] {
            if let Err(e) = self.delete(kind, &Scope::Project { id }) {
                log::warn!("{} von Projekt {id} nicht gelöscht: {e}", kind.name());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_per_kind_and_scope() {
        let d = tempfile::tempdir().unwrap();
        let s = DocStore::new(d.path().join("dokumente"));
        let project = Scope::Project { id: 7 };
        let folder = Scope::Folder {
            path: r"D:\Fotos\Urlaub: 2026".into(),
            recursive: true,
        };
        assert_eq!(s.get(DocKind::Album, &project).unwrap(), None);
        assert!(s.projects(DocKind::Album).is_empty());

        s.put(DocKind::Album, &project, "{\"a\":1}").unwrap();
        s.put(DocKind::Slideshow, &project, "{\"s\":1}").unwrap();
        s.put(DocKind::Album, &folder, "{\"f\":1}").unwrap();
        assert_eq!(
            s.get(DocKind::Album, &project).unwrap().as_deref(),
            Some("{\"a\":1}")
        );
        assert_eq!(
            s.get(DocKind::Slideshow, &project).unwrap().as_deref(),
            Some("{\"s\":1}")
        );
        assert_eq!(
            s.get(DocKind::Album, &folder).unwrap().as_deref(),
            Some("{\"f\":1}")
        );
        assert_eq!(s.projects(DocKind::Album), vec![7]);

        s.delete(DocKind::Album, &project).unwrap();
        s.delete(DocKind::Album, &project).unwrap();
        assert_eq!(s.get(DocKind::Album, &project).unwrap(), None);
        assert!(s.projects(DocKind::Album).is_empty());

        s.delete_project(7);
        assert_eq!(s.get(DocKind::Slideshow, &project).unwrap(), None);
        assert!(s.get(DocKind::Album, &folder).unwrap().is_some());
    }
}
