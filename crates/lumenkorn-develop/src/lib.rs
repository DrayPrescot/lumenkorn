//! Lumenkorn-Entwicklung.
//!
//! Non-destruktiv: Die Originaldatei wird nur gelesen. Eine Bearbeitung ist ein
//! [`DevelopParams`]-Satz, der bei jeder Darstellung neu auf das Quellbild
//! angewendet wird.
//!
//! Ablauf:
//!
//! ```text
//! Datei ─▶ Source::load ─▶ lineares Float-RGB (Rec.709, szenenbezogen)
//!                              │   (für die Bearbeitung als Proxy, z. B. 2048 px)
//!                              ▼
//!                       pipeline::render(params)
//!                              │   Entrauschen/Reparaturen → Weißabgleich → Belichtung
//!                              │   → Basiskurve → Tonwerte → Farbe → HSL → Masken
//!                              │   → Schärfe → Geometrie (Einzelheiten: [`pipeline`])
//!                              ▼
//!                       Anzeige-RGB (sRGB-kodiert, 0..1) ─▶ JPEG / 16-Bit-TIFF
//! ```
//!
//! Die Pipeline läuft auf der CPU (Rayon) und – wenn eingeschaltet – auf der
//! Grafikkarte ([`gpu`], wgpu-Compute-Shader mit denselben Formeln).
//! [`render_auto`] wählt und fällt bei GPU-Problemen auf die CPU zurück.

pub mod auto;
pub mod capture_sharpen;
pub mod color;
pub mod denoised;
pub(crate) mod dt;
mod error;
pub mod exr_io;
pub mod geometry;
pub mod gpu;
pub mod i18n;
mod image_f32;
pub mod lens;
pub mod lensfun;
mod local;
pub mod masks;
pub mod merge;
pub mod output;
pub mod params;
pub mod pipeline;
pub mod raster;
pub mod repair;
pub mod source;

pub use error::{Error, Result};

/// Zwischenspeicher bis zu dieser Größe (Pixel, ~12,6 MP) gelten als „klein“
/// (Vorschau, Thumbnails) und überleben [`release_large_caches`].
pub(crate) const LARGE_PIXELS: usize = 12 << 20;

/// Gibt alle Zwischenspeicher der Bildbearbeitung frei (reparierte Quelle,
/// Masken, GPU-Puffer). Für das Verlassen des Entwickeln-Modus: Danach
/// dauert das erste Rendern etwas länger, der Speicher ist aber wieder frei.
pub fn release_caches() {
    pipeline::release_cache();
    masks::release_cache();
    gpu::trim();
}

/// Nach einem Export: nur die großen Zwischenspeicher freigeben. Die der
/// Vorschau bleiben – wer währenddessen weiterbearbeitet, merkt nichts.
pub fn release_large_caches() {
    pipeline::release_large();
    masks::release_large();
    gpu::trim_large();
}
pub use image_f32::ImageF32;
pub use lens::{LensProfile, LensSummary};
pub use masks::{GrayMask, MaskSource, NoMasks};
pub use params::{
    ColorGrade, ColorSample, Crop, DenoiseRef, DevelopParams, GradeWheel, HSL_BANDS, Hsl,
    LocalAdjust, MaskLayer, MaskShape, PromptPoint, RepairLayer, Stroke, TransferGroups,
};
pub use pipeline::{RenderOptions, render, render_auto, render_auto_rgb8, render_with};
pub use source::{Source, SourceKind};
