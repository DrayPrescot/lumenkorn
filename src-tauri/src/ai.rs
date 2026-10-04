//! KI im Hintergrund: Modell laden, Bilder analysieren, semantisch suchen,
//! Modelle herunterladen.

use crate::AppState;
use crate::guard::{self, Guard, Stage};
use crate::memory::Slot;
use crate::worker::Job;
use lumenkorn_ai::accel::{self, Backend, Preference};
use lumenkorn_ai::index::{from_bytes, to_bytes};
use lumenkorn_ai::models::{self, Component, Status};
use lumenkorn_ai::segment::{self, Sam, SamEmbedding, SubjectModel};
use lumenkorn_ai::{Clip, EmbeddingIndex, Tagger, runtime, vocab};
use lumenkorn_core::maskstore::MaskFiles;
use lumenkorn_core::thumbs::Variant;
use lumenkorn_core::{Error, ImageSummary, Scope};
use lumenkorn_develop::{DevelopParams, PromptPoint, Source};
use serde::{Deserialize, Serialize};
use std::path::Path as StdPath;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tauri::{AppHandle, Emitter, Manager, State};

/// Kennung, unter der Embeddings im Katalog stehen. Ein anderes Modell bekommt
/// eine andere Kennung, damit nie Vektoren verschiedener Modelle gemischt werden.
const MODEL_KEY: &str = models::MODEL_ID;
const BATCH: usize = 16;

pub struct AiState {
    loaded: Mutex<Option<Loaded>>,
    /// Letzter Ladefehler, damit nicht jeder Stapel es erneut versucht.
    load_error: Mutex<Option<String>>,
    index: RwLock<Option<EmbeddingIndex>>,
    pub(crate) downloading: AtomicBool,
    cancel: AtomicBool,
    settings_path: PathBuf,
    pub(crate) settings: Mutex<AiSettings>,
    subject: Mutex<Option<Arc<SubjectModel>>>,
    sam: Mutex<Option<Arc<Sam>>>,
    /// SAM-Embedding des zuletzt geklickten Bildes – das Berechnen dauert Sekunden.
    sam_embedding: Mutex<Option<(i64, Arc<SamEmbedding>)>>,
    denoiser: Mutex<Option<Arc<lumenkorn_ai::denoise::Denoiser>>>,
    /// Bild, das gerade entrauscht wird
    denoising: Mutex<Option<i64>>,
    denoise_cancel: AtomicBool,
    /// Ordner für Absturz-Markierungen (der KI-Ordner)
    pub(crate) guard_dir: PathBuf,
    /// Meldung, wenn Lumenkorn beim letzten Start abgestürzt ist
    crash_notice: Mutex<Option<String>>,
}

#[derive(Clone)]
struct Loaded {
    clip: Arc<Clip>,
    tagger: Arc<Tagger>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiSettings {
    /// Neue Bilder automatisch analysieren
    pub auto_tag: bool,
    /// Rechenwerk für die KI: automatisch (GPU, falls installiert), CPU, …
    pub accel: Preference,
    /// Bildbearbeitung (Vorschau, Thumbnails, Export) auf der Grafikkarte
    pub develop_gpu: bool,
    /// Gesichter finden und Personen gruppieren
    pub faces: bool,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            auto_tag: true,
            accel: Preference::Auto,
            develop_gpu: true,
            faces: true,
        }
    }
}

fn backend_id(b: Backend) -> &'static str {
    match b {
        Backend::Cpu => "cpu",
        Backend::Webgpu => "webgpu",
        Backend::Directml => "directml",
    }
}

fn backend_from_id(id: &str) -> Option<Backend> {
    match id {
        "cpu" => Some(Backend::Cpu),
        "webgpu" => Some(Backend::Webgpu),
        "directml" => Some(Backend::Directml),
        _ => None,
    }
}

/// Wertet Abstürze des letzten Starts aus und schaltet den Auslöser ab, damit
/// Lumenkorn sicher startet. Liefert die Meldung für die Oberfläche.
fn handle_previous_crashes(ai_dir: &Path, settings: &mut AiSettings) -> Option<String> {
    let log = crate::logfile::path()
        .map(|p| format!(" Details stehen in {}.", p.display()))
        .unwrap_or_default();
    let mut notes = Vec::new();
    for c in guard::take_previous(ai_dir) {
        let what = match c.stage {
            Stage::KiLaufzeit => "beim Laden der KI-Laufzeit",
            Stage::KiModell => "beim Öffnen eines KI-Modells",
            Stage::KiAnalyse => "beim Analysieren der Fotos",
            Stage::GpuBearbeitung => "beim Öffnen der Grafikkarte für die Bildbearbeitung",
        };
        if c.stage == Stage::GpuBearbeitung {
            settings.develop_gpu = false;
            notes.push(format!(
                "Lumenkorn ist beim letzten Start {what} abgestürzt. Die Bildbearbeitung rechnet \
                 jetzt auf dem Prozessor; im KI-Dialog unter „Grafikkarte“ lässt sie sich wieder \
                 einschalten."
            ));
            continue;
        }
        match c.backend.as_deref().and_then(backend_from_id) {
            Some(b) if b.is_gpu() => {
                // GPU-Laufzeit beiseitelegen, damit sie nicht wieder geladen wird
                let dir = accel::runtime_dir(ai_dir, b);
                let off = dir.with_extension("deaktiviert");
                let _ = std::fs::remove_dir_all(&off);
                if let Err(e) = std::fs::rename(&dir, &off) {
                    log::warn!("{} nicht deaktiviert: {e}", dir.display());
                }
                settings.accel = Preference::Cpu;
                notes.push(format!(
                    "Lumenkorn ist beim letzten Start {what} mit der Grafikkarte ({}) \
                     abgestürzt. Die GPU-Laufzeit wurde deaktiviert, die KI rechnet jetzt auf dem \
                     Prozessor.{log}",
                    b.label()
                ));
            }
            _ => {
                settings.auto_tag = false;
                notes.push(format!(
                    "Lumenkorn ist beim letzten Start {what} abgestürzt ({}). Die automatische \
                     Verschlagwortung ist pausiert – im KI-Dialog lässt sie sich wieder \
                     einschalten.{log}",
                    c.detail
                ));
            }
        }
    }
    (!notes.is_empty()).then(|| notes.join("\n\n"))
}

/// Löscht Absturz-Markierungen – beim normalen Beenden, damit ein Schließen
/// mitten in einer Analyse nicht als Absturz gilt.
pub fn clear_crash_markers(state: &AppState) {
    let _ = guard::take_previous(&state.ai.guard_dir);
}

/// Aktuelles Rechenwerk der KI, für Absturz-Markierungen.
fn current_backend() -> &'static str {
    let a = accel::active();
    match a.library {
        Some(b) if a.gpu_enabled => backend_id(b),
        _ => "cpu",
    }
}

pub(crate) fn model_guard(state: &AppState, what: &str) -> Guard {
    Guard::begin(
        &state.ai.guard_dir,
        Stage::KiModell,
        Some(current_backend()),
        what,
    )
}

pub(crate) fn analysis_guard(state: &AppState, what: &str) -> Guard {
    Guard::begin(
        &state.ai.guard_dir,
        Stage::KiAnalyse,
        Some(current_backend()),
        what,
    )
}

/// Sucht die Grafikkarte für die Bildbearbeitung im Hintergrund – geschützt,
/// falls der Treiber dabei abstürzt.
fn start_develop_gpu(guard_dir: PathBuf) {
    let _ = std::thread::Builder::new()
        .name("lumenkorn-gpu".into())
        .spawn(move || {
            let _g = Guard::begin(&guard_dir, Stage::GpuBearbeitung, None, "wgpu");
            let _ = lumenkorn_develop::gpu::init();
        });
}

impl AiState {
    pub fn new(data_dir: &Path) -> Self {
        let ai_dir = models::ai_dir(data_dir);
        let settings_path = ai_dir.join("einstellungen.json");
        let mut settings: AiSettings = std::fs::read_to_string(&settings_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // DirectML-Laufzeit zurückgezogen (stürzte ab) – Reste entfernen
        let dml = accel::runtime_dir(&ai_dir, Backend::Directml);
        let mut changed = false;
        if dml.exists() {
            log::warn!(
                "Entferne zurückgezogene DirectML-Laufzeit {}",
                dml.display()
            );
            let _ = std::fs::remove_dir_all(&dml);
        }
        if settings.accel == Preference::Directml {
            settings.accel = Preference::Auto;
            changed = true;
        }
        let crash_notice = handle_previous_crashes(&ai_dir, &mut settings);
        if crash_notice.is_some() || changed {
            let _ = std::fs::create_dir_all(&ai_dir);
            if let Ok(json) = serde_json::to_string_pretty(&settings) {
                let _ = std::fs::write(&settings_path, json);
            }
        }
        // Grafikkarte im Hintergrund suchen – bis dahin rechnet die CPU.
        lumenkorn_develop::gpu::set_enabled(settings.develop_gpu);
        if settings.develop_gpu {
            start_develop_gpu(ai_dir.clone());
        }
        Self {
            guard_dir: ai_dir,
            crash_notice: Mutex::new(crash_notice),
            loaded: Mutex::new(None),
            load_error: Mutex::new(None),
            index: RwLock::new(None),
            downloading: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            settings_path,
            settings: Mutex::new(settings),
            subject: Mutex::new(None),
            sam: Mutex::new(None),
            sam_embedding: Mutex::new(None),
            denoiser: Mutex::new(None),
            denoising: Mutex::new(None),
            denoise_cancel: AtomicBool::new(false),
        }
    }
}

pub(crate) fn err(e: impl std::fmt::Display) -> Error {
    Error::Invalid(e.to_string())
}

/// Bibliothek, die beim (nächsten) Start geladen wird.
fn desired_library(state: &AppState) -> Option<(PathBuf, Backend)> {
    let pref = state.ai.settings.lock().unwrap().accel;
    accel::choose_library(&models::ai_dir(&state.data_dir), pref)
}

pub(crate) fn ensure_runtime(state: &AppState) -> Result<(), Error> {
    if runtime::is_loaded() {
        return Ok(());
    }
    let (lib, backend) = desired_library(state)
        .ok_or_else(|| Error::Invalid("ONNX Runtime ist nicht installiert".into()))?;
    let _g = Guard::begin(
        &state.ai.guard_dir,
        Stage::KiLaufzeit,
        Some(backend_id(backend)),
        &lib.display().to_string(),
    );
    runtime::init_with(&lib, backend).map_err(err)?;
    accel::set_use_gpu(state.ai.settings.lock().unwrap().accel != Preference::Cpu);
    Ok(())
}

/// Vergisst alle geladenen Modelle, damit sie mit dem neuen Rechenwerk neu
/// angelegt werden. Laufende Berechnungen behalten ihre Kopie.
/// Leert `slot`, wenn das Modell gerade nirgends benutzt wird.
pub(crate) fn drop_if_unused<T>(slot: &Mutex<Option<Arc<T>>>) -> bool {
    let mut g = slot.lock().unwrap();
    if g.as_ref().is_some_and(|m| Arc::strong_count(m) == 1) {
        *g = None;
        return true;
    }
    false
}

/// Entlädt ungenutzte Modelle: `interactive` = Masken (Motiv, Himmel, Objekt)
/// und Rauschreduzierung, `background` = CLIP (Verschlagwortung).
/// Liefert die Namen der entladenen Modelle.
pub(crate) fn release_models(
    state: &AppState,
    interactive: bool,
    background: bool,
) -> Vec<&'static str> {
    let ai = &state.ai;
    let mut out = Vec::new();
    if interactive {
        if drop_if_unused(&ai.subject) {
            out.push("IS-Net");
        }
        if drop_if_unused(&ai.sam) {
            out.push("SAM");
        }
        // Das SAM-Embedding gehört zum Modell und ist ohne es wertlos
        if ai.sam.lock().unwrap().is_none() {
            *ai.sam_embedding.lock().unwrap() = None;
        }
        if ai.denoising.lock().unwrap().is_none() && drop_if_unused(&ai.denoiser) {
            out.push("DRUNet");
        }
    }
    if background {
        let mut g = ai.loaded.lock().unwrap();
        if g.as_ref()
            .is_some_and(|l| Arc::strong_count(&l.clip) == 1 && Arc::strong_count(&l.tagger) == 1)
        {
            *g = None;
            out.push("CLIP");
        }
    }
    out
}

fn unload_models(state: &AppState) {
    *state.ai.loaded.lock().unwrap() = None;
    *state.ai.load_error.lock().unwrap() = None;
    *state.ai.subject.lock().unwrap() = None;
    *state.ai.sam.lock().unwrap() = None;
    *state.ai.sam_embedding.lock().unwrap() = None;
    *state.ai.denoiser.lock().unwrap() = None;
}

/// Lädt Laufzeit und Modell beim ersten Bedarf.
fn ensure_loaded(state: &AppState) -> Result<Loaded, Error> {
    state.usage.touch(Slot::Background);
    let ai = &state.ai;
    if let Some(l) = ai.loaded.lock().unwrap().clone() {
        return Ok(l);
    }
    let status = models::status(&state.data_dir);
    if !status.model_ready {
        return Err(Error::Invalid("KI-Modell ist nicht installiert".into()));
    }
    ensure_runtime(state)?;
    let result = (|| -> lumenkorn_ai::Result<Loaded> {
        let t = std::time::Instant::now();
        let _g = model_guard(state, "CLIP (Verschlagwortung)");
        let clip = Arc::new(Clip::load(&models::model_dir(&state.data_dir))?);
        let tagger = Arc::new(Tagger::new(&clip, vocab::default_groups())?);
        log::info!("KI-Modell geladen in {:.2?}", t.elapsed());
        Ok(Loaded { clip, tagger })
    })();
    match result {
        Ok(l) => {
            *ai.loaded.lock().unwrap() = Some(l.clone());
            *ai.load_error.lock().unwrap() = None;
            Ok(l)
        }
        Err(e) => {
            *ai.load_error.lock().unwrap() = Some(e.to_string());
            Err(err(e))
        }
    }
}

fn with_index<T>(state: &AppState, f: impl FnOnce(&EmbeddingIndex) -> T) -> Result<T, Error> {
    {
        let guard = state.ai.index.read().unwrap();
        if let Some(idx) = guard.as_ref() {
            return Ok(f(idx));
        }
    }
    let rows = state.catalog.lock().unwrap().embeddings(MODEL_KEY)?;
    let mut idx = EmbeddingIndex::default();
    for (id, bytes) in rows {
        idx.upsert(id, &from_bytes(&bytes));
    }
    let out = f(&idx);
    *state.ai.index.write().unwrap() = Some(idx);
    Ok(out)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AiProgress {
    done: i64,
    total: i64,
    ids: Vec<i64>,
}

/// Analysiert einen Stapel Bilder. `Ok(true)`, wenn noch weitere ausstehen.
pub fn run_batch(app: &AppHandle) -> Result<bool, Error> {
    let state = app.state::<AppState>();
    if !state.ai.settings.lock().unwrap().auto_tag || state.ai.downloading.load(Ordering::Relaxed) {
        return Ok(false);
    }
    if !models::status(&state.data_dir).model_ready || state.ai.load_error.lock().unwrap().is_some()
    {
        return Ok(false);
    }
    let ids = state.catalog.lock().unwrap().pending_ai(BATCH)?;
    if ids.is_empty() {
        return Ok(false);
    }
    let loaded = ensure_loaded(&state)?;

    // Thumbnails (512 px) reichen für CLIP (224 px) völlig.
    let mut images = Vec::new();
    let mut ok_ids = Vec::new();
    let jobs: Vec<_> = {
        let cat = state.catalog.lock().unwrap();
        ids.iter().map(|id| cat.thumb_job(*id)).collect()
    };
    let decoded: Vec<Option<image::DynamicImage>> = {
        use rayon::prelude::*;
        jobs.into_par_iter()
            .map(|job| {
                job.and_then(|j| state.cache.get_or_render(Variant::Thumb, &j))
                    .ok()
                    .and_then(|bytes| image::load_from_memory(&bytes).ok())
            })
            .collect()
    };
    for (id, img) in ids.iter().zip(decoded) {
        match img {
            Some(i) => {
                images.push(i);
                ok_ids.push(*id);
            }
            None => {
                let _ = state.catalog.lock().unwrap().set_ai_failed(*id);
            }
        }
    }

    let embeddings = {
        let _g = Guard::begin(
            &state.ai.guard_dir,
            Stage::KiAnalyse,
            Some(current_backend()),
            "CLIP",
        );
        loaded.clip.embed_images(&images).map_err(err)?
    };
    for (id, emb) in ok_ids.iter().zip(&embeddings) {
        let tags: Vec<(String, f64)> = loaded
            .tagger
            .tag(emb)
            .into_iter()
            .map(|t| (t.tag, t.confidence as f64))
            .collect();
        state
            .catalog
            .lock()
            .unwrap()
            .save_ai_result(*id, MODEL_KEY, &to_bytes(emb), &tags)?;
        if let Some(idx) = state.ai.index.write().unwrap().as_mut() {
            idx.upsert(*id, emb);
        }
    }

    let (done, total) = state.catalog.lock().unwrap().ai_counts()?;
    let _ = app.emit(
        "ai-progress",
        AiProgress {
            done,
            total,
            ids: ok_ids,
        },
    );
    Ok(true)
}

// --- Befehle --------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStatus {
    #[serde(flatten)]
    files: Status,
    downloading: bool,
    loaded: bool,
    error: Option<String>,
    analyzed: i64,
    total: i64,
    settings: AiSettings,
    accel: AccelStatus,
    /// Lumenkorn ist beim letzten Start abgestürzt – was wurde abgeschaltet?
    crash_notice: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccelStatus {
    active: accel::ActiveInfo,
    /// Grafikkarten laut Betriebssystem (nur Anzeige)
    devices: Vec<accel::GpuDevice>,
    /// Für die gewählte Einstellung müsste eine andere Bibliothek geladen werden.
    restart_required: bool,
    /// Bildbearbeitung auf der GPU (wgpu)
    develop: lumenkorn_develop::gpu::GpuStatus,
}

fn accel_status(state: &AppState) -> AccelStatus {
    let restart_required = match (runtime::loaded(), desired_library(state)) {
        (Some((_, loaded)), Some((_, wanted))) => loaded != wanted,
        _ => false,
    };
    AccelStatus {
        active: accel::active(),
        devices: accel::detect_gpus(),
        restart_required,
        develop: lumenkorn_develop::gpu::status(),
    }
}

#[tauri::command(async)]
pub fn ai_status(state: State<AppState>) -> Result<AiStatus, Error> {
    let (analyzed, total) = state.catalog.lock().unwrap().ai_counts()?;
    Ok(AiStatus {
        crash_notice: state.ai.crash_notice.lock().unwrap().clone(),
        accel: accel_status(&state),
        files: models::status(&state.data_dir),
        downloading: state.ai.downloading.load(Ordering::Relaxed),
        loaded: state.ai.loaded.lock().unwrap().is_some(),
        error: state.ai.load_error.lock().unwrap().clone(),
        analyzed,
        total,
        settings: state.ai.settings.lock().unwrap().clone(),
    })
}

#[tauri::command(async)]
pub fn ai_set_settings(state: State<AppState>, settings: AiSettings) -> Result<(), Error> {
    if let Some(dir) = state.ai.settings_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(
        &state.ai.settings_path,
        serde_json::to_string_pretty(&settings)?,
    )?;
    let auto = settings.auto_tag;
    lumenkorn_develop::gpu::set_enabled(settings.develop_gpu);
    if settings.develop_gpu {
        start_develop_gpu(state.ai.guard_dir.clone());
    }
    let accel_changed = state.ai.settings.lock().unwrap().accel != settings.accel;
    let gpu = settings.accel != Preference::Cpu;
    *state.ai.settings.lock().unwrap() = settings;
    if accel_changed && runtime::is_loaded() {
        accel::set_use_gpu(gpu);
        unload_models(&state);
    }
    if auto {
        state.worker.submit(Job::Ai);
    }
    state.worker.submit(Job::Faces);
    Ok(())
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DownloadEvent {
    progress: Option<models::Progress>,
    finished: bool,
    error: Option<String>,
}

/// Lädt fehlende KI-Komponenten im Hintergrund. Fortschritt per `ai-download`.
#[tauri::command(async)]
pub fn ai_download(app: AppHandle, component: Option<Component>) -> Result<(), Error> {
    let component = component.unwrap_or(Component::Clip);
    let state = app.state::<AppState>();
    if state.ai.downloading.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    state.ai.cancel.store(false, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("lumenkorn-download".into())
        .spawn(move || {
            let state = app.state::<AppState>();
            let progress = |p: models::Progress| {
                let _ = app.emit(
                    "ai-download",
                    DownloadEvent {
                        progress: Some(p),
                        finished: false,
                        error: None,
                    },
                );
            };
            let res =
                models::download_missing(&state.data_dir, component, &progress, &state.ai.cancel);
            state.ai.downloading.store(false, Ordering::SeqCst);
            *state.ai.load_error.lock().unwrap() = None;
            let error = match (res, component) {
                (Ok(()), Component::Clip) => ensure_loaded(&state).err().map(|e| e.to_string()),
                (Ok(()), Component::Faces) => {
                    let e = ensure_runtime(&state).err().map(|e| e.to_string());
                    if e.is_none() {
                        state.worker.submit(Job::Faces);
                    }
                    e
                }
                (Ok(()), Component::Masks | Component::Denoise | Component::Repair) => {
                    ensure_runtime(&state).err().map(|e| e.to_string())
                }
                // Ist noch keine Laufzeit geladen, wird die neue beim nächsten
                // Bedarf genommen; sonst zeigt der Status „Neustart nötig“.
                (Ok(()), Component::Webgpu | Component::Directml) => None,
                (Err(e), _) => Some(e.to_string()),
            };
            if error.is_none() && component == Component::Clip {
                state.worker.submit(Job::Ai);
            }
            let _ = app.emit(
                "ai-download",
                DownloadEvent {
                    progress: None,
                    finished: true,
                    error,
                },
            );
        })?;
    Ok(())
}

/// Absturzmeldung gelesen.
#[tauri::command]
pub fn ai_dismiss_notice(state: State<AppState>) {
    *state.ai.crash_notice.lock().unwrap() = None;
}

/// Startet Lumenkorn neu – nötig, wenn eine andere Laufzeitbibliothek geladen
/// werden soll. Laufende Entrauschungen werden vorher abgebrochen.
#[tauri::command]
pub fn app_restart(app: AppHandle) {
    let state = app.state::<AppState>();
    state.ai.denoise_cancel.store(true, Ordering::SeqCst);
    state.ai.cancel.store(true, Ordering::SeqCst);
    clear_crash_markers(&state);
    app.restart();
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Benchmark {
    gpu: bool,
    backend: String,
    /// Kantenlänge des Testbildes
    size: usize,
    /// Rechenzeit in Sekunden (ohne ersten, aufwärmenden Durchlauf)
    seconds: f64,
    /// Hochgerechnet auf ein 24-Megapixel-Bild
    seconds_24mp: f64,
}

/// Misst die Rauschreduzierung an einem synthetischen Bild – zeigt, ob und wie
/// schnell die GPU rechnet.
#[tauri::command]
pub async fn ai_benchmark(app: AppHandle) -> Result<Benchmark, Error> {
    spawn(app, |state| {
        let d = denoiser(state)?;
        let size = 512usize;
        let data: Vec<[f32; 3]> = (0..size * size)
            .map(|i| {
                let (x, y) = ((i % size) as f32, (i / size) as f32);
                let n = ((i.wrapping_mul(2654435761)) % 1000) as f32 / 1000.0 - 0.5;
                let v = 0.2 + 0.15 * (x / 37.0).sin() * (y / 23.0).cos() + 0.05 * n;
                [v, v * 0.9, v * 1.1]
            })
            .collect();
        let img = lumenkorn_ai::denoise::Rgb {
            width: size,
            height: size,
            data: &data,
        };
        let never = AtomicBool::new(false);
        let _g = analysis_guard(state, "Geschwindigkeitstest");
        // Erster Lauf: Shader übersetzen, Speicher anlegen
        d.denoise(&img, 1.0, &|_, _| {}, &never).map_err(err)?;
        let t = std::time::Instant::now();
        d.denoise(&img, 1.0, &|_, _| {}, &never).map_err(err)?;
        let seconds = t.elapsed().as_secs_f64();
        let backend = match runtime::loaded() {
            Some((_, b)) if d.on_gpu() => b.label().to_string(),
            _ => "CPU".to_string(),
        };
        Ok(Benchmark {
            gpu: d.on_gpu(),
            backend,
            size,
            seconds,
            seconds_24mp: seconds * 24.0e6 / (size * size) as f64,
        })
    })
    .await
}

#[tauri::command]
pub fn ai_cancel_download(state: State<AppState>) {
    state.ai.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command(async)]
pub fn ai_retag_all(state: State<AppState>) -> Result<(), Error> {
    state.catalog.lock().unwrap().reset_ai()?;
    *state.ai.load_error.lock().unwrap() = None;
    state.worker.submit(Job::Ai);
    Ok(())
}

/// Semantische Suche: Bilder nach Ähnlichkeit zur Beschreibung sortiert.
#[tauri::command]
pub async fn semantic_search(
    app: AppHandle,
    query: String,
    scope: Scope,
    min_rating: Option<i64>,
) -> Result<Vec<ImageSummary>, Error> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let loaded = ensure_loaded(&state)?;
        let english = vocab::translate_query(&query);
        let q = loaded
            .clip
            .embed_texts(&[english.as_str()])
            .map_err(err)?
            .remove(0);
        // Kosinus-Werte von CLIP liegen eng beieinander; relativ zum besten
        // Treffer filtern ist robuster als eine feste Schwelle.
        let hits = with_index(&state, |idx| idx.search(&q, 2000, -1.0))?;
        let best = hits.first().map(|h| h.1).unwrap_or(0.0);
        let ids: Vec<i64> = hits
            .iter()
            .take_while(|(_, s)| *s >= best - 0.08)
            .map(|h| h.0)
            .collect();
        log::info!(
            "Semantische Suche „{query}“ → „{english}“: {} Treffer",
            ids.len()
        );
        state
            .catalog
            .lock()
            .unwrap()
            .images_in_order(&ids, &scope, min_rating)
    })
    .await
    .map_err(err)?
}

// --- KI-Masken --------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaskResult {
    mask_id: String,
    /// Anteil der Bildfläche, 0…1 – 0 heißt „nichts gefunden“
    coverage: f32,
}

/// Quellbild für die Segmentierung: unbearbeitet entwickelt, Originalausrichtung
/// (Masken beziehen sich auf das aufrechte Original, nicht auf Drehung/Zuschnitt).
fn segmentation_input(state: &AppState, id: i64) -> Result<image::DynamicImage, Error> {
    let src = crate::develop::load_proxy(state, id, segment::MASK_EDGE as usize)?;
    let img = lumenkorn_develop::render(&src, &DevelopParams::default());
    Ok(image::DynamicImage::ImageRgb8(
        lumenkorn_develop::output::to_rgb8(&img),
    ))
}

fn masks_needed(state: &AppState) -> Result<(), Error> {
    if !models::status(&state.data_dir).masks_ready {
        return Err(Error::Invalid(
            "Die KI-Masken sind noch nicht installiert. Lade sie über „KI“ unten links.".into(),
        ));
    }
    ensure_runtime(state)
}

fn subject_model(state: &AppState) -> Result<Arc<SubjectModel>, Error> {
    state.usage.touch(Slot::Interactive);
    masks_needed(state)?;
    let mut guard = state.ai.subject.lock().unwrap();
    if let Some(m) = guard.as_ref() {
        return Ok(m.clone());
    }
    let _g = model_guard(state, "IS-Net (Motiv)");
    let m = Arc::new(
        SubjectModel::load(&models::masks_dir(&state.data_dir).join("subject.onnx"))
            .map_err(err)?,
    );
    *guard = Some(m.clone());
    Ok(m)
}

fn sam_model(state: &AppState) -> Result<Arc<Sam>, Error> {
    state.usage.touch(Slot::Interactive);
    masks_needed(state)?;
    let mut guard = state.ai.sam.lock().unwrap();
    if let Some(m) = guard.as_ref() {
        return Ok(m.clone());
    }
    let dir = models::masks_dir(&state.data_dir);
    let _g = model_guard(state, "Segment Anything (Objekt)");
    let m = Arc::new(
        Sam::load(&dir.join("sam-encoder.onnx"), &dir.join("sam-decoder.onnx")).map_err(err)?,
    );
    *guard = Some(m.clone());
    Ok(m)
}

fn sam_embedding(state: &AppState, sam: &Sam, id: i64) -> Result<Arc<SamEmbedding>, Error> {
    if let Some((cached, e)) = state.ai.sam_embedding.lock().unwrap().as_ref()
        && *cached == id
    {
        return Ok(e.clone());
    }
    let t = std::time::Instant::now();
    let _g = analysis_guard(state, "Segment Anything");
    let e = Arc::new(sam.embed(&segmentation_input(state, id)?).map_err(err)?);
    log::info!("SAM-Embedding für Bild {id} in {:.2?}", t.elapsed());
    *state.ai.sam_embedding.lock().unwrap() = Some((id, e.clone()));
    Ok(e)
}

fn store(
    state: &AppState,
    id: Option<String>,
    image_id: i64,
    kind: &str,
    mask: &image::GrayImage,
) -> Result<MaskResult, Error> {
    // Nur eigene Masken überschreiben – nie die eines anderen Fotos
    let mask_id = id
        .filter(|i| state.masks.path(i).is_some() && i.starts_with(&format!("{image_id}-")))
        .unwrap_or_else(|| MaskFiles::new_id(image_id, kind));
    state.masks.put(&mask_id, mask)?;
    let coverage = mask.pixels().filter(|p| p[0] > 127).count() as f32
        / (mask.width() * mask.height()).max(1) as f32;
    Ok(MaskResult { mask_id, coverage })
}

async fn spawn<T: Send + 'static>(
    app: AppHandle,
    f: impl FnOnce(&AppState) -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    tauri::async_runtime::spawn_blocking(move || f(&app.state::<AppState>()))
        .await
        .map_err(err)?
}

/// „Motiv auswählen“
#[tauri::command]
pub async fn mask_subject(app: AppHandle, id: i64) -> Result<MaskResult, Error> {
    spawn(app, move |state| {
        let model = subject_model(state)?;
        let t = std::time::Instant::now();
        let _g = analysis_guard(state, "Motiv");
        let mask = model
            .predict(&segmentation_input(state, id)?)
            .map_err(err)?;
        log::info!("Motivmaske für Bild {id} in {:.2?}", t.elapsed());
        store(state, None, id, "subject", &mask)
    })
    .await
}

/// „Himmel auswählen“. Nutzt SAM zur Verfeinerung, wenn installiert.
#[tauri::command]
pub async fn mask_sky(app: AppHandle, id: i64) -> Result<MaskResult, Error> {
    spawn(app, move |state| {
        let input = segmentation_input(state, id)?;
        let sam = models::status(&state.data_dir)
            .masks_ready
            .then(|| sam_model(state))
            .transpose()?;
        let mask = match &sam {
            Some(s) => {
                let emb = sam_embedding(state, s, id)?;
                segment::sky_mask(&input, Some((s, &emb))).map_err(err)?
            }
            None => segment::sky_mask(&input, None).map_err(err)?,
        };
        store(state, None, id, "sky", &mask)
    })
    .await
}

/// Objekt per Klick. Mit `mask_id` wird eine vorhandene Maske mit den neuen
/// Punkten überschrieben (Punkte nachsetzen, um die Auswahl zu verfeinern).
#[tauri::command]
pub async fn mask_object(
    app: AppHandle,
    id: i64,
    points: Vec<PromptPoint>,
    mask_id: Option<String>,
) -> Result<MaskResult, Error> {
    spawn(app, move |state| {
        let sam = sam_model(state)?;
        let emb = sam_embedding(state, &sam, id)?;
        let pts: Vec<segment::Point> = points
            .iter()
            .map(|p| segment::Point {
                x: p.x,
                y: p.y,
                positive: p.positive,
            })
            .collect();
        let mask = {
            let _g = analysis_guard(state, "Segment Anything");
            sam.predict(&emb, &pts).map_err(err)?
        };
        store(state, mask_id, id, "object", &mask)
    })
    .await
}

// --- Rauschreduzierung --------------------------------------------------------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DenoiseEvent {
    image_id: i64,
    done: usize,
    total: usize,
    finished: bool,
    error: Option<String>,
    /// Datei-ID des Ergebnisses (bei Erfolg)
    denoise_id: Option<String>,
}

fn denoiser(state: &AppState) -> Result<Arc<lumenkorn_ai::denoise::Denoiser>, Error> {
    state.usage.touch(Slot::Interactive);
    if !models::status(&state.data_dir).denoise_ready {
        return Err(Error::Invalid(
            "Die KI-Rauschreduzierung ist noch nicht installiert. Lade sie über „KI“ unten links."
                .into(),
        ));
    }
    ensure_runtime(state)?;
    let mut g = state.ai.denoiser.lock().unwrap();
    if let Some(d) = g.as_ref() {
        return Ok(d.clone());
    }
    let path = models::denoise_dir(&state.data_dir).join(models::DENOISE_MODEL);
    let _g = model_guard(state, "DRUNet (Rauschreduzierung)");
    let d = Arc::new(lumenkorn_ai::denoise::Denoiser::load(&path).map_err(err)?);
    *g = Some(d.clone());
    Ok(d)
}

/// Entrauscht ein Bild in voller Auflösung im Hintergrund. Fortschritt und
/// Ergebnis kommen als `denoise-progress`. `strength` skaliert die geschätzte
/// Rauschstärke (1,0 = automatisch).
#[tauri::command(async)]
pub fn denoise_start(app: AppHandle, id: i64, strength: Option<f32>) -> Result<(), Error> {
    let state = app.state::<AppState>();
    {
        let mut running = state.ai.denoising.lock().unwrap();
        if running.is_some() {
            return Err(Error::Invalid(
                "Es wird bereits ein Foto entrauscht.".into(),
            ));
        }
        *running = Some(id);
    }
    state.ai.denoise_cancel.store(false, Ordering::SeqCst);
    // Reglerwert „Stärke“ (0–100) → Faktor auf die geschätzte Rauschstärke
    let strength = lumenkorn_ai::denoise::strength_factor(strength.unwrap_or(50.0));
    std::thread::Builder::new()
        .name("lumenkorn-denoise".into())
        .spawn(move || {
            let state = app.state::<AppState>();
            let emit =
                |done, total, finished, error: Option<String>, denoise_id: Option<String>| {
                    let _ = app.emit(
                        "denoise-progress",
                        DenoiseEvent {
                            image_id: id,
                            done,
                            total,
                            finished,
                            error,
                            denoise_id,
                        },
                    );
                };
            let res = (|| -> Result<String, Error> {
                let d = denoiser(&state)?;
                let job = state.catalog.lock().unwrap().thumb_job(id)?;
                let t = std::time::Instant::now();
                let src = Source::load(
                    StdPath::new(&job.path),
                    job.kind == lumenkorn_core::formats::ImageKind::Raw,
                    job.orientation.map(|o| o as u16),
                    None,
                )?;
                let img = &src.image;
                let _g = analysis_guard(&state, "Rauschreduzierung");
                let out = d
                    .denoise(
                        &lumenkorn_ai::denoise::Rgb {
                            width: img.width,
                            height: img.height,
                            data: &img.data,
                        },
                        strength,
                        &|done, total| emit(done, total, false, None, None),
                        &state.ai.denoise_cancel,
                    )
                    .map_err(err)?;
                let denoise_id = MaskFiles::new_id(id, "dn");
                std::fs::create_dir_all(&state.denoised_dir)?;
                let path = lumenkorn_develop::denoised::path(&state.denoised_dir, &denoise_id)
                    .ok_or_else(|| Error::Invalid("ungültige ID".into()))?;
                lumenkorn_develop::denoised::write(&path, img.width, img.height, &out)?;
                log::info!(
                    "Bild {id} entrauscht in {:.1?} ({}×{})",
                    t.elapsed(),
                    img.width,
                    img.height
                );
                Ok(denoise_id)
            })();
            *state.ai.denoising.lock().unwrap() = None;
            match res {
                Ok(did) => emit(1, 1, true, None, Some(did)),
                Err(e) => emit(0, 0, true, Some(e.to_string()), None),
            }
        })?;
    Ok(())
}

#[tauri::command]
pub fn denoise_cancel(state: State<AppState>) {
    state.ai.denoise_cancel.store(true, Ordering::SeqCst);
}
