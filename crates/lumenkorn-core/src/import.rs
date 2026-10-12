//! Fotos importieren: von einer Speicherkarte (oder einem beliebigen Ordner)
//! einsammeln, nach Aufnahmezeit umbenennen und in `Ziel\JJJJ\JJJJ.MM.TT[_Anlass]`
//! kopieren – als `JJJJMMTT_HHMMSS_Zähler.ext`.
//!
//! Der Zähler ist die Bildnummer aus dem Originalnamen ohne führende Nullen
//! (`DSC07869.ARW` → `…_7869.ARW`), sonst eine laufende Nummer des Imports.
//! Dateien mit gleichem Namensstamm im selben Ordner (RAW + JPEG, `.xmp`)
//! bilden eine Gruppe und bekommen denselben neuen Namen.
//!
//! Jede Kopie wird vor dem Umbenennen Byte für Byte gegen das Original
//! geprüft (BLAKE3 beim Kopieren, danach erneut von der Zieldatei). Erst dann
//! darf das Original auf Wunsch gelöscht werden.

use crate::LockExt;
use crate::catalog::Catalog;
use crate::error::{Error, Result};
use crate::formats::{self, ImageKind};
use crate::metadata;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use walkdir::WalkDir;

/// Eine Aufnahme auf der Karte: Hauptdatei (RAW vor JPEG) und alle Dateien
/// mit demselben Namensstamm im selben Ordner.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: usize,
    /// Alle Dateien der Gruppe, die Hauptdatei zuerst
    pub files: Vec<PathBuf>,
    pub kind: ImageKind,
    /// Namensstamm der Originale (`DSC07869`)
    pub stem: String,
    pub size: u64,
    /// ISO-8601 ohne Zeitzone (`2026-10-11T14:32:10`)
    pub captured_at: String,
    /// Aufnahmezeit fehlte im EXIF – ersatzweise Änderungszeit der Datei
    pub date_from_file: bool,
    pub orientation: Option<i64>,
    /// Liegt schon (mit gleichem Inhalt) im Katalog
    pub already: bool,
}

impl Candidate {
    pub fn main(&self) -> &Path {
        &self.files[0]
    }
}

/// Für die Oberfläche
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportItem {
    pub id: usize,
    pub name: String,
    /// Weitere Dateien der Gruppe (`DSC07869.JPG`, `DSC07869.xmp`)
    pub extra: Vec<String>,
    pub kind: ImageKind,
    pub size: u64,
    pub captured_at: String,
    pub date_from_file: bool,
    pub already: bool,
    /// Neuer Name der Hauptdatei (ohne Ordner)
    pub new_name: String,
}

impl Candidate {
    pub fn item(&self, counter: &str) -> ImportItem {
        let name = |p: &PathBuf| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        ImportItem {
            id: self.id,
            name: name(&self.files[0]),
            extra: self.files[1..].iter().map(name).collect(),
            kind: self.kind,
            size: self.size,
            captured_at: self.captured_at.clone(),
            date_from_file: self.date_from_file,
            already: self.already,
            new_name: renamed(
                &self.files[0],
                &self.stem,
                &new_stem(&self.captured_at, counter),
            ),
        }
    }
}

/// Sammelt die Fotos unter `source` ein, nach Aufnahmezeit sortiert.
/// `progress(n)`: Zahl der gelesenen Aufnahmen.
pub fn scan(
    source: &Path,
    catalog: &Mutex<Catalog>,
    progress: &(dyn Fn(usize) + Sync),
) -> Result<Vec<Candidate>> {
    if !source.is_dir() {
        return Err(Error::Invalid(lumenkorn_develop::tr!(
            "{} ist kein Ordner",
            "{} is not a folder",
            source.display()
        )));
    }
    // Gruppen: (Ordner, Stamm in Kleinbuchstaben) → Dateien
    let mut groups: BTreeMap<(PathBuf, String), Vec<PathBuf>> = BTreeMap::new();
    for e in WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // Versteckte und Systemordner der Karte überspringen
            e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.')
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let p = e.into_path();
        let Some(stem) = group_stem(&p) else { continue };
        let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
        groups
            .entry((dir, stem.to_lowercase()))
            .or_default()
            .push(p);
    }
    let done = std::sync::atomic::AtomicUsize::new(0);
    let mut out: Vec<Candidate> = groups
        .into_par_iter()
        .filter_map(|(_, files)| {
            let c = candidate(files)?;
            progress(done.fetch_add(1, Ordering::Relaxed) + 1);
            Some(c)
        })
        .collect();
    out.sort_by(|a, b| (a.captured_at.as_str(), a.main()).cmp(&(b.captured_at.as_str(), b.main())));
    // Schon importiert: gleicher Inhalt (Größe und Hash) irgendwo im Katalog
    let hashes: Vec<Option<(String, i64)>> = out
        .par_iter()
        .map(|c| {
            let size = std::fs::metadata(c.main()).ok()?.len() as i64;
            Some((metadata::quick_hash(c.main()).ok()?, size))
        })
        .collect();
    {
        let cat = catalog.locked();
        for (c, h) in out.iter_mut().zip(hashes) {
            c.already = h.is_some_and(|(hash, size)| cat.has_content(&hash, size).unwrap_or(false));
        }
    }
    for (i, c) in out.iter_mut().enumerate() {
        c.id = i;
    }
    Ok(out)
}

/// Gruppenstamm einer Datei: bei Bildern der Namensstamm, bei Begleitdateien
/// (`DSC1.xmp`, `DSC1.ARW.xmp`) der des zugehörigen Bildes. Andere Dateien
/// (Videos, Kameradatenbanken) gehören zu keiner Gruppe.
fn group_stem(p: &Path) -> Option<String> {
    let name = p.file_name()?.to_str()?;
    let lower = name.to_ascii_lowercase();
    if let Some(base) = lower.strip_suffix(".xmp") {
        let orig = &name[..base.len()];
        // `DSC1.ARW.xmp` → `DSC1`
        let stem = match Path::new(orig).extension() {
            Some(_) if formats::classify(Path::new(orig)).is_some() => {
                Path::new(orig).file_stem()?.to_str()?
            }
            _ => orig,
        };
        return Some(stem.to_string());
    }
    formats::classify(p)?;
    Some(p.file_stem()?.to_str()?.to_string())
}

fn candidate(mut files: Vec<PathBuf>) -> Option<Candidate> {
    // Hauptdatei: RAW vor Rasterbild; Begleitdateien ans Ende
    let rank = |p: &PathBuf| match formats::classify(p) {
        Some((_, ImageKind::Raw)) => 0,
        Some(_) => 1,
        None => 2,
    };
    files.sort_by_key(|p| (rank(p), p.clone()));
    let (_, kind) = formats::classify(&files[0])?;
    let stem = files[0].file_stem()?.to_str()?.to_string();
    let size = files
        .iter()
        .filter_map(|f| std::fs::metadata(f).ok())
        .map(|m| m.len())
        .sum();
    let (captured, orientation) = metadata::read_capture(&files[0]);
    let (captured_at, date_from_file) = match captured {
        Some(c) => (c, false),
        None => (mtime_iso(&files[0])?, true),
    };
    Some(Candidate {
        id: 0,
        files,
        kind,
        stem,
        size,
        captured_at,
        date_from_file,
        orientation,
        already: false,
    })
}

/// Änderungszeit als `JJJJ-MM-TTTHH:MM:SS` (UTC – ohne EXIF-Datum nur ein
/// Notbehelf; Kameras schreiben es immer).
fn mtime_iso(p: &Path) -> Option<String> {
    let secs = std::fs::metadata(p)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Tage seit 1970 → Kalenderdatum (Howard Hinnant, „civil_from_days“)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    ))
}

/// Bildnummer aus dem Namensstamm: die Ziffern am Ende ohne führende Nullen
/// (`DSC07869` → `7869`, `IMG_0042` → `42`). Mehr als sechs Ziffern sind
/// eher ein Zeitstempel (Handys) als eine Bildnummer → `None`.
pub fn counter_of(stem: &str) -> Option<String> {
    let digits: String = stem
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if digits.is_empty() || digits.len() > 6 {
        return None;
    }
    let trimmed = digits.trim_start_matches('0');
    Some(if trimmed.is_empty() { "0" } else { trimmed }.to_string())
}

/// Zähler für alle Aufnahmen: Bildnummer, sonst fortlaufend ab 1 (in
/// Reihenfolge der Aufnahmezeit).
pub fn counters(cands: &[Candidate]) -> Vec<String> {
    let mut next = 0;
    cands
        .iter()
        .map(|c| {
            counter_of(&c.stem).unwrap_or_else(|| {
                next += 1;
                next.to_string()
            })
        })
        .collect()
}

/// `2026-10-11T14:32:10` + `7869` → `20261011_143210_7869`
pub fn new_stem(captured_at: &str, counter: &str) -> String {
    let digits: String = captured_at.chars().filter(char::is_ascii_digit).collect();
    let (date, time) = digits.split_at(8.min(digits.len()));
    format!("{date}_{}_{counter}", &time[..6.min(time.len())])
}

/// Neuer Dateiname: der alte Stamm wird ersetzt, der Rest bleibt
/// (`DSC07869.ARW.xmp` → `20261011_143210_7869.ARW.xmp`).
fn renamed(file: &Path, old_stem: &str, new_stem: &str) -> String {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let rest = name.get(old_stem.len()..).unwrap_or("");
    format!("{new_stem}{rest}")
}

/// Zielordner einer Aufnahme: `Ziel\JJJJ\JJJJ.MM.TT[_Anlass]`
pub fn target_dir(target: &Path, captured_at: &str, event: &str) -> PathBuf {
    let (y, m, d) = (
        captured_at.get(0..4).unwrap_or("0000"),
        captured_at.get(5..7).unwrap_or("00"),
        captured_at.get(8..10).unwrap_or("00"),
    );
    let event = clean_event(event);
    let day = if event.is_empty() {
        format!("{y}.{m}.{d}")
    } else {
        format!("{y}.{m}.{d}_{event}")
    };
    target.join(y).join(day)
}

/// Anlass als Teil eines Ordnernamens: ohne Zeichen, die Windows verbietet,
/// ohne Punkt oder Leerzeichen am Ende, höchstens 60 Zeichen.
pub fn clean_event(event: &str) -> String {
    let s: String = event
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(60)
        .collect();
    s.trim_end_matches(['.', ' ']).to_string()
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportProgress {
    pub done: usize,
    pub total: usize,
    pub bytes: u64,
    pub bytes_total: u64,
    /// Datei, die gerade kopiert wird
    pub current: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// Kopierte Aufnahmen
    pub imported: usize,
    /// Kopierte Dateien (mit RAW + JPEG, `.xmp`)
    pub files: usize,
    /// Schon mit gleichem Inhalt am Ziel
    pub existing: usize,
    /// Auf der Karte gelöschte Dateien
    pub deleted: usize,
    pub bytes: u64,
    pub failed: Vec<(String, String)>,
    /// Ordner, in die importiert wurde
    pub dirs: Vec<PathBuf>,
    pub canceled: bool,
}

/// Kopiert die Aufnahmen. `counters`: Zähler je Aufnahme (siehe [`counters`]).
pub fn run(
    cands: &[(&Candidate, String)],
    target: &Path,
    event: &str,
    delete_after: bool,
    cancel: &AtomicBool,
    progress: &dyn Fn(&ImportProgress),
) -> ImportReport {
    let mut report = ImportReport::default();
    let mut p = ImportProgress {
        total: cands.len(),
        bytes_total: cands.iter().map(|(c, _)| c.size).sum(),
        ..Default::default()
    };
    let mut dirs = std::collections::BTreeSet::new();
    for (c, counter) in cands {
        if cancel.load(Ordering::Relaxed) {
            report.canceled = true;
            break;
        }
        p.current = c
            .main()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        progress(&p);
        let dir = target_dir(target, &c.captured_at, event);
        match import_one(c, &dir, counter, cancel, &mut |n| {
            p.bytes += n;
            progress(&p);
        }) {
            Ok(Outcome::Copied(n)) => {
                report.imported += 1;
                report.files += c.files.len();
                report.bytes += n;
                dirs.insert(dir);
                if delete_after {
                    report.deleted += delete_originals(c, &mut report.failed);
                }
            }
            Ok(Outcome::Existing) => {
                report.existing += 1;
                dirs.insert(dir);
                if delete_after {
                    report.deleted += delete_originals(c, &mut report.failed);
                }
            }
            Err(e) => {
                if cancel.load(Ordering::Relaxed) {
                    report.canceled = true;
                    break;
                }
                log::warn!("Import von {}: {e}", c.main().display());
                report.failed.push((p.current.clone(), e.to_string()));
            }
        }
        p.done += 1;
        progress(&p);
    }
    report.dirs = dirs.into_iter().collect();
    report
}

enum Outcome {
    /// Kopiert (Bytes)
    Copied(u64),
    /// Lag mit gleichem Inhalt schon am Ziel
    Existing,
}

fn import_one(
    c: &Candidate,
    dir: &Path,
    counter: &str,
    cancel: &AtomicBool,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<Outcome> {
    std::fs::create_dir_all(dir)?;
    let base = new_stem(&c.captured_at, counter);
    // Freien Namen suchen: gleicher Name mit gleichem Inhalt → schon da;
    // mit anderem Inhalt (andere Kamera, gleiche Sekunde und Nummer) → `-2` …
    let mut stem = base.clone();
    for n in 2.. {
        let dests: Vec<PathBuf> = c
            .files
            .iter()
            .map(|f| dir.join(renamed(f, &c.stem, &stem)))
            .collect();
        let states: Vec<Option<bool>> = c
            .files
            .iter()
            .zip(&dests)
            .map(|(src, dst)| dst.exists().then(|| same_content(src, dst)))
            .collect();
        if states.iter().all(|s| *s == Some(true)) {
            return Ok(Outcome::Existing);
        }
        if states.iter().all(|s| *s != Some(false)) {
            break;
        }
        stem = format!("{base}-{n}");
    }
    let mut total = 0;
    for f in &c.files {
        let dst = dir.join(renamed(f, &c.stem, &stem));
        if dst.exists() && same_content(f, &dst) {
            continue; // Teil der Gruppe schon da (abgebrochener Import)
        }
        total += copy_verified(f, &dst, cancel, on_bytes)?;
    }
    Ok(Outcome::Copied(total))
}

/// Gleicher Inhalt: gleiche Größe und gleicher BLAKE3 der ganzen Datei.
fn same_content(a: &Path, b: &Path) -> bool {
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
    size(a).is_some()
        && size(a) == size(b)
        && hash_file(a)
            .ok()
            .is_some_and(|h| hash_file(b).ok() == Some(h))
}

fn hash_file(p: &Path) -> std::io::Result<blake3::Hash> {
    let mut h = blake3::Hasher::new();
    let mut f = File::open(p)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(h.finalize());
        }
        h.update(&buf[..n]);
    }
}

/// Kopiert über eine `.part`-Datei, prüft sie gegen den Hash des gelesenen
/// Originals und benennt sie erst dann um. Übernimmt die Änderungszeit.
fn copy_verified(
    src: &Path,
    dst: &Path,
    cancel: &AtomicBool,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<u64> {
    let part = dst.with_extension(format!(
        "{}.part",
        dst.extension().unwrap_or_default().to_string_lossy()
    ));
    let res = (|| -> Result<u64> {
        let mut input = File::open(src)?;
        let mut output = File::create(&part)?;
        let mut h = blake3::Hasher::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut total = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Invalid(
                    lumenkorn_develop::i18n::tr_s("Abgebrochen", "Canceled").into(),
                ));
            }
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            output.write_all(&buf[..n])?;
            total += n as u64;
            on_bytes(n as u64);
        }
        output.sync_all()?;
        if let Ok(t) = std::fs::metadata(src).and_then(|m| m.modified()) {
            let _ = output.set_modified(t);
        }
        drop(output);
        if hash_file(&part)? != h.finalize() {
            return Err(Error::Invalid(lumenkorn_develop::tr!(
                "{}: Kopie weicht vom Original ab",
                "{}: copy differs from the original",
                src.display()
            )));
        }
        std::fs::rename(&part, dst)?;
        Ok(total)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    res
}

/// Originale löschen (nur nach erfolgreicher, geprüfter Kopie aufgerufen).
fn delete_originals(c: &Candidate, failed: &mut Vec<(String, String)>) -> usize {
    let mut n = 0;
    for f in &c.files {
        match std::fs::remove_file(f) {
            Ok(()) => n += 1,
            Err(e) => failed.push((f.display().to_string(), e.to_string())),
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_come_from_the_camera_number() {
        assert_eq!(counter_of("DSC07869").as_deref(), Some("7869"));
        assert_eq!(counter_of("DSC00778").as_deref(), Some("778"));
        assert_eq!(counter_of("IMG_0042").as_deref(), Some("42"));
        assert_eq!(counter_of("DSC00000").as_deref(), Some("0"));
        assert_eq!(counter_of("Foto"), None);
        // Zeitstempel von Handys sind keine Bildnummer
        assert_eq!(counter_of("PXL_20261011_143210123"), None);
    }

    #[test]
    fn names_and_folders() {
        assert_eq!(
            new_stem("2026-10-11T14:32:10", "7869"),
            "20261011_143210_7869"
        );
        let p = Path::new("/karte/DSC07869.ARW.xmp");
        assert_eq!(
            renamed(p, "DSC07869", "20261011_143210_7869"),
            "20261011_143210_7869.ARW.xmp"
        );
        let t = Path::new("/bilder");
        assert_eq!(
            target_dir(t, "2026-10-11T14:32:10", ""),
            Path::new("/bilder").join("2026").join("2026.10.11")
        );
        assert_eq!(
            target_dir(t, "2026-10-11T14:32:10", " Hochzeit: Anna/Ben. "),
            Path::new("/bilder")
                .join("2026")
                .join("2026.10.11_Hochzeit_ Anna_Ben")
        );
    }

    #[test]
    fn groups_raw_jpeg_and_sidecars() {
        assert_eq!(group_stem(Path::new("a/DSC1.ARW")).as_deref(), Some("DSC1"));
        assert_eq!(group_stem(Path::new("a/DSC1.JPG")).as_deref(), Some("DSC1"));
        assert_eq!(group_stem(Path::new("a/DSC1.xmp")).as_deref(), Some("DSC1"));
        assert_eq!(
            group_stem(Path::new("a/DSC1.ARW.xmp")).as_deref(),
            Some("DSC1")
        );
        assert_eq!(group_stem(Path::new("a/C0001.MP4")), None);
    }

    #[test]
    fn mtime_fallback_is_a_valid_date() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x.jpg");
        std::fs::write(&f, b"x").unwrap();
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_791_820_330); // 2026-10-12T15:52:10Z
        File::options()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(t)
            .unwrap();
        assert_eq!(mtime_iso(&f).as_deref(), Some("2026-10-12T15:52:10"));
    }

    fn jpeg(path: &Path, seed: u8) {
        let img = image::RgbImage::from_fn(64, 48, |x, y| image::Rgb([seed, x as u8, y as u8]));
        img.save(path).unwrap();
    }

    #[test]
    fn imports_renames_verifies_and_deletes() {
        let d = tempfile::tempdir().unwrap();
        let card = d.path().join("DCIM").join("100MSDCF");
        std::fs::create_dir_all(&card).unwrap();
        jpeg(&card.join("DSC00778.JPG"), 1);
        std::fs::write(card.join("DSC00778.xmp"), b"<xmp/>").unwrap();
        jpeg(&card.join("DSC00779.JPG"), 2);
        std::fs::write(card.join("C0001.MP4"), b"video").unwrap();
        let cat = Mutex::new(Catalog::open_in_memory().unwrap());

        let found = scan(&d.path().join("DCIM"), &cat, &|_| {}).unwrap();
        assert_eq!(found.len(), 2, "Video gehört nicht dazu");
        assert_eq!(found[0].files.len(), 2, "JPEG mit xmp");
        assert!(found.iter().all(|c| c.date_from_file && !c.already));

        let target = d.path().join("Bilder");
        let counters = counters(&found);
        let pairs: Vec<(&Candidate, String)> = found.iter().zip(counters).collect();
        let report = run(
            &pairs,
            &target,
            "Test",
            true,
            &AtomicBool::new(false),
            &|_| {},
        );
        assert_eq!(
            (report.imported, report.files, report.deleted),
            (2, 3, 3),
            "{report:?}"
        );
        assert!(report.failed.is_empty());
        let dir = target_dir(&target, &found[0].captured_at, "Test");
        let stem = new_stem(&found[0].captured_at, "778");
        assert!(dir.join(format!("{stem}.JPG")).is_file());
        assert!(dir.join(format!("{stem}.xmp")).is_file());
        assert!(!card.join("DSC00778.JPG").exists(), "Original gelöscht");
        assert!(card.join("C0001.MP4").exists(), "Video bleibt");
        // Keine Reste
        assert!(
            std::fs::read_dir(&dir).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".part"))
        );
    }

    #[test]
    fn second_import_finds_existing_and_avoids_name_clashes() {
        let d = tempfile::tempdir().unwrap();
        let card = d.path().join("karte");
        std::fs::create_dir_all(&card).unwrap();
        jpeg(&card.join("DSC00010.JPG"), 1);
        let cat = Mutex::new(Catalog::open_in_memory().unwrap());
        let target = d.path().join("Bilder");
        let found = scan(&card, &cat, &|_| {}).unwrap();
        let pairs: Vec<(&Candidate, String)> = found.iter().zip(counters(&found)).collect();
        assert_eq!(
            run(&pairs, &target, "", false, &AtomicBool::new(false), &|_| {}).imported,
            1
        );
        // Noch einmal: liegt schon da
        let again = run(&pairs, &target, "", false, &AtomicBool::new(false), &|_| {});
        assert_eq!((again.imported, again.existing), (0, 1));
        // Andere Datei mit gleichem Zeitpunkt und gleicher Nummer → `-2`
        jpeg(&card.join("DSC00010.JPG"), 9);
        let mut other = found[0].clone();
        other.size = std::fs::metadata(other.main()).unwrap().len();
        let pairs = vec![(&other, "10".to_string())];
        let r = run(&pairs, &target, "", false, &AtomicBool::new(false), &|_| {});
        assert_eq!(r.imported, 1);
        let dir = target_dir(&target, &other.captured_at, "");
        let stem = new_stem(&other.captured_at, "10");
        assert!(dir.join(format!("{stem}-2.JPG")).is_file());
    }
}
