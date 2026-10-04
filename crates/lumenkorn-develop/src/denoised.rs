//! Speicherformat für entrauschte Zwischenstufen: lineares RGB als f16.
//!
//! ```text
//! "LWDN" | Version u32 | Breite u32 | Höhe u32 | Breite·Höhe·3 × f16 (LE)
//! ```
//!
//! Ein 24-MP-Bild belegt so rund 144 MB – Entrauschen ist teuer, also lohnt
//! sich der Platz.

use crate::error::{Error, Result};
use crate::image_f32::ImageF32;
use half::f16;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

const MAGIC: &[u8; 4] = b"LWDN";

/// Dateipfad zu einer ID; `None` bei unsicheren IDs.
pub fn path(dir: &Path, id: &str) -> Option<std::path::PathBuf> {
    let ok = !id.is_empty()
        && id.len() < 100
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then(|| dir.join(format!("{id}.lwdn")))
}

pub fn write(path: &Path, w: usize, h: usize, data: &[[f32; 3]]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = BufWriter::with_capacity(1 << 20, std::fs::File::create(&tmp)?);
        f.write_all(MAGIC)?;
        for v in [1u32, w as u32, h as u32] {
            f.write_all(&v.to_le_bytes())?;
        }
        for p in data {
            for c in p {
                f.write_all(&f16::from_f32(*c).to_le_bytes())?;
            }
        }
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
        return Err(Error::Params("keine entrauschte Lumenkorn-Datei".into()));
    }
    let w = u32::from_le_bytes(head[8..12].try_into().unwrap()) as usize;
    let h = u32::from_le_bytes(head[12..16].try_into().unwrap()) as usize;
    // Größe aus dem Kopf gegen die Dateilänge prüfen: Eine beschädigte Datei
    // darf keine Riesen-Allokation auslösen (Absturz statt Fehlermeldung).
    let bytes = w
        .checked_mul(h)
        .and_then(|n| n.checked_mul(6))
        .filter(|n| *n <= len.saturating_sub(16))
        .ok_or_else(|| Error::Params("entrauschte Datei beschädigt".into()))?;
    let mut raw = vec![0u8; bytes];
    f.read_exact(&mut raw)?;
    let data = raw
        .as_chunks::<6>()
        .0
        .iter()
        .map(|c| [0, 2, 4].map(|i| f16::from_le_bytes([c[i], c[i + 1]]).to_f32()))
        .collect();
    let img = ImageF32 {
        width: w,
        height: h,
        data,
    };
    Ok(match max_edge {
        Some(m) => img.downscale_to(m),
        None => img,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
