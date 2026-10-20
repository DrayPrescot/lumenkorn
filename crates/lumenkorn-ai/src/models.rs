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
// Alle Hugging-Face- und GitHub-Quellen sind auf eine feste Revision (Commit)
// gepinnt und tragen die SHA-256-Prüfsumme der Datei: Ein späteres Überschreiben
// der Quelle ändert nicht stillschweigend, was Lumenkorn lädt.
const HF: &str = "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/d15189d7028b43f1d3e65039190477f6af591c2a";
const REMBG: &str = "https://github.com/danielgatis/rembg/releases/download/v0.0.0";
/// Himmelserkennung skyseg (U²-Net von xiongzhu666, MIT-Lizenz)
const SKYSEG: &str = "https://huggingface.co/JianyuanWang/skyseg/resolve/3ba8c6df1d9ba9ff26f637c7ba9568ac11a9aa7f/skyseg.onnx";
/// Dateiname des Himmelsmodells im Maskenordner
pub const SKY_MODEL: &str = "sky.onnx";
/// OWLv2 (Google, Apache 2.0) als ONNX von Xenova; Tokenizer wie CLIP
const OWLV2: &str = "https://huggingface.co/Xenova/owlv2-base-patch16/resolve/7463883add9ec3fbcdfc5493010f5908313d7d32";
/// MediaPipe Selfie Multiclass (Google, Apache 2.0), unverändert nach ONNX umgewandelt
const SELFIE_MULTICLASS: &str = "https://huggingface.co/senty-au/selfie_multiclass_256x256-ONNX/resolve/6db8421a7150ac20558f2c24675078eb3a1a04d0/onnx/model.onnx";
/// MediaPipe Face Mesh V2 (Google, Apache 2.0), unverändert nach ONNX umgewandelt
const FACE_MESH: &str = "https://huggingface.co/fernandotonon/QtMeshEditor-facemesh-onnx/resolve/32714088ed9830a98df6ee653ad406a3b83ef014/face_landmarks.onnx";

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
/// OpenCV Model Zoo (Git LFS – daher über media.githubusercontent.com), fester Commit
const OPENCV_ZOO: &str = "https://media.githubusercontent.com/media/opencv/opencv_zoo/47534e27c9851bb1128ccc0102f1145e27f23f98/models";

pub fn repair_dir(data_dir: &Path) -> PathBuf {
    ai_dir(data_dir).join("reparatur")
}

fn default_repair() -> Vec<FileSpec> {
    vec![FileSpec::pinned(
        crate::inpaint::MODEL_FILE,
        format!("{OPENCV_ZOO}/inpainting_lama/inpainting_lama_2025jan.onnx"),
        92_591_623,
        "7df918ac3921d3daf0aae1d219776cf0dc4e4935f035af81841b40adcf74fdf2",
    )]
}

pub fn faces_dir(data_dir: &Path) -> PathBuf {
    ai_dir(data_dir).join("gesichter")
}

fn default_faces() -> Vec<FileSpec> {
    vec![
        FileSpec::pinned(
            crate::faces::DETECTOR_FILE,
            format!("{OPENCV_ZOO}/face_detection_yunet/face_detection_yunet_2023mar.onnx"),
            232_589,
            "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
        ),
        FileSpec::pinned(
            crate::faces::EMBEDDER_FILE,
            format!("{OPENCV_ZOO}/face_recognition_sface/face_recognition_sface_2021dec.onnx"),
            38_696_353,
            "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79",
        ),
    ]
}
/// Fertiges Modell im Entrausch-Ordner; entsteht lokal aus den Original-Gewichten.
pub const DENOISE_MODEL: &str = "drunet.onnx";

pub fn denoise_dir(data_dir: &Path) -> PathBuf {
    ai_dir(data_dir).join("entrauschen")
}

fn default_denoise() -> Vec<FileSpec> {
    vec![FileSpec::pinned(
        "drunet_color.pth",
        format!("{KAIR}/drunet_color.pth"),
        130_579_305,
        "479abe3c5327dfd10ff54a80ec7d4098ca80752a5c9492cdff31cee430bec4b4",
    )]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSpec {
    /// Dateiname im Modellordner
    pub name: String,
    pub url: String,
    /// Größe in Bytes (bei den Standardquellen exakt, sonst ungefähr)
    #[serde(default)]
    pub size: Option<u64>,
    /// SHA-256 der Datei. Geprüft wird nur beim frischen Download; bereits
    /// installierte Dateien gelten unabhängig davon als vorhanden.
    #[serde(default)]
    pub sha256: Option<String>,
}

impl FileSpec {
    fn pinned(name: &str, url: String, size: u64, sha256: &str) -> Self {
        Self {
            name: name.into(),
            url,
            size: Some(size),
            sha256: Some(sha256.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSpec {
    pub url: String,
    /// Pfad der Bibliothek innerhalb des Archivs (.tgz oder .zip)
    pub archive_entry: String,
    #[serde(default)]
    pub size: Option<u64>,
    /// SHA-256 des Archivs – es enthält ausführbaren Code.
    #[serde(default)]
    pub sha256: Option<String>,
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
    let f = |name: &str, file: &str, size: u64, sha: &str| {
        FileSpec::pinned(name, format!("{REMBG}/{file}"), size, sha)
    };
    vec![
        f(
            "subject.onnx",
            "isnet-general-use.onnx",
            178_648_008,
            "60920e99c45464f2ba57bee2ad08c919a52bbf852739e96947fbb4358c0d964a",
        ),
        f(
            "sam-encoder.onnx",
            "vit_b-encoder-quant.onnx",
            99_827_815,
            "cf99a3b79eea0ac4f7c9b06cdce5b2763ff41a3f631213c748d497cba233df6a",
        ),
        f(
            "sam-decoder.onnx",
            "vit_b-decoder-quant.onnx",
            8_742_591,
            "40b02c2f694bb87b8360bb04ad8aaa84eab9673f4092b4911e0513890812b42e",
        ),
        FileSpec::pinned(
            SKY_MODEL,
            SKYSEG.into(),
            175_997_079,
            "ab9c34c64c3d821220a2886a4a06da4642ffa14d5b30e8d5339056a089aa1d39",
        ),
        // Personen und Tiere (alle Apache 2.0)
        FileSpec::pinned(
            crate::parts::OWL_FILE,
            format!("{OWLV2}/onnx/model_quantized.onnx"),
            155_312_754,
            "279a4827037e076fc62cff27c84258cab4562f14512aa2ba30fda8b156451167",
        ),
        FileSpec::pinned(
            crate::parts::OWL_VOCAB,
            format!("{HF}/vocab.json"),
            862_328,
            "5047b556ce86ccaf6aa22b3ffccfc52d391ea4accdab9c2f2407da5b742d4363",
        ),
        FileSpec::pinned(
            crate::parts::OWL_MERGES,
            format!("{HF}/merges.txt"),
            524_619,
            "9fd691f7c8039210e0fced15865466c65820d09b63988b0174bfe25de299051a",
        ),
        FileSpec::pinned(
            crate::parts::SEG_FILE,
            SELFIE_MULTICLASS.into(),
            16_454_560,
            "35ec1ecd9ee7f85073c99c00020b7f6751b69506eeacf683bc8665f6117f85b0",
        ),
        FileSpec::pinned(
            crate::parts::MESH_FILE,
            FACE_MESH.into(),
            4_920_979,
            "d16e5a55e6a480284d468ee32469692464049518c311662ab3956681de31e3e9",
        ),
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
    // Prüfsummen der GitHub-Releases (bei neuer ORT_VERSION mit erneuern)
    let runtime = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some((
            format!("onnxruntime-linux-x64-{v}.tgz"),
            format!("lib/libonnxruntime.so.{v}"),
            9_125_960,
            "a3e1b79d7bb1bf09696ce675f49e4064e6c81f6202b8225624fff0e93f8d6407",
        )),
        ("linux", "aarch64") => Some((
            format!("onnxruntime-linux-aarch64-{v}.tgz"),
            format!("lib/libonnxruntime.so.{v}"),
            8_116_278,
            "e15ff8b5d85afe6c144d97c6fd432254bf76a219daaf17658087d6ecb3e8f0bb",
        )),
        ("windows", "x86_64") => Some((
            format!("onnxruntime-win-x64-{v}.zip"),
            "lib/onnxruntime.dll".to_string(),
            78_796_801,
            "abef733dacbe2f571547a7150b479b5cb9cc0df22f96c24983a42cadb1b4f8bc",
        )),
        ("windows", "aarch64") => Some((
            format!("onnxruntime-win-arm64-{v}.zip"),
            "lib/onnxruntime.dll".to_string(),
            79_812_663,
            "cbe4547463ece092b505c3581376ed5896d22b5429f39d5e645e425ecdd369ad",
        )),
        ("macos", "aarch64") => Some((
            format!("onnxruntime-osx-arm64-{v}.tgz"),
            format!("lib/libonnxruntime.{v}.dylib"),
            32_396_562,
            "1268b359718099bde2cedb55787f182a130067bc4f31e8c88478c445b850d3d8",
        )),
        _ => None,
    }
    .map(|(archive, entry, size, sha)| RuntimeSpec {
        url: format!("{gh}/{archive}"),
        archive_entry: entry,
        size: Some(size),
        sha256: Some(sha.to_string()),
    });

    let f = |name: &str, path: &str, size: u64, sha: &str| {
        FileSpec::pinned(name, format!("{HF}/{path}"), size, sha)
    };
    Manifest {
        runtime,
        model: vec![
            f(
                "vision.onnx",
                "onnx/vision_model_quantized.onnx",
                89_117_001,
                "583fd1110a514667812fee7d684952aaf82a99b959760c8d7dca7e0ab9839299",
            ),
            f(
                "text.onnx",
                "onnx/text_model_quantized.onnx",
                64_504_507,
                "73baab855d406190da9faa498cfedf65f15cf309f4cc7385b7b032e6d08e5c3a",
            ),
            f(
                "vocab.json",
                "vocab.json",
                862_328,
                "5047b556ce86ccaf6aa22b3ffccfc52d391ea4accdab9c2f2407da5b742d4363",
            ),
            f(
                "merges.txt",
                "merges.txt",
                524_619,
                "9fd691f7c8039210e0fced15865466c65820d09b63988b0174bfe25de299051a",
            ),
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
        Ok(Ok(m)) => match validate_manifest(&m) {
            Ok(()) => m,
            Err(why) => {
                log::warn!(
                    "{} wird nicht verwendet ({why}), nutze Standardquellen",
                    p.display()
                );
                default_manifest()
            }
        },
        Ok(Err(e)) => {
            log::warn!("{} ist ungültig, nutze Standardquellen: {e}", p.display());
            default_manifest()
        }
        Err(_) => default_manifest(),
    }
}

/// Ist `name` genau eine Pfadkomponente (kein Unterordner, kein `..`, kein
/// Laufwerk oder Datenstrom)?
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':', '\0'])
        && name.trim() == name
}

/// Adresse prüfen. Ohne Prüfsumme (`needs_https`) läuft der heruntergeladene
/// Inhalt unter Umständen als Code (Laufzeit, DLLs): dort nur verschlüsselt,
/// ausgenommen der eigene Rechner (Tests, lokaler Spiegel).
fn check_url(url: &str, needs_https: bool) -> std::result::Result<(), String> {
    let lower = url.to_ascii_lowercase();
    let loopback = lower.strip_prefix("http://").is_some_and(|r| {
        ["127.0.0.1", "localhost", "[::1]"].iter().any(|h| {
            r.strip_prefix(h)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']))
        })
    });
    if lower.starts_with("https://") || (lower.starts_with("http://") && (!needs_https || loopback))
    {
        Ok(())
    } else if needs_https {
        Err(format!("{url}: ohne Prüfsumme nur https erlaubt"))
    } else {
        Err(format!("{url}: nur http(s) erlaubt"))
    }
}

/// Schutz vor einer manipulierten oder fehlerhaften `quellen.json`: Dateinamen
/// dürfen den Modellordner nicht verlassen, Adressen müssen http(s) sein.
fn validate_manifest(m: &Manifest) -> std::result::Result<(), String> {
    let lists = [&m.model, &m.masks, &m.denoise, &m.faces, &m.repair];
    for f in lists.into_iter().flatten() {
        if !is_plain_name(&f.name) {
            return Err(format!("Dateiname {:?} ungültig", f.name));
        }
        check_url(&f.url, false)?;
    }
    if let Some(r) = &m.runtime {
        check_url(&r.url, r.sha256.is_none())?;
    }
    for g in &m.gpu_runtimes {
        check_url(&g.url, g.sha256.is_none())?;
        for e in &g.files {
            if !is_plain_name(e.rsplit('/').next().unwrap_or(e)) {
                return Err(format!("Archiveintrag {e:?} ungültig"));
            }
        }
    }
    Ok(())
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
    /// Personen und Tiere (OWLv2, Selfie Multiclass, Face Mesh) – wie der
    /// Himmel mit den Masken nachgeladen
    pub parts_ready: bool,
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
    let parts_ready = crate::parts::FILES.iter().all(|f| masks.join(f).is_file());
    let masks_ready = manifest
        .masks
        .iter()
        .filter(|f| f.name != SKY_MODEL && !crate::parts::FILES.contains(&f.name.as_str()))
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
        parts_ready,
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
        let expect = rt.sha256.clone().or(rt.size.map(|n| n.to_string()));
        download_part(
            &rt.url,
            expect.as_deref(),
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
        let res = match &rt.sha256 {
            Some(sha) => accel::verify_sha256(&tmp, sha),
            None => check_length(&tmp, rt.size),
        }
        .and_then(|()| extract_entry(&tmp, &rt.url, &rt.archive_entry, &target));
        discard_part(&tmp);
        res?;
    }

    for f in files {
        index += 1;
        let target = dir.join(&f.name);
        let tmp = dir.join(format!("{}.part", f.name));
        let expect = f.sha256.clone().or(f.size.map(|n| n.to_string()));
        download_part(
            &f.url,
            expect.as_deref(),
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
        // Mit Prüfsumme gilt sie (nur für frische Downloads – schon vorhandene
        // Dateien werden oben übersprungen und nie nachträglich geprüft)
        let checked = match &f.sha256 {
            Some(sha) => accel::verify_sha256(&tmp, sha),
            None => check_length(&tmp, f.size),
        };
        if let Err(e) = checked {
            discard_part(&tmp);
            return Err(e);
        }
        std::fs::rename(&tmp, &target)?;
        let _ = std::fs::remove_file(part_marker(&tmp));
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
        // Unbrauchbare (z. B. abgeschnittene) Datei löschen – sonst gälte sie
        // als vorhanden und jeder weitere Versuch scheiterte gleich
        let onnx = match crate::denoise::build_drunet_onnx(&std::fs::read(&pth)?) {
            Ok(o) => o,
            Err(e) => {
                let _ = std::fs::remove_file(&pth);
                return Err(e);
            }
        };
        let tmp = dir.join(format!("{DENOISE_MODEL}.part"));
        std::fs::write(&tmp, onnx)?;
        std::fs::rename(&tmp, dir.join(DENOISE_MODEL))?;
        let _ = std::fs::remove_file(pth);
    }
    Ok(())
}

/// Marker neben einem Teil-Download: woher er stammt.
fn part_marker(tmp: &Path) -> PathBuf {
    let mut name = tmp.as_os_str().to_owned();
    name.push(".src");
    PathBuf::from(name)
}

/// Löscht einen Teil-Download samt Marker.
fn discard_part(tmp: &Path) {
    let _ = std::fs::remove_file(tmp);
    let _ = std::fs::remove_file(part_marker(tmp));
}

/// Wie [`download`], setzt aber nur einen Teil fort, der von derselben Quelle
/// stammt: Neben dem Teil steht Adresse plus erwartete Prüfsumme bzw. Größe.
/// Weicht sie ab (andere Version, geänderte `quellen.json`) oder fehlt der
/// Marker (Teil aus einer älteren Version), wird neu begonnen. Nach Abbruch
/// oder Verbindungsfehler bleibt der Teil liegen; Prüffehler löschen ihn.
fn download_part(
    url: &str,
    expect: Option<&str>,
    tmp: &Path,
    progress: &dyn Fn(u64, Option<u64>),
    cancel: &AtomicBool,
) -> Result<()> {
    let id = format!("{url}\n{}", expect.unwrap_or_default());
    let marker = part_marker(tmp);
    if std::fs::read_to_string(&marker).ok().as_deref() != Some(id.as_str()) {
        let _ = std::fs::remove_file(tmp);
        std::fs::write(&marker, &id)?;
    }
    let res = download(url, tmp, progress, cancel);
    if res.is_err() && !tmp.exists() {
        let _ = std::fs::remove_file(&marker);
    }
    res
}

/// Schutz vor abgeschnittenen Downloads, falls der Server keine Länge
/// mitschickt (dann prüft `fetch` nichts): Die Datei muss mindestens 90 % der
/// bekannten Größe haben. Die Angaben weichen höchstens um ~1 % ab.
fn check_length(path: &Path, expected: Option<u64>) -> Result<()> {
    let Some(want) = expected else {
        return Ok(());
    };
    let got = std::fs::metadata(path)?.len();
    if got >= want / 10 * 9 {
        return Ok(());
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    Err(Error::Download(tr!(
        "{name}: unvollständig ({got} von etwa {want} Bytes)",
        "{name}: incomplete ({got} of about {want} bytes)"
    )))
}

fn install_gpu_runtime(
    ai: &Path,
    spec: &GpuRuntimeSpec,
    progress: &dyn Fn(Progress),
    cancel: &AtomicBool,
) -> Result<()> {
    let label = format!("{} – ONNX Runtime {}", spec.backend.label(), spec.version);
    let tmp = ai.join(format!("{}-download.part", spec.backend.dir_name()));
    let expect = spec.sha256.clone().or(spec.size.map(|n| n.to_string()));
    download_part(
        &spec.url,
        expect.as_deref(),
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
        let unpacked = spec.files.iter().try_for_each(|entry| {
            let name = if is_main_library(entry) {
                crate::runtime::library_file_name().to_string()
            } else {
                entry.rsplit('/').next().unwrap_or(entry).to_string()
            };
            extract_entry(&tmp, &spec.url, entry, &staging.join(name))
        });
        if let Err(e) = unpacked {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
        // Austausch ohne Lücke: die alte Laufzeit erst zur Seite legen, die neue
        // einsetzen und die alte erst danach löschen; scheitert ein Schritt
        // (z. B. weil Windows die geladene DLL festhält), bleibt die alte
        // Laufzeit unverändert erhalten.
        let old = target.with_extension("old");
        let _ = std::fs::remove_dir_all(&old);
        let had_old = target.exists();
        if had_old && let Err(e) = std::fs::rename(&target, &old) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e.into());
        }
        if let Err(e) = std::fs::rename(&staging, &target) {
            if had_old {
                let _ = std::fs::rename(&old, &target);
            }
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e.into());
        }
        let _ = std::fs::remove_dir_all(&old);
        Ok(())
    })();
    discard_part(&tmp);
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
    let resp = req
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
    let (tx, rx) = std::sync::mpsc::sync_channel::<std::result::Result<Vec<u8>, String>>(4);
    std::thread::spawn(move || {
        let mut resp = resp;
        let mut reader = resp.body_mut().with_config().limit(u64::MAX).reader();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let msg = match reader.read(&mut buf) {
                Ok(n) => Ok(buf[..n].to_vec()),
                Err(e) => Err(e.to_string()),
            };
            let end = !matches!(&msg, Ok(c) if !c.is_empty());
            // Empfänger weg (Abbruch): Thread beenden
            if tx.send(msg).is_err() || end {
                break;
            }
        }
    });
    let mut last = done;
    progress(done, total);
    loop {
        // Bei Abbruch und bei einem Fehler bleibt der Teil liegen – der nächste
        // Versuch setzt fort (`download_part` prüft, ob er zur Quelle passt).
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        // Gelesen wird in einem eigenen Thread, damit „Abbrechen“ auch dann
        // binnen Sekunden greift, wenn gerade keine Daten kommen (die
        // Hänger-Frist beträgt 60 s). Der Thread endet mit der Verbindung.
        let chunk = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(c) => c,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Error::Download(format!("{url}: Verbindung abgebrochen")));
            }
        };
        let chunk = chunk.map_err(Error::Download)?;
        if chunk.is_empty() {
            break;
        }
        out.write_all(&chunk)?;
        done += chunk.len() as u64;
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
            .filter_map(|i| {
                let f = zip.by_index(i).ok()?;
                f.name().ok().map(|n| n.into_owned())
            })
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
