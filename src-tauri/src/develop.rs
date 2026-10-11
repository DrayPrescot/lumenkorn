//! Entwickeln: Proxy-Cache, Live-Vorschau, Speichern, Snapshots und Export.

use crate::worker::Job;
use crate::{AppState, blocking};
use lumenkorn_core::formats::ImageKind;
use lumenkorn_core::{Catalog, Edit, Error, Preset};
use lumenkorn_develop::i18n::tr_s;
use lumenkorn_develop::output::{self, ExportOptions};
use lumenkorn_develop::{Crop, DevelopParams, ImageF32, RenderOptions, Source, tr};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Manager, State};

/// Längste Kante des Bearbeitungs-Proxys.
pub const PROXY_EDGE: usize = 2048;
const CACHE_ENTRIES: usize = 4;

struct Entry {
    id: i64,
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
    full_denoised: Mutex<Option<FullDenoised>>,
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
    source: Arc<Source>,
    scaled: Option<(usize, Arc<Source>)>,
}

impl DevelopCache {
    fn get(&self, id: i64, max_edge: usize) -> Option<Arc<Source>> {
        let mut entries = self.entries.lock().unwrap();
        let pos = entries.iter().position(|e| e.id == id)?;
        let mut e = entries.remove(pos);
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

    fn insert(&self, id: i64, proxy: Arc<Source>) {
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|e| e.id != id);
        entries.insert(
            0,
            Entry {
                id,
                proxy,
                scaled: None,
            },
        );
        entries.truncate(CACHE_ENTRIES);
    }

    /// Gibt die großen Puffer frei (volle Auflösung, entrauschte Fassungen).
    /// Die kleinen Proxys bleiben, damit das Zurückkehren schnell geht.
    pub fn release_large(&self) {
        *self.full.lock().unwrap() = None;
        *self.full_denoised.lock().unwrap() = None;
        self.denoised.lock().unwrap().clear();
    }

    pub fn forget(&self, id: i64) {
        self.entries.lock().unwrap().retain(|e| e.id != id);
        self.denoised.lock().unwrap().clear();
        *self.full_denoised.lock().unwrap() = None;
        let mut f = self.full.lock().unwrap();
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
        let c = state.develop.denoised.lock().unwrap();
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
    let mut c = state.develop.denoised.lock().unwrap();
    c.insert(0, (key, img.clone()));
    c.truncate(4);
    Some(img)
}

pub(crate) fn load_proxy(state: &AppState, id: i64, max_edge: usize) -> Result<Arc<Source>, Error> {
    if let Some(s) = state.develop.get(id, max_edge) {
        return Ok(s);
    }
    let job = state.catalog.lock().unwrap().thumb_job(id)?;
    let src = Source::load(
        Path::new(&job.path),
        job.kind == ImageKind::Raw,
        job.orientation.map(|o| o as u16),
        Some(PROXY_EDGE),
    )?;
    state.develop.insert(id, Arc::new(src));
    Ok(state.develop.get(id, max_edge).expect("gerade eingefügt"))
}

/// Quelle in voller Auflösung (bzw. verkleinert auf `max_edge`, wenn kleiner).
fn load_full(state: &AppState, id: i64, max_edge: usize) -> Result<Arc<Source>, Error> {
    let mut f = state.develop.full.lock().unwrap();
    if f.as_ref().is_none_or(|e| e.id != id) {
        *f = None; // alten Speicher vor dem Laden freigeben
        let job = state.catalog.lock().unwrap().thumb_job(id)?;
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
        *f = Some(FullEntry {
            id,
            source: Arc::new(src),
            scaled: None,
        });
    }
    let e = f.as_mut().expect("gerade geladen");
    if max_edge >= e.source.image.long_edge() {
        return Ok(e.source.clone());
    }
    if let Some((m, s)) = &e.scaled
        && *m == max_edge
    {
        return Ok(s.clone());
    }
    let s = Arc::new(e.source.downscaled(max_edge));
    e.scaled = Some((max_edge, s.clone()));
    Ok(s)
}

fn load_denoised_full(
    state: &AppState,
    denoise_id: &str,
    max_edge: usize,
) -> Option<Arc<ImageF32>> {
    let mut f = state.develop.full_denoised.lock().unwrap();
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
fn render_opts<'a>(state: &'a AppState, denoised: Option<&'a ImageF32>) -> RenderOptions<'a> {
    RenderOptions {
        masks: state.masks.as_ref(),
        overlay: None,
        denoised,
        region: None,
        repairs: state.repairs.as_ref(),
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
}

/// Lädt das Bild in den Bearbeitungs-Cache und liefert die aktuelle Bearbeitung.
#[tauri::command]
pub async fn develop_load(app: AppHandle, id: i64) -> Result<DevelopInfo, Error> {
    blocking(app, move |state| {
        let src = load_proxy(state, id, PROXY_EDGE)?;
        let cat = state.catalog.lock().unwrap();
        Ok(DevelopInfo {
            params: cat.develop_params(id)?,
            full_width: src.full_width,
            full_height: src.full_height,
            raw: src.kind == lumenkorn_develop::SourceKind::Raw,
            snapshots: snapshots(&cat, id)?,
            lens: src.lens.as_ref().map(|l| l.summary()),
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
            let f = state.develop.full.lock().unwrap();
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
) -> Result<lumenkorn_develop::auto::AutoTone, Error> {
    blocking(app, move |state| {
        let src = load_proxy(state, id, PROXY_EDGE)?;
        let t = std::time::Instant::now();
        let a = lumenkorn_develop::auto::auto_tone(&src, &params);
        log::info!("Auto für Bild {id} in {:.2?}: {a:?}", t.elapsed());
        Ok(a)
    })
    .await
}

/// Ob für das Foto die Objektivkorrektur eingeschaltet ist (gespeicherter
/// Stand; die Oberfläche speichert Änderungen sofort).
pub(crate) fn lens_setting(state: &AppState, id: i64) -> bool {
    state
        .catalog
        .lock()
        .unwrap()
        .develop_params(id)
        .map_or(true, |p| p.lens_correction)
}

/// Gespeicherte Einstellungen eines Fotos – zum Kopieren, ohne es zu laden.
#[tauri::command(async)]
pub fn develop_params(state: State<AppState>, id: i64) -> Result<DevelopParams, Error> {
    state.catalog.lock().unwrap().develop_params(id)
}

/// Speichert Parameter für ein oder mehrere Bilder (Einfügen auf Auswahl).
#[tauri::command(async)]
pub fn develop_save(
    app: AppHandle,
    state: State<AppState>,
    ids: Vec<i64>,
    params: DevelopParams,
) -> Result<(), Error> {
    state.catalog.lock().unwrap().save_develop(&ids, &params)?;
    // Das gerade bearbeitete Foto liegt als Proxy im Speicher: Vorschau und
    // Thumbnail daraus rendern statt das RAW erneut in voller Auflösung zu
    // entwickeln (das kostete bei jedem Speichern Sekunden und ~1,5 GB).
    // Zurückgesetzt: Die eingebettete Kameravorschau ist dann das Thumbnail.
    if let [id] = ids[..]
        && !params.is_default()
        && let Some(proxy) = state.develop.get(id, PROXY_EDGE)
    {
        let spawned = std::thread::Builder::new()
            .name("lumenkorn-save-thumbs".into())
            .spawn(move || {
                let state = app.state::<AppState>();
                match thumbs_from_proxy(&state, id, &proxy, &params) {
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
            });
        if spawned.is_ok() {
            return Ok(());
        }
        // Thread nicht startbar: wie ohne Proxy über den Worker rendern
    }
    for id in &ids {
        state.cache.remove(*id);
    }
    state.worker.submit(Job::Thumbs);
    Ok(())
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
    let cat = state.catalog.lock().unwrap();
    if cat.develop_params(id)? != params.sanitized() {
        return Ok(false);
    }
    state.cache.write(Variant::Preview, id, &preview)?;
    state.cache.write(Variant::Thumb, id, &thumb)?;
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
        let mut cat = state.catalog.lock().unwrap();
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
    state.catalog.lock().unwrap().presets()
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
    let cat = state.catalog.lock().unwrap();
    cat.create_preset(name, &params, groups)?;
    cat.presets()
}

#[tauri::command(async)]
pub fn preset_rename(state: State<AppState>, id: i64, name: String) -> Result<Vec<Preset>, Error> {
    let cat = state.catalog.lock().unwrap();
    if !name.trim().is_empty() {
        cat.rename_preset(id, name.trim())?;
    }
    cat.presets()
}

#[tauri::command(async)]
pub fn preset_delete(state: State<AppState>, id: i64) -> Result<Vec<Preset>, Error> {
    let cat = state.catalog.lock().unwrap();
    cat.delete_preset(id)?;
    cat.presets()
}

#[tauri::command(async)]
pub fn develop_snapshot(state: State<AppState>, id: i64, name: String) -> Result<Vec<Edit>, Error> {
    let cat = state.catalog.lock().unwrap();
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
    let p = state.catalog.lock().unwrap().restore_snapshot(edit_id)?;
    state.worker.submit(Job::Thumbs);
    Ok(p)
}

#[tauri::command(async)]
pub fn develop_delete_snapshot(
    state: State<AppState>,
    id: i64,
    edit_id: i64,
) -> Result<Vec<Edit>, Error> {
    let cat = state.catalog.lock().unwrap();
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
    finished: bool,
    dir: String,
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
            let state = app.state::<AppState>();
            let total = ids.len();
            let mut failed = Vec::new();
            let mut used = std::collections::BTreeSet::new();
            for (i, id) in ids.iter().enumerate() {
                let res = (|| -> Result<String, Error> {
                    let (job, params) = {
                        let cat = state.catalog.lock().unwrap();
                        (cat.thumb_job(*id)?, cat.develop_params(*id)?)
                    };
                    let name = Path::new(&job.path)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| id.to_string());
                    let _ = app.emit(
                        "export-progress",
                        ExportProgress {
                            done: i,
                            total,
                            current: Some(name.clone()),
                            failed: failed.clone(),
                            finished: false,
                            dir: shown(&used),
                        },
                    );
                    // Mit begrenzter Kantenlänge nicht in voller Auflösung
                    // entwickeln: Die Quelle muss nur groß genug sein, dass der
                    // Zuschnitt (und die Vergrößerung beim Geraderichten) die
                    // Zielgröße erreicht.
                    let load_edge = options.long_edge.filter(|m| *m > 0).map(|m| {
                        let crop = params.crop.map_or(1.0, |c| c.w.min(c.h)).max(0.05);
                        let tilt = if params.straighten != 0.0 { 1.5 } else { 1.0 };
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
                    let img = lumenkorn_develop::render_auto(
                        &src,
                        &params,
                        &render_opts(&state, denoised.as_ref()),
                    );
                    let target_dir = match &dir {
                        Some(d) => d.clone(),
                        None => lumenkorn_core::scanner::export_dir_for(Path::new(&job.path))
                            .map_err(|e| {
                                Error::Invalid(tr!("Export-Ordner: {e}", "Export folder: {e}"))
                            })?,
                    };
                    let target = unique_path(&target_dir, &name, options.format.extension());
                    used.insert(target_dir);
                    output::write(&img, &target, &options)?;
                    Ok(name)
                })();
                if let Err(e) = res {
                    log::warn!("Export {id}: {e}");
                    failed.push(format!("{id}: {e}"));
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
                    finished: true,
                    dir: shown(&used),
                },
            );
        })?;
    Ok(())
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
    use super::zoom_edge;

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
