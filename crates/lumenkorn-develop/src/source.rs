//! Quellbild laden: RAW entwickeln bzw. Rasterbild linearisieren.

use crate::error::{Error, Result};
use crate::geometry::apply_exif_orientation;
use crate::i18n::tr_s;
use crate::image_f32::ImageF32;
use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// Szenenbezogene Sensordaten mit Reserven über Weiß (> 1.0 möglich).
    Raw,
    /// Bereits für die Anzeige gerenderte Datei (JPEG, PNG, TIFF).
    Raster,
}

/// Aufrecht stehendes, lineares Rec.709-Bild als Ausgangspunkt jeder Entwicklung.
#[derive(Clone)]
pub struct Source {
    pub kind: SourceKind,
    pub image: ImageF32,
    /// Abmessungen in voller Auflösung (nach Orientierung).
    pub full_width: usize,
    pub full_height: usize,
    /// Objektivkorrektur aus den Daten der Kamera (nur RAWs, falls vorhanden)
    pub lens: Option<std::sync::Arc<crate::lens::LensProfile>>,
    /// Farbtemperatur, auf die das Bild abgeglichen ist (Kamera-Weißabgleich
    /// eines RAWs); ohne sie gibt es nur den relativen Weißabgleich
    pub as_shot_kelvin: Option<f32>,
}

impl Source {
    /// Lädt eine Datei. `max_edge` begrenzt die längste Kante (Proxy für die
    /// Bearbeitung); `None` lädt in voller Auflösung (Export).
    ///
    /// `orientation` ist die EXIF-Orientierung aus dem Katalog; fehlt sie bei
    /// einem RAW, wird die des RAW-Decoders verwendet.
    pub fn load(
        path: &Path,
        raw: bool,
        orientation: Option<u16>,
        max_edge: Option<usize>,
    ) -> Result<Source> {
        // EXR (Ergebnis von HDR/Panorama/Focus Stacking): linear und
        // szenenbezogen – entwickelt wird es wie ein RAW (Basiskurve, Lichterreserve).
        if let (true, Some(m)) = (crate::exr_io::is_exr(path), max_edge) {
            // Verkleinert und zwischengespeichert (große Panoramen nur einmal voll lesen)
            let (img, scene) = crate::exr_io::read_downscaled(path, m)?;
            let o = orientation.unwrap_or(1);
            let (w, h) = crate::exr_io::dimensions(path).unwrap_or((img.width, img.height));
            let (full_width, full_height) = if (5..=8).contains(&o) { (h, w) } else { (w, h) };
            return Ok(Source {
                kind: if scene {
                    SourceKind::Raw
                } else {
                    SourceKind::Raster
                },
                image: apply_exif_orientation(img, o),
                full_width,
                full_height,
                lens: None,
                as_shot_kelvin: None,
            });
        }
        // Volle Abmessungen (vor Verkleinerung); Rasterbilder setzen sie
        // selbst, weil sehr große schon beim Linearisieren verkleinert werden.
        let mut full = None;
        let mut lens = None;
        let mut as_shot_kelvin = None;
        let (kind, image) = if crate::exr_io::is_exr(path) {
            let (img, scene) = crate::exr_io::read(path)?;
            let kind = if scene {
                SourceKind::Raw
            } else {
                SourceKind::Raster
            };
            (kind, apply_exif_orientation(img, orientation.unwrap_or(1)))
        } else if raw {
            let (img, raw_orientation, opened) = decode_raw(path)?;
            let o = orientation.unwrap_or(raw_orientation);
            let size = img_size(&img, o);
            lens = crate::lens::LensProfile::read_from(&opened.file, opened.meta.as_ref(), o, size)
                .map(std::sync::Arc::new);
            full = Some(size);
            as_shot_kelvin = opened.kelvin;
            // Erst verkleinern, dann aufrichten: Bei Hochformat drehte sonst
            // jede Vorschau das volle Bild (bis 720 MB Kopie)
            let img = match max_edge {
                Some(m) => img.into_downscaled(m),
                None => img,
            };
            (SourceKind::Raw, apply_exif_orientation(img, o))
        } else {
            let img = crate::raster::open(path)?;
            let o = orientation.unwrap_or(1);
            let (w, h) = (img.width() as usize, img.height() as usize);
            full = Some(if (5..=8).contains(&o) { (h, w) } else { (w, h) });
            let img = crate::raster::to_linear(img, max_edge);
            (SourceKind::Raster, apply_exif_orientation(img, o))
        };
        let (full_width, full_height) = full.unwrap_or((image.width, image.height));
        let image = match max_edge {
            Some(m) => image.into_downscaled(m),
            None => image,
        };
        Ok(Source {
            kind,
            image,
            full_width,
            full_height,
            lens,
            as_shot_kelvin,
        })
    }

    /// Kleinere Kopie, z. B. für Thumbnails aus einem bereits geladenen Proxy.
    pub fn downscaled(&self, max_edge: usize) -> Source {
        // Feldweise statt `..self.clone()` – das kopierte sonst erst das
        // ganze Bild (bei 60 MP 720 MB), nur um es gleich zu verwerfen.
        Source {
            kind: self.kind,
            image: self.image.downscale_to(max_edge),
            full_width: self.full_width,
            full_height: self.full_height,
            lens: self.lens.clone(),
            as_shot_kelvin: self.as_shot_kelvin,
        }
    }
}

/// Größe nach dem Aufrichten
fn img_size(img: &ImageF32, o: u16) -> (usize, usize) {
    if (5..=8).contains(&o) {
        (img.height, img.width)
    } else {
        (img.width, img.height)
    }
}

/// Entwickelt ein RAW bis zum szenenbezogenen, linearen sRGB wie darktable
/// (siehe [`crate::dt::raw`]); Sonderfälle (Fuji SuperCCD, Vierfarb-Sensoren,
/// Monochrom, fehlende Kameramatrix) rechnet rawler. Mit Eingangsschärfung
/// (siehe [`crate::capture_sharpen`]).
/// Die einmal geöffnete RAW-Datei mit ihren Metadaten (für Objektivdaten).
struct Opened {
    file: rawler::rawsource::RawSource,
    meta: Option<rawler::decoders::RawMetadata>,
    /// Farbtemperatur des Kamera-Weißabgleichs
    kelvin: Option<f32>,
}

fn decode_raw(path: &Path) -> Result<(ImageF32, u16, Opened)> {
    use rawler::decoders::RawDecodeParams;
    // Eine Öffnung und ein Lesen der Metadaten für Entwicklung und Objektivprofil
    let file = rawler::rawsource::RawSource::new(path)?;
    let raw = rawler::decode(&file, &RawDecodeParams::default())?;
    let meta = rawler::get_decoder(&file)
        .ok()
        .and_then(|d| d.raw_metadata(&file, &RawDecodeParams::default()).ok());
    let orientation = raw.orientation.to_u16();
    let kelvin = crate::dt::raw::as_shot_kelvin(&raw);
    let mut img = if crate::dt::raw::applies(&raw) {
        let exif = meta
            .as_ref()
            .map(crate::dt::raw::Exif::from_metadata)
            .unwrap_or_default();
        crate::dt::raw::develop(raw, exif)?
    } else {
        rawler_pipeline(&raw)?
    };
    if img.width == 0 || img.height == 0 {
        return Err(Error::Raw(tr_s("leeres Bild", "empty image").into()));
    }
    crate::capture_sharpen::apply(&mut img);
    Ok((img, orientation, Opened { file, meta, kelvin }))
}

/// rawler komplett: Sonderfälle (Fuji SuperCCD, Vierfarb-Sensoren, Monochrom).
/// Mit derselben Grundbelichtung wie die darktable-Entwicklung (+0,7 EV).
fn rawler_pipeline(raw: &rawler::RawImage) -> Result<ImageF32> {
    // Alles außer der sRGB-Gammakurve – die Tonwertabbildung macht unsere Pipeline.
    let develop = RawDevelop::new_with(&[
        ProcessingStep::Rescale,
        ProcessingStep::Demosaic,
        ProcessingStep::FujiRotate,
        ProcessingStep::CropActiveArea,
        ProcessingStep::WhiteBalance,
        ProcessingStep::Calibrate,
        ProcessingStep::CropDefault,
    ]);
    let out = develop.develop_intermediate(raw)?;
    let gain = 0.7f32.exp2();
    Ok(match out {
        Intermediate::ThreeColor(px) => ImageF32 {
            width: px.width,
            height: px.height,
            data: px.data.iter().map(|p| sanitize(*p, gain)).collect(),
        },
        Intermediate::Monochrome(px) => ImageF32 {
            width: px.width,
            height: px.height,
            data: px
                .data
                .iter()
                .map(|v| sanitize([*v, *v, *v], gain))
                .collect(),
        },
        Intermediate::FourColor(_) => {
            return Err(Error::Raw(
                tr_s(
                    "Vierfarb-Sensor nach der Kalibrierung nicht unterstützt",
                    "Four-color sensor after calibration not supported",
                )
                .into(),
            ));
        }
    })
}

#[inline]
fn sanitize(p: [f32; 3], gain: f32) -> [f32; 3] {
    let f = |v: f32| {
        if v.is_finite() {
            v.max(0.0) * gain
        } else {
            0.0
        }
    };
    [f(p[0]), f(p[1]), f(p[2])]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Alle Beispiel-RAWs lassen sich entwickeln, Werte endlich, Mitteltöne
    /// plausibel. Braucht echte RAWs: `LUMENKORN_TEST_RAW=/ordner/mit/raws`
    /// (wie `tests/raw.rs`; die CI lädt Beispiel-RAWs von sechs Kameras).
    #[test]
    fn raws_develop_like_darktable() {
        let Ok(dir) = std::env::var("LUMENKORN_TEST_RAW") else {
            eprintln!("übersprungen: LUMENKORN_TEST_RAW nicht gesetzt");
            return;
        };
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .is_some_and(|e| !e.eq_ignore_ascii_case("jpg"))
            })
            .collect();
        files.sort();
        for f in files {
            let raw = rawler::decode_file(&f).unwrap();
            let dt = crate::dt::raw::applies(&raw);
            let (img, _, _) = decode_raw(&f).unwrap();
            assert!(img.data.iter().flatten().all(|v| v.is_finite()), "{f:?}");
            let mean = img.data.iter().map(|p| p[1] as f64).sum::<f64>() / img.data.len() as f64;
            eprintln!(
                "{}: darktable-Pipeline {dt}, Mittel Grün {mean:.3}",
                f.display()
            );
            assert!(mean > 0.005 && mean < 4.0, "{f:?}: {mean}");
        }
    }
}
