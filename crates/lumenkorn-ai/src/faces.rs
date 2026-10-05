//! Personenerkennung: Gesichter finden (YuNet) und wiedererkennen (SFace).
//!
//! Beide Modelle stammen aus dem OpenCV Model Zoo und stehen unter freien
//! Lizenzen (YuNet: MIT, SFace: Apache-2.0) – anders als die verbreiteten
//! InsightFace-Modelle, die nur nichtkommerziell genutzt werden dürfen.
//!
//! Ablauf je Foto:
//! 1. **Finden:** YuNet hat einen festen 640×640-Eingang. Das ganze Bild wird
//!    einmal eingepasst (große Gesichter) und zusätzlich in doppelter Auflösung
//!    in überlappenden Kacheln abgesucht (kleine Gesichter in Gruppenfotos).
//! 2. **Ausrichten:** Über fünf Merkmalspunkte (Augen, Nase, Mundwinkel) wird
//!    jedes Gesicht per Ähnlichkeitstransformation auf 112×112 normiert.
//! 3. **Merkmalsvektor:** SFace liefert 128 Zahlen; gleiche Personen haben eine
//!    hohe Kosinus-Ähnlichkeit (OpenCV empfiehlt ≥ 0,363 als „gleich“).

use crate::error::{Error, Result};
use image::imageops::FilterType;
use image::{Rgb, RgbImage};
use lumenkorn_develop::tr;
use ort::session::Session;
use ort::value::Tensor;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

pub const DETECTOR_FILE: &str = "yunet.onnx";
pub const EMBEDDER_FILE: &str = "sface.onnx";
pub const EMBED_DIM: usize = 128;

/// Ab dieser Ähnlichkeit gilt ein Gesicht als dieselbe Person (OpenCV: 0,363).
/// Gedacht für gut ausgeleuchtete Porträts – zu großzügig für kleine Gesichter.
pub const SAME_PERSON: f32 = 0.363;
/// Ab dieser Ähnlichkeit fasst Lumenkorn Gesichter automatisch zu einer Gruppe
/// zusammen bzw. schlägt eine benannte Person vor (Bestätigen bleibt beim Nutzer).
pub const GROUP: f32 = 0.5;

const INPUT: usize = 640;
const STRIDES: [usize; 3] = [8, 16, 32];
const SCORE_MIN: f32 = 0.8;
const NMS_IOU: f32 = 0.3;
/// Kleinere Gesichter (kürzere Kante, in Pixeln des untersuchten Bildes)
/// werden verworfen – zu wenig Information zum Wiedererkennen.
pub const MIN_FACE: f32 = 20.0;

/// Ein gefundenes Gesicht in Pixeln des untersuchten Bildes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Face {
    /// x, y, Breite, Höhe
    pub bbox: [f32; 4],
    /// rechtes Auge, linkes Auge, Nase, rechter und linker Mundwinkel (aus
    /// Sicht der Person; im Bild also zuerst das linke)
    pub landmarks: [[f32; 2]; 5],
    pub score: f32,
}

fn open(path: &Path) -> Result<Session> {
    if !crate::runtime::is_loaded() {
        return Err(Error::RuntimeMissing);
    }
    if !path.is_file() {
        return Err(Error::ModelMissing(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        ));
    }
    crate::accel::session(path)
}

// --- Finden ------------------------------------------------------------------

pub struct FaceDetector {
    session: Mutex<Session>,
}

impl FaceDetector {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(Self {
            session: Mutex::new(open(path)?),
        })
    }

    /// Gesichter in einem Bild (Pixelkoordinaten dieses Bildes).
    pub fn detect(&self, img: &RgbImage) -> Result<Vec<Face>> {
        let (w, h) = (img.width() as f32, img.height() as f32);
        let long = w.max(h);
        let mut faces = Vec::new();

        // 1. Ganzes Bild eingepasst
        let s = (INPUT as f32 / long).min(1.0);
        let small = resize(img, s);
        faces.extend(self.run_tile(&small, 0, 0, s)?);

        // 2. Doppelte Auflösung in Kacheln (kleine Gesichter)
        let s2 = (2.0 * INPUT as f32 / long).min(1.0);
        if s2 > s * 1.4 {
            let big = resize(img, s2);
            let (bw, bh) = (big.width() as usize, big.height() as usize);
            for &ty in &tile_starts(bh) {
                for &tx in &tile_starts(bw) {
                    let tile = image::imageops::crop_imm(
                        &big,
                        tx as u32,
                        ty as u32,
                        INPUT.min(bw - tx) as u32,
                        INPUT.min(bh - ty) as u32,
                    )
                    .to_image();
                    faces.extend(self.run_tile(&tile, tx, ty, s2)?);
                }
            }
        }

        let mut faces = nms(faces);
        faces.retain(|f| f.bbox[2].min(f.bbox[3]) >= MIN_FACE);
        Ok(faces)
    }

    /// Ein Durchlauf auf einem höchstens 640×640 großen Bildausschnitt, der
    /// bei (`ox`, `oy`) eines mit `scale` skalierten Bildes liegt.
    fn run_tile(&self, tile: &RgbImage, ox: usize, oy: usize, scale: f32) -> Result<Vec<Face>> {
        let n = INPUT * INPUT;
        // BGR, 0…255, rechts/unten mit Schwarz aufgefüllt (wie OpenCV)
        let mut data = vec![0f32; 3 * n];
        for (x, y, p) in tile.enumerate_pixels() {
            let i = y as usize * INPUT + x as usize;
            data[i] = p[2] as f32;
            data[n + i] = p[1] as f32;
            data[2 * n + i] = p[0] as f32;
        }
        let input = Tensor::from_array(([1usize, 3, INPUT, INPUT], data))?;
        let mut session = self.session.lock().unwrap();
        let out = session.run(vec![("input", input.into_dyn())])?;
        let get = |name: String| -> Result<Vec<f32>> {
            let (_, v) = out[name.as_str()].try_extract_tensor::<f32>()?;
            Ok(v.to_vec())
        };
        let mut faces = Vec::new();
        for s in STRIDES {
            let cls = get(format!("cls_{s}"))?;
            let obj = get(format!("obj_{s}"))?;
            let bbox = get(format!("bbox_{s}"))?;
            let kps = get(format!("kps_{s}"))?;
            let cols = INPUT / s;
            let sf = s as f32;
            for (idx, (&c, &o)) in cls.iter().zip(&obj).enumerate() {
                let score = (c.clamp(0.0, 1.0) * o.clamp(0.0, 1.0)).sqrt();
                if score < SCORE_MIN {
                    continue;
                }
                let (r, col) = ((idx / cols) as f32, (idx % cols) as f32);
                let b = &bbox[idx * 4..idx * 4 + 4];
                let cx = (col + b[0]) * sf;
                let cy = (r + b[1]) * sf;
                let bw = b[2].exp() * sf;
                let bh = b[3].exp() * sf;
                let k = &kps[idx * 10..idx * 10 + 10];
                // Zurück in Koordinaten des Ausgangsbildes
                let back = |x: f32, y: f32| [(x + ox as f32) / scale, (y + oy as f32) / scale];
                let tl = back(cx - bw / 2.0, cy - bh / 2.0);
                let mut landmarks = [[0f32; 2]; 5];
                for (j, lm) in landmarks.iter_mut().enumerate() {
                    *lm = back((k[2 * j] + col) * sf, (k[2 * j + 1] + r) * sf);
                }
                faces.push(Face {
                    bbox: [tl[0], tl[1], bw / scale, bh / scale],
                    landmarks,
                    score,
                });
            }
        }
        Ok(faces)
    }
}

/// Kachelanfänge, sodass das ganze Bild mit 128 px Überlappung bedeckt ist.
fn tile_starts(len: usize) -> Vec<usize> {
    if len <= INPUT {
        return vec![0];
    }
    let step = INPUT - 128;
    let mut v: Vec<usize> = (0..)
        .map(|i| i * step)
        .take_while(|s| s + INPUT < len)
        .collect();
    v.push(len - INPUT);
    v.dedup();
    v
}

fn resize(img: &RgbImage, s: f32) -> RgbImage {
    if (s - 1.0).abs() < 1e-3 {
        return img.clone();
    }
    let w = ((img.width() as f32 * s).round() as u32).max(1);
    let h = ((img.height() as f32 * s).round() as u32).max(1);
    image::imageops::resize(img, w, h, FilterType::Triangle)
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    let inter = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

/// Überlappende Treffer zusammenfassen; ein Gesicht, das zu großen Teilen in
/// einem anderen liegt (Kachelrand), zählt ebenfalls als doppelt.
fn nms(mut faces: Vec<Face>) -> Vec<Face> {
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut keep: Vec<Face> = Vec::new();
    for f in faces {
        let dup = keep.iter().any(|k| {
            let i = iou(&k.bbox, &f.bbox);
            let small = (f.bbox[2] * f.bbox[3]).min(k.bbox[2] * k.bbox[3]).max(1e-6);
            let x0 = k.bbox[0].max(f.bbox[0]);
            let y0 = k.bbox[1].max(f.bbox[1]);
            let x1 = (k.bbox[0] + k.bbox[2]).min(f.bbox[0] + f.bbox[2]);
            let y1 = (k.bbox[1] + k.bbox[3]).min(f.bbox[1] + f.bbox[3]);
            let inside = (x1 - x0).max(0.0) * (y1 - y0).max(0.0) / small;
            i > NMS_IOU || inside > 0.7
        });
        if !dup {
            keep.push(f);
        }
    }
    keep
}

// --- Ausrichten und Merkmalsvektor -----------------------------------------------

/// Zielpositionen der fünf Merkmalspunkte im 112×112-Bild (ArcFace-Standard,
/// wie in OpenCVs `FaceRecognizerSF::alignCrop`).
const TEMPLATE: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

/// Ähnlichkeitstransformation (Drehung, Skalierung, Verschiebung) von `src`
/// auf `dst` nach kleinsten Quadraten: (a, b, tx, ty) mit
/// x' = a·x − b·y + tx, y' = b·x + a·y + ty.
fn similarity(src: &[[f32; 2]; 5], dst: &[[f32; 2]; 5]) -> [f32; 4] {
    let mean = |p: &[[f32; 2]; 5]| {
        let (mut x, mut y) = (0.0, 0.0);
        for q in p {
            x += q[0];
            y += q[1];
        }
        [x / 5.0, y / 5.0]
    };
    let (ms, md) = (mean(src), mean(dst));
    let (mut num_a, mut num_b, mut den) = (0.0, 0.0, 0.0);
    for (s, d) in src.iter().zip(dst) {
        let (sx, sy) = (s[0] - ms[0], s[1] - ms[1]);
        let (dx, dy) = (d[0] - md[0], d[1] - md[1]);
        num_a += sx * dx + sy * dy;
        num_b += sx * dy - sy * dx;
        den += sx * sx + sy * sy;
    }
    let den = den.max(1e-6);
    let (a, b) = (num_a / den, num_b / den);
    [
        a,
        b,
        md[0] - (a * ms[0] - b * ms[1]),
        md[1] - (b * ms[0] + a * ms[1]),
    ]
}

/// Schneidet das Gesicht ausgerichtet auf 112×112 aus (bilinear).
pub fn align(img: &RgbImage, face: &Face) -> RgbImage {
    let [a, b, tx, ty] = similarity(&face.landmarks, &TEMPLATE);
    let det = (a * a + b * b).max(1e-9);
    let (w, h) = (img.width() as f32, img.height() as f32);
    RgbImage::from_fn(112, 112, |u, v| {
        // Ziel → Quelle: Umkehrung der Ähnlichkeitstransformation
        let (px, py) = (u as f32 + 0.5 - tx, v as f32 + 0.5 - ty);
        let sx = (a * px + b * py) / det - 0.5;
        let sy = (-b * px + a * py) / det - 0.5;
        if sx < -1.0 || sy < -1.0 || sx > w || sy > h {
            return Rgb([0, 0, 0]);
        }
        let x0 = sx.floor().clamp(0.0, w - 1.0);
        let y0 = sy.floor().clamp(0.0, h - 1.0);
        let x1 = (x0 + 1.0).min(w - 1.0);
        let y1 = (y0 + 1.0).min(h - 1.0);
        let (fx, fy) = ((sx - x0).clamp(0.0, 1.0), (sy - y0).clamp(0.0, 1.0));
        let p = |x: f32, y: f32| img.get_pixel(x as u32, y as u32).0;
        let (p00, p10, p01, p11) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
        let mut out = [0u8; 3];
        for c in 0..3 {
            let top = p00[c] as f32 + (p10[c] as f32 - p00[c] as f32) * fx;
            let bot = p01[c] as f32 + (p11[c] as f32 - p01[c] as f32) * fx;
            out[c] = (top + (bot - top) * fy).round().clamp(0.0, 255.0) as u8;
        }
        Rgb(out)
    })
}

pub struct FaceEmbedder {
    session: Mutex<Session>,
}

impl FaceEmbedder {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(Self {
            session: Mutex::new(open(path)?),
        })
    }

    /// Normierter Merkmalsvektor (Länge 1) eines ausgerichteten Gesichts.
    pub fn embed_aligned(&self, face112: &RgbImage) -> Result<Vec<f32>> {
        let n = 112 * 112;
        // RGB, 0…255 (OpenCV: blobFromImage mit swapRB=true auf BGR)
        let mut data = vec![0f32; 3 * n];
        for (x, y, p) in face112.enumerate_pixels() {
            let i = y as usize * 112 + x as usize;
            for c in 0..3 {
                data[c * n + i] = p[c] as f32;
            }
        }
        let input = Tensor::from_array(([1usize, 3, 112, 112], data))?;
        let mut session = self.session.lock().unwrap();
        let out = session.run(vec![("data", input.into_dyn())])?;
        let (_, v) = out["fc1"].try_extract_tensor::<f32>()?;
        let mut e = v.to_vec();
        if e.len() != EMBED_DIM {
            return Err(Error::ModelFormat(tr!(
                "SFace liefert {} statt {EMBED_DIM} Werte",
                "SFace returns {} instead of {EMBED_DIM} values",
                e.len()
            )));
        }
        normalize(&mut e);
        Ok(e)
    }

    pub fn embed(&self, img: &RgbImage, face: &Face) -> Result<Vec<f32>> {
        self.embed_aligned(&align(img, face))
    }
}

pub fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    for x in v {
        *x /= n;
    }
}

/// Kosinus-Ähnlichkeit zweier normierter Vektoren.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    // vektorisierbares Skalarprodukt aus clip (Gruppierung vergleicht viele Paare)
    crate::clip::dot(a, b)
}

/// Quadratischer Ausschnitt um das Gesicht (mit Rand) für Vorschaubilder.
pub fn thumbnail(img: &RgbImage, face: &Face, size: u32) -> RgbImage {
    let [x, y, w, h] = face.bbox;
    let side = w.max(h) * 1.6;
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let side = side.min(iw).min(ih).max(1.0);
    let x0 = (cx - side / 2.0).clamp(0.0, iw - side);
    let y0 = (cy - side / 2.0).clamp(0.0, ih - side);
    let crop = image::imageops::crop_imm(img, x0 as u32, y0 as u32, side as u32, side as u32);
    image::imageops::resize(&crop.to_image(), size, size, FilterType::Triangle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_recovers_known_transform() {
        // Drehung um 10°, Skalierung 1,5, Verschiebung (5, −3)
        let (s, c) = 10f32.to_radians().sin_cos();
        let (a, b) = (1.5 * c, 1.5 * s);
        let src = [
            [10.0, 20.0],
            [40.0, 22.0],
            [25.0, 35.0],
            [12.0, 50.0],
            [38.0, 52.0],
        ];
        let dst = src.map(|p| [a * p[0] - b * p[1] + 5.0, b * p[0] + a * p[1] - 3.0]);
        let t = similarity(&src, &dst);
        for (got, want) in t.iter().zip([a, b, 5.0, -3.0]) {
            assert!((got - want).abs() < 1e-3, "{t:?}");
        }
    }

    #[test]
    fn tiles_cover_the_image() {
        assert_eq!(tile_starts(500), vec![0]);
        for len in [641, 1000, 1280, 2000] {
            let t = tile_starts(len);
            assert_eq!(*t.last().unwrap() + INPUT, len);
            for w in t.windows(2) {
                assert!(w[1] - w[0] <= INPUT - 128, "{len}: {t:?}");
            }
        }
    }

    #[test]
    fn nms_merges_overlaps() {
        let f = |x: f32, s: f32| Face {
            bbox: [x, 0.0, 50.0, 50.0],
            landmarks: [[0.0; 2]; 5],
            score: s,
        };
        let kept = nms(vec![f(0.0, 0.9), f(5.0, 0.95), f(200.0, 0.85)]);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].bbox[0], 5.0);
    }
}
