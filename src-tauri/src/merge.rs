//! Bilder zusammenfügen: HDR aus einer Belichtungsreihe, Panorama, Focus
//! Stacking. Läuft im Hintergrund (Fortschritt über `merge-progress`), das
//! Ergebnis ist eine lineare EXR-Datei neben dem ersten Foto, die gleich in
//! den Katalog aufgenommen und wie ein RAW entwickelt wird.

use crate::AppState;
use crate::worker::Job;
use lumenkorn_core::Error;
use lumenkorn_core::LockExt;
use lumenkorn_core::formats::ImageKind;
use lumenkorn_develop::i18n::tr_s;
use lumenkorn_develop::merge::{focus, hdr, pano};
use lumenkorn_develop::{ImageF32, Source, SourceKind};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
pub struct MergeState {
    running: AtomicBool,
    cancel: AtomicBool,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeKind {
    Hdr,
    Pano,
    Focus,
}

impl MergeKind {
    fn suffix(self) -> &'static str {
        match self {
            Self::Hdr => "HDR",
            Self::Pano => "Pano",
            Self::Focus => "Fokus",
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MergeOptions {
    hdr: hdr::HdrOptions,
    pano: pano::PanoOptions,
    focus: focus::FocusOptions,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct MergeEvent {
    stage: String,
    done: usize,
    total: usize,
    finished: bool,
    error: Option<String>,
    /// Neues Foto im Katalog
    image_id: Option<i64>,
    file_name: Option<String>,
}

#[tauri::command]
pub fn merge_start(
    app: AppHandle,
    kind: MergeKind,
    ids: Vec<i64>,
    options: Option<MergeOptions>,
) -> Result<(), Error> {
    if ids.len() < 2 {
        return Err(Error::Invalid(
            tr_s(
                "Mindestens zwei Fotos auswählen.",
                "Select at least two photos.",
            )
            .into(),
        ));
    }
    if ids.len() > 200 {
        return Err(Error::Invalid(
            tr_s(
                "Höchstens 200 Fotos auf einmal zusammenfügen.",
                "Merge at most 200 photos at once.",
            )
            .into(),
        ));
    }
    // Gibt `running` wieder frei, auch wenn der Thread nicht startet
    let Some(running) = crate::busy::claim_in(&app, |s| &s.merge.running) else {
        return Err(Error::Invalid(
            tr_s(
                "Es werden bereits Fotos zusammengefügt.",
                "Photos are already being merged.",
            )
            .into(),
        ));
    };
    let state = app.state::<AppState>();
    state.merge.cancel.store(false, Ordering::SeqCst);
    let options = options.unwrap_or_default();
    let handle = app.clone();
    std::thread::Builder::new()
        .name("lumenkorn-merge".into())
        .spawn(move || {
            let app = handle;
            let state = app.state::<AppState>();
            let emit = |ev: MergeEvent| {
                let _ = app.emit("merge-progress", ev);
            };
            let t = std::time::Instant::now();
            // Auch nach einer Panik (z. B. ein Dekoder an einer kaputten Datei)
            // muss die Funktion wieder frei werden und die Oberfläche es erfahren.
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(&app, &state, kind, &ids, &options)
            }))
            .unwrap_or_else(|_| {
                Err(Error::Invalid(
                    tr_s(
                        "Interner Fehler beim Zusammenfügen (Details im Protokoll)",
                        "Internal error while merging (details in the log)",
                    )
                    .into(),
                ))
            });
            let cancelled = state.merge.cancel.load(Ordering::SeqCst);
            drop(running);
            // Große Zwischenpuffer dem Betriebssystem zurückgeben
            crate::memory::trim_heap();
            // Abbrechen ist kein Fehler: „Abgebrochen“/„Canceled“ melden (die Oberfläche schweigt dann)
            match res {
                Err(_) if cancelled => emit(MergeEvent {
                    stage: String::new(),
                    done: 0,
                    total: 0,
                    finished: true,
                    error: Some(lumenkorn_develop::i18n::tr_s("Abgebrochen", "Canceled").into()),
                    image_id: None,
                    file_name: None,
                }),
                Ok((id, name)) => {
                    log::info!(
                        "{kind:?} aus {} Fotos in {:.1?}: {name}",
                        ids.len(),
                        t.elapsed()
                    );
                    emit(MergeEvent {
                        stage: tr_s("Fertig", "Done").into(),
                        done: 1,
                        total: 1,
                        finished: true,
                        error: None,
                        image_id: id,
                        file_name: Some(name),
                    })
                }
                Err(e) => {
                    log::warn!("{kind:?} fehlgeschlagen: {e}");
                    emit(MergeEvent {
                        stage: String::new(),
                        done: 0,
                        total: 0,
                        finished: true,
                        error: Some(e.to_string()),
                        image_id: None,
                        file_name: None,
                    })
                }
            }
        })?;
    Ok(())
}

#[tauri::command]
pub fn merge_cancel(state: State<AppState>) {
    state.merge.cancel.store(true, Ordering::SeqCst);
}

fn run(
    app: &AppHandle,
    state: &AppState,
    kind: MergeKind,
    ids: &[i64],
    options: &MergeOptions,
) -> Result<(Option<i64>, String), Error> {
    let jobs = {
        let cat = state.catalog.locked();
        ids.iter()
            .map(|id| cat.thumb_job(*id))
            .collect::<Result<Vec<_>, _>>()?
    };
    // Aus RAWs entstanden → wie ein RAW entwickeln, sonst wie ein JPEG
    let scene_linear = AtomicBool::new(true);
    let load = |i: usize| -> lumenkorn_develop::Result<ImageF32> {
        let job = &jobs[i];
        let src = Source::load(
            Path::new(&job.path),
            job.kind == ImageKind::Raw,
            job.orientation.map(|o| o as u16),
            None,
        )?;
        if src.kind == SourceKind::Raster {
            scene_linear.store(false, Ordering::Relaxed);
        }
        Ok(src.image)
    };
    // Höchstens ~10 Meldungen pro Sekunde
    let last = Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(1));
    let progress = |stage: &str, done: usize, total: usize| {
        let mut l = last.locked();
        if done == 0 || done >= total || l.elapsed().as_millis() >= 100 {
            *l = std::time::Instant::now();
            let _ = app.emit(
                "merge-progress",
                MergeEvent {
                    stage: stage.into(),
                    done,
                    total,
                    finished: false,
                    error: None,
                    image_id: None,
                    file_name: None,
                },
            );
        }
    };
    let cancel = &state.merge.cancel;
    let n = jobs.len();
    // Referenzfoto (Index in `ids`), dessen Aufnahmedaten übernommen werden:
    // HDR ist auf die mittlere Belichtung skaliert (nicht auf das erste Foto,
    // das z. B. die −2-EV-Aufnahme sein kann), Focus Stacking richtet auf die
    // mittlere Aufnahme aus.
    let (img, reference) = match kind {
        MergeKind::Hdr => {
            let m = hdr::merge(n, &load, options.hdr, &progress, cancel)?;
            (m.image, m.reference)
        }
        MergeKind::Pano => (pano::merge(n, &load, options.pano, &progress, cancel)?, 0),
        MergeKind::Focus => (
            focus::merge(n, &load, options.focus, &progress, cancel)?,
            n / 2,
        ),
    };
    if cancel.load(Ordering::SeqCst) {
        return Err(Error::Invalid(tr_s("abgebrochen", "canceled").into()));
    }

    // Neben das erste Foto schreiben
    progress(tr_s("Speichern", "Saving"), 0, 1);
    let first = Path::new(&jobs[0].path);
    let dir = first.parent().unwrap_or(Path::new("."));
    let stem = first.file_stem().and_then(|s| s.to_str()).unwrap_or("Bild");
    let out: PathBuf =
        crate::develop::unique_path(dir, &format!("{stem}-{}", kind.suffix()), "exr");
    lumenkorn_develop::exr_io::write(&img, &out, scene_linear.load(Ordering::Relaxed))?;
    drop(img);
    let name = out
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    // In den Katalog aufnehmen, Aufnahmedaten vom Referenzfoto übernehmen
    let reference = ids[reference];
    let root_id = state.catalog.locked().image_root(reference)?;
    // Die Ordnerüberwachung meldet die neue Datei ebenfalls – scannt sie
    // gleichzeitig, kann einer der beiden Scans scheitern. Entscheidend ist
    // nur, dass die Datei danach im Katalog steht.
    let report = lumenkorn_core::scanner::scan_paths(
        &state.catalog,
        root_id,
        std::slice::from_ref(&out),
        &|_| {},
    );
    let adopt = || {
        state
            .catalog
            .locked()
            .adopt_capture_info(&out.to_string_lossy(), reference)
    };
    let mut id = adopt()?;
    if id.is_none() {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        id = adopt()?;
    }
    match report {
        Ok(report) => {
            let _ = app.emit("library-changed", report);
        }
        Err(e) => log::warn!("Zusammenfügen: Katalog-Scan von {name}: {e}"),
    }
    state.worker.submit(Job::Thumbs);
    Ok((id, name))
}
