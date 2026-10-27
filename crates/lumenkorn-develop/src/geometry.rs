//! Geometrie: Orientierung, Drehung, Spiegelung, Geraderichten, Perspektive,
//! Zuschnitt.

use crate::image_f32::ImageF32;
use crate::params::{Crop, DevelopParams};
use rayon::prelude::*;

/// Wendet eine EXIF-Orientierung (1–8) an, sodass das Bild aufrecht steht.
pub fn apply_exif_orientation(img: ImageF32, orientation: u16) -> ImageF32 {
    match orientation {
        2 => flip_h(&img),
        3 => rotate_180(&img),
        4 => flip_v(&img),
        // Spiegeln und Drehen in einem Durchgang (statt zweier Vollbilder)
        5 => swap_axes(&img, |x, y| (y, x)),
        6 => rotate_90_cw(&img),
        7 => {
            let (w, h) = (img.width, img.height);
            swap_axes(&img, |x, y| (w - 1 - y, h - 1 - x))
        }
        8 => rotate_270_cw(&img),
        _ => img,
    }
}

pub fn rotate_quarters(img: ImageF32, quarters: u8) -> ImageF32 {
    match quarters % 4 {
        1 => rotate_90_cw(&img),
        2 => rotate_180(&img),
        3 => rotate_270_cw(&img),
        _ => img,
    }
}

pub fn rotate_90_cw(img: &ImageF32) -> ImageF32 {
    let h = img.height;
    // Ziel (x, y) ← Quelle (y, h-1-x)
    swap_axes(img, |x, y| (y, h - 1 - x))
}

pub fn rotate_270_cw(img: &ImageF32) -> ImageF32 {
    let w = img.width;
    swap_axes(img, |x, y| (w - 1 - y, x))
}

/// Bild mit vertauschten Achsen (h × w); `src(x, y)` nennt den Quellpixel
/// für den Zielpixel. In Kacheln: Eine Zielzeile liest eine Spalte der
/// Quelle – zeilenweise wäre bei 45 MP fast jeder Zugriff ein Cache-Fehlgriff.
fn swap_axes(img: &ImageF32, src: impl Fn(usize, usize) -> (usize, usize) + Sync) -> ImageF32 {
    const T: usize = 64;
    let (ow, oh) = (img.height, img.width);
    let mut data = vec![[0f32; 3]; ow * oh];
    data.par_chunks_mut((ow * T).max(1))
        .enumerate()
        .for_each(|(band, out)| {
            let rows = out.len() / ow;
            for x0 in (0..ow).step_by(T) {
                for dy in 0..rows {
                    let y = band * T + dy;
                    for x in x0..(x0 + T).min(ow) {
                        let (sx, sy) = src(x, y);
                        out[dy * ow + x] = img.get(sx, sy);
                    }
                }
            }
        });
    ImageF32 {
        width: ow,
        height: oh,
        data,
    }
}

pub fn rotate_180(img: &ImageF32) -> ImageF32 {
    let (w, h) = (img.width, img.height);
    ImageF32::from_fn(w, h, |x, y| img.get(w - 1 - x, h - 1 - y))
}

pub fn flip_h(img: &ImageF32) -> ImageF32 {
    let w = img.width;
    ImageF32::from_fn(w, img.height, |x, y| img.get(w - 1 - x, y))
}

pub fn flip_v(img: &ImageF32) -> ImageF32 {
    let h = img.height;
    ImageF32::from_fn(img.width, h, |x, y| img.get(x, h - 1 - y))
}

/// Vergrößerung, damit ein um `deg` gedrehtes Bild seinen Rahmen (w × h) ohne
/// leere Ecken füllt.
pub fn straighten_scale(w: f32, h: f32, deg: f32) -> f32 {
    let t = deg.to_radians().abs();
    let (s, c) = t.sin_cos();
    (c + (h / w) * s).max(c + (w / h) * s)
}

/// Stärkste Neigung der Bildebene bei Perspektive ±100
pub const PERSPECTIVE_MAX_DEG: f64 = 30.0;

type M3 = [[f64; 3]; 3];

fn mul(a: &M3, b: &M3) -> M3 {
    let mut r = [[0.0; 3]; 3];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

fn apply(m: &M3, x: f64, y: f64) -> Option<(f64, f64)> {
    let z = m[2][0] * x + m[2][1] * y + m[2][2];
    (z > 1e-9).then(|| {
        (
            (m[0][0] * x + m[0][1] * y + m[0][2]) / z,
            (m[1][0] * x + m[1][1] * y + m[1][2]) / z,
        )
    })
}

fn translate(x: f64, y: f64) -> M3 {
    [[1.0, 0.0, x], [0.0, 1.0, y], [0.0, 0.0, 1.0]]
}

/// Geraderichten und Perspektive als eine projektive Abbildung. `src/lib/frame.ts`
/// rechnet dasselbe für die Masken- und Zuschnittanzeige.
pub struct Warp {
    /// Ausgabe → Quelle in Pixelkoordinaten (Pixelmitte bei +0,5), zeilenweise:
    /// `[x, y, z] = m · [px, py, 1]`, Quelle = (x/z, y/z)
    pub m: [f32; 9],
    /// Mittlere Vergrößerung gegenüber der Quelle (Wurzel des Flächen-
    /// verhältnisses von Rahmen und abgebildetem Viereck)
    pub zoom: f32,
}

impl Warp {
    /// `None`, wenn nichts zu tun ist. `w`×`h` ist der gedrehte Rahmen.
    pub fn new(w: usize, h: usize, p: &DevelopParams) -> Option<Warp> {
        Warp::from_angles(
            w,
            h,
            p.straighten,
            p.perspective_vertical,
            p.perspective_horizontal,
        )
    }

    pub fn from_angles(
        w: usize,
        h: usize,
        straighten: f32,
        vertical: f32,
        horizontal: f32,
    ) -> Option<Warp> {
        if (straighten == 0.0 && vertical == 0.0 && horizontal == 0.0) || w == 0 || h == 0 {
            return None;
        }
        let (wf, hf) = (w as f64, h as f64);
        let (cx, cy) = (wf / 2.0, hf / 2.0);
        // Perspektive: Bildebene um die waagerechte bzw. senkrechte Achse
        // kippen, Brennweite = lange Seite (etwa ein 35-mm-Objektiv). Von der
        // Ausgabe zurück zur Quelle: K·Rᵀ·K⁻¹, so verschoben, dass die
        // Bildmitte an ihrem Platz bleibt.
        let a = (vertical as f64 / 100.0 * PERSPECTIVE_MAX_DEG).to_radians();
        let b = (horizontal as f64 / 100.0 * PERSPECTIVE_MAX_DEG).to_radians();
        let f = wf.max(hf);
        let (sa, ca) = a.sin_cos();
        let (sb, cb) = b.sin_cos();
        let rx = [[1.0, 0.0, 0.0], [0.0, ca, -sa], [0.0, sa, ca]];
        let ry = [[cb, 0.0, sb], [0.0, 1.0, 0.0], [-sb, 0.0, cb]];
        let r = mul(&ry, &rx);
        let rt = std::array::from_fn(|i| std::array::from_fn(|j| r[j][i]));
        let k = [[f, 0.0, 0.0], [0.0, f, 0.0], [0.0, 0.0, 1.0]];
        let k_inv = [[1.0 / f, 0.0, 0.0], [0.0, 1.0 / f, 0.0], [0.0, 0.0, 1.0]];
        let fwd = mul(&mul(&k, &r), &k_inv);
        let (c0x, c0y) = apply(&fwd, 0.0, 0.0)?;
        let persp = mul(&mul(&mul(&k, &rt), &k_inv), &translate(c0x, c0y));
        // Geraderichten (im Uhrzeigersinn positiv) wirkt nach der Perspektive
        let (s, c) = (straighten as f64).to_radians().sin_cos();
        let unrot = [[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0]];
        let base = mul(&persp, &unrot);
        let zoom = if vertical == 0.0 && horizontal == 0.0 {
            straighten_scale(wf as f32, hf as f32, straighten) as f64
        } else {
            fill_zoom(&base, cx, cy)
        };
        let scale = [
            [1.0 / zoom, 0.0, 0.0],
            [0.0, 1.0 / zoom, 0.0],
            [0.0, 0.0, 1.0],
        ];
        let m = mul(
            &mul(&mul(&translate(cx, cy), &base), &scale),
            &translate(-cx, -cy),
        );
        // Fläche des Quellvierecks (Gaußsche Trapezformel)
        let quad: Vec<(f64, f64)> = [(0.0, 0.0), (wf, 0.0), (wf, hf), (0.0, hf)]
            .iter()
            .map(|&(x, y)| apply(&m, x, y).unwrap_or((x, y)))
            .collect();
        let area = (0..4)
            .map(|i| {
                let (a, b) = (quad[i], quad[(i + 1) % 4]);
                a.0 * b.1 - b.0 * a.1
            })
            .sum::<f64>()
            .abs()
            / 2.0;
        let n = m[2][2];
        Some(Warp {
            m: std::array::from_fn(|i| (m[i / 3][i % 3] / n) as f32),
            zoom: (wf * hf / area.max(1e-9)).sqrt() as f32,
        })
    }
}

/// Kleinste Vergrößerung, bei der alle vier Ecken der Ausgabe im Quellbild
/// liegen. Projektive Abbildungen erhalten Geraden, der Rahmen wird also zu
/// einem Viereck, und es genügt, die Ecken zu prüfen.
fn fill_zoom(base: &M3, cx: f64, cy: f64) -> f64 {
    let inside = |k: f64| {
        [(-cx, -cy), (cx, -cy), (cx, cy), (-cx, cy)]
            .iter()
            .all(|&(x, y)| {
                apply(base, x / k, y / k).is_some_and(|(sx, sy)| {
                    sx.abs() <= cx * (1.0 + 1e-6) && sy.abs() <= cy * (1.0 + 1e-6)
                })
            })
    };
    let (mut lo, mut hi) = (0.25, 50.0);
    if inside(lo) {
        return lo;
    }
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if inside(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// Projektive Abbildung mit [`Warp::m`] anwenden. Bilineare Abtastung.
pub fn warp(img: &ImageF32, m: &[f32; 9]) -> ImageF32 {
    ImageF32::from_fn(img.width, img.height, |x, y| {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let z = m[6] * px + m[7] * py + m[8];
        let sx = (m[0] * px + m[1] * py + m[2]) / z - 0.5;
        let sy = (m[3] * px + m[4] * py + m[5]) / z - 0.5;
        bilinear(img, sx, sy)
    })
}

/// Wie viel größer ein Bild geladen werden muss, damit nach Geraderichten und
/// Perspektive (die beide vergrößern) noch die gewünschte Auflösung bleibt.
/// Die Bildmaße sind vorher unbekannt: das stärkere von Quer- und Hochformat.
pub fn load_margin(p: &DevelopParams) -> f32 {
    [(3, 2), (2, 3)]
        .iter()
        .filter_map(|&(w, h)| Warp::new(w * 1000, h * 1000, p))
        .map(|w| w.zoom)
        .fold(1.0, f32::max)
}

/// Dreht um die Bildmitte (im Uhrzeigersinn positiv) und vergrößert so, dass
/// der Rahmen voll bleibt.
pub fn straighten(img: &ImageF32, deg: f32) -> ImageF32 {
    match Warp::from_angles(img.width, img.height, deg, 0.0, 0.0) {
        Some(w) => warp(img, &w.m),
        None => img.clone(),
    }
}

#[inline]
fn bilinear(img: &ImageF32, x: f32, y: f32) -> [f32; 3] {
    let x = x.clamp(0.0, (img.width - 1) as f32);
    let y = y.clamp(0.0, (img.height - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(img.width - 1), (y0 + 1).min(img.height - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let (a, b, c, d) = (
        img.get(x0, y0),
        img.get(x1, y0),
        img.get(x0, y1),
        img.get(x1, y1),
    );
    let mut out = [0f32; 3];
    for k in 0..3 {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bot = c[k] + (d[k] - c[k]) * tx;
        out[k] = top + (bot - top) * ty;
    }
    out
}

/// Schätzt, um wie viel Grad das Bild gedreht werden muss, damit Horizont und
/// Kanten gerade stehen. `None`, wenn das Bild keine eindeutigen geraden Kanten hat.
///
/// Verfahren: Kantenpunkte werden für jeden Kandidatenwinkel auf eine gedrehte
/// Achse projiziert. Liegen viele Kantenpunkte auf geraden Linien, ergibt der
/// richtige Winkel besonders scharfe Spitzen in der Projektion. Waagerechte und
/// senkrechte Kanten stimmen gemeinsam ab – Horizonte ebenso wie Hauswände.
pub fn detect_tilt(img: &ImageF32) -> Option<f32> {
    let small = img.downscale_to(512);
    let (w, h) = (small.width, small.height);
    if w < 16 || h < 16 {
        return None;
    }
    let lum: Vec<f32> = small.data.iter().map(|p| crate::color::luma(*p)).collect();
    let at = |x: usize, y: usize| lum[y * w + x];

    // Kantenpunkte (Sobel), getrennt nach waagerechten und senkrechten Kanten
    let mut horiz = Vec::new();
    let mut vert = Vec::new();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let gx = (at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1))
                - (at(x - 1, y - 1) + 2.0 * at(x - 1, y) + at(x - 1, y + 1));
            let gy = (at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1))
                - (at(x - 1, y - 1) + 2.0 * at(x, y - 1) + at(x + 1, y - 1));
            let m = (gx * gx + gy * gy).sqrt();
            if m < 0.1 {
                continue;
            }
            // Nur Kanten, die höchstens ~25° von einer Achse abweichen
            if gy.abs() > gx.abs() * 2.1 {
                horiz.push((x as f32, y as f32, m));
            } else if gx.abs() > gy.abs() * 2.1 {
                vert.push((x as f32, y as f32, m));
            }
        }
    }
    if horiz.len() + vert.len() < 40 {
        return None;
    }

    let diag = ((w * w + h * h) as f32).sqrt();
    let bins = (2.0 * diag) as usize + 2;
    let score = |deg: f32| -> f32 {
        let (s, c) = deg.to_radians().sin_cos();
        let mut acc = vec![0f32; bins];
        let mut sum = 0.0;
        for &(x, y, m) in &horiz {
            let r = -x * s + y * c + diag;
            acc[r as usize] += m;
        }
        for &(x, y, m) in &vert {
            let r = x * c + y * s + diag;
            acc[r as usize] += m;
        }
        for v in &acc {
            sum += v * v;
        }
        sum
    };

    // Grob in 0,5°-Schritten, dann fein um das Maximum
    let coarse: Vec<(f32, f32)> = (-40..=40)
        .map(|i| i as f32 * 0.5)
        .map(|d| (d, score(d)))
        .collect();
    let (best_c, best_s) = coarse.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1))?;
    let mean = coarse.iter().map(|c| c.1).sum::<f32>() / coarse.len() as f32;
    if best_s < mean * 1.25 {
        return None;
    }
    let fine = (-10..=10)
        .map(|i| best_c + i as f32 * 0.05)
        .map(|d| (d, score(d)))
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    // Inhalt um +φ im Uhrzeigersinn geneigt → um −φ zurückdrehen
    let fix = -fine.0;
    (fix.abs() >= 0.05).then_some(fix).or(Some(0.0))
}

pub fn crop(img: &ImageF32, c: Crop) -> ImageF32 {
    let (x0, y0, w, h) = crop_rect(img.width, img.height, c);
    ImageF32::from_fn(w, h, |x, y| img.get(x0 + x, y0 + y))
}

/// Zuschnitt in Pixeln (x0, y0, Breite, Höhe) für ein Bild der Größe `width`×`height`.
pub fn crop_rect(width: usize, height: usize, c: Crop) -> (usize, usize, usize, usize) {
    let x0 = ((c.x * width as f32).round() as usize).min(width - 1);
    let y0 = ((c.y * height as f32).round() as usize).min(height - 1);
    let w = ((c.w * width as f32).round() as usize).clamp(1, width - x0);
    let h = ((c.h * height as f32).round() as usize).clamp(1, height - y0);
    (x0, y0, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(w: usize, h: usize) -> ImageF32 {
        ImageF32::from_fn(w, h, |x, y| [(y * w + x) as f32, 0.0, 0.0])
    }

    #[test]
    fn tiled_orientations_match_pixelwise_reference() {
        // Größer als eine Kachel und nicht durch 64 teilbar
        let img = numbered(150, 71);
        let (w, h) = (img.width, img.height);
        let reference = |o: u16| -> ImageF32 {
            let at = |x: usize, y: usize| match o {
                5 => img.get(y, x),
                6 => img.get(y, h - 1 - x),
                7 => img.get(w - 1 - y, h - 1 - x),
                _ => img.get(w - 1 - y, x),
            };
            ImageF32::from_fn(h, w, at)
        };
        for o in 5..=8 {
            let got = apply_exif_orientation(img.clone(), o);
            assert_eq!((got.width, got.height), (h, w));
            assert!(got.data == reference(o).data, "Orientierung {o}");
        }
        // Orientierung 5 = gespiegelt nach 90°, 7 = gespiegelt nach 270°
        let five = flip_h(&rotate_90_cw(&img));
        assert!(apply_exif_orientation(img.clone(), 5).data == five.data);
        let seven = flip_h(&rotate_270_cw(&img));
        assert!(apply_exif_orientation(img.clone(), 7).data == seven.data);
        let empty = ImageF32::from_fn(0, 0, |_, _| [0.0; 3]);
        assert_eq!(rotate_90_cw(&empty).data.len(), 0);
    }

    #[test]
    fn rotations_are_consistent() {
        let img = numbered(3, 2); // 0 1 2 / 3 4 5
        let r = rotate_90_cw(&img);
        assert_eq!((r.width, r.height), (2, 3));
        // Nach 90° im Uhrzeigersinn steht die linke untere Ecke (3) oben links.
        assert_eq!(r.get(0, 0)[0], 3.0);
        assert_eq!(r.get(1, 0)[0], 0.0);
        let back = rotate_270_cw(&r);
        assert_eq!(back.data, img.data);
        assert_eq!(rotate_180(&rotate_180(&img)).data, img.data);
        assert_eq!(apply_exif_orientation(img.clone(), 6).data, r.data);
    }

    #[test]
    fn straighten_fills_frame_and_rotates_clockwise() {
        // Linke Hälfte 0, rechte Hälfte 1
        let img = ImageF32::from_fn(200, 100, |x, _| [if x < 100 { 0.0 } else { 1.0 }; 3]);
        assert_eq!(straighten(&img, 0.0).data, img.data);
        let r = straighten(&img, 10.0);
        assert_eq!((r.width, r.height), (200, 100));
        // Im Uhrzeigersinn: die Trennlinie kippt, oben liegt sie weiter rechts
        let edge = |row: usize| (0..200).find(|x| r.get(*x, row)[0] > 0.5).unwrap();
        assert!(edge(5) > edge(95), "oben {} unten {}", edge(5), edge(95));
        // Vergrößerung > 1 bei Drehung
        assert!(straighten_scale(200.0, 100.0, 10.0) > 1.0);
        assert!((straighten_scale(200.0, 100.0, 0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn straighten_matches_the_closed_form() {
        // Die Matrix ergibt dieselbe Abbildung wie die frühere direkte Formel
        let (w, h, deg) = (300usize, 200usize, -7.5f32);
        let m = Warp::from_angles(w, h, deg, 0.0, 0.0).unwrap().m;
        let k = straighten_scale(w as f32, h as f32, deg);
        let (s, c) = deg.to_radians().sin_cos();
        for (x, y) in [
            (0.5f32, 0.5f32),
            (299.5, 10.5),
            (150.0, 100.0),
            (17.5, 199.5),
        ] {
            let z = m[6] * x + m[7] * y + m[8];
            let sx = (m[0] * x + m[1] * y + m[2]) / z;
            let sy = (m[3] * x + m[4] * y + m[5]) / z;
            let (dx, dy) = ((x - 150.0) / k, (y - 100.0) / k);
            assert!((sx - (c * dx + s * dy + 150.0)).abs() < 1e-3);
            assert!((sy - (-s * dx + c * dy + 100.0)).abs() < 1e-3);
        }
    }

    /// Ausgabe → Quelle wie in `warp`, ohne −0,5
    fn map(m: &[f32; 9], x: f32, y: f32) -> (f32, f32) {
        let z = m[6] * x + m[7] * y + m[8];
        (
            (m[0] * x + m[1] * y + m[2]) / z,
            (m[3] * x + m[4] * y + m[5]) / z,
        )
    }

    #[test]
    fn perspective_straightens_converging_verticals_and_fills_the_frame() {
        let (w, h) = (400usize, 300usize);
        // Senkrechte Hauswand, nach oben fotografiert: Kanten laufen oben
        // zusammen. Positive Korrektur zieht den oberen Rand auseinander –
        // in der Quelle liegen die oberen Ecken der Ausgabe also näher an
        // der Mitte als die unteren.
        let wp = Warp::from_angles(w, h, 0.0, 50.0, 0.0).unwrap();
        let (tl, _) = map(&wp.m, 0.0, 0.0);
        let (bl, _) = map(&wp.m, 0.0, h as f32);
        assert!(tl > bl + 5.0, "oben {tl} unten {bl}");
        // Bildmitte bleibt, und keine Ecke greift außerhalb des Bildes
        assert!(wp.zoom > 1.0);
        let (mx, my) = map(&wp.m, 200.0, 150.0);
        assert!(
            (mx - 200.0).abs() < 0.5 && (my - 150.0).abs() < 25.0,
            "{mx} {my}"
        );
        for (x, y) in [(0.0, 0.0), (400.0, 0.0), (400.0, 300.0), (0.0, 300.0)] {
            let (sx, sy) = map(&wp.m, x, y);
            assert!(
                (-0.01..=400.01).contains(&sx) && (-0.01..=300.01).contains(&sy),
                "{sx} {sy}"
            );
        }
        // Waagerecht positiv: rechter Rand höher, also in der Quelle kürzer
        let wh = Warp::from_angles(w, h, 0.0, 0.0, 50.0).unwrap();
        let right = map(&wh.m, 400.0, 300.0).1 - map(&wh.m, 400.0, 0.0).1;
        let left = map(&wh.m, 0.0, 300.0).1 - map(&wh.m, 0.0, 0.0).1;
        assert!(right < left - 5.0, "rechts {right} links {left}");
        assert!(Warp::from_angles(w, h, 0.0, 0.0, 0.0).is_none());
    }

    /// Dieselben Zahlen liefert `warpOf` in `src/lib/frame.ts` (dort von Hand
    /// geprüft) – ändert sich eine Seite, verrutschen Masken in der Anzeige.
    #[test]
    fn warp_matches_the_frontend() {
        let cases = [
            (
                (2.5, 60.0, -35.0),
                1.236758,
                [
                    0.63944, -0.08166, 56.24089, -0.11168, 0.58744, 63.13025, -0.00028, -0.00056,
                ],
            ),
            (
                (0.0, 100.0, 0.0),
                1.244238,
                [
                    0.72727, -0.18182, 54.54535, 0.0, 0.49347, 55.52443, 0.0, -0.00091,
                ],
            ),
            (
                (-7.5, 0.0, 0.0),
                1.16548,
                [0.85068, -0.11199, 46.66397, 0.11199, 0.85068, 0.0, 0.0, 0.0],
            ),
            (
                (0.0, -40.0, 80.0),
                1.241845,
                [
                    1.18985, 0.11119, 4.58220, 0.06912, 1.12957, 11.16178, 0.00106, 0.00056,
                ],
            ),
        ];
        for ((st, v, h), zoom, m) in cases {
            let m: [f32; 8] = m;
            let w = Warp::from_angles(400, 300, st, v, h).unwrap();
            assert!((w.zoom - zoom).abs() < 1e-4, "{st} {v} {h}: {}", w.zoom);
            for (i, e) in m.iter().enumerate() {
                let tol = if e.abs() > 1.0f32 { 1e-3 } else { 1e-5 };
                assert!((w.m[i] - e).abs() < tol, "{st} {v} {h} [{i}]: {}", w.m[i]);
            }
        }
    }

    #[test]
    fn load_margin_covers_the_zoom() {
        let p = DevelopParams {
            perspective_vertical: 100.0,
            straighten: 10.0,
            ..Default::default()
        };
        let m = load_margin(&p);
        for (w, h) in [(6000, 4000), (4000, 6000)] {
            assert!(m >= Warp::new(w, h, &p).unwrap().zoom - 1e-3);
        }
        assert_eq!(load_margin(&DevelopParams::default()), 1.0);
    }

    #[test]
    fn detects_tilted_horizon() {
        // Horizont, der nach rechts um 4° abfällt (y wächst nach unten)
        for tilt in [4.0f32, -6.0] {
            let t = tilt.to_radians().tan();
            let img = ImageF32::from_fn(800, 500, |x, y| {
                let horizon = 250.0 + (x as f32 - 400.0) * t;
                if (y as f32) < horizon {
                    [0.7, 0.8, 0.9]
                } else {
                    [0.15, 0.2, 0.1]
                }
            });
            let fix = detect_tilt(&img).expect("Horizont gefunden");
            assert!((fix + tilt).abs() < 0.6, "Neigung {tilt}: Korrektur {fix}");
            // Die Korrektur stellt ihn tatsächlich gerade
            let r = straighten(&img, fix);
            let row = |x: usize| (0..500).find(|y| r.get(x, *y)[2] < 0.5).unwrap() as i32;
            assert!(
                (row(200) - row(600)).abs() <= 3,
                "nach Korrektur: {} vs {}",
                row(200),
                row(600)
            );
        }
        let flat = ImageF32::from_fn(300, 200, |_, _| [0.5; 3]);
        assert!(detect_tilt(&flat).is_none());
    }

    #[test]
    fn crops() {
        let img = numbered(10, 10);
        let c = crop(
            &img,
            Crop {
                x: 0.2,
                y: 0.5,
                w: 0.3,
                h: 0.5,
            },
        );
        assert_eq!((c.width, c.height), (3, 5));
        assert_eq!(c.get(0, 0)[0], 52.0);
    }
}
