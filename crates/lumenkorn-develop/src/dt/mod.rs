//! RAW-Entwicklung nach darktable (GPL-3.0-or-later), nach Rust übertragen.
//!
//! Reihenfolge und Standardwerte wie darktable im szenenbezogenen Ablauf mit
//! sigmoid: rawprepare → temperature → highlights (inpaint opposed) →
//! demosaic (RCD) → exposure → colorin → color calibration (CAT16) →
//! sigmoid → colorout. Quelle: <https://github.com/darktable-org/darktable>,
//! Stand `release-5.0.0`; `opposed.rs`, `rcd.rs` und `makernote.rs`
//! (Lichterschutz der Kamera, für exposure) auf dem Stand `release-5.6.2`.

pub(crate) mod cameras;
pub(crate) mod color;
pub(crate) mod makernote;
pub(crate) mod opposed;
pub(crate) mod raw;
pub(crate) mod rcd;
pub(crate) mod sigmoid;
