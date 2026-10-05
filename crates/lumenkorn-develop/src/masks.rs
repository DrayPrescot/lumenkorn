//! Masken auswerten: aus den Formen einer Maskenebene wird pro Pixel eine
//! Deckkraft 0…1 berechnet.

use crate::color::smoothstep;
use crate::image_f32::ImageF32;
use crate::params::{ColorSample, MaskLayer, MaskShape, Stroke};
use rayon::prelude::*;
use std::sync::{Arc, Mutex};

/// Graustufenmaske eines Modells, in beliebiger Auflösung. Wird über normierte
/// Koordinaten abgetastet und passt damit zu Proxy wie Vollbild.
#[derive(Debug, Clone)]
pub struct GrayMask {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl GrayMask {
    #[inline]
    pub fn sample(&self, nx: f32, ny: f32) -> f32 {
        let fx = (nx * self.width as f32 - 0.5).clamp(0.0, (self.width - 1) as f32);
        let fy = (ny * self.height as f32 - 0.5).clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: usize, y: usize| self.data[y * self.width + x] as f32 / 255.0;
        let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
        let bot = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
        top + (bot - top) * ty
    }

    pub fn from_image(img: &image::GrayImage) -> Self {
        Self {
            width: img.width() as usize,
            height: img.height() as usize,
            data: img.as_raw().clone(),
        }
    }
}

/// Liefert gespeicherte Modellmasken anhand ihrer ID.
pub trait MaskSource: Sync {
    fn get(&self, mask_id: &str) -> Option<Arc<GrayMask>>;
}

/// Keine Modellmasken verfügbar (z. B. in Tests).
pub struct NoMasks;

impl MaskSource for NoMasks {
    fn get(&self, _: &str) -> Option<Arc<GrayMask>> {
        None
    }
}

type CacheEntry = (String, usize, usize, Arc<Vec<f32>>);
static CACHE: Mutex<Vec<CacheEntry>> = Mutex::new(Vec::new());

/// Verwirft alle zwischengespeicherten Masken.
pub(crate) fn release_cache() {
    CACHE.lock().unwrap().clear();
}

/// Nur Masken in voller Auflösung verwerfen (nach einem Export).
pub(crate) fn release_large() {
    CACHE.lock().unwrap().retain(|e| e.3.len() <= 12 << 20);
}

/// Wie [`evaluate`], aber zwischengespeichert: Solange sich die Formen einer
/// Ebene nicht ändern (etwa beim Ziehen ihrer Regler), wird die Maske nicht
/// neu berechnet. Für CPU und GPU gleichermaßen.
pub fn evaluate_cached(
    layer: &MaskLayer,
    w: usize,
    h: usize,
    store: &dyn MaskSource,
    image: Option<&ImageF32>,
) -> Arc<Vec<f32>> {
    const ENTRIES: usize = 8;

    // Fehlt eine Modellmaske (noch), nicht merken – sie kann gleich da sein.
    let complete = layer.shapes.iter().all(|s| match s {
        MaskShape::Raster { mask_id, .. } => store.get(mask_id).is_some(),
        _ => true,
    });
    let mut key = serde_json::to_string(&layer.shapes).unwrap_or_default();
    // Farbmasken hängen vom Bildinhalt ab: das Bild gehört zum Schlüssel
    if layer
        .shapes
        .iter()
        .any(|s| matches!(s, MaskShape::Color { .. }))
    {
        key.push_str(&format!("@{:x}", image.map_or(0, fingerprint)));
    }
    if complete {
        let mut c = CACHE.lock().unwrap();
        if let Some(pos) = c
            .iter()
            .position(|(k, cw, ch, _)| *cw == w && *ch == h && *k == key)
        {
            let e = c.remove(pos);
            let a = e.3.clone();
            c.insert(0, e);
            return a;
        }
    }
    let a = Arc::new(evaluate(layer, w, h, store, image));
    if complete {
        let mut c = CACHE.lock().unwrap();
        c.insert(0, (key, w, h, a.clone()));
        c.truncate(ENTRIES);
        // Volle Auflösung (Zoom) belegt viel Speicher: ~256 MB behalten, aber
        // mindestens drei Masken der größten Art – sonst verdrängen sich zwei
        // Ebenen in der 45-MP-Zoomansicht bei jedem Rendern gegenseitig.
        let largest = c.iter().map(|e| e.3.len() * 4).max().unwrap_or(0);
        let max_bytes = (256usize << 20).max(3 * largest);
        while c.len() > 1 && c.iter().map(|e| e.3.len() * 4).sum::<usize>() > max_bytes {
            c.pop();
        }
    }
    a
}

/// Deckkraft der Maskenebene für ein Bild der Größe `w`×`h` (Originalausrichtung).
/// `image` ist das lineare Quellbild derselben Größe – nur Farbmasken brauchen es.
pub fn evaluate(
    layer: &MaskLayer,
    w: usize,
    h: usize,
    store: &dyn MaskSource,
    image: Option<&ImageF32>,
) -> Vec<f32> {
    let mut alpha = vec![0f32; w * h];
    for shape in &layer.shapes {
        let (a, subtract) = shape_alpha(shape, w, h, store, image);
        alpha.par_iter_mut().zip(a.par_iter()).for_each(|(t, a)| {
            *t = if subtract { *t * (1.0 - a) } else { t.max(*a) };
        });
    }
    alpha
}

fn shape_alpha(
    shape: &MaskShape,
    w: usize,
    h: usize,
    store: &dyn MaskSource,
    image: Option<&ImageF32>,
) -> (Vec<f32>, bool) {
    let long = w.max(h) as f32;
    let per_pixel = |f: &(dyn Fn(f32, f32) -> f32 + Sync)| -> Vec<f32> {
        (0..w * h)
            .into_par_iter()
            .map(|i| f((i % w) as f32 + 0.5, (i / w) as f32 + 0.5))
            .collect()
    };
    match shape {
        MaskShape::Linear {
            x0,
            y0,
            x1,
            y1,
            invert,
            subtract,
        } => {
            // In Pixeln rechnen, damit der Verlauf bei jedem Seitenverhältnis senkrecht steht.
            let (ax, ay) = (x0 * w as f32, y0 * h as f32);
            let (dx, dy) = (x1 * w as f32 - ax, y1 * h as f32 - ay);
            let len2 = (dx * dx + dy * dy).max(1e-6);
            let inv = *invert;
            let a = per_pixel(&|px, py| {
                let t = ((px - ax) * dx + (py - ay) * dy) / len2;
                let v = 1.0 - smoothstep(0.0, 1.0, t);
                if inv { 1.0 - v } else { v }
            });
            (a, *subtract)
        }
        MaskShape::Radial {
            cx,
            cy,
            rx,
            ry,
            angle,
            feather,
            invert,
            subtract,
        } => {
            let (cxp, cyp) = (cx * w as f32, cy * h as f32);
            let (rxp, ryp) = ((rx * long).max(0.5), (ry * long).max(0.5));
            let (s, c) = angle.to_radians().sin_cos();
            let inner = (1.0 - feather.max(0.01)).max(0.0);
            let inv = *invert;
            let a = per_pixel(&|px, py| {
                let (dx, dy) = (px - cxp, py - cyp);
                let u = (dx * c + dy * s) / rxp;
                let v = (-dx * s + dy * c) / ryp;
                let r = (u * u + v * v).sqrt();
                let m = 1.0 - smoothstep(inner, 1.0, r);
                if inv { 1.0 - m } else { m }
            });
            (a, *subtract)
        }
        MaskShape::Brush { strokes, subtract } => (paint(strokes, w, h), *subtract),
        MaskShape::Color {
            samples,
            range,
            invert,
            subtract,
        } => {
            let a = match image.filter(|i| i.width == w && i.height == h) {
                Some(img) => color_alpha(img, samples, *range, *invert),
                None => {
                    log::warn!("Farbmaske ohne passendes Quellbild – Ebene wirkt nicht");
                    vec![0.0; w * h]
                }
            };
            (a, *subtract)
        }
        MaskShape::Raster {
            mask_id,
            invert,
            subtract,
            ..
        } => {
            let a = match store.get(mask_id) {
                Some(m) => {
                    let inv = *invert;
                    let (fw, fh) = (w as f32, h as f32);
                    per_pixel(&|px, py| {
                        let v = m.sample(px / fw, py / fh);
                        if inv { 1.0 - v } else { v }
                    })
                }
                None => {
                    log::warn!("Maske {mask_id} fehlt – Ebene wirkt nicht");
                    vec![0.0; w * h]
                }
            };
            (a, *subtract)
        }
    }
}

// --- Farbmasken ----------------------------------------------------------------

pub use crate::color::oklab;

/// Farbe an der Stelle (x, y) (normiert), gemittelt über eine kleine Fläche,
/// damit Rauschen und einzelne Pixel nicht zählen.
pub fn sample_color(img: &ImageF32, x: f32, y: f32) -> [f32; 3] {
    let (w, h) = (img.width, img.height);
    let r = (w.max(h) / 400).max(2) as isize;
    let cx = (x.clamp(0.0, 1.0) * w as f32) as isize;
    let cy = (y.clamp(0.0, 1.0) * h as f32) as isize;
    let mut sum = [0f32; 3];
    let mut n = 0f32;
    for yy in (cy - r).max(0)..=(cy + r).min(h as isize - 1) {
        for xx in (cx - r).max(0)..=(cx + r).min(w as isize - 1) {
            let p = img.data[yy as usize * w + xx as usize];
            for k in 0..3 {
                sum[k] += p[k];
            }
            n += 1.0;
        }
    }
    oklab(sum.map(|v| v / n.max(1.0)))
}

/// Farbart und Helligkeit aus Oklab. In Oklab skalieren L, a und b bei
/// Belichtungsänderung gemeinsam (Kubikwurzel) – a/L und b/L bleiben gleich.
/// So gehört die Schattenseite derselben Farbe dazu. Der kleine Zuschlag
/// beruhigt fast schwarze Pixel, deren Farbe nur noch Rauschen ist.
fn chroma(lab: [f32; 3]) -> [f32; 3] {
    let k = 1.0 / (lab[0].max(0.0) + 0.05);
    [lab[0], lab[1] * k, lab[2] * k]
}

/// Deckkraft nach Farbähnlichkeit: 1 nahe einer gewählten Farbe, weich
/// auslaufend bis zur Toleranz. Verglichen wird vor allem die Farbart;
/// Helligkeit zählt nur schwach.
fn color_alpha(img: &ImageF32, samples: &[ColorSample], range: f32, invert: bool) -> Vec<f32> {
    let outer = 0.03 + range.clamp(0.0, 100.0) / 100.0 * 0.35;
    let inner = outer * 0.5;
    let refs: Vec<[f32; 3]> = samples.iter().map(|s| chroma(s.lab)).collect();
    let raw: Vec<f32> = img
        .data
        .par_iter()
        .map(|p| {
            let c = chroma(oklab(*p));
            let d = refs
                .iter()
                .map(|s| {
                    let dl = (c[0] - s[0]) * 0.25;
                    let (da, db) = (c[1] - s[1], c[2] - s[2]);
                    dl * dl + da * da + db * db
                })
                .fold(f32::MAX, f32::min)
                .sqrt();
            let v = 1.0 - smoothstep(inner, outer, d);
            if invert { 1.0 - v } else { v }
        })
        .collect();
    // Leicht glätten: Bildrauschen ergäbe sonst gesprenkelte Ränder
    let radius = (img.width.max(img.height) / 1000).max(1);
    box_blur(&raw, img.width, img.height, radius)
}

/// Trennbarer Kastenfilter (zwei Durchgänge) mit gleitender Summe.
fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let pass = |data: &[f32], w: usize, h: usize| -> Vec<f32> {
        // Zeilenweise; Ergebnis transponiert, damit der zweite Durchgang
        // wieder zeilenweise (cachefreundlich) laufen kann.
        let rows: Vec<Vec<f32>> = (0..h)
            .into_par_iter()
            .map(|y| {
                let row = &data[y * w..(y + 1) * w];
                let mut out = vec![0f32; w];
                let mut acc: f32 = row[..(r + 1).min(w)].iter().sum();
                let mut n = (r + 1).min(w) as f32;
                for x in 0..w {
                    out[x] = acc / n;
                    if x + r + 1 < w {
                        acc += row[x + r + 1];
                        n += 1.0;
                    }
                    if x >= r {
                        acc -= row[x - r];
                        n -= 1.0;
                    }
                }
                out
            })
            .collect();
        // Transponieren parallel in Spaltenblöcken (je 64 Spalten: zeilenweise
        // lesen, im Block schreiben) – einfädig kostete das bei 45 MP am meisten
        const B: usize = 64;
        let mut t = vec![0f32; w * h];
        t.par_chunks_mut(B * h).enumerate().for_each(|(blk, out)| {
            let x0 = blk * B;
            let cols = out.len() / h;
            for (y, row) in rows.iter().enumerate() {
                for (dx, v) in row[x0..x0 + cols].iter().enumerate() {
                    out[dx * h + y] = *v;
                }
            }
        });
        t
    };
    let t = pass(src, w, h);
    pass(&t, h, w)
}

/// Billiger Fingerabdruck eines Bildes für Cache-Schlüssel: Größe plus
/// 256 Stichproben – unterscheidet Fotos zuverlässig, ohne alle Pixel zu lesen.
fn fingerprint(img: &ImageF32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (img.width, img.height).hash(&mut h);
    let n = img.data.len();
    for i in 0..256usize {
        if let Some(p) = img.data.get(i.wrapping_mul(2_654_435_761) % n.max(1)) {
            p.map(f32::to_bits).hash(&mut h);
        }
    }
    h.finish()
}

/// Malt Pinselstriche. Jeder Strich wird nur in seinem Begrenzungsrahmen berechnet,
/// und darin jedes Segment nur in seinem eigenen Rahmen (Segment ± Radius):
/// Ein diagonaler Strich deckt mit seinem Rahmen fast das ganze Bild ab – alle
/// Segmente für jedes Pixel darin zu prüfen kostete bei 2 MP und 200 Segmenten
/// rund eine Sekunde pro Rendern.
///
/// Je Zeile wird der kleinste Abstand zu allen Segmenten gesammelt, deren Rahmen
/// die Zeile berührt; außerhalb seines Rahmens ist ein Segment mindestens `r`
/// entfernt und hätte (wegen `d >= r`) ohnehin nichts beigetragen. Das Minimum
/// ist exakt – das Ergebnis ist bitgleich mit der Prüfung aller Segmente.
fn paint(strokes: &[Stroke], w: usize, h: usize) -> Vec<f32> {
    let long = w.max(h) as f32;
    let mut alpha = vec![0f32; w * h];
    for s in strokes {
        let r = (s.radius * long).max(0.5);
        let inner = r * (1.0 - s.feather.clamp(0.0, 1.0));
        let pts: Vec<(f32, f32)> = s
            .points
            .iter()
            .map(|p| (p[0] * w as f32, p[1] * h as f32))
            .collect();
        if pts.is_empty() {
            continue;
        }
        let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in &pts {
            minx = minx.min(*x);
            miny = miny.min(*y);
            maxx = maxx.max(*x);
            maxy = maxy.max(*y);
        }
        let x0 = ((minx - r).floor().max(0.0)) as usize;
        let y0 = ((miny - r).floor().max(0.0)) as usize;
        let x1 = ((maxx + r).ceil() as usize).min(w);
        let y1 = ((maxy + r).ceil() as usize).min(h);
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        let segs: Vec<((f32, f32), (f32, f32))> = if pts.len() == 1 {
            vec![(pts[0], pts[0])]
        } else {
            pts.windows(2).map(|p| (p[0], p[1])).collect()
        };
        // Rahmen je Segment in Pixelindizes, auf den Strichrahmen begrenzt. Ein
        // Pixel Reserve, damit Rundung in `dist_to_segment` nie einen Treffer
        // am Rand abschneidet. Nicht endliche Punkte ergäben NaN-Abstände, die
        // das Minimum ohnehin ignoriert – solche Segmente entfallen.
        let boxes: Vec<(usize, usize, usize, usize)> = segs
            .iter()
            .map(|(a, b)| {
                let fin = [a.0, a.1, b.0, b.1].iter().all(|v| v.is_finite());
                if !fin {
                    return (0, 0, 0, 0);
                }
                let bx0 = ((a.0.min(b.0) - r).floor() - 1.0).max(x0 as f32) as usize;
                let by0 = ((a.1.min(b.1) - r).floor() - 1.0).max(y0 as f32) as usize;
                let bx1 = (((a.0.max(b.0) + r).ceil() + 1.0).max(0.0) as usize).min(x1);
                let by1 = (((a.1.max(b.1) + r).ceil() + 1.0).max(0.0) as usize).min(y1);
                (bx0, by0, bx1, by1)
            })
            .collect();
        alpha[y0 * w..y1 * w]
            .par_chunks_mut(w)
            .enumerate()
            .for_each_init(
                || vec![f32::MAX; x1 - x0],
                |dmin, (row, line)| {
                    let y = y0 + row;
                    let py = y as f32 + 0.5;
                    dmin.fill(f32::MAX);
                    let mut any = false;
                    for ((a, b), &(bx0, by0, bx1, by1)) in segs.iter().zip(&boxes) {
                        if y < by0 || y >= by1 || bx0 >= bx1 {
                            continue;
                        }
                        any = true;
                        for (x, d) in (bx0..bx1).zip(&mut dmin[bx0 - x0..bx1 - x0]) {
                            *d = d.min(dist_to_segment(x as f32 + 0.5, py, *a, *b));
                        }
                    }
                    if !any {
                        return;
                    }
                    for (out, &d) in line[x0..x1].iter_mut().zip(dmin.iter()) {
                        if d >= r {
                            continue;
                        }
                        let a = (1.0 - smoothstep(inner, r, d)) * s.opacity;
                        *out = if s.erase {
                            *out * (1.0 - a)
                        } else {
                            out.max(a)
                        };
                    }
                },
            );
    }
    alpha
}

#[inline]
fn dist_to_segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= 1e-9 {
        0.0
    } else {
        (((px - a.0) * dx + (py - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    let (qx, qy) = (a.0 + t * dx, a.1 + t * dy);
    ((px - qx).powi(2) + (py - qy).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::MaskLayer;

    fn layer(shapes: Vec<MaskShape>) -> MaskLayer {
        MaskLayer {
            shapes,
            ..Default::default()
        }
    }

    #[test]
    fn linear_gradient_goes_from_one_to_zero() {
        let l = layer(vec![MaskShape::Linear {
            x0: 0.5,
            y0: 0.0,
            x1: 0.5,
            y1: 0.5,
            invert: false,
            subtract: false,
        }]);
        let a = evaluate(&l, 100, 100, &NoMasks, None);
        assert!(a[0] > 0.99, "oben voll");
        assert!(a[60 * 100 + 50] < 0.01, "unter dem Ende aus");
        assert!((a[25 * 100 + 50] - 0.5).abs() < 0.05, "Mitte halb");
    }

    #[test]
    fn radial_invert_and_subtract() {
        let r = MaskShape::Radial {
            cx: 0.5,
            cy: 0.5,
            rx: 0.2,
            ry: 0.2,
            angle: 0.0,
            feather: 0.2,
            invert: false,
            subtract: false,
        };
        let a = evaluate(&layer(vec![r.clone()]), 100, 100, &NoMasks, None);
        assert!(a[50 * 100 + 50] > 0.99 && a[0] < 0.01);

        // Alles außer der Ellipse: Fläche minus Ellipse
        let full = MaskShape::Linear {
            x0: 0.0,
            y0: -10.0,
            x1: 0.0,
            y1: -9.0,
            invert: true,
            subtract: false,
        };
        let sub = match r {
            MaskShape::Radial {
                cx,
                cy,
                rx,
                ry,
                angle,
                feather,
                ..
            } => MaskShape::Radial {
                cx,
                cy,
                rx,
                ry,
                angle,
                feather,
                invert: false,
                subtract: true,
            },
            _ => unreachable!(),
        };
        let b = evaluate(&layer(vec![full, sub]), 100, 100, &NoMasks, None);
        assert!(b[50 * 100 + 50] < 0.01 && b[0] > 0.99);
    }

    /// Bisherige Fassung von `paint` (jedes Pixel im Strichrahmen gegen alle
    /// Segmente) als Referenz für die schnelle.
    fn paint_reference(strokes: &[Stroke], w: usize, h: usize) -> Vec<f32> {
        let long = w.max(h) as f32;
        let mut alpha = vec![0f32; w * h];
        for s in strokes {
            let r = (s.radius * long).max(0.5);
            let inner = r * (1.0 - s.feather.clamp(0.0, 1.0));
            let pts: Vec<(f32, f32)> = s
                .points
                .iter()
                .map(|p| (p[0] * w as f32, p[1] * h as f32))
                .collect();
            if pts.is_empty() {
                continue;
            }
            let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for (x, y) in &pts {
                minx = minx.min(*x);
                miny = miny.min(*y);
                maxx = maxx.max(*x);
                maxy = maxy.max(*y);
            }
            let x0 = ((minx - r).floor().max(0.0)) as usize;
            let y0 = ((miny - r).floor().max(0.0)) as usize;
            let x1 = ((maxx + r).ceil() as usize).min(w);
            let y1 = ((maxy + r).ceil() as usize).min(h);
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            let segs: Vec<((f32, f32), (f32, f32))> = if pts.len() == 1 {
                vec![(pts[0], pts[0])]
            } else {
                pts.windows(2).map(|p| (p[0], p[1])).collect()
            };
            for y in y0..y1 {
                let py = y as f32 + 0.5;
                for x in x0..x1 {
                    let px = x as f32 + 0.5;
                    let d = segs
                        .iter()
                        .map(|(a, b)| dist_to_segment(px, py, *a, *b))
                        .fold(f32::MAX, f32::min);
                    if d >= r {
                        continue;
                    }
                    let a = (1.0 - smoothstep(inner, r, d)) * s.opacity;
                    let out = &mut alpha[y * w + x];
                    *out = if s.erase {
                        *out * (1.0 - a)
                    } else {
                        out.max(a)
                    };
                }
            }
        }
        alpha
    }

    #[test]
    fn fast_brush_matches_reference_exactly() {
        // Einfacher Zufallsgenerator (LCG) – reproduzierbar ohne Abhängigkeit
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = move || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 40) as f32 / (1u64 << 24) as f32
        };
        for (case, (w, h)) in [(97, 61), (200, 133), (64, 256), (300, 300)]
            .into_iter()
            .enumerate()
        {
            let strokes: Vec<Stroke> = (0..6 + case)
                .map(|i| {
                    let n = 1 + (rnd() * 40.0) as usize;
                    // Punkte teils außerhalb des Bildes (Strich am Rand)
                    let (mut x, mut y) = (rnd() * 1.2 - 0.1, rnd() * 1.2 - 0.1);
                    let points = (0..n)
                        .map(|_| {
                            x += (rnd() - 0.5) * 0.15;
                            y += (rnd() - 0.5) * 0.15;
                            [x, y]
                        })
                        .collect();
                    Stroke {
                        points,
                        radius: 0.002 + rnd() * 0.12,
                        feather: [0.0, 1.0, rnd()][i % 3],
                        opacity: [1.0, rnd()][i % 2],
                        erase: i % 4 == 3,
                    }
                })
                .collect();
            let a = paint(&strokes, w, h);
            let b = paint_reference(&strokes, w, h);
            assert!(a.iter().any(|v| *v > 0.0), "Testaufbau: es wird gemalt");
            for (i, (x, y)) in a.iter().zip(&b).enumerate() {
                assert_eq!(
                    x.to_bits(),
                    y.to_bits(),
                    "{w}×{h}, Pixel {i}: {x} statt {y}"
                );
            }
        }
    }

    #[test]
    fn brush_paints_and_erases() {
        let paint = Stroke {
            points: vec![[0.1, 0.5], [0.9, 0.5]],
            radius: 0.05,
            feather: 0.0,
            opacity: 1.0,
            erase: false,
        };
        let erase = Stroke {
            points: vec![[0.5, 0.5]],
            radius: 0.1,
            feather: 0.0,
            opacity: 1.0,
            erase: true,
        };
        let a = evaluate(
            &layer(vec![MaskShape::Brush {
                strokes: vec![paint, erase],
                subtract: false,
            }]),
            200,
            100,
            &NoMasks,
            None,
        );
        assert!(a[50 * 200 + 40] > 0.99, "Strich links");
        assert!(a[50 * 200 + 100] < 0.01, "radiert in der Mitte");
        assert!(a[10 * 200 + 40] < 0.01, "neben dem Strich");
    }

    #[test]
    fn raster_masks_are_resolution_independent() {
        struct One(Arc<GrayMask>);
        impl MaskSource for One {
            fn get(&self, _: &str) -> Option<Arc<GrayMask>> {
                Some(self.0.clone())
            }
        }
        // 4×2-Maske: linke Hälfte an
        let m = GrayMask {
            width: 4,
            height: 2,
            data: vec![255, 255, 0, 0, 255, 255, 0, 0],
        };
        let store = One(Arc::new(m));
        let shape = MaskShape::Raster {
            mask_id: "x".into(),
            kind: "subject".into(),
            prompt: vec![],
            invert: false,
            subtract: false,
        };
        for (w, h) in [(40, 20), (400, 200)] {
            let a = evaluate(&layer(vec![shape.clone()]), w, h, &store, None);
            assert!(a[h / 2 * w + w / 10] > 0.99 && a[h / 2 * w + w * 9 / 10] < 0.01);
        }
    }

    #[test]
    fn color_mask_selects_similar_colors() {
        // Links Rot (oben hell, unten im Schatten), rechts Blau
        let img = ImageF32::from_fn(200, 100, |x, y| {
            let shade = if y < 50 { 1.0 } else { 0.45 };
            if x < 100 {
                [0.8 * shade, 0.1 * shade, 0.08 * shade]
            } else {
                [0.05, 0.15, 0.7]
            }
        });
        let red = ColorSample {
            x: 0.25,
            y: 0.25,
            lab: sample_color(&img, 0.25, 0.25),
        };
        let shape = |range: f32, invert: bool| MaskShape::Color {
            samples: vec![red],
            range,
            invert,
            subtract: false,
        };
        let a = evaluate(
            &layer(vec![shape(30.0, false)]),
            200,
            100,
            &NoMasks,
            Some(&img),
        );
        let at = |a: &[f32], x: usize, y: usize| a[y * 200 + x];
        assert!(at(&a, 50, 25) > 0.99, "gewählte Farbe");
        assert!(at(&a, 50, 75) > 0.5, "dieselbe Farbe im Schatten");
        assert!(at(&a, 150, 25) < 0.01, "andere Farbe");

        // Kleine Toleranz: der Schatten fällt heraus
        let tight = evaluate(
            &layer(vec![shape(0.0, false)]),
            200,
            100,
            &NoMasks,
            Some(&img),
        );
        assert!(at(&tight, 50, 75) < at(&a, 50, 75));

        let inv = evaluate(
            &layer(vec![shape(30.0, true)]),
            200,
            100,
            &NoMasks,
            Some(&img),
        );
        assert!(at(&inv, 150, 25) > 0.99 && at(&inv, 50, 25) < 0.01);

        // Ohne Bild wirkt sie nicht (statt abzustürzen)
        let none = evaluate(&layer(vec![shape(30.0, false)]), 200, 100, &NoMasks, None);
        assert!(none.iter().all(|v| *v == 0.0));

        // Anderes Bild gleicher Größe: nicht aus dem Cache des ersten
        let l = layer(vec![shape(30.0, false)]);
        let c1 = evaluate_cached(&l, 200, 100, &NoMasks, Some(&img));
        let blue = ImageF32::from_fn(200, 100, |_, _| [0.05, 0.15, 0.7]);
        let c2 = evaluate_cached(&l, 200, 100, &NoMasks, Some(&blue));
        assert!(c1[25 * 200 + 50] > 0.99 && c2[25 * 200 + 50] < 0.01);
    }
}
