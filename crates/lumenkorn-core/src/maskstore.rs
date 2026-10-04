//! Ablage der KI-Masken als 8-Bit-PNG im Datenordner, z. B.
//! `<daten>/masken/42-sky-18f3a.png`.
//!
//! Masken sind Teil einer Bearbeitung, aber teuer zu berechnen. Deshalb liegt
//! das Pixelergebnis als Datei vor, während die Bearbeitung nur die ID und die
//! Eingaben (Klickpunkte) speichert.

use crate::error::{Error, Result};
use lumenkorn_develop::{GrayMask, MaskSource};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const CACHE: usize = 12;

#[derive(Debug)]
pub struct MaskFiles {
    dir: PathBuf,
    cache: Mutex<Vec<(String, Arc<GrayMask>)>>,
}

impl MaskFiles {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            cache: Mutex::default(),
        }
    }

    /// Zwischenspeicher leeren (Dateien bleiben).
    pub fn clear_cache(&self) {
        self.cache.lock().unwrap().clear();
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn valid(id: &str) -> bool {
        !id.is_empty()
            && id.len() < 100
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    }

    pub fn path(&self, id: &str) -> Option<PathBuf> {
        Self::valid(id).then(|| self.dir.join(format!("{id}.png")))
    }

    /// Neue, eindeutige Masken-ID.
    pub fn new_id(image_id: i64, kind: &str) -> String {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros())
            .unwrap_or(0);
        let kind: String = kind
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(12)
            .collect();
        format!("{image_id}-{kind}-{t:x}")
    }

    /// Speichert (oder ersetzt) eine Maske.
    pub fn put(&self, id: &str, mask: &image::GrayImage) -> Result<()> {
        let path = self.path(id).ok_or_else(|| {
            Error::Invalid(lumenkorn_develop::tr!(
                "ungültige Masken-ID {id}",
                "invalid mask ID {id}"
            ))
        })?;
        std::fs::create_dir_all(&self.dir)?;
        let tmp = path.with_extension("tmp.png");
        mask.save(&tmp)?;
        std::fs::rename(&tmp, &path)?;
        let mut c = self.cache.lock().unwrap();
        c.retain(|(k, _)| k != id);
        c.insert(0, (id.to_string(), Arc::new(GrayMask::from_image(mask))));
        c.truncate(CACHE);
        Ok(())
    }
}

impl MaskSource for MaskFiles {
    fn get(&self, id: &str) -> Option<Arc<GrayMask>> {
        {
            let mut c = self.cache.lock().unwrap();
            if let Some(pos) = c.iter().position(|(k, _)| k == id) {
                let e = c.remove(pos);
                let m = e.1.clone();
                c.insert(0, e);
                return Some(m);
            }
        }
        let img = image::open(self.path(id)?).ok()?.to_luma8();
        let m = Arc::new(GrayMask::from_image(&img));
        let mut c = self.cache.lock().unwrap();
        c.insert(0, (id.to_string(), m.clone()));
        c.truncate(CACHE);
        Some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_rejects_unsafe_ids() {
        let d = tempfile::tempdir().unwrap();
        let store = MaskFiles::new(d.path().join("masken"));
        let id = MaskFiles::new_id(7, "sky");
        let img =
            image::GrayImage::from_fn(8, 4, |x, _| image::Luma([if x < 4 { 255 } else { 0 }]));
        store.put(&id, &img).unwrap();
        let fresh = MaskFiles::new(d.path().join("masken"));
        let m = fresh.get(&id).unwrap();
        assert!(m.sample(0.1, 0.5) > 0.99 && m.sample(0.9, 0.5) < 0.01);
        assert!(store.put("../x", &img).is_err());
        assert!(fresh.get("../../etc/passwd").is_none());
    }
}
