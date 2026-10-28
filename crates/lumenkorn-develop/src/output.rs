//! Ausgabe: JPEG für Vorschau und Export, 16-Bit-TIFF für die Weitergabe an
//! andere Programme.

use crate::error::{Error, Result};
use crate::image_f32::ImageF32;
use image::{ImageBuffer, Rgb, RgbImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub use crate::embed::ExportMeta;

// Direkt in den fertigen Puffer schreiben: `flat_map_iter().collect()` ist
// nicht indexiert – rayon sammelte Teilstücke und fügte sie mit einer
// weiteren Kopie zusammen (bei 45 MP 135 bzw. 270 MB).
pub fn to_rgb8(img: &ImageF32) -> RgbImage {
    let mut data = vec![0u8; img.data.len() * 3];
    data.par_chunks_mut(3)
        .zip(img.data.par_iter())
        .for_each(|(o, c)| {
            for k in 0..3 {
                o[k] = (c[k].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        });
    ImageBuffer::from_raw(img.width as u32, img.height as u32, data).expect("Puffergröße")
}

pub fn to_rgb16(img: &ImageF32) -> ImageBuffer<Rgb<u16>, Vec<u16>> {
    let mut data = vec![0u16; img.data.len() * 3];
    data.par_chunks_mut(3)
        .zip(img.data.par_iter())
        .for_each(|(o, c)| {
            for k in 0..3 {
                o[k] = (c[k].clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
            }
        });
    ImageBuffer::from_raw(img.width as u32, img.height as u32, data).expect("Puffergröße")
}

pub fn encode_jpeg(img: &ImageF32, quality: u8) -> Result<Vec<u8>> {
    encode_jpeg_rgb8(&to_rgb8(img), quality)
}

/// JPEG aus bereits 8-bittigen Daten (z. B. direkt von der GPU).
///
/// Ab Qualität 90 mit voller Farbauflösung (4:4:4, für Exporte), darunter
/// mit halbierter (4:2:0) wie bei Kamera-JPEGs – fürs Auge gleich, Vorschauen
/// werden so ein Drittel kleiner.
pub fn encode_jpeg_rgb8(rgb: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    encode_jpeg_rgb8_meta(rgb, quality, None)
}

/// Wie [`encode_jpeg_rgb8`], mit EXIF und XMP aus `meta`.
pub fn encode_jpeg_rgb8_meta(
    rgb: &RgbImage,
    quality: u8,
    meta: Option<&ExportMeta>,
) -> Result<Vec<u8>> {
    let (w, h) = rgb.dimensions();
    if w > u16::MAX as u32 || h > u16::MAX as u32 {
        return Err(Error::Params(crate::tr!(
            "{w}×{h} ist zu groß für JPEG (max. 65535)",
            "{w}×{h} is too large for JPEG (max. 65535)"
        )));
    }
    let mut out = Vec::with_capacity(rgb.as_raw().len() / 12);
    let mut enc = jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100));
    enc.set_sampling_factor(if quality >= 90 {
        jpeg_encoder::SamplingFactor::F_1_1
    } else {
        jpeg_encoder::SamplingFactor::F_2_2
    });
    for seg in meta.map(crate::embed::jpeg_segments).unwrap_or_default() {
        enc.add_app_segment(1, seg)?;
    }
    enc.encode(
        rgb.as_raw(),
        w as u16,
        h as u16,
        jpeg_encoder::ColorType::Rgb,
    )?;
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Jpeg,
    Tiff16,
    Png,
    /// Leicht verlustbehaftetes DNG, bearbeitet oder als Original (siehe
    /// [`ExportOptions::dng_edited`] und [`crate::dng`])
    Dng,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Tiff16 => "tif",
            ExportFormat::Png => "png",
            ExportFormat::Dng => "dng",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    pub format: ExportFormat,
    /// Mehrere Formate auf einmal (z. B. DNG und JPEG); leer: nur `format`
    #[serde(default)]
    pub formats: Vec<ExportFormat>,
    /// 1…100, nur für JPEG
    #[serde(default = "default_quality")]
    pub quality: u8,
    /// Längste Kante in Pixeln; `None` = volle Auflösung
    #[serde(default)]
    pub long_edge: Option<usize>,
    /// DNG mit allen Bearbeitungen; sonst die Daten des Originals ohne
    /// Bearbeitung (zum Weiterentwickeln in anderen Programmen)
    #[serde(default = "yes")]
    pub dng_edited: bool,
    /// Gibt es die Zieldatei schon? Standard: mit „-2“ daneben legen
    #[serde(default)]
    pub on_conflict: OnConflict,
}

/// Umgang mit schon vorhandenen Zieldateien beim Export
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnConflict {
    /// Zusätzlich exportieren: `name-2.jpg`
    #[default]
    Rename,
    /// Vorhandene Datei ersetzen (nie ein Original der Bibliothek)
    Overwrite,
    /// Vorhandene Datei behalten, dieses Format nicht schreiben
    Skip,
}

fn default_quality() -> u8 {
    90
}

fn yes() -> bool {
    true
}

impl ExportOptions {
    /// Zu schreibende Formate, ohne Doppelte
    pub fn all_formats(&self) -> Vec<ExportFormat> {
        let mut out = Vec::new();
        let list = if self.formats.is_empty() {
            std::slice::from_ref(&self.format)
        } else {
            &self.formats[..]
        };
        for f in list {
            if !out.contains(f) {
                out.push(*f);
            }
        }
        out
    }
}

/// Schreibt das entwickelte Bild. Schreibt zuerst in eine temporäre Datei, damit
/// nie eine halbe Datei am Ziel liegt. `meta` landet als EXIF und XMP in der
/// Datei (alle Formate).
pub fn write(
    img: &ImageF32,
    path: &Path,
    opts: &ExportOptions,
    meta: Option<&ExportMeta>,
) -> Result<()> {
    write_as(img, path, opts, false, meta)
}

/// DNG aus dem unbearbeiteten Original (`scene`: lineares Rec.709 aus
/// [`crate::Source`]).
pub fn write_original_dng(
    scene: &ImageF32,
    path: &Path,
    opts: &ExportOptions,
    meta: Option<&ExportMeta>,
) -> Result<()> {
    let opts = ExportOptions {
        format: ExportFormat::Dng,
        ..opts.clone()
    };
    write_as(scene, path, &opts, true, meta)
}

/// 8-Bit-PNG; EXIF als `eXIf`-Chunk, XMP als `iTXt` (`XML:com.adobe.xmp`)
fn write_png<W: std::io::Write>(img: &ImageF32, w: W, meta: Option<&ExportMeta>) -> Result<()> {
    let err = |e: png::EncodingError| Error::Params(format!("PNG: {e}"));
    let rgb = to_rgb8(img);
    let mut enc = png::Encoder::new(w, rgb.width(), rgb.height());
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Balanced);
    if let Some(x) = meta.and_then(|m| m.xmp.clone()) {
        enc.add_itxt_chunk("XML:com.adobe.xmp".into(), x)
            .map_err(err)?;
    }
    let mut wr = enc.write_header().map_err(err)?;
    if let Some(m) = meta {
        wr.write_chunk(png::chunk::eXIf, &crate::embed::exif_tiff(m))
            .map_err(err)?;
    }
    wr.write_image_data(rgb.as_raw()).map_err(err)?;
    wr.finish().map_err(err)
}

/// 16-Bit-TIFF, mit Metadaten im ersten Verzeichnis und einem EXIF-Verzeichnis
fn write_tiff16<W: std::io::Write + std::io::Seek>(
    img: &ImageF32,
    w: W,
    meta: Option<&ExportMeta>,
) -> Result<()> {
    use tiff::encoder::{Rational, TiffEncoder, colortype::RGB16};
    use tiff::tags::Tag;
    let err = |e: tiff::TiffError| Error::Params(format!("TIFF: {e}"));
    let rgb = to_rgb16(img);
    let mut enc = TiffEncoder::new(w).map_err(err)?;
    let (ifd0, exif) = meta.map(crate::embed::tags).unwrap_or_default();
    let write = |d: &mut tiff::encoder::DirectoryEncoder<_, _>,
                 tags: &[(u16, crate::embed::Val)]| {
        use crate::embed::Val;
        for (t, v) in tags {
            let tag = Tag::from_u16_exhaustive(*t);
            match v {
                Val::Ascii(s) => d.write_tag(tag, s.as_str()),
                Val::Short(s) => d.write_tag(tag, *s),
                Val::Rational(n, dd) => d.write_tag(tag, Rational { n: *n, d: *dd }),
                // Kann das tiff-Crate nicht schreiben (ExifVersion), Leser kommen ohne aus
                Val::Undefined(_) => Ok(()),
            }?;
        }
        Ok::<_, tiff::TiffError>(())
    };
    let exif_off = if exif.is_empty() {
        None
    } else {
        let mut d = enc.extra_directory().map_err(err)?;
        write(&mut d, &exif).map_err(err)?;
        Some(d.finish_with_offsets().map_err(err)?.offset)
    };
    let mut im = enc
        .new_image::<RGB16>(rgb.width(), rgb.height())
        .map_err(err)?;
    let d = im.encoder();
    write(d, &ifd0).map_err(err)?;
    if let Some(off) = exif_off {
        d.write_tag(Tag::ExifDirectory, off).map_err(err)?;
    }
    if let Some(x) = meta.and_then(|m| m.xmp.as_ref()) {
        d.write_tag(Tag::from_u16_exhaustive(700), x.as_bytes())
            .map_err(err)?;
    }
    im.write_data(rgb.as_raw()).map_err(err)?;
    Ok(())
}

fn write_as(
    img: &ImageF32,
    path: &Path,
    opts: &ExportOptions,
    scene: bool,
    meta: Option<&ExportMeta>,
) -> Result<()> {
    // Ohne Verkleinern keine Kopie (bei 60 MP 720 MB)
    let img: std::borrow::Cow<ImageF32> = match opts.long_edge {
        Some(m) if m > 0 && m < img.long_edge() => std::borrow::Cow::Owned(img.downscale_to(m)),
        _ => std::borrow::Cow::Borrowed(img),
    };
    // PNG/TIFF direkt in die Datei kodieren statt erst in den Speicher
    // (bei einem 150-MP-Panorama als 16-Bit-TIFF ~900 MB weniger)
    let file = |p: &Path| -> Result<std::io::BufWriter<std::fs::File>> {
        Ok(std::io::BufWriter::with_capacity(
            1 << 20,
            std::fs::File::create(p)?,
        ))
    };
    // Fehler der Kodierung unverändert zurückgeben (io::Error trägt sie nicht)
    let mut enc_err: Option<crate::error::Error> = None;
    let res = crate::fsutil::write_atomic_with(path, |tmp| {
        (|| -> Result<()> {
            match opts.format {
                ExportFormat::Jpeg => std::fs::write(
                    tmp,
                    encode_jpeg_rgb8_meta(&to_rgb8(&img), opts.quality.clamp(1, 100), meta)?,
                )?,
                ExportFormat::Png => {
                    let mut f = file(tmp)?;
                    write_png(&img, &mut f, meta)?;
                    std::io::Write::flush(&mut f)?;
                }
                ExportFormat::Tiff16 => {
                    let mut f = file(tmp)?;
                    write_tiff16(&img, &mut f, meta)?;
                    std::io::Write::flush(&mut f)?;
                }
                ExportFormat::Dng => {
                    let content = if scene {
                        crate::dng::DngContent::Scene(&img)
                    } else {
                        crate::dng::DngContent::Developed(&img)
                    };
                    std::fs::write(tmp, crate::dng::encode_lossy(content, meta)?)?
                }
            }
            Ok(())
        })()
        .map_err(|e| match e {
            crate::error::Error::Io(io) => io,
            e => {
                let msg = e.to_string();
                enc_err = Some(e);
                std::io::Error::other(msg)
            }
        })
    });
    match res {
        Ok(()) => Ok(()),
        Err(io) => Err(enc_err.take().unwrap_or(io.into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Echte RAWs aus `LUMENKORN_TEST_RAW`: Das Original-DNG liest Lumenkorn
    /// wieder ein, Größe und Farben stimmen
    #[test]
    fn original_dng_from_real_raws() {
        let Ok(dir) = std::env::var("LUMENKORN_TEST_RAW") else {
            return;
        };
        let out = std::env::temp_dir().join(format!("lw-dng-{}", std::process::id()));
        std::fs::create_dir_all(&out).unwrap();
        let mean = |img: &ImageF32| -> [f32; 3] {
            let n = img.data.len() as f32;
            let s = img
                .data
                .iter()
                .fold([0.0f32; 3], |a, c| [0, 1, 2].map(|k| a[k] + c[k].max(0.0)));
            s.map(|v| v / n)
        };
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
            if ![
                "rw2", "nef", "arw", "cr2", "cr3", "orf", "raf", "pef", "dng",
            ]
            .contains(&ext.to_ascii_lowercase().as_str())
            {
                continue;
            }
            let src = crate::Source::load(&p, true, None, Some(1024)).unwrap();
            let target = out.join(p.file_stem().unwrap()).with_extension("dng");
            let opts = ExportOptions {
                format: ExportFormat::Dng,
                formats: Vec::new(),
                quality: 90,
                long_edge: None,
                dng_edited: false,
                on_conflict: OnConflict::Rename,
            };
            write_original_dng(&src.image, &target, &opts, None).unwrap();
            let back = crate::Source::load(&target, true, None, None).unwrap();
            assert_eq!(
                (back.image.width, back.image.height),
                (src.image.width, src.image.height),
                "{}",
                p.display()
            );
            // Gleiche Farben; die Helligkeit trägt `BaselineExposure`, das
            // Lumenkorns eigener DNG-Leser (anders als Adobe) nicht anwendet
            let (a, b) = (mean(&src.image), mean(&back.image));
            for k in [0, 2] {
                let (ra, rb) = (a[k] / a[1], b[k] / b[1]);
                assert!((ra - rb).abs() < 0.03, "{}: {a:?} vs {b:?}", p.display());
            }
        }
        std::fs::remove_dir_all(&out).unwrap();
    }

    #[test]
    fn writes_all_formats() {
        let d = std::env::temp_dir().join(format!("lw-export-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let img = ImageF32::from_fn(300, 200, |x, y| [x as f32 / 300.0, y as f32 / 200.0, 0.5]);
        let meta = ExportMeta {
            camera: Some("Testkamera".into()),
            exposure: Some(0.004),
            captured_at: Some("2025-06-01T19:42:07".into()),
            xmp: Some("<x:xmpmeta/>".into()),
            ..Default::default()
        };
        for (format, name) in [
            (ExportFormat::Jpeg, "a.jpg"),
            (ExportFormat::Tiff16, "a.tif"),
            (ExportFormat::Png, "a.png"),
            (ExportFormat::Dng, "a.dng"),
        ] {
            let p = d.join(name);
            write(
                &img,
                &p,
                &ExportOptions {
                    format,
                    formats: Vec::new(),
                    quality: 90,
                    long_edge: Some(150),
                    dng_edited: true,
                    on_conflict: OnConflict::Rename,
                },
                Some(&meta),
            )
            .unwrap();
            if format != ExportFormat::Dng {
                let back = image::open(&p).unwrap();
                assert_eq!((back.width(), back.height()), (150, 100), "{name}");
                if format == ExportFormat::Tiff16 {
                    assert_eq!(back.color(), image::ColorType::Rgb16);
                }
            }
            // Metadaten in allen Formaten, für einen unabhängigen Leser lesbar
            {
                let f = std::fs::File::open(&p).unwrap();
                let exif = exif::Reader::new()
                    .read_from_container(&mut std::io::BufReader::new(f))
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
                let get = |t| {
                    exif.get_field(t, exif::In::PRIMARY)
                        .map(|f| f.display_value().to_string())
                };
                assert_eq!(
                    get(exif::Tag::Model).as_deref(),
                    Some("\"Testkamera\""),
                    "{name}"
                );
                assert_eq!(
                    get(exif::Tag::ExposureTime).as_deref(),
                    Some("1/250"),
                    "{name}"
                );
                assert_eq!(
                    get(exif::Tag::Orientation).as_deref(),
                    Some("row 0 at top and column 0 at left"),
                    "{name}"
                );
                assert!(get(exif::Tag::DateTimeOriginal).is_some(), "{name}");
                let raw = std::fs::read(&p).unwrap();
                assert!(raw.windows(12).any(|w| w == b"<x:xmpmeta/>"), "{name}: XMP");
            }
        }
        // keine temporären Reste
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 4);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
