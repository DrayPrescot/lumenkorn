//! KI im Hintergrund: Einstellungen, Modelle laden und entladen, Absturzschutz,
//! Status. Die Teile: `analysis` (Schlagworte, Tierarten), `search` (Suchindex),
//! `downloads`, `masks` (KI-Masken), `denoise` (Rauschreduzierung).

use crate::guard::{self, Guard, Stage};
use crate::memory::Slot;
use crate::worker::Job;
use crate::{AppState, blocking};
use lumenkorn_ai::accel::{self, Backend, Preference};
use lumenkorn_ai::animals::{AnimalTagger, BioTaxonomy, Bioclip};
use lumenkorn_ai::index::{from_bytes, to_bytes};
use lumenkorn_ai::models::{self, Component, Status};
use lumenkorn_ai::parts;
use lumenkorn_ai::segment::{self, Sam, SamEmbedding, SkyModel, SubjectModel};
use lumenkorn_ai::{Clip, EmbeddingIndex, TagSuggestion, Tagger, runtime, vocab};
use lumenkorn_core::LockExt;
use lumenkorn_core::maskstore::MaskFiles;
use lumenkorn_core::thumbs::Variant;
use lumenkorn_core::{Error, ImageQuery, ImageSummary, Scope};
use lumenkorn_develop::i18n::tr_s;
use lumenkorn_develop::{DevelopParams, PromptPoint, Source, tr};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};
use tauri::{AppHandle, Emitter, Manager, State};

mod analysis;
mod denoise;
mod downloads;
mod masks;
mod search;

pub use analysis::*;
pub use denoise::*;
pub use downloads::*;
pub use masks::*;
pub use search::*;

/// Kennung, unter der Embeddings im Katalog stehen. Ein anderes Modell bekommt
/// eine andere Kennung, damit nie Vektoren verschiedener Modelle gemischt werden.
const MODEL_KEY: &str = models::MODEL_ID;
const BATCH: usize = 16;
/// Stand der Tierart-Schlagworte schon analysierter Fotos: 1 = mit CLIP
/// nachgetragen, 3 = mit BioCLIP 2 und der großen Liste nachbestimmt (sobald
/// es installiert ist; 2 war die erste, kleine Liste).
const ANIMAL_TAGS_CLIP: u32 = 1;
const ANIMAL_TAGS_BIOCLIP: u32 = 3;
/// Tierfotos je Durchgang beim Nachbestimmen mit BioCLIP (je Foto etwa
/// 0,3 s auf der CPU – der Worker soll zwischendurch Neues annehmen)
const BIOCLIP_BACKFILL_BATCH: usize = 16;
/// Datei im KI-Ordner mit dem Stand, bis zu dem nachgetragen wurde
const ANIMAL_TAGS_FILE: &str = "tierarten.json";
/// Laufendes Nachtragen (Ziel-Stand, zuletzt bearbeitete Bild-ID), damit es
/// nach einem Neustart weitergeht statt von vorn
const ANIMAL_BACKFILL_FILE: &str = "tierarten-fortschritt.json";
/// Fotos je Durchgang beim Nachtragen (nur Rechnen auf gespeicherten Vektoren)
const ANIMAL_BACKFILL_BATCH: usize = 500;
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
    /// Tierarten (Text-Embeddings der Taxonomie), beim ersten Verschlagworten
    /// berechnet – nicht schon beim Laden von CLIP, das die Suche ausbremst
    animals: Mutex<Option<Arc<AnimalTagger>>>,
    /// Nachtragen der Tierarten: Ziel-Stand und zuletzt bearbeitete Bild-ID
    animal_backfill: Mutex<(u32, i64)>,
    /// BioCLIP 2 (Bildteil) für genauere Tierarten, falls installiert
    bioclip: Mutex<Option<Arc<BioAnimals>>>,
    /// Letzte Benutzung des Suchindex; der Hausmeister gibt ihn danach frei
    index_used: Mutex<std::time::Instant>,
    /// Gewünschte Downloads: Sie laufen nacheinander, ein weiterer Klick
    /// während eines Downloads reiht sich ein
    downloads: Mutex<Downloads>,
    settings_path: PathBuf,
    pub(crate) settings: Mutex<AiSettings>,
    /// Hält, wer die Einstellungen ändert (lesen, ergänzen, speichern)
    settings_write: Mutex<()>,
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
    /// Hauptschalter: aus = keine KI-Laufzeit, keine Modelle, keine Analyse im
    /// Hintergrund (für schwache Rechner). Vorhandene Schlagworte, Masken,
    /// Entrauschungen und Reparaturen bleiben und wirken weiter.
    pub enabled: bool,
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
    /// Beim Start nicht mehr fragen, ob die GPU-Laufzeit für die KI geladen
    /// und eingeschaltet werden soll (vom Nutzer abgelehnt)
    pub gpu_prompt_declined: bool,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_tag: true,
            accel: Preference::Auto,
            develop_gpu: true,
            faces: true,
            learn_style: true,
            gpu_prompt_declined: false,
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
    guard::clear_on_exit(&state.ai.guard_dir);
}

/// Aktuelles Rechenwerk der KI, für Absturz-Markierungen.
fn current_backend() -> &'static str {
    let a = accel::active();
    match a.library {
        Some(b) if a.gpu_enabled => backend_id(b),
        _ => "cpu",
    }
}

/// Rechenwerk eines geladenen Modells: Rechnet es auf der CPU, soll ein
/// Absturz dabei nicht die GPU-Laufzeit abschalten.
fn model_backend(on_gpu: bool) -> &'static str {
    if on_gpu { current_backend() } else { "cpu" }
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
        let mut settings: AiSettings =
            lumenkorn_develop::fsutil::load_json_or_default(&settings_path);
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
        if (crash_notice.is_some() || changed)
            && let Err(e) = lumenkorn_develop::fsutil::save_json(&settings_path, &settings)
        {
            log::warn!("KI-Einstellungen nicht gespeichert: {e}");
        }
        // Grafikkarte im Hintergrund suchen – bis dahin rechnet die CPU.
        lumenkorn_develop::gpu::set_enabled(settings.develop_gpu);
        if settings.develop_gpu {
            start_develop_gpu(ai_dir.clone());
        }
        let animal_backfill =
            lumenkorn_develop::fsutil::load_json_or_default(&ai_dir.join(ANIMAL_BACKFILL_FILE));
        Self {
            guard_dir: ai_dir,
            crash_notice: Mutex::new(crash_notice),
            loaded: Mutex::new(None),
            loading: Mutex::new(()),
            load_error: Mutex::new(None),
            index: RwLock::new(None),
            animals: Mutex::new(None),
            animal_backfill: Mutex::new(animal_backfill),
            bioclip: Mutex::new(None),
            index_used: Mutex::new(std::time::Instant::now()),
            downloads: Mutex::new(Downloads::default()),
            settings_path,
            settings: Mutex::new(settings),
            settings_write: Mutex::new(()),
            subject: Mutex::new(None),
            sam: Mutex::new(None),
            sky: Mutex::new(None),
            owl: Mutex::new(None),
            parts_seg: Mutex::new(None),
            mesh: Mutex::new(None),
            sam_embedding: Mutex::new(None),
            detect_cache: Mutex::new(None),
            denoiser: Mutex::new(None),
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

/// Ist die KI eingeschaltet (Hauptschalter in den Einstellungen)?
pub(crate) fn enabled(state: &AppState) -> bool {
    state.ai.settings.locked().enabled
}

/// Fehler für alles, was ein Modell bräuchte, solange die KI aus ist.
fn disabled_error() -> Error {
    Error::Invalid(
        tr_s(
            "Die KI ist in den Einstellungen ausgeschaltet",
            "AI is turned off in the settings",
        )
        .into(),
    )
}

/// Jedes Modell wird hierüber geladen: Bei ausgeschalteter KI scheitert es,
/// und die Laufzeit kommt gar nicht erst in den Speicher.
pub(crate) fn ensure_runtime(state: &AppState) -> Result<(), Error> {
    if !enabled(state) {
        return Err(disabled_error());
    }
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
        if !state.tasks.denoise.is_running() && drop_if_unused(&ai.denoiser) {
            out.push("DRUNet");
        }
    }
    if background && drop_if_unused(&ai.bioclip) {
        out.push("BioCLIP 2");
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
    /// Tiererkennung mit BioCLIP: wie weit analysierte Fotos durchgesehen sind
    animals_progress: Option<Progress>,
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
        downloading: state.tasks.download.is_running(),
        loaded: state.ai.loaded.locked().is_some(),
        error: state.ai.load_error.locked().clone(),
        analyzed,
        total,
        settings: state.ai.settings.locked().clone(),
        animals_progress: animal_progress(&state),
    })
}

/// Ändert die übergebenen Einstellungen; nicht genannte bleiben, wie sie sind.
/// Die Oberfläche schickt nur, was sich geändert hat: Zwei schnelle Klicks könnten
/// mit dem ganzen Stand die jeweils andere Änderung zurücknehmen.
#[tauri::command(async)]
pub fn ai_set_settings(state: State<AppState>, settings: serde_json::Value) -> Result<(), Error> {
    let _writing = state.ai.settings_write.locked();
    let mut merged = serde_json::to_value(&*state.ai.settings.locked())?;
    if let (Some(m), Some(patch)) = (merged.as_object_mut(), settings.as_object()) {
        for (k, v) in patch {
            m.insert(k.clone(), v.clone());
        }
    }
    let settings: AiSettings = serde_json::from_value(merged)?;
    lumenkorn_develop::fsutil::save_json(&state.ai.settings_path, &settings)?;
    let auto = settings.enabled && settings.auto_tag;
    let was_enabled = enabled(&state);
    let now_enabled = settings.enabled;
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
    if was_enabled && !now_enabled {
        // Ausgeschaltet: Modelle und Suchindex sofort freigeben. Was gerade
        // rechnet, behält seine Kopie und lässt sie danach los.
        state.tasks.denoise.cancel();
        unload_models(&state);
        *state.ai.bioclip.locked() = None;
        *state.ai.animals.locked() = None;
        forget_in_index(&state, None);
        crate::repair::release_model(&state);
        crate::people::release_models(&state);
        crate::memory::trim_heap();
        log::info!("KI ausgeschaltet: Modelle freigegeben");
    } else if !was_enabled && now_enabled {
        // Fehler aus der Zeit, als sie aus war, nicht nachtragen
        *state.ai.load_error.locked() = None;
        *state.people.load_error.locked() = None;
    }
    if auto {
        state.worker.submit(Job::Ai);
    }
    state.worker.submit(Job::Faces);
    crate::style::settings_changed(&state);
    Ok(())
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
    state.tasks.denoise.cancel();
    state.tasks.download.cancel();
    clear_crash_markers(&state);
    crate::exit::restart(&app);
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
