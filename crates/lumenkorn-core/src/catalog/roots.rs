//! Quellordner und Ordnerbaum.

use super::*;

impl Catalog {
    // ------------------------------------------------------------------
    // Quellordner
    // ------------------------------------------------------------------

    pub fn add_root(&self, path: &Path) -> Result<Root> {
        if !path.is_dir() {
            return Err(Error::Invalid(tr!(
                "{} ist kein Ordner",
                "{} is not a folder",
                path.display()
            )));
        }
        let p = paths::normalize_dir(path);

        // Verschachtelte Quellordner würden Bilder doppelt erfassen.
        for r in self.roots()? {
            if paths::is_within(&p, &r.path) {
                return Err(Error::Invalid(tr!(
                    "{p} liegt bereits in Quellordner {}",
                    "{p} is already inside source folder {}",
                    r.path
                )));
            }
            if paths::is_within(&r.path, &p) {
                return Err(Error::Invalid(tr!(
                    "{p} enthält den bestehenden Quellordner {}. Bitte diesen zuerst entfernen.",
                    "{p} contains the existing source folder {}. Please remove it first.",
                    r.path
                )));
            }
        }

        self.conn.execute(
            "INSERT INTO roots (path, added_at) VALUES (?1, ?2)",
            params![p, now()],
        )?;
        self.root(self.conn.last_insert_rowid())
    }

    /// Entfernt den Quellordner aus dem Katalog. Die Dateien bleiben unangetastet.
    pub fn remove_root(&self, id: i64) -> Result<()> {
        let n = self.conn.execute("DELETE FROM roots WHERE id = ?1", [id])?;
        flush_search(&self.conn)?;
        // Gruppen, deren Gesichter alle in diesem Ordner lagen
        self.drop_empty_groups(None)?;
        if n == 0 {
            return Err(Error::NotFound(tr!(
                "Quellordner {id}",
                "Source folder {id}"
            )));
        }
        Ok(())
    }

    /// Wie [`Catalog::remove_root`], löscht die Fotos aber in Portionen und
    /// gibt die Katalogsperre dazwischen frei. Bei Katalogen mit Zehntausenden
    /// Fotos hielte das Löschen in einem Zug die Sperre sonst sehr lange.
    /// Der Aufrufer darf die Sperre nicht selbst halten.
    pub fn remove_root_chunked(catalog: &std::sync::Mutex<Catalog>, id: i64) -> Result<()> {
        const CHUNK: i64 = 2000;
        catalog.locked().root(id)?;
        loop {
            let n = catalog.locked().with_tx(|tx| {
                Ok(tx.execute(
                    "DELETE FROM images WHERE id IN
                       (SELECT id FROM images WHERE root_id = ?1 LIMIT ?2)",
                    params![id, CHUNK],
                )?)
            })?;
            if n == 0 {
                break;
            }
        }
        // Rest (z. B. während des Löschens neu gescannt) und der Ordner selbst
        catalog.locked().remove_root(id)
    }

    /// Ordner mit eigener Einstellung „Unterordner einbeziehen“
    pub fn folder_options(&self) -> Result<Vec<(String, bool)>> {
        let mut st = self
            .conn
            .prepare("SELECT path, subfolders FROM folder_options")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// „Unterordner einbeziehen“ für einen Ordner setzen
    pub fn set_folder_subfolders(&self, path: &Path, include: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO folder_options (path, subfolders) VALUES (?1, ?2)
             ON CONFLICT(path) DO UPDATE SET subfolders = ?2",
            params![paths::normalize_dir(path), include],
        )?;
        Ok(())
    }

    pub fn roots(&self) -> Result<Vec<Root>> {
        let mut st = self.conn.prepare(
            "SELECT r.id, r.path, r.last_scan,
                    (SELECT COUNT(*) FROM images i WHERE i.root_id = r.id AND i.missing = 0)
             FROM roots r ORDER BY r.path",
        )?;
        let rows = st.query_map([], root_from_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn root(&self, id: i64) -> Result<Root> {
        self.conn
            .query_row(
                "SELECT r.id, r.path, r.last_scan,
                        (SELECT COUNT(*) FROM images i WHERE i.root_id = r.id AND i.missing = 0)
                 FROM roots r WHERE r.id = ?1",
                [id],
                root_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(tr!("Quellordner {id}", "Source folder {id}")))
    }

    /// Ordnerbaum eines Quellordners: Ordner mit Bildern und (seit v8) alle
    /// übrigen Ordner von der Platte, auch leere.
    pub fn folder_tree(&self, root_id: i64) -> Result<FolderNode> {
        let root = self.root(root_id)?;
        let mut st = self.conn.prepare(
            "SELECT dir, COUNT(*) FROM images WHERE root_id = ?1 AND missing = 0 GROUP BY dir",
        )?;
        let counts: Vec<(String, i64)> = st
            .query_map([root_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;

        // Pfad -> direkte Anzahl. Zwischenordner ohne Bilder werden mit 0 ergänzt.
        let mut nodes: BTreeMap<String, i64> = BTreeMap::new();
        nodes.insert(root.path.clone(), 0);
        // Ordner von der Platte (auch leere) mit 0 Fotos
        let folders: Vec<String> = self
            .conn
            .prepare_cached("SELECT path FROM folders WHERE root_id = ?1")?
            .query_map([root_id], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let counts = counts
            .into_iter()
            .chain(folders.into_iter().map(|f| (f, 0)));
        for (dir, n) in counts {
            if !paths::is_within(&dir, &root.path) {
                continue;
            }
            *nodes.entry(dir.clone()).or_default() += n;
            let mut cur = Path::new(&dir)
                .parent()
                .map(|p| p.to_string_lossy().into_owned());
            while let Some(p) = cur {
                if !paths::is_within(&p, &root.path) || nodes.contains_key(&p) {
                    break;
                }
                nodes.insert(p.clone(), 0);
                cur = Path::new(&p)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned());
            }
        }
        Ok(build_tree(&root.path, &root.name, &nodes))
    }

    /// Ordner eines Quellordners nach einem vollständigen Scan ersetzen.
    /// `true`, wenn sich etwas geändert hat.
    pub(crate) fn set_folders(&mut self, root_id: i64, dirs: &[String]) -> Result<bool> {
        let before: std::collections::HashSet<String> = self
            .conn
            .prepare("SELECT path FROM folders WHERE root_id = ?1")?
            .query_map([root_id], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let after: std::collections::HashSet<String> = dirs.iter().cloned().collect();
        if before == after {
            return Ok(false);
        }
        self.with_tx(|tx| {
            tx.execute("DELETE FROM folders WHERE root_id = ?1", [root_id])?;
            let mut st =
                tx.prepare("INSERT OR IGNORE INTO folders (root_id, path) VALUES (?1, ?2)")?;
            for d in &after {
                st.execute(params![root_id, d])?;
            }
            Ok(())
        })?;
        Ok(true)
    }

    /// Ordner nach gemeldeten Änderungen: `added` neu bzw. noch vorhanden,
    /// `gone` samt allem darunter entfernt. `true`, wenn sich etwas geändert hat.
    pub(crate) fn update_folders(
        &mut self,
        root_id: i64,
        added: &[String],
        gone: &[String],
    ) -> Result<bool> {
        if added.is_empty() && gone.is_empty() {
            return Ok(false);
        }
        self.with_tx(|tx| {
            let mut n = 0;
            for g in gone {
                // Der Ordner selbst und alles darunter (Bereichsabfrage wie beim Ordnerfilter)
                let g = paths::trim_dir(g);
                let (prefix, upper) = subtree_range(&g);
                n += tx.execute(
                    "DELETE FROM folders WHERE root_id = ?1 AND (path = ?2 OR (path >= ?3 AND path < ?4))",
                    params![root_id, g, prefix, upper],
                )?;
            }
            let mut st = tx.prepare("INSERT OR IGNORE INTO folders (root_id, path) VALUES (?1, ?2)")?;
            for d in added {
                n += st.execute(params![root_id, d])?;
            }
            Ok(n > 0)
        })
    }
}
