//! Ein einfaches Float-RGB-Bild. Bewusst ohne Generics, damit die Pipeline
//! übersichtlich bleibt und sich später leicht auf die GPU übertragen lässt.

use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct ImageF32 {
    pub width: usize,
    pub height: usize,
    pub data: Vec<[f32; 3]>,
}

impl ImageF32 {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![[0.0; 3]; width * height],
        }
    }

    pub fn from_fn(
        width: usize,
        height: usize,
        f: impl Fn(usize, usize) -> [f32; 3] + Sync,
    ) -> Self {
        let data = (0..width * height)
            .into_par_iter()
            .map(|i| f(i % width, i / width))
            .collect();
        Self {
            width,
            height,
            data,
        }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> [f32; 3] {
        self.data[y * self.width + x]
    }

    pub fn long_edge(&self) -> usize {
        self.width.max(self.height)
    }

    /// Flächengemittelte Verkleinerung, sodass die längste Kante `max` Pixel hat.
    /// Bilder, die schon kleiner sind, bleiben unverändert (als Kopie – wer das
    /// Original nicht mehr braucht, nimmt [`ImageF32::into_downscaled`]).
    pub fn downscale_to(&self, max: usize) -> ImageF32 {
        match self.downscaled_size(max) {
            Some((w, h)) => self.resize_area(w, h),
            None => self.clone(),
        }
    }

    /// Wie [`ImageF32::downscale_to`], verbraucht aber das Bild: Ist keine
    /// Verkleinerung nötig, wird es ohne Kopie zurückgegeben (bei 60 MP spart
    /// das 720 MB Spitzenspeicher und eine volle Kopie).
    pub fn into_downscaled(self, max: usize) -> ImageF32 {
        match self.downscaled_size(max) {
            Some((w, h)) => self.resize_area(w, h),
            None => self,
        }
    }

    /// Zielgröße für [`ImageF32::downscale_to`], `None` wenn nichts zu tun ist.
    fn downscaled_size(&self, max: usize) -> Option<(usize, usize)> {
        let long = self.long_edge();
        if long <= max || max == 0 {
            return None;
        }
        let scale = max as f64 / long as f64;
        let w = ((self.width as f64 * scale).round() as usize).max(1);
        let h = ((self.height as f64 * scale).round() as usize).max(1);
        Some((w, h))
    }

    /// Verkleinern durch Mittelung aller Quellpixel, die ein Zielpixel abdeckt.
    pub fn resize_area(&self, w: usize, h: usize) -> ImageF32 {
        let sx = self.width as f64 / w as f64;
        let sy = self.height as f64 / h as f64;
        let src = self;
        // Direkt in die fertige Ausgabe schreiben (früher Zeilen-Vecs, danach
        // zusammengefügt – eine zweite Kopie der Ausgabe)
        let mut data = vec![[0f32; 3]; w * h];
        data.par_chunks_mut(w.max(1))
            .enumerate()
            .for_each(|(ty, out)| {
                let y0 = (ty as f64 * sy) as usize;
                let y1 = (((ty + 1) as f64 * sy) as usize).clamp(y0 + 1, src.height);
                for (tx, o) in out.iter_mut().enumerate() {
                    let x0 = (tx as f64 * sx) as usize;
                    let x1 = (((tx + 1) as f64 * sx) as usize).clamp(x0 + 1, src.width);
                    let mut acc = [0.0f64; 3];
                    for y in y0..y1 {
                        let row = &src.data[y * src.width..(y + 1) * src.width];
                        for p in &row[x0..x1] {
                            acc[0] += p[0] as f64;
                            acc[1] += p[1] as f64;
                            acc[2] += p[2] as f64;
                        }
                    }
                    let n = ((y1 - y0) * (x1 - x0)) as f64;
                    *o = [
                        (acc[0] / n) as f32,
                        (acc[1] / n) as f32,
                        (acc[2] / n) as f32,
                    ];
                }
            });
        ImageF32 {
            width: w,
            height: h,
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn into_downscaled_matches_downscale_to() {
        let img = ImageF32::from_fn(123, 77, |x, y| [x as f32 * 0.01, y as f32 * 0.02, 0.5]);
        for max in [0, 50, 122, 123, 500] {
            let a = img.downscale_to(max);
            let b = img.clone().into_downscaled(max);
            assert_eq!((a.width, a.height), (b.width, b.height));
            assert_eq!(a.data, b.data);
        }
        // Ohne Verkleinerung: dieselbe Allokation, keine Kopie
        let ptr = img.data.as_ptr();
        assert_eq!(img.into_downscaled(1000).data.as_ptr(), ptr);
    }
}
