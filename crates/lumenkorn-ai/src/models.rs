//! Woher Laufzeit und Modell kommen, ob sie vorhanden sind, und wie sie
//! heruntergeladen werden.
//!
//! Alles landet im Lumenkorn-Datenordner:
//!
//! ```text
//! <daten>/ki/runtime/libonnxruntime.so      (bzw. onnxruntime.dll / .dylib)
//! <daten>/ki/runtime-webgpu/…               (GPU über Vulkan / DirectX 12)
//! <daten>/ki/runtime-directml/…             (GPU über DirectML, nur Windows)
//! <daten>/ki/clip-vit-b32/vision.onnx
//! <daten>/ki/clip-vit-b32/text.onnx
//! <daten>/ki/clip-vit-b32/vocab.json
//! <daten>/ki/clip-vit-b32/merges.txt
//! <daten>/ki/masken/subject.onnx            (IS-Net, „Motiv auswählen“)
//! <daten>/ki/masken/sam-encoder.onnx        (Segment Anything, Objekt per Klick)
//! <daten>/ki/masken/sam-decoder.onnx
//! <daten>/ki/quellen.json                   (optional: eigene Download-Adressen)
//! ```

use crate::accel::{self, Backend};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const ORT_VERSION: &str = "1.28.0";
pub const MODEL_ID: &str = "clip-vit-b32";
const HF: &str = "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/main";
const REMBG: &str = "https://github.com/danielgatis/rembg/releases/download/v0.0.0";

/// Was heruntergeladen werden soll.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Component {
    /// CLIP: Verschlagwortung und Inhaltssuche
    Clip,
    /// Segmentierung: Motiv, Objekt per Klick, Himmel
    Masks,
    /// Rauschreduzierung (DRUNet)
    Denoise,
    /// Personenerkennung (YuNet + SFace)
    Faces,
    /// KI-Reparatur (LaMa)
    Repair,
    /// GPU-Laufzeit über WebGPU (Vulkan / DirectX 12 / Metal)
    Webgpu,
    /// GPU-Laufzeit über DirectML (Windows)
    Directml,
}

impl Component {
    fn gpu_backend(self) -> Option<Backend> {
        match self {
            Component::Webgpu => Some(Backend::Webgpu),
            Component::Directml => Some(Backend::Directml),
            _ => None,
        }
    }
}

/// Offizielle ONNX-Runtime-Pakete von PyPI. Sie enthalten neben dem
/// Python-Teil die eigenständige C-Bibliothek, die Lumenkorn lädt.
const PYPI: &str = "https://files.pythonhosted.org/packages";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuRuntimeSpec {
    pub backend: Backend,
    pub version: String,
    pub url: String,
    /// SHA-256 des Archivs – es enthält ausführbaren Code.
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    /// Dateien im Archiv (Pfadende). Die Hauptbibliothek erkennt Lumenkorn am
    /// Namen und legt sie unter dem plattformüblichen Namen ab.
    pub files: Vec<String>,
}

fn default_gpu_runtimes() -> Vec<GpuRuntimeSpec> {
    let spec =
        |backend, version: &str, path: &str, sha: &str, size, files: &[&str]| GpuRuntimeSpec {
            backend,
            version: version.into(),
            url: format!("{PYPI}/{path}"),
            sha256: Some(sha.into()),
            size: Some(size),
            files: files
                .iter()
                .map(|f| format!("onnxruntime/capi/{f}"))
                .collect(),
        };
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => vec![spec(
            Backend::Webgpu,
            "1.27.0",
            "24/11/6cd5f840be97819cbd172bacc1868726ab5fa54ceaaa79508c5a64e61233/onnxruntime_webgpu-1.27.0-cp312-cp312-manylinux_2_27_x86_64.manylinux_2_28_x86_64.whl",
            "fe5e49e00ad71bc667316d57670bfbf525438783d4a3468e0d337314ec985cbe",
            25_163_729,
            &[
                "libonnxruntime.so.1.27.0",
                "libonnxruntime_providers_shared.so",
            ],
        )],
        ("windows", "x86_64") => vec![spec(
            Backend::Webgpu,
            "1.27.0",
            "df/28/016260c51c877ba5b3eba823b43107e894659e73e0ecf250eb07e801e3c2/onnxruntime_webgpu-1.27.0-cp312-cp312-win_amd64.whl",
            "7ef99275b13e8cb9584bd0db7a6f00ebf76095601eeccf7d34749b89ee991c19",
            27_226_899,
            &[
                "onnxruntime.dll",
                "onnxruntime_providers_shared.dll",
                "dxcompiler.dll",
                "dxil.dll",
            ],
        )],
        ("macos", "aarch64") => vec![spec(
            Backend::Webgpu,
            "1.27.0",
            "b4/1f/be670672fa0dc367fd28113b5e410a6e71cb6ee66c188ed1a8eb1439c941/onnxruntime_webgpu-1.27.0-cp312-cp312-macosx_14_0_arm64.whl",
            "8e6c43281318283a31977ebe752e7b0454db9b2dc42c768ac88641952357196a",
            24_081_620,
            &["libonnxruntime.1.27.0.dylib"],
        )],
        _ => vec![],
    }
}

/// Ist das die Hauptbibliothek (und nicht ein Begleiter)?
fn is_main_library(file: &str) -> bool {
    let f = file.rsplit('/').next().unwrap_or(file);
    f == "onnxruntime.dll"
        || f.starts_with("libonnxruntime.so")
        || (f.starts_with("libonnxruntime.") && f.ends_with(".dylib"))
}

const KAIR: &str = "https://github.com/cszn/KAIR/releases/download/v1.0";
/// OpenCV Model Zoo (Git LFS – daher über media.githubusercontent.com)
const OPENCV_ZOO: &str = "https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models";

pub fn repair_dir(data_dir: &Path) -> PathBuf {
    ai_dir(data_dir).join("reparatur")
}

fn default_repair() -> Vec<FileSpec> {
    vec![FileSpec {
        name: crate::inpaint::MODEL_FILE.into(),
        url: format!("{OPENCV_ZOO}/inpainting_lama/inpainting_lama_2025jan.onnx"),
        size: Some(92_591_623),
    }]
}

pub fn faces_dir(data_dir: &Path) -> PathBuf {
    ai_dir(data_dir).join("gesichter")
}

fn default_faces() -> Vec<FileSpec> {
    vec![
        FileSpec {
            name: crate::faces::DETECTOR_FILE.into(),
            url: format!("{OPENCV_ZOO}/face_detection_yunet/face_detection_yunet_2023mar.onnx"),
            size: Some(232_589),
        },
        FileSpec {
            name: crate::faces::EMBEDDER_FILE.into(),
            url: format!("{OPENCV_ZOO}/face_recognition_sface/face_recognition_sface_2021dec.onnx"),
            size: Some(38_696_353),
        },
    ]
}
/// Fertiges Modell im Entrausch-Ordner; entsteht lokal aus den Original-Gewichten.
pub const DENOISE_MODEL: &str = "drunet.onnx";

pub fn denoise_dir(data_dir: &Path) -> PathBuf {
    ai_dir(data_dir).join("entrauschen")
}

fn default_denoise() -> Vec<FileSpec> {
    vec![FileSpec {
        name: "drunet_color.pth".into(),
        url: format!("{KAIR}/drunet_color.pth"),
        size: Some(130_579_305),
    }]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSpec {
    /// Dateiname im Modellordner
    pub name: String,
    pub url: String,
    /// Ungefähre Größe in Bytes, nur für die Anzeige
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSpec {
    pub url: String,
    /// Pfad der Bibliothek innerhalb des Archivs (.tgz oder .zip)
    pub archive_entry: String,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// `None`, wenn es für diese Plattform keinen Download gibt (z. B. Android).
    pub runtime: Option<RuntimeSpec>,
    pub model: Vec<FileSpec>,
    #[serde(default = "default_masks")]
    pub masks: Vec<FileSpec>,
    #[serde(default = "default_denoise")]
    pub denoise: Vec<FileSpec>,
    #[serde(default = "default_gpu_runtimes")]
    pub gpu_runtimes: Vec<GpuRuntimeSpec>,
    #[serde(default = "default_faces")]
    pub faces: Vec<FileSpec>,
    #[serde(default = "default_repair")]
    pub repair: Vec<FileSpec>,
}

fn default_masks() -> Vec<FileSpec> {
    let f = |name: &str, file: &str, size: u64| FileSpec {
        name: name.into(),
        url: format!("{REMBG}/{file}"),
        size: Some(size),
    };
    vec![
        f("subject.onnx", "isnet-general-use.onnx", 178_648_008),
        f("sam-encoder.onnx", "vit_b-encoder-quant.onnx", 99_827_815),
        f("sam-decoder.onnx", "vit_b-decoder-quant.onnx", 8_742_591),
    ]
}

pub fn masks_dir(data_dir: &Path) -> PathBuf {
    ai_dir(data_dir).join("masken")
}

fn component_files(m: &Manifest, c: Component) -> &[FileSpec] {
    match c {
        Component::Clip => &m.model,
        Component::Masks => &m.masks,
        Component::Denoise => &m.denoise,
        Component::Faces => &m.faces,
        Component::Repair => &m.repair,
        Component::Webgpu | Component::Directml => &[],
    }
}

fn component_dir(data_dir: &Path, c: Component) -> PathBuf {
    match c {
        Component::Clip => model_dir(data_dir),
        Component::Masks => masks_dir(data_dir),
        Component::Denoise => denoise_dir(data_dir),
        Component::Faces => faces_dir(data_dir),
        Component::Repair => repair_dir(data_dir),
        Component::Webgpu => accel::runtime_dir(&ai_dir(data_dir), Backend::Webgpu),
        Component::Directml => accel::runtime_dir(&ai_dir(data_dir), Backend::Directml),
    }
}

pub fn ai_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("ki")
}

pub fn model_dir(data_dir: &Path) -> PathBuf {
    ai_dir(data_dir).join(MODEL_ID)
}

/// Standardquellen: ONNX Runtime von GitHub, CLIP ViT-B/32 (quantisiert, ~150 MB)
/// von Hugging Face.
pub fn default_manifest() -> Manifest {
    let gh = format!("https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}");
    let v = ORT_VERSION;
    let runtime = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some((
            format!("onnxruntime-linux-x64-{v}.tgz"),
            format!("lib/libonnxruntime.so.{v}"),
            8_500_000,
        )),
        ("linux", "aarch64") => Some((
            format!("onnxruntime-linux-aarch64-{v}.tgz"),
            format!("lib/libonnxruntime.so.{v}"),
            7_500_000,
        )),
        ("windows", "x86_64") => Some((
            format!("onnxruntime-win-x64-{v}.zip"),
            "lib/onnxruntime.dll".to_string(),
            8_000_000,
        )),
        ("windows", "aarch64") => Some((
            format!("onnxruntime-win-arm64-{v}.zip"),
            "lib/onnxruntime.dll".to_string(),
            8_000_000,
        )),
        ("macos", "aarch64") => Some((
            format!("onnxruntime-osx-arm64-{v}.tgz"),
            format!("lib/libonnxruntime.{v}.dylib"),
            9_000_000,
        )),
        _ => None,
    }
    .map(|(archive, entry, size)| RuntimeSpec {
        url: format!("{gh}/{archive}"),
        archive_entry: entry,
        size: Some(size),
    });

    let f = |name: &str, path: &str, size: u64| FileSpec {
        name: name.into(),
        url: format!("{HF}/{path}"),
        size: Some(size),
    };
    Manifest {
        runtime,
        model: vec![
            f(
                "vision.onnx",
                "onnx/vision_model_quantized.onnx",
                88_000_000,
            ),
            f("text.onnx", "onnx/text_model_quantized.onnx", 64_000_000),
            f("vocab.json", "vocab.json", 860_000),
            f("merges.txt", "merges.txt", 525_000),
        ],
        masks: default_masks(),
        denoise: default_denoise(),
        gpu_runtimes: default_gpu_runtimes(),
        faces: default_faces(),
        repair: default_repair(),
    }
}

/// `quellen.json` im KI-Ordner überschreibt die Standardquellen – z. B. für einen
/// eigenen Spiegelserver oder ein anderes (kompatibles) CLIP-Modell.
pub fn load_manifest(data_dir: &Path) -> Manifest {
    let p = ai_dir(data_dir).join("quellen.json");
    match std::fs::read_to_string(&p).map(|s| serde_json::from_str::<Manifest>(&s)) {
        Ok(Ok(m)) => m,
        Ok(Err(e)) => {
            log::warn!("{} ist ungültig, nutze Standardquellen: {e}", p.display());
            default_manifest()
        }
        Err(_) => default_manifest(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub runtime_path: Option<String>,
    pub runtime_loaded: bool,
    /// Für diese Plattform gibt es einen Laufzeit-Download.
    pub runtime_downloadable: bool,
    pub model_ready: bool,
    pub missing: Vec<String>,
    /// Geschätzte Downloadgröße der fehlenden Teile in Bytes
    pub download_size: u64,
    pub model_dir: String,
    pub masks_ready: bool,
    pub masks_missing: Vec<String>,
    /// Downloadgröße für die Maskenmodelle (inkl. Laufzeit, falls sie fehlt)
    pub masks_download_size: u64,
    pub denoise_ready: bool,
    pub denoise_download_size: u64,
    /// GPU-Laufzeiten für diese Plattform, die empfohlene zuerst
    pub gpu_runtimes: Vec<GpuRuntimeStatus>,
    pub repair_ready: bool,
    pub repair_download_size: u64,
    pub faces_ready: bool,
    /// Downloadgröße für die Personenerkennung (inkl. Laufzeit, falls sie fehlt)
    pub faces_download_size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuRuntimeStatus {
    pub backend: Backend,
    pub label: String,
    pub version: String,
    pub installed: bool,
    pub download_size: u64,
}

pub fn status(data_dir: &Path) -> Status {
    let manifest = load_manifest(data_dir);
    let runtime = crate::runtime::find_library(&ai_dir(data_dir));
    let dir = model_dir(data_dir);
    let missing: Vec<&FileSpec> = manifest
        .model
        .iter()
        .filter(|f| !dir.join(&f.name).is_file())
        .collect();
    let mut size: u64 = missing.iter().filter_map(|f| f.size).sum();
    if runtime.is_none() {
        size += manifest.runtime.as_ref().and_then(|r| r.size).unwrap_or(0);
    }
    let mdir = masks_dir(data_dir);
    let mmissing: Vec<&FileSpec> = manifest
        .masks
        .iter()
        .filter(|f| !mdir.join(&f.name).is_file())
        .collect();
    let mut msize: u64 = mmissing.iter().filter_map(|f| f.size).sum();
    if runtime.is_none() {
        msize += manifest.runtime.as_ref().and_then(|r| r.size).unwrap_or(0);
    }
    let denoise_ready = denoise_dir(data_dir).join(DENOISE_MODEL).is_file();
    let mut dsize: u64 = if denoise_ready {
        0
    } else {
        manifest.denoise.iter().filter_map(|f| f.size).sum()
    };
    if runtime.is_none() {
        dsize += manifest.runtime.as_ref().and_then(|r| r.size).unwrap_or(0);
    }
    let ai = ai_dir(data_dir);
    let mut gpu_runtimes: Vec<GpuRuntimeStatus> = manifest
        .gpu_runtimes
        .iter()
        .map(|g| GpuRuntimeStatus {
            backend: g.backend,
            label: g.backend.label().into(),
            version: g.version.clone(),
            installed: accel::installed(&ai, g.backend).is_some(),
            download_size: g.size.unwrap_or(0),
        })
        .collect();
    let order = accel::gpu_backends();
    gpu_runtimes.sort_by_key(|g| order.iter().position(|b| *b == g.backend));
    let fdir = faces_dir(data_dir);
    let fmissing: Vec<&FileSpec> = manifest
        .faces
        .iter()
        .filter(|f| !fdir.join(&f.name).is_file())
        .collect();
    let mut fsize: u64 = fmissing.iter().filter_map(|f| f.size).sum();
    if runtime.is_none() {
        fsize += manifest.runtime.as_ref().and_then(|r| r.size).unwrap_or(0);
    }
    let rdir = repair_dir(data_dir);
    let rmissing: Vec<&FileSpec> = manifest
        .repair
        .iter()
        .filter(|f| !rdir.join(&f.name).is_file())
        .collect();
    let mut rsize: u64 = rmissing.iter().filter_map(|f| f.size).sum();
    if runtime.is_none() {
        rsize += manifest.runtime.as_ref().and_then(|r| r.size).unwrap_or(0);
    }
    Status {
        repair_ready: rmissing.is_empty(),
        repair_download_size: rsize,
        faces_ready: fmissing.is_empty(),
        faces_download_size: fsize,
        gpu_runtimes,
        denoise_ready,
        denoise_download_size: dsize,
        masks_ready: mmissing.is_empty(),
        masks_missing: mmissing.iter().map(|f| f.name.clone()).collect(),
        masks_download_size: msize,
        runtime_path: runtime.map(|p| p.display().to_string()),
        runtime_loaded: crate::runtime::is_loaded(),
        runtime_downloadable: manifest.runtime.is_some(),
        model_ready: missing.is_empty(),
        missing: missing.iter().map(|f| f.name.clone()).collect(),
        download_size: size,
        model_dir: dir.display().to_string(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// Was gerade geladen wird, z. B. „vision.onnx“
    pub item: String,
    pub index: usize,
    pub count: usize,
    pub done: u64,
    pub total: Option<u64>,
}

/// Lädt alles Fehlende herunter. Bereits vorhandene Dateien werden übersprungen,
/// abgebrochene Downloads hinterlassen keine halben Dateien.
pub fn download_missing(
    data_dir: &Path,
    component: Component,
    progress: &dyn Fn(Progress),
    cancel: &AtomicBool,
) -> Result<()> {
    let manifest = load_manifest(data_dir);
    let ai = ai_dir(data_dir);
    if let Some(backend) = component.gpu_backend() {
        let spec = manifest
            .gpu_runtimes
            .iter()
            .find(|g| g.backend == backend)
            .ok_or_else(|| {
                Error::Download(format!(
                    "{} gibt es für diese Plattform nicht",
                    backend.label()
                ))
            })?;
        std::fs::create_dir_all(&ai)?;
        return install_gpu_runtime(&ai, spec, progress, cancel);
    }

    let dir = component_dir(data_dir, component);
    std::fs::create_dir_all(&dir)?;

    let need_runtime = crate::runtime::find_library(&ai).is_none();
    let denoise_done = component == Component::Denoise && dir.join(DENOISE_MODEL).is_file();
    let files: Vec<&FileSpec> = component_files(&manifest, component)
        .iter()
        .filter(|_| !denoise_done)
        .filter(|f| !dir.join(&f.name).is_file())
        .collect();
    let count = files.len() + usize::from(need_runtime && manifest.runtime.is_some());
    let mut index = 0;

    if need_runtime && let Some(rt) = &manifest.runtime {
        index += 1;
        let tmp = ai.join("runtime-download.part");
        download(
            &rt.url,
            &tmp,
            &|done, total| {
                progress(Progress {
                    item: "ONNX Runtime".into(),
                    index,
                    count,
                    done,
                    total,
                })
            },
            cancel,
        )?;
        let target_dir = ai.join("runtime");
        std::fs::create_dir_all(&target_dir)?;
        let target = target_dir.join(crate::runtime::library_file_name());
        let res = extract_entry(&tmp, &rt.url, &rt.archive_entry, &target);
        let _ = std::fs::remove_file(&tmp);
        res?;
    }

    for f in files {
        index += 1;
        let target = dir.join(&f.name);
        let tmp = dir.join(format!("{}.part", f.name));
        download(
            &f.url,
            &tmp,
            &|done, total| {
                progress(Progress {
                    item: f.name.clone(),
                    index,
                    count,
                    done,
                    total: total.or(f.size),
                })
            },
            cancel,
        )?;
        std::fs::rename(&tmp, &target)?;
    }

    // DRUNet: Original-Gewichte lokal in ONNX übersetzen, dann aufräumen.
    if component == Component::Denoise && !dir.join(DENOISE_MODEL).is_file() {
        progress(Progress {
            item: "Modell wird vorbereitet".into(),
            index: count.max(1),
            count: count.max(1),
            done: 0,
            total: None,
        });
        let pth = dir.join("drunet_color.pth");
        let onnx = crate::denoise::build_drunet_onnx(&std::fs::read(&pth)?)?;
        let tmp = dir.join(format!("{DENOISE_MODEL}.part"));
        std::fs::write(&tmp, onnx)?;
        std::fs::rename(&tmp, dir.join(DENOISE_MODEL))?;
        let _ = std::fs::remove_file(pth);
    }
    Ok(())
}

fn install_gpu_runtime(
    ai: &Path,
    spec: &GpuRuntimeSpec,
    progress: &dyn Fn(Progress),
    cancel: &AtomicBool,
) -> Result<()> {
    let label = format!("{} – ONNX Runtime {}", spec.backend.label(), spec.version);
    let tmp = ai.join(format!("{}-download.part", spec.backend.dir_name()));
    download(
        &spec.url,
        &tmp,
        &|done, total| {
            progress(Progress {
                item: label.clone(),
                index: 1,
                count: 1,
                done,
                total: total.or(spec.size),
            })
        },
        cancel,
    )?;
    let res = (|| -> Result<()> {
        if let Some(sha) = &spec.sha256 {
            accel::verify_sha256(&tmp, sha)?;
        }
        // In einen Zwischenordner entpacken und erst vollständig umbenennen –
        // eine halb installierte Laufzeit würde beim Start geladen.
        let target = accel::runtime_dir(ai, spec.backend);
        let staging = target.with_extension("part");
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging)?;
        for entry in &spec.files {
            let name = if is_main_library(entry) {
                crate::runtime::library_file_name().to_string()
            } else {
                entry.rsplit('/').next().unwrap_or(entry).to_string()
            };
            extract_entry(&tmp, &spec.url, entry, &staging.join(name))?;
        }
        let _ = std::fs::remove_dir_all(&target);
        std::fs::rename(&staging, &target)?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&tmp);
    res
}

fn download(
    url: &str,
    target: &Path,
    progress: &dyn Fn(u64, Option<u64>),
    cancel: &AtomicBool,
) -> Result<()> {
    // Zertifikate des Betriebssystems verwenden – sonst scheitern Downloads
    // hinter Firmen-Proxys oder Virenscannern mit eigenem Stammzertifikat.
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_body(Some(Duration::from_secs(600)))
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .call()
        .map_err(|e| Error::Download(format!("{url}: {e}")))?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let mut reader = resp.body_mut().with_config().limit(u64::MAX).reader();
    let mut out = File::create(target)?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    let mut last = 0u64;
    progress(0, total);
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(out);
            let _ = std::fs::remove_file(target);
            return Err(Error::Cancelled);
        }
        let n = reader
            .read(&mut buf)
            .map_err(|e| Error::Download(e.to_string()))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
        if done - last >= 1 << 20 {
            progress(done, total);
            last = done;
        }
    }
    out.flush()?;
    progress(done, total);
    if let Some(t) = total
        && done != t
    {
        let _ = std::fs::remove_file(target);
        return Err(Error::Download(format!(
            "{url}: unvollständig ({done} von {t} Bytes)"
        )));
    }
    Ok(())
}

/// Holt eine einzelne Datei aus einem .tgz- oder .zip-Archiv. Der Eintrag wird
/// über das Pfadende gefunden, damit der Ordnername im Archiv egal ist.
fn extract_entry(archive: &Path, url: &str, entry: &str, target: &Path) -> Result<()> {
    let tmp = target.with_file_name(format!(
        "{}.part",
        target.file_name().unwrap_or_default().to_string_lossy()
    ));
    let found = if url.ends_with(".zip") || url.ends_with(".whl") {
        let mut zip = zip::ZipArchive::new(File::open(archive)?)
            .map_err(|e| Error::Download(e.to_string()))?;
        let name = (0..zip.len())
            .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
            .find(|n| n.ends_with(entry));
        match name {
            Some(n) => {
                let mut f = zip
                    .by_name(&n)
                    .map_err(|e| Error::Download(e.to_string()))?;
                std::io::copy(&mut f, &mut File::create(&tmp)?)?;
                true
            }
            None => false,
        }
    } else {
        let gz = flate2::read::GzDecoder::new(File::open(archive)?);
        let mut tar = tar::Archive::new(gz);
        let mut found = false;
        for e in tar.entries()? {
            let mut e = e?;
            let path = e.path()?.to_string_lossy().into_owned();
            if path.ends_with(entry) && e.header().entry_type().is_file() {
                std::io::copy(&mut e, &mut File::create(&tmp)?)?;
                found = true;
                break;
            }
        }
        found
    };
    if !found {
        return Err(Error::Download(format!("{entry} nicht im Archiv gefunden")));
    }
    std::fs::rename(tmp, target)?;
    Ok(())
}
