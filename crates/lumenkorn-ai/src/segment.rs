//! KI-Masken: Motiv (IS-Net), Objekt per Klick (Segment Anything) und Himmel.
//!
//! Alle Masken entstehen als Graustufenbild im Seitenverhältnis des Fotos
//! (längste Kante höchstens [`MASK_EDGE`]) und werden über normierte
//! Koordinaten in jeder Auflösung genutzt.

use crate::accel::Model;
use crate::error::{Error, Result};
use image::imageops::FilterType;
use image::{DynamicImage, GrayImage, Luma, RgbImage};
use lumenkorn_develop::{i18n::tr_s, tr};
use ort::value::Tensor;
use std::collections::VecDeque;
use std::path::Path;

/// Längste Kante gespeicherter Masken.
pub const MASK_EDGE: u32 = 1024;

fn open_session(path: &Path) -> Result<Model> {
    if !crate::runtime::is_loaded() {
        return Err(Error::RuntimeMissing);
    }
    if !path.is_file() {
        return Err(Error::ModelMissing(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        ));
    }
    crate::accel::session(path)
}

fn mask_size(w: u32, h: u32) -> (u32, u32) {
    let s = (MASK_EDGE as f32 / w.max(h) as f32).min(1.0);
    (
        ((w as f32 * s).round() as u32).max(1),
        ((h as f32 * s).round() as u32).max(1),
    )
}

// --- Motiv ----------------------------------------------------------------

/// Motiv-Erkennung mit IS-Net (DIS, „isnet-general-use“): 1024×1024 RGB rein,
/// Wahrscheinlichkeitskarte raus.
pub struct SubjectModel {
    session: Model,
    input: String,
    output: String,
}

impl SubjectModel {
    pub fn load(path: &Path) -> Result<Self> {
        let mut session = open_session(path)?;
        let s = session.inspect();
        let input = s
            .inputs()
            .first()
            .map(|i| i.name().to_string())
            .ok_or_else(|| Error::ModelFormat(tr_s("kein Eingang", "no input").into()))?;
        let output = s
            .outputs()
            .first()
            .map(|o| o.name().to_string())
            .ok_or_else(|| Error::ModelFormat(tr_s("kein Ausgang", "no output").into()))?;
        Ok(Self {
            session,
            input,
            output,
        })
    }

    pub fn predict(&self, img: &DynamicImage) -> Result<GrayImage> {
        const S: u32 = 1024;
        let rgb = img.resize_exact(S, S, FilterType::Lanczos3).to_rgb8();
        // Vorverarbeitung wie im Referenzcode: durch das Maximum teilen, −0,5.
        let max = rgb.as_raw().iter().copied().max().unwrap_or(255).max(1) as f32;
        let n = (S * S) as usize;
        let mut data = vec![0f32; 3 * n];
        for (i, p) in rgb.pixels().enumerate() {
            for c in 0..3 {
                data[c * n + i] = p[c] as f32 / max - 0.5;
            }
        }
        let input = Tensor::from_array(([1usize, 3, S as usize, S as usize], data))?;
        let mut session = self.session.lock();
        let outputs = session.run(vec![(self.input.as_str(), input.into_dyn())])?;
        let (_, pred) = outputs[self.output.as_str()].try_extract_tensor::<f32>()?;
        let (mi, ma) = pred
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        let range = (ma - mi).max(1e-6);
        let raw = GrayImage::from_fn(S, S, |x, y| {
            Luma([(((pred[(y * S + x) as usize] - mi) / range) * 255.0) as u8])
        });
        let (w, h) = mask_size(img.width(), img.height());
        Ok(image::imageops::resize(&raw, w, h, FilterType::Triangle))
    }
}

// --- Segment Anything -----------------------------------------------------

/// Bild-Embedding von SAM. Teuer (Sekunden) – einmal pro Bild berechnen und für
/// alle Klicks wiederverwenden.
pub struct SamEmbedding {
    data: Vec<f32>,
    /// Bildgröße, für die das Embedding berechnet wurde
    width: u32,
    height: u32,
    /// Skalierung auf die 1024er-Eingabe
    scale: f32,
}

pub struct Sam {
    encoder: Model,
    decoder: Model,
}

#[derive(Debug, Clone, Copy)]
pub struct Point {
    /// normiert 0…1
    pub x: f32,
    pub y: f32,
    pub positive: bool,
}

impl Sam {
    pub fn load(encoder: &Path, decoder: &Path) -> Result<Self> {
        Ok(Self {
            encoder: open_session(encoder)?,
            decoder: open_session(decoder)?,
        })
    }

    pub fn embed(&self, img: &DynamicImage) -> Result<SamEmbedding> {
        const S: u32 = 1024;
        let (w, h) = mask_size(img.width(), img.height());
        let scale = S as f32 / w.max(h) as f32;
        let (nw, nh) = (
            ((w as f32 * scale).round() as u32).min(S),
            ((h as f32 * scale).round() as u32).min(S),
        );
        let rgb = img.resize_exact(nw, nh, FilterType::Triangle).to_rgb8();
        const MEAN: [f32; 3] = [123.675, 116.28, 103.53];
        const STD: [f32; 3] = [58.395, 57.12, 57.375];
        let n = (S * S) as usize;
        // Rechts und unten mit Nullen auffüllen (entspricht dem Mittelwert nach Normierung).
        let mut data = vec![0f32; 3 * n];
        for (x, y, p) in rgb.enumerate_pixels() {
            let i = (y * S + x) as usize;
            for c in 0..3 {
                data[c * n + i] = (p[c] as f32 - MEAN[c]) / STD[c];
            }
        }
        let input = Tensor::from_array(([1usize, 3, S as usize, S as usize], data))?;
        let mut enc = self.encoder.lock();
        let name = enc.inputs()[0].name().to_string();
        let out_name = enc.outputs()[0].name().to_string();
        let outputs = enc.run(vec![(name.as_str(), input.into_dyn())])?;
        let (_, emb) = outputs[out_name.as_str()].try_extract_tensor::<f32>()?;
        Ok(SamEmbedding {
            data: emb.to_vec(),
            width: w,
            height: h,
            scale,
        })
    }

    /// Maske für die Klickpunkte. Liefert die Maske mit der höchsten vom Modell
    /// geschätzten Qualität.
    pub fn predict(&self, emb: &SamEmbedding, points: &[Point]) -> Result<GrayImage> {
        if points.is_empty() {
            return Err(Error::ModelFormat(tr_s("keine Punkte", "no points").into()));
        }
        let mut coords = Vec::with_capacity((points.len() + 1) * 2);
        let mut labels = Vec::with_capacity(points.len() + 1);
        for p in points {
            coords.push(p.x * emb.width as f32 * emb.scale);
            coords.push(p.y * emb.height as f32 * emb.scale);
            labels.push(if p.positive { 1.0f32 } else { 0.0 });
        }
        // Füllpunkt, wie es der ONNX-Export ohne Rahmen erwartet
        coords.extend([0.0, 0.0]);
        labels.push(-1.0);
        let n = labels.len();

        let inputs = vec![
            (
                "image_embeddings",
                Tensor::from_array(([1usize, 256, 64, 64], emb.data.clone()))?.into_dyn(),
            ),
            (
                "point_coords",
                Tensor::from_array(([1usize, n, 2], coords))?.into_dyn(),
            ),
            (
                "point_labels",
                Tensor::from_array(([1usize, n], labels))?.into_dyn(),
            ),
            (
                "mask_input",
                Tensor::from_array(([1usize, 1, 256, 256], vec![0f32; 256 * 256]))?.into_dyn(),
            ),
            (
                "has_mask_input",
                Tensor::from_array(([1usize], vec![0f32]))?.into_dyn(),
            ),
            (
                "orig_im_size",
                Tensor::from_array(([2usize], vec![emb.height as f32, emb.width as f32]))?
                    .into_dyn(),
            ),
        ];
        let mut dec = self.decoder.lock();
        let outputs = dec.run(inputs)?;
        let (shape, masks) = outputs["masks"].try_extract_tensor::<f32>()?;
        let (_, iou) = outputs["iou_predictions"].try_extract_tensor::<f32>()?;
        let count = shape.get(1).copied().unwrap_or(1).max(1) as usize;
        let best = (0..count.min(iou.len()))
            .max_by(|a, b| iou[*a].total_cmp(&iou[*b]))
            .unwrap_or(0);
        let (w, h) = (emb.width, emb.height);
        let plane = (w * h) as usize;
        if masks.len() < (best + 1) * plane {
            return Err(Error::ModelFormat(tr!(
                "unerwartete Maskenform {shape:?}",
                "unexpected mask shape {shape:?}"
            )));
        }
        let m = &masks[best * plane..(best + 1) * plane];
        // Logits → weiche, aber schmale Kante um 0; Reste weit außerhalb auf 0
        let mut out = GrayImage::from_fn(w, h, |x, y| {
            let v = m[(y * w + x) as usize];
            let a = 1.0 / (1.0 + (-v * 4.0).exp());
            Luma([if a < 0.03 { 0 } else { (a * 255.0) as u8 }])
        });
        fix_border(&mut out);
        Ok(out)
    }
}

// --- Himmel ---------------------------------------------------------------

/// Himmel finden: Bereichswachstum von hellen, glatten Flächen am oberen Rand,
/// eingeschlossene Löcher (Sonne, Mond) werden gefüllt. Steht SAM zur Verfügung,
/// verfeinert es die Kante – die Heuristik liefert ihm die Klickpunkte.
pub fn sky_mask(img: &DynamicImage, sam: Option<(&Sam, &SamEmbedding)>) -> Result<GrayImage> {
    const WORK: u32 = 256;
    let small = img.resize(WORK, WORK, FilterType::Triangle).to_rgb8();
    let region = sky_region(&small);
    let (sw, sh) = small.dimensions();
    let area = region.iter().filter(|v| **v).count() as f32 / (sw * sh) as f32;
    let (w, h) = mask_size(img.width(), img.height());
    if area < 0.02 {
        return Ok(GrayImage::new(w, h));
    }

    let heuristic = GrayImage::from_fn(sw, sh, |x, y| {
        Luma([if region[(y * sw + x) as usize] {
            255
        } else {
            0
        }])
    });

    if let Some((sam, emb)) = sam {
        let points = sky_prompts(&region, sw, sh);
        if !points.is_empty()
            && let Ok(m) = sam.predict(emb, &points)
        {
            let refined = image::imageops::resize(&m, sw, sh, FilterType::Triangle);
            let iou = iou(&refined, &heuristic);
            log::info!("Himmel: SAM-Übereinstimmung mit Heuristik {iou:.2}");
            if iou > 0.6 {
                // SAM verfeinert nur die Kante: höchstens ein schmaler Saum über
                // die geprüfte Fläche hinaus (sonst nimmt es Horizont, Inseln,
                // Wasser oder Boden mit), innen bleibt die Fläche voll – auch
                // gefüllte Löcher wie die Sonne.
                let (uw, uh) = (sw as usize, sh as usize);
                let soft = |b: &[bool]| {
                    let g = GrayImage::from_fn(sw, sh, |x, y| {
                        Luma([if b[(y * sw + x) as usize] { 255 } else { 0 }])
                    });
                    image::imageops::resize(
                        &image::imageops::blur(&g, 1.0),
                        w,
                        h,
                        FilterType::Triangle,
                    )
                };
                let upper = soft(&morph(&region, uw, uh, SAM_EDGE, true));
                let lower = soft(&morph(&region, uw, uh, SAM_EDGE, false));
                let mut out = image::imageops::resize(&m, w, h, FilterType::Triangle);
                for ((o, u), l) in out.pixels_mut().zip(upper.pixels()).zip(lower.pixels()) {
                    o[0] = o[0].min(u[0]).max(l[0]);
                }
                return Ok(out);
            }
        }
    }
    // Leicht weichzeichnen, damit die Kante nicht treppig wirkt.
    let soft = image::imageops::blur(&heuristic, 1.0);
    Ok(image::imageops::resize(&soft, w, h, FilterType::Triangle))
}

/// Art eines himmelsartigen Pixels.
#[derive(Clone, Copy, PartialEq)]
enum SkyKind {
    /// Kein Himmel (Grün, Erdtöne, kräftiges Rot, zu dunkel …)
    No,
    /// Blau bis Cyan – auch dunkles Dämmerungsblau
    Blue,
    /// Helles Grau/Weiß (Wolken, bedeckter Himmel) oder sehr helle warme Töne
    /// (Sonnenuntergang)
    Bright,
}

/// Himmelsfarbe eines Pixels (sRGB 0…1, Helligkeit `y`).
fn sky_kind(c: [f32; 3], y: f32) -> SkyKind {
    let (r, g, b) = (c[0], c[1], c[2]);
    let max = r.max(g).max(b);
    let chroma = max - r.min(g).min(b);
    // Blau/Cyan: Blau vorn und Rot klar dahinter
    if b >= g - 0.02 && b > r + 0.04 && y > 0.06 {
        return SkyKind::Blue;
    }
    // Wolken, Dunst, bedeckter Himmel: hell und fast farblos
    if y > 0.5 && chroma < 0.12 {
        return SkyKind::Bright;
    }
    // Abendhimmel: sehr hell auch kräftiger warm, sonst nur dezent getönt
    // (Wände, Haut, Sand scheiden dann über Glätte und Lage der Fläche aus)
    if g > b && ((y > 0.7 && chroma < 0.45) || (y > 0.45 && chroma < 0.28 && chroma < max * 0.4)) {
        return SkyKind::Bright;
    }
    SkyKind::No
}

/// Himmel finden (Arbeitsbild ~256 px), ohne KI-Modell.
///
/// 1. Jedes Pixel der Fläche muss eine Himmelsfarbe haben ([`sky_kind`]) und
///    glatt sein; gewachsen wird vom oberen Rand aus über ähnliche Nachbarn.
///    (Früher genügte ein langsamer Farbverlauf – so lief die Fläche über
///    Wände, Gras und Studiohintergründe.)
/// 2. Jede zusammenhängende Fläche muss als Ganzes überzeugen: blaue Flächen
///    einheitlich blau, helle graue nur, wenn sie zu den hellsten Teilen des
///    Bildes gehören und sehr glatt sind – das schließt Wände und graue
///    Hintergründe aus, die sonst genauso aussehen.
/// 3. Kleine eingeschlossene Löcher (Sonne, Mond, Vögel) werden gefüllt.
fn sky_region(img: &RgbImage) -> Vec<bool> {
    let (w, h) = img.dimensions();
    let (w, h) = (w as usize, h as usize);
    let n = w * h;
    let px: Vec<[f32; 3]> = img
        .pixels()
        .map(|p| {
            [
                p[0] as f32 / 255.0,
                p[1] as f32 / 255.0,
                p[2] as f32 / 255.0,
            ]
        })
        .collect();
    let lum: Vec<f32> = px
        .iter()
        .map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
        .collect();
    let at = |x: usize, y: usize, dx: isize, dy: isize| {
        let xx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
        let yy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
        lum[yy * w + xx]
    };
    // Kantenstärke: Gradient über zwei Pixel (fängt auch feine Struktur wie
    // Laub, Putz oder Stoff, die ein Pixelabstand bei Verkleinerung verwischt)
    let grad: Vec<f32> = (0..n)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let g1 =
                (at(x, y, 1, 0) - at(x, y, -1, 0)).abs() + (at(x, y, 0, 1) - at(x, y, 0, -1)).abs();
            let g2 =
                (at(x, y, 2, 0) - at(x, y, -2, 0)).abs() + (at(x, y, 0, 2) - at(x, y, 0, -2)).abs();
            g1.max(g2 * 0.6)
        })
        .collect();
    let kind: Vec<SkyKind> = (0..n).map(|i| sky_kind(px[i], lum[i])).collect();
    // Glatt genug für Himmel: blauer Himmel ist sehr glatt, Wolken etwas weniger
    let smooth = |i: usize| match kind[i] {
        SkyKind::Blue => grad[i] < 0.07,
        SkyKind::Bright => grad[i] < 0.09,
        SkyKind::No => false,
    };

    // Bereichswachstum über ähnliche Nachbarn. Wolken mit Kanten zum blauen
    // Himmel werden dabei zu eigenen Flächen; sie kommen im zweiten Schritt
    // unten dazu (dort mit Prüfung, dass sie nicht unter dem Himmel liegen –
    // ein Sprung beim Wachsen nahm sonst hellen Fels oder Schnee neben dem
    // Himmel mit und lief darin nach unten).
    let similar = |a: usize, b: usize| {
        let (c, d) = (px[a], px[b]);
        ((d[0] - c[0]).powi(2) + (d[1] - c[1]).powi(2) + (d[2] - c[2]).powi(2)).sqrt() < 0.05
    };
    let band = (h / 12).max(1);
    let mut label = vec![u32::MAX; n];
    let mut parent: Vec<u32> = Vec::new();
    fn root(parent: &mut [u32], mut a: u32) -> u32 {
        while parent[a as usize] != a {
            parent[a as usize] = parent[parent[a as usize] as usize];
            a = parent[a as usize];
        }
        a
    }
    // Startpunkte im oberen Bildteil – auch Himmel, der nicht bis zur Kante
    // reicht (Streifen unter Wolken), wird so zur Fläche; ob er dazugehört,
    // entscheidet die Prüfung unten.
    for y in 0..h * 3 / 5 {
        for x in 0..w {
            let start = y * w + x;
            if label[start] != u32::MAX || !smooth(start) {
                continue;
            }
            let id = parent.len() as u32;
            parent.push(id);
            label[start] = id;
            let mut queue = VecDeque::from([start]);
            while let Some(i) = queue.pop_front() {
                let (cx, cy) = (i % w, i / w);
                for (dx, dy) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (cx as isize + dx, cy as isize + dy);
                    if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if !smooth(j) || !similar(i, j) {
                        continue;
                    }
                    if label[j] == u32::MAX {
                        label[j] = id;
                        queue.push_back(j);
                    } else {
                        let (a, b) = (root(&mut parent, id), root(&mut parent, label[j]));
                        if a != b {
                            parent[b as usize] = a;
                        }
                    }
                }
            }
        }
    }
    // Geordnet, damit das Ergebnis nicht von der Reihenfolge einer HashMap abhängt
    let mut comps: std::collections::BTreeMap<u32, Vec<usize>> = Default::default();
    for (i, &l) in label.iter().enumerate() {
        if l != u32::MAX {
            let r = root(&mut parent, l);
            comps.entry(r).or_default().push(i);
        }
    }

    // Flächen als Ganzes prüfen
    let mut sorted = lum.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let pct = |q: f32| sorted[((sorted.len() - 1) as f32 * q) as usize];
    let (bright_ref, median) = (pct(0.85), pct(0.5));
    let mut region = vec![false; n];
    // Flächen, die nicht bis zur Kante reichen oder abgelehnt wurden, für den
    // zweiten Schritt (siehe unten)
    struct Later<'a> {
        members: &'a Vec<usize>,
        blue: f32,
        /// B−G über der nötigen Schwelle (positiv = klar blau)
        blue_margin: f32,
        mean_lum: f32,
        mean_grad: f32,
    }
    let mut later: Vec<Later> = Vec::new();
    for members in comps.values() {
        let m = members.len();
        // Winzige Flecken (Lichtreflex, Fensterscheibe) sind kein Himmel
        if m < n / 400 {
            continue;
        }
        // Breite am oberen Rand: Spalten, in denen die Fläche oben ankommt
        let mut cols = vec![false; w];
        for &i in members.iter().filter(|&&i| i / w < band) {
            cols[i % w] = true;
        }
        let top_width = cols.iter().filter(|c| **c).count();
        let mean_lum = members.iter().map(|&i| lum[i]).sum::<f32>() / m as f32;
        let mean_grad = members.iter().map(|&i| grad[i]).sum::<f32>() / m as f32;
        let blues: Vec<usize> = members
            .iter()
            .copied()
            .filter(|&i| kind[i] == SkyKind::Blue)
            .collect();
        let blue = blues.len() as f32 / m as f32;
        // Wie viel blauer als grün (nur die blauen Pixel): an Testfotos Himmel
        // ≥ 0,032 (auch mit Wolken, Dunst, Dämmerung), türkises Laub ≤ 0,025
        let (over_green, blue_lum) = if blues.is_empty() {
            (0.0, 0.0)
        } else {
            let k = blues.len() as f32;
            (
                blues.iter().map(|&i| px[i][2] - px[i][1]).sum::<f32>() / k,
                blues.iter().map(|&i| lum[i]).sum::<f32>() / k,
            )
        };
        // Sehr heller Himmel ist fast weiß – dort kann Blau nur wenig vor Grün
        // liegen; die Schwelle sinkt daher zum Weiß hin
        let need_blue = 0.028 * (2.5 * (1.0 - blue_lum)).min(1.0);
        // Unterkante: tiefste Zeile je Spalte. Beim Himmel ist das der Horizont
        // (eher waagerecht), bei Hintergründen hinter Personen die Silhouette
        // (in der Mitte hoch, an den Seiten tief) – Streuung relativ zur Höhe.
        let mut bottom = vec![None::<usize>; w];
        for &i in members {
            let (x, y) = (i % w, i / w);
            bottom[x] = Some(bottom[x].map_or(y, |b: usize| b.max(y)));
        }
        let bs: Vec<f32> = bottom
            .iter()
            .flatten()
            .map(|&b| b as f32 / h as f32)
            .collect();
        let bmean = bs.iter().sum::<f32>() / bs.len().max(1) as f32;
        let ragged =
            (bs.iter().map(|b| (b - bmean).powi(2)).sum::<f32>() / bs.len().max(1) as f32).sqrt();
        // Anteil in der unteren Bildhälfte: Himmel liegt oben, Wände und
        // Studiohintergründe reichen meist bis weit nach unten
        let low = members.iter().filter(|&&i| i / w > h / 2).count() as f32 / m as f32;
        let reaches_top = m >= n / 100 && top_width >= w / 10;
        let ok = reaches_top
            && if blue > 0.15 {
                // Blauer Himmel (auch mit Wolken): klar blau und glatt
                over_green > need_blue && mean_grad < 0.045
            } else {
                // Grau/Weiß: muss zu den hellsten Teilen gehören. Dazu entweder
                // sehr glatt (gleichmäßig bedeckt; Wände, Putz und Studio-
                // hintergründe hatten an Testfotos Kantenwerte 0,013–0,026,
                // bedeckter Himmel unter 0,01) und oben breit anliegend – oder,
                // mit Wolkenstruktur, groß, oben breit und kaum in der unteren Hälfte.
                let bright = mean_lum >= bright_ref * 0.9 && mean_lum > median + 0.05;
                let even = mean_grad < 0.012 && m >= n / 25 && top_width >= w / 3 && low < 0.35;
                let clouds = mean_grad < 0.045
                    && m >= n / 12
                    && top_width * 5 >= w * 2
                    && low < 0.1
                    && ragged < 0.14;
                bright && (even || clouds)
            };
        // Größere Flächen ins Log: zeigt bei Problemfotos, woran es lag
        if m >= n / 50 {
            log::info!(
                "Himmel-Fläche: {:.1} % des Bildes, oben {:.0} % der Breite, Helligkeit {mean_lum:.2} \
                 (85 %-Wert {bright_ref:.2}, Median {median:.2}), Kante {mean_grad:.3}, blau {blue:.2}, \
                 B−G {over_green:.3} (nötig {need_blue:.3}), unten {low:.2}, Zerklüftung {ragged:.3} → {}",
                m as f32 / n as f32 * 100.0,
                top_width as f32 / w as f32 * 100.0,
                if ok { "Himmel" } else { "nein" }
            );
        }
        if ok {
            for &i in members {
                region[i] = true;
            }
        } else if low < 0.1 {
            later.push(Later {
                members,
                blue,
                blue_margin: over_green - need_blue,
                mean_lum,
                mean_grad,
            });
        }
    }
    // Zweiter Schritt: Wolken mit rauem Rand und Himmelsstreifen, die nicht
    // bis zur Kante reichen, werden beim Wachsen nicht verbunden. Grenzt eine
    // solche Fläche oben im Bild direkt an erkannten Himmel und ist kaum
    // dunkler als er, gehört sie dazu. Wasser unter dem Horizont ist deutlich
    // dunkler als der Himmel darüber und bleibt draußen.
    for _ in 0..3 {
        let sky: Vec<usize> = (0..n).filter(|&i| region[i]).collect();
        if sky.is_empty() {
            break;
        }
        let sky_lum = sky.iter().map(|&i| lum[i]).sum::<f32>() / sky.len() as f32;
        let near = morph(&region, w, h, 2, true);
        // Tiefste Himmelszeile je Spalte: Was überwiegend darunter liegt, ist
        // Vordergrund (Fels, Schneefeld, Gebäude), keine Wolke
        let mut sky_bottom = vec![None::<usize>; w];
        for &i in &sky {
            let (x, y) = (i % w, i / w);
            sky_bottom[x] = Some(sky_bottom[x].map_or(y, |b: usize| b.max(y)));
        }
        let mut added = false;
        for c in &later {
            let members = c.members;
            if region[members[0]] || !members.iter().any(|&i| near[i]) {
                continue;
            }
            let fits = if c.blue > 0.15 {
                c.blue_margin > 0.0 && c.mean_grad < 0.045
            } else {
                let under = members
                    .iter()
                    .filter(|&&i| sky_bottom[i % w].is_some_and(|b| i / w > b))
                    .count();
                c.mean_grad < 0.06 && under * 2 < members.len()
            };
            if fits && c.mean_lum >= sky_lum - 0.15 {
                for &i in members.iter() {
                    region[i] = true;
                }
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    if !region.iter().any(|v| *v) {
        return region;
    }
    fill_holes(&region, w, h, n / 60)
}

/// Löcher füllen: Was nicht zur Fläche gehört und keinen Weg zum seitlichen
/// oder unteren Rand hat, liegt im Himmel (Sonne, Mond, Vögel) – aber nur bis
/// `max_hole` Pixel je Loch; größere sind Gebäude, Bäume oder Berge.
fn fill_holes(region: &[bool], w: usize, h: usize, max_hole: usize) -> Vec<bool> {
    let mut outside = vec![false; w * h];
    let mut q = VecDeque::new();
    for x in 0..w {
        q.push_back((x, h - 1));
    }
    for y in 0..h {
        q.push_back((0, y));
        q.push_back((w - 1, y));
    }
    while let Some((x, y)) = q.pop_front() {
        let i = y * w + x;
        if region[i] || outside[i] {
            continue;
        }
        outside[i] = true;
        if x > 0 {
            q.push_back((x - 1, y));
        }
        if x + 1 < w {
            q.push_back((x + 1, y));
        }
        if y > 0 {
            q.push_back((x, y - 1));
        }
        if y + 1 < h {
            q.push_back((x, y + 1));
        }
    }
    // Eingeschlossene Bereiche je Loch zählen, nur kleine übernehmen
    let mut out = region.to_vec();
    let mut seen = vec![false; w * h];
    for start in 0..w * h {
        if region[start] || outside[start] || seen[start] {
            continue;
        }
        let mut hole = vec![start];
        seen[start] = true;
        let mut k = 0;
        while k < hole.len() {
            let i = hole[k];
            k += 1;
            let (x, y) = (i % w, i / w);
            let mut push = |j: usize| {
                if !region[j] && !outside[j] && !seen[j] {
                    seen[j] = true;
                    hole.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < w {
                push(i + 1);
            }
            if y > 0 {
                push(i - w);
            }
            if y + 1 < h {
                push(i + w);
            }
        }
        if hole.len() <= max_hole {
            for i in hole {
                out[i] = true;
            }
        }
    }
    out
}

/// SAM unterschätzt Masken in den äußersten Pixelreihen; ohne Korrektur bleibt
/// am Bildrand eine helle Linie stehen. Die Randpixel übernehmen den Wert von
/// drei Pixeln weiter innen, falls der größer ist.
fn fix_border(m: &mut GrayImage) {
    let (w, h) = m.dimensions();
    if w < 8 || h < 8 {
        return;
    }
    for x in 0..w {
        for (edge, inner) in [
            (0, 3),
            (1, 3),
            (2, 3),
            (h - 1, h - 4),
            (h - 2, h - 4),
            (h - 3, h - 4),
        ] {
            let v = m.get_pixel(x, inner)[0].max(m.get_pixel(x, edge)[0]);
            m.put_pixel(x, edge, Luma([v]));
        }
    }
    for y in 0..h {
        for (edge, inner) in [
            (0, 3),
            (1, 3),
            (2, 3),
            (w - 1, w - 4),
            (w - 2, w - 4),
            (w - 3, w - 4),
        ] {
            let v = m.get_pixel(inner, y)[0].max(m.get_pixel(edge, y)[0]);
            m.put_pixel(edge, y, Luma([v]));
        }
    }
}

/// Klickpunkte für SAM: je Bilddrittel ein Punkt im Himmel, dazu Gegenpunkte unten.
fn sky_prompts(region: &[bool], w: u32, h: u32) -> Vec<Point> {
    let (w, h) = (w as usize, h as usize);
    let mut pts = Vec::new();
    for band in 0..3 {
        let x = w * (2 * band + 1) / 6;
        // höchster zusammenhängender Himmelspunkt in dieser Spalte, etwas nach unten versetzt
        let col: Vec<usize> = (0..h).filter(|y| region[y * w + x]).collect();
        if col.len() > h / 20 {
            let y = col[col.len() / 3];
            pts.push(Point {
                x: x as f32 / w as f32,
                y: y as f32 / h as f32,
                positive: true,
            });
        }
    }
    for x in [w / 3, 2 * w / 3] {
        if let Some(y) = (h * 3 / 4..h).rev().find(|y| !region[y * w + x]) {
            pts.push(Point {
                x: x as f32 / w as f32,
                y: y as f32 / h as f32,
                positive: false,
            });
        }
    }
    if pts.iter().any(|p| p.positive) {
        pts
    } else {
        vec![]
    }
}

/// Breite des Saums (Pixel im Arbeitsbild, ~1 % der Kante), in dem SAM die
/// Himmelskante verschieben darf.
const SAM_EDGE: usize = 3;

/// Binäres Dilatieren (`grow`) bzw. Erodieren mit quadratischem Radius `r`.
fn morph(b: &[bool], w: usize, h: usize, r: usize, grow: bool) -> Vec<bool> {
    // Getrennt waagerecht und senkrecht – bei Quadrat gleichwertig
    let pass = |src: &[bool], horizontal: bool| -> Vec<bool> {
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let (lo, hi, at): (usize, usize, Box<dyn Fn(usize) -> usize>) = if horizontal {
                    (
                        x.saturating_sub(r),
                        (x + r).min(w - 1),
                        Box::new(move |k| y * w + k),
                    )
                } else {
                    (
                        y.saturating_sub(r),
                        (y + r).min(h - 1),
                        Box::new(move |k| k * w + x),
                    )
                };
                let mut it = (lo..=hi).map(|k| src[at(k)]);
                if grow { it.any(|v| v) } else { it.all(|v| v) }
            })
            .collect()
    };
    pass(&pass(b, true), false)
}

fn iou(a: &GrayImage, b: &GrayImage) -> f32 {
    let (mut inter, mut union) = (0u32, 0u32);
    for (p, q) in a.pixels().zip(b.pixels()) {
        let (x, y) = (p[0] > 127, q[0] > 127);
        inter += (x && y) as u32;
        union += (x || y) as u32;
    }
    if union == 0 {
        0.0
    } else {
        inter as f32 / union as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    /// Himmel mit Verlauf und Sonne über einer dunklen Bergkette.
    fn landscape() -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(600, 400, |x, y| {
            let ridge = 220.0 + 40.0 * ((x as f32) / 60.0).sin();
            let sun = ((x as f32 - 450.0).powi(2) + (y as f32 - 80.0).powi(2)).sqrt() < 25.0;
            if (y as f32) > ridge {
                Rgb([25, 22, 30])
            } else if sun {
                Rgb([250, 240, 210])
            } else {
                let t = y as f32 / ridge;
                Rgb([
                    (110.0 + 60.0 * t) as u8,
                    (150.0 + 40.0 * t) as u8,
                    (220.0 - 20.0 * t) as u8,
                ])
            }
        }))
    }

    #[test]
    fn finds_sky_including_sun_and_excludes_mountains() {
        let m = sky_mask(&landscape(), None).unwrap();
        let (w, h) = m.dimensions();
        let at = |x: f32, y: f32| m.get_pixel((x * w as f32) as u32, (y * h as f32) as u32)[0];
        assert!(at(0.2, 0.1) > 200, "Himmel oben links");
        assert!(at(0.75, 0.2) > 200, "Sonne gehört zum Himmel");
        assert!(at(0.5, 0.9) < 30, "Berge unten nicht");
    }

    /// Leichtes Rauschen wie bei echten Fotos (in Blöcken, damit es das
    /// Verkleinern auf das Arbeitsbild übersteht).
    fn noise(x: u32, y: u32, amp: i32) -> i32 {
        (((x / 4) * 7919 + (y / 4) * 104729) % 23) as i32 * amp / 11 - amp
    }

    #[test]
    fn grey_studio_backdrop_is_not_sky() {
        // Mittelgrauer, leicht fleckiger Hintergrund hinter einer hellen Person
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(600, 400, |x, y| {
            let person =
                ((x as f32 - 300.0) / 120.0).powi(2) + ((y as f32 - 330.0) / 220.0).powi(2) < 1.0;
            if person {
                Rgb([235, 200, 180])
            } else {
                let v = (150 + noise(x, y, 14)) as u8;
                Rgb([v, v, v.saturating_add(4)])
            }
        }));
        let m = sky_mask(&img, None).unwrap();
        assert!(m.pixels().all(|p| p[0] == 0));
    }

    #[test]
    fn teal_bokeh_is_not_sky() {
        // Unscharfes, türkises Laub hinter einer hellen Blüte
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(600, 400, |x, y| {
            let bloom = ((x as f32 - 300.0).powi(2) + (y as f32 - 220.0).powi(2)).sqrt() < 110.0;
            if bloom {
                Rgb([245, 170, 110])
            } else {
                let n = noise(x, y, 4);
                Rgb([(40 + n) as u8, (95 + n) as u8, (100 + n) as u8])
            }
        }));
        let m = sky_mask(&img, None).unwrap();
        assert!(m.pixels().all(|p| p[0] == 0));
    }

    #[test]
    fn dusk_sky_is_found() {
        // Dunkelblauer Dämmerungshimmel über schwarzer Silhouette mit Lichtern
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(600, 400, |x, y| {
            if y > 300 {
                if (x / 40) % 3 == 0 && y < 310 {
                    Rgb([255, 210, 120])
                } else {
                    Rgb([10, 10, 14])
                }
            } else {
                let t = y as f32 / 300.0;
                Rgb([
                    (15.0 + 20.0 * t) as u8,
                    (35.0 + 30.0 * t) as u8,
                    (90.0 + 40.0 * t) as u8,
                ])
            }
        }));
        let m = sky_mask(&img, None).unwrap();
        let (w, h) = m.dimensions();
        assert!(m.get_pixel(w / 2, h / 4)[0] > 200, "Himmel");
        assert!(m.get_pixel(w / 2, h * 9 / 10)[0] < 30, "Boden");
    }

    /// Blauer Himmel mit weißen Quellwolken über dunklem Land, rechts ein
    /// heller Fels, der in den Himmel ragt.
    fn clouds_and_rock() -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(600, 400, |x, y| {
            let (fx, fy) = (x as f32, y as f32);
            let rock = ((fx - 480.0) / 70.0).powi(2) + ((fy - 230.0) / 90.0).powi(2) < 1.0;
            if rock {
                let v = (205 + noise(x, y, 6)) as u8;
                return Rgb([v, v.saturating_sub(8), v.saturating_sub(20)]);
            }
            if y > 200 {
                return Rgb([
                    (60 + noise(x, y, 10)) as u8,
                    (55 + noise(x, y, 10)) as u8,
                    40,
                ]);
            }
            let t = fy / 200.0;
            let sky = [100.0 + 70.0 * t, 145.0 + 45.0 * t, 225.0 - 15.0 * t];
            // Wolken mit weichem Rand
            let c = |cx: f32, cy: f32, rx: f32, ry: f32| {
                let d = ((fx - cx) / rx).powi(2) + ((fy - cy) / ry).powi(2);
                (1.2 - d).clamp(0.0, 1.0)
            };
            let k = c(120.0, 60.0, 90.0, 35.0)
                .max(c(330.0, 110.0, 110.0, 40.0))
                .max(c(520.0, 40.0, 70.0, 30.0));
            let px = sky.map(|v| v + (240.0 - v) * k);
            Rgb(px.map(|v| v as u8))
        }))
    }

    #[test]
    fn clouds_belong_to_the_sky_but_the_rock_does_not() {
        let m = sky_mask(&clouds_and_rock(), None).unwrap();
        let (w, h) = m.dimensions();
        let at = |x: f32, y: f32| {
            m.get_pixel((x / 600.0 * w as f32) as u32, (y / 400.0 * h as f32) as u32)[0]
        };
        assert!(at(30.0, 100.0) > 200, "blauer Himmel");
        assert!(at(120.0, 60.0) > 200, "Wolke links");
        assert!(at(330.0, 110.0) > 200, "Wolke mitte");
        assert!(at(480.0, 230.0) < 30, "Fels");
        assert!(at(200.0, 330.0) < 30, "Land");
    }

    #[test]
    fn no_sky_in_a_closeup() {
        // Grob strukturierte Oberfläche (z. B. Holz, Stoff) in Blöcken, damit die
        // Struktur auch nach dem Verkleinern erhalten bleibt.
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(600, 400, |x, y| {
            let n = ((x / 8) * 7919 + (y / 8) * 104729) % 97;
            Rgb([(90 + n) as u8, (60 + n / 2) as u8, 40])
        }));
        let m = sky_mask(&img, None).unwrap();
        assert!(m.pixels().all(|p| p[0] == 0));
    }
}
