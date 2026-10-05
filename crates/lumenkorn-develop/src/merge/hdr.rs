//! HDR aus einer Belichtungsreihe.
//!
//! 1. Referenz ist die mittlere Belichtung; alle anderen werden auf sie
//!    ausgerichtet (Freihand-Reihen verwackeln leicht).
//! 2. Belichtungsverhältnis je Bild aus den Pixeln selbst gemessen (Median
//!    der Helligkeitsverhältnisse gut belichteter Stellen) – EXIF-Angaben sind
//!    nicht nötig und Blendenstufen-Rundungen spielen keine Rolle.
//! 3. Zusammenführen zur linearen Strahldichte: je Pixel ein gewichtetes Mittel
//!    der durch ihr Verhältnis geteilten Werte. Gewicht hoch für mittlere Werte,
//!    null für ausgefressene und fast schwarze; länger belichtete Bilder zählen
//!    mehr (weniger Rauschen).
//! 4. Geisterbilder (optional): Wo sich etwas bewegt hat, weicht die Strahldichte
//!    eines Bildes deutlich von der Referenz ab – dort zählt nur die Referenz
//!    (bzw. das nächstgelegene gut belichtete Bild).
//!
//! Ergebnis im Maßstab der Referenzbelichtung: Werte über 1 sind die aus den
//! dunkleren Aufnahmen gewonnenen Lichter.

use super::align::{self, Features, Mat3};
use super::{Loader, Progress, check};
use crate::error::{Error, Result};
use crate::i18n::tr_s;
use crate::image_f32::ImageF32;
use rayon::prelude::*;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HdrOptions {
    /// Freihand-Reihen ausrichten
    pub align: bool,
    /// Bewegte Motive (Geisterbilder) unterdrücken
    pub deghost: bool,
}

impl Default for HdrOptions {
    fn default() -> Self {
        Self {
            align: true,
            deghost: true,
        }
    }
}

fn luma(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

/// Gewicht eines Pixelwerts: 0 bei Rauschen und an der Sättigungsgrenze.
fn weight(p: [f32; 3]) -> f32 {
    let hi = p[0].max(p[1]).max(p[2]);
    let lo = luma(p);
    let top = 1.0 - crate::color::smoothstep(0.80, 0.97, hi);
    let bottom = crate::color::smoothstep(0.002, 0.03, lo);
    top * bottom
}

/// Ergebnis von [`merge`].
pub struct HdrMerged {
    /// Lineares HDR im Maßstab der Referenzbelichtung
    pub image: ImageF32,
    /// Index (in Ladereihenfolge) der Referenzaufnahme – deren Aufnahmedaten
    /// (Belichtungszeit, ISO, Blende) passen zum Ergebnis, nicht die des ersten Bildes.
    pub reference: usize,
}

pub fn merge(
    n: usize,
    load: Loader,
    opts: HdrOptions,
    progress: Progress,
    cancel: &AtomicBool,
) -> Result<HdrMerged> {
    if n < 2 {
        return Err(Error::Params(
            tr_s(
                "Für HDR mindestens zwei Belichtungen wählen",
                "Select at least two exposures for HDR",
            )
            .into(),
        ));
    }
    // --- 1. Laden, Merkmale, mittlere Helligkeit --------------------------------
    // Volle Bilder behalten, solange es in den Speicher passt (sonst neu laden).
    const KEEP_BYTES: usize = 3 << 30;
    let mut full: Vec<Option<ImageF32>> = Vec::new();
    let mut feats: Vec<Features> = Vec::new();
    let mut smalls: Vec<ImageF32> = Vec::new();
    let mut kept = 0usize;
    let mut size = (0, 0);
    for i in 0..n {
        check(cancel)?;
        progress(tr_s("Bilder laden", "Loading images"), i, n);
        let img = load(i)?;
        if i == 0 {
            size = (img.width, img.height);
        } else if (img.width, img.height) != size {
            return Err(Error::Params(
                tr_s(
                    "Die Belichtungsreihe muss aus gleich großen Fotos bestehen",
                    "The bracketed series must consist of photos of the same size",
                )
                .into(),
            ));
        }
        feats.push(align::features(&img, 1200, 2000));
        smalls.push(img.downscale_to(600));
        let bytes = img.data.len() * 12;
        if kept + bytes <= KEEP_BYTES {
            kept += bytes;
            full.push(Some(img));
        } else {
            full.push(None);
        }
    }
    let (w, h) = size;

    // Referenz: mittlere Helligkeit
    let mean =
        |im: &ImageF32| im.data.iter().map(|p| luma(*p)).sum::<f32>() / im.data.len().max(1) as f32;
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| mean(&smalls[a]).total_cmp(&mean(&smalls[b])));
    let r = order[n / 2];

    // --- 2. Ausrichten und Belichtungsverhältnisse -------------------------------
    // Jede Aufnahme wird mit ihrer Nachbarin in Richtung Referenz verglichen und
    // die Ergebnisse verkettet: Bei weiten Reihen (±6 EV) haben die äußersten
    // Bilder mit der Referenz kaum gemeinsame gut belichtete Stellen, mit der
    // Nachbarin (2 EV) aber reichlich.
    let pos_r = n / 2;
    // (Bild, Nachbarin Richtung Referenz), von innen nach außen
    let chain: Vec<(usize, usize)> = (0..pos_r)
        .rev()
        .map(|p| (order[p], order[p + 1]))
        .chain((pos_r + 1..n).map(|p| (order[p], order[p - 1])))
        .collect();

    // homs[i]: Referenzkoordinaten → Koordinaten in Bild i
    progress(tr_s("Ausrichten", "Aligning"), 0, n);
    let mut homs: Vec<Mat3> = vec![align::IDENTITY; n];
    if opts.align {
        for (k, &(i, j)) in chain.iter().enumerate() {
            check(cancel)?;
            // fit.h bildet Bild i auf j ab; gebraucht wird j → i, verkettet mit Referenz → j
            let step = align::align(&feats[j], &feats[i], 25).and_then(|f| align::invert(&f.h));
            homs[i] = match step {
                Some(j_to_i) => align::mul(&j_to_i, &homs[j]),
                // Direkt mit der Referenz versuchen, sonst Stativ annehmen
                None => align::align(&feats[r], &feats[i], 25)
                    .and_then(|f| align::invert(&f.h))
                    .unwrap_or(homs[j]),
            };
            progress(tr_s("Ausrichten", "Aligning"), k + 1, n);
        }
    }
    drop(feats);

    // Verhältnisse in voller Auflösung messen: In verkleinerten Bildern mischen
    // sich ausgefressene mit gesunden Pixeln, das verfälscht das Verhältnis.
    progress(tr_s("Belichtung messen", "Measuring exposure"), 0, n);
    let step = ((w * h) as f64 / 300_000.0).sqrt().max(1.0) as usize;
    let mut ratio = vec![1.0f32; n];
    for (k, &(i, j)) in chain.iter().enumerate() {
        check(cancel)?;
        progress(tr_s("Belichtung messen", "Measuring exposure"), k, n);
        // Ohne Kopie, wenn beide im Speicher liegen
        let (li, lj);
        let img_i: &ImageF32 = match &full[i] {
            Some(img) => img,
            None => {
                li = load(i)?;
                &li
            }
        };
        let img_j: &ImageF32 = match &full[j] {
            Some(img) => img,
            None => {
                lj = load(j)?;
                &lj
            }
        };
        let (hi, hj) = (homs[i], homs[j]);
        let mut q: Vec<f32> = (0..h)
            .into_par_iter()
            .step_by(step)
            .flat_map_iter(|y| {
                (0..w).step_by(step).filter_map(move |x| {
                    let (uj, vj) = align::apply(&hj, x as f64, y as f64);
                    let (ui, vi) = align::apply(&hi, x as f64, y as f64);
                    let a = align::sample(img_j, uj, vj)?;
                    let b = align::sample(img_i, ui, vi)?;
                    let (la, lb) = (luma(a), luma(b));
                    (weight(a) > 0.5 && weight(b) > 0.5 && la > 1e-4).then(|| lb / la)
                })
            })
            .collect();
        if q.len() < 50 {
            return Err(Error::Params(
                tr_s(
                    "Die Belichtungen überlappen sich kaum – ist das wirklich eine Reihe? (Abstand höchstens ~3 EV)",
                    "The exposures barely overlap – is this really a bracketed series? (spacing at most ~3 EV)",
                )
                .into(),
            ));
        }
        let mid = q.len() / 2;
        ratio[i] = ratio[j] * *q.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1;
    }
    log::info!("HDR: Referenz {r}, Belichtungsverhältnisse {ratio:?}");

    // --- 3./4. Zusammenführen ----------------------------------------------------
    let mut acc = vec![[0f32; 3]; w * h];
    let mut wsum = vec![0f32; w * h];
    // Fallback, wo kein Bild gut belichtet ist: dunkelstes (für Lichter) bzw.
    // hellstes (für Schatten) – jeweils auf die Referenz skaliert.
    let darkest = order[0];
    let brightest = order[n - 1];
    let reference = match full[r].take() {
        Some(img) => img,
        None => load(r)?,
    };
    for (step, &i) in order.iter().enumerate() {
        check(cancel)?;
        progress(tr_s("Zusammenführen", "Merging"), step, n);
        let owned: Option<ImageF32> = if i == r {
            None
        } else {
            Some(match full[i].take() {
                Some(img) => img,
                None => load(i)?,
            })
        };
        let img: &ImageF32 = owned.as_ref().unwrap_or(&reference);
        let hm = homs[i];
        let k = ratio[i];
        // Längere Belichtung = besseres Signal-Rausch-Verhältnis
        let snr = k.max(1e-3);
        acc.par_chunks_mut(w)
            .zip(wsum.par_chunks_mut(w))
            .enumerate()
            .for_each(|(y, (arow, wrow))| {
                for x in 0..w {
                    let p = if i == r {
                        Some(reference.data[y * w + x])
                    } else {
                        let (u, v) = align::apply(&hm, x as f64, y as f64);
                        align::sample(img, u, v)
                    };
                    let Some(p) = p else { continue };
                    let mut wt = weight(p) * snr;
                    if opts.deghost && i != r {
                        let rp = reference.data[y * w + x];
                        let wr = weight(rp);
                        if wr > 0.3 {
                            let (lr, li) = (luma(rp).max(1e-5), (luma(p) / k).max(1e-5));
                            let dev = (li / lr).ln().abs();
                            // Ab ~40 % Abweichung: bewegt – nur Referenz verwenden
                            wt *= 1.0 - crate::color::smoothstep(0.25, 0.45, dev);
                        }
                    }
                    if wt <= 0.0 {
                        continue;
                    }
                    for c in 0..3 {
                        arow[x][c] += wt * p[c] / k;
                    }
                    wrow[x] += wt;
                }
            });
        // Fallback-Bilder brauchen wir gleich noch; die übrigen sind fertig
        if i == darkest || i == brightest {
            full[i] = owned;
        }
    }

    progress(tr_s("Zusammenführen", "Merging"), n, n);
    let mut fetch = |i: usize| -> Result<Option<ImageF32>> {
        if i == r {
            return Ok(None);
        }
        Ok(Some(match full[i].take() {
            Some(img) => img,
            None => load(i)?,
        }))
    };
    let dark_img = fetch(darkest)?;
    let bright_img = fetch(brightest)?;
    let pick = |idx: usize, img: &Option<ImageF32>, x: usize, y: usize| -> [f32; 3] {
        let src = img.as_ref().unwrap_or(&reference);
        let p = if idx == r {
            Some(reference.data[y * w + x])
        } else {
            let (u, v) = align::apply(&homs[idx], x as f64, y as f64);
            align::sample(src, u, v)
        };
        // Außerhalb der ausgerichteten Aufnahme: Referenz (schon im richtigen Maßstab)
        match p {
            Some(p) => p.map(|c| c / ratio[idx]),
            None => reference.data[y * w + x],
        }
    };
    // Im Ergebnispuffer selbst normieren (spart eine Kopie in voller Auflösung)
    acc.par_iter_mut()
        .zip(wsum.par_iter())
        .enumerate()
        .for_each(|(k, (a, ws))| {
            if *ws > 1e-6 {
                *a = a.map(|c| c / ws);
                return;
            }
            let (x, y) = (k % w, k / w);
            // Hell (ausgefressen) → dunkelste Aufnahme, sonst hellste
            *a = if luma(reference.data[k]) > 0.5 {
                pick(darkest, &dark_img, x, y)
            } else {
                pick(brightest, &bright_img, x, y)
            };
        });
    Ok(HdrMerged {
        image: ImageF32 {
            width: w,
            height: h,
            data: acc,
        },
        reference: r,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::align::tests::{texture, warp};

    #[test]
    fn merges_bracket_into_wide_range() {
        // Szene mit großem Kontrastumfang: links sehr hell (bis 6), rechts dunkel
        let tex = texture(600, 400, 3);
        let scene = ImageF32::from_fn(600, 400, |x, y| {
            let t = tex.get(x, y);
            let g = 0.02 * (300.0f32).powf(x as f32 / 599.0); // 0,02 … 6
            t.map(|v| v * g * 1.5)
        });
        let expose = |img: &ImageF32, k: f32| ImageF32 {
            data: img
                .data
                .iter()
                .map(|p| p.map(|v| (v * k).min(1.0)))
                .collect(),
            ..img.clone()
        };
        // Drei Belichtungen, die dunkle leicht verschoben (Freihand)
        let shift: Mat3 = [[1.0, 0.0, 3.0], [0.0, 1.0, -2.0], [0.0, 0.0, 1.0]];
        let imgs = [
            warp(&expose(&scene, 0.25), &shift, 600, 400),
            expose(&scene, 1.0),
            expose(&scene, 4.0),
        ];
        let cancel = AtomicBool::new(false);
        let out = merge(
            3,
            &|i| Ok(imgs[i].clone()),
            HdrOptions::default(),
            &|_, _, _| {},
            &cancel,
        )
        .unwrap();
        assert_eq!(out.reference, 1, "Referenz ist die mittlere Belichtung");
        let out = out.image;
        // Hellster Bereich: in der Referenz (×1) ausgefressen, im HDR erhalten
        let (x, y) = (560..595)
            .map(|x| (x, 200))
            .max_by(|a, b| luma(scene.get(a.0, a.1)).total_cmp(&luma(scene.get(b.0, b.1))))
            .unwrap();
        let truth = scene.get(x, y);
        let got = out.get(x, y);
        assert!(luma(truth) > 1.5, "Testaufbau: Lichter über 1");
        assert!(
            (luma(got) / luma(truth) - 1.0).abs() < 0.1,
            "Lichter: {got:?} statt {truth:?}"
        );
        // Dunkler Bereich: aus der langen Belichtung, korrekt skaliert
        let (x, y) = (15, 200);
        let (truth, got) = (scene.get(x, y), out.get(x, y));
        assert!(
            (luma(got) / luma(truth) - 1.0).abs() < 0.1,
            "Tiefen: {got:?} statt {truth:?}"
        );
    }

    /// Weite Reihe: 5 Aufnahmen im Abstand von 3 EV (±6 EV). Die äußersten
    /// Bilder haben mit der Referenz keine gemeinsam gut belichteten Stellen –
    /// nur mit ihren Nachbarinnen.
    #[test]
    fn merges_wide_bracket_by_chaining() {
        let tex = texture(600, 400, 9);
        // 0,0005 … 30: über 16 Blendenstufen
        let scene = ImageF32::from_fn(600, 400, |x, y| {
            let g = 0.0005 * (60000.0f32).powf(x as f32 / 599.0);
            tex.get(x, y).map(|v| v * g * 1.5)
        });
        let expose = |k: f32| ImageF32 {
            data: scene
                .data
                .iter()
                .map(|p| p.map(|v| (v * k).min(1.0)))
                .collect(),
            ..scene.clone()
        };
        // Bewusst ungeordnet
        let ks = [8.0f32, 1.0 / 64.0, 1.0, 64.0, 1.0 / 8.0];
        let imgs: Vec<ImageF32> = ks.iter().map(|&k| expose(k)).collect();
        let cancel = AtomicBool::new(false);
        let opts = HdrOptions {
            align: false,
            deghost: true,
        };
        let out = merge(5, &|i| Ok(imgs[i].clone()), opts, &|_, _, _| {}, &cancel).unwrap();
        // Referenz ist die mittlere Belichtung (×1, hier Nr. 2 der Eingabe)
        assert_eq!(out.reference, 2);
        let out = out.image;
        // Überall im Maßstab der Szene
        for x in [40usize, 150, 300, 450, 560] {
            let (mut a, mut b) = (0.0, 0.0);
            for y in 150..250 {
                a += luma(out.get(x, y));
                b += luma(scene.get(x, y));
            }
            assert!((a / b - 1.0).abs() < 0.05, "x = {x}: {a} statt {b}");
        }
    }
}
