//! Hardware-Beschleunigung: Welche ONNX-Runtime-Variante geladen wird und mit
//! welchem Execution Provider die Modelle laufen.
//!
//! | Rechenwerk | Plattform        | Grafik-API         | Hersteller           |
//! |------------|------------------|--------------------|----------------------|
//! | WebGPU     | Linux, Windows   | Vulkan / DirectX 12| AMD, NVIDIA, Intel   |
//! | DirectML   | Windows          | DirectX 12         | AMD, NVIDIA, Intel   |
//! | CPU        | überall          | –                  | –                    |
//!
//! Beide GPU-Varianten brauchen nur den normalen Grafiktreiber – unter Linux
//! also Mesa (RADV) für AMD-Karten, kein ROCm. Eine GPU-Laufzeit enthält immer
//! auch den CPU-Pfad; zwischen GPU und CPU lässt sich daher ohne Neustart
//! wechseln. Nur eine andere Laufzeitbibliothek erfordert einen Neustart.

use crate::error::{Error, Result};
#[cfg(target_os = "linux")]
use lumenkorn_develop::i18n::tr_s;
use lumenkorn_develop::tr;
use ort::session::Session;
use ort::session::builder::{GraphOptimizationLevel, SessionBuilder};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Eine Laufzeitvariante.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Cpu,
    Webgpu,
    Directml,
}

impl Backend {
    pub fn is_gpu(self) -> bool {
        self != Backend::Cpu
    }

    pub fn label(self) -> &'static str {
        match self {
            Backend::Cpu => "CPU",
            Backend::Webgpu if cfg!(target_os = "windows") => "WebGPU (DirectX 12)",
            Backend::Webgpu if cfg!(target_os = "macos") => "WebGPU (Metal)",
            Backend::Webgpu => "WebGPU (Vulkan)",
            Backend::Directml => "DirectML (DirectX 12)",
        }
    }

    /// Unterordner im KI-Ordner.
    pub fn dir_name(self) -> &'static str {
        match self {
            Backend::Cpu => "runtime",
            Backend::Webgpu => "runtime-webgpu",
            Backend::Directml => "runtime-directml",
        }
    }
}

/// Gewünschte Beschleunigung (Einstellung).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preference {
    /// GPU, wenn eine GPU-Laufzeit installiert ist, sonst CPU
    #[default]
    Auto,
    Cpu,
    Webgpu,
    Directml,
}

impl Preference {
    fn backend(self) -> Option<Backend> {
        match self {
            Preference::Auto => None,
            Preference::Cpu => Some(Backend::Cpu),
            Preference::Webgpu => Some(Backend::Webgpu),
            Preference::Directml => Some(Backend::Directml),
        }
    }
}

/// GPU-Varianten, die es für diese Plattform gibt – die empfohlene zuerst.
pub fn gpu_backends() -> Vec<Backend> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        // DirectML wird nicht mehr angeboten: Die letzte DirectML-Laufzeit
        // (1.24) stürzte beim Laden der Modelle mit einer Zugriffsverletzung
        // ab (Radeon RX 7900 XTX, Windows 11). WebGPU über DirectX 12 nutzt die
        // neuere Laufzeit, die auch unter Linux getestet ist.
        ("windows", "x86_64") => vec![Backend::Webgpu],
        ("linux", "x86_64") | ("macos", "aarch64") => vec![Backend::Webgpu],
        _ => vec![],
    }
}

/// Ordner der Laufzeitvariante.
pub fn runtime_dir(ai_dir: &Path, backend: Backend) -> PathBuf {
    ai_dir.join(backend.dir_name())
}

/// Installierte Bibliothek einer Variante.
pub fn installed(ai_dir: &Path, backend: Backend) -> Option<PathBuf> {
    let p = runtime_dir(ai_dir, backend).join(crate::runtime::library_file_name());
    p.is_file().then_some(p)
}

/// Welche Bibliothek beim Start geladen werden soll.
///
/// „Automatisch“ und GPU-Wahl: GPU-Laufzeit bevorzugt (sie kann auch CPU).
/// „Nur Prozessor“: die reine CPU-Laufzeit, damit kein Grafiktreiber-Code
/// geladen wird – die GPU-Laufzeit nur, wenn keine CPU-Laufzeit da ist.
pub fn choose_library(ai_dir: &Path, pref: Preference) -> Option<(PathBuf, Backend)> {
    if let Ok(p) = std::env::var("ORT_DYLIB_PATH") {
        let p = PathBuf::from(p);
        if p.is_file() {
            // Unbekannte Bibliothek: GPU-Variante nur, wenn ausdrücklich gewählt
            // und (noch) angeboten.
            let b = pref
                .backend()
                .filter(|b| gpu_backends().contains(b))
                .unwrap_or(Backend::Cpu);
            return Some((p, b));
        }
    }
    let offered = gpu_backends();
    let mut order: Vec<Backend> = Vec::new();
    if pref == Preference::Cpu {
        order.push(Backend::Cpu);
    }
    if let Some(b) = pref.backend().filter(|b| offered.contains(b)) {
        order.push(b);
    }
    order.extend(offered);
    order.push(Backend::Cpu);
    for b in order {
        if let Some(p) = installed(ai_dir, b) {
            return Some((p, b));
        }
    }
    // Mitgeliefert neben der ausführbaren Datei (Installer)
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))?;
    let p = exe_dir.join(crate::runtime::library_file_name());
    p.is_file().then_some((p, Backend::Cpu))
}

// --- Sitzungen -------------------------------------------------------------------

struct Active {
    /// Variante der geladenen Bibliothek
    library: Backend,
    /// Ob Modelle die GPU nutzen sollen
    use_gpu: bool,
    /// Letzter Fehler beim Anlegen einer GPU-Sitzung
    gpu_error: Option<String>,
    /// Wo jedes geladene Modell rechnet: (Datei, auf der GPU)
    placements: Vec<(String, bool)>,
}

static ACTIVE: Mutex<Active> = Mutex::new(Active {
    library: Backend::Cpu,
    use_gpu: false,
    gpu_error: None,
    placements: Vec::new(),
});

/// Wird von [`crate::runtime::init`] aufgerufen.
pub(crate) fn set_library(backend: Backend) {
    ACTIVE.lock().unwrap().library = backend;
}

/// Schaltet die GPU-Nutzung für künftig geladene Modelle ein oder aus.
pub fn set_use_gpu(on: bool) {
    let mut a = ACTIVE.lock().unwrap();
    a.use_gpu = on;
    a.gpu_error = None;
    a.placements.clear();
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveInfo {
    /// Variante der geladenen Bibliothek, `None` vor dem ersten Laden
    pub library: Option<Backend>,
    /// Neue Sitzungen werden auf der GPU angelegt
    pub gpu_enabled: bool,
    pub gpu_error: Option<String>,
    /// Geladene Modelle und ob sie auf der GPU rechnen
    pub placements: Vec<Placement>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub model: String,
    pub gpu: bool,
}

pub fn active() -> ActiveInfo {
    let a = ACTIVE.lock().unwrap();
    let loaded = crate::runtime::is_loaded();
    ActiveInfo {
        library: loaded.then_some(a.library),
        gpu_enabled: loaded && a.library.is_gpu() && a.use_gpu,
        gpu_error: a.gpu_error.clone(),
        placements: a
            .placements
            .iter()
            .map(|(m, g)| Placement {
                model: m.clone(),
                gpu: *g,
            })
            .collect(),
    }
}

fn cpu_builder() -> Result<SessionBuilder> {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    Ok(Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(threads)?)
}

fn gpu_builder(backend: Backend) -> Result<SessionBuilder> {
    // Ganz oder gar nicht: Kann ein Modell nicht vollständig auf der GPU
    // laufen (z. B. int8-Operationen unter WebGPU), scheitert das Anlegen, und
    // es läuft komplett auf der CPU. Ständiges Hin- und Herkopieren zwischen
    // GPU und CPU wäre oft langsamer als die CPU allein.
    let b = cpu_builder()?.with_config_entry("session.disable_cpu_ep_fallback", "1")?;
    Ok(match backend {
        Backend::Webgpu => {
            let ep = ort::ep::WebGPU::default();
            b.with_execution_providers([ep.build().error_on_failure()])?
        }
        Backend::Directml => {
            let ep = ort::ep::DirectML::default();
            // Laut ONNX-Runtime-Doku Pflicht für DirectML
            b.with_memory_pattern(false)?
                .with_parallel_execution(false)?
                .with_execution_providers([ep.build().error_on_failure()])?
        }
        Backend::Cpu => b,
    })
}

/// Öffnet ein Modell – auf der GPU, wenn eingeschaltet, sonst (oder wenn die
/// GPU scheitert) auf der CPU. Ein GPU-Fehler wird gemerkt und angezeigt,
/// verhindert aber nie, dass die Funktion überhaupt läuft.
pub fn session(path: &Path) -> Result<Session> {
    let (backend, want_gpu) = {
        let a = ACTIVE.lock().unwrap();
        (a.library, a.use_gpu && a.library.is_gpu())
    };
    if want_gpu {
        let t = std::time::Instant::now();
        match gpu_builder(backend).and_then(|mut b| Ok(b.commit_from_file(path)?)) {
            Ok(s) => {
                log::info!(
                    "{} auf der GPU ({}) geladen in {:.2?}",
                    file_name(path),
                    backend.label(),
                    t.elapsed()
                );
                record(path, true);
                return Ok(s);
            }
            Err(e)
                if e.to_string()
                    .contains("fallback to CPU EP has been explicitly disabled") =>
            {
                log::info!(
                    "{} enthält Operationen ohne GPU-Umsetzung – läuft auf der CPU",
                    file_name(path)
                );
            }
            Err(e) => {
                log::warn!("GPU nicht nutzbar für {}: {e}", file_name(path));
                ACTIVE.lock().unwrap().gpu_error = Some(e.to_string());
            }
        }
    }
    let s = cpu_builder()?.commit_from_file(path)?;
    record(path, false);
    Ok(s)
}

fn record(path: &Path, gpu: bool) {
    let name = file_name(path);
    let mut a = ACTIVE.lock().unwrap();
    a.placements.retain(|(m, _)| *m != name);
    a.placements.push((name, gpu));
}

/// Ob ein Modell (Dateiname) zuletzt auf der GPU geladen wurde.
pub fn on_gpu(model_file: &str) -> bool {
    ACTIVE
        .lock()
        .unwrap()
        .placements
        .iter()
        .any(|(m, g)| m == model_file && *g)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// --- Erkennung der Grafikkarte ----------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuDevice {
    pub vendor: String,
    pub name: String,
    /// Grafikspeicher in Bytes, falls bekannt
    pub memory: Option<u64>,
    /// Eigenständige Grafikkarte (statt im Prozessor integriert), falls bekannt
    pub discrete: Option<bool>,
}

// Only the Linux GPU listing (sysfs) needs the PCI tables below
#[cfg(target_os = "linux")]
fn vendor_name(id: u32) -> &'static str {
    match id {
        0x1002 => "AMD",
        0x10de => "NVIDIA",
        0x8086 => "Intel",
        0x5143 => "Qualcomm",
        0x13b5 => "ARM",
        _ => tr_s("Unbekannt", "Unknown"),
    }
}

#[cfg(target_os = "linux")]
/// Bekannte Bezeichnungen häufiger Grafikchips (PCI-Geräte-ID). Unbekannte
/// Chips werden mit Hersteller und ID angezeigt.
fn device_name(vendor: u32, device: u32) -> Option<&'static str> {
    Some(match (vendor, device) {
        (0x1002, 0x744c) => "Radeon RX 7900 XTX / XT / GRE",
        (0x1002, 0x7448) => "Radeon Pro W7900",
        (0x1002, 0x747e) => "Radeon RX 7800 XT / 7700 XT",
        (0x1002, 0x7480) => "Radeon RX 7600 / 7600 XT",
        (0x1002, 0x7550) => "Radeon RX 9070 XT / 9070",
        (0x1002, 0x7590) => "Radeon RX 9060 XT",
        (0x1002, 0x73bf) => "Radeon RX 6900 XT / 6800 XT / 6800",
        (0x1002, 0x73df) => "Radeon RX 6700 XT / 6750 XT",
        (0x1002, 0x73ff) => "Radeon RX 6600 / 6650 XT",
        _ => return None,
    })
}

/// Grafikkarten laut Betriebssystem, eigenständige zuerst. Dient nur der
/// Anzeige – welche Karte tatsächlich rechnet, entscheidet der Treiber
/// (bei mehreren: die leistungsstärkere).
pub fn detect_gpus() -> Vec<GpuDevice> {
    let mut out = detect_platform();
    out.sort_by_key(|d| (d.discrete != Some(true), std::cmp::Reverse(d.memory)));
    out
}

#[cfg(target_os = "linux")]
fn detect_platform() -> Vec<GpuDevice> {
    let read_hex = |p: &Path| -> Option<u32> {
        let s = std::fs::read_to_string(p).ok()?;
        u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return out;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        // nur card0, card1 … (nicht card0-DP-1 u. Ä.)
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let dev = e.path().join("device");
        let Ok(real) = std::fs::canonicalize(&dev) else {
            continue;
        };
        if !seen.insert(real) {
            continue;
        }
        // Nur Anzeige-Controller (PCI-Klasse 0x03xxxx)
        if read_hex(&dev.join("class")).is_some_and(|c| c >> 16 != 0x03) {
            continue;
        }
        let (Some(v), Some(d)) = (read_hex(&dev.join("vendor")), read_hex(&dev.join("device")))
        else {
            continue;
        };
        let vram = std::fs::read_to_string(dev.join("mem_info_vram_total"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok());
        let product = std::fs::read_to_string(dev.join("product_name"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let name = product
            .or_else(|| device_name(v, d).map(str::to_string))
            .unwrap_or_else(|| tr!("Grafikchip {d:04x}", "Graphics chip {d:04x}"));
        // amdgpu: integrierte Chips haben wenig eigenen Speicher (meist ≤ 2 GB)
        let discrete = match v {
            0x10de => Some(true),
            0x1002 => vram.map(|m| m > 2 << 30),
            0x8086 => vram.map(|m| m > 2 << 30).or(Some(false)),
            _ => None,
        };
        out.push(GpuDevice {
            vendor: vendor_name(v).into(),
            name,
            memory: vram,
            discrete,
        });
    }
    out
}

#[cfg(not(target_os = "linux"))]
fn detect_platform() -> Vec<GpuDevice> {
    // Windows: DirectML/WebGPU wählen selbst die leistungsstärkste Karte. Eine
    // Abfrage über DXGI würde eine weitere große Abhängigkeit bedeuten – die
    // Anzeige beschränkt sich dort auf „Grafikkarte über DirectX 12“.
    Vec::new()
}

/// Prüft eine heruntergeladene Datei gegen ihre SHA-256-Prüfsumme.
pub(crate) fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    use sha2::Digest;
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if got.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(Error::Download(tr!(
            "Prüfsumme stimmt nicht ({}): erwartet {expected}, erhalten {got}",
            "Checksum mismatch ({}): expected {expected}, got {got}",
            file_name(path)
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_of_downloads() {
        // SHA-256 von „abc“ (FIPS 180-2) – Groß-/Kleinschreibung egal
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("abc");
        std::fs::write(&f, b"abc").unwrap();
        let ok = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        verify_sha256(&f, ok).unwrap();
        verify_sha256(&f, &ok.to_uppercase()).unwrap();
        assert!(verify_sha256(&f, &ok.replace('b', "c")).is_err());
    }

    #[test]
    fn preference_roundtrip() {
        for p in [
            Preference::Auto,
            Preference::Cpu,
            Preference::Webgpu,
            Preference::Directml,
        ] {
            let s = serde_json::to_string(&p).unwrap();
            assert_eq!(serde_json::from_str::<Preference>(&s).unwrap(), p);
        }
    }

    #[test]
    fn prefers_gpu_library() {
        let dir = std::env::temp_dir().join(format!("lw-accel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let lib = crate::runtime::library_file_name();
        std::fs::create_dir_all(dir.join("runtime")).unwrap();
        std::fs::write(dir.join("runtime").join(lib), b"x").unwrap();
        assert_eq!(
            choose_library(&dir, Preference::Auto).map(|x| x.1),
            Some(Backend::Cpu)
        );
        if let Some(&g) = gpu_backends().first() {
            std::fs::create_dir_all(runtime_dir(&dir, g)).unwrap();
            std::fs::write(runtime_dir(&dir, g).join(lib), b"x").unwrap();
            assert_eq!(choose_library(&dir, Preference::Auto).map(|x| x.1), Some(g));
            // „Nur Prozessor“: keine GPU-Bibliothek laden, solange die CPU-Laufzeit da ist
            assert_eq!(
                choose_library(&dir, Preference::Cpu).map(|x| x.1),
                Some(Backend::Cpu)
            );
            std::fs::remove_dir_all(dir.join("runtime")).unwrap();
            assert_eq!(choose_library(&dir, Preference::Cpu).map(|x| x.1), Some(g));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn never_loads_withdrawn_directml() {
        let dir = std::env::temp_dir().join(format!("lw-dml-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let lib = crate::runtime::library_file_name();
        std::fs::create_dir_all(runtime_dir(&dir, Backend::Directml)).unwrap();
        std::fs::write(runtime_dir(&dir, Backend::Directml).join(lib), b"x").unwrap();
        for p in [Preference::Auto, Preference::Directml, Preference::Cpu] {
            assert_ne!(
                choose_library(&dir, p).map(|x| x.1),
                Some(Backend::Directml)
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_does_not_panic() {
        let _ = detect_gpus();
    }
}
