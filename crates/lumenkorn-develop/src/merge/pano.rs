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
//! 6. Nähte: Auf einer verkleinerten Fassung bekommt jedes Pixel genau ein
//!    Bild. Die Naht zwischen zwei Bildern läuft dort, wo sie sich am wenigsten
//!    unterscheiden, nicht quer durch nahe Gegenstände, die sich zwischen den
//!    Aufnahmen verschieben (Parallaxe) – sonst entstünden Geisterbilder.
//! 7. Zusammensetzen in voller Größe, nur entlang der Nähte schmal
//!    überblendet; optional auf das größte vollständig gefüllte Rechteck
//!    zuschneiden.

use super::align::{self, Features, Mat3};
use super::{FrameCache, Loader, Merged, Progress, check};
use crate::color::luma;
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
    // Je zwei Schätzungen für f²; die größere zuerst (wie OpenCV). Sind beide
    // positiv, gilt die mit dem betragsgrößeren Nenner – numerisch stabiler.
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
        // Nur bei klarer Schwenkebene: Die x-Achsen müssen eine Ebene
        // aufspannen (mittlerer Eigenwert nicht unter 5 % des größten). Liegen
        // sie fast auf einer Linie (z. B. senkrechtes Panorama), ist die
        // Normale – die Hochachse – nicht bestimmt.
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

    // Umkehrung (Panoramapunkt → Strahl), aufgeteilt in den nur von der
    // Spalte und den nur von der Zeile abhängigen Teil: sin/cos einmal je
    // Spalte bzw. Zeile statt je Pixel und Bild.

    /// Spaltenanteil für `u`: (u/f, –) bzw. sin/cos von u/f.
    fn column(&self, u: f64) -> (f64, f64) {
        match self.kind {
            Projection::Perspective => (u / self.f, 0.0),
            Projection::Cylindrical | Projection::Spherical => (u / self.f).sin_cos(),
        }
    }

    /// Zeilenanteil für `v`: (v/f, –) bzw. sin/cos von v/f.
    fn row(&self, v: f64) -> (f64, f64) {
        match self.kind {
            Projection::Perspective | Projection::Cylindrical => (v / self.f, 0.0),
            Projection::Spherical => (v / self.f).sin_cos(),
        }
    }

    /// Strahl zu Panoramapunkt (u, v) aus `column(u)` und `row(v)`.
    #[inline]
    fn inverse(&self, col: (f64, f64), row: (f64, f64)) -> V3 {
        match self.kind {
            Projection::Perspective => [col.0, row.0, 1.0],
            Projection::Cylindrical => [col.0, row.0, col.1],
            Projection::Spherical => [col.0 * row.1, row.0, col.1 * row.1],
        }
    }
}

/// Spannbaum der Bilder: je Bild (Nachbar, Homografie Nachbar → Bild).
type Tree = Vec<Vec<(usize, Mat3)>>;

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

    /// Ausdehnung im Panorama (u0, v0, u1, v1), abgetastet an 61 Punkten je
    /// Bildkante; `None`, wenn ein Randpunkt nicht abbildbar ist.
    fn bounds(&self, f: f64, proj: &Proj) -> Option<(f64, f64, f64, f64)> {
        let (mut u0, mut v0, mut u1, mut v1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        let (w, h) = ((self.w - 1) as f64, (self.h - 1) as f64);
        for t in 0..=60 {
            let t = t as f64 / 60.0;
            for (x, y) in [(t * w, 0.0), (t * w, h), (0.0, t * h), (w, t * h)] {
                let (u, v) = proj.forward(self.ray(f, x, y))?;
                u0 = u0.min(u);
                u1 = u1.max(u);
                v0 = v0.min(v);
                v1 = v1.max(v);
            }
        }
        Some((u0, v0, u1, v1))
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
    merge_report(n, load, opts, progress, cancel).map(|m| m.image)
}

/// Wie [`merge`], meldet zusätzlich Fotos, deren Drehung nicht bestimmbar war.
pub fn merge_report(
    n: usize,
    load: Loader,
    opts: PanoOptions,
    progress: Progress,
    cancel: &AtomicBool,
) -> Result<Merged> {
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
    let mut cache = FrameCache::new(load);
    let mut feats: Vec<Features> = Vec::new();
    let mut smalls: Vec<ImageF32> = Vec::new();
    let mut sizes = Vec::new();
    for i in 0..n {
        check(cancel)?;
        progress(tr_s("Bilder laden", "Loading images"), i, n);
        let img = load(i)?;
        feats.push(align::features(&img, 1200, 2500));
        smalls.push(img.downscale_to(800));
        sizes.push((img.width, img.height));
        cache.offer(img);
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
            // Prüfung nach Brown & Lowe (α = 8, β = 0,3): echte Überlappung,
            // wenn die passenden Paare einen festen Anteil aller Zuordnungen
            // ausmachen – wenige Treffer unter vielen Zuordnungen sind oft
            // Zufall. Ab 40 passenden Paaren gilt die Überlappung als sicher.
            fit.filter(|f| f.inliers as f64 >= 8.0 + 0.3 * f.matches as f64 || f.inliers >= 40)
                .map(|f| (i, j, f))
        })
        .collect();
    check(cancel)?;

    let (tree, reference) = spanning_tree(n, &fits)?;

    // --- 2. Brennweite und Drehungen ---------------------------------------------
    let f = estimate_focal(&fits, &sizes);
    let (rot, unaligned) = rotations(&tree, reference, f, &sizes);

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
    let bounds = cams.iter().try_fold(
        (f64::MAX, f64::MAX, f64::MIN, f64::MIN),
        |(u0, v0, u1, v1), cam| {
            let (a, b, c, d) = cam.bounds(f, &proj)?;
            Some((u0.min(a), v0.min(b), u1.max(c), v1.max(d)))
        },
    );
    let Some((mut u0, mut v0, mut u1, mut v1)) = bounds else {
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
    // Zwischengespeicherte Bilder und Ergebnispuffer (acc + wsum, 16 Byte je
    // Pixel – bei 150 MP 2,4 GB) teilen sich das Budget: überzählige Bilder
    // freigeben, sie werden beim Zusammenfügen neu geladen.
    cache.reserve(out_w * out_h * 16);

    // --- 5. Helligkeitsausgleich ---------------------------------------------------
    progress(tr_s("Helligkeit angleichen", "Matching brightness"), 0, 1);
    gain_compensation(&mut cams, &smalls, &fits, f);

    // --- 6. Nähte ------------------------------------------------------------------
    progress(tr_s("Nähte suchen", "Finding seams"), 0, 1);
    let order = tree_order(&tree, reference);
    let seams = Seams::find(
        &cams,
        &smalls,
        f,
        &proj,
        (u0, v0),
        (out_w, out_h),
        &order,
        cancel,
    )?;

    // --- 7. Zusammensetzen ----------------------------------------------------------
    let mut acc = vec![[0f32; 3]; out_w * out_h];
    let mut wsum = vec![0f32; out_w * out_h];
    let cols: Vec<(f64, f64)> = (0..out_w).map(|x| proj.column(x as f64 + u0)).collect();
    for (i, cam) in cams.iter().enumerate() {
        check(cancel)?;
        progress(tr_s("Zusammenfügen", "Stitching"), i, n);
        let img = cache.take(i)?;
        // Ausdehnung dieses Bildes im Panorama (abbildbar, das wurde oben für
        // alle Bilder geprüft; der Maßstab von `proj` ändert daran nichts)
        let Some((bu0, bv0, bu1, bv1)) = cam.bounds(f, &proj) else {
            continue;
        };
        let x0 = ((bu0 - u0).floor().max(0.0)) as usize;
        let x1 = (((bu1 - u0).ceil() + 1.0) as usize).min(out_w);
        let y0 = ((bv0 - v0).floor().max(0.0)) as usize;
        let y1 = (((bv1 - v0).ceil() + 1.0) as usize).min(out_h);
        let (iw, ih) = (cam.w as f64, cam.h as f64);
        let gain = cam.gain;
        let mask = seams.masks[i].as_ref();
        acc[y0 * out_w..y1 * out_w]
            .par_chunks_mut(out_w)
            .zip(wsum[y0 * out_w..y1 * out_w].par_chunks_mut(out_w))
            .enumerate()
            .for_each(|(r, (arow, wrow))| {
                let row = proj.row((y0 + r) as f64 + v0);
                for x in x0..x1 {
                    let ray = proj.inverse(cols[x], row);
                    let Some((px, py)) = cam.to_pixel(f, ray) else {
                        continue;
                    };
                    let Some(p) = align::sample(&img, px, py) else {
                        continue;
                    };
                    // Gewicht: Anteil dieses Bildes laut Nahtmaske. Dazu sehr
                    // schwach ein zur Bildmitte steigendes Gewicht; es zählt
                    // nur, wo die gröbere Maske auf ein Bild zeigt, das hier in
                    // voller Auflösung knapp nicht mehr hinreicht.
                    let wx = (px.min(iw - 1.0 - px) / (iw * 0.5)).clamp(0.0, 1.0);
                    let wy = (py.min(ih - 1.0 - py) / (ih * 0.5)).clamp(0.0, 1.0);
                    let center = ((wx * wy) as f32).powi(2) + 1e-6;
                    let m = mask.map_or(0.0, |m| {
                        m.at(x as f64 * seams.scale, (y0 + r) as f64 * seams.scale)
                    });
                    let wt = m + 1e-4 * center;
                    for c in 0..3 {
                        arow[x][c] += wt * p[c] * gain;
                    }
                    wrow[x] += wt;
                }
            });
    }
    progress(tr_s("Zusammenfügen", "Stitching"), n, n);
    drop(seams);
    drop(cache);
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
    Ok(Merged {
        image: out,
        unaligned,
    })
}

/// Maximaler Spannbaum (Kruskal) über die Anzahl passender Punkte: je Bild die
/// Nachbarn samt Homografie Nachbar → Bild. Dazu die Referenz: die Mitte des
/// Baums (kleinste größte Entfernung, bei Gleichstand die meisten Nachbarn).
/// Fehler, wenn nicht alle Bilder verbunden sind.
fn spanning_tree(n: usize, fits: &[(usize, usize, align::Fit)]) -> Result<(Tree, usize)> {
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
    let mut tree: Tree = vec![vec![]; n];
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
    Ok((tree, reference))
}

/// Homografie `h` (Bild `from` → Bild `to`, Pixel) in bildmittigen Koordinaten.
fn centered(h: &Mat3, sizes: &[(usize, usize)], to: usize, from: usize) -> Mat3 {
    let center = |i: usize| -> Mat3 {
        let (w, h) = sizes[i];
        [
            [1.0, 0.0, (w as f64 - 1.0) / 2.0],
            [0.0, 1.0, (h as f64 - 1.0) / 2.0],
            [0.0, 0.0, 1.0],
        ]
    };
    align::mul(
        &align::invert(&center(to)).unwrap(),
        &align::mul(h, &center(from)),
    )
}

/// Brennweite in Pixeln: Median der Schätzungen aus allen Bildpaaren
/// (unplausible verworfen); ohne Schätzung ein Normalobjektiv.
fn estimate_focal(fits: &[(usize, usize, align::Fit)], sizes: &[(usize, usize)]) -> f64 {
    let long = sizes.iter().map(|s| s.0.max(s.1)).max().unwrap_or(1) as f64;
    let mut fs: Vec<f64> = fits
        .iter()
        .filter_map(|(i, j, fit)| focals_from_h(&centered(&fit.h, sizes, *i, *j)))
        .filter(|f| *f > 0.3 * long && *f < 30.0 * long)
        .collect();
    fs.sort_by(|a, b| a.total_cmp(b));
    if fs.is_empty() {
        log::warn!("Panorama: Brennweite nicht schätzbar, nehme Normalobjektiv an");
        long * 1.2
    } else {
        fs[fs.len() / 2]
    }
}

/// Drehung jeder Kamera relativ zur Referenz, entlang des Spannbaums
/// verkettet (H = K·R·K⁻¹ ⇒ R = K⁻¹·H·K, auf die nächste Drehmatrix gebracht).
fn rotations(
    tree: &[Vec<(usize, Mat3)>],
    reference: usize,
    f: f64,
    sizes: &[(usize, usize)],
) -> (Vec<Mat3>, Vec<usize>) {
    let n = tree.len();
    let mut unaligned = Vec::new();
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
            let m = align::mul(&kinv, &align::mul(&centered(&h_cp, sizes, p, c), &k));
            let r_pc = orthonormalize(&m).unwrap_or_else(|| {
                log::warn!("Panorama: Drehung von Foto {} nicht bestimmbar", c + 1);
                unaligned.push(c);
                align::IDENTITY
            });
            rot[c] = Some(align::mul(&rp, &r_pc));
            queue.push_back(c);
        }
    }
    let rot = rot
        .into_iter()
        .map(|r| r.unwrap_or(align::IDENTITY))
        .collect();
    (rot, unaligned)
}

/// Helligkeitsausgleich: je Bild ein Faktor (`Cam::gain`), damit
/// Überlappungen gleich hell sind; gemessen auf den verkleinerten Bildern.
fn gain_compensation(
    cams: &mut [Cam],
    smalls: &[ImageF32],
    fits: &[(usize, usize, align::Fit)],
    f: f64,
) {
    let n = cams.len();
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
    // Schwache Regularisierung (zieht die Faktoren zu 1): hält das System
    // lösbar, auch für Bilder ohne messbare Überlappung
    for i in 0..n {
        a[i][i] += 1e-3;
        b[i] += 1e-3;
    }
    for (i, j, _) in fits {
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
            // Punkte sind nach Stärke sortiert, je Rasterzelle aber höchstens 4
            // (`detect`) – auch jede k-te liegt daher über das ganze Bild verteilt
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

/// Reihenfolge zum Einsetzen der Bilder: Breitensuche ab der Referenz, damit
/// jedes neue Bild an schon eingesetzte anschließt.
fn tree_order(tree: &Tree, reference: usize) -> Vec<usize> {
    let mut seen = vec![false; tree.len()];
    seen[reference] = true;
    let mut order = vec![reference];
    let mut k = 0;
    while k < order.len() {
        for &(c, _) in &tree[order[k]] {
            if !seen[c] {
                seen[c] = true;
                order.push(c);
            }
        }
        k += 1;
    }
    order
}

/// Weiche Maske eines Bildes auf dem verkleinerten Panorama (Ausschnitt).
struct SeamMask {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    data: Vec<f32>,
}

impl SeamMask {
    /// Bilinear an (x, y) in Koordinaten des verkleinerten Panoramas
    fn at(&self, x: f64, y: f64) -> f32 {
        let (x, y) = (x - self.x0 as f64, y - self.y0 as f64);
        if !(x >= 0.0 && y >= 0.0 && x <= (self.w - 1) as f64 && y <= (self.h - 1) as f64) {
            return 0.0;
        }
        let (xa, ya) = (x.floor() as usize, y.floor() as usize);
        let (xb, yb) = ((xa + 1).min(self.w - 1), (ya + 1).min(self.h - 1));
        let (tx, ty) = ((x - xa as f64) as f32, (y - ya as f64) as f32);
        let p = |xx: usize, yy: usize| self.data[yy * self.w + xx];
        let top = p(xa, ya) + (p(xb, ya) - p(xa, ya)) * tx;
        let bot = p(xa, yb) + (p(xb, yb) - p(xa, yb)) * tx;
        top + (bot - top) * ty
    }
}

/// Nähte zwischen den Bildern: je Bild eine weiche Maske, welche Pixel es
/// beiträgt. `scale`: Panoramapixel → Pixel der verkleinerten Fassung.
struct Seams {
    scale: f64,
    masks: Vec<Option<SeamMask>>,
}

/// Höchstens so viele Pixel hat die verkleinerte Fassung für die Nahtsuche
const SEAM_PIXELS: f64 = 12e6;

/// Abbild eines Bildes auf der verkleinerten Fassung (Ausschnitt `x0..x1`,
/// `y0..y1`; `None` außerhalb des Bildes).
struct Warped {
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    data: Vec<Option<[f32; 3]>>,
}

impl Warped {
    fn at(&self, x: usize, y: usize) -> Option<[f32; 3]> {
        self.data[(y - self.y0) * (self.x1 - self.x0) + (x - self.x0)]
    }
}

/// Naht durch eine Überlappung (Rechteck `ox0..=ox1`, `oy0..=oy1`): je Zeile
/// (senkrechte Naht) bzw. Spalte die Stelle des Schnitts.
struct Cut {
    vertical: bool,
    /// Das neue Bild liegt hinter dem Schnitt (rechts bzw. unten)
    after: bool,
    ox0: usize,
    oy0: usize,
    path: Vec<usize>,
}

impl Cut {
    /// Pixel der Überlappung geht an das neue Bild
    fn take(&self, x: usize, y: usize) -> bool {
        let (along, across, origin) = if self.vertical {
            (y - self.oy0, x, self.ox0)
        } else {
            (x - self.ox0, y, self.oy0)
        };
        let cut = origin + self.path[along];
        if self.after {
            across > cut
        } else {
            across <= cut
        }
    }
}

impl Seams {
    #[allow(clippy::too_many_arguments)]
    fn find(
        cams: &[Cam],
        smalls: &[ImageF32],
        f: f64,
        proj: &Proj,
        (u0, v0): (f64, f64),
        (out_w, out_h): (usize, usize),
        order: &[usize],
        cancel: &AtomicBool,
    ) -> Result<Seams> {
        // Auflösung der verkleinerten Bilder: ein kleines Pixel ≈ ein Pixel hier
        let k = smalls
            .iter()
            .zip(cams)
            .map(|(s, c)| s.width as f64 / c.w as f64)
            .fold(0.0, f64::max);
        let mut scale = (k * f / proj.f).min(1.0);
        let area = out_w as f64 * out_h as f64 * scale * scale;
        if area > SEAM_PIXELS {
            scale *= (SEAM_PIXELS / area).sqrt();
        }
        let lw = ((out_w as f64 * scale).ceil() as usize).max(1);
        let lh = ((out_h as f64 * scale).ceil() as usize).max(1);
        let cols: Vec<(f64, f64)> = (0..lw)
            .map(|x| proj.column(x as f64 / scale + u0))
            .collect();
        let rows: Vec<(f64, f64)> = (0..lh).map(|y| proj.row(y as f64 / scale + v0)).collect();

        // Zusammengesetztes Bild und Zuordnung (Bildnummer, −1 = leer)
        let mut comp = vec![[0f32; 3]; lw * lh];
        let mut label = vec![-1i32; lw * lh];
        for &i in order {
            check(cancel)?;
            let cam = &cams[i];
            let small = &smalls[i];
            let fs = small.width as f64 / cam.w as f64;
            let Some((bu0, bv0, bu1, bv1)) = cam.bounds(f, proj) else {
                continue;
            };
            let x0 = (((bu0 - u0) * scale).floor().max(0.0)) as usize;
            let x1 = ((((bu1 - u0) * scale).ceil() + 1.0) as usize).min(lw);
            let y0 = (((bv0 - v0) * scale).floor().max(0.0)) as usize;
            let y1 = ((((bv1 - v0) * scale).ceil() + 1.0) as usize).min(lh);
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            let data = (y0..y1)
                .into_par_iter()
                .flat_map_iter(|y| {
                    let (cols, rows) = (&cols, &rows);
                    (x0..x1).map(move |x| {
                        let ray = proj.inverse(cols[x], rows[y]);
                        let (px, py) = cam.to_pixel(f, ray)?;
                        align::sample(small, px * fs, py * fs).map(|p| p.map(|c| c * cam.gain))
                    })
                })
                .collect();
            let warped = Warped {
                x0,
                x1,
                y0,
                y1,
                data,
            };
            let cut = Self::cut(&warped, &comp, &label, lw);
            for y in y0..y1 {
                for x in x0..x1 {
                    let Some(p) = warped.at(x, y) else {
                        continue;
                    };
                    let idx = y * lw + x;
                    let take = label[idx] < 0 || cut.as_ref().is_some_and(|c| c.take(x, y));
                    if take {
                        label[idx] = i as i32;
                        comp[idx] = p;
                    }
                }
            }
        }
        drop(comp);

        // Je Bild eine weiche Maske: schmal weichgezeichnet (zweimal Kastenfilter)
        let radius = 2usize;
        let pad = 2 * radius + 1;
        let masks = (0..cams.len())
            .into_par_iter()
            .map(|i| {
                let me = i as i32;
                let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
                for y in 0..lh {
                    for x in 0..lw {
                        if label[y * lw + x] == me {
                            x0 = x0.min(x);
                            y0 = y0.min(y);
                            x1 = x1.max(x);
                            y1 = y1.max(y);
                        }
                    }
                }
                if x0 > x1 {
                    return None;
                }
                let (x0, y0) = (x0.saturating_sub(pad), y0.saturating_sub(pad));
                let (x1, y1) = ((x1 + pad).min(lw - 1), (y1 + pad).min(lh - 1));
                let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
                let mut data: Vec<f32> = (0..w * h)
                    .map(|k| f32::from(u8::from(label[(y0 + k / w) * lw + x0 + k % w] == me)))
                    .collect();
                for _ in 0..2 {
                    data = box_blur(&data, w, h, radius);
                }
                Some(SeamMask { x0, y0, w, h, data })
            })
            .collect();
        Ok(Seams { scale, masks })
    }

    /// Naht zwischen dem neuen Bild und dem schon Eingesetzten; `None` ohne
    /// Überlappung. Senkrecht bei hoher, waagerecht bei breiter Überlappung.
    fn cut(warped: &Warped, comp: &[[f32; 3]], label: &[i32], lw: usize) -> Option<Cut> {
        let (mut ox0, mut oy0, mut ox1, mut oy1) = (usize::MAX, usize::MAX, 0, 0);
        let (mut sx, mut sy, mut cnt) = (0.0, 0.0, 0.0);
        for y in warped.y0..warped.y1 {
            for x in warped.x0..warped.x1 {
                if warped.at(x, y).is_none() {
                    continue;
                }
                sx += x as f64;
                sy += y as f64;
                cnt += 1.0;
                if label[y * lw + x] >= 0 {
                    ox0 = ox0.min(x);
                    oy0 = oy0.min(y);
                    ox1 = ox1.max(x);
                    oy1 = oy1.max(y);
                }
            }
        }
        if ox0 > ox1 {
            return None;
        }
        let (ow, oh) = (ox1 - ox0 + 1, oy1 - oy0 + 1);
        let vertical = oh >= ow;
        // Auf welcher Seite das neue Bild liegt: sein Schwerpunkt
        let after = if vertical {
            sx / cnt > (ox0 + ox1) as f64 / 2.0
        } else {
            sy / cnt > (oy0 + oy1) as f64 / 2.0
        };
        // Kosten: Unterschied beider Bilder (Wurzel, grob wie wahrgenommen),
        // auf die Nachbarn ausgedehnt, damit die Naht Abstand zu Kanten hält.
        // Außerhalb der Überlappung teuer.
        let mut cost = vec![0f32; ow * oh];
        for y in oy0..=oy1 {
            for x in ox0..=ox1 {
                let idx = y * lw + x;
                cost[(y - oy0) * ow + (x - ox0)] = match (warped.at(x, y), label[idx] >= 0) {
                    (Some(p), true) => {
                        let q = comp[idx];
                        0.01 + (0..3)
                            .map(|k| (p[k].max(0.0).sqrt() - q[k].max(0.0).sqrt()).abs())
                            .sum::<f32>()
                    }
                    _ => 4.0,
                };
            }
        }
        let cost = dilate(&cost, ow, oh, 2);
        let path = if vertical {
            min_path(&cost, ow, oh)
        } else {
            min_path(&transpose(&cost, ow, oh), oh, ow)
        };
        Some(Cut {
            vertical,
            after,
            ox0,
            oy0,
            path,
        })
    }
}

/// Größter Wert im Umkreis `r` (Quadrat)
fn dilate(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = v[y * w + a..=y * w + b].iter().copied().fold(0.0, f32::max);
        }
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            out[y * w + x] = (a..=b).map(|yy| tmp[yy * w + x]).fold(0.0, f32::max);
        }
    }
    out
}

fn transpose(v: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            out[x * h + y] = v[y * w + x];
        }
    }
    out
}

/// Günstigster Weg von oben nach unten (je Zeile eine Spalte, benachbarte
/// Zeilen höchstens eine Spalte versetzt): Spalte je Zeile.
fn min_path(cost: &[f32], w: usize, h: usize) -> Vec<usize> {
    let mut acc = cost[..w].to_vec();
    let mut from = vec![0u32; w * h];
    let mut next = vec![0f32; w];
    for y in 1..h {
        for x in 0..w {
            let mut best = (acc[x], x);
            if x > 0 && acc[x - 1] < best.0 {
                best = (acc[x - 1], x - 1);
            }
            if x + 1 < w && acc[x + 1] < best.0 {
                best = (acc[x + 1], x + 1);
            }
            next[x] = best.0 + cost[y * w + x];
            from[y * w + x] = best.1 as u32;
        }
        std::mem::swap(&mut acc, &mut next);
    }
    let mut x = (0..w)
        .min_by(|&a, &b| acc[a].total_cmp(&acc[b]))
        .unwrap_or(0);
    let mut path = vec![0usize; h];
    for y in (0..h).rev() {
        path[y] = x;
        if y > 0 {
            x = from[y * w + x] as usize;
        }
    }
    path
}

/// Kastenfilter mit Radius `r` (getrennt waagerecht und senkrecht)
fn box_blur(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let n = (2 * r + 1) as f32;
    let mut tmp = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = v[y * w + a..=y * w + b].iter().sum::<f32>() / n;
        }
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            out[y * w + x] = (a..=b).map(|yy| tmp[yy * w + x]).sum::<f32>() / n;
        }
    }
    out
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

    /// Naher Gegenstand (Parallaxe): Ein roter Pfahl steht in zwei
    /// benachbarten Aufnahmen an verschiedenen Stellen der Szene. Mit Nähten
    /// erscheint er höchstens einmal deckend, nicht als halbdurchsichtiger
    /// Doppelgänger (früher: gewichtetes Mittel in der Überlappung).
    #[test]
    fn no_ghosts_from_parallax() {
        let scene = texture(2000, 700, 31);
        let (w, h, f) = (640usize, 480usize, 700.0f64);
        let mut shots = [
            shoot(&scene, f, -0.55, w, h, 1.0),
            shoot(&scene, f, 0.0, w, h, 1.0),
            shoot(&scene, f, 0.55, w, h, 1.0),
        ];
        // In der Überlappung von Aufnahme 0 und 1 (rechter Rand von 0, linker
        // von 1), im Panorama um ~40 px gegeneinander versetzt
        let post = |img: &mut ImageF32, x0: usize| {
            for y in 60..420 {
                for x in x0..x0 + 30 {
                    img.data[y * img.width + x] = [1.0, 0.0, 0.0];
                }
            }
        };
        post(&mut shots[0], 560);
        post(&mut shots[1], 150);
        let cancel = AtomicBool::new(false);
        let out = merge(
            3,
            &|i| Ok(shots[i].clone()),
            PanoOptions::default(),
            &|_, _, _| {},
            &cancel,
        )
        .unwrap();
        // „Rötung“: Hintergrund ≤ 0,15, Pfahl ≈ 1. Halb rot = Geisterbild
        let ghost = out
            .data
            .iter()
            .filter(|p| (0.3..0.75).contains(&(p[0] - (p[1] + p[2]) / 2.0)))
            .count();
        let solid = out
            .data
            .iter()
            .filter(|p| p[0] - (p[1] + p[2]) / 2.0 >= 0.75)
            .count();
        assert!(solid > 5000, "Pfahl fehlt ({solid} Pixel)");
        assert!(
            ghost < solid / 20,
            "Geisterbild: {ghost} halb rote Pixel neben {solid} deckenden"
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
