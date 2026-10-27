//! KI-Masken für Personen (ganze Person oder Teile wie Haare, Augen, Lippen)
//! und Tiere (Vögel, Säugetiere, Reptilien).
//!
//! Drei Modelle, alle Apache 2.0:
//!
//! - **OWLv2** (Google, `owlv2-base-patch16`, ONNX-Export von Xenova):
//!   Objekterkennung nach Textbeschreibung („a photo of a person“, „a photo
//!   of a lizard“) – findet Personen, Gesichter und Tiere als Rechtecke.
//! - **MediaPipe Selfie Multiclass** (Google, 256×256, ONNX-Umwandlung von
//!   senty-au): je Pixel Hintergrund, Haare, Körperhaut, Gesichtshaut,
//!   Kleidung, Accessoires – auf einem Ausschnitt um jede Person bzw. jedes
//!   Gesicht.
//! - **MediaPipe Face Mesh V2** (Google, 478 Punkte inkl. Iris, ONNX-Umwandlung
//!   für QtMeshEditor): Umrisse von Augen, Iris, Augenbrauen und Lippen.
//!
//! Ganze Personen und Tiere schneidet Segment Anything aus dem gefundenen
//! Rechteck aus (genauere Kanten als jedes der anderen Modelle).

use crate::accel::Model;
use crate::clip::{MEAN as CLIP_MEAN, STD as CLIP_STD};
use crate::error::{Error, Result};
use crate::segment::{Sam, SamEmbedding};
use crate::tokenizer::ClipTokenizer;
use image::imageops::FilterType;
use image::{DynamicImage, GrayImage, Luma, Rgb, RgbImage};
use lumenkorn_develop::{i18n::tr_s, tr};
use ort::value::Tensor;
use rayon::prelude::*;
use std::path::Path;

/// Dateinamen im Masken-Ordner
pub const OWL_FILE: &str = "owlv2.onnx";
pub const OWL_VOCAB: &str = "owlv2-vocab.json";
pub const OWL_MERGES: &str = "owlv2-merges.txt";
pub const SEG_FILE: &str = "selfie-multiclass.onnx";
pub const MESH_FILE: &str = "face-mesh.onnx";
/// Alle Dateien dieser Funktion (für den Download-Status)
pub const FILES: [&str; 5] = [OWL_FILE, OWL_VOCAB, OWL_MERGES, SEG_FILE, MESH_FILE];

/// Längste Kante der Teil-Masken – größer als bei Motiv und Himmel, weil
/// Augen und Lippen sonst nur wenige Pixel hätten.
pub const PARTS_EDGE: u32 = 2048;

/// Welcher Teil einer Person ausgewählt wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PersonPart {
    /// die ganze Person
    Person,
    /// Gesichtshaut (ohne Augen, Brauen, Lippen)
    Face,
    Hair,
    /// Haut außer dem Gesicht (Hals, Arme, Beine …)
    BodySkin,
    Clothes,
    /// Augen (Lidspalte: Augapfel und Iris)
    Eyes,
    Iris,
    Brows,
    Lips,
}

/// Welche Tiere ausgewählt werden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AnimalKind {
    All,
    Bird,
    Mammal,
    Reptile,
}

// --- Textabfragen ------------------------------------------------------------

const PERSON_QUERIES: [&str; 1] = ["person"];
const FACE_QUERIES: [&str; 1] = ["human face"];
const BIRDS: [&str; 10] = [
    "bird", "parrot", "owl", "eagle", "duck", "swan", "pigeon", "seagull", "penguin", "chicken",
];
const MAMMALS: [&str; 18] = [
    "dog", "cat", "horse", "cow", "sheep", "deer", "bear", "elephant", "monkey", "squirrel",
    "rabbit", "fox", "lion", "giraffe", "zebra", "pig", "goat", "seal",
];
const REPTILES: [&str; 8] = [
    "lizard",
    "snake",
    "turtle",
    "tortoise",
    "crocodile",
    "chameleon",
    "gecko",
    "iguana",
];

fn prompt(noun: &str) -> String {
    format!("a photo of a {noun}")
}

fn no(what: &str) -> Error {
    Error::ModelFormat(what.into())
}

// --- OWLv2 ---------------------------------------------------------------------

/// Gefundenes Rechteck (normiert auf das Bild: x0, y0, x1, y1) mit der
/// Wahrscheinlichkeit je Abfrage.
#[derive(Debug, Clone)]
pub struct Detection {
    pub rect: [f32; 4],
    pub scores: Vec<f32>,
}

pub struct Owl {
    session: Model,
    tokenizer: ClipTokenizer,
    ids: String,
    mask: Option<String>,
    pixels: String,
    logits: String,
    boxes: String,
    size: usize,
}

/// Länge der Textabfragen (Positionseinbettungen des OWLv2-Textmodells)
const OWL_TEXT_LEN: usize = 16;

impl Owl {
    pub fn load(dir: &Path) -> Result<Self> {
        let mut session = crate::accel::open_model(&dir.join(OWL_FILE))?;
        let read = |f: &str| {
            std::fs::read_to_string(dir.join(f)).map_err(|_| Error::ModelMissing(f.into()))
        };
        let tokenizer = ClipTokenizer::from_strings(&read(OWL_VOCAB)?, &read(OWL_MERGES)?)?;
        let s = session.inspect();
        let input = |want: &str| {
            s.inputs()
                .iter()
                .find(|i| i.name() == want)
                .map(|i| i.name().to_string())
        };
        let output = |want: &str| {
            s.outputs()
                .iter()
                .find(|o| o.name() == want)
                .map(|o| o.name().to_string())
        };
        let ids = input("input_ids").ok_or_else(|| no("OWLv2: input_ids"))?;
        let pixels = input("pixel_values").ok_or_else(|| no("OWLv2: pixel_values"))?;
        let mask = input("attention_mask");
        let logits = output("logits").ok_or_else(|| no("OWLv2: logits"))?;
        let boxes = output("pred_boxes").ok_or_else(|| no("OWLv2: pred_boxes"))?;
        // Eingabegröße laut Modell (OWLv2 base: 960), sonst 960
        let size = s
            .inputs()
            .iter()
            .find(|i| i.name() == pixels)
            .and_then(|i| i.dtype().tensor_shape().and_then(|s| s.get(2).copied()))
            .filter(|v| *v > 0)
            .map_or(960, |v| v as usize);
        Ok(Self {
            session,
            tokenizer,
            ids,
            mask,
            pixels,
            logits,
            boxes,
            size,
        })
    }

    /// Sucht die Textabfragen im Bild. Liefert je Bildbereich (Patch) mit
    /// einer Wahrscheinlichkeit über `min_score` das Rechteck und die Werte
    /// aller Abfragen.
    pub fn detect(
        &self,
        img: &RgbImage,
        queries: &[String],
        min_score: f32,
    ) -> Result<Vec<Detection>> {
        if queries.is_empty() {
            return Ok(vec![]);
        }
        let s = self.size;
        let (w, h) = (img.width() as f32, img.height() as f32);
        // Auf ein Quadrat auffüllen (rechts/unten, Grau 0,5) wie der
        // Owlv2ImageProcessor, dann auf die Eingabegröße
        let side = w.max(h);
        let scale = s as f32 / side;
        let (nw, nh) = (
            ((w * scale).round() as u32).clamp(1, s as u32),
            ((h * scale).round() as u32).clamp(1, s as u32),
        );
        let small = image::imageops::resize(img, nw, nh, FilterType::Triangle);
        let n = s * s;
        let mut data = vec![0f32; 3 * n];
        for c in 0..3 {
            let pad = (0.5 - CLIP_MEAN[c]) / CLIP_STD[c];
            data[c * n..(c + 1) * n].fill(pad);
        }
        for (x, y, p) in small.enumerate_pixels() {
            let i = y as usize * s + x as usize;
            for c in 0..3 {
                data[c * n + i] = (p[c] as f32 / 255.0 - CLIP_MEAN[c]) / CLIP_STD[c];
            }
        }
        let q = queries.len();
        let mut ids = Vec::with_capacity(q * OWL_TEXT_LEN);
        let mut mask = Vec::with_capacity(q * OWL_TEXT_LEN);
        for text in queries {
            let mut t = vec![self.tokenizer.sot];
            t.extend(
                self.tokenizer
                    .encode(text)
                    .into_iter()
                    .take(OWL_TEXT_LEN - 2),
            );
            t.push(self.tokenizer.eot);
            let used = t.len();
            // Auffüllen mit 0 („!“) wie der OWLv2-Tokenizer; das Textmodell
            // nimmt das Endzeichen (größte ID) als Satzende
            t.resize(OWL_TEXT_LEN, 0);
            ids.extend(t);
            mask.extend((0..OWL_TEXT_LEN).map(|i| (i < used) as i64));
        }
        let mut inputs = vec![
            (
                self.ids.as_str(),
                Tensor::from_array(([q, OWL_TEXT_LEN], ids))?.into_dyn(),
            ),
            (
                self.pixels.as_str(),
                Tensor::from_array(([1usize, 3, s, s], data))?.into_dyn(),
            ),
        ];
        if let Some(m) = &self.mask {
            inputs.push((
                m.as_str(),
                Tensor::from_array(([q, OWL_TEXT_LEN], mask))?.into_dyn(),
            ));
        }
        let mut session = self.session.lock();
        let out = session.run(inputs)?;
        let (lshape, logits) = out[self.logits.as_str()].try_extract_tensor::<f32>()?;
        let (_, boxes) = out[self.boxes.as_str()].try_extract_tensor::<f32>()?;
        let patches = lshape.get(1).copied().unwrap_or(0).max(0) as usize;
        if logits.len() < patches * q || boxes.len() < patches * 4 {
            return Err(Error::ModelFormat(tr!(
                "OWLv2: unerwartete Ausgabe {lshape:?}",
                "OWLv2: unexpected output {lshape:?}"
            )));
        }
        let sig = |v: f32| 1.0 / (1.0 + (-v).exp());
        // Rechtecke beziehen sich auf das aufgefüllte Quadrat
        let (fx, fy) = (side / w, side / h);
        let mut found = Vec::new();
        for p in 0..patches {
            let scores: Vec<f32> = (0..q).map(|k| sig(logits[p * q + k])).collect();
            if scores.iter().all(|v| *v < min_score) {
                continue;
            }
            let b = &boxes[p * 4..p * 4 + 4];
            let (cx, cy, bw, bh) = (b[0] * fx, b[1] * fy, b[2] * fx, b[3] * fy);
            let rect = [
                (cx - bw / 2.0).clamp(0.0, 1.0),
                (cy - bh / 2.0).clamp(0.0, 1.0),
                (cx + bw / 2.0).clamp(0.0, 1.0),
                (cy + bh / 2.0).clamp(0.0, 1.0),
            ];
            if rect[2] - rect[0] > 0.002 && rect[3] - rect[1] > 0.002 {
                found.push(Detection { rect, scores });
            }
        }
        Ok(found)
    }
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let ix = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let iy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = ix * iy;
    let area = |r: &[f32; 4]| (r[2] - r[0]) * (r[3] - r[1]);
    inter / (area(a) + area(b) - inter).max(1e-9)
}

/// Beste Rechtecke einer Gruppe von Abfragen (`group`: Indizes in
/// `scores`): Wert über `min` und mindestens `rel` des besten Werts, sich
/// überlappende Treffer zusammengefasst (NMS). Mit `others` muss die Gruppe
/// bei diesem Rechteck auch besser abschneiden als jede andere.
fn pick(
    dets: &[Detection],
    group: &[usize],
    others: &[&[usize]],
    min: f32,
    rel: f32,
) -> Vec<[f32; 4]> {
    let best_of = |d: &Detection, g: &[usize]| g.iter().map(|i| d.scores[*i]).fold(0.0, f32::max);
    let mut cand: Vec<(f32, [f32; 4])> = dets
        .iter()
        .filter(|d| {
            let me = best_of(d, group);
            others.iter().all(|o| best_of(d, o) <= me)
        })
        .map(|d| (best_of(d, group), d.rect))
        .collect();
    let top = cand.iter().map(|c| c.0).fold(0.0, f32::max);
    cand.retain(|c| c.0 >= min && c.0 >= top * rel);
    cand.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut keep: Vec<[f32; 4]> = Vec::new();
    for (_, r) in cand {
        if keep.iter().all(|k| iou(k, &r) < 0.5) {
            keep.push(r);
        }
    }
    keep
}

// --- Selfie Multiclass ----------------------------------------------------------

/// Klassen der Teil-Segmentierung
const C_HAIR: usize = 1;
const C_BODY: usize = 2;
const C_FACE: usize = 3;
const C_CLOTHES: usize = 4;
const C_OTHER: usize = 5;
const SEG: usize = 256;

pub struct PartsSeg {
    session: Model,
    input: String,
    output: String,
}

/// Wahrscheinlichkeiten je Klasse (6 × 256 × 256, zeilenweise)
struct ClassMap(Vec<f32>);

impl ClassMap {
    fn at(&self, class: usize, x: f32, y: f32) -> f32 {
        // bilinear in Pixelmitten
        let fx = (x * SEG as f32 - 0.5).clamp(0.0, (SEG - 1) as f32);
        let fy = (y * SEG as f32 - 0.5).clamp(0.0, (SEG - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(SEG - 1), (y0 + 1).min(SEG - 1));
        let (ax, ay) = (fx - x0 as f32, fy - y0 as f32);
        let p = &self.0[class * SEG * SEG..(class + 1) * SEG * SEG];
        let v = |x: usize, y: usize| p[y * SEG + x];
        (v(x0, y0) * (1.0 - ax) + v(x1, y0) * ax) * (1.0 - ay)
            + (v(x0, y1) * (1.0 - ax) + v(x1, y1) * ax) * ay
    }
}

impl PartsSeg {
    pub fn load(path: &Path) -> Result<Self> {
        let mut session = crate::accel::open_model(path)?;
        let s = session.inspect();
        let input = s
            .inputs()
            .first()
            .map(|i| i.name().to_string())
            .ok_or_else(|| no("Selfie Multiclass: kein Eingang"))?;
        let output = s
            .outputs()
            .first()
            .map(|o| o.name().to_string())
            .ok_or_else(|| no("Selfie Multiclass: kein Ausgang"))?;
        Ok(Self {
            session,
            input,
            output,
        })
    }

    /// Klassen für einen Ausschnitt (wird auf 256×256 gestreckt).
    fn predict(&self, crop: &RgbImage) -> Result<ClassMap> {
        let small = image::imageops::resize(crop, SEG as u32, SEG as u32, FilterType::Triangle);
        // NHWC, 0…1
        let data: Vec<f32> = small.as_raw().iter().map(|v| *v as f32 / 255.0).collect();
        let input = Tensor::from_array(([1usize, SEG, SEG, 3], data))?;
        let mut session = self.session.lock();
        let out = session.run(vec![(self.input.as_str(), input.into_dyn())])?;
        let (shape, logits) = out[self.output.as_str()].try_extract_tensor::<f32>()?;
        let n = SEG * SEG;
        if logits.len() != 6 * n {
            return Err(Error::ModelFormat(tr!(
                "Selfie Multiclass: unerwartete Ausgabe {shape:?}",
                "Selfie Multiclass: unexpected output {shape:?}"
            )));
        }
        // Ausgabe meist NHWC (1×256×256×6), sonst NCHW
        let nchw = shape.get(1).copied() == Some(6);
        let mut probs = vec![0f32; 6 * n];
        for i in 0..n {
            let l = |c: usize| {
                if nchw {
                    logits[c * n + i]
                } else {
                    logits[i * 6 + c]
                }
            };
            // Softmax (das Modell liefert Logits)
            let m = (0..6).map(l).fold(f32::MIN, f32::max);
            let e: [f32; 6] = std::array::from_fn(|c| (l(c) - m).exp());
            let sum: f32 = e.iter().sum();
            for c in 0..6 {
                probs[c * n + i] = e[c] / sum;
            }
        }
        Ok(ClassMap(probs))
    }
}

// --- Face Mesh ------------------------------------------------------------------

const MESH: usize = 256;
const MESH_POINTS: usize = 478;

pub struct FaceMesh {
    session: Model,
    input: String,
    landmarks: String,
    presence: Option<String>,
}

impl FaceMesh {
    pub fn load(path: &Path) -> Result<Self> {
        let mut session = crate::accel::open_model(path)?;
        let s = session.inspect();
        let input = s
            .inputs()
            .first()
            .map(|i| i.name().to_string())
            .ok_or_else(|| no("Face Mesh: kein Eingang"))?;
        // Der Ausgang mit 1434 Werten sind die Punkte, ein einzelner Wert die
        // Gesichts-Wahrscheinlichkeit
        let count = |o: &ort::value::Outlet| {
            o.dtype()
                .tensor_shape()
                .map_or(0, |s| s.iter().map(|d| (*d).max(1)).product::<i64>())
        };
        let landmarks = s
            .outputs()
            .iter()
            .find(|o| count(o) == (MESH_POINTS * 3) as i64)
            .or_else(|| s.outputs().first())
            .map(|o| o.name().to_string())
            .ok_or_else(|| no("Face Mesh: kein Ausgang"))?;
        let presence = s
            .outputs()
            .iter()
            .find(|o| o.name() != landmarks && count(o) == 1)
            .map(|o| o.name().to_string());
        Ok(Self {
            session,
            input,
            landmarks,
            presence,
        })
    }

    /// 478 Punkte im 256er-Ausschnitt, `None`, wenn das Modell dort kein
    /// Gesicht sieht.
    fn predict(&self, crop: &RgbImage) -> Result<Option<Vec<[f32; 2]>>> {
        let data: Vec<f32> = crop.as_raw().iter().map(|v| *v as f32 / 255.0).collect();
        let input = Tensor::from_array(([1usize, MESH, MESH, 3], data))?;
        let mut session = self.session.lock();
        let out = session.run(vec![(self.input.as_str(), input.into_dyn())])?;
        if let Some(p) = &self.presence {
            let (_, v) = out[p.as_str()].try_extract_tensor::<f32>()?;
            // Logit (manche Umwandlungen liefern schon 0…1)
            let score = v.first().copied().unwrap_or(1.0);
            let prob = if (0.0..=1.0).contains(&score) {
                score
            } else {
                1.0 / (1.0 + (-score).exp())
            };
            if prob < 0.5 {
                return Ok(None);
            }
        }
        let (shape, v) = out[self.landmarks.as_str()].try_extract_tensor::<f32>()?;
        if v.len() < MESH_POINTS * 3 {
            return Err(Error::ModelFormat(tr!(
                "Face Mesh: unerwartete Ausgabe {shape:?}",
                "Face Mesh: unexpected output {shape:?}"
            )));
        }
        Ok(Some(
            (0..MESH_POINTS).map(|i| [v[i * 3], v[i * 3 + 1]]).collect(),
        ))
    }
}

/// Umrisse im Face Mesh (MediaPipe-Indizes)
const EYE_R: [usize; 16] = [
    33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246,
];
const EYE_L: [usize; 16] = [
    263, 249, 390, 373, 374, 380, 381, 382, 362, 398, 384, 385, 386, 387, 388, 466,
];
const BROW_R: [usize; 10] = [46, 53, 52, 65, 55, 107, 66, 105, 63, 70];
const BROW_L: [usize; 10] = [276, 283, 282, 295, 285, 336, 296, 334, 293, 300];
const LIPS_OUT: [usize; 20] = [
    61, 146, 91, 181, 84, 17, 314, 405, 321, 375, 291, 409, 270, 269, 267, 0, 37, 39, 40, 185,
];
const LIPS_IN: [usize; 20] = [
    78, 95, 88, 178, 87, 14, 317, 402, 318, 324, 308, 415, 310, 311, 312, 13, 82, 81, 80, 191,
];
const OVAL: [usize; 36] = [
    10, 338, 297, 332, 284, 251, 389, 356, 454, 323, 361, 288, 397, 365, 379, 378, 400, 377, 152,
    148, 176, 149, 150, 136, 172, 58, 132, 93, 234, 127, 162, 21, 54, 103, 67, 109,
];
/// Iris: Mitte und vier Randpunkte je Auge
const IRIS_R: usize = 468;
const IRIS_L: usize = 473;

// --- Geometrie ------------------------------------------------------------------

/// Ausschnitt: Quadrat um `center` (Pixel) mit Seitenlänge `side`, um `angle`
/// gedreht, als `out`×`out`-Bild. Außerhalb des Fotos schwarz.
#[derive(Clone, Copy)]
struct Roi {
    center: [f32; 2],
    side: f32,
    angle: f32,
}

impl Roi {
    fn sample(&self, img: &RgbImage, out: u32) -> RgbImage {
        let (c, s) = (self.angle.cos(), self.angle.sin());
        RgbImage::from_fn(out, out, |x, y| {
            let u = ((x as f32 + 0.5) / out as f32 - 0.5) * self.side;
            let v = ((y as f32 + 0.5) / out as f32 - 0.5) * self.side;
            let px = self.center[0] + u * c - v * s;
            let py = self.center[1] + u * s + v * c;
            bilinear(img, px - 0.5, py - 0.5)
        })
    }
    /// Punkt im Ausschnitt (0…`out`) → Pixel im Foto
    fn image_point(self, p: [f32; 2], out: f32) -> [f32; 2] {
        let (c, s) = (self.angle.cos(), self.angle.sin());
        let u = (p[0] / out - 0.5) * self.side;
        let v = (p[1] / out - 0.5) * self.side;
        [
            self.center[0] + u * c - v * s,
            self.center[1] + u * s + v * c,
        ]
    }
}

fn bilinear(img: &RgbImage, x: f32, y: f32) -> Rgb<u8> {
    let (w, h) = (img.width() as i64, img.height() as i64);
    if x < -1.0 || y < -1.0 || x > w as f32 || y > h as f32 {
        return Rgb([0, 0, 0]);
    }
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (ax, ay) = (x - x0 as f32, y - y0 as f32);
    let get = |x: i64, y: i64| {
        let p = img.get_pixel(x.clamp(0, w - 1) as u32, y.clamp(0, h - 1) as u32);
        [p[0] as f32, p[1] as f32, p[2] as f32]
    };
    let (a, b, c, d) = (
        get(x0, y0),
        get(x0 + 1, y0),
        get(x0, y0 + 1),
        get(x0 + 1, y0 + 1),
    );
    Rgb([0, 1, 2].map(|i| {
        ((a[i] * (1.0 - ax) + b[i] * ax) * (1.0 - ay) + (c[i] * (1.0 - ax) + d[i] * ax) * ay)
            .round()
            .clamp(0.0, 255.0) as u8
    }))
}

/// Maske als Gleitkomma-Puffer in Maskengröße
struct Canvas {
    w: usize,
    h: usize,
    v: Vec<f32>,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Self {
        Self {
            w: w as usize,
            h: h as usize,
            v: vec![0.0; w as usize * h as usize],
        }
    }

    /// Vieleck (Pixel der Maske) mit `value` füllen – 4×4-fach abgetastet
    /// für glatte Kanten; `max` = zusammenführen, sonst abziehen.
    fn polygon(&mut self, pts: &[[f32; 2]], add: bool) {
        if pts.len() < 3 {
            return;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in pts {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        let xs = (x0.floor().max(0.0) as usize)..((x1.ceil() as usize + 1).min(self.w));
        let ys = (y0.floor().max(0.0) as usize)..((y1.ceil() as usize + 1).min(self.h));
        if xs.is_empty() || ys.is_empty() {
            return;
        }
        const N: usize = 4;
        // Zeilen parallel: Gesichtsoval bei 2048 px sind ~800×1000 Pixel mit je
        // 16 Abtastungen über alle Eckpunkte
        let w = self.w;
        self.v[ys.start * w..ys.end * w]
            .par_chunks_mut(w)
            .enumerate()
            .for_each(|(dy, row)| {
                let y = ys.start + dy;
                for x in xs.clone() {
                    let mut inside = 0;
                    for sy in 0..N {
                        for sx in 0..N {
                            let px = x as f32 + (sx as f32 + 0.5) / N as f32;
                            let py = y as f32 + (sy as f32 + 0.5) / N as f32;
                            inside += point_in(pts, px, py) as usize;
                        }
                    }
                    let a = inside as f32 / (N * N) as f32;
                    let o = &mut row[x];
                    *o = if add { o.max(a) } else { *o * (1.0 - a) };
                }
            });
    }

    fn circle(&mut self, c: [f32; 2], r: f32) {
        let n = 32;
        let pts: Vec<[f32; 2]> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                [c[0] + r * a.cos(), c[1] + r * a.sin()]
            })
            .collect();
        self.polygon(&pts, true);
    }

    fn into_gray(self) -> GrayImage {
        GrayImage::from_fn(self.w as u32, self.h as u32, |x, y| {
            Luma(
                [
                    (self.v[y as usize * self.w + x as usize].clamp(0.0, 1.0) * 255.0).round()
                        as u8,
                ],
            )
        })
    }
}

fn point_in(pts: &[[f32; 2]], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[j]);
        if (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Vieleck um seinen Schwerpunkt vergrößern (Faktor > 1) oder verkleinern.
fn grow(pts: &[[f32; 2]], f: f32) -> Vec<[f32; 2]> {
    let n = pts.len().max(1) as f32;
    let c = [
        pts.iter().map(|p| p[0]).sum::<f32>() / n,
        pts.iter().map(|p| p[1]).sum::<f32>() / n,
    ];
    pts.iter()
        .map(|p| [c[0] + (p[0] - c[0]) * f, c[1] + (p[1] - c[1]) * f])
        .collect()
}

fn mask_size(w: u32, h: u32) -> (u32, u32) {
    crate::segment::fit_size(w, h, PARTS_EDGE)
}

// --- Personen ---------------------------------------------------------------------

/// Die Modelle für Personen und Tiere. SAM (mit dem Embedding des Fotos) ist
/// optional; ohne es entstehen ganze Personen aus der Teil-Segmentierung.
pub struct PartModels<'a> {
    pub owl: &'a Owl,
    pub seg: &'a PartsSeg,
    pub mesh: &'a FaceMesh,
    pub sam: Option<(&'a Sam, &'a SamEmbedding)>,
}

/// Zwischenergebnisse für ein Bild, damit mehrere Teile (Haare, Gesicht,
/// Augen, Lippen …) nacheinander nicht jedes Mal OWLv2 und Face Mesh neu
/// rechnen. Gilt nur für dieselbe Eingabe; der Aufrufer wirft es weg, sobald
/// sich das Bild (oder die Objektivkorrektur) ändert.
#[derive(Default)]
pub struct DetectCache {
    person: Option<Vec<Detection>>,
    faces: Option<Vec<Face>>,
    animal: Option<Vec<Detection>>,
}

/// Ein gefundenes Gesicht: Punkte in Maskenpixeln
pub struct Face {
    pts: Vec<[f32; 2]>,
}

impl Face {
    fn poly(&self, idx: &[usize]) -> Vec<[f32; 2]> {
        idx.iter().map(|i| self.pts[*i]).collect()
    }
    /// Breite des Gesichts (Abstand der äußeren Augenwinkel × 2)
    fn size(&self) -> f32 {
        let (a, b) = (self.pts[33], self.pts[263]);
        2.0 * ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
    }
}

/// Maske für einen Teil aller Personen im Foto (Größe: längste Kante
/// [`PARTS_EDGE`]). Leer, wenn keine Person gefunden wurde.
pub fn person_mask(
    img: &DynamicImage,
    part: PersonPart,
    m: &PartModels,
    cache: &mut DetectCache,
) -> Result<GrayImage> {
    let rgb = img.to_rgb8();
    let (mw, mh) = mask_size(rgb.width(), rgb.height());
    let rgb = if (mw, mh) != rgb.dimensions() {
        image::imageops::resize(&rgb, mw, mh, FilterType::Triangle)
    } else {
        rgb
    };
    let queries: Vec<String> = PERSON_QUERIES
        .iter()
        .chain(FACE_QUERIES.iter())
        .map(|q| prompt(q))
        .collect();
    if cache.person.is_none() {
        cache.person = Some(m.owl.detect(&rgb, &queries, 0.05)?);
    }
    let dets = cache.person.as_deref().unwrap_or_default();
    let persons = pick(dets, &[0], &[], 0.1, 0.3);
    let face_boxes = pick(dets, &[1], &[], 0.1, 0.25);
    log::info!(
        "Personen: {} gefunden, Gesichter: {}",
        persons.len(),
        face_boxes.len()
    );
    let (fw, fh) = (mw as f32, mh as f32);
    let px = |r: &[f32; 4]| [r[0] * fw, r[1] * fh, r[2] * fw, r[3] * fh];
    let mut canvas = Canvas::new(mw, mh);

    match part {
        PersonPart::Person => {
            for r in &persons {
                let mask = match m.sam {
                    Some((sam, emb)) => Some(sam.predict_box(emb, sam_rect(r))?),
                    None => None,
                };
                match mask {
                    Some(sm) => paste_full(&mut canvas, &sm),
                    None => {
                        let roi = box_roi(&px(r), 1.06);
                        let classes = m.seg.predict(&roi_crop(&rgb, &roi))?;
                        paste_classes(
                            &mut canvas,
                            &roi,
                            &classes,
                            &[C_HAIR, C_BODY, C_FACE, C_CLOTHES, C_OTHER],
                        );
                    }
                }
            }
        }
        PersonPart::Hair | PersonPart::BodySkin | PersonPart::Clothes => {
            let class = match part {
                PersonPart::Hair => C_HAIR,
                PersonPart::BodySkin => C_BODY,
                _ => C_CLOTHES,
            };
            for r in &persons {
                let roi = box_roi(&px(r), 1.08);
                let classes = m.seg.predict(&roi_crop(&rgb, &roi))?;
                paste_classes(&mut canvas, &roi, &classes, &[class]);
            }
            // Haare zusätzlich im (höher aufgelösten) Ausschnitt ums Gesicht
            if part == PersonPart::Hair {
                for f in &face_boxes {
                    let roi = box_roi(&px(f), 2.4);
                    let classes = m.seg.predict(&roi_crop(&rgb, &roi))?;
                    paste_classes(&mut canvas, &roi, &classes, &[C_HAIR]);
                }
            }
            if persons.is_empty() && face_boxes.is_empty() {
                return Ok(canvas.into_gray());
            }
        }
        PersonPart::Face
        | PersonPart::Eyes
        | PersonPart::Iris
        | PersonPart::Brows
        | PersonPart::Lips => {
            if cache.faces.is_none() {
                cache.faces = Some(find_faces(
                    &rgb,
                    &face_boxes.iter().map(px).collect::<Vec<_>>(),
                    m.mesh,
                )?);
            }
            for f in cache.faces.as_deref().unwrap_or_default() {
                face_part(&mut canvas, &rgb, f, part, m.seg)?;
            }
        }
    }
    let gray = canvas.into_gray();
    // Haarkanten an den Farben des Fotos ausrichten (feine Strähnen)
    Ok(if part == PersonPart::Hair {
        crate::segment::refine_edge(&gray, &rgb)
    } else {
        gray
    })
}

/// Quadratischer Ausschnitt um ein Rechteck (Pixel), `f` = Rand
fn box_roi(r: &[f32; 4], f: f32) -> Roi {
    let (w, h) = (r[2] - r[0], r[3] - r[1]);
    Roi {
        center: [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0],
        side: w.max(h) * f,
        angle: 0.0,
    }
}

/// Ausschnitt mit eigener Größe (Selfie Multiclass streckt ohnehin auf 256)
fn roi_crop(img: &RgbImage, roi: &Roi) -> RgbImage {
    roi.sample(img, SEG as u32)
}

/// Klassenwahrscheinlichkeiten eines (ungedrehten) Ausschnitts in die Maske
/// übernehmen (Maximum mit dem Vorhandenen).
fn paste_classes(c: &mut Canvas, roi: &Roi, map: &ClassMap, classes: &[usize]) {
    let half = roi.side / 2.0;
    let x0 = (roi.center[0] - half).floor().max(0.0) as usize;
    let y0 = (roi.center[1] - half).floor().max(0.0) as usize;
    let x1 = ((roi.center[0] + half).ceil() as usize).min(c.w);
    let y1 = ((roi.center[1] + half).ceil() as usize).min(c.h);
    for y in y0..y1 {
        for x in x0..x1 {
            let u = (x as f32 + 0.5 - (roi.center[0] - half)) / roi.side;
            let v = (y as f32 + 0.5 - (roi.center[1] - half)) / roi.side;
            if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                continue;
            }
            let p: f32 = classes.iter().map(|k| map.at(*k, u, v)).sum();
            // Wahrscheinlichkeit → weiche, aber klare Kante
            let a = smooth((p - 0.35) / 0.3);
            let o = &mut c.v[y * c.w + x];
            *o = o.max(a);
        }
    }
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// SAM-Maske (beliebige Größe) in die Maske übernehmen
fn paste_full(c: &mut Canvas, m: &GrayImage) {
    let m = image::imageops::resize(m, c.w as u32, c.h as u32, FilterType::Triangle);
    for (o, p) in c.v.iter_mut().zip(m.as_raw()) {
        *o = o.max(*p as f32 / 255.0);
    }
}

/// Face Mesh für jedes gefundene Gesicht: erst grob um das Rechteck, dann
/// ein zweites Mal gerade ausgerichtet um die gefundenen Punkte.
fn find_faces(img: &RgbImage, boxes: &[[f32; 4]], mesh: &FaceMesh) -> Result<Vec<Face>> {
    let mut faces = Vec::new();
    for b in boxes {
        let mut roi = box_roi(b, 1.5);
        let mut pts = None;
        for _ in 0..2 {
            let crop = roi.sample(img, MESH as u32);
            let Some(local) = mesh.predict(&crop)? else {
                break;
            };
            let p: Vec<[f32; 2]> = local
                .iter()
                .map(|q| roi.image_point(*q, MESH as f32))
                .collect();
            // Neigung aus den äußeren Augenwinkeln, Größe aus der Punktwolke
            let (a, e) = (p[33], p[263]);
            let angle = (e[1] - a[1]).atan2(e[0] - a[0]);
            let (c, s) = (angle.cos(), angle.sin());
            let center = [
                p.iter().map(|q| q[0]).sum::<f32>() / p.len() as f32,
                p.iter().map(|q| q[1]).sum::<f32>() / p.len() as f32,
            ];
            let (mut ext_u, mut ext_v) = (0f32, 0f32);
            for q in &p {
                let (dx, dy) = (q[0] - center[0], q[1] - center[1]);
                ext_u = ext_u.max((dx * c + dy * s).abs());
                ext_v = ext_v.max((-dx * s + dy * c).abs());
            }
            roi = Roi {
                center,
                side: 2.0 * ext_u.max(ext_v) * 1.5,
                angle,
            };
            pts = Some(p);
        }
        if let Some(pts) = pts {
            faces.push(Face { pts });
        }
    }
    Ok(faces)
}

fn face_part(
    c: &mut Canvas,
    img: &RgbImage,
    f: &Face,
    part: PersonPart,
    seg: &PartsSeg,
) -> Result<()> {
    let eyes = [f.poly(&EYE_R), f.poly(&EYE_L)];
    match part {
        PersonPart::Eyes => {
            for e in &eyes {
                c.polygon(e, true);
            }
        }
        PersonPart::Iris => {
            // Kreis um die Irismitte, beschnitten auf die Lidspalte
            let mut iris = Canvas::new(c.w as u32, c.h as u32);
            for center in [IRIS_R, IRIS_L] {
                let m = f.pts[center];
                let r = (1..5)
                    .map(|k| {
                        let q = f.pts[center + k];
                        ((q[0] - m[0]).powi(2) + (q[1] - m[1]).powi(2)).sqrt()
                    })
                    .sum::<f32>()
                    / 4.0;
                iris.circle(m, r);
            }
            let mut lids = Canvas::new(c.w as u32, c.h as u32);
            for e in &eyes {
                lids.polygon(e, true);
            }
            for ((o, i), l) in c.v.iter_mut().zip(&iris.v).zip(&lids.v) {
                *o = o.max(i.min(*l));
            }
        }
        PersonPart::Brows => {
            c.polygon(&f.poly(&BROW_R), true);
            c.polygon(&f.poly(&BROW_L), true);
        }
        PersonPart::Lips => {
            let mut lips = Canvas::new(c.w as u32, c.h as u32);
            lips.polygon(&f.poly(&LIPS_OUT), true);
            lips.polygon(&f.poly(&LIPS_IN), false);
            for (o, l) in c.v.iter_mut().zip(&lips.v) {
                *o = o.max(*l);
            }
        }
        PersonPart::Face => {
            // Gesichtshaut aus der Teil-Segmentierung (Haaransatz, Bart),
            // begrenzt auf das Gesichtsoval; Augen, Brauen, Lippen frei
            let mut skin = Canvas::new(c.w as u32, c.h as u32);
            let oval = f.poly(&OVAL);
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for p in &oval {
                x0 = x0.min(p[0]);
                y0 = y0.min(p[1]);
                x1 = x1.max(p[0]);
                y1 = y1.max(p[1]);
            }
            let roi = box_roi(&[x0, y0, x1, y1], 1.6);
            let classes = seg.predict(&roi_crop(img, &roi))?;
            paste_classes(&mut skin, &roi, &classes, &[C_FACE]);
            let mut bound = Canvas::new(c.w as u32, c.h as u32);
            bound.polygon(&grow(&oval, 1.08), true);
            for (s, b) in skin.v.iter_mut().zip(&bound.v) {
                *s = s.min(*b);
            }
            let pad = 1.0 + 0.04 * (f.size() / 200.0).clamp(0.5, 2.0);
            for e in &eyes {
                skin.polygon(&grow(e, 1.25 * pad), false);
            }
            skin.polygon(&grow(&f.poly(&BROW_R), 1.15 * pad), false);
            skin.polygon(&grow(&f.poly(&BROW_L), 1.15 * pad), false);
            skin.polygon(&grow(&f.poly(&LIPS_OUT), 1.08 * pad), false);
            for (o, s) in c.v.iter_mut().zip(&skin.v) {
                *o = o.max(*s);
            }
        }
        _ => {}
    }
    Ok(())
}

// --- Tiere ----------------------------------------------------------------------

/// Maske aller Tiere der gewählten Art. Braucht SAM für die Umrisse.
pub fn animal_mask(
    img: &DynamicImage,
    kind: AnimalKind,
    owl: &Owl,
    sam: (&Sam, &SamEmbedding),
    cache: &mut DetectCache,
) -> Result<GrayImage> {
    let rgb = img.to_rgb8();
    let groups: [&[&str]; 3] = [&BIRDS, &MAMMALS, &REPTILES];
    let mut queries = Vec::new();
    let mut idx: Vec<Vec<usize>> = Vec::new();
    for g in groups {
        let start = queries.len();
        queries.extend(g.iter().map(|q| prompt(q)));
        idx.push((start..queries.len()).collect());
    }
    // Personen mitsuchen, damit ein Mensch nicht als Säugetier zählt
    let person = queries.len();
    queries.push(prompt("person"));
    if cache.animal.is_none() {
        cache.animal = Some(owl.detect(&rgb, &queries, 0.05)?);
    }
    let dets = cache.animal.as_deref().unwrap_or_default();
    let human = [person];
    let all: Vec<usize> = idx.iter().flatten().copied().collect();
    let rects = match kind {
        AnimalKind::All => pick(dets, &all, &[&human], 0.1, 0.25),
        AnimalKind::Bird => pick(dets, &idx[0], &[&idx[1], &idx[2], &human], 0.1, 0.25),
        AnimalKind::Mammal => pick(dets, &idx[1], &[&idx[0], &idx[2], &human], 0.1, 0.25),
        AnimalKind::Reptile => pick(dets, &idx[2], &[&idx[0], &idx[1], &human], 0.1, 0.25),
    };
    log::info!("Tiere ({kind:?}): {} gefunden", rects.len());
    let (w, h) = sam.1.size();
    let mut canvas = Canvas::new(w, h);
    for r in &rects {
        paste_full(&mut canvas, &sam.0.predict_box(sam.1, sam_rect(r))?);
    }
    Ok(canvas.into_gray())
}

/// OWLv2-Rahmen sitzen oft knapp (Füße, Schwanz abgeschnitten); SAM schneidet
/// an der Rahmenkante ab. Mit 5 % Rand je Seite bleibt die Kante frei.
fn sam_rect(r: &[f32; 4]) -> [f32; 4] {
    let (dw, dh) = ((r[2] - r[0]) * 0.05, (r[3] - r[1]) * 0.05);
    [
        (r[0] - dw).max(0.0),
        (r[1] - dh).max(0.0),
        (r[2] + dw).min(1.0),
        (r[3] + dh).min(1.0),
    ]
}

/// Text für Fehlermeldungen, falls Teile fehlen
pub fn missing_hint() -> &'static str {
    tr_s(
        "Für Personen und Tiere fehlen KI-Modelle. Lade die KI-Masken unten links unter „KI“ erneut herunter.",
        "AI models for people and animals are missing. Download the AI masks again via “AI” at the bottom left.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polygons_fill_with_soft_edges_and_subtract() {
        let mut c = Canvas::new(20, 20);
        let sq = [[4.0, 4.0], [16.0, 4.0], [16.0, 16.0], [4.0, 16.0]];
        c.polygon(&sq, true);
        assert_eq!(c.v[10 * 20 + 10], 1.0);
        assert_eq!(c.v[2 * 20 + 2], 0.0);
        // Halb abgedeckte Randpixel
        let mut half = Canvas::new(4, 4);
        half.polygon(&[[0.0, 0.0], [2.5, 0.0], [2.5, 4.0], [0.0, 4.0]], true);
        assert!((half.v[2] - 0.5).abs() < 0.01, "{}", half.v[2]);
        c.polygon(&[[8.0, 8.0], [12.0, 8.0], [12.0, 12.0], [8.0, 12.0]], false);
        assert_eq!(c.v[10 * 20 + 10], 0.0);
        assert_eq!(c.v[5 * 20 + 5], 1.0);
    }

    #[test]
    fn roi_maps_points_back_with_rotation() {
        let roi = Roi {
            center: [100.0, 50.0],
            side: 40.0,
            angle: std::f32::consts::FRAC_PI_2,
        };
        // Mitte bleibt Mitte; rechts im Ausschnitt ist bei 90° unten im Foto
        let m = roi.image_point([128.0, 128.0], 256.0);
        assert!((m[0] - 100.0).abs() < 1e-3 && (m[1] - 50.0).abs() < 1e-3);
        let r = roi.image_point([256.0, 128.0], 256.0);
        assert!(
            (r[0] - 100.0).abs() < 1e-3 && (r[1] - 70.0).abs() < 1e-3,
            "{r:?}"
        );
    }

    #[test]
    fn picking_merges_overlaps_and_respects_other_groups() {
        let d = |rect: [f32; 4], scores: Vec<f32>| Detection { rect, scores };
        let dets = vec![
            d([0.1, 0.1, 0.4, 0.5], vec![0.6, 0.1]),
            d([0.11, 0.1, 0.41, 0.5], vec![0.5, 0.1]), // gleiche Stelle
            d([0.6, 0.2, 0.9, 0.6], vec![0.3, 0.7]),   // eher Gruppe 2
            d([0.5, 0.7, 0.6, 0.8], vec![0.12, 0.0]),  // zu schwach (relativ)
        ];
        let a = pick(&dets, &[0], &[&[1]], 0.1, 0.3);
        assert_eq!(a.len(), 1);
        let b = pick(&dets, &[1], &[], 0.1, 0.3);
        assert_eq!(b, vec![[0.6, 0.2, 0.9, 0.6]]);
    }
}
