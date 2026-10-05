//! Panorama aus überlappenden Aufnahmen (aus einem Standpunkt gedreht).
//!
//! 1. Merkmale aller Bilder, paarweise Homografien; ein maximaler Spannbaum
//!    über die Anzahl passender Punkte bestimmt, wer an wen anschließt –
//!    die Reihenfolge der Fotos spielt keine Rolle, auch mehrzeilige
//!    Panoramen gehen.
//! 2. Brennweite (in Pixeln) aus den Homografien geschätzt, daraus die
//!    Drehung jeder Kamera relativ zum Referenzbild (H = K·R·K⁻¹).
//! 3. Horizont ausgleichen: Alle Kameras eines Schwenks drehen um dieselbe
//!    Achse – sie wird zur Senkrechten, damit das Panorama nicht „wellt“.
//! 4. Projektion: zylindrisch (Standard, ideal für breite Schwenks),
//!    kugelförmig (auch hoch und breit) oder perspektivisch (gerade Linien
//!    bleiben gerade, nur für mäßige Bildwinkel).
//! 5. Helligkeitsausgleich: je Bild ein Faktor, damit Überlappungen gleich hell
//!    sind (Belichtung schwankt bei Automatik).
//! 6. Überblenden mit zur Bildmitte hin steigendem Gewicht, optional auf das
//!    größte vollständig gefüllte Rechteck zuschneiden.

use super::align::{self, Features, Mat3};
use super::{Loader, Progress, check};
use crate::error::{Error, Result};
use crate::i18n::tr_s;
use crate::image_f32::ImageF32;
use rayon::prelude::*;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Projection {
    Cylindrical,
    Spherical,
    Perspective,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PanoOptions {
    pub projection: Projection,
    /// Auf das größte vollständig gefüllte Rechteck zuschneiden
    pub auto_crop: bool,
    /// Höchstens so viele Megapixel (größere Ergebnisse werden verkleinert)
    pub max_megapixels: f32,
}

impl Default for PanoOptions {
    fn default() -> Self {
        Self {
            projection: Projection::Cylindrical,
            auto_crop: true,
            max_megapixels: 150.0,
        }
    }
}

type V3 = [f64; 3];

fn mv(m: &Mat3, v: V3) -> V3 {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt().max(1e-12);
    a.map(|v| v / l)
}

fn det(m: &Mat3) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// Nächste Drehmatrix (Polarzerlegung per Newton-Iteration).
fn orthonormalize(m: &Mat3) -> Option<Mat3> {
    let d = det(m);
    if d.abs() < 1e-12 {
        return None;
    }
    let s = d.abs().cbrt() * d.signum();
    let mut x = m.map(|r| r.map(|v| v / s));
    for _ in 0..30 {
        let it = align::transpose(&align::invert(&x)?);
        let next: Mat3 =
            std::array::from_fn(|i| std::array::from_fn(|j| 0.5 * (x[i][j] + it[i][j])));
        let diff: f64 = (0..3)
            .flat_map(|i| (0..3).map(move |j| (i, j)))
            .map(|(i, j)| (next[i][j] - x[i][j]).abs())
            .sum();
        x = next;
        if diff < 1e-12 {
            break;
        }
    }
    (det(&x) > 0.0).then_some(x)
}

/// Brennweiten aus einer Homografie zweier Bilder derselben Kamera
/// (Szeliski/Shum, wie OpenCV `focalsFromHomography`). Koordinaten zentriert.
fn focals_from_h(h: &Mat3) -> Option<f64> {
    let h = [
        h[0][0], h[0][1], h[0][2], h[1][0], h[1][1], h[1][2], h[2][0], h[2][1], h[2][2],
    ];
    let pick = |v1: f64, v2: f64, d1: f64, d2: f64| -> Option<f64> {
        let (v1, v2) = if v1 < v2 { (v2, v1) } else { (v1, v2) };
        if v1 > 0.0 && v2 > 0.0 {
            Some(if d1.abs() > d2.abs() { v1 } else { v2 }.sqrt())
        } else if v1 > 0.0 {
            Some(v1.sqrt())
        } else {
            None
        }
    };
    let d1 = h[6] * h[7];
    let d2 = (h[7] - h[6]) * (h[7] + h[6]);
    let f1 = pick(
        -(h[0] * h[1] + h[3] * h[4]) / d1,
        (h[0] * h[0] + h[3] * h[3] - h[1] * h[1] - h[4] * h[4]) / d2,
        d1,
        d2,
    );
    let d1 = h[0] * h[3] + h[1] * h[4];
    let d2 = h[0] * h[0] + h[1] * h[1] - h[3] * h[3] - h[4] * h[4];
    let f0 = pick(-h[2] * h[5] / d1, (h[5] * h[5] - h[2] * h[2]) / d2, d1, d2);
    match (f0, f1) {
        (Some(a), Some(b)) => Some((a * b).sqrt()),
        (Some(a), None) | (None, Some(a)) => Some(a),
        _ => None,
    }
    .filter(|f| f.is_finite())
}

/// Eigenvektor zum kleinsten Eigenwert einer symmetrischen 3×3-Matrix
/// (Jacobi-Verfahren) samt aller Eigenwerte.
fn smallest_eigen(mut a: Mat3) -> (V3, [f64; 3]) {
    let mut v = align::IDENTITY;
    for _ in 0..50 {
        let (mut p, mut q, mut max) = (0, 1, 0.0);
        for (i, j) in [(0, 1), (0, 2), (1, 2)] {
            if a[i][j].abs() > max {
                max = a[i][j].abs();
                p = i;
                q = j;
            }
        }
        if max < 1e-14 {
            break;
        }
        let theta = 0.5 * (2.0 * a[p][q]).atan2(a[q][q] - a[p][p]);
        let (s, c) = theta.sin_cos();
        let mut j = align::IDENTITY;
        j[p][p] = c;
        j[q][q] = c;
        j[p][q] = s;
        j[q][p] = -s;
        a = align::mul(&align::transpose(&j), &align::mul(&a, &j));
        v = align::mul(&v, &j);
    }
    let ev = [a[0][0], a[1][1], a[2][2]];
    let k = (0..3).min_by(|&x, &y| ev[x].total_cmp(&ev[y])).unwrap();
    ([v[0][k], v[1][k], v[2][k]], ev)
}

/// Richtet die Kameradrehungen aus: Horizont waagerecht (falls eine klare
/// Schwenkebene erkennbar ist) und Panorama auf die mittlere Blickrichtung
/// zentriert. Wurde ausgeglichen, wird nur um die Hochachse zentriert – sonst
/// würde ein nach oben/unten geneigter Schwenk wieder gekippt und der Horizont
/// in zylindrischer/Kugelprojektion schief bzw. wellig.
fn level_and_center(mut rot: Vec<Mat3>) -> Vec<Mat3> {
    let mean_z = |rot: &[Mat3]| -> V3 {
        rot.iter().fold([0.0; 3], |a, r| {
            [a[0] + r[0][2], a[1] + r[1][2], a[2] + r[2][2]]
        })
    };
    let mut leveled = false;
    if rot.len() >= 3 {
        let mut c = [[0.0; 3]; 3];
        for r in &rot {
            let x = [r[0][0], r[1][0], r[2][0]];
            for i in 0..3 {
                for j in 0..3 {
                    c[i][j] += x[i] * x[j];
                }
            }
        }
        let (up, ev) = smallest_eigen(c);
        let mut sorted = ev;
        sorted.sort_by(|a, b| a.total_cmp(b));
        // Nur bei klarer Schwenkebene (sonst z. B. senkrechtes Panorama)
        if sorted[1] > 0.05 * sorted[2] {
            let ys: V3 = rot.iter().fold([0.0; 3], |a, r| {
                [a[0] + r[0][1], a[1] + r[1][1], a[2] + r[2][1]]
            });
            let up = if dot(up, ys) < 0.0 {
                up.map(|v| -v)
            } else {
                up
            };
            let zs = mean_z(&rot);
            let y = norm(up);
            let z = norm({
                let d = dot(zs, y);
                [zs[0] - d * y[0], zs[1] - d * y[1], zs[2] - d * y[2]]
            });
            let x = cross(y, z);
            let g: Mat3 = [x, y, z];
            rot = rot.iter().map(|r| align::mul(&g, r)).collect();
            leveled = true;
        }
    }
    // Auf die mittlere Blickrichtung zentrieren. Nach dem Ausgleich ist y die
    // Hochachse und muss es bleiben: dann nur den waagerechten Anteil der
    // Blickrichtung nehmen (Drehung um die Hochachse, Neigung bleibt erhalten).
    let mut zs = mean_z(&rot);
    if leveled {
        zs[1] = 0.0;
    }
    let z = norm(zs);
    let y0 = [0.0, 1.0, 0.0];
    let x = norm(cross(y0, z));
    let y = cross(z, x);
    let g: Mat3 = [x, y, z];
    rot.iter().map(|r| align::mul(&g, r)).collect()
}

#[derive(Clone, Copy)]
struct Proj {
    kind: Projection,
    f: f64,
}

impl Proj {
    fn forward(&self, r: V3) -> Option<(f64, f64)> {
        match self.kind {
            Projection::Perspective => {
                (r[2] > 1e-6).then(|| (self.f * r[0] / r[2], self.f * r[1] / r[2]))
            }
            Projection::Cylindrical => {
                let rho = (r[0] * r[0] + r[2] * r[2]).sqrt();
                (rho > 1e-9).then(|| (self.f * r[0].atan2(r[2]), self.f * r[1] / rho))
            }
            Projection::Spherical => {
                let l = dot(r, r).sqrt();
                (l > 1e-9).then(|| {
                    (
                        self.f * r[0].atan2(r[2]),
                        self.f * (r[1] / l).clamp(-1.0, 1.0).asin(),
                    )
                })
            }
        }
    }

    fn inverse(&self, u: f64, v: f64) -> V3 {
        match self.kind {
            Projection::Perspective => [u / self.f, v / self.f, 1.0],
            Projection::Cylindrical => {
                let (s, c) = (u / self.f).sin_cos();
                [s, v / self.f, c]
            }
            Projection::Spherical => {
                let (s, c) = (u / self.f).sin_cos();
                let (sp, cp) = (v / self.f).sin_cos();
                [s * cp, sp, c * cp]
            }
        }
    }
}

/// Kamera eines Eingangsbildes im Panorama.
#[derive(Clone)]
struct Cam {
    /// Strahl in Kamerakoordinaten → Strahl im Panorama
    r: Mat3,
    w: usize,
    h: usize,
    gain: f32,
}

impl Cam {
    /// Panorama-Strahl → Pixel im Bild
    fn to_pixel(&self, f: f64, ray: V3) -> Option<(f64, f64)> {
        let c = [
            self.r[0][0] * ray[0] + self.r[1][0] * ray[1] + self.r[2][0] * ray[2],
            self.r[0][1] * ray[0] + self.r[1][1] * ray[1] + self.r[2][1] * ray[2],
            self.r[0][2] * ray[0] + self.r[1][2] * ray[1] + self.r[2][2] * ray[2],
        ];
        (c[2] > 1e-6).then(|| {
            (
                f * c[0] / c[2] + (self.w as f64 - 1.0) / 2.0,
                f * c[1] / c[2] + (self.h as f64 - 1.0) / 2.0,
            )
        })
    }

    fn ray(&self, f: f64, x: f64, y: f64) -> V3 {
        mv(
            &self.r,
            [
                (x - (self.w as f64 - 1.0) / 2.0) / f,
                (y - (self.h as f64 - 1.0) / 2.0) / f,
                1.0,
            ],
        )
    }
}

pub fn merge(
    n: usize,
    load: Loader,
    opts: PanoOptions,
    progress: Progress,
    cancel: &AtomicBool,
) -> Result<ImageF32> {
    if n < 2 {
        return Err(Error::Params(
            tr_s(
                "Für ein Panorama mindestens zwei Fotos wählen",
                "Select at least two photos for a panorama",
            )
            .into(),
        ));
    }
    // --- 1. Merkmale --------------------------------------------------------------
    const KEEP_BYTES: usize = 3 << 30;
    let mut full: Vec<Option<ImageF32>> = Vec::new();
    let mut feats: Vec<Features> = Vec::new();
    let mut smalls: Vec<ImageF32> = Vec::new();
    let mut sizes = Vec::new();
    let mut kept = 0;
    for i in 0..n {
        check(cancel)?;
        progress(tr_s("Bilder laden", "Loading images"), i, n);
        let img = load(i)?;
        feats.push(align::features(&img, 1200, 2500));
        smalls.push(img.downscale_to(800));
        sizes.push((img.width, img.height));
        let bytes = img.data.len() * 12;
        if kept + bytes <= KEEP_BYTES {
            kept += bytes;
            full.push(Some(img));
        } else {
            full.push(None);
        }
    }

    // Paarweise Homografien (j → i). Bei vielen Fotos nicht jedes Paar voll
    // prüfen (200 Fotos = 19 900 Paare), sondern nur die aussichtsreichsten.
    let pairs = candidate_pairs(&feats, cancel);
    check(cancel)?;
    let done = std::sync::atomic::AtomicUsize::new(0);
    let fits: Vec<(usize, usize, align::Fit)> = pairs
        .par_iter()
        .filter_map(|&(i, j)| {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return None;
            }
            let fit = align::align(&feats[i], &feats[j], 20);
            let k = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            progress(
                tr_s("Überlappungen suchen", "Finding overlaps"),
                k,
                pairs.len(),
            );
            // Wenige, aber dicht zusammenliegende Treffer sind oft Zufall
            fit.filter(|f| f.inliers as f64 >= 8.0 + 0.3 * f.matches as f64 || f.inliers >= 40)
                .map(|f| (i, j, f))
        })
        .collect();
    check(cancel)?;

    // Maximaler Spannbaum (Kruskal)
    let mut edges: Vec<&(usize, usize, align::Fit)> = fits.iter().collect();
    edges.sort_by_key(|e| std::cmp::Reverse(e.2.inliers));
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        let mut y = x;
        while p[y] != r {
            let nx = p[y];
            p[y] = r;
            y = nx;
        }
        r
    }
    let mut tree: Vec<Vec<(usize, Mat3)>> = vec![vec![]; n]; // Nachbar, H Nachbar → ich
    for e in &edges {
        let (i, j, fit) = (e.0, e.1, &e.2);
        let (a, b) = (find(&mut parent, i), find(&mut parent, j));
        if a != b {
            parent[a] = b;
            let inv = align::invert(&fit.h).unwrap_or(align::IDENTITY);
            tree[i].push((j, fit.h)); // j → i
            tree[j].push((i, inv)); // i → j
        }
    }
    let root0 = find(&mut parent, 0);
    let lonely: Vec<usize> = (0..n).filter(|&i| find(&mut parent, i) != root0).collect();
    if !lonely.is_empty() {
        return Err(Error::Params(crate::tr!(
            "{} von {n} Fotos überlappen mit keinem anderen ausreichend (mind. ~20 % Überlappung nötig): Nr. {}",
            "{} of {n} photos do not overlap enough with any other (at least ~20% overlap needed): no. {}",
            lonely.len(),
            lonely
                .iter()
                .map(|i| (i + 1).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    // Referenz: Mitte des Baums (kleinste größte Entfernung)
    let hops = |s: usize| -> usize {
        let mut d = vec![usize::MAX; n];
        d[s] = 0;
        let mut q = std::collections::VecDeque::from([s]);
        while let Some(x) = q.pop_front() {
            for &(y, _) in &tree[x] {
                if d[y] == usize::MAX {
                    d[y] = d[x] + 1;
                    q.push_back(y);
                }
            }
        }
        d.into_iter().max().unwrap_or(0)
    };
    let reference = (0..n)
        .min_by_key(|&i| (hops(i), usize::MAX - tree[i].len()))
        .unwrap();

    // --- 2. Brennweite und Drehungen ---------------------------------------------
    let center = |i: usize| -> Mat3 {
        let (w, h) = sizes[i];
        [
            [1.0, 0.0, (w as f64 - 1.0) / 2.0],
            [0.0, 1.0, (h as f64 - 1.0) / 2.0],
            [0.0, 0.0, 1.0],
        ]
    };
    let centered = |h: &Mat3, to: usize, from: usize| -> Mat3 {
        align::mul(
            &align::invert(&center(to)).unwrap(),
            &align::mul(h, &center(from)),
        )
    };
    let long = sizes.iter().map(|s| s.0.max(s.1)).max().unwrap_or(1) as f64;
    let mut fs: Vec<f64> = fits
        .iter()
        .filter_map(|(i, j, fit)| focals_from_h(&centered(&fit.h, *i, *j)))
        .filter(|f| *f > 0.3 * long && *f < 30.0 * long)
        .collect();
    fs.sort_by(|a, b| a.total_cmp(b));
    let f = if fs.is_empty() {
        log::warn!("Panorama: Brennweite nicht schätzbar, nehme Normalobjektiv an");
        long * 1.2
    } else {
        fs[fs.len() / 2]
    };
    let k = [[f, 0.0, 0.0], [0.0, f, 0.0], [0.0, 0.0, 1.0]];
    let kinv = [[1.0 / f, 0.0, 0.0], [0.0, 1.0 / f, 0.0], [0.0, 0.0, 1.0]];

    let mut rot: Vec<Option<Mat3>> = vec![None; n];
    rot[reference] = Some(align::IDENTITY);
    let mut queue = std::collections::VecDeque::from([reference]);
    while let Some(p) = queue.pop_front() {
        let rp = rot[p].unwrap();
        for &(c, h_cp) in &tree[p] {
            // tree[p] enthält (c, H c → p)
            if rot[c].is_some() {
                continue;
            }
            let m = align::mul(&kinv, &align::mul(&centered(&h_cp, p, c), &k));
            let r_pc = orthonormalize(&m).unwrap_or(align::IDENTITY);
            rot[c] = Some(align::mul(&rp, &r_pc));
            queue.push_back(c);
        }
    }
    let rot: Vec<Mat3> = rot
        .into_iter()
        .map(|r| r.unwrap_or(align::IDENTITY))
        .collect();

    // --- 3. Horizont ausgleichen und zentrieren -------------------------------------
    let rot = level_and_center(rot);

    let mut cams: Vec<Cam> = (0..n)
        .map(|i| Cam {
            r: rot[i],
            w: sizes[i].0,
            h: sizes[i].1,
            gain: 1.0,
        })
        .collect();

    // --- 4. Ausdehnung des Panoramas --------------------------------------------
    let mut proj = Proj {
        kind: opts.projection,
        f,
    };
    let bounds = |proj: &Proj| -> Option<(f64, f64, f64, f64)> {
        let (mut u0, mut v0, mut u1, mut v1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for cam in &cams {
            for t in 0..=60 {
                let t = t as f64 / 60.0;
                let (w, h) = ((cam.w - 1) as f64, (cam.h - 1) as f64);
                for (x, y) in [(t * w, 0.0), (t * w, h), (0.0, t * h), (w, t * h)] {
                    let (u, v) = proj.forward(cam.ray(f, x, y))?;
                    u0 = u0.min(u);
                    u1 = u1.max(u);
                    v0 = v0.min(v);
                    v1 = v1.max(v);
                }
            }
        }
        Some((u0, v0, u1, v1))
    };
    let Some((mut u0, mut v0, mut u1, mut v1)) = bounds(&proj) else {
        return Err(Error::Params(
            tr_s(
                "Zu weitwinklig für die perspektivische Projektion – „Zylindrisch“ oder „Kugel“ wählen",
                "Too wide-angle for the perspective projection – choose “Cylindrical” or “Spherical”",
            )
            .into(),
        ));
    };
    if opts.projection != Projection::Perspective && u1 - u0 > 2.0 * std::f64::consts::PI * f * 0.98
    {
        return Err(Error::Params(
            tr_s(
                "360°-Panoramen werden noch nicht unterstützt",
                "360° panoramas are not supported yet",
            )
            .into(),
        ));
    }
    let area = (u1 - u0) * (v1 - v0);
    let cap = opts.max_megapixels.max(1.0) as f64 * 1e6;
    if opts.projection == Projection::Perspective
        && area > 25.0 * sizes.iter().map(|s| (s.0 * s.1) as f64).sum::<f64>()
    {
        return Err(Error::Params(
            tr_s(
                "Die perspektivische Projektion verzerrt diese Aufnahmen zu stark – „Zylindrisch“ wählen",
                "The perspective projection distorts these shots too much – choose “Cylindrical”",
            )
            .into(),
        ));
    }
    if area > cap {
        let s = (cap / area).sqrt();
        proj.f *= s;
        (u0, v0, u1, v1) = (u0 * s, v0 * s, u1 * s, v1 * s);
    }
    let out_w = (u1 - u0).ceil() as usize + 1;
    let out_h = (v1 - v0).ceil() as usize + 1;
    log::info!(
        "Panorama: {n} Bilder, Referenz {reference}, f = {f:.0} px, {out_w}×{out_h}, {:?}",
        opts.projection
    );

    // --- 5. Helligkeitsausgleich ---------------------------------------------------
    {
        progress(tr_s("Helligkeit angleichen", "Matching brightness"), 0, 1);
        // Mittlere Helligkeit beider Bilder in ihrer Überlappung
        let overlap = |i: usize, j: usize| -> Option<(f64, f64, f64)> {
            let (si, sj) = (&smalls[i], &smalls[j]);
            let (fi, fj) = (
                si.width as f64 / cams[i].w as f64,
                sj.width as f64 / cams[j].w as f64,
            );
            let (mut a, mut b, mut cnt) = (0.0, 0.0, 0.0);
            for y in (0..sj.height).step_by(3) {
                for x in (0..sj.width).step_by(3) {
                    let pj = sj.data[y * sj.width + x];
                    let ray = cams[j].ray(f, x as f64 / fj, y as f64 / fj);
                    let Some((xi, yi)) = cams[i].to_pixel(f, ray) else {
                        continue;
                    };
                    let Some(pi) = align::sample(si, xi * fi, yi * fi) else {
                        continue;
                    };
                    let (li, lj) = (luma(pi) as f64, luma(pj) as f64);
                    if (0.01..0.9).contains(&li) && (0.01..0.9).contains(&lj) {
                        a += li;
                        b += lj;
                        cnt += 1.0;
                    }
                }
            }
            (cnt > 30.0).then(|| (a / cnt, b / cnt, cnt))
        };
        // Minimiere Σ N_ij [(g_i·I_ij − g_j·I_ji)² / σn² + (1 − g_i)² / σg²]
        // (Brown & Lowe; Helligkeiten wie dort in 0..255)
        let (an, bg) = (1.0 / (10.0f64 * 10.0), 1.0 / (0.1f64 * 0.1));
        let mut a = vec![vec![0.0; n]; n];
        let mut b = vec![0.0; n];
        for i in 0..n {
            a[i][i] += 1e-3;
            b[i] += 1e-3;
        }
        for (i, j, _) in &fits {
            let Some((ii, jj, cnt)) = overlap(*i, *j) else {
                continue;
            };
            let (ii, jj) = (ii * 255.0, jj * 255.0);
            a[*i][*i] += cnt * (an * ii * ii + bg);
            a[*j][*j] += cnt * (an * jj * jj + bg);
            a[*i][*j] -= cnt * an * ii * jj;
            a[*j][*i] -= cnt * an * ii * jj;
            b[*i] += cnt * bg;
            b[*j] += cnt * bg;
        }
        if let Some(g) = align::solve(a, b) {
            for (cam, g) in cams.iter_mut().zip(g) {
                cam.gain = g.clamp(0.25, 4.0) as f32;
            }
        }
        log::info!(
            "Panorama: Helligkeitsfaktoren {:?}",
            cams.iter().map(|c| c.gain).collect::<Vec<_>>()
        );
    }

    // --- 6. Überblenden ------------------------------------------------------------
    let mut acc = vec![[0f32; 3]; out_w * out_h];
    let mut wsum = vec![0f32; out_w * out_h];
    for i in 0..n {
        check(cancel)?;
        progress(tr_s("Zusammenfügen", "Stitching"), i, n);
        let img = match full[i].take() {
            Some(img) => img,
            None => load(i)?,
        };
        let cam = &cams[i];
        // Ausdehnung dieses Bildes im Panorama
        let (mut bu0, mut bv0, mut bu1, mut bv1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for t in 0..=60 {
            let t = t as f64 / 60.0;
            let (w, h) = ((cam.w - 1) as f64, (cam.h - 1) as f64);
            for (x, y) in [(t * w, 0.0), (t * w, h), (0.0, t * h), (w, t * h)] {
                if let Some((u, v)) = proj.forward(cam.ray(f, x, y)) {
                    bu0 = bu0.min(u);
                    bu1 = bu1.max(u);
                    bv0 = bv0.min(v);
                    bv1 = bv1.max(v);
                }
            }
        }
        let x0 = ((bu0 - u0).floor().max(0.0)) as usize;
        let x1 = (((bu1 - u0).ceil() + 1.0) as usize).min(out_w);
        let y0 = ((bv0 - v0).floor().max(0.0)) as usize;
        let y1 = (((bv1 - v0).ceil() + 1.0) as usize).min(out_h);
        let (iw, ih) = (cam.w as f64, cam.h as f64);
        let gain = cam.gain;
        acc[y0 * out_w..y1 * out_w]
            .par_chunks_mut(out_w)
            .zip(wsum[y0 * out_w..y1 * out_w].par_chunks_mut(out_w))
            .enumerate()
            .for_each(|(r, (arow, wrow))| {
                let v = (y0 + r) as f64 + v0;
                for x in x0..x1 {
                    let ray = proj.inverse(x as f64 + u0, v);
                    let Some((px, py)) = cam.to_pixel(f, ray) else {
                        continue;
                    };
                    let Some(p) = align::sample(&img, px, py) else {
                        continue;
                    };
                    // Gewicht steigt zur Bildmitte (weiche, schmale Übergänge)
                    let wx = (px.min(iw - 1.0 - px) / (iw * 0.5)).clamp(0.0, 1.0);
                    let wy = (py.min(ih - 1.0 - py) / (ih * 0.5)).clamp(0.0, 1.0);
                    let wt = ((wx * wy) as f32).powi(2) + 1e-6;
                    for c in 0..3 {
                        arow[x][c] += wt * p[c] * gain;
                    }
                    wrow[x] += wt;
                }
            });
    }
    progress(tr_s("Zusammenfügen", "Stitching"), n, n);
    drop(full);
    drop(smalls);
    // Im Puffer selbst normieren – bei 150 MP spart das 1,8 GB
    acc.par_iter_mut().zip(wsum.par_iter()).for_each(|(a, w)| {
        *a = if *w > 0.0 { a.map(|c| c / w) } else { [0.0; 3] };
    });
    let rect = if opts.auto_crop {
        largest_filled_rect(&wsum, out_w, out_h).filter(|r| r.2 * r.3 > out_w * out_h / 10)
    } else {
        None
    };
    drop(wsum);
    let mut out = ImageF32 {
        width: out_w,
        height: out_h,
        data: acc,
    };
    if let Some((x, y, w, h)) = rect {
        crop_in_place(&mut out, x, y, w, h);
    }
    Ok(out)
}

/// Bildpaare, für die sich die volle Ausrichtung lohnt. Bis 10 Fotos alle;
/// darüber je Foto die 6 Partner mit den meisten übereinstimmenden Merkmalen
/// in einer groben Vorauswahl (gleichmäßig verteilte Teilmenge der Punkte).
fn candidate_pairs(feats: &[Features], cancel: &AtomicBool) -> Vec<(usize, usize)> {
    let n = feats.len();
    let all: Vec<(usize, usize)> = (0..n)
        .flat_map(|i| (i + 1..n).map(move |j| (i, j)))
        .collect();
    if n <= 10 {
        return all;
    }
    const SUBSET: usize = 300;
    const PARTNERS: usize = 6;
    let subsets: Vec<Vec<align::Keypoint>> = feats
        .iter()
        .map(|f| {
            // Punkte liegen nach Rasterzellen geordnet: jede k-te Ecke deckt das ganze Bild ab
            let step = (f.points.len() / SUBSET).max(1);
            f.points.iter().step_by(step).copied().collect()
        })
        .collect();
    let scores: Vec<usize> = all
        .par_iter()
        .map(|&(i, j)| {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return 0;
            }
            align::match_points(&subsets[i], &subsets[j]).len()
        })
        .collect();
    let mut keep = std::collections::BTreeSet::new();
    for i in 0..n {
        let mut mine: Vec<(usize, usize)> = all
            .iter()
            .zip(&scores)
            .enumerate()
            .filter(|(_, ((a, b), _))| *a == i || *b == i)
            .map(|(k, (_, s))| (*s, k))
            .collect();
        mine.sort_by_key(|m| std::cmp::Reverse(m.0));
        for &(_, k) in mine.iter().take(PARTNERS) {
            keep.insert(all[k]);
        }
    }
    keep.into_iter().collect()
}

fn luma(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

/// Zuschneiden ohne zweiten Puffer: Zeilen nach vorn schieben, Rest abschneiden.
fn crop_in_place(img: &mut ImageF32, x: usize, y: usize, w: usize, h: usize) {
    let stride = img.width;
    for row in 0..h {
        let src = (y + row) * stride + x;
        img.data.copy_within(src..src + w, row * w);
    }
    img.data.truncate(w * h);
    img.data.shrink_to_fit();
    img.width = w;
    img.height = h;
}

/// Größtes achsenparalleles Rechteck, in dem jedes Pixel Bildinhalt hat
/// (Histogramm-Verfahren auf einer vergröberten Maske, dann vorsichtig
/// um eine Zelle verkleinert).
fn largest_filled_rect(wsum: &[f32], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    let cell = (w.max(h) / 1500).max(1);
    let (cw, ch) = (w / cell, h / cell);
    if cw < 3 || ch < 3 {
        return None;
    }
    // Zelle gilt als gefüllt, wenn alle ihre Pixel gefüllt sind
    let full: Vec<bool> = (0..cw * ch)
        .into_par_iter()
        .map(|k| {
            let (cx, cy) = (k % cw, k / cw);
            (cy * cell..(cy + 1) * cell)
                .all(|y| (cx * cell..(cx + 1) * cell).all(|x| wsum[y * w + x] > 0.0))
        })
        .collect();
    let mut heights = vec![0usize; cw];
    let mut best = (0usize, 0usize, 0usize, 0usize, 0usize); // Fläche, x, y, w, h
    for y in 0..ch {
        for x in 0..cw {
            heights[x] = if full[y * cw + x] { heights[x] + 1 } else { 0 };
        }
        // Größtes Rechteck im Histogramm
        let mut stack: Vec<usize> = Vec::new();
        for x in 0..=cw {
            let hgt = if x < cw { heights[x] } else { 0 };
            while let Some(&top) = stack.last() {
                if heights[top] <= hgt {
                    break;
                }
                stack.pop();
                let left = stack.last().map_or(0, |&l| l + 1);
                let width = x - left;
                let a = heights[top] * width;
                if a > best.0 {
                    best = (a, left, y + 1 - heights[top], width, heights[top]);
                }
            }
            stack.push(x);
        }
    }
    let (a, x, y, bw, bh) = best;
    (a > 0).then(|| {
        // Eine Zelle Sicherheitsabstand nur, wo die Kante nicht am Bildrand liegt
        let x0 = (x * cell + if x > 0 { cell } else { 0 }).min(w - 1);
        let y0 = (y * cell + if y > 0 { cell } else { 0 }).min(h - 1);
        let x1 = ((x + bw) * cell)
            .saturating_sub(if x + bw < cw { cell } else { 0 })
            .max(x0 + 1);
        let y1 = ((y + bh) * cell)
            .saturating_sub(if y + bh < ch { cell } else { 0 })
            .max(y0 + 1);
        (x0, y0, x1 - x0, y1 - y0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::align::tests::texture;

    /// Erzeugt Aufnahmen einer weiten Szene mit einer gedrehten Kamera:
    /// Die Szene ist auf einen Zylinder gemalt, die Kamera schwenkt.
    fn shoot(scene: &ImageF32, f: f64, yaw: f64, w: usize, h: usize, exposure: f32) -> ImageF32 {
        let (s, c) = yaw.sin_cos();
        ImageF32::from_fn(w, h, |x, y| {
            let d = [
                (x as f64 - (w - 1) as f64 / 2.0) / f,
                (y as f64 - (h - 1) as f64 / 2.0) / f,
                1.0,
            ];
            // Kamera um die Hochachse gedreht
            let r = [c * d[0] + s * d[2], d[1], -s * d[0] + c * d[2]];
            let theta = r[0].atan2(r[2]);
            let hh = r[1] / (r[0] * r[0] + r[2] * r[2]).sqrt();
            let u = theta * f + scene.width as f64 / 2.0;
            let v = hh * f + scene.height as f64 / 2.0;
            align::sample(scene, u, v)
                .unwrap_or([0.0; 3])
                .map(|v| v * exposure)
        })
    }

    /// Geneigter Schwenk (alle Aufnahmen 0,25 rad nach oben): Nach Ausgleich
    /// und Zentrierung müssen alle Kamera-x-Achsen waagerecht liegen.
    #[test]
    fn leveling_keeps_horizon_for_pitched_pan() {
        let rx = |a: f64| -> Mat3 {
            let (s, c) = a.sin_cos();
            [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
        };
        let ry = |a: f64| -> Mat3 {
            let (s, c) = a.sin_cos();
            [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
        };
        let pitch = 0.25;
        let world: Vec<Mat3> = [-0.5, -0.25, 0.0, 0.25, 0.5]
            .iter()
            .map(|&yaw| align::mul(&ry(yaw), &rx(pitch)))
            .collect();
        // Wie in `merge`: Drehungen relativ zu einer (nicht mittigen) Referenzkamera
        let rref = align::transpose(&world[1]);
        let rot: Vec<Mat3> = world.iter().map(|r| align::mul(&rref, r)).collect();
        let out = level_and_center(rot);
        for (i, r) in out.iter().enumerate() {
            // x-Achse der Kamera = erste Spalte
            assert!(
                r[1][0].abs() < 1e-6,
                "Kamera {i}: Horizont gekippt ({})",
                r[1][0]
            );
            // Neigung bleibt erhalten
            assert!(
                (r[1][2].abs() - pitch.sin()).abs() < 1e-6,
                "Kamera {i}: Neigung {}",
                r[1][2]
            );
        }
        // Waagerecht zentriert: mittlere Blickrichtung ohne seitlichen Anteil
        let zx: f64 = out.iter().map(|r| r[0][2]).sum();
        assert!(zx.abs() < 1e-6, "nicht zentriert: {zx}");
    }

    #[test]
    fn stitches_a_horizontal_sweep() {
        let scene = texture(2000, 700, 11);
        let (w, h, f) = (640usize, 480usize, 700.0f64);
        // Drei Aufnahmen, ~35 % Überlappung, mittlere etwas heller belichtet
        let shots = [
            shoot(&scene, f, -0.55, w, h, 1.0),
            shoot(&scene, f, 0.0, w, h, 1.25),
            shoot(&scene, f, 0.55, w, h, 1.0),
        ];
        let cancel = AtomicBool::new(false);
        // Reihenfolge absichtlich gemischt
        let order = [2usize, 0, 1];
        let out = merge(
            3,
            &|i| Ok(shots[order[i]].clone()),
            PanoOptions::default(),
            &|_, _, _| {},
            &cancel,
        )
        .unwrap();
        // Bildwinkel: 3 Aufnahmen à ~49,1° mit 0,55 rad (31,5°) Abstand ≈ 112°
        let expect_w = (1.1 + 2.0 * (w as f64 / 2.0 / f).atan()) * f;
        assert!(
            (out.width as f64) > expect_w * 0.85 && (out.width as f64) < expect_w * 1.05,
            "Breite {} statt ~{expect_w:.0}",
            out.width
        );
        assert!(
            out.height > h * 8 / 10 && out.height <= h + 10,
            "Höhe {}",
            out.height
        );
        // Zugeschnitten: keine leeren Pixel
        assert!(out.data.iter().all(|p| luma(*p) > 0.0), "leere Ecken");
        // Helligkeitsausgleich: linke und rechte Hälfte ähnlich hell wie die Szene
        let mean = |x0: usize, x1: usize| {
            let mut s = 0.0;
            let mut c = 0.0;
            for y in 0..out.height {
                for x in x0..x1 {
                    s += luma(out.get(x, y));
                    c += 1.0;
                }
            }
            s / c
        };
        let (l, m) = (
            mean(0, out.width / 4),
            mean(out.width * 3 / 8, out.width * 5 / 8),
        );
        assert!(
            (l / m - 1.0).abs() < 0.1,
            "Helligkeitssprung: links {l:.3}, Mitte {m:.3}"
        );
    }

    /// Viele Fotos (Vorauswahl der Paare statt alle 66 zu prüfen), gemischt.
    #[test]
    fn stitches_many_shots_with_pair_preselection() {
        let scene = texture(2600, 700, 21);
        let (w, h, f) = (640usize, 480usize, 700.0f64);
        let n = 12;
        let shots: Vec<ImageF32> = (0..n)
            .map(|k| shoot(&scene, f, (k as f64 - 5.5) * 0.2, w, h, 1.0))
            .collect();
        let order = [7usize, 2, 11, 0, 5, 9, 1, 4, 10, 3, 8, 6];
        let cancel = AtomicBool::new(false);
        let out = merge(
            n,
            &|i| Ok(shots[order[i]].clone()),
            PanoOptions::default(),
            &|_, _, _| {},
            &cancel,
        )
        .unwrap();
        let expect_w = (2.2 + 2.0 * (w as f64 / 2.0 / f).atan()) * f;
        assert!(
            (out.width as f64) > expect_w * 0.85 && (out.width as f64) < expect_w * 1.05,
            "Breite {} statt ~{expect_w:.0}",
            out.width
        );
        assert!(out.height > h * 8 / 10, "Höhe {}", out.height);
    }
}
