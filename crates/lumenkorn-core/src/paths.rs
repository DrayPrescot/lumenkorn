//! Pfad-Normalisierung, damit Ordnervergleiche in SQL zuverlässig sind.

use std::path::{MAIN_SEPARATOR, Path, PathBuf};

/// Trennzeichen, wie es in der Datenbank verwendet wird.
pub const SEP: char = MAIN_SEPARATOR;

/// Normalisiert einen Ordnerpfad: absolut machen (sofern möglich),
/// abschließende Trenner entfernen (außer bei Laufwerkswurzeln wie `/` oder `C:\`).
pub fn normalize_dir(p: &Path) -> String {
    let abs: PathBuf = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let mut s = abs.to_string_lossy().into_owned();

    // Windows: canonicalize liefert `\\?\C:\…` – für Anzeige und Vergleich störend.
    if let Some(rest) = s.strip_prefix(r"\\?\")
        && !rest.starts_with("UNC")
    {
        s = rest.to_string();
    }
    trim_dir(&s)
}

/// Wie [`normalize_dir`], aber ohne Dateisystemzugriff – für Pfade, die schon
/// normiert aus dem Katalog stammen. (`canonicalize` kann auf einem schlafenden
/// Netzlaufwerk Sekunden blockieren, während die Katalogsperre gehalten wird.)
pub(crate) fn trim_dir(p: &str) -> String {
    let mut s = p.to_string();
    while s.len() > 1 && s.ends_with(SEP) && !is_drive_root(&s) {
        s.pop();
    }
    s
}

fn is_drive_root(s: &str) -> bool {
    // "C:\" bzw. "/"
    s == "/" || (s.len() == 3 && s.as_bytes()[1] == b':' && s.ends_with(SEP))
}

/// Präfix, das ein Unterordner von `dir` in der DB haben muss.
pub(crate) fn child_prefix(dir: &str) -> String {
    if dir.ends_with(SEP) {
        dir.to_string()
    } else {
        format!("{dir}{SEP}")
    }
}

/// Liegt `path` in `dir` (oder ist es `dir` selbst)?
pub fn is_within(path: &str, dir: &str) -> bool {
    path == dir || path.starts_with(&child_prefix(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_trailing_separator() {
        let d = tempfile::tempdir().unwrap();
        let with = format!("{}{SEP}", d.path().display());
        assert_eq!(normalize_dir(Path::new(&with)), normalize_dir(d.path()));
        assert!(!normalize_dir(d.path()).ends_with(SEP));
    }

    #[test]
    fn within() {
        let a = format!("{SEP}fotos");
        let b = format!("{SEP}fotos{SEP}2024");
        let c = format!("{SEP}fotos-alt");
        assert!(is_within(&b, &a));
        assert!(is_within(&a, &a));
        assert!(!is_within(&c, &a));
    }
}
