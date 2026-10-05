//! Speicherformat für entrauschte Zwischenstufen: lineares RGB als f16.
//!
//! ```text
//! "LWDN" | Version u32 | Breite u32 | Höhe u32 | Breite·Höhe·3 × f16 (LE)
//! ```
//!
//! Ein 24-MP-Bild belegt so rund 144 MB – Entrauschen ist teuer, also lohnt
//! sich der Platz.

use crate::error::{Error, Result};
use crate::i18n::tr_s;
use crate::image_f32::ImageF32;
use half::f16;
use half::slice::HalfFloatSliceExt;
use rayon::prelude::*;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

const MAGIC: &[u8; 4] = b"LWDN";

/// Dateipfad zu einer ID; `None` bei unsicheren IDs.
pub fn path(dir: &Path, id: &str) -> Option<std::path::PathBuf> {
    crate::repair::valid_id(id).then(|| dir.join(format!("{id}.lwdn")))
}

pub fn write(path: &Path, w: usize, h: usize, data: &[[f32; 3]]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = BufWriter::with_capacity(1 << 20, std::fs::File::create(&tmp)?);
        f.write_all(MAGIC)?;
        for v in [1u32, w as u32, h as u32] {
            f.write_all(&v.to_le_bytes())?;
        }
        write_f16(&mut f, data)?;
        f.flush()?;
    }
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// Lädt die Datei; mit `max_edge` gleich verkleinert wie [`crate::Source::load`].
pub fn read(path: &Path, max_edge: Option<usize>) -> Result<ImageF32> {
    let file = std::fs::File::open(path)?;
    let len = file.metadata()?.len() as usize;
    let mut f = BufReader::with_capacity(1 << 20, file);
    let mut head = [0u8; 16];
    f.read_exact(&mut head)?;
    if &head[0..4] != MAGIC {
        return Err(Error::Params(
            tr_s(
                "keine entrauschte Lumenkorn-Datei",
                "not a denoised Lumenkorn file",
            )
            .into(),
        ));
    }
    let w = u32::from_le_bytes(head[8..12].try_into().unwrap()) as usize;
    let h = u32::from_le_bytes(head[12..16].try_into().unwrap()) as usize;
    // Größe aus dem Kopf gegen die Dateilänge prüfen: Eine beschädigte Datei
    // darf keine Riesen-Allokation auslösen (Absturz statt Fehlermeldung).
    let bytes = w
        .checked_mul(h)
        .and_then(|n| n.checked_mul(6))
        .filter(|n| *n <= len.saturating_sub(16))
        .ok_or_else(|| {
            Error::Params(tr_s("entrauschte Datei beschädigt", "denoised file corrupted").into())
        })?;
    let data = read_f16(&mut f, bytes / 6)?;
    let img = ImageF32 {
        width: w,
        height: h,
        data,
    };
    Ok(match max_edge {
        Some(m) => img.into_downscaled(m),
        None => img,
    })
}

/// Werte je Block: 2 Mi f32 → 4 MB f16. Blockweise, damit weder eine volle
/// f16-Kopie (bei 24 MP 144 MB) entsteht noch 72 Mio. einzelne Schreibaufrufe.
const BLOCK: usize = 2 << 20;
/// Teilstück für die parallele Umrechnung innerhalb eines Blocks.
const PAR: usize = 64 << 10;

/// Schreibt RGB-Werte als f16 (Little Endian), byte-gleich zur Umrechnung Wert
/// für Wert mit `f16::from_f32`. Auch für die Flicken der Reparaturen.
pub(crate) fn write_f16(f: &mut impl Write, data: &[[f32; 3]]) -> std::io::Result<()> {
    let flat: &[f32] = data.as_flattened();
    let mut buf = vec![0u8; flat.len().min(BLOCK) * 2];
    for block in flat.chunks(BLOCK) {
        let out = &mut buf[..block.len() * 2];
        out.par_chunks_mut(PAR * 2)
            .zip(block.par_chunks(PAR))
            .for_each(|(o, v)| {
                let mut h = vec![f16::ZERO; v.len()];
                h.convert_from_f32_slice(v);
                for (b, x) in o.as_chunks_mut::<2>().0.iter_mut().zip(&h) {
                    *b = x.to_le_bytes();
                }
            });
        f.write_all(out)?;
    }
    Ok(())
}

/// Liest `n` RGB-Werte im Format von [`write_f16`].
pub(crate) fn read_f16(f: &mut impl Read, n: usize) -> std::io::Result<Vec<[f32; 3]>> {
    let mut data = vec![[0f32; 3]; n];
    let flat: &mut [f32] = data.as_flattened_mut();
    let mut buf = vec![0u8; flat.len().min(BLOCK) * 2];
    for block in flat.chunks_mut(BLOCK) {
        let raw = &mut buf[..block.len() * 2];
        f.read_exact(raw)?;
        block
            .par_chunks_mut(PAR)
            .zip(raw.par_chunks(PAR * 2))
            .for_each(|(o, b)| {
                let h: Vec<f16> = b
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| f16::from_le_bytes(*c))
                    .collect();
                h.convert_to_f32_slice(o);
            });
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bisherige Umrechnung Wert für Wert – Referenz für das Dateiformat.
    fn old_bytes(data: &[[f32; 3]]) -> Vec<u8> {
        let mut v = Vec::new();
        for p in data {
            for c in p {
                v.extend_from_slice(&f16::from_f32(*c).to_le_bytes());
            }
        }
        v
    }

    #[test]
    fn bulk_f16_is_byte_identical() {
        // Sonderwerte (Subnormale, Überlauf, NaN, ±0) und viele gewöhnliche,
        // über mehrere Blöcke und Teilstücke hinweg
        let special = [
            0.0,
            -0.0,
            1e-8,
            6e-8,
            3e-5,
            65504.0,
            65520.0,
            1e9,
            -1e9,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
            -f32::NAN,
            f32::MIN_POSITIVE,
            0.333_333,
            2.25,
        ];
        let n = BLOCK + PAR + 7;
        let data: Vec<[f32; 3]> = (0..n)
            .map(|i| {
                let k = |j: usize| {
                    let m = i * 3 + j;
                    if m % 97 < special.len() {
                        special[m % 97]
                    } else {
                        ((m as f32) * 0.618_034).fract() * 4.0 - 0.5
                    }
                };
                [k(0), k(1), k(2)]
            })
            .collect();
        let mut new = Vec::new();
        write_f16(&mut new, &data).unwrap();
        let old = old_bytes(&data);
        assert_eq!(new.len(), old.len());
        assert!(new == old, "Dateiinhalt weicht ab");

        // Zurück: bitgleich mit der bisherigen Umrechnung
        let back = read_f16(&mut new.as_slice(), n).unwrap();
        for (p, c) in back.iter().zip(old.as_chunks::<6>().0) {
            for k in 0..3 {
                let v = f16::from_le_bytes([c[2 * k], c[2 * k + 1]]).to_f32();
                assert_eq!(p[k].to_bits(), v.to_bits());
            }
        }
    }

    #[test]
    fn roundtrip() {
        let d = std::env::temp_dir().join(format!("lw-dn-{}.lwdn", std::process::id()));
        let data: Vec<[f32; 3]> = (0..60).map(|i| [i as f32 / 10.0, 0.5, 2.25]).collect();
        write(&d, 10, 6, &data).unwrap();
        let back = read(&d, None).unwrap();
        assert_eq!((back.width, back.height), (10, 6));
        for (a, b) in back.data.iter().zip(&data) {
            for k in 0..3 {
                assert!((a[k] - b[k]).abs() < 0.01 * b[k].max(0.1));
            }
        }
        assert_eq!(read(&d, Some(5)).unwrap().width, 5);
        std::fs::remove_file(d).unwrap();
    }
}
