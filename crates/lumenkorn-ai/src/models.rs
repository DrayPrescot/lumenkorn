//! Woher Laufzeit und Modell kommen, ob sie vorhanden sind, und wie sie
//! heruntergeladen werden.
//!
//! Alles landet im Lumenkorn-Datenordner:
//!
//! ```text
//! <daten>/ki/runtime/libonnxruntime.so      (bzw. onnxruntime.dll / .dylib)
//! <daten>/ki/runtime-webgpu/…               (GPU über Vulkan / DirectX 12 / Metal)
//! <daten>/ki/clip-vit-b32/vision.onnx
//! <daten>/ki/clip-vit-b32/text.onnx
//! <daten>/ki/clip-vit-b32/vocab.json
//! <daten>/ki/clip-vit-b32/merges.txt
//! <daten>/ki/masken/subject.onnx            (IS-Net, „Motiv auswählen“)
//! <daten>/ki/masken/sam-encoder.onnx        (Segment Anything, Objekt per Klick)
//! <daten>/ki/masken/sam-decoder.onnx
//! <daten>/ki/masken/sky.onnx                (skyseg/U²-Net, „Himmel“)
//! <daten>/ki/entrauschen/drunet.onnx        (DRUNet, aus drunet_color.pth erzeugt)
//! <daten>/ki/gesichter/yunet.onnx           (Gesichter finden)
//! <daten>/ki/gesichter/sface.onnx           (Gesichter wiedererkennen)
//! <daten>/ki/reparatur/lama.onnx            (LaMa, KI-Reparatur)
//! <daten>/ki/quellen.json                   (optional: eigene Download-Adressen)
//! ```

use crate::accel::{self, Backend};
use crate::error::{Error, Result};
use lumenkorn_develop::{i18n::tr_s, tr};
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
/// Himmelserkennung skyseg (U²-Net von xiongzhu666, MIT-Lizenz)
const SKYSEG: &str = "https://huggingface.co/JianyuanWang/skyseg/resolve/main/skyseg.onnx";
/// Dateiname des Himmelsmodells im Maskenordner
pub const SKY_MODEL: &str = "sky.onnx";

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
}

impl Component {
    fn gpu_backend(self) -> Option<Backend> {
        match self {
            Component::Webgpu => Some(Backend::Webgpu),
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
        FileSpec {
            name: SKY_MODEL.into(),
            url: SKYSEG.into(),
            size: Some(176_000_000),
        },
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
        Component::Webgpu => &[],
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
    /// Für diese Plattform gibt es einen Laufzeit-Download.
    pub runtime_downloadable: bool,
    pub model_ready: bool,
    /// Fehlende CLIP-Dateien (Diagnose und Tests; die Oberfläche zeigt nur
    /// `model_ready`)
    pub missing: Vec<String>,
    /// Geschätzte Downloadgröße der fehlenden Teile in Bytes
    pub download_size: u64,
    pub model_dir: String,
    /// Motiv und Objekt (IS-Net, SAM) sind installiert
    pub masks_ready: bool,
    /// Das KI-Himmelsmodell ist installiert (kam später dazu und wird mit den
    /// Masken nachgeladen; ohne es bleiben Motiv und Objekt nutzbar)
    pub sky_ready: bool,
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

/// Ob eine Komponente vollständig ist und wie viel für sie noch zu laden wäre –
/// `runtime_size` ist die Größe von ONNX Runtime, falls die noch fehlt (jede
/// KI-Funktion braucht sie, daher zählt sie überall mit).
fn need(m: &Manifest, data_dir: &Path, c: Component, runtime_size: u64) -> (bool, u64) {
    let dir = component_dir(data_dir, c);
    let missing: Vec<&FileSpec> = component_files(m, c)
        .iter()
        .filter(|f| !dir.join(&f.name).is_file())
        .collect();
    let size: u64 = missing.iter().filter_map(|f| f.size).sum();
    (missing.is_empty(), size + runtime_size)
}

pub fn status(data_dir: &Path) -> Status {
    let manifest = load_manifest(data_dir);
    let runtime = crate::runtime::find_library(&ai_dir(data_dir));
    let runtime_size = match runtime {
        Some(_) => 0,
        None => manifest.runtime.as_ref().and_then(|r| r.size).unwrap_or(0),
    };
    let need = |c| need(&manifest, data_dir, c, runtime_size);
    let (model_ready, download_size) = need(Component::Clip);
    let clip_dir = model_dir(data_dir);
    let missing = manifest
        .model
        .iter()
        .filter(|f| !clip_dir.join(&f.name).is_file())
        .map(|f| f.name.clone())
        .collect();
    let (_, masks_download_size) = need(Component::Masks);
    let masks = masks_dir(data_dir);
    let sky_ready = masks.join(SKY_MODEL).is_file();
    let masks_ready = manifest
        .masks
        .iter()
        .filter(|f| f.name != SKY_MODEL)
        .all(|f| masks.join(&f.name).is_file());
    let (faces_ready, faces_download_size) = need(Component::Faces);
    let (repair_ready, repair_download_size) = need(Component::Repair);
    // Entrauschen: geladen wird die PyTorch-Datei, fertig ist es erst mit dem
    // daraus erzeugten ONNX-Modell – daher nicht über die Dateiliste.
    let denoise_ready = denoise_dir(data_dir).join(DENOISE_MODEL).is_file();
    let denoise_download_size = runtime_size
        + if denoise_ready {
            0
        } else {
            manifest.denoise.iter().filter_map(|f| f.size).sum()
        };
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
    Status {
        repair_ready,
        repair_download_size,
        faces_ready,
        faces_download_size,
        gpu_runtimes,
        denoise_ready,
        denoise_download_size,
        masks_ready,
        sky_ready,
        masks_download_size,
        runtime_path: runtime.map(|p| p.display().to_string()),
        runtime_downloadable: manifest.runtime.is_some(),
        model_ready,
        missing,
        download_size,
        model_dir: clip_dir.display().to_string(),
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
/// abgebrochene Downloads hinterlassen keine halben Dateien; nach einem
/// Verbindungsfehler setzt der nächste Versuch beim vorhandenen Teil fort.
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
                Error::Download(tr!(
                    "{} gibt es für diese Plattform nicht",
                    "{} is not available for this platform",
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
            item: tr_s("Modell wird vorbereitet", "Preparing model").into(),
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

/// Kommen so lange keine Daten, gilt die Verbindung als hängend. Bewusst kein
/// Gesamtlimit: Modelle sind bis ~180 MB groß, bei langsamen Leitungen dauert
/// das legitim deutlich länger als jede feste Frist.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

fn download(
    url: &str,
    target: &Path,
    progress: &dyn Fn(u64, Option<u64>),
    cancel: &AtomicBool,
) -> Result<()> {
    download_with(&agent(STALL_TIMEOUT), url, target, progress, cancel)
}

fn agent(stall: Duration) -> ureq::Agent {
    use ureq::unversioned::resolver::DefaultResolver;
    use ureq::unversioned::transport::{Connector, DefaultConnector};
    // Zertifikate des Betriebssystems verwenden – sonst scheitern Downloads
    // hinter Firmen-Proxys oder Virenscannern mit eigenem Stammzertifikat.
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    let config = ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_connect(Some(Duration::from_secs(20)))
        // Statuscodes selbst auswerten: 206/416 gehören zur Fortsetzung.
        .http_status_as_error(false)
        .build();
    // ureq 3 kennt nur Gesamtfristen (`timeout_recv_body` gilt für den ganzen
    // Body); die Hänger-Erkennung pro Lesevorgang steckt daher im Transport.
    let connector = DefaultConnector::new().chain(StallConnector { stall });
    ureq::Agent::with_parts(config, connector, DefaultResolver::default())
}

/// Lädt nach `target`. Liegt dort schon ein Teil (abgebrochener Versuch),
/// wird per `Range` fortgesetzt – sofern der Server das sauber bestätigt.
fn download_with(
    agent: &ureq::Agent,
    url: &str,
    target: &Path,
    progress: &dyn Fn(u64, Option<u64>),
    cancel: &AtomicBool,
) -> Result<()> {
    let existing = std::fs::metadata(target).map(|m| m.len()).unwrap_or(0);
    if fetch(agent, url, target, existing, progress, cancel)? {
        return Ok(());
    }
    // Fortsetzung abgelehnt oder unplausibel: Teil verwerfen, neu laden.
    let _ = std::fs::remove_file(target);
    if fetch(agent, url, target, 0, progress, cancel)? {
        Ok(())
    } else {
        Err(Error::Download(format!("{url}: HTTP 206")))
    }
}

/// `Ok(false)`: Fortsetzung ab `offset` nicht möglich, neu anfangen.
fn fetch(
    agent: &ureq::Agent,
    url: &str,
    target: &Path,
    offset: u64,
    progress: &dyn Fn(u64, Option<u64>),
    cancel: &AtomicBool,
) -> Result<bool> {
    // Keine Kompression: Längen und Byte-Bereiche beziehen sich sonst auf die
    // komprimierten Daten.
    let mut req = agent.get(url).header("accept-encoding", "identity");
    if offset > 0 {
        req = req.header("range", format!("bytes={offset}-"));
    }
    let mut resp = req
        .call()
        .map_err(|e| Error::Download(format!("{url}: {e}")))?;
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let length = header("content-length").and_then(|v| v.trim().parse::<u64>().ok());
    let plan = resume_plan(
        offset,
        resp.status().as_u16(),
        header("content-range").as_deref(),
        length,
    );
    let (mut out, mut done, total) = match plan {
        Resume::Fresh { total } => (File::create(target)?, 0, total),
        Resume::Append { total } => (
            std::fs::OpenOptions::new().append(true).open(target)?,
            offset,
            total,
        ),
        Resume::Restart => return Ok(false),
        Resume::Fail(status) => {
            return Err(Error::Download(format!("{url}: HTTP {status}")));
        }
    };
    let mut reader = resp.body_mut().with_config().limit(u64::MAX).reader();
    let mut buf = vec![0u8; 256 * 1024];
    let mut last = done;
    progress(done, total);
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(out);
            let _ = std::fs::remove_file(target);
            return Err(Error::Cancelled);
        }
        // Bei einem Fehler bleibt der Teil liegen – der nächste Versuch setzt fort.
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
        // Zu kurz: fortsetzbar. Zu lang: Teil ist sicher kaputt.
        if done > t {
            let _ = std::fs::remove_file(target);
        }
        return Err(Error::Download(tr!(
            "{url}: unvollständig ({done} von {t} Bytes)",
            "{url}: incomplete ({done} of {t} bytes)"
        )));
    }
    Ok(true)
}

/// Wie mit der Antwort auf eine (ggf. fortsetzende) Anfrage umzugehen ist.
#[derive(Debug, PartialEq, Eq)]
enum Resume {
    /// Vollständige Datei von vorn; `total` = erwartete Gesamtgröße
    Fresh { total: Option<u64> },
    /// Server liefert genau den Rest ab dem vorhandenen Teil
    Append { total: Option<u64> },
    /// Fortsetzung nicht möglich – Teil verwerfen und ohne `Range` neu anfragen
    Restart,
    /// HTTP-Fehler
    Fail(u16),
}

fn resume_plan(
    offset: u64,
    status: u16,
    content_range: Option<&str>,
    content_length: Option<u64>,
) -> Resume {
    match status {
        // Server ignoriert `Range` (oder es gab keins): komplette Datei.
        200 => Resume::Fresh {
            total: content_length,
        },
        206 if offset > 0 => {
            let Some((start, end, total)) = content_range.and_then(parse_content_range) else {
                return Resume::Restart;
            };
            let len = end - start + 1;
            let consistent = start == offset
                && content_length.is_none_or(|l| l == len)
                && total.is_none_or(|t| t == end + 1);
            if consistent {
                Resume::Append {
                    total: Some(end + 1),
                }
            } else {
                Resume::Restart
            }
        }
        // 416: Teil passt nicht (mehr) zur Datei, z. B. geänderte Quelle.
        416 if offset > 0 => Resume::Restart,
        _ => Resume::Fail(status),
    }
}

/// `bytes 100-199/200` bzw. `bytes 100-199/*` → (Anfang, Ende inkl., Gesamt)
fn parse_content_range(v: &str) -> Option<(u64, u64, Option<u64>)> {
    let rest = v.trim().strip_prefix("bytes")?.trim_start();
    let (range, total) = rest.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.trim().parse::<u64>().ok()?;
    let end = end.trim().parse::<u64>().ok()?;
    let total = match total.trim() {
        "*" => None,
        t => Some(t.parse::<u64>().ok()?),
    };
    (end >= start).then_some((start, end, total))
}

/// Begrenzt jede Wartezeit auf Daten auf `stall`: Liefert die Verbindung so
/// lange nichts, bricht der Download ab (statt ewig zu hängen), während ein
/// langsamer, aber stetiger Download beliebig lange dauern darf.
#[derive(Debug)]
struct StallConnector {
    stall: Duration,
}

impl ureq::unversioned::transport::Connector<Box<dyn ureq::unversioned::transport::Transport>>
    for StallConnector
{
    type Out = StallTransport;

    fn connect(
        &self,
        _: &ureq::unversioned::transport::ConnectionDetails,
        chained: Option<Box<dyn ureq::unversioned::transport::Transport>>,
    ) -> std::result::Result<Option<Self::Out>, ureq::Error> {
        Ok(chained.map(|inner| StallTransport {
            inner,
            stall: self.stall,
        }))
    }
}

#[derive(Debug)]
struct StallTransport {
    inner: Box<dyn ureq::unversioned::transport::Transport>,
    stall: Duration,
}

/// Kürzere der beiden Fristen; `true`, wenn die Hänger-Frist greift.
fn clamp_timeout(
    t: ureq::unversioned::transport::NextTimeout,
    stall: Duration,
) -> (ureq::unversioned::transport::NextTimeout, bool) {
    use ureq::unversioned::transport::{NextTimeout, time};
    let clamped = t.after.is_not_happening() || *t.after > stall;
    if clamped {
        (
            NextTimeout {
                after: time::Duration::Exact(stall),
                reason: t.reason,
            },
            true,
        )
    } else {
        (t, false)
    }
}

fn is_timeout(e: &ureq::Error) -> bool {
    match e {
        ureq::Error::Timeout(_) => true,
        ureq::Error::Io(io) => matches!(
            io.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ),
        _ => false,
    }
}

impl ureq::unversioned::transport::Transport for StallTransport {
    fn buffers(&mut self) -> &mut dyn ureq::unversioned::transport::Buffers {
        self.inner.buffers()
    }

    fn transmit_output(
        &mut self,
        amount: usize,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> std::result::Result<(), ureq::Error> {
        self.inner.transmit_output(amount, timeout)
    }

    fn await_input(
        &mut self,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> std::result::Result<bool, ureq::Error> {
        let (timeout, clamped) = clamp_timeout(timeout, self.stall);
        self.inner.await_input(timeout).map_err(|e| {
            if clamped && is_timeout(&e) {
                let secs = self.stall.as_secs();
                ureq::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    tr!(
                        "Verbindung hängt (seit {secs} s keine Daten)",
                        "connection stalled (no data for {secs} s)"
                    ),
                ))
            } else {
                e
            }
        })
    }

    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }

    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
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
        return Err(Error::Download(tr!(
            "{entry} nicht im Archiv gefunden",
            "{entry} not found in archive"
        )));
    }
    std::fs::rename(tmp, target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use ureq::unversioned::transport::{NextTimeout, time};

    #[test]
    fn content_range_parsing() {
        assert_eq!(
            parse_content_range("bytes 100-199/200"),
            Some((100, 199, Some(200)))
        );
        assert_eq!(parse_content_range("bytes 5-9/*"), Some((5, 9, None)));
        assert_eq!(parse_content_range("bytes */200"), None);
        assert_eq!(parse_content_range("bytes 9-5/20"), None);
        assert_eq!(parse_content_range("items 0-1/2"), None);
    }

    #[test]
    fn resume_decision() {
        // Ohne vorhandenen Teil: normale Antwort
        assert_eq!(
            resume_plan(0, 200, None, Some(10)),
            Resume::Fresh { total: Some(10) }
        );
        // Server ignoriert Range: von vorn
        assert_eq!(
            resume_plan(40, 200, None, Some(100)),
            Resume::Fresh { total: Some(100) }
        );
        // Sauber bestätigte Fortsetzung
        assert_eq!(
            resume_plan(40, 206, Some("bytes 40-99/100"), Some(60)),
            Resume::Append { total: Some(100) }
        );
        assert_eq!(
            resume_plan(40, 206, Some("bytes 40-99/*"), None),
            Resume::Append { total: Some(100) }
        );
        // Falscher Anfang, falsche Länge, Teilbereich, fehlender Header
        for (cr, len) in [
            (Some("bytes 0-99/100"), Some(100)),
            (Some("bytes 40-99/100"), Some(50)),
            (Some("bytes 40-69/100"), Some(30)),
            (None, Some(60)),
        ] {
            assert_eq!(resume_plan(40, 206, cr, len), Resume::Restart, "{cr:?}");
        }
        // Teil passt nicht zur Datei
        assert_eq!(resume_plan(40, 416, None, None), Resume::Restart);
        // Fehler bleiben Fehler; 206 ohne Anfrage ist unplausibel
        assert_eq!(resume_plan(0, 404, None, None), Resume::Fail(404));
        assert_eq!(resume_plan(40, 500, None, None), Resume::Fail(500));
        assert_eq!(
            resume_plan(0, 206, Some("bytes 0-9/10"), Some(10)),
            Resume::Fail(206)
        );
    }

    #[test]
    fn stall_timeout_replaces_total_budget() {
        let stall = Duration::from_secs(60);
        // Keine Frist (Body ohne Gesamtlimit) → Hänger-Frist
        let (t, c) = clamp_timeout(
            NextTimeout {
                after: time::Duration::NotHappening,
                reason: ureq::Timeout::RecvBody,
            },
            stall,
        );
        assert!(c);
        assert_eq!(*t.after, stall);
        // Längere Frist wird gekürzt, kürzere (z. B. Verbindungsaufbau) bleibt
        let (t, c) = clamp_timeout(
            NextTimeout {
                after: time::Duration::from_secs(600),
                reason: ureq::Timeout::RecvBody,
            },
            stall,
        );
        assert!(c);
        assert_eq!(*t.after, stall);
        let (t, c) = clamp_timeout(
            NextTimeout {
                after: time::Duration::from_secs(5),
                reason: ureq::Timeout::Connect,
            },
            stall,
        );
        assert!(!c);
        assert_eq!(*t.after, Duration::from_secs(5));
    }

    fn content() -> Vec<u8> {
        (0..200_000u32).map(|i| (i % 251) as u8).collect()
    }

    /// Lokaler Server. `mode`: "range" beantwortet Range mit 206, "ignore"
    /// liefert immer alles (200), "416" lehnt jede Range ab, "stall" sendet
    /// die Hälfte und schweigt dann. Liefert URL und mitgeschriebene Range-Header.
    fn serve(mode: &'static str) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let ranges = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = ranges.clone();
        std::thread::spawn(move || {
            let body = content();
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_lowercase();
                let range = req
                    .lines()
                    .find_map(|l| l.strip_prefix("range: bytes="))
                    .and_then(|r| r.trim().trim_end_matches('-').parse::<usize>().ok());
                seen.lock()
                    .unwrap()
                    .push(range.map(|r| r.to_string()).unwrap_or_default());
                match (mode, range) {
                    ("range", Some(from)) => {
                        let _ = write!(
                            s,
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                            body.len() - from,
                            from,
                            body.len() - 1,
                            body.len()
                        );
                        let _ = s.write_all(&body[from..]);
                    }
                    ("416", Some(_)) => {
                        let _ = write!(
                            s,
                            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                    }
                    ("stall", _) => {
                        let _ = write!(
                            s,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = s.write_all(&body[..body.len() / 2]);
                        let _ = s.flush();
                        std::thread::sleep(Duration::from_secs(30));
                    }
                    _ => {
                        let _ = write!(
                            s,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = s.write_all(&body);
                    }
                }
            }
        });
        (format!("http://{addr}/f"), ranges)
    }

    fn run(mode: &'static str, part: Option<&[u8]>) -> (Result<()>, Vec<u8>, Vec<String>) {
        let (url, ranges) = serve(mode);
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("f.part");
        if let Some(p) = part {
            std::fs::write(&target, p).unwrap();
        }
        let res = download_with(
            &agent(Duration::from_millis(500)),
            &url,
            &target,
            &|_, _| {},
            &AtomicBool::new(false),
        );
        let data = std::fs::read(&target).unwrap_or_default();
        let ranges = ranges.lock().unwrap().clone();
        (res, data, ranges)
    }

    #[test]
    fn resumes_from_partial_file() {
        let c = content();
        let (res, data, ranges) = run("range", Some(&c[..70_000]));
        res.unwrap();
        assert_eq!(data, c);
        assert_eq!(ranges, vec!["70000".to_string()]);
    }

    #[test]
    fn server_without_range_support_restarts() {
        let c = content();
        let (res, data, _) = run("ignore", Some(&c[..70_000]));
        res.unwrap();
        assert_eq!(data, c);
    }

    #[test]
    fn rejected_range_discards_part() {
        let c = content();
        // Vorhandener Teil passt nicht zur Datei → verwerfen, neu laden
        let (res, data, ranges) = run("416", Some(b"veraltet"));
        res.unwrap();
        assert_eq!(data, c);
        assert_eq!(ranges, vec!["8".to_string(), String::new()]);
    }

    #[test]
    fn stalled_connection_aborts_and_keeps_part() {
        let started = std::time::Instant::now();
        let (res, data, _) = run("stall", None);
        let err = res.unwrap_err().to_string();
        assert!(
            err.contains("keine Daten") || err.contains("no data"),
            "{err}"
        );
        // Abbruch nach der Hänger-Frist, nicht erst nach den 30 s des Servers
        assert!(started.elapsed() < Duration::from_secs(10));
        // Der empfangene Teil bleibt für die Fortsetzung liegen
        assert_eq!(data, content()[..data.len()]);
        assert!(!data.is_empty());
    }
}
