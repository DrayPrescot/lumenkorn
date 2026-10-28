//! Projekte.

use super::*;

impl Catalog {
    // ------------------------------------------------------------------
    // Projekte
    // ------------------------------------------------------------------

    pub fn create_project(&self, name: &str, description: &str) -> Result<Project> {
        let name = project_name(name)?;
        let t = now();
        self.conn.execute(
            "INSERT INTO projects (name, description, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![name, description, t],
        )?;
        self.project(self.conn.last_insert_rowid())
    }

    pub fn update_project(&self, id: i64, name: &str, description: &str) -> Result<Project> {
        let name = project_name(name)?;
        let n = self.conn.execute(
            "UPDATE projects SET name = ?2, description = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, name, description, now()],
        )?;
        if n == 0 {
            return Err(Error::NotFound(tr!("Projekt {id}", "Project {id}")));
        }
        self.project(id)
    }

    pub fn delete_project(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM projects WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Fügt einem Projekt einen Ordner hinzu. Der Ordner kann in einem beliebigen
    /// Quellordner liegen; mehrere Ordner aus verschiedenen Orten sind erlaubt.
    pub fn add_project_folder(
        &self,
        project_id: i64,
        path: &Path,
        recursive: bool,
    ) -> Result<Project> {
        let p = paths::normalize_dir(path);
        self.project(project_id)?;
        self.conn.execute(
            "INSERT INTO project_folders (project_id, path, recursive) VALUES (?1, ?2, ?3)
             ON CONFLICT (project_id, path) DO UPDATE SET recursive = excluded.recursive",
            params![project_id, p, recursive],
        )?;
        self.touch_project(project_id)?;
        self.project(project_id)
    }

    pub fn remove_project_folder(&self, project_id: i64, path: &str) -> Result<Project> {
        self.conn.execute(
            "DELETE FROM project_folders WHERE project_id = ?1 AND path = ?2",
            params![project_id, path],
        )?;
        self.touch_project(project_id)?;
        self.project(project_id)
    }

    fn touch_project(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET updated_at = ?2 WHERE id = ?1",
            params![id, now()],
        )?;
        Ok(())
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        let ids: Vec<i64> = {
            let mut st = self
                .conn
                .prepare("SELECT id FROM projects ORDER BY name COLLATE NOCASE")?;
            st.query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        ids.into_iter().map(|id| self.project(id)).collect()
    }

    pub fn project(&self, id: i64) -> Result<Project> {
        let mut p = self
            .conn
            .query_row(
                "SELECT id, name, description, created_at, updated_at FROM projects WHERE id = ?1",
                [id],
                |r| {
                    Ok(Project {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        description: r.get(2)?,
                        folders: vec![],
                        image_count: 0,
                        created_at: r.get(3)?,
                        updated_at: r.get(4)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(tr!("Projekt {id}", "Project {id}")))?;

        let mut st = self.conn.prepare(
            "SELECT path, recursive FROM project_folders WHERE project_id = ?1 ORDER BY path",
        )?;
        p.folders = st
            .query_map([id], |r| {
                Ok(ProjectFolder {
                    path: r.get(0)?,
                    recursive: r.get(1)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        p.image_count = self.count_images(&Scope::Project { id })?;
        Ok(p)
    }
}
