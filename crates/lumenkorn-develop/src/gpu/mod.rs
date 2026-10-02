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
use rayon::prelude::*;
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

/// Die GPU, falls eingeschaltet und bereit. Wartet nie auf die Erkennung.
/// Gibt die zwischengespeicherten GPU-Puffer frei (Bildpuffer, Masken).
/// Geräte und Shader bleiben – das nächste Rendern legt die Puffer neu an.
pub fn trim() {
    if let Some(Ok(g)) = GPU.get() {
        let mut st = g.state.lock().unwrap();
        st.sets.clear();
        st.alphas.clear();
    }
}

/// Nur die großen Puffer freigeben (nach einem Export) – die der Vorschau
/// bleiben, damit das Weiterbearbeiten nicht neu hochladen muss.
pub fn trim_large() {
    if let Some(Ok(g)) = GPU.get() {
        let mut st = g.state.lock().unwrap();
        st.sets.retain(|s| s.cap <= KEEP_MAX);
        st.alphas.retain(|(a, _)| a.len() <= KEEP_MAX);
    }
}

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
    _p0: f32,
    /// HSL je Farbbereich: [0..2) Farbton, [2..4) Sättigung, [4..6) Luminanz
    hsl: [[f32; 4]; 6],
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
}

const ENTRY_POINTS: [&str; 13] = [
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
/// Puffer bis zu dieser Größe (Pixel) gelten als „klein“ (Vorschau, Thumbnails).
const KEEP_MAX: usize = 12 << 20;
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
#[derive(Clone, Copy, PartialEq, Eq)]
struct ImageKey(usize, usize, usize, u64);

fn image_key(img: &ImageF32) -> ImageKey {
    let n = img.data.len();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for k in 0..64 {
        let p = img.data[(k * 7919) % n.max(1)];
        for v in p {
            h = (h ^ v.to_bits() as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    ImageKey(img.data.as_ptr() as usize, img.width, img.height, h)
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
        .map_err(|e| format!("keine Grafikkarte gefunden ({e})"))?;
        let ai = adapter.get_info();
        if ai.device_type == wgpu::DeviceType::Cpu && !allow_software {
            return Err(format!(
                "nur Software-Rendering verfügbar ({}) – die CPU-Pipeline ist schneller",
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
        .map_err(|e| format!("Grafikkarte nicht nutzbar ({e})"))?;
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
            source: wgpu::ShaderSource::Wgsl(include_str!("pipeline.wgsl").into()),
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
            return Err(format!("Shader nicht übersetzbar: {e}"));
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

fn to_vec4(img: &ImageF32) -> Vec<[f32; 4]> {
    img.data
        .par_iter()
        .map(|p| [p[0], p[1], p[2], 1.0])
        .collect()
}

/// Ohne rayon – für den Fall, dass unter der Zustandssperre umgerechnet
/// werden muss. Parallele Arbeit unter einer Sperre kann sich verklemmen:
/// Wartet ein rayon-Thread in `par_iter`, übernimmt er womöglich ein anderes
/// Rendern (z. B. ein Thumbnail), das dieselbe Sperre will.
fn to_vec4_seq(img: &ImageF32) -> Vec<[f32; 4]> {
    img.data.iter().map(|p| [p[0], p[1], p[2], 1.0]).collect()
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
        self.render_masked(src, params, opts, pack, &src.image)
    }

    /// Wie [`Gpu::render`]; Farbmasken werden aber auf `mask_image` bestimmt.
    /// Mit Reparaturen ist `src` die vorbereitete (reparierte, entrauschte)
    /// Fassung – die CPU bestimmt Farbmasken auf dem Original, die GPU muss
    /// es ebenso tun, sonst wählt dieselbe Maske je nach Weg andere Pixel.
    pub(crate) fn render_masked(
        &self,
        src: &Source,
        params: &DevelopParams,
        opts: &RenderOptions,
        pack: bool,
        mask_image: &ImageF32,
    ) -> Result<Rendered, String> {
        let p = params.sanitized();
        let raw = src.kind == SourceKind::Raw;
        let (w, h) = (src.image.width, src.image.height);
        let n = w * h;
        if n == 0 {
            return Err("leeres Bild".into());
        }
        if !self.fits(n) {
            return Err(format!(
                "Bild zu groß für einen GPU-Puffer ({w}×{h}, Grenze {} MB)",
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

        // Quelle (und entrauschte Fassung) vorab ohne Sperre umrechnen, falls
        // kein Puffer sie schon enthält – siehe `to_vec4_seq`.
        let key = image_key(&src.image);
        let dn_key = denoised.map(|(_, dn)| image_key(dn));
        let (have_src, have_dn) = {
            let st = self.state.lock().unwrap();
            let fits = |s: &&Bufs| s.cap >= n;
            (
                st.sets.iter().filter(fits).any(|s| s.src.key == Some(key)),
                dn_key.is_none_or(|k| {
                    st.sets
                        .iter()
                        .filter(fits)
                        .any(|s| s.dn.as_ref().is_some_and(|d| d.key == Some(k)))
                }),
            )
        };
        let src4 = (!have_src).then(|| to_vec4(&src.image));
        let dn4 = match (have_dn, denoised) {
            (false, Some((_, dn))) => Some(to_vec4(dn)),
            _ => None,
        };

        let oom = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let val = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut st = self.state.lock().unwrap();
        let si = st.set_for(&self.device, n, self.max_binding);

        // Quelle und entrauschte Fassung hochladen, falls nicht schon da
        if st.sets[si].src.key != Some(key) {
            let data = src4.unwrap_or_else(|| to_vec4_seq(&src.image));
            self.queue
                .write_buffer(&st.sets[si].src.buf, 0, bytemuck::cast_slice(&data));
            st.sets[si].src.key = Some(key);
        }
        if let Some((_, dn)) = denoised {
            let key = image_key(dn);
            let set = &mut st.sets[si];
            let cap = set.cap;
            let slot = set.dn.get_or_insert_with(|| ImgBuf {
                buf: storage(&self.device, "entrauscht", (cap * 16) as u64),
                key: None,
            });
            if slot.key != Some(key) {
                let data = dn4.unwrap_or_else(|| to_vec4_seq(dn));
                self.queue
                    .write_buffer(&slot.buf, 0, bytemuck::cast_slice(&data));
                slot.key = Some(key);
            }
        }
        // Masken hochladen (je Ebene ein Puffer, nach Inhalt zwischengespeichert)
        let mut alpha_idx = Vec::new();
        for (_, _, _, a) in &layers {
            let pos = match st.alphas.iter().position(|(k, _)| Arc::ptr_eq(k, a)) {
                Some(pos) => pos,
                None => {
                    let buf = storage(&self.device, "maske", (a.len() * 4) as u64);
                    self.queue.write_buffer(&buf, 0, bytemuck::cast_slice(a));
                    st.alphas.push((a.clone(), buf));
                    st.alphas.len() - 1
                }
            };
            alpha_idx.push(pos);
        }
        // Höchstens so viele Masken behalten, wie gerade gebraucht + 8
        let keep = alpha_idx.len() + 8;
        if st.alphas.len() > keep {
            let used: Vec<*const Vec<f32>> = alpha_idx
                .iter()
                .map(|&i| Arc::as_ptr(&st.alphas[i].0))
                .collect();
            let mut drop_n = st.alphas.len() - keep;
            st.alphas.retain(|(k, _)| {
                if drop_n > 0 && !used.contains(&Arc::as_ptr(k)) {
                    drop_n -= 1;
                    false
                } else {
                    true
                }
            });
            alpha_idx = layers
                .iter()
                .map(|(_, _, _, a)| {
                    st.alphas
                        .iter()
                        .position(|(k, _)| Arc::ptr_eq(k, a))
                        .unwrap()
                })
                .collect();
        }

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
        // Masken in voller Auflösung nicht unbegrenzt sammeln (Export-Stapel)
        let mut bytes = 0usize;
        st.alphas.retain(|(a, _)| {
            bytes += a.len() * 4;
            bytes <= (512 << 20)
        });
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
            return Err(fail("Validierung", e));
        }
        if let Some(e) = pollster::block_on(oom.pop()) {
            return Err(fail("Grafikspeicher", e));
        }

        let slice = set.staging.slice(0..copy_size);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| format!("Warten auf die GPU: {e}"))?;
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("Rücklesen: {e}"))?;
        let out = {
            let view = slice
                .get_mapped_range()
                .map_err(|e| format!("Rücklesen: {e}"))?;
            if pack {
                let px: &[u32] = bytemuck::cast_slice(&view);
                let mut rgb = Vec::with_capacity(px.len() * 3);
                for v in px {
                    rgb.extend_from_slice(&v.to_le_bytes()[..3]);
                }
                Rendered::Rgb8(
                    image::RgbImage::from_raw(out_w as u32, out_h as u32, rgb)
                        .ok_or("Puffergröße")?,
                )
            } else {
                let px: &[[f32; 4]] = bytemuck::cast_slice(&view);
                Rendered::Float(ImageF32 {
                    width: out_w,
                    height: out_h,
                    data: px.iter().map(|p| [p[0], p[1], p[2]]).collect(),
                })
            }
        };
        set.staging.unmap();
        Ok(out)
    }
}
