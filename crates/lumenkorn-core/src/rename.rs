//! Stapel-Umbenennung von Fotodateien: Muster ausfüllen, Konflikte erkennen,
//! Umbenennungen planen (auch Tausch und Ketten) und sicher ausführen.
//!
//! Die Prüfung läuft vollständig hier im Backend; die Oberfläche zeigt nur
//! die Vorschau. Es wird nie eine vorhandene Datei überschrieben.

use crate::catalog::RenameInfo;
use crate::geotime::parse_time;
use crate::timeshift::format_time;
use lumenkorn_develop::i18n::tr_s;
use lumenkorn_develop::tr;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Einstellungen der Umbenennung.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    /// Muster mit Platzhaltern wie `{yyyy}` oder `{n}`
    pub pattern: String,
    /// Erste laufende Nummer
    pub start: i64,
    /// Stellen der laufenden Nummer (mit Nullen aufgefüllt)
    pub digits: usize,
    /// `true`: nach Aufnahmezeit nummerieren, sonst in der übergebenen Reihenfolge
    pub by_capture: bool,
}

/// Ein Foto in der Vorschau.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: i64,
    pub old: String,
    pub new: String,
    /// Grund, warum dieses Foto nicht umbenannt werden kann
    pub conflict: Option<String>,
}

/// Eine Datei, die umbenannt wird (Foto oder Begleitdatei).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub src: PathBuf,
    pub dst: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub items: Vec<Item>,
    pub moves: Vec<Move>,
    /// Neuer Pfad je Foto, dessen Name sich ändert: (id, Pfad, Dateiname)
    pub changes: Vec<(i64, String, String)>,
}

impl Plan {
    pub fn has_conflict(&self) -> bool {
        self.items.iter().any(|i| i.conflict.is_some())
    }
}

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

fn bad_char(c: char) -> bool {
    c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
}

/// Werte aus Metadaten dürfen keine ungültigen Zeichen einschleppen.
fn clean_value(s: &str) -> String {
    s.chars()
        .map(|c| if bad_char(c) { '-' } else { c })
        .collect()
}

/// Warum ist dieser Name (ohne Endung) unzulässig? Regeln von Windows, damit
/// der Ordner auch dort lesbar bleibt.
pub fn invalid_reason(stem: &str, ext_part: &str) -> Option<String> {
    if stem.trim().is_empty() {
        return Some(tr_s("Der Name ist leer", "The name is empty").into());
    }
    if let Some(c) = stem.chars().find(|c| bad_char(*c)) {
        let shown = if c.is_control() {
            String::new()
        } else {
            format!(" ({c})")
        };
        return Some(tr!(
            "Ungültiges Zeichen im Namen{shown}",
            "Invalid character in the name{shown}"
        ));
    }
    if stem.ends_with('.') || stem.ends_with(' ') {
        return Some(
            tr_s(
                "Der Name darf nicht mit Punkt oder Leerzeichen enden",
                "The name must not end with a dot or a space",
            )
            .into(),
        );
    }
    let first = stem.split('.').next().unwrap_or("").trim_end();
    if RESERVED.contains(&first.to_ascii_uppercase().as_str()) {
        return Some(tr!(
            "„{first}“ ist ein reservierter Name",
            "\"{first}\" is a reserved name"
        ));
    }
    if stem.chars().count() + ext_part.chars().count() > 255 {
        return Some(tr_s("Der Name ist zu lang", "The name is too long").into());
    }
    None
}

/// Zeitpunkt als `JJJJ-MM-TTThh:mm:ss` und Sekunden: Aufnahmezeit, sonst
/// Änderungszeit der Datei (UTC).
fn time_of(p: &RenameInfo) -> (String, i64) {
    if let Some(secs) = p.captured_at.as_deref().and_then(parse_time) {
        return (format_time(secs), secs);
    }
    (format_time(p.mtime), p.mtime)
}

/// Namensstamm und Endung (mit Punkt, Schreibweise wie im Original).
fn split_name(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// Setzt das Muster für ein Foto ein. `Err` bei unbekanntem Platzhalter.
fn expand(pattern: &str, p: &RenameInfo, number: i64, digits: usize) -> Result<String, String> {
    let (iso, _) = time_of(p);
    let (stem, _) = split_name(&p.file_name);
    let folder = Path::new(&p.dir)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find('}') else {
            return Err(tr_s(
                "Eine Klammer „{“ wird nicht geschlossen",
                "An opening brace \"{\" is not closed",
            )
            .into());
        };
        let shown = &after[..j];
        match shown.to_ascii_lowercase().as_str() {
            "name" => out.push_str(&clean_value(stem)),
            "yyyy" => out.push_str(&iso[0..4]),
            "mm" => out.push_str(&iso[5..7]),
            "dd" => out.push_str(&iso[8..10]),
            "hh" => out.push_str(&iso[11..13]),
            "min" => out.push_str(&iso[14..16]),
            "ss" => out.push_str(&iso[17..19]),
            "n" => out.push_str(&format!("{number:0digits$}")),
            "camera" => out.push_str(&clean_value(p.camera.as_deref().unwrap_or(""))),
            "lens" => out.push_str(&clean_value(p.lens.as_deref().unwrap_or(""))),
            "folder" => out.push_str(&clean_value(&folder)),
            _ => {
                return Err(tr!(
                    "Unbekannter Platzhalter {{{shown}}}",
                    "Unknown placeholder {{{shown}}}"
                ));
            }
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

fn is_xmp(n: &str) -> bool {
    n.to_ascii_lowercase().ends_with(".xmp")
}

/// XMP-Begleitdateien von Fotos, die im selben Ordner liegen und denselben
/// Namensstamm haben (bei einem einzelnen Foto: nur dieses):
/// `foto.cr3.xmp` gehört immer dazu, `foto.xmp` nur, wenn kein anderes Foto
/// gleichen Namens daneben liegt (RAW+JPEG-Paare teilen sich diese Datei).
pub fn sidecars_of(photos: &[&Path]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(first) = photos.first() else {
        return out;
    };
    let names: HashSet<&str> = photos
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
        .collect();
    let Some(name) = first.file_name().and_then(|n| n.to_str()) else {
        return out;
    };
    let dir = first.parent().unwrap_or(Path::new("."));
    let stem = first.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let mut seen = HashSet::new();
    let entries: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    let sibling = entries.iter().any(|n| {
        !is_xmp(n)
            && !names.contains(n.as_str())
            && Path::new(n)
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s == stem)
    });
    for n in &entries {
        if !is_xmp(n) {
            continue;
        }
        let base = &n[..n.len() - 4];
        if names.contains(base) || (!sibling && base == stem) {
            // Groß-/Kleinschreibung: dieselbe Datei nur einmal
            if seen.insert(n.to_ascii_lowercase()) {
                out.push(dir.join(n));
            }
        }
    }
    out
}

/// Plant die Umbenennung. `photos` in der Reihenfolge der Auswahl.
/// Greift lesend auf das Dateisystem zu (Begleitdateien, Zielnamen).
pub fn plan(photos: &[RenameInfo], opts: &Options) -> Plan {
    let digits = opts.digits.clamp(1, 12);
    let start = opts.start.max(0);

    // Reihenfolge der Nummerierung
    let mut order: Vec<usize> = (0..photos.len()).collect();
    if opts.by_capture {
        order.sort_by_key(|&i| (time_of(&photos[i]).1, i));
    }
    // RAW+JPEG-Paare: gleicher Ordner und gleicher Namensstamm
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut group_of: HashMap<(String, String), usize> = HashMap::new();
    for &i in &order {
        let k = (
            photos[i].dir.clone(),
            split_name(&photos[i].file_name).0.to_string(),
        );
        let g = *group_of.entry(k).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[g].push(i);
    }

    let mut items: Vec<Item> = photos
        .iter()
        .map(|p| Item {
            id: p.id,
            old: p.file_name.clone(),
            new: p.file_name.clone(),
            conflict: None,
        })
        .collect();
    let mut new_paths: Vec<PathBuf> = photos.iter().map(|p| PathBuf::from(&p.path)).collect();
    // Begleitdateien gehören zu dem Foto, das sie bei der Prüfung vertritt
    let mut moves: Vec<(usize, Move)> = Vec::new();

    for (g, members) in groups.iter().enumerate() {
        let lead = &photos[members[0]];
        let number = start + g as i64;
        let stem = expand(&opts.pattern, lead, number, digits);
        let problem = match &stem {
            Err(e) => Some(e.clone()),
            Ok(s) => invalid_reason(s, split_name(&lead.file_name).1),
        };
        let new_stem = stem.unwrap_or_default();
        let dir = Path::new(&lead.dir);
        for &i in members {
            let ext = split_name(&photos[i].file_name).1;
            if problem.is_none() {
                let new_name = format!("{new_stem}{ext}");
                new_paths[i] = dir.join(&new_name);
                items[i].new = new_name;
            }
            items[i].conflict = if photos[i].missing {
                Some(
                    tr_s(
                        "Das Foto fehlt und kann nicht umbenannt werden",
                        "The photo is missing and cannot be renamed",
                    )
                    .into(),
                )
            } else if !exists(Path::new(&photos[i].path)) {
                Some(tr_s("Die Datei wurde nicht gefunden", "The file was not found").into())
            } else {
                problem.clone()
            };
        }
        if problem.is_some() {
            continue;
        }
        // Begleitdateien der vorhandenen Fotos der Gruppe
        let present: Vec<usize> = members
            .iter()
            .copied()
            .filter(|&i| !photos[i].missing)
            .collect();
        let paths: Vec<&Path> = present
            .iter()
            .map(|&i| Path::new(&photos[i].path))
            .collect();
        for side in sidecars_of(&paths) {
            let n = side.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let base = &n[..n.len() - 4];
            let suffix = &n[base.len()..];
            let (owner, target) = match present
                .iter()
                .copied()
                .find(|&i| photos[i].file_name == base)
            {
                Some(i) => (i, format!("{}{suffix}", items[i].new)),
                None => (present[0], format!("{new_stem}{suffix}")),
            };
            moves.push((
                owner,
                Move {
                    dst: dir.join(target),
                    src: side,
                },
            ));
        }
    }
    for i in 0..photos.len() {
        moves.push((
            i,
            Move {
                src: PathBuf::from(&photos[i].path),
                dst: new_paths[i].clone(),
            },
        ));
    }
    // Unveränderte Dateien nicht anfassen
    moves.retain(|(_, m)| m.src != m.dst);

    // Zielnamen, die mehrere Fotos belegen würden
    let mut targets: HashMap<String, HashSet<usize>> = HashMap::new();
    for (i, p) in new_paths.iter().enumerate() {
        targets.entry(key(p)).or_default().insert(i);
    }
    for (i, m) in &moves {
        targets.entry(key(&m.dst)).or_default().insert(*i);
    }
    for owners in targets.values().filter(|o| o.len() > 1) {
        for &i in owners {
            items[i].conflict.get_or_insert_with(|| {
                tr_s(
                    "Ein anderes Foto bekäme denselben Namen",
                    "Another photo would get the same name",
                )
                .into()
            });
        }
    }
    // Ziele, die es schon gibt und die nicht von dieser Umbenennung frei werden
    let vacated: HashSet<String> = moves.iter().map(|(_, m)| key(&m.src)).collect();
    for (i, m) in &moves {
        if items[*i].conflict.is_none() && exists(&m.dst) && !vacated.contains(&key(&m.dst)) {
            let n = m
                .dst
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            items[*i].conflict = Some(tr!(
                "„{n}“ gibt es schon im Ordner",
                "\"{n}\" already exists in the folder"
            ));
        }
    }

    let changes = photos
        .iter()
        .enumerate()
        .filter(|(i, p)| items[*i].new != p.file_name)
        .map(|(i, p)| {
            (
                p.id,
                new_paths[i].to_string_lossy().into_owned(),
                items[i].new.clone(),
            )
        })
        .collect();
    Plan {
        items,
        moves: moves.into_iter().map(|(_, m)| m).collect(),
        changes,
    }
}

/// Verschiebt, ohne ein vorhandenes Ziel zu überschreiben.
fn move_no_clobber(src: &Path, dst: &Path) -> std::io::Result<()> {
    // Ein harter Link schlägt fehl, wenn das Ziel existiert – atomar
    match std::fs::hard_link(src, dst) {
        Ok(()) => std::fs::remove_file(src),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
        // Dateisysteme ohne harte Links (FAT32, exFAT): prüfen und umbenennen
        Err(_) => {
            if exists(dst) {
                return Err(std::io::ErrorKind::AlreadyExists.into());
            }
            std::fs::rename(src, dst)
        }
    }
}

/// Führt die Umbenennungen aus. Liegt ein Ziel auf einem Namen, den eine
/// andere Datei erst freigibt (Tausch, Kette), geht es über einen
/// Zwischennamen im selben Ordner. Bei einem Fehler wird zurückgerollt.
pub fn execute(moves: &[Move]) -> Result<(), String> {
    execute_with(moves, &mut move_no_clobber)
}

type RenameFn<'a> = &'a mut dyn FnMut(&Path, &Path) -> std::io::Result<()>;

fn execute_with(moves: &[Move], rename: RenameFn) -> Result<(), String> {
    let sources: HashSet<String> = moves.iter().map(|m| key(&m.src)).collect();
    let mut direct = Vec::new();
    let mut staged = Vec::new();
    for (i, m) in moves.iter().enumerate() {
        if sources.contains(&key(&m.dst)) {
            let dir = m.src.parent().unwrap_or(Path::new("."));
            let tmp = dir.join(format!(".lumenkorn-rename-{}-{i}.tmp", std::process::id()));
            staged.push((m, tmp));
        } else {
            direct.push(m);
        }
    }
    // Schon ausgeführte Schritte (von, nach) für das Zurückrollen
    let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut step = |from: &Path, to: &Path| -> Result<(), String> {
        rename(from, to).map_err(|e| {
            tr!(
                "{} konnte nicht umbenannt werden: {}.",
                "Could not rename {}: {}.",
                from.display(),
                e
            )
        })?;
        done.push((from.to_path_buf(), to.to_path_buf()));
        Ok(())
    };
    let mut run = || -> Result<(), String> {
        for (m, tmp) in &staged {
            step(&m.src, tmp)?;
        }
        for m in &direct {
            step(&m.src, &m.dst)?;
        }
        for (m, tmp) in &staged {
            step(tmp, &m.dst)?;
        }
        Ok(())
    };
    let result = run();
    let Err(e) = result else { return Ok(()) };
    let mut stuck = Vec::new();
    for (from, to) in done.iter().rev() {
        if std::fs::rename(to, from).is_err() {
            stuck.push(to.display().to_string());
        }
    }
    Err(if stuck.is_empty() {
        format!(
            "{e} {}",
            tr_s("Es wurde nichts umbenannt.", "Nothing was renamed.")
        )
    } else {
        format!(
            "{e} {} {}",
            tr_s(
                "Zurücksetzen nicht vollständig, diese Dateien tragen noch den neuen Namen:",
                "Could not undo everything, these files still have the new name:"
            ),
            stuck.join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn info(id: i64, dir: &Path, name: &str, at: Option<&str>) -> RenameInfo {
        RenameInfo {
            id,
            path: dir.join(name).to_string_lossy().into_owned(),
            dir: dir.to_string_lossy().into_owned(),
            file_name: name.into(),
            captured_at: at.map(Into::into),
            camera: Some("Canon/EOS".into()),
            lens: None,
            mtime: 0,
            missing: false,
        }
    }

    fn opts(pattern: &str) -> Options {
        Options {
            pattern: pattern.into(),
            start: 1,
            digits: 3,
            by_capture: true,
        }
    }

    fn touch(dir: &Path, names: &[&str]) {
        for n in names {
            fs::write(dir.join(n), n).unwrap();
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        v.sort();
        v
    }

    fn text(dir: &Path, name: &str) -> String {
        fs::read_to_string(dir.join(name)).unwrap()
    }

    #[test]
    fn expands_tokens_and_cleans_values() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), &["IMG_1.JPG"]);
        let p = info(1, d.path(), "IMG_1.JPG", Some("2024-06-01T14:03:22"));
        let o = opts("{yyyy}{mm}{dd}_{hh}{min}{ss}_{n}_{name}_{camera}");
        let plan = plan(&[p], &o);
        assert_eq!(plan.items[0].new, "20240601_140322_001_IMG_1_Canon-EOS.JPG");
        assert!(plan.items[0].conflict.is_none());
        assert_eq!(plan.changes.len(), 1);
    }

    #[test]
    fn falls_back_to_mtime_and_numbers_by_capture() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), &["a.jpg", "b.jpg"]);
        let mut a = info(1, d.path(), "a.jpg", Some("2024-06-01T10:00:00"));
        let mut b = info(2, d.path(), "b.jpg", None);
        b.mtime = 86_400 * 365; // 1971
        a.mtime = 5;
        let mut o = opts("{yyyy}-{n}");
        let p = plan(&[a.clone(), b.clone()], &o);
        assert_eq!(p.items[0].new, "2024-002.jpg");
        assert_eq!(p.items[1].new, "1971-001.jpg");
        o.by_capture = false;
        o.start = 10;
        let p = plan(&[a, b], &o);
        assert_eq!(p.items[0].new, "2024-010.jpg");
        assert_eq!(p.items[1].new, "1971-011.jpg");
    }

    #[test]
    fn pairs_share_name_number_and_sidecars() {
        let d = tempfile::tempdir().unwrap();
        touch(
            d.path(),
            &["a.arw", "a.jpg", "a.xmp", "a.arw.xmp", "z.cr3", "z.xmp"],
        );
        let ps = [
            info(1, d.path(), "a.arw", Some("2024-01-01T10:00:00")),
            info(2, d.path(), "a.jpg", Some("2024-01-01T10:00:00")),
            info(3, d.path(), "z.cr3", Some("2024-01-02T10:00:00")),
        ];
        let p = plan(&ps, &opts("x{n}"));
        assert!(!p.has_conflict());
        assert_eq!(p.items[0].new, "x001.arw");
        assert_eq!(p.items[1].new, "x001.jpg");
        assert_eq!(p.items[2].new, "x002.cr3");
        execute(&p.moves).unwrap();
        assert_eq!(
            names(d.path()),
            [
                "x001.arw",
                "x001.arw.xmp",
                "x001.jpg",
                "x001.xmp",
                "x002.cr3",
                "x002.xmp"
            ]
        );
    }

    #[test]
    fn shared_xmp_stays_with_unselected_sibling() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), &["a.arw", "a.jpg", "a.xmp", "a.arw.xmp"]);
        let ps = [info(1, d.path(), "a.arw", None)];
        let p = plan(&ps, &opts("n"));
        assert!(!p.has_conflict(), "{:?}", p.items);
        execute(&p.moves).unwrap();
        assert_eq!(names(d.path()), ["a.jpg", "a.xmp", "n.arw", "n.arw.xmp"]);
    }

    #[test]
    fn detects_conflicts() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), &["a.jpg", "b.jpg", "c.jpg", "taken.jpg"]);
        let ps = [
            info(1, d.path(), "a.jpg", Some("2024-01-01T10:00:00")),
            info(2, d.path(), "b.jpg", Some("2024-01-01T10:00:00")),
        ];
        // Gleicher Name für verschiedene Fotos (Nummer fehlt)
        let p = plan(&ps, &opts("same"));
        assert!(p.items.iter().all(|i| i.conflict.is_some()));
        // Vorhandene Datei, die nicht Teil der Umbenennung ist
        let p = plan(&ps[..1], &opts("taken"));
        assert!(p.items[0].conflict.as_ref().unwrap().contains("taken.jpg"));
        for bad in [
            "a:b", "a|b", "CON", "nul.x", "x.", "x ", "", "{bogus}", "{n",
        ] {
            let p = plan(&ps[..1], &opts(bad));
            assert!(p.items[0].conflict.is_some(), "{bad}");
        }
        let mut gone = ps[0].clone();
        gone.missing = true;
        assert!(plan(&[gone], &opts("ok")).items[0].conflict.is_some());
        let nofile = info(9, d.path(), "nope.jpg", None);
        assert!(plan(&[nofile], &opts("ok")).items[0].conflict.is_some());
    }

    #[test]
    fn swap_goes_through_temp_names() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), &["a.jpg", "b.jpg"]);
        let moves = vec![
            Move {
                src: d.path().join("a.jpg"),
                dst: d.path().join("b.jpg"),
            },
            Move {
                src: d.path().join("b.jpg"),
                dst: d.path().join("a.jpg"),
            },
        ];
        execute(&moves).unwrap();
        assert_eq!(text(d.path(), "a.jpg"), "b.jpg");
        assert_eq!(text(d.path(), "b.jpg"), "a.jpg");
        assert_eq!(names(d.path()), ["a.jpg", "b.jpg"]);
    }

    #[test]
    fn chain_into_names_of_selected_photos_is_allowed() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), &["001.jpg", "002.jpg"]);
        let ps = [
            info(1, d.path(), "001.jpg", Some("2024-01-01T10:00:00")),
            info(2, d.path(), "002.jpg", Some("2024-01-01T10:00:01")),
        ];
        let mut o = opts("{n}");
        o.start = 2;
        let p = plan(&ps, &o);
        assert!(!p.has_conflict(), "{:?}", p.items);
        execute(&p.moves).unwrap();
        assert_eq!(text(d.path(), "002.jpg"), "001.jpg");
        assert_eq!(text(d.path(), "003.jpg"), "002.jpg");
        assert_eq!(names(d.path()), ["002.jpg", "003.jpg"]);
    }

    #[test]
    fn case_only_change_is_allowed() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), &["abc.jpg"]);
        let p = plan(&[info(1, d.path(), "abc.jpg", None)], &opts("ABC"));
        assert!(!p.has_conflict());
        execute(&p.moves).unwrap();
        assert_eq!(names(d.path()), ["ABC.jpg"]);
    }

    #[test]
    fn rolls_back_on_error() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), &["a.jpg", "b.jpg", "c.jpg"]);
        let moves: Vec<Move> = ["a", "b", "c"]
            .iter()
            .map(|n| Move {
                src: d.path().join(format!("{n}.jpg")),
                dst: d.path().join(format!("new_{n}.jpg")),
            })
            .collect();
        let mut calls = 0;
        let r = execute_with(&moves, &mut |from, to| {
            calls += 1;
            if calls == 3 {
                return Err(std::io::Error::other("kaputt"));
            }
            std::fs::rename(from, to)
        });
        assert!(r.unwrap_err().contains("kaputt"));
        assert_eq!(names(d.path()), ["a.jpg", "b.jpg", "c.jpg"]);
        assert_eq!(text(d.path(), "a.jpg"), "a.jpg");
    }

    #[test]
    fn never_overwrites() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), &["a.jpg", "b.jpg"]);
        let r = execute(&[Move {
            src: d.path().join("a.jpg"),
            dst: d.path().join("b.jpg"),
        }]);
        assert!(r.is_err());
        assert_eq!(text(d.path(), "b.jpg"), "b.jpg");
        assert_eq!(names(d.path()), ["a.jpg", "b.jpg"]);
    }
}
