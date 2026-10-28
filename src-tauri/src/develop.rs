//! Entwickeln: Proxy-Cache, Live-Vorschau, Speichern, Snapshots und Export.

use crate::worker::Job;
use crate::{AppState, blocking};
use lumenkorn_core::LockExt;
use lumenkorn_core::formats::ImageKind;
use lumenkorn_core::{Catalog, Edit, Error, Preset};
use lumenkorn_develop::i18n::tr_s;
use lumenkorn_develop::output::{self, ExportFormat, ExportOptions};
use lumenkorn_develop::{Crop, DevelopParams, ImageF32, RenderOptions, Source, tr};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Manager, State};

/// Längste Kante des Bearbeitungs-Proxys.
pub const PROXY_EDGE: usize = 2048;
const CACHE_ENTRIES: usize = 4;

/// Stand der Quelldatei beim Laden. Wird sie von außen geändert (in einer
/// anderen Software überarbeitet, RAW ersetzt), passt der Stempel nicht mehr
/// und das Bild wird neu geladen – statt weiter den alten Inhalt zu zeigen.
struct FileStamp {
    path: PathBuf,
    stamp: Option<(u64, std::time::SystemTime)>,
    /// Letzte Prüfung gegen das Dateisystem (`None`: beim nächsten Mal prüfen)
    checked: Option<std::time::Instant>,
}

/// So selten wird die Quelldatei bei Zwischenabfragen (Regler) geprüft –
/// auf einem NAS wäre jede Prüfung ein Netzwerkzugriff.
const STAMP_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1500);

impl FileStamp {
    fn read(path: &Path) -> Option<(u64, std::time::SystemTime)> {
        let m = std::fs::metadata(path).ok()?;
        Some((m.len(), m.modified().ok()?))
    }

    fn of(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            stamp: Self::read(path),
            checked: Some(std::time::Instant::now()),
        }
    }

    fn changed(&self) -> bool {
        Self::read(&self.path) != self.stamp
    }

    /// Wie [`Self::changed`], prüft aber höchstens alle `interval` wirklich.
    fn changed_throttled(
        &mut self,
        now: std::time::Instant,
        interval: std::time::Duration,
    ) -> bool {
        if self
            .checked
            .is_some_and(|t| now.saturating_duration_since(t) < interval)
        {
            return false;
        }
        self.checked = Some(now);
        self.changed()
    }
}

struct Entry {
    id: i64,
    file: FileStamp,
    proxy: Arc<Source>,
    /// Zuletzt angefragte kleinere Fassung (Fenstergröße), spart das Verkleinern.
    scaled: Option<(usize, Arc<Source>)>,
}

/// (Datei-ID der Zwischenstufe, Kantenlänge) → verkleinertes Bild
type DenoisedEntry = ((String, usize), Arc<ImageF32>);

/// Hält die zuletzt bearbeiteten Bilder dekodiert im Speicher – eine
/// RAW-Dekodierung dauert Sekunden, ein Regler soll in Millisekunden reagieren.
#[derive(Default)]
pub struct DevelopCache {
    entries: Mutex<Vec<Entry>>,
    /// Entrauschte Proxys: (Datei-ID, Kantenlänge) → Bild
    denoised: Mutex<Vec<DenoisedEntry>>,
    /// Volle Auflösung für die Detailansicht beim Zoomen (nur ein Bild):
    /// Quelle und verkleinerte Fassung für Zoomstufen unter 100 %.
    full: Mutex<Option<FullEntry>>,
    /// Hält nur, wer gerade die volle Auflösung lädt (Sekunden). `full`
    /// bleibt währenddessen frei für Abfragen und `release_large`.
    full_loading: Mutex<()>,
    /// Ladesperre je Bild für die Proxys: Gleichzeitige Aufrufe für dasselbe
    /// Bild dekodieren es nur einmal, andere Bilder warten nicht darauf.
    proxy_loading: Mutex<Vec<(i64, Arc<Mutex<()>>)>>,
    full_denoised: Mutex<Option<FullDenoised>>,
    /// Ein Export läuft. Zwei gleichzeitig verdoppelten den Speicherbedarf
    /// (volle Auflösung), und das Aufräumen am Ende des ersten gäbe Puffer
    /// des zweiten frei.
    pub(crate) exporting: std::sync::atomic::AtomicBool,
}

/// Entrauschte Fassung in voller Auflösung samt zuletzt verkleinerter.
type FullDenoised = (String, Arc<ImageF32>, Option<(usize, Arc<ImageF32>)>);

/// Kantenlängen für Zoomstufen unter 100 % auf Stufen runden: Beim Zoomen mit
/// dem Mausrad wird so die verkleinerte Quelle wiederverwendet, statt bei
/// jedem Schritt das Vollbild neu zu verkleinern.
fn zoom_edge(full_long: usize, scale: f32) -> usize {
    const STEP: usize = 512;
    let want = (full_long as f32 * scale).ceil() as usize;
    want.div_ceil(STEP)
        .saturating_mul(STEP)
        .clamp(256, full_long)
}

struct FullEntry {
    id: i64,
    file: FileStamp,
    source: Arc<Source>,
    scaled: Option<(usize, Arc<Source>)>,
}

impl DevelopCache {
    fn get(&self, id: i64, max_edge: usize) -> Option<Arc<Source>> {
        let mut entries = self.entries.locked();
        let pos = entries.iter().position(|e| e.id == id)?;
        let mut e = entries.remove(pos);
        if e.file
            .changed_throttled(std::time::Instant::now(), STAMP_INTERVAL)
        {
            log::info!("{} wurde geändert – neu laden", e.file.path.display());
            return None;
        }
        let out = if max_edge >= e.proxy.image.long_edge() {
            e.proxy.clone()
        } else {
            match &e.scaled {
                Some((m, s)) if *m == max_edge => s.clone(),
                _ => {
                    let s = Arc::new(e.proxy.downscaled(max_edge));
                    e.scaled = Some((max_edge, s.clone()));
                    s
                }
            }
        };
        entries.insert(0, e);
        Some(out)
    }

    /// Die nächste Abfrage prüft die Datei sicher (beim Öffnen eines Fotos).
    fn recheck_file(&self, id: i64) {
        for e in self.entries.locked().iter_mut().filter(|e| e.id == id) {
            e.file.checked = None;
        }
        if let Some(f) = self.full.locked().as_mut().filter(|f| f.id == id) {
            f.file.checked = None;
        }
    }

    fn insert(&self, id: i64, file: FileStamp, proxy: Arc<Source>) {
        let mut entries = self.entries.locked();
        entries.retain(|e| e.id != id);
        entries.insert(
            0,
            Entry {
                id,
                file,
                proxy,
                scaled: None,
            },
        );
        entries.truncate(CACHE_ENTRIES);
    }

    /// Gibt die großen Puffer frei (volle Auflösung, entrauschte Fassungen).
    /// Die kleinen Proxys bleiben, damit das Zurückkehren schnell geht.
    pub fn release_large(&self) {
        *self.full.locked() = None;
        *self.full_denoised.locked() = None;
        self.denoised.locked().clear();
    }

    pub fn forget(&self, id: i64) {
        self.entries.locked().retain(|e| e.id != id);
        self.denoised.locked().clear();
        *self.full_denoised.locked() = None;
        let mut f = self.full.locked();
        if f.as_ref().is_some_and(|e| e.id == id) {
            *f = None;
        }
    }
}

/// Entrauschte Fassung in derselben Größe wie [`load_proxy`] (für Vorschau).
pub(crate) fn load_denoised(
    state: &AppState,
    denoise_id: &str,
    max_edge: usize,
) -> Option<Arc<ImageF32>> {
    let key = (denoise_id.to_string(), max_edge);
    {
        let c = state.develop.denoised.locked();
        if let Some((_, img)) = c.iter().find(|(k, _)| *k == key) {
            return Some(img.clone());
        }
    }
    let path = lumenkorn_develop::denoised::path(&state.denoised_dir, denoise_id)?;
    let img = lumenkorn_develop::denoised::read(&path, Some(PROXY_EDGE)).ok()?;
    let img = Arc::new(if max_edge < img.long_edge() {
        img.downscale_to(max_edge)
    } else {
        img
    });
    let mut c = state.develop.denoised.locked();
    c.insert(0, (key, img.clone()));
    c.truncate(4);
    Some(img)
}

pub(crate) fn load_proxy(state: &AppState, id: i64, max_edge: usize) -> Result<Arc<Source>, Error> {
    if let Some(s) = state.develop.get(id, max_edge) {
        return Ok(s);
    }
    // Je Bild nur ein Ladevorgang; wer wartet, nimmt danach das Ergebnis
    let lock = {
        let mut l = state.develop.proxy_loading.locked();
        match l.iter().find(|(i, _)| *i == id) {
            Some((_, m)) => m.clone(),
            None => {
                let m = Arc::new(Mutex::new(()));
                l.push((id, m.clone()));
                m
            }
        }
    };
    let loaded = {
        let _loading = lock.lock().unwrap_or_else(|e| e.into_inner());
        load_proxy_locked(state, id, max_edge)
    };
    // Eintrag aufräumen; Wartende halten ihre Sperre über den eigenen Arc
    state
        .develop
        .proxy_loading
        .locked()
        .retain(|(i, m)| !(*i == id && Arc::ptr_eq(m, &lock)));
    loaded
}

fn load_proxy_locked(state: &AppState, id: i64, max_edge: usize) -> Result<Arc<Source>, Error> {
    if let Some(s) = state.develop.get(id, max_edge) {
        return Ok(s);
    }
    let job = state.catalog.locked().thumb_job(id)?;
    // Stempel vor dem Laden: Ändert sich die Datei währenddessen, gilt der
    // Eintrag gleich als veraltet
    let file = FileStamp::of(Path::new(&job.path));
    let src = Source::load(
        Path::new(&job.path),
        job.kind == ImageKind::Raw,
        job.orientation.map(|o| o as u16),
        Some(PROXY_EDGE),
    )?;
    let src = Arc::new(src);
    state.develop.insert(id, file, src.clone());
    // Der Eintrag kann schon wieder fehlen (Datei geändert, verdrängt,
    // vergessen) – das frisch geladene Bild direkt verwenden
    Ok(state
        .develop
        .get(id, max_edge)
        .unwrap_or_else(|| fitted(&src, max_edge)))
}

/// Das Bild auf `max_edge` verkleinert (oder unverändert, wenn es kleiner ist).
fn fitted(src: &Arc<Source>, max_edge: usize) -> Arc<Source> {
    if max_edge >= src.image.long_edge() {
        src.clone()
    } else {
        Arc::new(src.downscaled(max_edge))
    }
}

/// Quelle in voller Auflösung (bzw. verkleinert auf `max_edge`, wenn kleiner).
fn load_full(state: &AppState, id: i64, max_edge: usize) -> Result<Arc<Source>, Error> {
    if let Some(s) = cached_full(state, id, max_edge) {
        return Ok(s);
    }
    // Laden ohne `full` zu halten; gleichzeitige Anfragen warten hier und
    // nehmen dann das Ergebnis des ersten
    let _loading = state
        .develop
        .full_loading
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(s) = cached_full(state, id, max_edge) {
        return Ok(s);
    }
    *state.develop.full.locked() = None; // alten Speicher vor dem Laden freigeben
    let job = state.catalog.locked().thumb_job(id)?;
    let file = FileStamp::of(Path::new(&job.path));
    let t = std::time::Instant::now();
    let src = Source::load(
        Path::new(&job.path),
        job.kind == ImageKind::Raw,
        job.orientation.map(|o| o as u16),
        None,
    )?;
    log::info!(
        "Volle Auflösung für Detailansicht geladen in {:.2?}",
        t.elapsed()
    );
    let src = Arc::new(src);
    *state.develop.full.locked() = Some(FullEntry {
        id,
        file,
        source: src.clone(),
        scaled: None,
    });
    // Der Eintrag kann schon wieder fehlen (Datei geändert, `forget`,
    // `release_large`) – das frisch geladene Bild direkt verwenden
    Ok(cached_full(state, id, max_edge).unwrap_or_else(|| fitted(&src, max_edge)))
}

/// Volle Auflösung (bzw. auf `max_edge` verkleinert) aus dem Speicher, falls
/// es dieses Foto in unveränderter Fassung ist.
fn cached_full(state: &AppState, id: i64, max_edge: usize) -> Option<Arc<Source>> {
    let mut f = state.develop.full.locked();
    let e = f.as_mut().filter(|e| e.id == id)?;
    if e.file
        .changed_throttled(std::time::Instant::now(), STAMP_INTERVAL)
    {
        *f = None;
        return None;
    }
    if max_edge >= e.source.image.long_edge() {
        return Some(e.source.clone());
    }
    if let Some((m, s)) = &e.scaled
        && *m == max_edge
    {
        return Some(s.clone());
    }
    let s = Arc::new(e.source.downscaled(max_edge));
    e.scaled = Some((max_edge, s.clone()));
    Some(s)
}

fn load_denoised_full(
    state: &AppState,
    denoise_id: &str,
    max_edge: usize,
) -> Option<Arc<ImageF32>> {
    let mut f = state.develop.full_denoised.locked();
    if f.as_ref().is_none_or(|(k, _, _)| k != denoise_id) {
        *f = None;
        let path = lumenkorn_develop::denoised::path(&state.denoised_dir, denoise_id)?;
        let img = lumenkorn_develop::denoised::read(&path, None).ok()?;
        *f = Some((denoise_id.to_string(), Arc::new(img), None));
    }
    let (_, img, scaled) = f.as_mut()?;
    if max_edge >= img.long_edge() {
        return Some(img.clone());
    }
    if let Some((m, s)) = scaled
        && *m == max_edge
    {
        return Some(s.clone());
    }
    let s = Arc::new(img.downscale_to(max_edge));
    *scaled = Some((max_edge, s.clone()));
    Some(s)
}

/// Render-Optionen mit den Masken- und Reparaturdateien der App;
/// Überlagerung und Ausschnitt setzen die Aufrufer bei Bedarf selbst.
pub(crate) fn render_opts<'a>(
    state: &'a AppState,
    denoised: Option<&'a ImageF32>,
) -> RenderOptions<'a> {
    RenderOptions {
        masks: state.masks.as_ref(),
        overlay: None,
        denoised,
        region: None,
        repairs: state.repairs.as_ref(),
        keep_prepared: false,
    }
}

/// Schnappschüsse eines Fotos (alle Bearbeitungen außer der aktuellen).
fn snapshots(cat: &Catalog, id: i64) -> Result<Vec<Edit>, Error> {
    Ok(cat
        .edits(id)?
        .into_iter()
        .filter(|e| !e.is_current)
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevelopInfo {
    params: DevelopParams,
    full_width: usize,
    full_height: usize,
    raw: bool,
    snapshots: Vec<Edit>,
    /// Korrekturdaten der Kamera, falls das RAW welche enthält
    lens: Option<lumenkorn_develop::LensSummary>,
    /// Farbtemperatur des Kamera-Weißabgleichs (nur RAWs): Dann zeigt der
    /// Weißabgleich Kelvin statt einer relativen Temperatur
    as_shot_kelvin: Option<f32>,
}

/// Lädt das Bild in den Bearbeitungs-Cache und liefert die aktuelle Bearbeitung.
#[tauri::command]
pub async fn develop_load(app: AppHandle, id: i64) -> Result<DevelopInfo, Error> {
    blocking(app, move |state| {
        // Beim Öffnen immer prüfen, ob die Datei von außen geändert wurde
        state.develop.recheck_file(id);
        let src = load_proxy(state, id, PROXY_EDGE)?;
        let cat = state.catalog.locked();
        let mut params = cat.develop_params(id)?;
        // Ältere Bearbeitung mit relativer Temperatur: als Kelvin zeigen
        // (gleiches Verhältnis Rot zu Blau); gespeichert wird erst bei einer Änderung
        if let Some(k0) = src.as_shot_kelvin
            && params.kelvin == 0.0
            && params.temperature != 0.0
        {
            let ratio = (0.7 * params.temperature / 100.0).exp();
            params.kelvin =
                (lumenkorn_develop::wb::kelvin_for_ratio(k0, ratio) / 10.0).round() * 10.0;
            params.temperature = 0.0;
        }
        Ok(DevelopInfo {
            params,
            full_width: src.full_width,
            full_height: src.full_height,
            raw: src.kind == lumenkorn_develop::SourceKind::Raw,
            snapshots: snapshots(&cat, id)?,
            lens: src.lens.as_ref().map(|l| l.summary()),
            as_shot_kelvin: src.as_shot_kelvin.map(|k| (k / 10.0).round() * 10.0),
        })
    })
    .await
}

/// Rendert die Vorschau mit den übergebenen (noch ungespeicherten) Parametern.
/// Antwortet binär mit einem JPEG – ohne Umweg über JSON/Base64.
#[tauri::command]
pub async fn develop_render(
    app: AppHandle,
    id: i64,
    params: DevelopParams,
    max_edge: Option<usize>,
    overlay: Option<usize>,
    overlay_repair: Option<usize>,
) -> Result<Response, Error> {
    blocking(app, move |state| {
        // Auswahl einer Reparatur rot zeigen: als zusätzliche, wirkungslose Maskenebene
        let mut params = params;
        let mut overlay = overlay;
        if let Some(i) = overlay_repair
            && let Some(r) = params.repairs.get(i)
        {
            let m = r.as_mask();
            params.masks.push(m);
            overlay = Some(params.masks.len() - 1);
        }
        let edge = max_edge.unwrap_or(PROXY_EDGE).clamp(256, PROXY_EDGE);
        let src = load_proxy(state, id, edge)?;
        let denoised = params
            .denoise
            .as_ref()
            .and_then(|d| load_denoised(state, &d.id, edge));
        let opts = RenderOptions {
            overlay,
            keep_prepared: true,
            ..render_opts(state, denoised.as_deref())
        };
        let rgb = lumenkorn_develop::render_auto_rgb8(&src, &params, &opts);
        Ok(Response::new(output::encode_jpeg_rgb8(&rgb, 88)?))
    })
    .await
}

/// Pipette für Farbmasken: Farbe (Oklab) an der Stelle (x, y), normiert auf
/// das aufrechte Original – gemessen im unbearbeiteten Quellbild.
#[tauri::command]
pub async fn develop_sample_color(
    app: AppHandle,
    id: i64,
    x: f32,
    y: f32,
) -> Result<[f32; 3], Error> {
    if !(x.is_finite() && y.is_finite()) {
        return Err(Error::Invalid(
            tr_s("Ungültige Position", "Invalid position").into(),
        ));
    }
    blocking(app, move |state| {
        let src = load_proxy(state, id, PROXY_EDGE)?;
        Ok(lumenkorn_develop::masks::sample_color(&src.image, x, y))
    })
    .await
}

/// Ausschnitt des bearbeiteten Bildes in hoher Auflösung für die Zoomansicht.
/// `region` ist normiert auf das fertige Bild (nach Zuschnitt), `scale` das
/// Verhältnis von Bildschirm- zu Originalpixeln (1,0 = 100 %).
#[tauri::command]
pub async fn develop_render_detail(
    app: AppHandle,
    id: i64,
    params: DevelopParams,
    region: Crop,
    scale: f32,
) -> Result<Response, Error> {
    blocking(app, move |state| {
        let scale = if scale.is_finite() {
            scale.clamp(0.05, 1.0)
        } else {
            1.0
        };
        // Für Zoomstufen unter 100 % reicht eine verkleinerte Quelle
        let full_long = {
            let f = state.develop.full.locked();
            f.as_ref()
                .filter(|e| e.id == id)
                .map(|e| e.source.image.long_edge())
        };
        let full_long = match full_long {
            Some(l) => l,
            None => load_full(state, id, usize::MAX)?.image.long_edge(),
        };
        let edge = zoom_edge(full_long, scale);
        let src = load_full(state, id, edge)?;
        let denoised = params
            .denoise
            .as_ref()
            .and_then(|d| load_denoised_full(state, &d.id, src.image.long_edge()));
        let opts = RenderOptions {
            region: Some(region),
            keep_prepared: true,
            ..render_opts(state, denoised.as_deref())
        };
        let t = std::time::Instant::now();
        let rgb = lumenkorn_develop::render_auto_rgb8(&src, &params, &opts);
        log::debug!(
            "Detail {}×{} in {:.0?}",
            rgb.width(),
            rgb.height(),
            t.elapsed()
        );
        Ok(Response::new(output::encode_jpeg_rgb8(&rgb, 90)?))
    })
    .await
}

/// Schätzt die Korrektur fürs Geraderichten (Grad) im aktuell gedrehten und
/// gespiegelten Bild. `None`, wenn keine eindeutigen Kanten gefunden werden.
#[tauri::command]
pub async fn develop_detect_tilt(
    app: AppHandle,
    id: i64,
    rotate: u8,
    flip: bool,
) -> Result<Option<f32>, Error> {
    blocking(app, move |state| {
        let src = load_proxy(state, id, 1024)?;
        let p = DevelopParams {
            rotate,
            flip_horizontal: flip,
            lens_correction: lens_setting(state, id),
            ..Default::default()
        };
        let img = lumenkorn_develop::render(&src, &p);
        Ok(lumenkorn_develop::geometry::detect_tilt(&img))
    })
    .await
}

/// „Auto“: Tonwerte und Dynamik für das Foto mit den übergebenen (noch
/// ungespeicherten) Einstellungen vorschlagen.
#[tauri::command]
pub async fn develop_auto(
    app: AppHandle,
    id: i64,
    params: DevelopParams,
) -> Result<crate::style::AutoResult, Error> {
    blocking(app, move |state| {
        let src = load_proxy(state, id, PROXY_EDGE)?;
        let t = std::time::Instant::now();
        // Nach dem gelernten Stil angepasst, falls eingeschaltet
        let a = crate::style::auto(state, id, &src, &params);
        log::info!("Auto für Bild {id} in {:.2?}: {a:?}", t.elapsed());
        Ok(a)
    })
    .await
}

/// Automatischer Weißabgleich: (Temperatur, Tönung) für das Foto
#[tauri::command]
pub async fn develop_auto_wb(
    app: AppHandle,
    id: i64,
    params: DevelopParams,
) -> Result<lumenkorn_develop::auto::AutoWb, Error> {
    blocking(app, move |state| {
        let src = load_proxy(state, id, PROXY_EDGE)?;
        let wb = lumenkorn_develop::auto::auto_white_balance(&src, &params);
        log::info!("Auto-Weißabgleich für Bild {id}: {wb:?}");
        Ok(wb)
    })
    .await
}

/// Ob für das Foto die Objektivkorrektur eingeschaltet ist (gespeicherter
/// Stand; die Oberfläche speichert Änderungen sofort).
pub(crate) fn lens_setting(state: &AppState, id: i64) -> bool {
    state
        .catalog
        .locked()
        .develop_params(id)
        .map_or(true, |p| p.lens_correction)
}

/// Gespeicherte Einstellungen eines Fotos – zum Kopieren, ohne es zu laden.
#[tauri::command(async)]
pub fn develop_params(state: State<AppState>, id: i64) -> Result<DevelopParams, Error> {
    state.catalog.locked().develop_params(id)
}

/// Speichert Parameter für ein oder mehrere Bilder (Einfügen auf Auswahl).
#[tauri::command(async)]
pub fn develop_save(
    app: AppHandle,
    state: State<AppState>,
    ids: Vec<i64>,
    params: DevelopParams,
) -> Result<(), Error> {
    state.catalog.locked().save_develop(&ids, &params)?;
    // Lernbeispiel für „Auto nach deinem Stil“ (nach einer Pause, im Hintergrund)
    crate::style::nudge(&state);
    // Das gerade bearbeitete Foto liegt als Proxy im Speicher: Vorschau und
    // Thumbnail daraus rendern statt das RAW erneut in voller Auflösung zu
    // entwickeln (das kostete bei jedem Speichern Sekunden und ~1,5 GB).
    // Zurückgesetzt: Die eingebettete Kameravorschau ist dann das Thumbnail.
    if let [id] = ids[..]
        && !params.is_default()
        && let Some(proxy) = state.develop.get(id, PROXY_EDGE)
        // Ohne startbaren Thread: wie ohne Proxy über den Worker rendern
        && let Some(q) = save_queue(&app)
    {
        q.push(id, proxy, params);
        return Ok(());
    }
    for id in &ids {
        state.cache.remove(*id);
    }
    state.worker.submit(Job::Thumbs);
    Ok(())
}

/// Wartende Vorschau-Aufträge nach dem Speichern: je Foto nur der neueste.
/// Ein einziger Hintergrundthread arbeitet sie nacheinander ab, damit mehrere
/// Speichervorgänge nicht parallel mit der Live-Vorschau um die GPU konkurrieren.
struct SaveQueue {
    pending: Mutex<std::collections::HashMap<i64, (Arc<Source>, DevelopParams)>>,
    wake: std::sync::Condvar,
}

impl SaveQueue {
    fn push(&self, id: i64, proxy: Arc<Source>, params: DevelopParams) {
        self.pending.locked().insert(id, (proxy, params));
        self.wake.notify_one();
    }
}

/// Kurz warten, ob schnell ein neuerer Stand desselben Fotos folgt.
const SAVE_SETTLE: std::time::Duration = std::time::Duration::from_millis(300);

/// Die Warteschlange; der Thread startet beim ersten Speichern.
fn save_queue(app: &AppHandle) -> Option<Arc<SaveQueue>> {
    static QUEUE: std::sync::OnceLock<Option<Arc<SaveQueue>>> = std::sync::OnceLock::new();
    QUEUE
        .get_or_init(|| {
            let q = Arc::new(SaveQueue {
                pending: Mutex::new(Default::default()),
                wake: std::sync::Condvar::new(),
            });
            let (qt, app) = (q.clone(), app.clone());
            std::thread::Builder::new()
                .name("lumenkorn-save-thumbs".into())
                .spawn(move || save_thumbs_loop(&app, &qt))
                .ok()
                .map(|_| q)
        })
        .clone()
}

fn save_thumbs_loop(app: &AppHandle, q: &SaveQueue) {
    loop {
        let id = {
            let mut p = q.pending.locked();
            loop {
                if let Some(&id) = p.keys().next() {
                    break id;
                }
                p = q.wake.wait(p).unwrap_or_else(|e| e.into_inner());
            }
        };
        // Folgt gleich ein neuerer Stand, rendert nur dieser
        std::thread::sleep(SAVE_SETTLE);
        let Some((proxy, params)) = q.pending.locked().remove(&id) else {
            continue;
        };
        let state = app.state::<AppState>();
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            thumbs_from_proxy(&state, id, &proxy, &params)
        }))
        .unwrap_or_else(|_| {
            Err(Error::Invalid(
                tr_s("Interner Fehler", "Internal error").into(),
            ))
        });
        match res {
            Ok(true) => {
                let _ = app.emit("thumbs-ready", serde_json::json!({ "ids": [id] }));
            }
            Ok(false) => {} // inzwischen neuer gespeichert – der rendert selbst
            Err(e) => {
                log::warn!("Vorschau aus Proxy für {id}: {e}");
                state.cache.remove(id);
                state.worker.submit(Job::Thumbs);
            }
        }
    }
}

/// Rendert Vorschau (2048 px) und Thumbnail (512 px) aus dem Bearbeitungs-
/// Proxy. `false`, wenn die Parameter nicht mehr die gespeicherten sind.
fn thumbs_from_proxy(
    state: &AppState,
    id: i64,
    proxy: &Source,
    params: &DevelopParams,
) -> Result<bool, Error> {
    use lumenkorn_core::thumbs::{PREVIEW_SIZE, THUMB_SIZE, Variant};
    let denoised = params
        .denoise
        .as_ref()
        .and_then(|d| load_denoised(state, &d.id, PROXY_EDGE));
    let preview = lumenkorn_develop::render_auto_rgb8(
        proxy,
        params,
        &render_opts(state, denoised.as_deref()),
    );
    // Auf `max` Pixel lange Kante, seitenverhältnistreu
    let scaled = |img: &image::RgbImage, max: u32| {
        let long = img.width().max(img.height());
        image::imageops::thumbnail(
            img,
            (img.width() * max / long).max(1),
            (img.height() * max / long).max(1),
        )
    };
    let thumb = scaled(&preview, THUMB_SIZE);
    // Der Proxy ist höchstens PROXY_EDGE = PREVIEW_SIZE groß, die Vorschau
    // passt also schon; verkleinert wird nur zur Sicherheit.
    let preview = if preview.width().max(preview.height()) > PREVIEW_SIZE {
        scaled(&preview, PREVIEW_SIZE)
    } else {
        preview
    };
    // Kodieren ohne Katalogsperre; unter der Sperre nur prüfen, schreiben, markieren
    let (preview, thumb) = (
        output::encode_jpeg_rgb8(&preview, 84)?,
        output::encode_jpeg_rgb8(&thumb, 84)?,
    );
    // Dateien ohne Katalogsperre schreiben; die Parameter vorher und nachher
    // kurz prüfen. Ist es danach veraltet, wird nicht markiert – der neuere
    // Stand rendert und überschreibt die Dateien.
    if state.catalog.locked().develop_params(id)? != params.sanitized() {
        return Ok(false);
    }
    state.cache.write(Variant::Preview, id, &preview)?;
    state.cache.write(Variant::Thumb, id, &thumb)?;
    let cat = state.catalog.locked();
    if cat.develop_params(id)? != params.sanitized() {
        return Ok(false);
    }
    cat.set_thumb_state(id, true)?;
    Ok(true)
}

/// Überträgt Teile einer Bearbeitung auf andere Fotos; deren übrige
/// Einstellungen (und alles Fotospezifische) bleiben erhalten.
#[tauri::command(async)]
pub fn develop_transfer(
    state: State<AppState>,
    ids: Vec<i64>,
    source: DevelopParams,
    groups: lumenkorn_develop::TransferGroups,
) -> Result<(), Error> {
    let source = source.sanitized();
    {
        let mut cat = state.catalog.locked();
        for id in &ids {
            let merged = cat.develop_params(*id)?.transfer_from(&source, groups);
            cat.save_develop(&[*id], &merged)?;
        }
    }
    for id in &ids {
        state.cache.remove(*id);
    }
    state.worker.submit(Job::Thumbs);
    Ok(())
}

// --- Presets ------------------------------------------------------------------
// Angewendet werden sie wie Einfügen über `develop_transfer` mit den Gruppen des
// Presets; hier nur Anlegen, Umbenennen, Löschen.

#[tauri::command(async)]
pub fn preset_list(state: State<AppState>) -> Result<Vec<Preset>, Error> {
    state.catalog.locked().presets()
}

#[tauri::command(async)]
pub fn preset_create(
    state: State<AppState>,
    name: String,
    params: DevelopParams,
    groups: lumenkorn_develop::TransferGroups,
) -> Result<Vec<Preset>, Error> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Invalid(
            tr_s(
                "Das Preset braucht einen Namen.",
                "The preset needs a name.",
            )
            .into(),
        ));
    }
    let cat = state.catalog.locked();
    cat.create_preset(name, &params, groups)?;
    cat.presets()
}

#[tauri::command(async)]
pub fn preset_rename(state: State<AppState>, id: i64, name: String) -> Result<Vec<Preset>, Error> {
    let cat = state.catalog.locked();
    if !name.trim().is_empty() {
        cat.rename_preset(id, name.trim())?;
    }
    cat.presets()
}

#[tauri::command(async)]
pub fn preset_delete(state: State<AppState>, id: i64) -> Result<Vec<Preset>, Error> {
    let cat = state.catalog.locked();
    cat.delete_preset(id)?;
    cat.presets()
}

#[tauri::command(async)]
pub fn develop_snapshot(state: State<AppState>, id: i64, name: String) -> Result<Vec<Edit>, Error> {
    let cat = state.catalog.locked();
    let name = if name.trim().is_empty() {
        tr_s("Schnappschuss", "Snapshot").to_string()
    } else {
        name.trim().to_string()
    };
    cat.create_snapshot(id, &name)?;
    snapshots(&cat, id)
}

#[tauri::command(async)]
pub fn develop_restore(state: State<AppState>, edit_id: i64) -> Result<DevelopParams, Error> {
    let p = state.catalog.locked().restore_snapshot(edit_id)?;
    state.worker.submit(Job::Thumbs);
    Ok(p)
}

#[tauri::command(async)]
pub fn develop_delete_snapshot(
    state: State<AppState>,
    id: i64,
    edit_id: i64,
) -> Result<Vec<Edit>, Error> {
    let cat = state.catalog.locked();
    cat.delete_snapshot(edit_id)?;
    snapshots(&cat, id)
}

// --- Export -------------------------------------------------------------------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ExportProgress {
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
fn develop_for_export(
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

/// Exportiert in voller Auflösung im Hintergrund. Fortschritt per `export-progress`.
/// Ohne `dir` landet jedes Foto im Unterordner „Export“ neben dem Original.
#[tauri::command(async)]
pub fn export_images(
    app: AppHandle,
    ids: Vec<i64>,
    dir: Option<String>,
    options: ExportOptions,
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
    let Some(running) = crate::busy::claim_in(&app, |s| &s.develop.exporting) else {
        return Err(Error::Invalid(
            tr_s(
                "Es läuft bereits ein Export – bitte warten, bis er fertig ist.",
                "An export is already running – please wait until it has finished.",
            )
            .into(),
        ));
    };
    // Anzeige des Ziels: gewählter Ordner bzw. die tatsächlich benutzten
    let chosen = dir.clone();
    let shown = move |used: &std::collections::BTreeSet<PathBuf>| match (&chosen, used.len()) {
        (Some(d), _) => d.display().to_string(),
        (None, 1) => used.first().unwrap().display().to_string(),
        (None, 0) => tr_s("Unterordner „Export“", "Subfolder “Export”").to_string(),
        (None, n) => tr!("„Export“ in {n} Ordnern", "“Export” in {n} folders"),
    };
    std::thread::Builder::new()
        .name("lumenkorn-export".into())
        .spawn(move || {
            let _running = running;
            let state = app.state::<AppState>();
            // Ohne gewählten Ordner: Unterordner aus den Einstellungen
            let sub = crate::settings::read(&state.data_dir).export_subfolder;
            let formats = options.all_formats();
            let total = ids.len();
            let mut failed = Vec::new();
            let mut skipped = 0;
            let mut used = std::collections::BTreeSet::new();
            // In diesem Export geschrieben: nie überschreiben (z. B. IMG_1.CR2
            // und IMG_1.JPG aus demselben Ordner)
            let mut written = std::collections::HashSet::new();
            for (i, id) in ids.iter().enumerate() {
                let mut cur_name: Option<String> = None;
                // Ein Panic bei einem Bild (z. B. in der GPU-Pipeline) darf den
                // Export nicht beenden: Das Bild gilt als fehlgeschlagen.
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> Result<String, Error> {
                        let (job, params, meta) = {
                            let cat = state.catalog.locked();
                            // Fehlende Metadaten verhindern den Export nicht
                            let meta = cat
                                .export_meta(*id)
                                .inspect_err(|e| log::warn!("Metadaten für Export {id}: {e}"))
                                .ok();
                            (cat.thumb_job(*id)?, cat.develop_params(*id)?, meta)
                        };
                        let name = Path::new(&job.path)
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| id.to_string());
                        cur_name = Some(name.clone());
                        let _ = app.emit(
                            "export-progress",
                            ExportProgress {
                                done: i,
                                total,
                                current: Some(name.clone()),
                                failed: failed.clone(),
                                skipped,
                                finished: false,
                                dir: shown(&used),
                            },
                        );
                        let target_dir =
                            match &dir {
                                Some(d) => d.clone(),
                                None => lumenkorn_core::scanner::export_dir_for(
                                    Path::new(&job.path),
                                    &sub,
                                )
                                .map_err(|e| {
                                    Error::Invalid(tr!("Export-Ordner: {e}", "Export folder: {e}"))
                                })?,
                            };
                        used.insert(target_dir.clone());
                        let orientation = job.orientation.map(|o| o as u16);
                        // Entwickelt wird einmal für alle Formate (außer dem Original-DNG)
                        let mut developed = None;
                        let mut errors = Vec::new();
                        for format in &formats {
                            let Some(target) = export_target(
                                &target_dir,
                                &name,
                                format.extension(),
                                options.on_conflict,
                                &written,
                                |p| {
                                    state
                                        .catalog
                                        .locked()
                                        .has_image_path(&p.to_string_lossy())
                                        .unwrap_or(true)
                                },
                            ) else {
                                skipped += 1;
                                continue;
                            };
                            written.insert(target.clone());
                            let res = if *format == ExportFormat::Dng && !options.dng_edited {
                                // Original ohne Bearbeitung: die Szenendaten der Quelle
                                Source::load(
                                    Path::new(&job.path),
                                    job.kind == ImageKind::Raw,
                                    orientation,
                                    options.long_edge.filter(|m| *m > 0),
                                )
                                .and_then(|src| {
                                    output::write_original_dng(
                                        &src.image,
                                        &target,
                                        &options,
                                        meta.as_ref(),
                                    )
                                })
                                .map_err(Error::from)
                            } else {
                                let img = developed.get_or_insert_with(|| {
                                    develop_for_export(&state, &job, &params, options.long_edge)
                                });
                                match img {
                                    Ok(img) => {
                                        let opts = ExportOptions {
                                            format: *format,
                                            ..options.clone()
                                        };
                                        output::write(img, &target, &opts, meta.as_ref())
                                            .map_err(Error::from)
                                    }
                                    Err(e) => Err(Error::Invalid(e.to_string())),
                                }
                            };
                            if let Err(e) = res {
                                errors.push(if formats.len() > 1 {
                                    format!("{}: {e}", format.extension().to_uppercase())
                                } else {
                                    e.to_string()
                                });
                            }
                        }
                        if errors.is_empty() {
                            Ok(name)
                        } else {
                            Err(Error::Invalid(errors.join("; ")))
                        }
                    },
                ))
                .unwrap_or_else(|_| {
                    Err(Error::Invalid(
                        tr_s("Interner Fehler", "Internal error").into(),
                    ))
                });
                if let Err(e) = res {
                    log::warn!("Export {id}: {e}");
                    match cur_name {
                        Some(n) => failed.push(format!("{n}: {e}")),
                        None => failed.push(format!("{id}: {e}")),
                    }
                }
            }
            // Puffer in voller Auflösung nicht bis zum nächsten Export behalten
            lumenkorn_develop::release_large_caches();
            let _ = app.emit(
                "export-progress",
                ExportProgress {
                    done: total,
                    total,
                    current: None,
                    failed,
                    skipped,
                    finished: true,
                    dir: shown(&used),
                },
            );
        })?;
    Ok(())
}

/// Zieldatei eines Exports nach `mode`; `None`: überspringen. Originale der
/// Bibliothek und Dateien aus diesem Export werden nie überschrieben, sondern
/// bekommen einen neuen Namen.
fn export_target(
    dir: &Path,
    stem: &str,
    ext: &str,
    mode: output::OnConflict,
    written: &std::collections::HashSet<PathBuf>,
    is_original: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() && !written.contains(&first) {
        return Some(first);
    }
    let protected = written.contains(&first) || is_original(&first);
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
fn planned_export_dir(dir: Option<&Path>, sub: &str, photo: &Path) -> Option<PathBuf> {
    match dir {
        Some(d) => Some(d.to_path_buf()),
        None => lumenkorn_core::scanner::export_dir_path(photo, sub),
    }
}

/// Vor dem Export: Welche Zieldateien gibt es schon? (Dateinamen; die
/// Oberfläche fragt dann, ob überschrieben, zusätzlich exportiert oder
/// übersprungen werden soll)
#[tauri::command(async)]
pub fn export_conflicts(
    state: State<AppState>,
    ids: Vec<i64>,
    dir: Option<String>,
    options: ExportOptions,
) -> Result<Vec<String>, Error> {
    let dir = dir.map(PathBuf::from);
    let sub = crate::settings::read(&state.data_dir).export_subfolder;
    let formats = options.all_formats();
    let paths: Vec<String> = {
        let cat = state.catalog.locked();
        ids.iter()
            .filter_map(|id| cat.thumb_job(*id).ok().map(|j| j.path))
            .collect()
    };
    let mut out = Vec::new();
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
            if t.exists() {
                out.push(
                    t.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    Ok(out)
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
    use super::{FileStamp, export_target, output::OnConflict, zoom_edge};

    #[test]
    fn export_target_follows_conflict_mode() {
        let dir = std::env::temp_dir().join(format!("lw-target-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.jpg"), b"alt").unwrap();
        std::fs::write(dir.join("orig.jpg"), b"original").unwrap();
        let none = std::collections::HashSet::new();
        let lib = |p: &std::path::Path| p.ends_with("orig.jpg");
        let t = |stem, mode, written: &std::collections::HashSet<_>| {
            export_target(&dir, stem, "jpg", mode, written, lib)
        };
        // Frei: immer der eigentliche Name
        assert_eq!(t("neu", OnConflict::Skip, &none), Some(dir.join("neu.jpg")));
        assert_eq!(
            t("a", OnConflict::Overwrite, &none),
            Some(dir.join("a.jpg"))
        );
        assert_eq!(t("a", OnConflict::Rename, &none), Some(dir.join("a-2.jpg")));
        assert_eq!(t("a", OnConflict::Skip, &none), None);
        // Originale der Bibliothek nie überschreiben, auch nicht auslassen
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

    #[test]
    fn file_stamp_notices_external_changes() {
        let dir = std::env::temp_dir().join(format!("lw-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("foto.jpg");
        std::fs::write(&p, b"alt").unwrap();
        let s = FileStamp::of(&p);
        assert!(!s.changed());
        std::fs::write(&p, b"neuer Inhalt").unwrap();
        assert!(s.changed(), "andere Größe");
        let mut s = FileStamp::of(&p);
        std::fs::remove_file(&p).unwrap();
        assert!(s.changed(), "Datei weg");
        // Gedrosselt: innerhalb des Intervalls keine Prüfung, danach schon
        let (t0, iv) = (std::time::Instant::now(), std::time::Duration::from_secs(2));
        s.checked = Some(t0);
        assert!(!s.changed_throttled(t0 + iv / 2, iv));
        assert!(s.changed_throttled(t0 + iv, iv));
        s.checked = None;
        assert!(
            s.changed_throttled(t0, iv),
            "ohne Zeitstempel sofort prüfen"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zoom_edges_snap_to_steps() {
        assert_eq!(zoom_edge(9504, 1.0), 9504);
        assert_eq!(zoom_edge(9504, 0.30), 3072);
        assert_eq!(
            zoom_edge(9504, 0.31),
            3072,
            "kleiner Zoomschritt: gleiche Quelle"
        );
        assert_eq!(zoom_edge(9504, 0.01), 512, "kleinste Stufe");
        assert_eq!(zoom_edge(9504, 0.999), 9504);
    }
}
