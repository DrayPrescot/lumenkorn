//! Dateien sicher schreiben und kleine JSON-Dateien (Einstellungen, Gelerntes)
//! laden und speichern. Liegt in der untersten Crate, damit Kern, KI und App
//! dieselben Helfer nutzen.
//!
//! Geschrieben wird immer erst in eine temporäre Datei daneben, die dann die
//! Zieldatei ersetzt: Ein Absturz mitten im Schreiben hinterlässt die alte
//! oder die neue Datei, nie eine halbe.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Temporärer Name neben `path`, eindeutig je Prozess und Aufruf (gleichzeitige
/// Schreiber kommen sich nicht in die Quere)
fn tmp_path(path: &Path) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(
        ".{}-{}.tmp",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    path.with_file_name(name)
}

/// `write` schreibt in die übergebene temporäre Datei; danach ersetzt sie
/// `path`. Bei einem Fehler wird die temporäre Datei entfernt.
pub fn write_atomic_with(
    path: &Path,
    write: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let tmp = tmp_path(path);
    let r = write(&tmp).and_then(|_| std::fs::rename(&tmp, path));
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// `bytes` nach `path` schreiben (atomar, siehe oben).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, |tmp| std::fs::write(tmp, bytes))
}

/// JSON-Datei lesen. Fehlt sie, gilt der Standard. Ist sie unlesbar oder
/// kaputt, ebenfalls – aber mit Eintrag im Log, und die kaputte Datei bleibt
/// als `….bad` liegen, damit beim nächsten Speichern nichts unbemerkt verloren
/// geht.
pub fn load_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return T::default(),
        Err(e) => {
            log::warn!("{} nicht lesbar: {e}", path.display());
            return T::default();
        }
    };
    match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            let mut bad = path.as_os_str().to_os_string();
            bad.push(".bad");
            log::warn!(
                "{} ist beschädigt ({e}), Standardwerte gelten; Kopie: {}",
                path.display(),
                Path::new(&bad).display()
            );
            let _ = std::fs::copy(path, &bad);
            T::default()
        }
    }
}

/// Als lesbares JSON speichern (atomar); legt den Ordner bei Bedarf an.
pub fn save_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<()> {
    let json = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_atomic(path, json.as_bytes())
}

/// Sekunden seit 1970 (UTC)
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Conf {
        name: String,
        n: u32,
    }

    #[test]
    fn json_round_trip_and_damaged_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("conf.json");
        assert_eq!(load_json_or_default::<Conf>(&p), Conf::default());
        let c = Conf {
            name: "a".into(),
            n: 3,
        };
        save_json(&p, &c).unwrap();
        assert_eq!(load_json_or_default::<Conf>(&p), c);
        // Keine temporären Dateien bleiben liegen
        let left: Vec<_> = std::fs::read_dir(p.parent().unwrap()).unwrap().collect();
        assert_eq!(left.len(), 1);
        // Kaputte Datei: Standard, Kopie bleibt
        std::fs::write(&p, "{ kaputt").unwrap();
        assert_eq!(load_json_or_default::<Conf>(&p), Conf::default());
        assert!(dir.path().join("sub").join("conf.json.bad").exists());
    }

    #[test]
    fn failed_write_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.bin");
        let r = write_atomic_with(&p, |tmp| {
            std::fs::write(tmp, b"halb")?;
            Err(io::Error::other("abgebrochen"))
        });
        assert!(r.is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
