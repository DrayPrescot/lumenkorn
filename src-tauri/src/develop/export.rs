//! Export: Dateien entwickeln und schreiben (im Hintergrund, Fortschritt per
//! `export-progress`), Zielnamen bei Konflikten, Vorabprüfung `export_conflicts`.

use super::*;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct ExportProgress {
    done: usize,
    total: usize,
    current: Option<String>,
    failed: Vec<String>,
    /// Dateien, die schon vorhanden waren und bleiben sollten
    skipped: usize,
    finished: bool,
    dir: String,
}

/// Entwickelt ein Foto für den Export. Mit begrenzter Kantenlänge nicht in
/// voller Auflösung: Die Quelle muss nur groß genug sein, dass der Zuschnitt
/// (und die Vergrößerung beim Geraderichten) die Zielgröße erreicht.
pub(crate) fn develop_for_export(
    state: &AppState,
    job: &lumenkorn_core::catalog::ThumbJob,
    params: &DevelopParams,
    long_edge: Option<usize>,
) -> Result<ImageF32, Error> {
    let load_edge = long_edge.filter(|m| *m > 0).map(|m| {
        let crop = params.crop.map_or(1.0, |c| c.w.min(c.h)).max(0.05);
        let tilt = lumenkorn_develop::geometry::load_margin(params);
        (m as f32 / crop * tilt).ceil() as usize
    });
    let src = Source::load(
        Path::new(&job.path),
        job.kind == ImageKind::Raw,
        job.orientation.map(|o| o as u16),
        load_edge,
    )?;
    let denoised = params.denoise.as_ref().and_then(|d| {
        let p = lumenkorn_develop::denoised::path(&state.denoised_dir, &d.id)?;
        lumenkorn_develop::denoised::read(&p, Some(src.image.long_edge())).ok()
    });
    Ok(lumenkorn_develop::render_auto(
        &src,
        params,
        &render_opts(state, denoised.as_ref()),
    ))
}

/// Welche vorhandenen Dateien „Überschreiben“ beim Export nie ersetzt. Sie
/// bekommen stattdessen einen neuen Namen (`name-2.jpg`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Keep {
    /// Nur die Fotos dieses Exports. Der Export-Dialog nennt vorher, welche
    /// Fotos der Bibliothek ersetzt würden.
    Sources,
    /// Alle Fotos der Bibliothek: KI-Agenten (MCP) dürfen keine Fotos ersetzen.
    Library,
}

/// Exportiert in voller Auflösung im Hintergrund. Fortschritt per `export-progress`.
/// Ohne `dir` landet jedes Foto im Unterordner „Export“ neben dem Original.
#[tauri::command(async)]
pub fn export_images(
    app: AppHandle,
    ids: Vec<i64>,
    dir: Option<String>,
    options: ExportOptions,
) -> Result<(), Error> {
    start_export(app, ids, dir, options, Keep::Sources)
}

/// [`export_images`] mit der Wahl, was nie überschrieben wird
pub(crate) fn start_export(
    app: AppHandle,
    ids: Vec<i64>,
    dir: Option<String>,
    options: ExportOptions,
    keep: Keep,
) -> Result<(), Error> {
    let dir = dir.map(PathBuf::from);
    if let Some(d) = &dir
        && !d.is_dir()
    {
        return Err(Error::Invalid(tr!(
            "{} ist kein Ordner",
            "{} is not a folder",
            d.display()
        )));
    }
    let claim = crate::task::claim(&app, |t| &t.export)?.full_res()?;
    claim.spawn("export", move |app, _claim| {
        let state = app.state::<AppState>();
        let mut run = ExportRun::new(app, &state, &ids, dir, &options, keep);
        for (i, id) in ids.iter().enumerate() {
            let mut name = None;
            // Ein Panic bei einem Bild (z. B. in der GPU-Pipeline) darf den
            // Export nicht beenden: Das Bild gilt als fehlgeschlagen.
            let res = crate::task::catch_panic(|| run.photo(i, *id, &mut name));
            if let Err(e) = res {
                log::warn!("Export {id}: {e}");
                let who = name.unwrap_or_else(|| id.to_string());
                run.failed.push(format!("{who}: {e}"));
            }
        }
        // Puffer in voller Auflösung nicht bis zum nächsten Export behalten
        lumenkorn_develop::release_large_caches();
        run.progress(ids.len(), None, true);
    })
}

/// Ein laufender Export: Ziele, was schon geschrieben wurde und was schiefging.
struct ExportRun<'a> {
    app: &'a AppHandle,
    state: &'a AppState,
    options: &'a ExportOptions,
    formats: Vec<ExportFormat>,
    /// Gewählter Ordner; ohne ihn der Unterordner `sub` neben jedem Foto
    dir: Option<PathBuf>,
    sub: String,
    keep: Keep,
    /// Die Fotos dieses Exports: nie überschreiben, auch nicht von einem anderen Foto
    /// desselben Laufs (sonst ersetzte IMG_1.CR2 die Datei IMG_1.JPG, bevor diese
    /// selbst exportiert wird). `None`: nicht lesbar, dann wird nichts ersetzt.
    sources: Option<SourceFiles>,
    total: usize,
    failed: Vec<String>,
    /// Dateien, die schon vorhanden waren und bleiben sollten
    skipped: usize,
    /// Tatsächlich benutzte Zielordner (für die Anzeige)
    used: std::collections::BTreeSet<PathBuf>,
    /// In diesem Export geschrieben: nie überschreiben (z. B. IMG_1.CR2 und IMG_1.JPG
    /// aus demselben Ordner)
    written: std::collections::HashSet<PathBuf>,
}

impl<'a> ExportRun<'a> {
    fn new(
        app: &'a AppHandle,
        state: &'a AppState,
        ids: &[i64],
        dir: Option<PathBuf>,
        options: &'a ExportOptions,
        keep: Keep,
    ) -> Self {
        let sources = state
            .catalog
            .locked()
            .thumb_jobs(ids)
            .inspect_err(|e| log::warn!("Quellen des Exports: {e}"))
            .ok()
            .map(|jobs| SourceFiles::new(jobs.into_values().map(|j| PathBuf::from(j.path))));
        ExportRun {
            app,
            state,
            options,
            formats: options.all_formats(),
            dir,
            sub: crate::settings::read(&state.data_dir).export_subfolder,
            keep,
            sources,
            total: ids.len(),
            failed: Vec::new(),
            skipped: 0,
            used: Default::default(),
            written: Default::default(),
        }
    }

    /// Anzeige des Ziels: gewählter Ordner bzw. die tatsächlich benutzten
    fn shown(&self) -> String {
        match (&self.dir, self.used.len()) {
            (Some(d), _) => d.display().to_string(),
            (None, 1) => self.used.first().unwrap().display().to_string(),
            (None, 0) => tr_s("Unterordner „Export“", "Subfolder “Export”").to_string(),
            (None, n) => tr!("„Export“ in {n} Ordnern", "“Export” in {n} folders"),
        }
    }

    fn progress(&self, done: usize, current: Option<String>, finished: bool) {
        let _ = self.app.emit(
            "export-progress",
            ExportProgress {
                done,
                total: self.total,
                current,
                failed: self.failed.clone(),
                skipped: self.skipped,
                finished,
                dir: self.shown(),
            },
        );
    }

    /// Darf `p` nicht ersetzt werden? (Foto dieses Exports bzw. mit [`Keep::Library`]
    /// jedes Foto der Bibliothek)
    fn is_protected(&self, p: &Path) -> bool {
        self.sources.as_ref().is_none_or(|s| s.contains(p))
            || (self.keep == Keep::Library
                && self
                    .state
                    .catalog
                    .locked()
                    .library_paths(&[p.to_string_lossy().into_owned()])
                    .map_or(true, |found| !found.is_empty()))
    }

    /// Foto `id` (das `i`-te) in alle Formate schreiben. `name` bekommt den Dateinamen
    /// für Fehlermeldungen, sobald er bekannt ist.
    fn photo(&mut self, i: usize, id: i64, name: &mut Option<String>) -> Result<(), Error> {
        let (job, params, meta) = {
            let cat = self.state.catalog.locked();
            // Fehlende Metadaten verhindern den Export nicht
            let meta = cat
                .export_meta(id)
                .inspect_err(|e| log::warn!("Metadaten für Export {id}: {e}"))
                .ok();
            (cat.thumb_job(id)?, cat.develop_params(id)?, meta)
        };
        let stem = Path::new(&job.path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| id.to_string());
        *name = Some(stem.clone());
        self.progress(i, Some(stem.clone()), false);
        let target_dir = match &self.dir {
            Some(d) => d.clone(),
            None => lumenkorn_core::scanner::export_dir_for(Path::new(&job.path), &self.sub)
                .map_err(|e| Error::Invalid(tr!("Export-Ordner: {e}", "Export folder: {e}")))?,
        };
        self.used.insert(target_dir.clone());
        // Entwickelt wird einmal für alle Formate (außer dem Original-DNG)
        let mut developed = None;
        let mut errors = Vec::new();
        for format in self.formats.clone() {
            let Some(target) = export_target(
                &target_dir,
                &stem,
                format.extension(),
                self.options.on_conflict,
                &self.written,
                |p| self.is_protected(p),
            ) else {
                self.skipped += 1;
                continue;
            };
            self.written.insert(target.clone());
            let res = self.write_format(
                format,
                &job,
                &params,
                meta.as_ref(),
                &mut developed,
                &target,
            );
            match res {
                Ok(()) => {
                    if let Err(e) = self.state.catalog.locked().record_export(id, &target) {
                        // Nur für späteres Hochladen; der Export selbst gelang
                        log::warn!("Export nicht eingetragen: {e}");
                    }
                }
                Err(e) if self.formats.len() > 1 => {
                    errors.push(format!("{}: {e}", format.extension().to_uppercase()))
                }
                Err(e) => errors.push(e.to_string()),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(Error::Invalid(errors.join("; ")))
        }
    }

    /// Eine Zieldatei schreiben. `developed`: das einmal entwickelte Bild, beim ersten
    /// Format, das es braucht, erzeugt.
    fn write_format(
        &self,
        format: ExportFormat,
        job: &lumenkorn_core::catalog::ThumbJob,
        params: &DevelopParams,
        meta: Option<&output::ExportMeta>,
        developed: &mut Option<Result<ImageF32, Error>>,
        target: &Path,
    ) -> Result<(), Error> {
        let options = self.options;
        if format == ExportFormat::Dng && !options.dng_edited {
            // Original ohne Bearbeitung: die Szenendaten der Quelle
            let src = Source::load(
                Path::new(&job.path),
                job.kind == ImageKind::Raw,
                job.orientation.map(|o| o as u16),
                options.long_edge.filter(|m| *m > 0),
            )?;
            return Ok(output::write_original_dng(
                &src.image, target, options, meta,
            )?);
        }
        let img = developed
            .get_or_insert_with(|| develop_for_export(self.state, job, params, options.long_edge));
        match img {
            Ok(img) => {
                let opts = ExportOptions {
                    format,
                    ..options.clone()
                };
                Ok(output::write(img, target, &opts, meta)?)
            }
            Err(e) => Err(Error::Invalid(e.to_string())),
        }
    }
}

/// Quellfotos eines Exports. Die Zieldatei heißt wie ihr Foto, also kann nur
/// ein Quellfoto gleichen Namens dieselbe Datei sein: Gesucht wird nach dem
/// Dateinamen (klein geschrieben), verglichen dann die Datei selbst. Ein
/// harter Link unter anderem Namen bleibt unberührt, weil der Export die
/// Zieldatei atomar ersetzt (neue Datei, dann Umbenennen).
pub(super) struct SourceFiles(std::collections::HashMap<String, Vec<PathBuf>>);

impl SourceFiles {
    fn key(name: &std::ffi::OsStr) -> String {
        name.to_string_lossy().to_lowercase()
    }

    fn new(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut map = std::collections::HashMap::<String, Vec<PathBuf>>::new();
        for p in paths {
            if let Some(name) = p.file_name() {
                map.entry(Self::key(name)).or_default().push(p);
            }
        }
        Self(map)
    }

    /// Ist die vorhandene Datei `target` eines der Quellfotos? Vergleicht unter
    /// Windows die Datei-ID, nicht die Schreibweise (Groß-/Kleinschreibung,
    /// kurze 8.3-Namen, verknüpfte Ordner). Lässt es sich nicht klären, gilt
    /// „ja“: Im Zweifel wird nichts ersetzt.
    fn contains(&self, target: &Path) -> bool {
        let Some(name) = target.file_name() else {
            return true;
        };
        self.0.get(&Self::key(name)).is_some_and(|candidates| {
            candidates
                .iter()
                .any(|src| same_file::is_same_file(src, target).unwrap_or(true))
        })
    }
}

/// Zieldatei eines Exports nach `mode`; `None`: überspringen. Was
/// `is_protected` meldet (die Quellfotos des Exports, bei KI-Agenten alle
/// Fotos der Bibliothek) und Dateien aus diesem Export werden nie
/// überschrieben, sondern bekommen einen neuen Namen.
pub(super) fn export_target(
    dir: &Path,
    stem: &str,
    ext: &str,
    mode: output::OnConflict,
    written: &std::collections::HashSet<PathBuf>,
    is_protected: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() && !written.contains(&first) {
        return Some(first);
    }
    let protected = written.contains(&first) || is_protected(&first);
    // Nächster freie Name, auch unter den in diesem Export vergebenen
    let renamed = || {
        (2..)
            .map(|n| dir.join(format!("{stem}-{n}.{ext}")))
            .find(|p| !p.exists() && !written.contains(p))
    };
    match mode {
        _ if protected => renamed(),
        output::OnConflict::Overwrite => Some(first),
        output::OnConflict::Skip => None,
        output::OnConflict::Rename => renamed(),
    }
}

/// Wo der Export ein Foto ablegen würde, ohne Ordner anzulegen
pub(super) fn planned_export_dir(dir: Option<&Path>, sub: &str, photo: &Path) -> Option<PathBuf> {
    match dir {
        Some(d) => Some(d.to_path_buf()),
        None => lumenkorn_core::scanner::export_dir_path(photo, sub),
    }
}

/// Zieldateien eines Exports, die es schon gibt
#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExportConflicts {
    /// Dateinamen
    names: Vec<String>,
    /// Davon Fotos der Bibliothek, die „Überschreiben“ ersetzen würde
    library: usize,
    /// Davon Fotos dieses Exports selbst: Sie werden nie überschrieben
    sources: usize,
}

/// Vor dem Export: Welche Zieldateien gibt es schon? Die Oberfläche fragt
/// dann, ob überschrieben, zusätzlich exportiert oder übersprungen werden
/// soll, und warnt, wenn dabei Fotos der Bibliothek ersetzt würden.
#[tauri::command(async)]
pub fn export_conflicts(
    state: State<AppState>,
    ids: Vec<i64>,
    dir: Option<String>,
    options: ExportOptions,
) -> Result<ExportConflicts, Error> {
    let dir = dir.map(PathBuf::from);
    let sub = crate::settings::read(&state.data_dir).export_subfolder;
    let formats = options.all_formats();
    let jobs = state.catalog.locked().thumb_jobs(&ids)?;
    let paths: Vec<&str> = ids
        .iter()
        .filter_map(|id| jobs.get(id).map(|j| j.path.as_str()))
        .collect();
    let sources = SourceFiles::new(paths.iter().map(PathBuf::from));
    // Dateisystem ohne Katalogsperre
    let mut seen = std::collections::HashSet::new();
    let mut existing = Vec::new();
    for p in &paths {
        let photo = Path::new(p);
        let (Some(d), Some(stem)) = (
            planned_export_dir(dir.as_deref(), &sub, photo),
            photo.file_stem(),
        ) else {
            continue;
        };
        for f in &formats {
            let t = d.join(format!("{}.{}", stem.to_string_lossy(), f.extension()));
            if t.exists() && seen.insert(t.clone()) {
                existing.push(t);
            }
        }
    }
    let (own, others): (Vec<&PathBuf>, Vec<&PathBuf>) =
        existing.iter().partition(|t| sources.contains(t));
    let others: Vec<String> = others
        .iter()
        .map(|t| t.to_string_lossy().into_owned())
        .collect();
    let library = state.catalog.locked().library_paths(&others)?.len();
    Ok(ExportConflicts {
        names: existing
            .iter()
            .map(|t| {
                t.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect(),
        library,
        sources: own.len(),
    })
}

/// `name.jpg`, bei Kollision `name-2.jpg`, `name-3.jpg` …
pub(crate) fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem}-{n}.{ext}")))
        .find(|p| !p.exists())
        .expect("unendlich")
}

#[cfg(test)]
mod tests {
    use super::{SourceFiles, export_target, output::OnConflict};

    #[test]
    fn export_target_follows_conflict_mode() {
        let dir = std::env::temp_dir().join(format!("lw-target-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.jpg"), b"alt").unwrap();
        std::fs::write(dir.join("orig.jpg"), b"original").unwrap();
        let none = std::collections::HashSet::new();
        let protected = |p: &std::path::Path| p.ends_with("orig.jpg");
        let t = |stem, mode, written: &std::collections::HashSet<_>| {
            export_target(&dir, stem, "jpg", mode, written, protected)
        };
        // Frei: immer der eigentliche Name
        assert_eq!(t("neu", OnConflict::Skip, &none), Some(dir.join("neu.jpg")));
        assert_eq!(
            t("a", OnConflict::Overwrite, &none),
            Some(dir.join("a.jpg"))
        );
        assert_eq!(t("a", OnConflict::Rename, &none), Some(dir.join("a-2.jpg")));
        assert_eq!(t("a", OnConflict::Skip, &none), None);
        // Geschützte Dateien nie überschreiben, auch nicht auslassen
        assert_eq!(
            t("orig", OnConflict::Overwrite, &none),
            Some(dir.join("orig-2.jpg"))
        );
        // Im selben Export geschrieben (IMG_1.CR2 und IMG_1.JPG): daneben legen
        let written = [dir.join("neu.jpg")].into_iter().collect();
        assert_eq!(
            t("neu", OnConflict::Overwrite, &written),
            Some(dir.join("neu-2.jpg"))
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Quellfotos werden an der Datei erkannt, nicht an der Schreibweise des
    /// Pfads; eine andere Datei gleichen Namens ist kein Quellfoto.
    #[test]
    fn source_files_compare_the_file_not_the_spelling() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        for p in [
            a.join("IMG_1.jpg"),
            b.join("IMG_1.jpg"),
            a.join("other.jpg"),
        ] {
            std::fs::write(p, b"x").unwrap();
        }
        let sources = SourceFiles::new([a.join("IMG_1.jpg")]);
        assert!(sources.contains(&a.join("IMG_1.jpg")));
        assert!(sources.contains(&a.join(".").join("IMG_1.jpg")));
        assert!(!sources.contains(&b.join("IMG_1.jpg")), "andere Datei");
        assert!(!sources.contains(&a.join("other.jpg")), "anderer Name");
        #[cfg(windows)]
        assert!(sources.contains(&a.join("img_1.JPG")), "Schreibweise");
        // Quelle nicht lesbar (Laufwerk getrennt): im Zweifel geschützt
        let gone = SourceFiles::new([tmp.path().join("weg").join("IMG_1.jpg")]);
        assert!(gone.contains(&b.join("IMG_1.jpg")));
    }

    /// Quelle im Ziel: „Überschreiben“ ersetzt Fotos der Bibliothek, aber nie
    /// ein Foto dieses Exports – auch nicht die Kamera-JPEG eines RAW+JPEG-Paars,
    /// das zusammen exportiert wird.
    #[test]
    fn overwrite_never_replaces_a_photo_of_the_export() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let (raw, jpg, other) = (
            dir.join("IMG_1.CR2"),
            dir.join("IMG_1.jpg"),
            dir.join("other.jpg"),
        );
        for p in [&raw, &jpg, &other] {
            std::fs::write(p, b"original").unwrap();
        }
        let none = std::collections::HashSet::new();
        let target = |stem, sources: &SourceFiles| {
            export_target(dir, stem, "jpg", OnConflict::Overwrite, &none, |p| {
                sources.contains(p)
            })
        };
        // Das Foto selbst (Export in den eigenen Ordner)
        let only_jpg = SourceFiles::new([jpg.clone()]);
        assert_eq!(target("IMG_1", &only_jpg), Some(dir.join("IMG_1-2.jpg")));
        // RAW und JPEG im selben Export: Die JPEG ist noch nicht exportiert
        let pair = SourceFiles::new([raw.clone(), jpg.clone()]);
        assert_eq!(target("IMG_1", &pair), Some(dir.join("IMG_1-2.jpg")));
        // Nur das RAW: Die JPEG ist ein Foto der Bibliothek und wird ersetzt
        let only_raw = SourceFiles::new([raw.clone()]);
        assert_eq!(target("IMG_1", &only_raw), Some(jpg.clone()));
        assert_eq!(target("other", &pair), Some(other.clone()));
    }
}
