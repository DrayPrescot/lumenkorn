//! Ordner-Scan: gleicht die Dateien auf der Platte mit dem Katalog ab.
//!
//! Es wird nichts importiert oder kopiert. Neue Dateien werden aufgenommen,
//! geänderte neu eingelesen, verschobene/umbenannte per Inhalts-Hash
//! wiedererkannt (Bearbeitungen bleiben erhalten) und verschwundene als
//! „fehlend“ markiert statt gelöscht.

use crate::catalog::{Catalog, KnownImage, NewImage};
use crate::error::{Error, Result};
use crate::formats::{self, ImageKind};
use crate::metadata;
use crate::model::{ScanPhase, ScanProgress, ScanReport};
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

fn is_skipped(e: &DirEntry) -> bool {
    if e.depth() == 0 {
        return false;
    }
    let name = e.file_name().to_string_lossy();
    name.starts_with('.')
        || (e.file_type().is_dir() && (SKIP_DIRS.contains(&name.as_ref()) || is_excluded(e.path())))
}

/// Quellordner nicht erreichbar (Laufwerk abgesteckt, NAS schläft)? Dann
/// nichts abgleichen – sonst gälten alle Fotos darin als fehlend.
fn ensure_online(root: &Path) -> Result<()> {
    if root.is_dir() {
        Ok(())
    } else {
        Err(Error::NotFound(format!(
            "Quellordner nicht erreichbar: {} (Laufwerk getrennt?)",
            root.display()
        )))
    }
}

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
        let cat = catalog.lock().unwrap();
        (cat.root(root_id)?, cat.known_images(root_id)?)
    };
    ensure_online(Path::new(&root.path))?;
    let mut found = Vec::new();
    walk(Path::new(&root.path), &mut found, &|n| {
        progress(ScanProgress {
            root_id,
            phase: ScanPhase::Walking,
            done: n,
            total: 0,
        })
    });
    reconcile(catalog, root_id, found, known, progress)
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
    let (root, known) = {
        let cat = catalog.lock().unwrap();
        (cat.root(root_id)?, cat.known_images(root_id)?)
    };
    let root_path = Path::new(&root.path);
    ensure_online(root_path)?;
    // Nur Pfade in diesem Quellordner, keine versteckten oder übersprungenen
    let mut changed: Vec<&PathBuf> = changed
        .iter()
        .filter(|p| p.starts_with(root_path) && !in_skipped_dir(root_path, p))
        .collect();
    changed.sort();
    changed.dedup();
    // Pfade unterhalb eines ebenfalls gemeldeten Ordners werden mit ihm
    // erfasst. Sortiert folgen Unterpfade direkt auf ihren Ordner.
    let mut tops: Vec<&PathBuf> = Vec::new();
    for p in changed {
        if tops.last().is_none_or(|t| !p.starts_with(t)) {
            tops.push(p);
        }
    }

    let mut found = Vec::new();
    for p in &tops {
        match std::fs::symlink_metadata(p) {
            Ok(m) if m.is_dir() => walk(p, &mut found, &|_| {}),
            Ok(m) if m.is_file() => {
                if let Some(f) = found_file(p.to_path_buf(), &m) {
                    found.push(f);
                }
            }
            // Gelöscht, umbenannt oder Symlink: nur bekannte Bilder betroffen
            _ => {}
        }
    }

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
    reconcile(catalog, root_id, found, affected, progress)
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

/// Sammelt alle Bilddateien unter `dir` ein.
fn walk(dir: &Path, found: &mut Vec<FoundFile>, progress: &dyn Fn(usize)) {
    for entry in WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_skipped(e))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                log::warn!("Überspringe: {err}");
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if let Some(f) = found_file(entry.into_path(), &meta) {
            found.push(f);
            if found.len().is_multiple_of(500) {
                progress(found.len());
            }
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
            let meta = metadata::read(&f.path, f.kind);
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
        let mut cat = catalog.lock().unwrap();
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
    let mut cat = catalog.lock().unwrap();
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
