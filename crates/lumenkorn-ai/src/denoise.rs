//! KI-Rauschreduzierung mit DRUNet (Zhang et al., „Plug-and-Play Image
//! Restoration with Deep Denoiser Prior“, KAIR, MIT-Lizenz).
//!
//! - Das Modell wird aus den offiziellen PyTorch-Gewichten lokal als ONNX gebaut
//!   ([`build_drunet_onnx`]), es ist also kein inoffiziell gehostetes Modell nötig.
//! - DRUNet erwartet ein sRGB-kodiertes Bild und eine Rauschkarte σ. Lumenkorn
//!   schätzt σ abhängig von der Helligkeit aus dem Bild selbst – Kamerarauschen
//!   ist in dunklen Bereichen stärker.
//! - Große Bilder werden in überlappenden Kacheln verarbeitet.

use crate::error::{Error, Result};
use crate::onnx_writer::{Attr, Graph, Initializer, Io, Node};
use crate::torch_legacy::{self, Tensor as TorchTensor};
use ort::session::Session;
use ort::value::Tensor;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Kachelgröße auf der CPU (große Kacheln = wenig doppelt gerechneter Rand).
const TILE_CPU: usize = 1024;
/// Auf der GPU: 512 + Rand = 576² × 64 Kanäle × 4 Byte ≈ 85 MB pro Zwischenergebnis.
/// Das bleibt unter der Puffergrenze von 128 MB, die WebGPU auf jeder Karte
/// garantiert; größere Kacheln scheitern dort beim Anlegen der Shader.
const TILE_GPU: usize = 512;
const MARGIN: usize = 32;

// --- Modell bauen -------------------------------------------------------------

/// Baut DRUNet (UNetRes, 4 Stufen, je 4 Residualblöcke) als ONNX.
/// Eingänge: `image` [1,3,H,W] und `sigma` [1,1,H,W]; H, W Vielfache von 8.
pub fn build_drunet_onnx(pth: &[u8]) -> Result<Vec<u8>> {
    let sd: HashMap<String, TorchTensor> =
        torch_legacy::load_state_dict(pth)?.into_iter().collect();
    let mut g = Graph {
        nodes: vec![],
        initializers: vec![],
        inputs: vec![],
        outputs: vec![],
    };
    let mut n = 0usize;
    let mut name = |p: &str| {
        n += 1;
        format!("{p}{n}")
    };

    let weight = |g: &mut Graph, key: &str| -> Result<String> {
        let t = sd
            .get(key)
            .ok_or_else(|| Error::ModelFormat(format!("Gewicht {key} fehlt")))?;
        g.initializers.push(Initializer {
            name: key.to_string(),
            dims: t.shape.iter().map(|d| *d as i64).collect(),
            data: t.data.clone(),
        });
        Ok(key.to_string())
    };
    macro_rules! node {
        ($op:expr, [$($i:expr),*], $attrs:expr) => {{
            let out = name($op);
            g.nodes.push(Node { op: $op, inputs: vec![$($i.to_string()),*], output: out.clone(), attrs: $attrs });
            out
        }};
    }
    let conv_attrs = |stride: i64, pad: i64, k: i64| {
        vec![
            ("kernel_shape", Attr::Ints(vec![k, k])),
            ("strides", Attr::Ints(vec![stride, stride])),
            ("pads", Attr::Ints(vec![pad, pad, pad, pad])),
        ]
    };

    let x0 = node!("Concat", ["image", "sigma"], vec![("axis", Attr::Int(1))]);
    let w = weight(&mut g, "m_head.weight")?;
    let x1 = node!("Conv", [x0, w], conv_attrs(1, 1, 3));

    // Residualblock: x + conv(relu(conv(x)))
    let mut res = |g: &mut Graph, x: String, p: &str| -> Result<String> {
        let w0 = weight(g, &format!("{p}.res.0.weight"))?;
        let w2 = weight(g, &format!("{p}.res.2.weight"))?;
        let a = name("Conv");
        g.nodes.push(Node {
            op: "Conv",
            inputs: vec![x.clone(), w0],
            output: a.clone(),
            attrs: conv_attrs(1, 1, 3),
        });
        let r = name("Relu");
        g.nodes.push(Node {
            op: "Relu",
            inputs: vec![a],
            output: r.clone(),
            attrs: vec![],
        });
        let b = name("Conv");
        g.nodes.push(Node {
            op: "Conv",
            inputs: vec![r, w2],
            output: b.clone(),
            attrs: conv_attrs(1, 1, 3),
        });
        let s = name("Add");
        g.nodes.push(Node {
            op: "Add",
            inputs: vec![x, b],
            output: s.clone(),
            attrs: vec![],
        });
        Ok(s)
    };

    let mut down = |g: &mut Graph, mut x: String, p: &str| -> Result<String> {
        for i in 0..4 {
            x = res(g, x, &format!("{p}.{i}"))?;
        }
        let w = weight(g, &format!("{p}.4.weight"))?;
        let o = format!("{p}_down");
        g.nodes.push(Node {
            op: "Conv",
            inputs: vec![x, w],
            output: o.clone(),
            attrs: conv_attrs(2, 0, 2),
        });
        Ok(o)
    };
    let x2 = down(&mut g, x1.clone(), "m_down1")?;
    let x3 = down(&mut g, x2.clone(), "m_down2")?;
    let x4 = down(&mut g, x3.clone(), "m_down3")?;
    let mut x = x4.clone();
    for i in 0..4 {
        x = res(&mut g, x, &format!("m_body.{i}"))?;
    }

    let mut up = |g: &mut Graph, x: String, skip: &str, p: &str| -> Result<String> {
        let s = format!("{p}_in");
        g.nodes.push(Node {
            op: "Add",
            inputs: vec![x, skip.to_string()],
            output: s.clone(),
            attrs: vec![],
        });
        let w = weight(g, &format!("{p}.0.weight"))?;
        let mut y = format!("{p}_up");
        g.nodes.push(Node {
            op: "ConvTranspose",
            inputs: vec![s, w],
            output: y.clone(),
            attrs: vec![
                ("kernel_shape", Attr::Ints(vec![2, 2])),
                ("strides", Attr::Ints(vec![2, 2])),
            ],
        });
        for i in 1..5 {
            y = res(g, y, &format!("{p}.{i}"))?;
        }
        Ok(y)
    };
    x = up(&mut g, x, &x4, "m_up3")?;
    x = up(&mut g, x, &x3, "m_up2")?;
    x = up(&mut g, x, &x2, "m_up1")?;
    let s = node!("Add", [x, x1], vec![]);
    let w = weight(&mut g, "m_tail.weight")?;
    let out = node!("Conv", [s, w], conv_attrs(1, 1, 3));
    g.nodes.push(Node {
        op: "Identity",
        inputs: vec![out],
        output: "denoised".into(),
        attrs: vec![],
    });

    g.inputs = vec![
        Io {
            name: "image".into(),
            dims: vec![Some(1), Some(3), None, None],
        },
        Io {
            name: "sigma".into(),
            dims: vec![Some(1), Some(1), None, None],
        },
    ];
    g.outputs = vec![Io {
        name: "denoised".into(),
        dims: vec![Some(1), Some(3), None, None],
    }];
    Ok(crate::onnx_writer::encode(&g))
}

// --- Inferenz -----------------------------------------------------------------

pub struct Denoiser {
    session: Mutex<Session>,
    /// Läuft auf der GPU (bestimmt die Kachelgröße)
    gpu: bool,
}

/// Lineares RGB-Bild (szenenbezogen, Werte ≥ 0).
pub struct Rgb<'a> {
    pub width: usize,
    pub height: usize,
    pub data: &'a [[f32; 3]],
}

impl Denoiser {
    pub fn load(path: &Path) -> Result<Self> {
        if !crate::runtime::is_loaded() {
            return Err(Error::RuntimeMissing);
        }
        let session = crate::accel::session(path)?;
        let gpu = path
            .file_name()
            .is_some_and(|n| crate::accel::on_gpu(&n.to_string_lossy()));
        Ok(Self {
            session: Mutex::new(session),
            gpu,
        })
    }

    /// Ein Durchlauf auf sRGB-kodierten Daten (CHW) mit Rauschkarte; H, W Vielfache von 8.
    fn run(&self, image: Vec<f32>, sigma: Vec<f32>, h: usize, w: usize) -> Result<Vec<f32>> {
        let img = Tensor::from_array(([1usize, 3, h, w], image))?;
        let sig = Tensor::from_array(([1usize, 1, h, w], sigma))?;
        let mut s = self.session.lock().unwrap();
        let out = s.run(vec![("image", img.into_dyn()), ("sigma", sig.into_dyn())])?;
        let (_, v) = out["denoised"].try_extract_tensor::<f32>()?;
        Ok(v.to_vec())
    }

    /// Entrauscht ein lineares Bild. `strength` skaliert die geschätzte
    /// Rauschstärke (1,0 = geschätzt). Fortschritt als (fertig, gesamt).
    /// Rechnet das Modell auf der GPU?
    pub fn on_gpu(&self) -> bool {
        self.gpu
    }

    pub fn denoise(
        &self,
        img: &Rgb,
        strength: f32,
        progress: &dyn Fn(usize, usize),
        cancel: &AtomicBool,
    ) -> Result<Vec<[f32; 3]>> {
        let (w, h) = (img.width, img.height);
        let gain = encoding_gain(img.data);
        let enc: Vec<[f32; 3]> = img
            .data
            .iter()
            .map(|p| p.map(|v| encode(v * gain)))
            .collect();
        let curve = estimate_noise(&enc, w, h);
        log::info!(
            "Rauschschätzung (σ×255 je Helligkeitsstufe): {:?}",
            curve
                .iter()
                .map(|s| (s * 255.0 * 10.0).round() / 10.0)
                .collect::<Vec<_>>()
        );

        // Rand spiegeln, damit auch Randkacheln Kontext haben
        let tile = if self.gpu { TILE_GPU } else { TILE_CPU };
        let reflect = |i: isize, n: usize| -> usize {
            let n = n as isize;
            let mut i = i;
            if i < 0 {
                i = -i - 1;
            }
            if i >= n {
                i = 2 * n - i - 1;
            }
            i.clamp(0, n - 1) as usize
        };

        // Kachelgröße an kleine Bilder anpassen (Vielfache von 8 fürs Netz)
        let tile_w = tile.min(w.div_ceil(8) * 8);
        let tile_h = tile.min(h.div_ceil(8) * 8);
        let tiles_x = w.div_ceil(tile_w);
        let tiles_y = h.div_ceil(tile_h);
        let total = tiles_x * tiles_y;
        let mut out = vec![[0f32; 3]; w * h];
        let (tw, th) = (tile_w + 2 * MARGIN, tile_h + 2 * MARGIN);
        let mut done = 0;
        progress(0, total);

        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let (x0, y0) = (tx * tile_w, ty * tile_h);
                let n = tw * th;
                let mut image = vec![0f32; 3 * n];
                let mut sigma = vec![0f32; n];
                for yy in 0..th {
                    let sy = reflect(y0 as isize + yy as isize - MARGIN as isize, h);
                    for xx in 0..tw {
                        let sx = reflect(x0 as isize + xx as isize - MARGIN as isize, w);
                        let p = enc[sy * w + sx];
                        let i = yy * tw + xx;
                        for c in 0..3 {
                            image[c * n + i] = p[c];
                        }
                        let y = 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
                        sigma[i] = sigma_at(&curve, y) * strength;
                    }
                }
                let res = self.run(image, sigma, th, tw)?;
                for yy in 0..tile_h.min(h - y0) {
                    for xx in 0..tile_w.min(w - x0) {
                        let i = (yy + MARGIN) * tw + xx + MARGIN;
                        let (x, y) = (x0 + xx, y0 + yy);
                        let orig = img.data[y * w + x];
                        let mut px = [0f32; 3];
                        for c in 0..3 {
                            let e = enc[y * w + x][c];
                            px[c] = if e >= 0.999 {
                                // Über dem Kodierbereich: Original behalten
                                orig[c]
                            } else {
                                decode(res[c * n + i].clamp(0.0, 1.0)) / gain
                            };
                        }
                        out[y * w + x] = px;
                    }
                }
                done += 1;
                progress(done, total);
            }
        }
        Ok(out)
    }
}

/// Verstärkung, damit fast alle Werte in 0…1 liegen (RAWs haben Reserven über 1).
fn encoding_gain(data: &[[f32; 3]]) -> f32 {
    let step = (data.len() / 200_000).max(1);
    let mut m: Vec<f32> = data
        .iter()
        .step_by(step)
        .map(|p| p[0].max(p[1]).max(p[2]))
        .collect();
    if m.is_empty() {
        return 1.0;
    }
    let k = (((m.len() as f32) * 0.999) as usize).min(m.len() - 1);
    let v = *m.select_nth_unstable_by(k, |a, b| a.total_cmp(b)).1;
    1.0 / v.max(1.0)
}

#[inline]
fn encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

#[inline]
fn decode(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

const BINS: usize = 8;

/// Rauschstärke je Helligkeitsstufe (σ in 0…1), robust über den Median des
/// Laplace-Filters (Immerkær). Kanten verfälschen den Median kaum.
pub fn estimate_noise(enc: &[[f32; 3]], w: usize, h: usize) -> [f32; BINS] {
    let mut samples: Vec<Vec<f32>> = vec![Vec::new(); BINS];
    let step = ((w * h) / 400_000).max(1);
    let mut k = 0;
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            k += 1;
            if k % step != 0 {
                continue;
            }
            let at = |dx: isize, dy: isize, c: usize| {
                enc[(y as isize + dy) as usize * w + (x as isize + dx) as usize][c]
            };
            let mut laps = [0f32; 3];
            // Helligkeit aus dem 3×3-Mittel: der verrauschte Einzelwert würde
            // dunkle Pixel mit positivem Rauschen in hellere Stufen schieben.
            let mut mean = 0f32;
            for (c, lap) in laps.iter_mut().enumerate() {
                let l = at(-1, -1, c) - 2.0 * at(0, -1, c) + at(1, -1, c) - 2.0 * at(-1, 0, c)
                    + 4.0 * at(0, 0, c)
                    - 2.0 * at(1, 0, c)
                    + at(-1, 1, c)
                    - 2.0 * at(0, 1, c)
                    + at(1, 1, c);
                *lap = l.abs();
                let mut box3 = 0.0;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        box3 += at(dx, dy, c);
                    }
                }
                mean += box3 / 27.0;
            }
            let b = ((mean * BINS as f32) as usize).min(BINS - 1);
            samples[b].extend(laps);
        }
    }
    // Median |L| → σ: L hat bei Gaußrauschen die Standardabweichung 6σ;
    // Faktor 1,4826 rechnet den Median des Betrags in eine Standardabweichung um.
    let mut curve = [f32::NAN; BINS];
    for (i, s) in samples.iter_mut().enumerate() {
        if s.len() >= 600 {
            let mid = s.len() / 2;
            let (_, m, _) = s.select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
            curve[i] = 1.4826 * *m / 6.0;
        }
    }
    // Lücken auffüllen
    let known: Vec<(usize, f32)> = curve
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, v)| v.is_finite())
        .collect();
    if known.is_empty() {
        return [5.0 / 255.0; BINS];
    }
    for (i, slot) in curve.iter_mut().enumerate() {
        if !slot.is_finite() {
            *slot = known
                .iter()
                .min_by_key(|(j, _)| (*j as isize - i as isize).unsigned_abs())
                .unwrap()
                .1;
        }
    }
    curve.map(|v| v.clamp(0.0, 75.0 / 255.0))
}

#[inline]
fn sigma_at(curve: &[f32; BINS], y: f32) -> f32 {
    let f = (y * BINS as f32 - 0.5).clamp(0.0, (BINS - 1) as f32);
    let i = f as usize;
    let j = (i + 1).min(BINS - 1);
    curve[i] + (curve[j] - curve[i]) * (f - i as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_estimate_matches_known_sigma() {
        // Glatter Verlauf plus bekanntes Gaußrauschen
        let (w, h) = (400, 300);
        let mut seed = 12345u64;
        let mut gauss = || {
            // Box-Muller
            let mut u = || {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((seed >> 11) as f32 / (1u64 << 53) as f32).max(1e-7)
            };
            let (a, b) = (u(), u());
            (-2.0 * a.ln()).sqrt() * (2.0 * std::f32::consts::PI * b).cos()
        };
        let sigma = 10.0 / 255.0;
        let data: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let base = 0.2 + 0.6 * (i % w) as f32 / w as f32;
                [
                    base + sigma * gauss(),
                    base + sigma * gauss(),
                    base + sigma * gauss(),
                ]
            })
            .collect();
        let curve = estimate_noise(&data, w, h);
        let mid = curve[4];
        assert!(
            (mid - sigma).abs() < sigma * 0.35,
            "geschätzt {} statt {}",
            mid * 255.0,
            sigma * 255.0
        );
    }

    #[test]
    fn follows_brightness_dependent_noise() {
        // Wie bei hoher ISO: Rauschen in den Schatten doppelt so stark
        let (w, h) = (600, 400);
        let mut seed = 99u64;
        let mut gauss = || {
            let mut u = || {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((seed >> 11) as f32 / (1u64 << 53) as f32).max(1e-7)
            };
            let (a, b) = (u(), u());
            (-2.0 * a.ln()).sqrt() * (2.0 * std::f32::consts::PI * b).cos()
        };
        let sig = |v: f32| (6.0 + 12.0 * (1.0 - v)) / 255.0;
        let data: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let base = 0.15 + 0.7 * (i % w) as f32 / w as f32;
                [0, 1, 2].map(|_| base + sig(base) * gauss())
            })
            .collect();
        let curve = estimate_noise(&data, w, h);
        for (bin, v) in [(2usize, 0.3f32), (6, 0.8)] {
            let truth = sig(v);
            assert!(
                (curve[bin] - truth).abs() < truth * 0.25,
                "Stufe {bin}: {} statt {}",
                curve[bin] * 255.0,
                truth * 255.0
            );
        }
    }

    #[test]
    fn transfer_roundtrip() {
        for i in 0..=100 {
            let v = i as f32 / 100.0;
            assert!((decode(encode(v)) - v).abs() < 1e-4);
        }
    }
}
