//! Lumenkorn-KI: läuft vollständig lokal.
//!
//! - [`clip`]: Bild- und Text-Embeddings mit einem CLIP-Modell (ONNX).
//! - [`tagger`]: Zero-Shot-Verschlagwortung gegen ein deutsches Vokabular.
//! - [`index`]: semantische Suche über die Embeddings aller Bilder.
//! - [`segment`]: Masken – Motiv, Objekt per Klick (SAM), Himmel.
//! - [`denoise`]: Rauschreduzierung mit DRUNet.
//! - [`faces`]: Personenerkennung (YuNet + SFace).
//! - [`inpaint`]: KI-Reparatur (LaMa).
//! - [`accel`]: GPU-Beschleunigung über WebGPU.
//! - [`models`]: Herunterladen von ONNX Runtime und Modelldateien.
//!
//! ONNX Runtime wird dynamisch geladen ([`runtime::init`]). Fehlt die
//! Bibliothek, funktioniert Lumenkorn weiter – nur ohne KI.

pub mod accel;
pub mod clip;
pub mod denoise;
mod error;
pub mod faces;
pub mod index;
pub mod inpaint;
pub mod models;
mod onnx_writer;
pub mod runtime;
pub mod segment;
pub mod tagger;
pub mod tokenizer;
pub mod torch_legacy;
pub mod vocab;

pub use clip::{Clip, EMBED_DIM};
pub use error::{Error, Result};
pub use index::EmbeddingIndex;
pub use tagger::{TagSuggestion, Tagger};
