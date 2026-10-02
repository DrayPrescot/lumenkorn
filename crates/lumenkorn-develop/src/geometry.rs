//! Geometrie: Orientierung, Drehung, Spiegelung, Zuschnitt.

use crate::image_f32::ImageF32;
use crate::params::Crop;

/// Wendet eine EXIF-Orientierung (1–8) an, sodass das Bild aufrecht steht.
pub fn apply_exif_orientation(img: ImageF32, orientation: u16) -> ImageF32 {
    match orientation {
        2 => flip_h(&img),
        3 => rotate_180(&img),
        4 => flip_v(&img),
        5 => flip_h(&rotate_90_cw(&img)),
        6 => rotate_90_cw(&img),
        7 => flip_h(&rotate_270_cw(&img)),
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
    let (w, h) = (img.width, img.height);
    // Ziel (x, y) ← Quelle (y, h-1-x)
    ImageF32::from_fn(h, w, |x, y| img.get(y, h - 1 - x))
}

pub fn rotate_270_cw(img: &ImageF32) -> ImageF32 {
    let (w, h) = (img.width, img.height);
    ImageF32::from_fn(h, w, |x, y| img.get(w - 1 - y, x))
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

/// Dreht um die Bildmitte (im Uhrzeigersinn positiv) und vergrößert so, dass
/// der Rahmen voll bleibt. Bilineare Abtastung.
pub fn straighten(img: &ImageF32, deg: f32) -> ImageF32 {
    let (w, h) = (img.width, img.height);
    let (wf, hf) = (w as f32, h as f32);
    let k = straighten_scale(wf, hf, deg);
    let (s, c) = deg.to_radians().sin_cos();
    let (cx, cy) = (wf / 2.0, hf / 2.0);
    ImageF32::from_fn(w, h, |x, y| {
        // Zielpixel → Quellposition: zurückdrehen und verkleinern
        let (dx, dy) = ((x as f32 + 0.5 - cx) / k, (y as f32 + 0.5 - cy) / k);
        let sx = c * dx + s * dy + cx - 0.5;
        let sy = -s * dx + c * dy + cy - 0.5;
        bilinear(img, sx, sy)
    })
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
    let lum: Vec<f32> = small
        .data
        .iter()
        .map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2])
        .collect();
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
