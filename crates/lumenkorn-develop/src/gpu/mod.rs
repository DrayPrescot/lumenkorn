//! Entwicklungspipeline auf der Grafikkarte (wgpu: Vulkan, DirectX 12, Metal).
//!
//! Dieselben Stufen wie [`crate::pipeline`], als Compute-Shader
//! (`pipeline.wgsl`). Nur die Formen der Maskenebenen werden weiter auf der CPU
//! berechnet – zwischengespeichert, sodass das beim Ziehen eines Reglers nicht
//! erneut passiert.
//!
//! Die CPU-Pipeline bleibt die Referenz: Kann die GPU ein Bild nicht rechnen
//! (kein passendes Gerät, zu wenig Grafikspeicher, Treiberfehler), fällt
//! [`crate::pipeline::render_auto`] stillschweigend auf die CPU zurück.

use crate::geometry;
use crate::image_f32::ImageF32;
use crate::masks;
use crate::params::DevelopParams;
use crate::pipeline::{RAW_BASE_CONTRAST, RenderOptions, white_balance};
use crate::source::{Source, SourceKind};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use wgpu::Buffer;

// --- Öffentliche Schnittstelle ---------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuInfo {
    /// Name laut Treiber, z. B. „AMD Radeon RX 7900 XTX (RADV NAVI31)“
    pub name: String,
    /// Grafikschnittstelle: Vulkan, DirectX 12, Metal
    pub backend: String,
    /// discrete, integrated, virtual, software, other
    pub kind: String,
    pub driver: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuStatus {
    /// Bildbearbeitung auf der GPU ist eingeschaltet
    pub enabled: bool,
    /// `None`: Erkennung läuft noch
    pub available: Option<bool>,
    pub info: Option<GpuInfo>,
    /// Warum keine GPU genutzt wird bzw. letzter Fehler beim Rechnen
    pub error: Option<String>,
}

static GPU: OnceLock<Result<Gpu, String>> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// Sucht und öffnet die Grafikkarte (einmal pro Prozess, blockiert ~0,1–1 s).
/// Reine Software-Renderer werden abgelehnt – die CPU-Pipeline ist schneller.
/// `LUMENKORN_GPU_SOFTWARE=1` erlaubt sie trotzdem (für Tests).
pub fn init() -> Result<&'static Gpu, String> {
    GPU.get_or_init(|| {
        let allow_sw = std::env::var("LUMENKORN_GPU_SOFTWARE").is_ok_and(|v| v == "1");
        let r = Gpu::new(allow_sw);
        match &r {
            Ok(g) => log::info!(
                "GPU für die Bildbearbeitung: {} ({})",
                g.info.name,
                g.info.backend
            ),
            Err(e) => log::info!("Keine GPU für die Bildbearbeitung: {e}"),
        }
        r
    })
    .as_ref()
    .map_err(Clone::clone)
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
    *LAST_ERROR.lock().unwrap() = None;
}

pub fn status() -> GpuStatus {
    let g = GPU.get();
    GpuStatus {
        enabled: ENABLED.load(Ordering::SeqCst),
        available: g.map(|r| r.is_ok()),
        info: g.and_then(|r| r.as_ref().ok()).map(|g| g.info.clone()),
        error: LAST_ERROR
            .lock()
            .unwrap()
            .clone()
            .or_else(|| g.and_then(|r| r.as_ref().err().cloned())),
    }
}

/// Gibt die zwischengespeicherten GPU-Puffer frei (Bildpuffer, Masken).
/// Geräte und Shader bleiben – das nächste Rendern legt die Puffer neu an.
pub(crate) fn trim() {
    if let Some(Ok(g)) = GPU.get() {
        let mut st = g.state.lock().unwrap();
        st.sets.clear();
        st.alphas.clear();
    }
}

/// Nur die großen Puffer freigeben (nach einem Export) – die der Vorschau
/// bleiben, damit das Weiterbearbeiten nicht neu hochladen muss.
pub(crate) fn trim_large() {
    if let Some(Ok(g)) = GPU.get() {
        let mut st = g.state.lock().unwrap();
        st.sets.retain(|s| s.cap <= crate::LARGE_PIXELS);
        st.alphas.retain(|(a, _)| a.len() <= crate::LARGE_PIXELS);
    }
}

/// Die GPU, falls eingeschaltet und bereit. Wartet nie auf die Erkennung.
pub(crate) fn active() -> Option<&'static Gpu> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    GPU.get()?.as_ref().ok()
}

pub(crate) fn note_error(e: &str) {
    log::warn!("GPU-Bildbearbeitung fehlgeschlagen, rechne auf der CPU: {e}");
    *LAST_ERROR.lock().unwrap() = Some(e.to_string());
}

/// Ergebnis eines GPU-Durchlaufs.
pub enum Rendered {
    Float(ImageF32),
    Rgb8(image::RgbImage),
}

// --- Parameter je Durchlauf (muss zu `struct U` im Shader passen) ---------------

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct U {
    w: u32,
    h: u32,
    mw: u32,
    mh: u32,
    dw: u32,
    dh: u32,
    radius: u32,
    flags: u32,
    wb_r: f32,
    wb_g: f32,
    wb_b: f32,
    gain: f32,
    denoise: f32,
    fx: f32,
    fy: f32,
    amount: f32,
    black: f32,
    white: f32,
    shadows: f32,
    highlights: f32,
    contrast: f32,
    clarity: f32,
    sat: f32,
    vib: f32,
    out_w: u32,
    out_h: u32,
    rot: u32,
    flip: u32,
    ow: u32,
    oh: u32,
    crop_x: u32,
    crop_y: u32,
    st_s: f32,
    st_c: f32,
    st_k: f32,
    /// Dehaze −1…1, Dunsthelligkeit und Aufhellung (`pipeline::Haze`)
    dehaze: f32,
    haze: f32,
    haze_lift: f32,
    _p2: f32,
    _p3: f32,
    /// HSL je Farbbereich: [0..2) Farbton, [2..4) Sättigung, [4..6) Luminanz
    hsl: [[f32; 4]; 6],
    /// Color Grading (`pipeline::GradeVals`): Tiefen, Mitten, Lichter, Global
    grade: [[f32; 4]; 4],
    /// Grenze, Übergangsbreite, ungenutzt
    grade_pw: [f32; 4],
}

const F_RAW: u32 = 1;
const F_DENOISE: u32 = 2;
const F_BASE: u32 = 4;
const F_DETAIL: u32 = 8;
const F_PACK: u32 = 16;
const F_STRAIGHTEN: u32 = 32;

#[derive(Clone, Copy)]
enum P {
    Prep,
    Luma,
    MapDown,
    BlurH,
    BlurV,
    Tone,
    Color,
    LocalWb,
    Blend,
    Sharpen,
    Overlay,
    Geom,
    Hsl,
    Dark,
    Dehaze,
    Grade,
    Mono,
}

const ENTRY_POINTS: [&str; 17] = [
    "prep",
    "luma_pass",
    "map_down",
    "blur_h",
    "blur_v",
    "tone",
    "color",
    "local_wb",
    "blend",
    "sharpen",
    "overlay",
    "geom",
    "hsl",
    "dark_pass",
    "dehaze_pass",
    "grade",
    "mono",
];

/// Zuordnung der Puffer zu den Bindungen 1–8 des Shaders.
#[derive(Default, Clone, Copy)]
struct Binds<'a> {
    ina: Option<&'a Buffer>,
    inb: Option<&'a Buffer>,
    outa: Option<&'a Buffer>,
    fa: Option<&'a Buffer>,
    fb: Option<&'a Buffer>,
    fc: Option<&'a Buffer>,
    fo: Option<&'a Buffer>,
    packed: Option<&'a Buffer>,
}

#[allow(clippy::large_enum_variant)] // wenige Dutzend pro Bild, Boxen lohnt nicht
enum Op<'a> {
    Run {
        p: P,
        u: U,
        b: Binds<'a>,
        groups: [u32; 3],
    },
    Copy {
        from: &'a Buffer,
        to: &'a Buffer,
        size: u64,
    },
}

fn img2d(w: usize, h: usize) -> [u32; 3] {
    [w.div_ceil(16) as u32, h.div_ceil(16) as u32, 1]
}

fn lines(n: usize) -> [u32; 3] {
    [n.div_ceil(64) as u32, 1, 1]
}

/// Größte Helligkeitskarte (local.rs: 640 px lange Kante)
const MAP_CAP: usize = 640 * 640;
/// Größere Sätze bleiben bis hierhin für die Zoomansicht erhalten (sonst
/// würde jedes Verschieben bei 100 % Gigabytes neu anlegen und hochladen);
/// freigegeben werden sie beim Verlassen des Entwickelns bzw. nach dem Export.
const KEEP_LARGE_MAX: usize = 64 << 20;

// --- Gerät und Puffer --------------------------------------------------------------

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub info: GpuInfo,
    layout: wgpu::BindGroupLayout,
    pipes: Vec<wgpu::ComputePipeline>,
    /// Ersatzpuffer für ungenutzte Bindungen (je Bindung einer)
    dummies: Vec<Buffer>,
    max_binding: u64,
    uniform_stride: u64,
    state: Mutex<State>,
}

struct ImgBuf {
    buf: Buffer,
    key: Option<ImageKey>,
}

/// Puffersatz für Bilder bis `cap` Pixel.
struct Bufs {
    cap: usize,
    src: ImgBuf,
    dn: Option<ImgBuf>,
    img: Buffer,
    loc: Buffer,
    tmp: Buffer,
    y: Buffer,
    packed: Buffer,
    staging: Buffer,
    used: u64,
}

struct State {
    sets: Vec<Bufs>,
    maps: [Buffer; 3],
    uniform: Option<(Buffer, u64)>,
    alphas: Vec<(Arc<Vec<f32>>, Buffer)>,
    tick: u64,
}

/// Erkennt, ob ein Quellbild schon auf der GPU liegt.
///
/// Adresse und Größe allein genügen nicht: Ein freigegebener großer Puffer
/// wird oft für das nächste gleich große Bild wiederverwendet. Deshalb gehören
/// dazu (a) Stichproben über das *ganze* Bild – verschiedene Fotos (Serien mit
/// gleichem Himmel) unterscheiden sich durch Rauschen praktisch überall – und
/// (b) eine Kennung des Aufrufers (`ident`, 0 = keine). Die braucht es für
/// Fassungen, die sich nur in einem kleinen Bereich unterscheiden, den keine
/// Stichprobe sicher trifft: Die Pipeline vergibt jeder neu erzeugten
/// reparierten Quelle eine fortlaufende Nummer (`pipeline::repaired_source`).
/// Ein vollständiger Inhalts-Hash wäre bei jedem Reglerzug zu teuer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ImageKey(usize, usize, usize, u64, u64);

/// Shader-Quelltext mit den Konstanten aus pipeline.rs/color.rs als Kopf –
/// so stehen die Zahlen nur einmal im Code. `{:?}` schreibt f32 immer mit
/// Dezimalpunkt und exakt umkehrbar, der Shader rechnet also mit denselben Werten.
fn shader_source() -> String {
    use crate::pipeline::{
        CLARITY_GAIN, CONTRAST_MIX, DEHAZE_CHROMA, DEHAZE_GAIN, DEHAZE_MIN_T, HAZE_ADD,
        OVERLAY_COLOR, OVERLAY_OPACITY, RAW_BASE_GAIN, RAW_WHITE, SHADOW_HIGHLIGHT_GAIN,
        SHARPEN_GAIN, SHARPEN_THRESHOLD, SOFT_CLIP_KNEE,
    };
    let f = |name: &str, v: f32| format!("const {name}: f32 = {v:?};\n");
    let list = |v: &[f32]| {
        v.iter()
            .map(|x| format!("{x:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let c = crate::color::HSL_CENTERS;
    [
        f("RAW_BASE_GAIN", RAW_BASE_GAIN),
        f("RAW_WHITE", RAW_WHITE),
        f("SOFT_CLIP_KNEE", SOFT_CLIP_KNEE),
        f("SHADOW_HIGHLIGHT_GAIN", SHADOW_HIGHLIGHT_GAIN),
        f("CLARITY_GAIN", CLARITY_GAIN),
        f("CONTRAST_MIX", CONTRAST_MIX),
        f("DEHAZE_GAIN", DEHAZE_GAIN),
        f("DEHAZE_CHROMA", DEHAZE_CHROMA),
        f("DEHAZE_MIN_T", DEHAZE_MIN_T),
        f("HAZE_ADD", HAZE_ADD),
        f("SHARPEN_GAIN", SHARPEN_GAIN),
        f("SHARPEN_THRESHOLD", SHARPEN_THRESHOLD),
        f("OVERLAY_OPACITY", OVERLAY_OPACITY),
        format!(
            "const OVERLAY_COLOR = vec3<f32>({});\n",
            list(&OVERLAY_COLOR)
        ),
        format!(
            "const HSL_CENTERS = array<f32, {}>({});\n",
            c.len(),
            list(&c)
        ),
        include_str!("pipeline.wgsl").to_string(),
    ]
    .concat()
}

/// Zahl der Stichproben für [`image_key`].
const KEY_SAMPLES: usize = 4096;

pub(crate) fn image_key(img: &ImageF32, ident: u64) -> ImageKey {
    let n = img.data.len();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    if n > 0 {
        // Gleichmäßig über alle Zeilen verteilt, je Abschnitt leicht versetzt
        // (sonst träfen die Proben bei passender Breite immer dieselbe Spalte).
        let step = n.div_ceil(KEY_SAMPLES).max(1);
        for k in 0..KEY_SAMPLES.min(n) {
            let i = (k * step + (k * 7919) % step).min(n - 1);
            for v in img.data[i] {
                h = (h ^ v.to_bits() as u64).wrapping_mul(0x100_0000_01b3);
            }
        }
    }
    ImageKey(img.data.as_ptr() as usize, img.width, img.height, h, ident)
}

/// Rückt den Eintrag `pos` ans Ende (zuletzt benutzt).
fn touch<T>(v: &mut Vec<T>, pos: usize) {
    let e = v.remove(pos);
    v.push(e);
}

/// Behält vom Ende her (zuletzt benutzt) so viele Einträge, wie in `limit`
/// Bytes passen; ältere am Anfang fallen weg.
fn evict_oldest<T>(v: &mut Vec<T>, size: impl Fn(&T) -> usize, limit: usize) {
    let mut bytes = 0usize;
    let keep_from = v
        .iter()
        .rposition(|e| {
            bytes += size(e);
            bytes > limit
        })
        .map_or(0, |i| i + 1);
    v.drain(..keep_from);
}

fn storage(device: &wgpu::Device, label: &str, bytes: u64) -> Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.max(16),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

impl Gpu {
    fn new(allow_software: bool) -> Result<Gpu, String> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY);
        let instance = wgpu::Instance::new(desc);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| crate::tr!("keine Grafikkarte gefunden ({e})", "no GPU found ({e})"))?;
        let ai = adapter.get_info();
        if ai.device_type == wgpu::DeviceType::Cpu && !allow_software {
            return Err(crate::tr!(
                "nur Software-Rendering verfügbar ({}) – die CPU-Pipeline ist schneller",
                "only software rendering available ({}) – the CPU pipeline is faster",
                ai.name
            ));
        }
        let al = adapter.limits();
        let limits = wgpu::Limits {
            max_storage_buffer_binding_size: al.max_storage_buffer_binding_size,
            max_buffer_size: al.max_buffer_size,
            ..wgpu::Limits::default()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("lumenkorn"),
            required_limits: limits.clone(),
            ..Default::default()
        }))
        .map_err(|e| crate::tr!("Grafikkarte nicht nutzbar ({e})", "GPU not usable ({e})"))?;
        // Fehler nie als Absturz: sie werden über Fehlerbereiche abgefangen.
        device.on_uncaptured_error(Arc::new(|e| log::error!("wgpu: {e}")));

        let info = GpuInfo {
            name: ai.name.clone(),
            backend: match ai.backend {
                wgpu::Backend::Vulkan => "Vulkan".into(),
                wgpu::Backend::Dx12 => "DirectX 12".into(),
                wgpu::Backend::Metal => "Metal".into(),
                b => format!("{b:?}"),
            },
            kind: match ai.device_type {
                wgpu::DeviceType::DiscreteGpu => "discrete",
                wgpu::DeviceType::IntegratedGpu => "integrated",
                wgpu::DeviceType::VirtualGpu => "virtual",
                wgpu::DeviceType::Cpu => "software",
                wgpu::DeviceType::Other => "other",
            }
            .into(),
            driver: format!("{} {}", ai.driver, ai.driver_info)
                .trim()
                .to_string(),
        };

        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pipeline.wgsl"),
            source: wgpu::ShaderSource::Wgsl(shader_source().into()),
        });
        let storage_entry = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let mut entries = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: std::num::NonZeroU64::new(std::mem::size_of::<U>() as u64),
            },
            count: None,
        }];
        for (b, ro) in [
            (1, true),
            (2, true),
            (3, false),
            (4, true),
            (5, true),
            (6, true),
            (7, false),
            (8, false),
        ] {
            entries.push(storage_entry(b, ro));
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lumenkorn"),
            entries: &entries,
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("lumenkorn"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipes = ENTRY_POINTS
            .iter()
            .map(|ep| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(ep),
                    layout: Some(&pl),
                    module: &module,
                    entry_point: Some(ep),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })
            .collect();
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(crate::tr!(
                "Shader nicht übersetzbar: {e}",
                "Shader failed to compile: {e}"
            ));
        }
        let dummies = (1..=8)
            .map(|i| storage(&device, &format!("leer{i}"), 16))
            .collect();
        let maps = [
            storage(&device, "karte-grob", (MAP_CAP * 4) as u64),
            storage(&device, "karte-fein", (MAP_CAP * 4) as u64),
            storage(&device, "karte-tmp", (MAP_CAP * 4) as u64),
        ];
        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let uniform_stride = (std::mem::size_of::<U>() as u64).next_multiple_of(align);
        let max_binding = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        Ok(Gpu {
            device,
            queue,
            info,
            layout,
            pipes,
            dummies,
            max_binding,
            uniform_stride,
            state: Mutex::new(State {
                sets: Vec::new(),
                maps,
                uniform: None,
                alphas: Vec::new(),
                tick: 0,
            }),
        })
    }

    /// Passt ein Bild mit `n` Pixeln in einen Puffer?
    pub fn fits(&self, n: usize) -> bool {
        (n as u64) * 16 <= self.max_binding
    }
}

impl State {
    /// Puffersatz für mindestens `n` Pixel (wiederverwendet oder neu).
    fn set_for(&mut self, device: &wgpu::Device, n: usize, max_binding: u64) -> usize {
        self.tick += 1;
        let tick = self.tick;
        if let Some(i) = self
            .sets
            .iter()
            .enumerate()
            .filter(|(_, s)| s.cap >= n)
            .min_by_key(|(_, s)| s.cap)
            .map(|(i, _)| i)
        {
            self.sets[i].used = tick;
            return i;
        }
        // Vorschau (≈3 MP) und Thumbnails (<1 MP) behalten je einen Satz.
        if self.sets.len() >= 2 {
            let lru = (0..self.sets.len())
                .min_by_key(|i| self.sets[*i].used)
                .unwrap();
            self.sets.remove(lru);
        }
        let cap = if n <= 1 << 20 {
            1 << 20
        } else {
            n.next_multiple_of(1 << 18)
        };
        // Aufrunden darf die Puffergrenze der Karte nicht sprengen
        let cap = if (cap as u64) * 16 > max_binding {
            n
        } else {
            cap
        };
        let v4 = (cap * 16) as u64;
        let f = (cap * 4) as u64;
        self.sets.push(Bufs {
            cap,
            src: ImgBuf {
                buf: storage(device, "quelle", v4),
                key: None,
            },
            dn: None,
            img: storage(device, "bild", v4),
            loc: storage(device, "ebene", v4),
            tmp: storage(device, "tmp", v4),
            y: storage(device, "helligkeit", f),
            packed: storage(device, "rgba8", f),
            staging: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rücklesen"),
                size: v4,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            used: tick,
        });
        self.sets.len() - 1
    }
}

/// Pixel je Hochlade-Block: 64 MB RGBA-f32.
const UPLOAD_BLOCK: usize = (64 << 20) / 16;

/// Teilt `out` (je Einheit `ko` Elemente) und `inp` (je Einheit `ki`) in
/// gleich große Stücke und rechnet sie auf eigenen Threads um.
///
/// Bewusst ohne rayon: Hochladen und Rücklesen laufen unter der
/// Zustandssperre, und parallele Arbeit mit rayon unter einer Sperre kann sich
/// verklemmen – wartet ein rayon-Thread in `par_iter`, übernimmt er womöglich
/// ein anderes Rendern (z. B. ein Thumbnail), das dieselbe Sperre will.
fn par_scoped<T: Send, U: Sync>(
    out: &mut [T],
    ko: usize,
    inp: &[U],
    ki: usize,
    f: impl Fn(&mut [T], &[U]) + Sync,
) {
    let units = (out.len() / ko).min(inp.len() / ki);
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(units.div_ceil(1 << 16))
        .max(1);
    if threads == 1 {
        f(out, inp);
        return;
    }
    let per = units.div_ceil(threads);
    std::thread::scope(|sc| {
        for (o, i) in out.chunks_mut(per * ko).zip(inp.chunks(per * ki)) {
            let f = &f;
            sc.spawn(move || f(o, i));
        }
    });
}

impl Gpu {
    /// Lädt ein Bild als RGBA-f32 hoch – in Blöcken statt über eine volle
    /// RGBA-Kopie (16 B/Pixel, bei 60 MP 960 MB nur für das Hochladen).
    /// Nach jedem Block wird abgeschickt, damit wgpu nicht alle
    /// Zwischenpuffer bis zum nächsten Rendern festhält.
    fn upload_rgba(&self, buf: &Buffer, img: &ImageF32) {
        let mut block = vec![[0f32; 4]; img.data.len().min(UPLOAD_BLOCK)];
        for (b, chunk) in img.data.chunks(UPLOAD_BLOCK).enumerate() {
            let out = &mut block[..chunk.len()];
            par_scoped(out, 1, chunk, 1, |o, i| {
                for (o, p) in o.iter_mut().zip(i) {
                    *o = [p[0], p[1], p[2], 1.0];
                }
            });
            self.queue.write_buffer(
                buf,
                (b * UPLOAD_BLOCK * 16) as u64,
                bytemuck::cast_slice(out),
            );
            self.queue.submit([]);
        }
    }
}

/// Tonwert-Parameter, wie `Tone` in pipeline.rs (normiert auf −1…1).
struct ToneVals {
    contrast: f32,
    highlights: f32,
    shadows: f32,
    whites: f32,
    blacks: f32,
    clarity: f32,
}

impl ToneVals {
    fn active(&self) -> bool {
        self.highlights != 0.0
            || self.shadows != 0.0
            || self.whites != 0.0
            || self.blacks != 0.0
            || self.clarity != 0.0
            || self.contrast != 0.0
    }
}

struct Ctx<'a> {
    ops: Vec<Op<'a>>,
    w: usize,
    h: usize,
    y: &'a Buffer,
    maps: &'a [Buffer; 3],
}

impl<'a> Ctx<'a> {
    fn run(&mut self, p: P, u: U, b: Binds<'a>, groups: [u32; 3]) {
        self.ops.push(Op::Run { p, u, b, groups });
    }

    fn base_u(&self) -> U {
        U {
            w: self.w as u32,
            h: self.h as u32,
            ..Default::default()
        }
    }

    /// Weichgezeichnete Helligkeitskarte wie `LumaMap::build` in `dst`.
    fn build_map(&mut self, dst: &'a Buffer, long_edge: usize, radius: usize) -> (u32, u32) {
        let (w, h) = (self.w, self.h);
        let scale = (long_edge as f32 / w.max(h) as f32).min(1.0);
        let mw = ((w as f32 * scale).round() as usize).max(1);
        let mh = ((h as f32 * scale).round() as usize).max(1);
        let tmp = &self.maps[2];
        let u = U {
            mw: mw as u32,
            mh: mh as u32,
            fx: w as f32 / mw as f32,
            fy: h as f32 / mh as f32,
            radius: radius as u32,
            ..self.base_u()
        };
        let y = self.y;
        self.run(
            P::MapDown,
            u,
            Binds {
                fa: Some(y),
                fo: Some(dst),
                ..Default::default()
            },
            img2d(mw, mh),
        );
        if radius > 0 {
            for _ in 0..3 {
                self.run(
                    P::BlurH,
                    u,
                    Binds {
                        fa: Some(dst),
                        fo: Some(tmp),
                        ..Default::default()
                    },
                    lines(mh),
                );
                self.run(
                    P::BlurV,
                    u,
                    Binds {
                        fa: Some(tmp),
                        fo: Some(dst),
                        ..Default::default()
                    },
                    lines(mw),
                );
            }
        }
        (mw as u32, mh as u32)
    }

    fn luma(&mut self, img: &'a Buffer) {
        let (u, y) = (self.base_u(), self.y);
        self.run(
            P::Luma,
            u,
            Binds {
                ina: Some(img),
                fo: Some(y),
                ..Default::default()
            },
            img2d(self.w, self.h),
        );
    }

    /// Wie `tone()` in pipeline.rs, in place auf `img`.
    fn tone(&mut self, img: &'a Buffer, t: &ToneVals) {
        if !t.active() {
            return;
        }
        self.luma(img);
        let mut u = self.base_u();
        let (base, detail) = (&self.maps[0], &self.maps[1]);
        let mut b = Binds {
            outa: Some(img),
            fa: Some(self.y),
            ..Default::default()
        };
        if t.highlights != 0.0 || t.shadows != 0.0 {
            let (mw, mh) = self.build_map(base, 128, 3);
            u.mw = mw;
            u.mh = mh;
            u.flags |= F_BASE;
            b.fb = Some(base);
        }
        if t.clarity != 0.0 {
            let (dw, dh) = self.build_map(detail, 640, 3);
            u.dw = dw;
            u.dh = dh;
            u.flags |= F_DETAIL;
            b.fc = Some(detail);
        }
        u.black = -t.blacks * 0.12;
        u.white = 1.0 - t.whites * 0.15;
        u.shadows = t.shadows;
        u.highlights = t.highlights;
        u.contrast = t.contrast;
        u.clarity = t.clarity;
        self.run(P::Tone, u, b, img2d(self.w, self.h));
    }

    /// Wie `dehaze()` in pipeline.rs, in place.
    fn dehaze(&mut self, img: &'a Buffer, amount: f32, haze: crate::pipeline::Haze) {
        if amount == 0.0 {
            return;
        }
        let mut u = U {
            dehaze: amount,
            haze: haze.level,
            haze_lift: haze.lift,
            ..self.base_u()
        };
        let mut b = Binds {
            outa: Some(img),
            ..Default::default()
        };
        if amount > 0.0 {
            // Dunkelkanal in den Helligkeitspuffer, daraus die Karte
            let (base_u, y) = (self.base_u(), self.y);
            self.run(
                P::Dark,
                base_u,
                Binds {
                    ina: Some(img),
                    fo: Some(y),
                    ..Default::default()
                },
                img2d(self.w, self.h),
            );
            let map = &self.maps[0];
            let (mw, mh) = self.build_map(map, 256, 2);
            u.mw = mw;
            u.mh = mh;
            b.fa = Some(self.y);
            b.fb = Some(map);
        }
        self.run(P::Dehaze, u, b, img2d(self.w, self.h));
    }

    /// Wie Color Grading in pipeline.rs (`grade_px`), in place.
    fn grade(&mut self, img: &'a Buffer, g: &crate::params::ColorGrade) {
        let Some(v) = crate::pipeline::grade_setup(g) else {
            return;
        };
        let u = U {
            grade: v.wheels,
            grade_pw: [v.pivot, v.width, 0.0, 0.0],
            ..self.base_u()
        };
        self.run(
            P::Grade,
            u,
            Binds {
                outa: Some(img),
                ..Default::default()
            },
            img2d(self.w, self.h),
        );
    }

    /// Wie `hsl()` in pipeline.rs, in place.
    fn hsl(&mut self, img: &'a Buffer, h: &crate::params::Hsl) {
        if h.is_neutral() {
            return;
        }
        let a = crate::pipeline::hsl_adjust(h);
        let mut packed = [[0f32; 4]; 6];
        for (k, vals) in [a.hue, a.sat, a.lum].iter().enumerate() {
            for (i, v) in vals.iter().enumerate() {
                packed[k * 2 + i / 4][i % 4] = *v;
            }
        }
        let u = U {
            hsl: packed,
            ..self.base_u()
        };
        self.run(
            P::Hsl,
            u,
            Binds {
                outa: Some(img),
                ..Default::default()
            },
            img2d(self.w, self.h),
        );
    }

    fn color(&mut self, img: &'a Buffer, sat: f32, vib: f32) {
        if sat == 0.0 && vib == 0.0 {
            return;
        }
        let u = U {
            sat,
            vib,
            ..self.base_u()
        };
        self.run(
            P::Color,
            u,
            Binds {
                outa: Some(img),
                ..Default::default()
            },
            img2d(self.w, self.h),
        );
    }
}

impl Gpu {
    /// Entwickelt ein Bild. `pack`: Ergebnis als 8-Bit-RGB (Vorschau,
    /// Thumbnails), sonst als Float (Export).
    pub fn render(
        &self,
        src: &Source,
        params: &DevelopParams,
        opts: &RenderOptions,
        pack: bool,
    ) -> Result<Rendered, String> {
        self.render_masked(src, params, opts, pack, &src.image, 0)
    }

    /// Wie [`Gpu::render`]; Farbmasken werden aber auf `mask_image` bestimmt.
    /// Mit Reparaturen ist `src` die vorbereitete (reparierte, entrauschte)
    /// Fassung – die CPU bestimmt Farbmasken auf dem Original, die GPU muss
    /// es ebenso tun, sonst wählt dieselbe Maske je nach Weg andere Pixel.
    /// `ident`: Kennung der Quellfassung für den Puffer-Cache (0 = keine),
    /// siehe [`ImageKey`].
    pub(crate) fn render_masked(
        &self,
        src: &Source,
        params: &DevelopParams,
        opts: &RenderOptions,
        pack: bool,
        mask_image: &ImageF32,
        ident: u64,
    ) -> Result<Rendered, String> {
        let p = params.sanitized();
        let raw = src.kind == SourceKind::Raw;
        let (w, h) = (src.image.width, src.image.height);
        let n = w * h;
        if n == 0 {
            return Err(crate::i18n::tr_s("leeres Bild", "empty image").into());
        }
        if !self.fits(n) {
            return Err(crate::tr!(
                "Bild zu groß für einen GPU-Puffer ({w}×{h}, Grenze {} MB)",
                "Image too large for a GPU buffer ({w}×{h}, limit {} MB)",
                self.max_binding >> 20
            ));
        }

        // Ausgabegröße nach Drehen und Zuschneiden
        let rot = (p.rotate % 4) as u32;
        let (ow, oh) = if rot % 2 == 1 { (h, w) } else { (w, h) };
        let (mut crop_x, mut crop_y, mut out_w, mut out_h) = match p.crop {
            Some(c) => geometry::crop_rect(ow, oh, c),
            None => (0, 0, ow, oh),
        };
        // Ausschnitt des fertigen Bildes: nur das Ausgabefenster verschieben
        if let Some(r) = opts.region.and_then(crate::pipeline::sanitize_region) {
            let (rx, ry, rw, rh) = geometry::crop_rect(out_w, out_h, r);
            crop_x += rx;
            crop_y += ry;
            out_w = rw;
            out_h = rh;
        }

        // Maskenebenen auf der CPU (zwischengespeichert)
        let mut layers = Vec::new();
        for (i, layer) in p.masks.iter().enumerate() {
            let want_overlay = opts.overlay == Some(i);
            let active = layer.enabled && !layer.adjust.is_neutral();
            if active || want_overlay {
                let alpha = masks::evaluate_cached(layer, w, h, opts.masks, Some(mask_image));
                layers.push((i, active, want_overlay, alpha));
            }
        }
        let denoised = match (&p.denoise, opts.denoised) {
            (Some(d), Some(dn)) if dn.width == w && dn.height == h => Some((d.amount / 100.0, dn)),
            _ => None,
        };

        let key = image_key(&src.image, ident);

        let oom = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let val = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut st = self.state.lock().unwrap();
        let si = st.set_for(&self.device, n, self.max_binding);

        // Quelle und entrauschte Fassung hochladen, falls nicht schon da
        if st.sets[si].src.key != Some(key) {
            self.upload_rgba(&st.sets[si].src.buf, &src.image);
            st.sets[si].src.key = Some(key);
        }
        if let Some((_, dn)) = denoised {
            let key = image_key(dn, 0);
            let set = &mut st.sets[si];
            let cap = set.cap;
            let slot = set.dn.get_or_insert_with(|| ImgBuf {
                buf: storage(&self.device, "entrauscht", (cap * 16) as u64),
                key: None,
            });
            if slot.key != Some(key) {
                self.upload_rgba(&slot.buf, dn);
                slot.key = Some(key);
            }
        }
        // Masken hochladen (je Ebene ein Puffer, nach Inhalt zwischengespeichert).
        // Reihenfolge = Nutzung: Benutzte rücken ans Ende, verdrängt wird vorn.
        for (_, _, _, a) in &layers {
            match st.alphas.iter().position(|(k, _)| Arc::ptr_eq(k, a)) {
                Some(pos) => touch(&mut st.alphas, pos),
                None => {
                    let buf = storage(&self.device, "maske", (a.len() * 4) as u64);
                    self.queue.write_buffer(&buf, 0, bytemuck::cast_slice(a));
                    st.alphas.push((a.clone(), buf));
                }
            }
        }
        // Höchstens so viele Masken behalten, wie gerade gebraucht + 8 (die
        // gerade gebrauchten liegen am Ende und bleiben)
        let keep = layers.len() + 8;
        if st.alphas.len() > keep {
            let drop_n = st.alphas.len() - keep;
            st.alphas.drain(..drop_n);
        }
        let alpha_idx: Vec<usize> = layers
            .iter()
            .map(|(_, _, _, a)| {
                st.alphas
                    .iter()
                    .position(|(k, _)| Arc::ptr_eq(k, a))
                    .expect("eben hochgeladen")
            })
            .collect();

        // Parameterpuffer: obere Schranke für die Zahl der Durchläufe
        let bound = (64 + 40 * layers.len() as u64) * self.uniform_stride;
        if st.uniform.as_ref().is_none_or(|(_, size)| *size < bound) {
            let b = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("parameter"),
                size: bound.max(256 * self.uniform_stride),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let size = b.size();
            st.uniform = Some((b, size));
        }

        let result = {
            let st = &*st;
            let set = &st.sets[si];
            let mut cx = Ctx {
                ops: Vec::new(),
                w,
                h,
                y: &set.y,
                maps: &st.maps,
            };

            // --- Szenenbezogene Stufen und Kodierung
            let wb = white_balance(p.temperature / 100.0, p.tint / 100.0);
            let mut u = U {
                wb_r: wb[0],
                wb_g: wb[1],
                wb_b: wb[2],
                gain: 2f32.powf(p.exposure),
                ..cx.base_u()
            };
            if raw {
                u.flags |= F_RAW;
            }
            let mut b = Binds {
                ina: Some(&set.src.buf),
                outa: Some(&set.img),
                ..Default::default()
            };
            if let (Some((amount, _)), Some(dn)) = (denoised, &set.dn) {
                u.flags |= F_DENOISE;
                u.denoise = amount;
                b.inb = Some(&dn.buf);
            }
            cx.run(P::Prep, u, b, img2d(w, h));

            // --- Dunst (Dunsthelligkeit wie auf der CPU aus der Quelle)
            if p.dehaze != 0.0 {
                let amount = p.dehaze / 100.0;
                let haze = crate::pipeline::haze_level(&src.image, &p, raw, amount);
                cx.dehaze(&set.img, amount, haze);
            }

            // --- Tonwerte und Farbe
            cx.tone(
                &set.img,
                &ToneVals {
                    contrast: p.contrast / 100.0 + if raw { RAW_BASE_CONTRAST } else { 0.0 },
                    highlights: p.highlights / 100.0,
                    shadows: p.shadows / 100.0,
                    whites: p.whites / 100.0,
                    blacks: p.blacks / 100.0,
                    clarity: p.clarity / 100.0,
                },
            );
            cx.color(&set.img, p.saturation / 100.0, p.vibrance / 100.0);
            cx.hsl(&set.img, &p.hsl);
            // Bei Schwarzweiß kommt das Color Grading erst nach der Umwandlung
            if !p.monochrome {
                cx.grade(&set.img, &p.color_grade);
            }

            // --- Lokale Anpassungen
            let mut overlay = None;
            for ((i, active, want_overlay, _), ai) in layers.iter().zip(&alpha_idx) {
                let alpha = &st.alphas[*ai].1;
                if *active {
                    let a = &p.masks[*i].adjust;
                    cx.ops.push(Op::Copy {
                        from: &set.img,
                        to: &set.loc,
                        size: (n * 16) as u64,
                    });
                    if a.exposure != 0.0 || a.temperature != 0.0 || a.tint != 0.0 {
                        let wb = white_balance(a.temperature / 100.0, a.tint / 100.0);
                        let u = U {
                            wb_r: wb[0],
                            wb_g: wb[1],
                            wb_b: wb[2],
                            gain: 2f32.powf(a.exposure),
                            ..cx.base_u()
                        };
                        cx.run(
                            P::LocalWb,
                            u,
                            Binds {
                                outa: Some(&set.loc),
                                ..Default::default()
                            },
                            img2d(w, h),
                        );
                    }
                    cx.tone(
                        &set.loc,
                        &ToneVals {
                            contrast: a.contrast / 100.0,
                            highlights: a.highlights / 100.0,
                            shadows: a.shadows / 100.0,
                            whites: 0.0,
                            blacks: 0.0,
                            clarity: a.clarity / 100.0,
                        },
                    );
                    cx.color(&set.loc, a.saturation / 100.0, 0.0);
                    let u = cx.base_u();
                    cx.run(
                        P::Blend,
                        u,
                        Binds {
                            outa: Some(&set.img),
                            ina: Some(&set.loc),
                            fa: Some(alpha),
                            ..Default::default()
                        },
                        img2d(w, h),
                    );
                }
                if *want_overlay {
                    overlay = Some(alpha);
                }
            }

            // --- Schwarzweiß (wie pipeline.rs: nach den lokalen Anpassungen)
            if p.monochrome {
                let u = cx.base_u();
                cx.run(
                    P::Mono,
                    u,
                    Binds {
                        outa: Some(&set.img),
                        ..Default::default()
                    },
                    img2d(w, h),
                );
                cx.grade(&set.img, &p.color_grade);
            }

            // --- Schärfen
            let mut cur = &set.img;
            if p.sharpen > 0.0 {
                cx.luma(&set.img);
                let u = U {
                    amount: p.sharpen / 100.0,
                    ..cx.base_u()
                };
                cx.run(
                    P::Sharpen,
                    u,
                    Binds {
                        ina: Some(&set.img),
                        fa: Some(&set.y),
                        outa: Some(&set.tmp),
                        ..Default::default()
                    },
                    img2d(w, h),
                );
                cur = &set.tmp;
            }
            if let Some(alpha) = overlay {
                let u = cx.base_u();
                cx.run(
                    P::Overlay,
                    u,
                    Binds {
                        outa: Some(cur),
                        fa: Some(alpha),
                        ..Default::default()
                    },
                    img2d(w, h),
                );
            }

            // --- Geometrie und Ausgabe
            let mut u = U {
                out_w: out_w as u32,
                out_h: out_h as u32,
                rot,
                flip: p.flip_horizontal as u32,
                ow: ow as u32,
                oh: oh as u32,
                crop_x: crop_x as u32,
                crop_y: crop_y as u32,
                ..cx.base_u()
            };
            if p.straighten != 0.0 {
                let (s, c) = p.straighten.to_radians().sin_cos();
                u.flags |= F_STRAIGHTEN;
                u.st_s = s;
                u.st_c = c;
                u.st_k = geometry::straighten_scale(ow as f32, oh as f32, p.straighten);
            }
            let out_px = out_w * out_h;
            let (result, out_bytes) = if pack {
                u.flags |= F_PACK;
                (&set.packed, (out_px * 4) as u64)
            } else {
                let dst = if std::ptr::eq(cur, &set.tmp) {
                    &set.loc
                } else {
                    &set.tmp
                };
                (dst, (out_px * 16) as u64)
            };
            let mut b = Binds {
                ina: Some(cur),
                ..Default::default()
            };
            if pack {
                b.packed = Some(result);
            } else {
                b.outa = Some(result);
            }
            cx.run(P::Geom, u, b, img2d(out_w, out_h));
            cx.ops.push(Op::Copy {
                from: result,
                to: &set.staging,
                size: out_bytes,
            });

            let (ubuf, usize_) = st.uniform.as_ref().expect("angelegt");
            let runs = cx
                .ops
                .iter()
                .filter(|o| matches!(o, Op::Run { .. }))
                .count() as u64;
            debug_assert!(runs * self.uniform_stride <= *usize_);
            self.submit(cx, ubuf, set, out_w, out_h, pack, oom, val)
        };

        if result.is_err() {
            // Womöglich unvollständig angelegte Puffer (z. B. Speicher voll)
            // nicht wiederverwenden – sonst schlüge jedes weitere Rendern fehl.
            st.sets.remove(si);
            st.alphas.clear();
            st.uniform = None;
        } else if st.sets[si].cap > KEEP_LARGE_MAX {
            // Sehr große Puffer (Export großer Panoramen) nicht dauerhaft belegen
            st.sets.remove(si);
        }
        // Masken in voller Auflösung nicht unbegrenzt sammeln (Export-Stapel);
        // die zuletzt benutzten (am Ende) bleiben.
        evict_oldest(&mut st.alphas, |(a, _)| a.len() * 4, 512 << 20);
        drop(st);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn submit(
        &self,
        cx: Ctx<'_>,
        ubuf: &Buffer,
        set: &Bufs,
        out_w: usize,
        out_h: usize,
        pack: bool,
        oom: wgpu::ErrorScopeGuard,
        val: wgpu::ErrorScopeGuard,
    ) -> Result<Rendered, String> {
        let stride = self.uniform_stride as usize;
        let mut ubytes = Vec::new();
        for op in &cx.ops {
            if let Op::Run { u, .. } = op {
                let start = ubytes.len();
                ubytes.extend_from_slice(bytemuck::bytes_of(u));
                ubytes.resize(start + stride, 0);
            }
        }
        self.queue.write_buffer(ubuf, 0, &ubytes);

        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let mut i = 0;
        let mut slot = 0u32;
        let mut copy_size = 0;
        while i < cx.ops.len() {
            match &cx.ops[i] {
                Op::Copy { from, to, size } => {
                    enc.copy_buffer_to_buffer(from, 0, to, 0, *size);
                    copy_size = *size;
                    i += 1;
                }
                Op::Run { .. } => {
                    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: None,
                        timestamp_writes: None,
                    });
                    while let Some(Op::Run { p, b, groups, .. }) = cx.ops.get(i) {
                        let bufs = [b.ina, b.inb, b.outa, b.fa, b.fb, b.fc, b.fo, b.packed];
                        let mut entries = vec![wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: ubuf,
                                offset: 0,
                                size: std::num::NonZeroU64::new(std::mem::size_of::<U>() as u64),
                            }),
                        }];
                        for (k, buf) in bufs.iter().enumerate() {
                            entries.push(wgpu::BindGroupEntry {
                                binding: k as u32 + 1,
                                resource: buf.unwrap_or(&self.dummies[k]).as_entire_binding(),
                            });
                        }
                        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: None,
                            layout: &self.layout,
                            entries: &entries,
                        });
                        pass.set_pipeline(&self.pipes[*p as usize]);
                        pass.set_bind_group(0, &bg, &[slot * stride as u32]);
                        pass.dispatch_workgroups(groups[0], groups[1], groups[2]);
                        slot += 1;
                        i += 1;
                    }
                }
            }
        }
        self.queue.submit([enc.finish()]);

        let fail = |what: &str, e: wgpu::Error| format!("{what}: {e}");
        if let Some(e) = pollster::block_on(val.pop()) {
            return Err(fail(crate::i18n::tr_s("Validierung", "Validation"), e));
        }
        if let Some(e) = pollster::block_on(oom.pop()) {
            return Err(fail(crate::i18n::tr_s("Grafikspeicher", "GPU memory"), e));
        }

        let slice = set.staging.slice(0..copy_size);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| crate::tr!("Warten auf die GPU: {e}", "Waiting for the GPU: {e}"))?;
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| crate::tr!("Rücklesen: {e}", "Readback: {e}"))?;
        let out = {
            let view = slice
                .get_mapped_range()
                .map_err(|e| crate::tr!("Rücklesen: {e}", "Readback: {e}"))?;
            // Umrechnen parallel (ohne rayon, siehe `par_scoped`)
            if pack {
                let px: &[u32] = bytemuck::cast_slice(&view);
                let mut rgb = vec![0u8; px.len() * 3];
                par_scoped(&mut rgb, 3, px, 1, |o, i| {
                    for (o, v) in o.as_chunks_mut::<3>().0.iter_mut().zip(i) {
                        let b = v.to_le_bytes();
                        *o = [b[0], b[1], b[2]];
                    }
                });
                Rendered::Rgb8(
                    image::RgbImage::from_raw(out_w as u32, out_h as u32, rgb)
                        .ok_or(crate::i18n::tr_s("Puffergröße", "buffer size"))?,
                )
            } else {
                let px: &[[f32; 4]] = bytemuck::cast_slice(&view);
                let mut data = vec![[0f32; 3]; px.len()];
                par_scoped(&mut data, 1, px, 1, |o, i| {
                    for (o, p) in o.iter_mut().zip(i) {
                        *o = [p[0], p[1], p[2]];
                    }
                });
                Rendered::Float(ImageF32 {
                    width: out_w,
                    height: out_h,
                    data,
                })
            }
        };
        set.staging.unmap();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gleicher Puffer (also gleiche Adresse), neuer Inhalt nur in der unteren
    /// Bildhälfte – etwa ein anderes Foto einer Serie mit gleichem Himmel.
    #[test]
    fn image_key_sees_changes_outside_top_strip() {
        let mut img = ImageF32::from_fn(2000, 1000, |x, y| [x as f32, y as f32, 0.5]);
        let a = image_key(&img, 0);
        for p in &mut img.data[600 * 2000..] {
            p[2] = 0.25;
        }
        let b = image_key(&img, 0);
        assert_eq!(a.0, b.0, "Testaufbau: gleiche Adresse");
        assert_ne!(a, b);
    }

    /// Fassungen, die sich nur in einem kleinen Flicken unterscheiden, trennt
    /// erst die Kennung des Aufrufers.
    #[test]
    fn image_key_includes_identity() {
        let img = ImageF32::from_fn(64, 64, |x, y| [x as f32, y as f32, 0.0]);
        assert_eq!(image_key(&img, 3), image_key(&img, 3));
        assert_ne!(image_key(&img, 3), image_key(&img, 4));
    }

    #[test]
    fn mask_cache_keeps_most_recent() {
        // Einträge (Name, Bytes); hinten = zuletzt benutzt
        let mut v = vec![("alt", 300), ("mittel", 300), ("neu", 300)];
        evict_oldest(&mut v, |e| e.1, 700);
        assert_eq!(v, [("mittel", 300), ("neu", 300)]);
        // Wiederbenutzt rückt ans Ende und überlebt
        let mut v = vec![("a", 300), ("b", 300), ("c", 300)];
        touch(&mut v, 0);
        evict_oldest(&mut v, |e| e.1, 700);
        assert_eq!(v, [("c", 300), ("a", 300)]);
        // Alles passt: nichts fällt weg
        let mut v = vec![("a", 1), ("b", 1)];
        evict_oldest(&mut v, |e| e.1, 2);
        assert_eq!(v.len(), 2);
    }
}
