#![allow(clippy::needless_range_loop)] // Farbkanäle: Index über drei feste Felder ist hier klarer
//! KI-Reparaturen: gespeicherte Flicken ins Quellbild einsetzen.
//!
//! Ein Flicken entsteht einmal in voller Auflösung (lumenkorn-ai, LaMa) und
//! besteht aus einem oder mehreren rechteckigen Teilen mit linearem RGB und
//! weicher Deckkraft. Die Koordinaten beziehen sich auf das aufrechte
//! Originalbild – wie Masken. So passt ein Flicken zu Vorschau (verkleinert)
//! und Export (volle Auflösung) und bleibt bei Zuschnitt und Drehen am Ort.
//!
//! Dateiformat:
//!
//! ```text
//! "LWRP" | Version u32 | Breite u32 | Höhe u32 | Anzahl Teile u32
//! je Teil: x u32 | y u32 | w u32 | h u32 | w·h·3 × f16 (RGB) | w·h × u8 (Deckkraft)
//! ```

use crate::error::{Error, Result};
use crate::i18n::tr_s;
use crate::image_f32::ImageF32;
use half::f16;
use rayon::prelude::*;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MAGIC: &[u8; 4] = b"LWRP";

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() < 100
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Ein rechteckiger Teil eines Flickens, in Pixeln des Bildes in voller Auflösung.
#[derive(Debug, Clone)]
pub struct PatchPart {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<[f32; 3]>,
    /// 0…255
    pub alpha: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Patch {
    /// Größe des Bildes, für das der Flicken berechnet wurde
    pub width: usize,
    pub height: usize,
    pub parts: Vec<PatchPart>,
}

/// Liefert gespeicherte Flicken anhand ihrer ID.
pub trait RepairSource: Sync {
    fn get(&self, id: &str) -> Option<Arc<Patch>>;
}

pub struct NoRepairs;

impl RepairSource for NoRepairs {
    fn get(&self, _: &str) -> Option<Arc<Patch>> {
        None
    }
}

impl Patch {
    pub fn write(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("tmp");
        {
            let mut f = BufWriter::with_capacity(1 << 20, std::fs::File::create(&tmp)?);
            f.write_all(MAGIC)?;
            for v in [1, self.width, self.height, self.parts.len()] {
                f.write_all(&(v as u32).to_le_bytes())?;
            }
            for p in &self.parts {
                for v in [p.x, p.y, p.w, p.h] {
                    f.write_all(&(v as u32).to_le_bytes())?;
                }
                for c in &p.rgb {
                    for v in c {
                        f.write_all(&f16::from_f32(*v).to_le_bytes())?;
                    }
                }
                f.write_all(&p.alpha)?;
            }
            f.flush()?;
        }
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn read(path: &Path) -> Result<Patch> {
        let mut f = BufReader::with_capacity(1 << 20, std::fs::File::open(path)?);
        let u32s = |n: usize, f: &mut BufReader<std::fs::File>| -> Result<Vec<usize>> {
            let mut b = vec![0u8; 4 * n];
            f.read_exact(&mut b)?;
            Ok(b.as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c) as usize)
                .collect())
        };
        let mut magic = [0u8; 4];
        f.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(Error::Params(
                tr_s("keine Lumenkorn-Reparatur", "not a Lumenkorn repair").into(),
            ));
        }
        let head = u32s(4, &mut f)?;
        let (width, height, n) = (head[1], head[2], head[3]);
        if n > 10_000 {
            return Err(Error::Params(
                tr_s("Reparatur beschädigt", "repair corrupted").into(),
            ));
        }
        let mut parts = Vec::with_capacity(n);
        for _ in 0..n {
            let r = u32s(4, &mut f)?;
            let (x, y, w, h) = (r[0], r[1], r[2], r[3]);
            if x.checked_add(w).is_none_or(|v| v > width)
                || y.checked_add(h).is_none_or(|v| v > height)
                || w.checked_mul(h).is_none_or(|n| n > 256 << 20)
            {
                return Err(Error::Params(
                    tr_s("Reparatur beschädigt", "repair corrupted").into(),
                ));
            }
            let mut raw = vec![0u8; w * h * 6];
            f.read_exact(&mut raw)?;
            let rgb = raw
                .as_chunks::<6>()
                .0
                .iter()
                .map(|c| [0, 2, 4].map(|i| f16::from_le_bytes([c[i], c[i + 1]]).to_f32()))
                .collect();
            let mut alpha = vec![0u8; w * h];
            f.read_exact(&mut alpha)?;
            parts.push(PatchPart {
                x,
                y,
                w,
                h,
                rgb,
                alpha,
            });
        }
        Ok(Patch {
            width,
            height,
            parts,
        })
    }

    /// Setzt den Flicken in ein Bild beliebiger Größe (gleiches Seitenverhältnis) ein.
    pub fn apply(&self, img: &mut ImageF32) {
        let (iw, ih) = (img.width, img.height);
        let sx = self.width as f32 / iw as f32;
        let sy = self.height as f32 / ih as f32;
        // Beim Verkleinern mehrere Proben je Zielpixel (gegen Treppen und Flimmern)
        let k = (sx.max(sy).ceil() as usize).clamp(1, 4);
        for p in &self.parts {
            let x0 = ((p.x as f32 / sx).floor() as usize).min(iw);
            let y0 = ((p.y as f32 / sy).floor() as usize).min(ih);
            let x1 = (((p.x + p.w) as f32 / sx).ceil() as usize).min(iw);
            let y1 = (((p.y + p.h) as f32 / sy).ceil() as usize).min(ih);
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            let sample = |fx: f32, fy: f32| -> ([f32; 3], f32) {
                let lx = (fx - p.x as f32 - 0.5).clamp(0.0, (p.w - 1) as f32);
                let ly = (fy - p.y as f32 - 0.5).clamp(0.0, (p.h - 1) as f32);
                let (ax, ay) = (lx as usize, ly as usize);
                let (bx, by) = ((ax + 1).min(p.w - 1), (ay + 1).min(p.h - 1));
                let (tx, ty) = (lx - ax as f32, ly - ay as f32);
                let w = [
                    (1.0 - tx) * (1.0 - ty),
                    tx * (1.0 - ty),
                    (1.0 - tx) * ty,
                    tx * ty,
                ];
                let idx = [ay * p.w + ax, ay * p.w + bx, by * p.w + ax, by * p.w + bx];
                let mut c = [0f32; 3];
                let mut a = 0f32;
                for (wi, i) in w.iter().zip(idx) {
                    for ch in 0..3 {
                        c[ch] += wi * p.rgb[i][ch];
                    }
                    a += wi * p.alpha[i] as f32 / 255.0;
                }
                (c, a)
            };
            let fx0 = p.x as f32;
            let fy0 = p.y as f32;
            let fx1 = (p.x + p.w) as f32;
            let fy1 = (p.y + p.h) as f32;
            img.data[y0 * iw..y1 * iw]
                .par_chunks_mut(iw)
                .enumerate()
                .for_each(|(r, row)| {
                    let y = y0 + r;
                    for (x, px) in row.iter_mut().enumerate().take(x1).skip(x0) {
                        let mut c = [0f32; 3];
                        let mut a = 0f32;
                        let mut n = 0f32;
                        for j in 0..k {
                            for i in 0..k {
                                let fx = (x as f32 + (i as f32 + 0.5) / k as f32) * sx;
                                let fy = (y as f32 + (j as f32 + 0.5) / k as f32) * sy;
                                n += 1.0;
                                // außerhalb des Teils: Deckkraft 0
                                if fx < fx0 || fy < fy0 || fx >= fx1 || fy >= fy1 {
                                    continue;
                                }
                                let (cc, aa) = sample(fx, fy);
                                for ch in 0..3 {
                                    c[ch] += cc[ch] * aa;
                                }
                                a += aa;
                            }
                        }
                        if a <= 0.0 {
                            continue;
                        }
                        let cover = a / n;
                        for ch in 0..3 {
                            let patch = c[ch] / a;
                            px[ch] += (patch - px[ch]) * cover;
                        }
                    }
                });
        }
    }
}

/// Flicken als Dateien in einem Ordner, mit kleinem Zwischenspeicher.
#[derive(Debug)]
pub struct RepairFiles {
    dir: PathBuf,
    cache: Mutex<Vec<(String, Arc<Patch>)>>,
}

impl RepairFiles {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            cache: Mutex::default(),
        }
    }

    /// Zwischenspeicher leeren (Dateien bleiben).
    pub fn clear_cache(&self) {
        self.cache.lock().unwrap().clear();
    }

    pub fn path(&self, id: &str) -> Option<PathBuf> {
        valid_id(id).then(|| self.dir.join(format!("{id}.lwrp")))
    }

    pub fn put(&self, id: &str, patch: Patch) -> Result<Arc<Patch>> {
        let path = self.path(id).ok_or_else(|| {
            Error::Params(crate::tr!(
                "ungültige Reparatur-ID {id}",
                "invalid repair ID {id}"
            ))
        })?;
        std::fs::create_dir_all(&self.dir)?;
        patch.write(&path)?;
        let p = Arc::new(patch);
        let mut c = self.cache.lock().unwrap();
        c.retain(|(k, _)| k != id);
        c.insert(0, (id.to_string(), p.clone()));
        c.truncate(8);
        Ok(p)
    }
}

impl RepairSource for RepairFiles {
    fn get(&self, id: &str) -> Option<Arc<Patch>> {
        {
            let c = self.cache.lock().unwrap();
            if let Some((_, p)) = c.iter().find(|(k, _)| k == id) {
                return Some(p.clone());
            }
        }
        let p = Arc::new(Patch::read(&self.path(id)?).ok()?);
        let mut c = self.cache.lock().unwrap();
        c.insert(0, (id.to_string(), p.clone()));
        c.truncate(8);
        Some(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch() -> Patch {
        // 100×50-Bild; Teil bei (20, 10), 10×10, rot, voll deckend
        Patch {
            width: 100,
            height: 50,
            parts: vec![PatchPart {
                x: 20,
                y: 10,
                w: 10,
                h: 10,
                rgb: vec![[1.0, 0.0, 0.0]; 100],
                alpha: vec![255; 100],
            }],
        }
    }

    #[test]
    fn applies_at_any_scale_and_roundtrips() {
        let mut full = ImageF32::from_fn(100, 50, |_, _| [0.2; 3]);
        patch().apply(&mut full);
        assert_eq!(full.get(25, 15), [1.0, 0.0, 0.0]);
        assert_eq!(full.get(5, 5), [0.2; 3]);

        // Halbe Größe: dieselbe Stelle
        let mut half = ImageF32::from_fn(50, 25, |_, _| [0.2; 3]);
        patch().apply(&mut half);
        assert!((half.get(12, 7)[0] - 1.0).abs() < 1e-5);
        assert_eq!(half.get(3, 3), [0.2; 3]);

        let dir = std::env::temp_dir().join(format!("lw-rp-{}", std::process::id()));
        let store = RepairFiles::new(&dir);
        store.put("t1", patch()).unwrap();
        let fresh = RepairFiles::new(&dir);
        let back = fresh.get("t1").unwrap();
        assert_eq!(back.parts[0].rgb[0], [1.0, 0.0, 0.0]);
        assert!(fresh.get("../x").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
