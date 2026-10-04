//! CLIP-Bild- und Text-Encoder über ONNX Runtime.
//!
//! Erwartete Dateien im Modellordner (Hugging-Face-/transformers.js-Export):
//!
//! | Datei         | Eingänge                                 | Ausgang (Name enthält „embeds“) |
//! |---------------|------------------------------------------|---------------------------------|
//! | `vision.onnx` | `pixel_values` f32 [N, 3, 224, 224]      | `image_embeds` [N, D]           |
//! | `text.onnx`   | `input_ids` i64 [N, 77], opt. `attention_mask` | `text_embeds` [N, D]      |
//! | `vocab.json`, `merges.txt` | BPE-Tokenizer                 |                                 |
//!
//! Ein- und Ausgänge werden über ihre Namen gesucht, nicht über ihre Position –
//! so funktionieren verschiedene Exporte (mit und ohne `last_hidden_state`).

use crate::error::{Error, Result};
use crate::tokenizer::{CONTEXT_LENGTH, ClipTokenizer};
use image::DynamicImage;
use image::imageops::FilterType;
use lumenkorn_develop::{i18n::tr_s, tr};
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;
use std::sync::Mutex;

pub const EMBED_DIM: usize = 512;
const IMAGE_SIZE: u32 = 224;
const MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
const STD: [f32; 3] = [0.268_629_54, 0.261_302_6, 0.275_777_1];

pub struct Clip {
    vision: Mutex<Session>,
    text: Mutex<Session>,
    tokenizer: ClipTokenizer,
    vision_in: String,
    vision_out: String,
    text_ids: String,
    text_mask: Option<String>,
    text_out: String,
}

impl Clip {
    /// Lädt das Modell. ONNX Runtime muss vorher mit [`crate::runtime::init`] geladen sein.
    pub fn load(dir: &Path) -> Result<Self> {
        if !crate::runtime::is_loaded() {
            return Err(Error::RuntimeMissing);
        }
        for f in ["vision.onnx", "text.onnx"] {
            if !dir.join(f).is_file() {
                return Err(Error::ModelMissing(f.into()));
            }
        }
        let open = |file: &str| crate::accel::session(&dir.join(file));
        let vision = open("vision.onnx")?;
        let text = open("text.onnx")?;

        let input_named = |s: &Session, want: &str| {
            s.inputs()
                .iter()
                .find(|i| i.name() == want)
                .map(|i| i.name().to_string())
        };
        let embeds_out = |s: &Session| {
            s.outputs()
                .iter()
                .find(|o| o.name().contains("embeds"))
                .or_else(|| s.outputs().first())
                .map(|o| o.name().to_string())
        };

        let vision_in = input_named(&vision, "pixel_values")
            .or_else(|| vision.inputs().first().map(|i| i.name().to_string()))
            .ok_or_else(|| {
                Error::ModelFormat(
                    tr_s("Bildmodell ohne Eingang", "Image model without input").into(),
                )
            })?;
        let vision_out = embeds_out(&vision).ok_or_else(|| {
            Error::ModelFormat(tr_s("Bildmodell ohne Ausgang", "Image model without output").into())
        })?;
        let text_ids = input_named(&text, "input_ids").ok_or_else(|| {
            Error::ModelFormat(
                tr_s("Textmodell ohne input_ids", "Text model without input_ids").into(),
            )
        })?;
        let text_mask = input_named(&text, "attention_mask");
        let text_out = embeds_out(&text).ok_or_else(|| {
            Error::ModelFormat(tr_s("Textmodell ohne Ausgang", "Text model without output").into())
        })?;

        Ok(Self {
            vision: Mutex::new(vision),
            text: Mutex::new(text),
            tokenizer: ClipTokenizer::load(dir)?,
            vision_in,
            vision_out,
            text_ids,
            text_mask,
            text_out,
        })
    }

    pub fn tokenizer(&self) -> &ClipTokenizer {
        &self.tokenizer
    }

    /// L2-normierte Bild-Embeddings.
    pub fn embed_images(&self, images: &[DynamicImage]) -> Result<Vec<Vec<f32>>> {
        if images.is_empty() {
            return Ok(vec![]);
        }
        let px = (IMAGE_SIZE * IMAGE_SIZE) as usize;
        let mut data = vec![0f32; images.len() * 3 * px];
        for (n, img) in images.iter().enumerate() {
            let rgb = preprocess(img);
            let base = n * 3 * px;
            for (i, p) in rgb.pixels().enumerate() {
                for c in 0..3 {
                    data[base + c * px + i] = (p[c] as f32 / 255.0 - MEAN[c]) / STD[c];
                }
            }
        }
        let shape = [images.len(), 3, IMAGE_SIZE as usize, IMAGE_SIZE as usize];
        let input = Tensor::from_array((shape, data))?;
        let mut session = self.vision.lock().unwrap();
        let outputs = session.run(vec![(self.vision_in.as_str(), input.into_dyn())])?;
        let (shape, values) = outputs[self.vision_out.as_str()].try_extract_tensor::<f32>()?;
        split_rows(shape, values, images.len())
    }

    /// L2-normierte Text-Embeddings.
    pub fn embed_texts(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        let mut ids = Vec::with_capacity(texts.len() * CONTEXT_LENGTH);
        let mut mask = Vec::with_capacity(texts.len() * CONTEXT_LENGTH);
        for t in texts {
            let (i, m) = self.tokenizer.encode_padded(t);
            ids.extend(i);
            mask.extend(m);
        }
        let shape = [texts.len(), CONTEXT_LENGTH];
        let mut inputs = vec![(
            self.text_ids.as_str(),
            Tensor::from_array((shape, ids))?.into_dyn(),
        )];
        if let Some(m) = &self.text_mask {
            inputs.push((m.as_str(), Tensor::from_array((shape, mask))?.into_dyn()));
        }
        let mut session = self.text.lock().unwrap();
        let outputs = session.run(inputs)?;
        let (shape, values) = outputs[self.text_out.as_str()].try_extract_tensor::<f32>()?;
        split_rows(shape, values, texts.len())
    }

    /// Text-Embedding als Mittel mehrerer Formulierungen („Prompt-Ensemble“) –
    /// stabiler als eine einzelne Formulierung.
    pub fn embed_concept(&self, prompts: &[String]) -> Result<Vec<f32>> {
        let refs: Vec<&str> = prompts.iter().map(String::as_str).collect();
        let rows = self.embed_texts(&refs)?;
        let mut mean = vec![0f32; rows[0].len()];
        for r in &rows {
            for (m, v) in mean.iter_mut().zip(r) {
                *m += v;
            }
        }
        normalize(&mut mean);
        Ok(mean)
    }
}

/// Kürzere Seite auf 224, dann mittig zuschneiden (wie der CLIP-Preprocessor).
fn preprocess(img: &DynamicImage) -> image::RgbImage {
    let (w, h) = (img.width().max(1), img.height().max(1));
    let scale = IMAGE_SIZE as f32 / w.min(h) as f32;
    let nw = ((w as f32 * scale).round() as u32).max(IMAGE_SIZE);
    let nh = ((h as f32 * scale).round() as u32).max(IMAGE_SIZE);
    let resized = img.resize_exact(nw, nh, FilterType::CatmullRom);
    let x = (nw - IMAGE_SIZE) / 2;
    let y = (nh - IMAGE_SIZE) / 2;
    resized.crop_imm(x, y, IMAGE_SIZE, IMAGE_SIZE).to_rgb8()
}

fn split_rows(shape: &[i64], values: &[f32], n: usize) -> Result<Vec<Vec<f32>>> {
    let dim = *shape
        .last()
        .ok_or_else(|| Error::ModelFormat(tr_s("leerer Ausgang", "empty output").into()))?
        as usize;
    if shape.len() != 2 || values.len() != n * dim {
        return Err(Error::ModelFormat(tr!(
            "unerwartete Ausgabeform {shape:?}",
            "unexpected output shape {shape:?}"
        )));
    }
    Ok(values
        .chunks_exact(dim)
        .map(|r| {
            let mut v = r.to_vec();
            normalize(&mut v);
            v
        })
        .collect())
}

pub fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

#[inline]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
