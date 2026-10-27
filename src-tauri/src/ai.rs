//! KI im Hintergrund: Modell laden, Bilder analysieren, semantisch suchen,
//! Modelle herunterladen.

use crate::guard::{self, Guard, Stage};
use crate::memory::Slot;
use crate::worker::Job;
use crate::{AppState, blocking};
use lumenkorn_ai::accel::{self, Backend, Preference};
use lumenkorn_ai::index::{from_bytes, to_bytes};
use lumenkorn_ai::models::{self, Component, Status};
use lumenkorn_ai::parts;
use lumenkorn_ai::segment::{self, Sam, SamEmbedding, SkyModel, SubjectModel};
use lumenkorn_ai::{Clip, EmbeddingIndex, Tagger, runtime, vocab};
use lumenkorn_core::LockExt;
use lumenkorn_core::maskstore::MaskFiles;
use lumenkorn_core::thumbs::Variant;
use lumenkorn_core::{Error, ImageQuery, ImageSummary, Scope};
use lumenkorn_develop::i18n::tr_s;
use lumenkorn_develop::{DevelopParams, PromptPoint, Source, tr};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tauri::{AppHandle, Emitter, Manager, State};

/// Kennung, unter der Embeddings im Katalog stehen. Ein anderes Modell bekommt
/// eine andere Kennung, damit nie Vektoren verschiedener Modelle gemischt werden.
const MODEL_KEY: &str = models::MODEL_ID;
const BATCH: usize = 16;
/// Höchstens so viele Treffer der semantischen Suche, bevor relativ zum besten
/// gefiltert wird: deckt auch große Ordner ab, hält das Sortieren aber billig.
const SEARCH_MAX_HITS: usize = 2000;
/// Treffer zählen, solange ihr Kosinus-Wert höchstens so weit unter dem
/// besten liegt (CLIP-Werte liegen eng beieinander, siehe `semantic_search`).
const SEARCH_MARGIN: f32 = 0.08;

pub struct AiState {
    loaded: Mutex<Option<Loaded>>,
    /// Hält, wer CLIP gerade lädt: Worker, Suche und Einstellungen luden es
    /// sonst gleichzeitig (doppelte Zeit und doppelter Speicher)
    loading: Mutex<()>,
    /// Letzter Ladefehler, damit nicht jeder Stapel es erneut versucht.
    load_error: Mutex<Option<String>>,
    index: RwLock<Option<EmbeddingIndex>>,
    pub(crate) downloading: AtomicBool,
    /// Gewünschte Downloads: Sie laufen nacheinander, ein weiterer Klick
    /// während eines Downloads reiht sich ein
    downloads: Mutex<Downloads>,
    cancel: AtomicBool,
    settings_path: PathBuf,
    pub(crate) settings: Mutex<AiSettings>,
    subject: Mutex<Option<Arc<SubjectModel>>>,
    sam: Mutex<Option<Arc<Sam>>>,
    sky: Mutex<Option<Arc<SkyModel>>>,
    /// Personen und Tiere: Detektor, Teil-Segmentierung, Gesichtsnetz
    owl: Mutex<Option<Arc<parts::Owl>>>,
    parts_seg: Mutex<Option<Arc<parts::PartsSeg>>>,
    mesh: Mutex<Option<Arc<parts::FaceMesh>>>,
    /// SAM-Embedding des zuletzt geklickten Bildes – das Berechnen dauert Sekunden.
    sam_embedding: Mutex<Option<(SamKey, Arc<SamEmbedding>)>>,
    /// OWLv2-Funde und Gesichtspunkte des zuletzt bearbeiteten Bildes: Teile wie
    /// Haare, Augen, Lippen … nacheinander brauchen sie nicht jedes Mal neu.
    detect_cache: Mutex<Option<(SamKey, parts::DetectCache)>>,
    denoiser: Mutex<Option<Arc<lumenkorn_ai::denoise::Denoiser>>>,
    /// Es wird gerade ein Bild entrauscht (hält DRUNet geladen)
    denoising: AtomicBool,
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
    /// „Auto“ lernt aus den Bearbeitungen (siehe `style`)
    pub learn_style: bool,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            auto_tag: true,
            accel: Preference::Auto,
            develop_gpu: true,
            faces: true,
            learn_style: true,
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
        .map(|p| tr!(" Details stehen in {}.", " Details are in {}.", p.display()))
        .unwrap_or_default();
    let mut notes = Vec::new();
    for c in guard::take_previous(ai_dir) {
        let what = match c.stage {
            Stage::KiLaufzeit => tr_s("beim Laden der KI-Laufzeit", "while loading the AI runtime"),
            Stage::KiModell => tr_s("beim Öffnen eines KI-Modells", "while opening an AI model"),
            Stage::KiAnalyse => tr_s("beim Analysieren der Fotos", "while analyzing photos"),
            Stage::GpuBearbeitung => tr_s(
                "beim Öffnen der Grafikkarte für die Bildbearbeitung",
                "while opening the GPU for image editing",
            ),
        };
        if c.stage == Stage::GpuBearbeitung {
            settings.develop_gpu = false;
            notes.push(tr!(
                "Lumenkorn ist beim letzten Start {what} abgestürzt. Die Bildbearbeitung rechnet \
                 jetzt auf dem Prozessor; im KI-Dialog unter „Grafikkarte“ lässt sie sich wieder \
                 einschalten.",
                "Lumenkorn crashed {what} during the last launch. Image editing now runs on the \
                 CPU; you can turn the GPU back on in the AI dialog under “GPU”."
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
                notes.push(tr!(
                    "Lumenkorn ist beim letzten Start {what} mit der Grafikkarte ({}) \
                     abgestürzt. Die GPU-Laufzeit wurde deaktiviert, die KI rechnet jetzt auf dem \
                     Prozessor.{log}",
                    "Lumenkorn crashed {what} on the GPU ({}) during the last launch. The GPU \
                     runtime has been disabled; AI now runs on the CPU.{log}",
                    b.label()
                ));
            }
            _ => {
                settings.auto_tag = false;
                notes.push(tr!(
                    "Lumenkorn ist beim letzten Start {what} abgestürzt ({}). Die automatische \
                     Verschlagwortung ist pausiert – im KI-Dialog lässt sie sich wieder \
                     einschalten.{log}",
                    "Lumenkorn crashed {what} during the last launch ({}). Automatic keywording \
                     is paused – you can turn it back on in the AI dialog.{log}",
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
            loading: Mutex::new(()),
            load_error: Mutex::new(None),
            index: RwLock::new(None),
            downloading: AtomicBool::new(false),
            downloads: Mutex::new(Downloads::default()),
            cancel: AtomicBool::new(false),
            settings_path,
            settings: Mutex::new(settings),
            subject: Mutex::new(None),
            sam: Mutex::new(None),
            sky: Mutex::new(None),
            owl: Mutex::new(None),
            parts_seg: Mutex::new(None),
            mesh: Mutex::new(None),
            sam_embedding: Mutex::new(None),
            detect_cache: Mutex::new(None),
            denoiser: Mutex::new(None),
            denoising: AtomicBool::new(false),
            denoise_cancel: AtomicBool::new(false),
        }
    }
}

/// Lädt ein Modell beim ersten Bedarf in `slot`; danach teilen sich alle
/// Aufrufer dieselbe Kopie. Die Slot-Sperre gilt während des Ladens, damit
/// gleichzeitige Aufrufe es nicht doppelt laden. `label` erscheint in der
/// Absturzmeldung, falls das Öffnen des Modells den Prozess mitreißt.
pub(crate) fn lazy_model<T>(
    state: &AppState,
    slot: &Mutex<Option<Arc<T>>>,
    label: &str,
    load: impl FnOnce() -> Result<T, Error>,
) -> Result<Arc<T>, Error> {
    let mut g = lock_slot(slot);
    if let Some(m) = g.as_ref() {
        return Ok(m.clone());
    }
    ensure_runtime(state)?;
    let _guard = model_guard(state, label);
    let m = Arc::new(load()?);
    *g = Some(m.clone());
    Ok(m)
}

pub(crate) fn err(e: impl std::fmt::Display) -> Error {
    Error::Invalid(e.to_string())
}

/// Bibliothek, die beim (nächsten) Start geladen wird.
fn desired_library(state: &AppState) -> Option<(PathBuf, Backend)> {
    let pref = state.ai.settings.locked().accel;
    accel::choose_library(&models::ai_dir(&state.data_dir), pref)
}

pub(crate) fn ensure_runtime(state: &AppState) -> Result<(), Error> {
    if runtime::is_loaded() {
        return Ok(());
    }
    let (lib, backend) = desired_library(state).ok_or_else(|| {
        Error::Invalid(
            tr_s(
                "ONNX Runtime ist nicht installiert",
                "ONNX Runtime is not installed",
            )
            .into(),
        )
    })?;
    let _g = Guard::begin(
        &state.ai.guard_dir,
        Stage::KiLaufzeit,
        Some(backend_id(backend)),
        &lib.display().to_string(),
    );
    runtime::init_with(&lib, backend).map_err(err)?;
    accel::set_use_gpu(state.ai.settings.locked().accel != Preference::Cpu);
    Ok(())
}

/// Sperrt einen Modell-Slot. Ist die Sperre vergiftet (ein Ladeversuch ist
/// abgestürzt), gilt der Slot als leer und wird beim nächsten Zugriff neu
/// geladen, statt dass jede weitere Nutzung ebenfalls paniken würde.
fn lock_slot<T>(slot: &Mutex<Option<Arc<T>>>) -> std::sync::MutexGuard<'_, Option<Arc<T>>> {
    match slot.lock() {
        Ok(g) => g,
        Err(e) => {
            log::warn!("Modell-Slot war vergiftet, wird neu geladen");
            let mut g = e.into_inner();
            *g = None;
            slot.clear_poison();
            g
        }
    }
}

/// Leert `slot`, wenn das Modell gerade nirgends benutzt wird.
pub(crate) fn drop_if_unused<T>(slot: &Mutex<Option<Arc<T>>>) -> bool {
    let mut g = lock_slot(slot);
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
        if drop_if_unused(&ai.sky) {
            out.push("skyseg");
        }
        if drop_if_unused(&ai.owl) {
            out.push("OWLv2");
        }
        if drop_if_unused(&ai.parts_seg) {
            out.push("Selfie Multiclass");
        }
        if drop_if_unused(&ai.mesh) {
            out.push("Face Mesh");
        }
        // Das SAM-Embedding gehört zum Modell und ist ohne es wertlos
        if ai.sam.locked().is_none() {
            *ai.sam_embedding.locked() = None;
        }
        // Die Funde gehören zu OWLv2 (und dem Face Mesh)
        if ai.owl.locked().is_none() || ai.mesh.locked().is_none() {
            *ai.detect_cache.locked() = None;
        }
        if !ai.denoising.load(Ordering::SeqCst) && drop_if_unused(&ai.denoiser) {
            out.push("DRUNet");
        }
    }
    if background {
        let mut g = ai.loaded.locked();
        if g.as_ref()
            .is_some_and(|l| Arc::strong_count(&l.clip) == 1 && Arc::strong_count(&l.tagger) == 1)
        {
            *g = None;
            out.push("CLIP");
        }
    }
    out
}

/// Vergisst alle geladenen Modelle, damit sie mit dem neuen Rechenwerk neu
/// angelegt werden. Laufende Berechnungen behalten ihre Kopie.
fn unload_models(state: &AppState) {
    *state.ai.loaded.locked() = None;
    *state.ai.load_error.locked() = None;
    *state.ai.subject.locked() = None;
    *state.ai.sam.locked() = None;
    *state.ai.sky.locked() = None;
    *state.ai.owl.locked() = None;
    *state.ai.parts_seg.locked() = None;
    *state.ai.mesh.locked() = None;
    *state.ai.sam_embedding.locked() = None;
    *state.ai.detect_cache.locked() = None;
    *state.ai.denoiser.locked() = None;
}

/// Lädt Laufzeit und Modell beim ersten Bedarf.
fn ensure_loaded(state: &AppState) -> Result<Loaded, Error> {
    state.usage.touch(Slot::Background);
    let ai = &state.ai;
    if let Some(l) = ai.loaded.locked().clone() {
        return Ok(l);
    }
    let _loading = ai.loading.lock().unwrap_or_else(|e| e.into_inner());
    // Inzwischen von einem anderen Aufrufer geladen
    if let Some(l) = ai.loaded.locked().clone() {
        return Ok(l);
    }
    let status = models::status(&state.data_dir);
    if !status.model_ready {
        return Err(Error::Invalid(
            tr_s(
                "KI-Modell ist nicht installiert",
                "AI model is not installed",
            )
            .into(),
        ));
    }
    ensure_runtime(state)?;
    let result = (|| -> lumenkorn_ai::Result<Loaded> {
        let t = std::time::Instant::now();
        let _g = model_guard(state, tr_s("CLIP (Verschlagwortung)", "CLIP (keywording)"));
        let clip = Arc::new(Clip::load(&models::model_dir(&state.data_dir))?);
        let tagger = Arc::new(Tagger::new(&clip, vocab::default_groups())?);
        log::info!("KI-Modell geladen in {:.2?}", t.elapsed());
        Ok(Loaded { clip, tagger })
    })();
    match result {
        Ok(l) => {
            *ai.loaded.locked() = Some(l.clone());
            *ai.load_error.locked() = None;
            Ok(l)
        }
        Err(e) => {
            *ai.load_error.locked() = Some(e.to_string());
            Err(err(e))
        }
    }
}

/// Führt `f` mit dem Suchindex aus; baut ihn bei der ersten Suche auf.
///
/// Der Aufbau läuft unter der Schreibsperre des Index: Zwei gleichzeitige
/// Suchen bauen ihn so nur einmal (die zweite wartet und nimmt das Ergebnis),
/// und Änderungen während des Aufbaus gehen nicht verloren – `upsert` und
/// `forget_in_index` warten auf die Sperre und wirken danach auf den fertigen
/// Index (was sie vorher in den Katalog geschrieben haben, liest der Aufbau
/// ohnehin schon mit). Die Katalogsperre gilt nur fürs Lesen der Zeilen, das
/// Dekodieren der ~100 MB Vektoren läuft ohne sie. Reihenfolge der Sperren:
/// Index → Katalog; umgekehrt hält niemand den Katalog, während er den Index sperrt.
fn with_index<T>(state: &AppState, f: impl FnOnce(&EmbeddingIndex) -> T) -> Result<T, Error> {
    {
        let guard = state
            .ai
            .index
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(idx) = guard.as_ref() {
            return Ok(f(idx));
        }
    }
    let mut guard = state
        .ai
        .index
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Hat eine parallele Suche ihn inzwischen gebaut, nur noch benutzen
    let idx = match guard.take() {
        Some(idx) => idx,
        None => {
            let rows = state.catalog.locked().embeddings(MODEL_KEY)?;
            let mut idx = EmbeddingIndex::default();
            for (id, bytes) in rows {
                idx.upsert(id, &from_bytes(&bytes));
            }
            idx
        }
    };
    Ok(f(guard.insert(idx)))
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
    if !state.ai.settings.locked().auto_tag || state.ai.downloading.load(Ordering::Relaxed) {
        return Ok(false);
    }
    if !models::status(&state.data_dir).model_ready || state.ai.load_error.locked().is_some() {
        return Ok(false);
    }
    let ids = state.catalog.locked().pending_ai(BATCH)?;
    if ids.is_empty() {
        return Ok(false);
    }
    let loaded = ensure_loaded(&state)?;

    // Thumbnails (512 px) reichen für CLIP (224 px) völlig.
    let mut images = Vec::new();
    let mut ok_ids = Vec::new();
    let jobs: Vec<_> = {
        let cat = state.catalog.locked();
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
                let _ = state.catalog.locked().set_ai_failed(*id);
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
            .locked()
            .save_ai_result(*id, MODEL_KEY, &to_bytes(emb), &tags)?;
        if let Some(idx) = state
            .ai
            .index
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            idx.upsert(*id, emb);
        }
    }

    let (done, total) = state.catalog.locked().ai_counts()?;
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
    let (analyzed, total) = state.catalog.locked().ai_counts()?;
    Ok(AiStatus {
        crash_notice: state.ai.crash_notice.locked().clone(),
        accel: accel_status(&state),
        files: models::status(&state.data_dir),
        downloading: state.ai.downloading.load(Ordering::Relaxed),
        loaded: state.ai.loaded.locked().is_some(),
        error: state.ai.load_error.locked().clone(),
        analyzed,
        total,
        settings: state.ai.settings.locked().clone(),
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
    let accel_changed = state.ai.settings.locked().accel != settings.accel;
    let gpu = settings.accel != Preference::Cpu;
    *state.ai.settings.locked() = settings;
    if accel_changed && runtime::is_loaded() {
        accel::set_use_gpu(gpu);
        unload_models(&state);
    }
    if auto {
        state.worker.submit(Job::Ai);
    }
    state.worker.submit(Job::Faces);
    crate::style::settings_changed(&state);
    Ok(())
}

#[derive(Default)]
struct Downloads {
    current: Option<Component>,
    queue: std::collections::VecDeque<Component>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DownloadEvent {
    progress: Option<models::Progress>,
    /// Was gerade geladen wird
    component: Option<Component>,
    /// Was danach noch drankommt
    queued: Vec<Component>,
    /// `component` ist fertig (mit `error` gescheitert)
    finished: bool,
    error: Option<String>,
}

fn download_event(state: &AppState, progress: Option<models::Progress>) -> DownloadEvent {
    let d = state.ai.downloads.locked();
    DownloadEvent {
        progress,
        component: d.current,
        queued: d.queue.iter().copied().collect(),
        finished: false,
        error: None,
    }
}

/// Entfernt gelöschte Fotos aus dem Suchindex der semantischen Suche (`None`:
/// Index verwerfen, er wird bei der nächsten Suche neu aufgebaut). Sonst
/// belegten sie bis zum Neustart Speicher und Plätze unter den Treffern.
pub(crate) fn forget_in_index(state: &AppState, ids: Option<&[i64]>) {
    let mut index = state
        .ai
        .index
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match (ids, index.as_mut()) {
        (Some(ids), Some(idx)) => ids.iter().for_each(|id| idx.remove(*id)),
        (None, _) => *index = None,
        (Some(_), None) => {}
    }
}

/// Lädt fehlende KI-Komponenten im Hintergrund. Fortschritt per `ai-download`.
/// Läuft schon ein Download, kommt `component` danach dran.
#[tauri::command(async)]
pub fn ai_download(app: AppHandle, component: Option<Component>) -> Result<(), Error> {
    let component = component.unwrap_or(Component::Clip);
    let state = app.state::<AppState>();
    // Einreihen und Starten unter derselben Sperre, unter der der Download-
    // Thread bei leerer Warteschlange endet – so geht kein Wunsch verloren.
    let mut downloads = state.ai.downloads.locked();
    if downloads.current != Some(component) && !downloads.queue.contains(&component) {
        downloads.queue.push_back(component);
    }
    // Setzt `downloading` auch zurück, wenn der Download-Thread panisch endet
    // oder gar nicht erst startet – sonst blieben Analyse und Gesichter-
    // erkennung bis zum Neustart blockiert.
    let Some(done) = crate::busy::claim_in(&app, |s| &s.ai.downloading) else {
        drop(downloads);
        let _ = app.emit("ai-download", download_event(&state, None));
        return Ok(());
    };
    drop(downloads);
    std::thread::Builder::new()
        .name("lumenkorn-download".into())
        .spawn(move || {
            let state = app.state::<AppState>();
            loop {
                let component = {
                    let mut d = state.ai.downloads.locked();
                    let next = d.queue.pop_front();
                    d.current = next;
                    match next {
                        Some(c) => c,
                        None => {
                            drop(done);
                            return;
                        }
                    }
                };
                state.ai.cancel.store(false, Ordering::SeqCst);
                let _ = app.emit("ai-download", download_event(&state, None));
                let error = download_one(&app, &state, component);
                let _ = app.emit(
                    "ai-download",
                    DownloadEvent {
                        finished: true,
                        error,
                        ..download_event(&state, None)
                    },
                );
            }
        })?;
    Ok(())
}

/// Ein Download samt Laden der Modelle; liefert die Fehlermeldung.
fn download_one(app: &AppHandle, state: &AppState, component: Component) -> Option<String> {
    let progress = |p: models::Progress| {
        let _ = app.emit("ai-download", download_event(state, Some(p)));
    };
    // Ein Panic im Download soll als Fehler enden, nicht als ewiges „läuft“
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        models::download_missing(&state.data_dir, component, &progress, &state.ai.cancel)
    }))
    .unwrap_or_else(|_| {
        Err(lumenkorn_ai::Error::Download(
            tr_s(
                "interner Fehler beim Herunterladen",
                "internal error while downloading",
            )
            .into(),
        ))
    });
    *state.ai.load_error.locked() = None;
    let error = match (res, component) {
        (Ok(()), Component::Clip) => ensure_loaded(state).err().map(|e| e.to_string()),
        (Ok(()), Component::Faces) => {
            let e = ensure_runtime(state).err().map(|e| e.to_string());
            if e.is_none() {
                state.worker.submit(Job::Faces);
            }
            e
        }
        (Ok(()), Component::Masks | Component::Denoise | Component::Repair) => {
            ensure_runtime(state).err().map(|e| e.to_string())
        }
        // Ist noch keine Laufzeit geladen, wird die neue beim nächsten
        // Bedarf genommen; sonst zeigt der Status „Neustart nötig“.
        (Ok(()), Component::Webgpu) => None,
        (Err(e), _) => Some(e.to_string()),
    };
    if error.is_none() && component == Component::Clip {
        state.worker.submit(Job::Ai);
    }
    error
}

/// Absturzmeldung gelesen.
#[tauri::command]
pub fn ai_dismiss_notice(state: State<AppState>) {
    *state.ai.crash_notice.locked() = None;
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
    blocking(app, |state| {
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
        let _g = analysis_guard(state, tr_s("Geschwindigkeitstest", "Speed test"));
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
    // Auch alles Eingereihte – „Abbrechen“ meint den ganzen Download
    state.ai.downloads.locked().queue.clear();
    state.ai.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command(async)]
pub fn ai_retag_all(state: State<AppState>) -> Result<(), Error> {
    state.catalog.locked().reset_ai()?;
    *state.ai.load_error.locked() = None;
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
    exts: Option<Vec<String>>,
    edited: Option<bool>,
) -> Result<Vec<ImageSummary>, Error> {
    blocking(app, move |state| {
        let loaded = ensure_loaded(state)?;
        let english = vocab::translate_query(&query);
        let q = loaded
            .clip
            .embed_texts(&[english.as_str()])
            .map_err(err)?
            .remove(0);
        // Erst den Bereich (Ordner/Projekt/Person, Mindestbewertung, Dateityp) bestimmen
        // und nur darin suchen: Sonst füllen Treffer aus dem übrigen Katalog
        // die SEARCH_MAX_HITS Plätze und setzen `best`, und ein kleiner Ordner findet
        // nichts. Nebenbei fallen gelöschte oder fehlende Fotos heraus, die
        // noch im Index stehen.
        let mut sq = ImageQuery::new(scope);
        sq.min_rating = min_rating;
        sq.exts = exts;
        sq.edited = edited;
        let allowed: HashSet<i64> = state.catalog.locked().query_image_ids(&sq)?;
        // Kosinus-Werte von CLIP liegen eng beieinander; relativ zum besten
        // Treffer filtern ist robuster als eine feste Schwelle.
        let hits = with_index(state, |idx| {
            idx.search_in(&q, SEARCH_MAX_HITS, -1.0, Some(&allowed))
        })?;
        let best = hits.first().map(|h| h.1).unwrap_or(0.0);
        let ids: Vec<i64> = hits
            .iter()
            .take_while(|(_, s)| *s >= best - SEARCH_MARGIN)
            .map(|h| h.0)
            .collect();
        log::info!(
            "Semantische Suche „{query}“ → „{english}“: {} Treffer",
            ids.len()
        );
        state.catalog.locked().images_in_order(&ids, &sq)
    })
    .await
}

/// CLIP-Vektoren einzelner Fotos aus dem Suchindex (für „Auto nach deinem
/// Stil“, Stufe 4: ähnliche Motive). Fehlende fehlen einfach.
pub(crate) fn embeddings_for(
    state: &AppState,
    ids: &[i64],
) -> std::collections::HashMap<i64, Vec<f32>> {
    with_index(state, |idx| {
        ids.iter()
            .filter_map(|id| Some((*id, idx.get(*id)?.to_vec())))
            .collect()
    })
    .unwrap_or_default()
}

/// CLIP-Werte für das Fotoalbum (siehe `album::select`).
pub(crate) struct ThemeScores {
    /// Kosinus zum Thema (Mittel der Themen-Beschreibungen)
    pub theme: std::collections::HashMap<i64, f32>,
    /// Höchster Kosinus zu einer der unerwünschten Beschreibungen
    /// (Bildschirmfoto, Dokument …)
    pub negative: std::collections::HashMap<i64, f32>,
    /// Bild-Embeddings (für Dubletten und Abwechslung)
    pub vectors: std::collections::HashMap<i64, Vec<f32>>,
}

/// Thema- und Ausschlusswerte für `ids` aus dem Suchindex. `None`, wenn die
/// KI nicht eingerichtet ist oder keins der Fotos schon analysiert wurde – das
/// Album wählt dann nur nach Qualität und Bewertung. Englische Beschreibungen
/// erwartet (CLIP).
pub(crate) fn theme_scores(
    state: &AppState,
    prompts: &[String],
    negatives: &[&str],
    ids: &[i64],
) -> Result<Option<ThemeScores>, Error> {
    if ids.is_empty() || !models::status(&state.data_dir).model_ready {
        return Ok(None);
    }
    let vectors: std::collections::HashMap<i64, Vec<f32>> = with_index(state, |idx| {
        ids.iter()
            .filter_map(|id| Some((*id, idx.get(*id)?.to_vec())))
            .collect()
    })?;
    if vectors.is_empty() {
        return Ok(None);
    }
    let loaded = ensure_loaded(state)?;
    let mut texts: Vec<&str> = prompts.iter().map(String::as_str).collect();
    texts.extend_from_slice(negatives);
    let emb = loaded.clip.embed_texts(&texts).map_err(err)?;
    let (pos, neg) = emb.split_at(prompts.len());
    let theme_vec = (!pos.is_empty()).then(|| lumenkorn_ai::clip::mean_normalized(pos));
    let mut theme = std::collections::HashMap::new();
    let mut negative = std::collections::HashMap::new();
    for (id, v) in &vectors {
        if let Some(t) = &theme_vec {
            theme.insert(*id, lumenkorn_ai::clip::dot(t, v));
        }
        let n = neg
            .iter()
            .map(|q| lumenkorn_ai::clip::dot(q, v))
            .fold(f32::MIN, f32::max);
        if !neg.is_empty() {
            negative.insert(*id, n);
        }
    }
    Ok(Some(ThemeScores {
        theme,
        negative,
        vectors,
    }))
}

// --- KI-Masken --------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaskResult {
    pub(crate) mask_id: String,
    /// Anteil der Bildfläche, 0…1 – 0 heißt „nichts gefunden“
    pub(crate) coverage: f32,
}

/// Quellbild für die Segmentierung: unbearbeitet entwickelt, Originalausrichtung
/// (Masken beziehen sich auf das aufrechte Original, nicht auf Drehung/Zuschnitt).
fn segmentation_input(state: &AppState, id: i64) -> Result<image::DynamicImage, Error> {
    segmentation_input_at(state, id, segment::MASK_EDGE)
}

/// Wie [`segmentation_input`], mit eigener längster Kante.
fn segmentation_input_at(
    state: &AppState,
    id: i64,
    edge: u32,
) -> Result<image::DynamicImage, Error> {
    let src = crate::develop::load_proxy(state, id, edge as usize)?;
    // Masken entstehen auf dem Bild, wie es angezeigt wird – mit oder ohne
    // Objektivkorrektur
    let p = DevelopParams {
        lens_correction: crate::develop::lens_setting(state, id),
        ..Default::default()
    };
    let img = lumenkorn_develop::render(&src, &p);
    Ok(image::DynamicImage::ImageRgb8(
        lumenkorn_develop::output::to_rgb8(&img),
    ))
}

fn masks_needed(state: &AppState) -> Result<(), Error> {
    state.usage.touch(Slot::Interactive);
    if !models::status(&state.data_dir).masks_ready {
        return Err(Error::Invalid(
            tr_s(
                "Die KI-Masken sind noch nicht installiert. Lade sie über „KI“ unten links.",
                "The AI masks are not installed yet. Download them via “AI” at the bottom left.",
            )
            .into(),
        ));
    }
    Ok(())
}

fn subject_model(state: &AppState) -> Result<Arc<SubjectModel>, Error> {
    masks_needed(state)?;
    lazy_model(
        state,
        &state.ai.subject,
        tr_s("IS-Net (Motiv)", "IS-Net (subject)"),
        || {
            SubjectModel::load(&models::masks_dir(&state.data_dir).join("subject.onnx"))
                .map_err(err)
        },
    )
}

fn sam_model(state: &AppState) -> Result<Arc<Sam>, Error> {
    masks_needed(state)?;
    lazy_model(
        state,
        &state.ai.sam,
        tr_s("Segment Anything (Objekt)", "Segment Anything (object)"),
        || {
            let dir = models::masks_dir(&state.data_dir);
            Sam::load(&dir.join("sam-encoder.onnx"), &dir.join("sam-decoder.onnx")).map_err(err)
        },
    )
}

/// (Bild, Objektivkorrektur): Die Eingabe des Embeddings hängt von beidem ab
type SamKey = (i64, bool);

/// Zwischenspeicher der Funde für dieses Bild entnehmen (leer, wenn er zu einem
/// anderen Bild bzw. einer anderen Objektivkorrektur gehört).
fn take_detect_cache(state: &AppState, id: i64) -> (SamKey, parts::DetectCache) {
    let key = (id, crate::develop::lens_setting(state, id));
    match state.ai.detect_cache.locked().take() {
        Some((k, c)) if k == key => (key, c),
        _ => (key, parts::DetectCache::default()),
    }
}

fn sam_embedding(state: &AppState, sam: &Sam, id: i64) -> Result<Arc<SamEmbedding>, Error> {
    let key = (id, crate::develop::lens_setting(state, id));
    if let Some((cached, e)) = state.ai.sam_embedding.locked().as_ref()
        && *cached == key
    {
        return Ok(e.clone());
    }
    let t = std::time::Instant::now();
    let _g = analysis_guard(state, "Segment Anything");
    let e = Arc::new(sam.embed(&segmentation_input(state, id)?).map_err(err)?);
    log::info!("SAM-Embedding für Bild {id} in {:.2?}", t.elapsed());
    *state.ai.sam_embedding.locked() = Some((key, e.clone()));
    Ok(e)
}

/// Legt eine Maske unter einer **neuen** ID ab. Bestehende Dateien werden nie
/// überschrieben: „Rückgängig“ und Schnappschüsse verweisen weiter auf die
/// alte ID und müssen dieselbe Maske wiederfinden. Unbenutzte Dateien räumt
/// der Hausmeister (`memory::sweep_data`) nach 24 Stunden weg.
fn store(
    state: &AppState,
    image_id: i64,
    kind: &str,
    mask: &image::GrayImage,
) -> Result<MaskResult, Error> {
    let mask_id = MaskFiles::new_id(image_id, kind);
    state.masks.put(&mask_id, mask)?;
    let coverage = mask.pixels().filter(|p| p[0] > 127).count() as f32
        / (mask.width() * mask.height()).max(1) as f32;
    Ok(MaskResult { mask_id, coverage })
}

/// „Motiv auswählen“
#[tauri::command]
pub async fn mask_subject(app: AppHandle, id: i64) -> Result<MaskResult, Error> {
    blocking(app, move |state| {
        let model = subject_model(state)?;
        let t = std::time::Instant::now();
        let _g = analysis_guard(state, tr_s("Motiv", "Subject"));
        let mask = model
            .predict(&segmentation_input(state, id)?)
            .map_err(err)?;
        log::info!("Motivmaske für Bild {id} in {:.2?}", t.elapsed());
        store(state, id, "subject", &mask)
    })
    .await
}

/// „Himmel auswählen“. Mit dem Himmelsmodell (skyseg) entscheidet die KI;
/// fehlt es, greift die Farb- und Flächenheuristik, verfeinert durch SAM,
/// wenn installiert.
#[tauri::command]
pub async fn mask_sky(app: AppHandle, id: i64) -> Result<MaskResult, Error> {
    blocking(app, move |state| {
        state.usage.touch(Slot::Interactive);
        let input = segmentation_input(state, id)?;
        let dir = models::masks_dir(&state.data_dir);
        let status = models::status(&state.data_dir);
        let t = std::time::Instant::now();
        let mask = if status.sky_ready {
            let sky = lazy_model(state, &state.ai.sky, "skyseg (Himmel)", || {
                SkyModel::load(&dir.join(models::SKY_MODEL)).map_err(err)
            })?;
            let _g = analysis_guard(state, tr_s("Himmel", "Sky"));
            segment::sky_mask(&input, Some(&sky), None).map_err(err)?
        } else if status.masks_ready {
            let s = sam_model(state)?;
            let emb = sam_embedding(state, &s, id)?;
            segment::sky_mask(&input, None, Some((&s, &emb))).map_err(err)?
        } else {
            segment::sky_mask(&input, None, None).map_err(err)?
        };
        log::info!("Himmelsmaske für Bild {id} in {:.2?}", t.elapsed());
        store(state, id, "sky", &mask)
    })
    .await
}

/// Modelle für Personen und Tiere (mit den KI-Masken nachgeladen).
fn parts_needed(state: &AppState) -> Result<(), Error> {
    masks_needed(state)?;
    if !models::status(&state.data_dir).parts_ready {
        return Err(Error::Invalid(parts::missing_hint().into()));
    }
    Ok(())
}

fn owl_model(state: &AppState) -> Result<Arc<parts::Owl>, Error> {
    parts_needed(state)?;
    lazy_model(state, &state.ai.owl, "OWLv2", || {
        parts::Owl::load(&models::masks_dir(&state.data_dir)).map_err(err)
    })
}

/// „Person“: die ganze Person oder ein Teil davon, bei allen Personen im Foto
#[tauri::command]
pub async fn mask_person(
    app: AppHandle,
    id: i64,
    part: parts::PersonPart,
) -> Result<MaskResult, Error> {
    blocking(app, move |state| {
        let owl = owl_model(state)?;
        let dir = models::masks_dir(&state.data_dir);
        let seg = lazy_model(state, &state.ai.parts_seg, "Selfie Multiclass", || {
            parts::PartsSeg::load(&dir.join(parts::SEG_FILE)).map_err(err)
        })?;
        let mesh = lazy_model(state, &state.ai.mesh, "Face Mesh", || {
            parts::FaceMesh::load(&dir.join(parts::MESH_FILE)).map_err(err)
        })?;
        // SAM nur für die ganze Person (Umriss)
        let sam = if part == parts::PersonPart::Person {
            let s = sam_model(state)?;
            let e = sam_embedding(state, &s, id)?;
            Some((s, e))
        } else {
            None
        };
        let input = segmentation_input_at(state, id, parts::PARTS_EDGE)?;
        let t = std::time::Instant::now();
        let _g = analysis_guard(state, tr_s("Personen", "People"));
        let m = parts::PartModels {
            owl: &owl,
            seg: &seg,
            mesh: &mesh,
            sam: sam.as_ref().map(|(s, e)| (s.as_ref(), e.as_ref())),
        };
        let (key, mut cache) = take_detect_cache(state, id);
        let mask = parts::person_mask(&input, part, &m, &mut cache).map_err(err)?;
        *state.ai.detect_cache.locked() = Some((key, cache));
        log::info!(
            "Personenmaske ({part:?}) für Bild {id} in {:.2?}",
            t.elapsed()
        );
        store(state, id, "person", &mask)
    })
    .await
}

/// „Tier“: alle Tiere einer Art (Vögel, Säugetiere, Reptilien oder alle)
#[tauri::command]
pub async fn mask_animal(
    app: AppHandle,
    id: i64,
    kind: parts::AnimalKind,
) -> Result<MaskResult, Error> {
    blocking(app, move |state| {
        let owl = owl_model(state)?;
        let sam = sam_model(state)?;
        let emb = sam_embedding(state, &sam, id)?;
        let input = segmentation_input(state, id)?;
        let t = std::time::Instant::now();
        let _g = analysis_guard(state, tr_s("Tiere", "Animals"));
        let (key, mut cache) = take_detect_cache(state, id);
        let mask = parts::animal_mask(&input, kind, &owl, (&sam, &emb), &mut cache).map_err(err)?;
        *state.ai.detect_cache.locked() = Some((key, cache));
        log::info!("Tiermaske ({kind:?}) für Bild {id} in {:.2?}", t.elapsed());
        store(state, id, "animal", &mask)
    })
    .await
}

/// Objekt per Klick. Mit `mask_id` (die bisherige Maske) wird die Auswahl
/// mit den neuen Punkten verfeinert; das Ergebnis bekommt immer eine neue ID,
/// die alte Datei bleibt für „Rückgängig“ erhalten.
#[tauri::command]
pub async fn mask_object(
    app: AppHandle,
    id: i64,
    points: Vec<PromptPoint>,
    mask_id: Option<String>,
) -> Result<MaskResult, Error> {
    blocking(app, move |state| {
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
        // Die alte Datei bleibt für „Rückgängig“: Frist des Aufräumens neu starten
        if let Some(old) = mask_id
            .filter(|i| i.starts_with(&format!("{id}-")))
            .and_then(|i| state.masks.path(&i))
            && let Ok(f) = std::fs::OpenOptions::new().write(true).open(&old)
        {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
        store(state, id, "object", &mask)
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
            tr_s(
                "Die KI-Rauschreduzierung ist noch nicht installiert. Lade sie über „KI“ unten links.",
                "AI denoise is not installed yet. Download it via “AI” at the bottom left.",
            )
            .into(),
        ));
    }
    lazy_model(
        state,
        &state.ai.denoiser,
        tr_s("DRUNet (Rauschreduzierung)", "DRUNet (denoise)"),
        || {
            let path = models::denoise_dir(&state.data_dir).join(models::DENOISE_MODEL);
            lumenkorn_ai::denoise::Denoiser::load(&path).map_err(err)
        },
    )
}

/// Entrauscht ein Bild in voller Auflösung im Hintergrund. Fortschritt und
/// Ergebnis kommen als `denoise-progress`. `strength` skaliert die geschätzte
/// Rauschstärke (1,0 = automatisch).
#[tauri::command(async)]
pub fn denoise_start(app: AppHandle, id: i64, strength: Option<f32>) -> Result<(), Error> {
    let state = app.state::<AppState>();
    // Gibt `denoising` auch nach einem Panic (oder fehlgeschlagenem
    // Thread-Start) frei; sonst blieben Entrauschen und das Entladen von
    // DRUNet bis zum Neustart gesperrt.
    let Some(finished) = crate::busy::claim_in(&app, |s| &s.ai.denoising) else {
        return Err(Error::Invalid(
            tr_s(
                "Es wird bereits ein Foto entrauscht.",
                "A photo is already being denoised.",
            )
            .into(),
        ));
    };
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
            // Ein Panic soll als Fehler gemeldet werden (sonst zeigte die
            // Oberfläche weiter „läuft“), nicht nur `denoising` freigeben
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || -> Result<String, Error> {
                    let d = denoiser(&state)?;
                    let job = state.catalog.locked().thumb_job(id)?;
                    let t = std::time::Instant::now();
                    let src = Source::load(
                        Path::new(&job.path),
                        job.kind == lumenkorn_core::formats::ImageKind::Raw,
                        job.orientation.map(|o| o as u16),
                        None,
                    )?;
                    let img = &src.image;
                    let _g = analysis_guard(&state, tr_s("Rauschreduzierung", "Denoise"));
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
                        .ok_or_else(|| Error::Invalid(tr_s("ungültige ID", "invalid ID").into()))?;
                    lumenkorn_develop::denoised::write(&path, img.width, img.height, &out)?;
                    log::info!(
                        "Bild {id} entrauscht in {:.1?} ({}×{})",
                        t.elapsed(),
                        img.width,
                        img.height
                    );
                    Ok(denoise_id)
                },
            ))
            .unwrap_or_else(|_| {
                Err(Error::Invalid(
                    tr_s(
                        "interner Fehler bei der Rauschreduzierung",
                        "internal error while denoising",
                    )
                    .into(),
                ))
            });
            drop(finished);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poisoned_slot_counts_as_empty() {
        let slot: Mutex<Option<Arc<u32>>> = Mutex::new(Some(Arc::new(1)));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = slot.lock().unwrap();
            panic!("Laden abgestürzt");
        }));
        assert!(slot.is_poisoned());
        // Kein Panic, der Slot ist leer und wieder benutzbar
        assert!(!drop_if_unused(&slot));
        assert!(!slot.is_poisoned());
        assert!(slot.locked().is_none());
    }
}
