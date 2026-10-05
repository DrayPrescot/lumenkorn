//! Weichgezeichnete Helligkeitskarten für lokale Anpassungen (Lichter/Tiefen,
//! Klarheit).
//!
//! Die Karten werden auf einer festen, kleinen Auflösung berechnet und beim
//! Anwenden bilinear abgetastet. Der Weichzeichner-Radius ist damit relativ zur
//! Bildgröße – die Vorschau (2048 px) sieht genauso aus wie der Export (6000 px).

use rayon::prelude::*;

pub struct LumaMap {
    w: usize,
    h: usize,
    data: Vec<f32>,
    // Skalierung von Bild- auf Kartenkoordinaten
    sx: f32,
    sy: f32,
}

impl LumaMap {
    /// `luma`: Helligkeit pro Pixel; `long_edge`: Kartengröße; `radius`: Box-Radius
    /// in Kartenpixeln (drei Durchgänge ≈ Gauß).
    pub fn build(
        luma: &[f32],
        width: usize,
        height: usize,
        long_edge: usize,
        radius: usize,
    ) -> Self {
        let scale = (long_edge as f32 / width.max(height) as f32).min(1.0);
        let w = ((width as f32 * scale).round() as usize).max(1);
        let h = ((height as f32 * scale).round() as usize).max(1);

        // Flächenmittel auf die Kartengröße
        let fx = width as f32 / w as f32;
        let fy = height as f32 / h as f32;
        let mut data: Vec<f32> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (tx, ty) = (i % w, i / w);
                let x0 = (tx as f32 * fx) as usize;
                let x1 = (((tx + 1) as f32 * fx) as usize).clamp(x0 + 1, width);
                let y0 = (ty as f32 * fy) as usize;
                let y1 = (((ty + 1) as f32 * fy) as usize).clamp(y0 + 1, height);
                let mut s = 0.0f32;
                for y in y0..y1 {
                    s += luma[y * width + x0..y * width + x1].iter().sum::<f32>();
                }
                s / ((x1 - x0) * (y1 - y0)) as f32
            })
            .collect();

        for _ in 0..3 {
            box_blur_h(&mut data, w, h, radius);
            box_blur_v(&mut data, w, h, radius);
        }
        LumaMap {
            w,
            h,
            data,
            sx: w as f32 / width as f32,
            sy: h as f32 / height as f32,
        }
    }

    /// Bilinear abgetasteter Wert an Bildposition (x, y).
    #[inline]
    pub fn sample(&self, x: usize, y: usize) -> f32 {
        let fx = ((x as f32 + 0.5) * self.sx - 0.5).clamp(0.0, (self.w - 1) as f32);
        let fy = ((y as f32 + 0.5) * self.sy - 0.5).clamp(0.0, (self.h - 1) as f32);
        let x0 = fx as usize;
        let y0 = fy as usize;
        let x1 = (x0 + 1).min(self.w - 1);
        let y1 = (y0 + 1).min(self.h - 1);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let a = self.data[y0 * self.w + x0];
        let b = self.data[y0 * self.w + x1];
        let c = self.data[y1 * self.w + x0];
        let d = self.data[y1 * self.w + x1];
        let top = a + (b - a) * tx;
        let bot = c + (d - c) * tx;
        top + (bot - top) * ty
    }
}

// Kastenfilter mit geklemmtem Rand (Randpixel wiederholt). Die GPU rechnet
// die Karten identisch (`blur_h`/`blur_v` in pipeline.wgsl) – Randverhalten
// daher nicht an `masks::box_blur` angleichen, sonst weichen CPU und GPU ab.
fn box_blur_h(data: &mut [f32], w: usize, _h: usize, r: usize) {
    if r == 0 {
        return;
    }
    data.par_chunks_mut(w).for_each(|row| {
        let src = row.to_vec();
        let n = (2 * r + 1) as f32;
        let at = |i: isize| src[i.clamp(0, w as isize - 1) as usize];
        let mut acc: f32 = (-(r as isize)..=r as isize).map(at).sum();
        for (x, v) in row.iter_mut().enumerate() {
            *v = acc / n;
            acc += at(x as isize + r as isize + 1) - at(x as isize - r as isize);
        }
    });
}

fn box_blur_v(data: &mut [f32], w: usize, h: usize, r: usize) {
    if r == 0 {
        return;
    }
    let src = data.to_vec();
    let n = (2 * r + 1) as f32;
    let cols: Vec<Vec<f32>> = (0..w)
        .into_par_iter()
        .map(|x| {
            let at = |i: isize| src[i.clamp(0, h as isize - 1) as usize * w + x];
            let mut acc: f32 = (-(r as isize)..=r as isize).map(at).sum();
            let mut col = vec![0.0; h];
            for (y, c) in col.iter_mut().enumerate() {
                *c = acc / n;
                acc += at(y as isize + r as isize + 1) - at(y as isize - r as isize);
            }
            col
        })
        .collect();
    for (x, col) in cols.into_iter().enumerate() {
        for (y, v) in col.into_iter().enumerate() {
            data[y * w + x] = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_stays_flat_and_is_resolution_independent() {
        let small: Vec<f32> = vec![0.4; 200 * 100];
        let m = LumaMap::build(&small, 200, 100, 64, 3);
        assert!((m.sample(10, 10) - 0.4).abs() < 1e-5);

        // Linke Hälfte dunkel, rechte hell: der Übergang liegt bei beiden Auflösungen
        // an derselben relativen Stelle.
        let make = |w: usize, h: usize| -> Vec<f32> {
            (0..w * h)
                .map(|i| if i % w < w / 2 { 0.1 } else { 0.9 })
                .collect()
        };
        let a = LumaMap::build(&make(400, 200), 400, 200, 128, 4);
        let b = LumaMap::build(&make(1600, 800), 1600, 800, 128, 4);
        for rel in [0.3f32, 0.45, 0.5, 0.55, 0.7] {
            let va = a.sample((rel * 400.0) as usize, 100);
            let vb = b.sample((rel * 1600.0) as usize, 400);
            assert!((va - vb).abs() < 0.02, "{rel}: {va} vs {vb}");
        }
    }
}
