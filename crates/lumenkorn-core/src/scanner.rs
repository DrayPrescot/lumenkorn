//! Ordner-Scan: gleicht die Dateien auf der Platte mit dem Katalog ab.
//!
//! Es wird nichts importiert oder kopiert. Neue Dateien werden aufgenommen,
//! geänderte neu eingelesen, verschobene/umbenannte per Inhalts-Hash
//! wiedererkannt (Bearbeitungen bleiben erhalten) und verschwundene als
//! „fehlend“ markiert statt gelöscht.

use crate::LockExt;
use crate::catalog::{Catalog, KnownImage, NewImage};
use crate::error::{Error, Result};
use crate::formats::{self, ImageKind};
use crate::metadata;
use crate::model::{ScanPhase, ScanProgress, ScanReport};
use lumenkorn_develop::{i18n::tr_s, tr};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;
use walkdir::{DirEntry, WalkDir};

/// Ordner, die nie Fotos des Nutzers enthalten.
const SKIP_DIRS: &[&str] = &[
    "@eaDir",
    "$RECYCLE.BIN",
    "System Volume Information",
    "node_modules",
    "#recycle",
];

struct FoundFile {
    path: PathBuf,
    ext: String,
    kind: ImageKind,
    size: i64,
    mtime: i64,
}

enum Action {
    /// Neue Datei (evtl. verschoben – wird anhand des Hashes entschieden).
    New,
    /// Bekannte Datei mit geändertem Inhalt.
    Changed(i64),
}

/// Eigene Ordner der App (Daten, Thumbnail-Cache): nie katalogisieren. Unter
/// Windows liegen sie in `%LOCALAPPDATA%` und sind nicht am Namen als
/// versteckt erkennbar – ein Quellordner wie `C:\Users\X` würde sonst die
/// eigenen Thumbnails als Fotos aufnehmen (und für jedes wieder eines erzeugen).
static EXCLUDED: std::sync::RwLock<Vec<PathBuf>> = std::sync::RwLock::new(Vec::new());

pub fn set_excluded_dirs(dirs: Vec<PathBuf>) {
    *EXCLUDED.write().unwrap_or_else(|e| e.into_inner()) = dirs;
}

pub fn is_excluded(p: &Path) -> bool {
    EXCLUDED
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|d| p.starts_with(d))
}

/// Markierung in Export-Ordnern, die Lumenkorn selbst angelegt hat. Solche
/// Ordner werden nicht katalogisiert – sonst tauchte jedes exportierte Foto als
/// neues Foto in der Bibliothek auf (wie in Lightroom landen Exporte nicht im
/// Katalog).
pub const EXPORT_MARKER: &str = ".lumenkorn-export";

/// Standard-Unterordner für den Export, relativ zum Ordner des Fotos
pub const DEFAULT_EXPORT_SUBFOLDER: &str = "Export";

/// Prüft den relativen Export-Unterordner aus den Einstellungen und liefert
/// ihn bereinigt (`/` als Trenner). Erlaubt sind mehrere Ebenen
/// (`Export/JPEG`), `..` (`../Exporte`) und `.` (neben dem Original); leer
/// heißt Standard. Absolute Pfade und unter Windows ungültige Namen nicht.
pub fn check_export_subfolder(rel: &str) -> std::result::Result<String, String> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Ok(DEFAULT_EXPORT_SUBFOLDER.into());
    }
    let norm = rel.replace('\\', "/");
    if norm.starts_with('/') || norm.contains(':') {
        return Err(tr!(
            "„{rel}“ ist kein relativer Pfad",
            "“{rel}” is not a relative path"
        ));
    }
    let parts: Vec<&str> = norm.split('/').filter(|p| !p.is_empty()).collect();
    for p in &parts {
        if matches!(*p, "." | "..") {
            continue;
        }
        let stem = p.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit());
        let bad = p.chars().any(|c| c.is_control() || "<>:\"|?*".contains(c))
            || p.ends_with('.')
            || p.ends_with(' ')
            || reserved;
        if bad {
            return Err(tr!(
                "„{p}“ ist als Ordnername nicht erlaubt",
                "“{p}” is not allowed as a folder name"
            ));
        }
    }
    Ok(if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    })
}

/// Exportordner für ein Foto, ohne ihn anzulegen: `rel` (siehe
/// [`check_export_subfolder`]) relativ zum Ordner des Fotos, `..` aufgelöst.
pub fn export_dir_path(photo: &Path, rel: &str) -> Option<PathBuf> {
    let mut dir = photo.parent()?.to_path_buf();
    for p in rel.split('/').filter(|p| !p.is_empty()) {
        match p {
            "." => {}
            ".." => {
                // Nicht über die Wurzel des Laufwerks hinaus
                if dir.parent().is_some() {
                    dir.pop();
                }
            }
            p => dir.push(p),
        }
    }
    Some(dir)
}

/// Standard-Exportziel: Unterordner `rel` neben dem Foto (Einstellungen,
/// sonst „Export“). Legt ihn an, falls nötig, und markiert ihn dann als
/// Export-Ordner. Ein schon vorhandener Ordner bleibt unmarkiert (er kann
/// eigene Fotos enthalten).
pub fn export_dir_for(photo: &Path, rel: &str) -> std::io::Result<PathBuf> {
    let dir = export_dir_path(photo, rel)
        .ok_or_else(|| std::io::Error::other(tr_s("Foto ohne Ordner", "Photo without folder")))?;
    if !dir.is_dir() {
        std::fs::create_dir_all(&dir)?;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            opts.attributes(0x2); // FILE_ATTRIBUTE_HIDDEN
        }
        use std::io::Write;
        opts.open(dir.join(EXPORT_MARKER))?.write_all(
            b"Von Lumenkorn angelegter Export-Ordner. Diese Datei loeschen, damit Lumenkorn die Fotos hier aufnimmt.\n",
        )?;
    }
    Ok(dir)
}

fn is_export_dir(dir: &Path) -> bool {
    dir.join(EXPORT_MARKER).is_file()
}

fn is_skipped(e: &DirEntry) -> bool {
    if e.depth() == 0 {
        return false;
    }
    let name = e.file_name().to_string_lossy();
    name.starts_with('.')
        || (e.file_type().is_dir()
            && (SKIP_DIRS.contains(&name.as_ref())
                || is_excluded(e.path())
                || is_export_dir(e.path())))
}

/// Quellordner nicht erreichbar (Laufwerk abgesteckt, NAS schläft)? Dann
/// nichts abgleichen – sonst gälten alle Fotos darin als fehlend.
fn ensure_online(root: &Path) -> Result<()> {
    if root.is_dir() {
        Ok(())
    } else {
        Err(Error::NotFound(tr!(
            "Quellordner nicht erreichbar: {} (Laufwerk getrennt?)",
            "Source folder not reachable: {} (drive disconnected?)",
            root.display()
        )))
    }
}

/// Findet ein Scan keine Datei, obwohl mehr als so viele Fotos bekannt sind,
/// gilt der Ordner als nicht bereit statt als leer.
const EMPTY_ROOT_GUARD: usize = 20;

fn mtime_secs(m: &std::fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Scannt einen Quellordner vollständig.
///
/// Die Datenbank wird nur kurz gesperrt; Dateisystemzugriffe und EXIF-Lesen
/// laufen parallel ohne Sperre, damit die Oberfläche bedienbar bleibt.
pub fn scan_root(
    catalog: &Mutex<Catalog>,
    root_id: i64,
    progress: &(dyn Fn(ScanProgress) + Sync),
) -> Result<ScanReport> {
    let (root, known) = {
        let cat = catalog.locked();
        (cat.root(root_id)?, cat.known_images(root_id)?)
    };
    ensure_online(Path::new(&root.path))?;
    let mut w = Walked::default();
    walk(Path::new(&root.path), &mut w, &|n| {
        progress(ScanProgress {
            root_id,
            phase: ScanPhase::Walking,
            done: n,
            total: 0,
        })
    });
    // Ein Quellordner, der plötzlich leer wirkt (Netzlaufwerk antwortet mit
    // einer leeren Liste), darf nicht alle Fotos als fehlend markieren
    let present = known.iter().filter(|k| !k.missing).count();
    if w.files.is_empty() && present > EMPTY_ROOT_GUARD {
        log::warn!(
            "Scan von {} abgebrochen: keine Dateien gefunden, aber {present} Fotos bekannt",
            root.path
        );
        return Err(Error::NotFound(tr!(
            "Im Quellordner {} wurden keine Fotos gefunden, obwohl {} bekannt sind. Der Abgleich wurde abgebrochen (Laufwerk nicht bereit?).",
            "No photos were found in source folder {}, although {} are known. The scan was cancelled (drive not ready?).",
            root.path,
            present
        )));
    }
    let mut known = known;
    w.keep_readable(&mut known);
    let mut report = reconcile(catalog, root_id, w.files, known, progress)?;
    // Alle Ordner auf der Platte merken – auch leere oder solche mit nur
    // nicht unterstützten Dateien erscheinen so im Ordnerbaum
    let dirs: Vec<String> = w
        .dirs
        .iter()
        .map(|d| d.to_string_lossy().into_owned())
        .collect();
    report.folders_changed = catalog.locked().set_folders(root_id, &dirs)?;
    report.unsupported = w.unsupported.into_iter().collect();
    report.unreadable = w.unreadable;
    Ok(report)
}

/// Gleicht nur die gemeldeten Pfade ab statt den ganzen Quellordner.
///
/// Für die Dateiüberwachung: Eine neue Datei in einem Ordner mit 50 000
/// Fotos kostet so einen Dateizugriff statt 50 000. Gemeldet werden können
/// Dateien und Ordner, auch solche, die es nicht mehr gibt (gelöscht,
/// umbenannt). Ein vorhandener Ordner wird samt Inhalt abgeglichen, für
/// einen verschwundenen werden alle Bilder darin als fehlend markiert –
/// außer sie tauchen im selben Durchgang an anderer Stelle wieder auf
/// (verschoben, per Inhalts-Hash erkannt).
pub fn scan_paths(
    catalog: &Mutex<Catalog>,
    root_id: i64,
    changed: &[PathBuf],
    progress: &(dyn Fn(ScanProgress) + Sync),
) -> Result<ScanReport> {
    let root = catalog.locked().root(root_id)?;
    let root_path = Path::new(&root.path);
    ensure_online(root_path)?;
    // Nur Pfade in diesem Quellordner, keine versteckten oder übersprungenen
    let mut changed: Vec<PathBuf> = changed
        .iter()
        .filter_map(|p| within_root(root_path, p))
        .filter(|p| !in_skipped_dir(root_path, p))
        .collect();
    changed.sort();
    changed.dedup();
    // Pfade unterhalb eines ebenfalls gemeldeten Ordners werden mit ihm
    // erfasst. Sortiert folgen Unterpfade direkt auf ihren Ordner.
    let mut tops: Vec<&PathBuf> = Vec::new();
    for p in &changed {
        if tops.last().is_none_or(|t| !p.starts_with(t)) {
            tops.push(p);
        }
    }

    let mut w = Walked::default();
    // Gemeldete Ordner, die es nicht mehr gibt (gelöscht, umbenannt)
    let mut gone: Vec<String> = Vec::new();
    for p in &tops {
        match std::fs::symlink_metadata(p) {
            Ok(m) if m.is_dir() => {
                if p.as_path() != root_path {
                    w.dirs.push(p.to_path_buf());
                }
                walk(p, &mut w, &|_| {});
            }
            Ok(m) if m.is_file() => {
                if let Some(f) = found_file(p.to_path_buf(), &m) {
                    w.files.push(f);
                } else if let Some(ext) = formats::unsupported_image(p) {
                    *w.unsupported.entry(ext).or_default() += 1;
                }
            }
            // Gelöscht oder umbenannt: bekannte Bilder und Ordner darunter betroffen
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                gone.push(p.to_string_lossy().into_owned())
            }
            // Anderer Fehler (Zugriff, Netz): weder gelöscht noch abgleichen
            Err(_) => w.failed.push(p.to_path_buf()),
            _ => {}
        }
    }
    let found = std::mem::take(&mut w.files);
    let top_strings: Vec<String> = tops
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let known = catalog.locked().known_images_under(root_id, &top_strings)?;

    // Betroffen sind die bekannten Bilder, die selbst oder deren Ordner (auf
    // irgendeiner Ebene) gemeldet wurden – per Nachschlagen statt jeden
    // Pfad mit jedem gemeldeten Ordner zu vergleichen.
    let reported: HashSet<&Path> = tops.iter().map(|p| p.as_path()).collect();
    // Dazu alle bereits fehlenden: Taucht eins hier wieder auf (zurück-
    // kopiert, Ordner zurückverschoben), wird es per Inhalts-Hash
    // wiedererkannt statt doppelt angelegt. Erneut als fehlend gezählt
    // werden sie nicht.
    let affected: Vec<KnownImage> = known
        .into_iter()
        .filter(|k| {
            k.missing
                || Path::new(&k.path)
                    .ancestors()
                    .take_while(|a| a.starts_with(root_path))
                    .any(|a| reported.contains(a))
        })
        .collect();
    let mut affected = affected;
    w.keep_readable(&mut affected);
    let mut report = reconcile(catalog, root_id, found, affected, progress)?;
    let dirs: Vec<String> = w
        .dirs
        .iter()
        .map(|d| d.to_string_lossy().into_owned())
        .collect();
    report.folders_changed = catalog.locked().update_folders(root_id, &dirs, &gone)?;
    report.unsupported = w.unsupported.into_iter().collect();
    report.unreadable = w.unreadable;
    Ok(report)
}

/// Ein gemeldeter Pfad in der Schreibweise des Quellordners, falls er darin
/// liegt. Der Quellordner ist normiert (`canonicalize`); gemeldete Pfade können
/// anders geschrieben sein – unter Windows etwa mit Kurznamen („RUNNER~1“) oder
/// über eine Verknüpfung. Dann wird der tiefste noch vorhandene Ordner normiert
/// und der Rest (z. B. eine gelöschte Datei) wieder angehängt.
fn within_root(root: &Path, p: &Path) -> Option<PathBuf> {
    if p.starts_with(root) {
        // Neu zusammensetzen: einheitliche Trennzeichen (unter Windows „\“
        // statt gemischt mit „/“), sonst passt der Pfad nicht zum Katalog
        return Some(p.components().collect());
    }
    let mut base = p;
    let mut rest = Vec::new();
    loop {
        if std::fs::canonicalize(base).is_ok() {
            let mut full = PathBuf::from(crate::paths::normalize_dir(base));
            full.extend(rest.iter().rev());
            return full.starts_with(root).then_some(full);
        }
        rest.push(base.file_name()?);
        base = base.parent()?;
    }
}

/// Liegt `p` in einem Ordner, den der Scan überspringt (versteckt, System)?
fn in_skipped_dir(root: &Path, p: &Path) -> bool {
    is_excluded(p)
        || p.strip_prefix(root).is_ok_and(|rel| {
            rel.components().any(|c| {
                let n = c.as_os_str().to_string_lossy();
                n.starts_with('.') || SKIP_DIRS.contains(&n.as_ref())
            })
        })
        // Export-Ordner (auch der gemeldete Ordner selbst)
        || p
            .ancestors()
            .take_while(|a| *a != root && a.starts_with(root))
            .any(is_export_dir)
}

fn found_file(path: PathBuf, meta: &std::fs::Metadata) -> Option<FoundFile> {
    let (ext, kind) = formats::classify(&path)?;
    Some(FoundFile {
        path,
        ext,
        kind,
        size: meta.len() as i64,
        mtime: mtime_secs(meta),
    })
}

/// Ergebnis eines Durchlaufs durch einen Ordner.
#[derive(Default)]
struct Walked {
    files: Vec<FoundFile>,
    /// Unterordner (ohne den Startordner)
    dirs: Vec<PathBuf>,
    /// Bilder in nicht unterstützten Formaten, je Endung
    unsupported: std::collections::BTreeMap<String, usize>,
    /// Nicht lesbare Einträge (z. B. fehlende Berechtigung)
    unreadable: usize,
    /// Pfade, an denen das Lesen scheiterte. Bekannte Bilder darunter gelten
    /// nicht als fehlend – ein kurz nicht antwortender NAS-Ordner soll nicht
    /// Tausende Fotos ausblenden.
    failed: Vec<PathBuf>,
}

impl Walked {
    /// Bekannte Bilder unter nicht lesbaren Pfaden nicht abgleichen
    fn keep_readable(&self, known: &mut Vec<KnownImage>) {
        if !self.failed.is_empty() {
            known.retain(|k| {
                !self
                    .failed
                    .iter()
                    .any(|f| Path::new(&k.path).starts_with(f))
            });
        }
    }
}

/// Sammelt alle Bilddateien und Unterordner unter `dir` ein.
fn walk(dir: &Path, out: &mut Walked, progress: &dyn Fn(usize)) {
    for entry in WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_skipped(e))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                log::warn!("Überspringe: {err}");
                out.unreadable += 1;
                // Nicht vorhanden ist kein Lesefehler (gelöschte Datei)
                let gone = err
                    .io_error()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound);
                if let (Some(p), false) = (err.path(), gone) {
                    out.failed.push(p.to_path_buf());
                }
                continue;
            }
        };
        if entry.file_type().is_dir() {
            if entry.depth() > 0 {
                out.dirs.push(entry.into_path());
            }
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let path = entry.into_path();
        if let Some(f) = found_file(path.clone(), &meta) {
            out.files.push(f);
            if out.files.len().is_multiple_of(500) {
                progress(out.files.len());
            }
        } else if let Some(ext) = formats::unsupported_image(&path) {
            *out.unsupported.entry(ext).or_default() += 1;
        }
    }
}

/// Gleicht gefundene Dateien mit bekannten Bildern ab und schreibt das
/// Ergebnis. `known` sind genau die Bilder, die der Durchgang abdeckt: Was
/// davon nicht gefunden wurde, gilt als verschoben oder fehlend.
fn reconcile(
    catalog: &Mutex<Catalog>,
    root_id: i64,
    found: Vec<FoundFile>,
    known: Vec<KnownImage>,
    progress: &(dyn Fn(ScanProgress) + Sync),
) -> Result<ScanReport> {
    let report_progress = |phase, done, total| {
        progress(ScanProgress {
            root_id,
            phase,
            done,
            total,
        })
    };

    // 1. Mit dem Katalog abgleichen
    let by_path: HashMap<&str, &KnownImage> = known.iter().map(|k| (k.path.as_str(), k)).collect();
    let mut seen: HashSet<i64> = HashSet::new();
    let mut unchanged_but_missing = Vec::new();
    let mut to_read: Vec<(FoundFile, Action)> = Vec::new();
    let mut report = ScanReport {
        root_id,
        ..Default::default()
    };

    for f in found {
        let key = f.path.to_string_lossy();
        match by_path.get(key.as_ref()) {
            Some(k) => {
                seen.insert(k.id);
                if k.size == f.size && k.mtime == f.mtime {
                    report.unchanged += 1;
                    if k.missing {
                        unchanged_but_missing.push(k.id);
                    }
                } else {
                    to_read.push((f, Action::Changed(k.id)));
                }
            }
            None => to_read.push((f, Action::New)),
        }
    }

    // 2. Hash und Metadaten parallel lesen
    let total = to_read.len();
    let done = AtomicUsize::new(0);
    let read: Vec<(NewImage, Action)> = to_read
        .into_par_iter()
        .filter_map(|(f, action)| {
            let hash = match metadata::quick_hash(&f.path) {
                Ok(h) => h,
                Err(err) => {
                    log::warn!("{}: {err}", f.path.display());
                    return None;
                }
            };
            // Ein Absturz im EXIF-/RAW-Leser darf nicht den ganzen Scan beenden
            let meta =
                std::panic::catch_unwind(|| metadata::read(&f.path, f.kind)).unwrap_or_else(|_| {
                    log::warn!("{}: Metadaten nicht lesbar (Absturz)", f.path.display());
                    Default::default()
                });
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_multiple_of(50) || n == total {
                report_progress(ScanPhase::Reading, n, total);
            }
            Some((to_new_image(root_id, &f, hash, meta), action))
        })
        .collect();

    // Nicht mehr gefundene Bilder sind Kandidaten für „verschoben“.
    let mut vanished: HashMap<(&str, i64), Vec<i64>> = HashMap::new();
    for k in known.iter().filter(|k| !seen.contains(&k.id)) {
        vanished
            .entry((k.quick_hash.as_str(), k.size))
            .or_default()
            .push(k.id);
    }

    // 3. Schreiben – in Portionen zu je 1000 Fotos, jede in einer Transaktion.
    // Dazwischen wird die Katalogsperre freigegeben: Der erste Import von
    // 50 000 Fotos (jedes mit Suchindex-Eintrag) hielte die Oberfläche sonst
    // für die ganze Dauer an.
    if !read.is_empty() {
        report_progress(ScanPhase::Writing, 0, read.len());
    }
    let mut written = 0;
    for (k, chunk) in read.chunks(1000).enumerate() {
        let mut cat = catalog.locked();
        cat.with_tx(|tx| {
            if k == 0 {
                for id in &unchanged_but_missing {
                    Catalog::set_missing(tx, *id, false)?;
                }
            }
            for (img, action) in chunk {
                match action {
                    Action::Changed(id) => {
                        Catalog::update_image_content(tx, *id, img)?;
                        report.updated += 1;
                    }
                    // Ein paralleler Scan (z. B. nach dem Zusammenfügen und
                    // zugleich die Ordnerüberwachung) hat den Pfad schon
                    // eingetragen: nur aktualisieren. Ein zweites Einfügen
                    // scheiterte am eindeutigen Pfad und verwürfe die Portion.
                    Action::New if let Some(id) = Catalog::image_id_by_path(tx, &img.path)? => {
                        Catalog::update_image_content(tx, id, img)?;
                    }
                    Action::New => {
                        let moved_from = vanished
                            .get_mut(&(img.quick_hash.as_str(), img.size))
                            .and_then(|ids| ids.pop());
                        match moved_from {
                            Some(id) => {
                                Catalog::move_image(tx, id, img)?;
                                report.moved += 1;
                            }
                            None => {
                                Catalog::insert_image(tx, img)?;
                                report.added += 1;
                            }
                        }
                    }
                }
            }
            Ok(())
        })?;
        drop(cat);
        written += chunk.len();
        if read.len() > 1000 {
            report_progress(ScanPhase::Writing, written, read.len());
        }
    }
    let mut cat = catalog.locked();
    cat.with_tx(|tx| {
        if read.is_empty() {
            for id in &unchanged_but_missing {
                Catalog::set_missing(tx, *id, false)?;
            }
        }
        let known_missing: HashSet<i64> =
            known.iter().filter(|k| k.missing).map(|k| k.id).collect();
        for id in vanished.values().flatten() {
            if !known_missing.contains(id) {
                Catalog::set_missing(tx, *id, true)?;
                report.missing += 1;
            }
        }
        Catalog::mark_scanned(tx, root_id)?;
        Ok(())
    })?;

    Ok(report)
}

fn to_new_image(
    root_id: i64,
    f: &FoundFile,
    quick_hash: String,
    meta: metadata::Metadata,
) -> NewImage {
    let path = f.path.to_string_lossy().into_owned();
    let dir = f
        .path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file_name = f
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    NewImage {
        root_id,
        path,
        dir,
        file_name,
        ext: f.ext.clone(),
        kind: f.kind,
        size: f.size,
        mtime: f.mtime,
        quick_hash,
        meta,
    }
}
