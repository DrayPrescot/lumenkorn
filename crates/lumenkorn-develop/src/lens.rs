//! Objektivkorrektur aus den Daten, die die Kamera selbst ins RAW schreibt:
//! Verzeichnung, Farbsäume (laterale chromatische Aberration) und
//! Vignettierung.
//!
//! - **Sony (ARW):** In der Raw-SubIFD stehen `DistortionCorrParams` (0x7037),
//!   `ChromaticAberrationCorrParams` (0x7035) und `VignettingCorrParams`
//!   (0x7032) – bis zu 16 Stützstellen über den Radius. Die Bedeutung der
//!   Werte (Skalierungen 2⁻¹⁴, 2⁻²¹, 2⁻¹³) ist von darktable und RawTherapee
//!   bekannt; hier unabhängig umgesetzt.
//! - **Panasonic (RW2):** `DistortionInfo` (0x119) in IFD0, 16 Wörter. Die
//!   Belegung ist nicht offiziell; nach den Notizen von Raphaël Rigo
//!   (github.com/trou/panasonic-rw2): Wort 7 schaltet die Korrektur ein,
//!   Wörter 8, 4, 11 sind a, b, c und Wort 5 die Skalierung (je /32768),
//!   `Ru = Rd + s·(a·Rd³ + b·Rd⁵ + c·Rd⁷)` vom Roh- zum korrigierten
//!   Radius. Die Normierung auf die halbe Diagonale und die Richtung sind
//!   gegen die eingebetteten Kamera-JPEGs von FZ80/FZ82 geprüft.
//! - **DNG:** die Opcodes `WarpRectilinear` (radialer Teil) und
//!   `FixVignetteRadial` aus der DNG-Spezifikation (OpcodeList 1–3). Die
//!   tangentialen Terme von `WarpRectilinear` sind bei Kameraobjektiven
//!   praktisch null und werden nicht angewendet.
//!
//! - **Lensfun:** für Objektive ohne eigene Daten, und für die
//!   Vignettierung bevorzugt (siehe [`crate::lensfun`]). Sony korrigiert die
//!   Vignettierung mit seinen Daten bewusst nur teilweise (wie die
//!   „Schattenaufhellung“ der Kamera), Lensfun misst den vollen Lichtabfall –
//!   wie die Profile von Lightroom. Verzeichnung und Farbsäume kommen dagegen
//!   bevorzugt aus der Kamera: Sie gelten genau für Brennweite und
//!   Fokusentfernung der Aufnahme.
//!
//! Alle Modelle werden auf eine gemeinsame Form gebracht: Abstand zur
//! optischen Mitte, normiert auf den größten Abstand zu einer Bildecke
//! (bei mittiger Mitte die halbe Diagonale), und je Kanal ein Faktor
//! `f(r)` – das Ausgabepixel im Abstand `r` holt seinen Wert aus dem
//! Abstand `r · f(r)`. Damit keine leeren Ränder entstehen, wird so weit
//! vergrößert, dass der kürzeste Rand gerade noch Bildinhalt trifft.

use crate::image_f32::ImageF32;
use rayon::prelude::*;
use std::path::Path;

/// Woher die Korrekturdaten stammen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LensSource {
    Sony,
    Panasonic,
    Dng,
    Lensfun,
}

/// `WarpRectilinear`: kr0…kr3 je Ebene und die optische Mitte (normiert).
pub type DngWarp = (Vec<[f64; 4]>, (f64, f64));
/// `FixVignetteRadial`: k0…k4 und die optische Mitte (normiert).
pub type DngVignette = ([f64; 5], (f64, f64));

/// Korrekturdaten eines Fotos.
#[derive(Debug, Clone, PartialEq)]
pub struct LensProfile {
    /// Optische Mitte, normiert auf das aufrecht gedrehte Bild
    center: (f32, f32),
    /// Stützstellen (Radius im Ausgabebild) und Faktor je Kanal (R, G, B)
    knots: Vec<f32>,
    factor: [Vec<f32>; 3],
    /// Stützstellen (Radius im Quellbild) und Verstärkung der Vignettierung
    vig_knots: Vec<f32>,
    vig_gain: Vec<f32>,
    /// Woher die einzelnen Korrekturen stammen (`None` = keine)
    pub distortion: Option<LensSource>,
    pub chromatic: Option<LensSource>,
    pub vignetting: Option<LensSource>,
    /// Objektiv laut Lensfun, falls dessen Profil genutzt wird
    pub lens_name: Option<String>,
}

/// Lineare Interpolation über Stützstellen, außerhalb konstant fortgesetzt.
fn spline(xs: &[f32], ys: &[f32], x: f32) -> f32 {
    if xs.is_empty() {
        return 1.0;
    }
    if x <= xs[0] {
        return ys[0];
    }
    for i in 1..xs.len() {
        if x <= xs[i] {
            let t = (x - xs[i - 1]) / (xs[i] - xs[i - 1]).max(1e-9);
            return ys[i - 1] + (ys[i] - ys[i - 1]) * t;
        }
    }
    ys[ys.len() - 1]
}

impl LensProfile {
    /// Sony: `dist`, `ca`, `vig` sind die Tag-Inhalte (erstes Element = Anzahl).
    pub fn from_sony(dist: &[i16], ca: &[i16], vig: &[i16]) -> Option<Self> {
        let n = *dist.first()? as usize;
        if !(2..=16).contains(&n)
            || dist.len() < n + 1
            || ca.first().map(|c| *c as usize) != Some(2 * n)
            || ca.len() < 2 * n + 1
            || vig.first().map(|v| *v as usize) != Some(n)
            || vig.len() < n + 1
        {
            return None;
        }
        // Stützstellen wie in darktables zweiter Fassung (gegen Kamera-JPEGs
        // abgeglichen): leicht nach außen versetzt
        let knots: Vec<f32> = (0..n).map(|i| (i as f32 + 0.5) / (n - 1) as f32).collect();
        let d: Vec<f32> = (0..n)
            .map(|i| dist[i + 1] as f32 * 2f32.powi(-14) + 1.0)
            .collect();
        let car: Vec<f32> = (0..n)
            .map(|i| d[i] * (ca[i + 1] as f32 * 2f32.powi(-21) + 1.0))
            .collect();
        let cab: Vec<f32> = (0..n)
            .map(|i| d[i] * (ca[n + i + 1] as f32 * 2f32.powi(-21) + 1.0))
            .collect();
        // Sony speichert die Abschwächung; das Pixel wird durch sie geteilt
        let vig_gain: Vec<f32> = (0..n)
            .map(|i| {
                let v = vig[i + 1] as f32 * 2f32.powi(-13);
                1.0 / 2f32.powf(0.5 - 2f32.powf(v - 1.0)).max(1e-3)
            })
            .collect();
        let distortion = dist[1..=n].iter().any(|v| *v != 0);
        let chromatic = ca[1..=2 * n].iter().any(|v| *v != 0);
        let vignetting = vig[1..=n].iter().any(|v| *v != 0);
        if !(distortion || chromatic || vignetting) {
            return None;
        }
        let src = |b: bool| b.then_some(LensSource::Sony);
        Some(Self {
            center: (0.5, 0.5),
            knots: knots.clone(),
            factor: [car, d, cab],
            vig_knots: knots,
            vig_gain,
            distortion: src(distortion),
            chromatic: src(chromatic),
            vignetting: src(vignetting),
            lens_name: None,
        })
    }

    /// Panasonic: die 16 Wörter von `DistortionInfo` (siehe Moduldoc). Nur
    /// Verzeichnung; Farbsäume und Vignettierung stehen nicht darin.
    pub fn from_panasonic(w: &[i16]) -> Option<Self> {
        const N: usize = 64;
        if w.len() < 16 || w[7] & 0xF != 1 {
            return None;
        }
        let q = |i: usize| w[i] as f64 / 32768.0;
        let s = 1.0 / (1.0 + q(5));
        let (a, b, c) = (s * q(8), s * q(4), s * q(11));
        if a == 0.0 && b == 0.0 && c == 0.0 {
            return None;
        }
        let ru = |rd: f64| rd + a * rd.powi(3) + b * rd.powi(5) + c * rd.powi(7);
        let dru =
            |rd: f64| 1.0 + 3.0 * a * rd.powi(2) + 5.0 * b * rd.powi(4) + 7.0 * c * rd.powi(6);
        let knots: Vec<f32> = (0..N).map(|i| i as f32 / (N - 1) as f32).collect();
        // Umkehrung (Newton): Ausgaberadius r ← Rohradius rd mit ru(rd) = r
        let mut f = Vec::with_capacity(N);
        for r in &knots {
            let r = *r as f64;
            if r == 0.0 {
                f.push((1.0 / dru(0.0)) as f32);
                continue;
            }
            let mut x = r;
            for _ in 0..30 {
                let d = dru(x);
                if d.abs() < 1e-6 {
                    break;
                }
                x -= (ru(x) - r) / d;
            }
            if !x.is_finite() || x <= 0.0 {
                return None;
            }
            f.push((x / r) as f32);
        }
        Some(Self {
            center: (0.5, 0.5),
            knots: knots.clone(),
            factor: [f.clone(), f.clone(), f],
            vig_knots: knots,
            vig_gain: vec![1.0; N],
            distortion: Some(LensSource::Panasonic),
            chromatic: None,
            vignetting: None,
            lens_name: None,
        })
    }

    /// DNG: radialer Teil von `WarpRectilinear` (je Ebene kr0…kr3, eine oder
    /// drei Ebenen) und `FixVignetteRadial` (k0…k4); Mitte normiert im
    /// unrotierten Bild, `orientation` dreht sie mit.
    pub fn from_dng(
        warp: Option<&DngWarp>,
        vignette: Option<DngVignette>,
        orientation: u16,
    ) -> Option<Self> {
        const N: usize = 64;
        let center = warp.map(|w| w.1).or(vignette.map(|v| v.1))?;
        let knots: Vec<f32> = (0..N).map(|i| i as f32 / (N - 1) as f32).collect();
        let poly = |k: &[f64; 4], r: f32| {
            let r2 = (r * r) as f64;
            (k[0] + r2 * (k[1] + r2 * (k[2] + r2 * k[3]))) as f32
        };
        let mut factor = [vec![1.0; N], vec![1.0; N], vec![1.0; N]];
        let mut distortion = false;
        let mut chromatic = false;
        if let Some((planes, _)) = warp
            && !planes.is_empty()
        {
            for (c, f) in factor.iter_mut().enumerate() {
                let k = planes[c.min(planes.len() - 1)];
                *f = knots.iter().map(|r| poly(&k, *r)).collect();
            }
            distortion = factor[1].iter().any(|v| (v - 1.0).abs() > 1e-5);
            chromatic = planes.len() >= 3
                && (0..N).any(|i| {
                    (factor[0][i] - factor[1][i]).abs() > 1e-6
                        || (factor[2][i] - factor[1][i]).abs() > 1e-6
                });
        }
        let mut vig_gain = vec![1.0; N];
        let mut vignetting = false;
        if let Some((k, _)) = vignette {
            vig_gain = knots
                .iter()
                .map(|r| {
                    let r2 = (r * r) as f64;
                    let mut g = 1.0;
                    let mut p = r2;
                    for c in k {
                        g += c * p;
                        p *= r2;
                    }
                    g.max(0.1) as f32
                })
                .collect();
            vignetting = vig_gain.iter().any(|g| (g - 1.0).abs() > 1e-4);
        }
        if !(distortion || chromatic || vignetting) {
            return None;
        }
        let src = |b: bool| b.then_some(LensSource::Dng);
        Some(Self {
            center: orient_point(orientation, (center.0 as f32, center.1 as f32)),
            knots: knots.clone(),
            factor,
            vig_knots: knots,
            vig_gain,
            distortion: src(distortion),
            chromatic: src(chromatic),
            vignetting: src(vignetting),
            lens_name: None,
        })
    }

    /// Liest die Korrekturdaten einer RAW-Datei: eingebettete (Sony ARW,
    /// DNG) und Lensfun, kombiniert wie im Moduldoc beschrieben; `None`, wenn
    /// es keine gibt. `orientation`: EXIF-Drehung, mit der das Bild
    /// aufgerichtet wird; `size`: Bildgröße nach dem Aufrichten.
    pub fn read(path: &Path, orientation: u16, size: (usize, usize)) -> Option<Self> {
        // Speicherabgebildet statt `fs::read`: Gelesen werden nur die Seiten
        // mit den Tags – nicht 40–100 MB RAW bei jedem Laden.
        let file = rawler::rawsource::RawSource::new(path).ok()?;
        let meta = rawler::get_decoder(&file).ok().and_then(|d| {
            d.raw_metadata(&file, &rawler::decoders::RawDecodeParams::default())
                .ok()
        });
        Self::read_from(&file, meta.as_ref(), orientation, size)
    }

    /// Wie [`Self::read`] mit schon geöffneter Datei und gelesenen Metadaten
    /// (spart zweimaliges Öffnen und Lesen beim Laden).
    pub fn read_from(
        file: &rawler::rawsource::RawSource,
        meta: Option<&rawler::decoders::RawMetadata>,
        orientation: u16,
        size: (usize, usize),
    ) -> Option<Self> {
        let embedded = Self::read_embedded(file.buf(), orientation);
        let shot = meta.map(shot);
        let (w, h) = if (5..=8).contains(&orientation) {
            (size.1, size.0)
        } else {
            size
        };
        let lensfun = shot.as_ref().and_then(|s| crate::lensfun::profile(s, w, h));
        if let Some(s) = &shot {
            log::debug!(
                "Objektiv: {} {} / {:?}, {:?} mm, f/{:?} → Lensfun: {}, Kameradaten: {}",
                s.camera_make,
                s.camera_model,
                s.lens_model,
                s.focal,
                s.aperture,
                lensfun.as_ref().map_or("–", |l| l.name.as_str()),
                embedded.is_some()
            );
        }
        Self::combine(embedded, lensfun, orientation)
    }

    /// Kameradaten und Lensfun zusammenführen: Verzeichnung und Farbsäume
    /// bevorzugt aus der Kamera, Vignettierung bevorzugt aus Lensfun.
    fn combine(
        embedded: Option<Self>,
        lensfun: Option<crate::lensfun::Profile>,
        orientation: u16,
    ) -> Option<Self> {
        let Some(l) = lensfun else { return embedded };
        let mut p = embedded.unwrap_or(Self {
            center: (0.5, 0.5),
            knots: vec![0.0, 1.0],
            factor: [vec![1.0; 2], vec![1.0; 2], vec![1.0; 2]],
            vig_knots: vec![0.0, 1.0],
            vig_gain: vec![1.0; 2],
            distortion: None,
            chromatic: None,
            vignetting: None,
            lens_name: None,
        });
        let mut used = false;
        if p.distortion.is_none()
            && let Some(f) = l.factor
        {
            // Farbsäume der Kamera merken, bevor die Faktoren ersetzt werden
            let camera_ca = p.chromatic.map(|_| (p.knots.clone(), p.factor.clone()));
            p.center = orient_point(orientation, l.center);
            p.knots = l.knots.clone();
            p.factor = f;
            p.distortion = Some(LensSource::Lensfun);
            match camera_ca {
                // Kamera meldet Farbsäume, aber keine Verzeichnung: deren
                // Verhältnis Rot/Grün bzw. Blau/Grün auf Lensfuns Verzeichnung
                // setzen – Farbsäume bleiben bevorzugt die der Kamera
                Some((k, cf)) => {
                    for c in [0, 2] {
                        p.factor[c] = p
                            .knots
                            .iter()
                            .zip(&p.factor[1])
                            .map(|(r, g)| {
                                g * spline(&k, &cf[c], *r) / spline(&k, &cf[1], *r).max(1e-6)
                            })
                            .collect();
                    }
                }
                None => p.chromatic = l.chromatic.then_some(LensSource::Lensfun),
            }
            used = true;
        }
        if let Some(g) = l.vig_gain {
            p.vig_knots = l.knots;
            p.vig_gain = g;
            p.vignetting = Some(LensSource::Lensfun);
            used = true;
        }
        if used {
            p.lens_name = Some(l.name);
        }
        (p.distortion.is_some() || p.chromatic.is_some() || p.vignetting.is_some()).then_some(p)
    }

    fn read_embedded(data: &[u8], orientation: u16) -> Option<Self> {
        let tiff = Tiff::new(data)?;
        let ifds = tiff.ifds();
        // Sony: in der Raw-SubIFD (DNG-Umwandlungen kopieren sie nach IFD0)
        for ifd in &ifds {
            if let (Some(d), Some(c), Some(v)) = (
                tiff.shorts(*ifd, 0x7037),
                tiff.shorts(*ifd, 0x7035),
                tiff.shorts(*ifd, 0x7032),
            ) {
                let s = |v: Vec<u16>| v.into_iter().map(|x| x as i16).collect::<Vec<_>>();
                if let Some(p) = Self::from_sony(&s(d), &s(c), &s(v)) {
                    return Some(p);
                }
            }
        }
        // Panasonic: DistortionInfo in IFD0
        if let Some(ifd0) = ifds.first()
            && let Some(bytes) = tiff.bytes(*ifd0, 0x119)
            && bytes.len() >= 32
        {
            let w: Vec<i16> = bytes[..32]
                .chunks(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect();
            if let Some(p) = Self::from_panasonic(&w) {
                return Some(p);
            }
        }
        // DNG: Opcodes aus allen Listen
        let mut warp = None;
        let mut vignette = None;
        for ifd in &ifds {
            for tag in [51008u16, 51009, 51022] {
                if let Some(bytes) = tiff.bytes(*ifd, tag) {
                    parse_opcodes(bytes, &mut warp, &mut vignette);
                }
            }
        }
        Self::from_dng(warp.as_ref(), vignette, orientation)
    }

    /// Kurzbeschreibung für die Oberfläche
    pub fn summary(&self) -> LensSummary {
        LensSummary {
            distortion: self.distortion,
            chromatic: self.chromatic,
            vignetting: self.vignetting,
            lens: self.lens_name.clone(),
        }
    }

    /// Vergrößerung, damit nach der Entzerrung kein leerer Rand bleibt
    /// (größter Faktor zwischen kürzestem Rand und Ecke).
    fn fill_scale(&self, w: f32, h: f32) -> f32 {
        let (cx, cy) = (self.center.0 * w, self.center.1 * h);
        let m = max_radius(cx, cy, w, h);
        let short = cx.min(w - cx).min(cy).min(h - cy) / m;
        let mut s: f32 = 0.0;
        for i in 0..=200 {
            let r = short + (1.0 - short) * i as f32 / 200.0;
            for f in &self.factor {
                s = s.max(spline(&self.knots, f, r));
            }
        }
        if s.is_finite() && s > 0.5 { s } else { 1.0 }
    }

    /// Wendet die Korrektur auf ein lineares Bild an (jede Auflösung – die
    /// Daten beziehen sich auf normierte Abstände).
    pub fn apply(&self, img: &ImageF32) -> ImageF32 {
        let (w, h) = (img.width, img.height);
        if w < 2 || h < 2 {
            return img.clone();
        }
        let (wf, hf) = (w as f32, h as f32);
        // Pixelmitten: Pixel i liegt bei i + 0.5
        let (cx, cy) = (self.center.0 * wf, self.center.1 * hf);
        let m = max_radius(cx, cy, wf, hf);
        let scale = self.fill_scale(wf, hf);
        // Tabellen über r² statt Interpolation je Pixel und Kanal: der Faktor
        // und die Vignettierung (am Quellradius des Grünkanals)
        const LUT: usize = 4096;
        let r2_max = (1.0 / (scale * scale)).max(1.0) * 1.01;
        let lut: Vec<[f32; 4]> = (0..LUT)
            .map(|i| {
                let r = (r2_max * i as f32 / (LUT - 1) as f32).sqrt();
                let f = [0, 1, 2].map(|c| spline(&self.knots, &self.factor[c], r));
                [
                    f[0],
                    f[1],
                    f[2],
                    spline(&self.vig_knots, &self.vig_gain, r * f[1]),
                ]
            })
            .collect();
        let inv = (LUT - 1) as f32 / r2_max;
        let inv_m2 = 1.0 / (m * m);
        let mut out = vec![[0f32; 3]; w * h];
        // Nur Vignettierung (kein Faktor ≠ 1): Jedes Pixel bleibt, wo es ist –
        // nur aufhellen statt dreimal bilinear abtasten
        if self.factor.iter().flatten().all(|f| *f == 1.0) {
            out.par_chunks_mut(w)
                .zip(img.data.par_chunks(w))
                .enumerate()
                .for_each(|(y, (row, src))| {
                    let dy = (y as f32 + 0.5 - cy) / scale;
                    for (x, (o, s)) in row.iter_mut().zip(src).enumerate() {
                        let dx = (x as f32 + 0.5 - cx) / scale;
                        let t = ((dx * dx + dy * dy) * inv_m2 * inv).min((LUT - 1) as f32);
                        let i = (t as usize).min(LUT - 2);
                        let a = t - i as f32;
                        let g = lut[i][3] + (lut[i + 1][3] - lut[i][3]) * a;
                        *o = s.map(|v| v * g);
                    }
                });
            return ImageF32 {
                width: w,
                height: h,
                data: out,
            };
        }
        out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let dy = (y as f32 + 0.5 - cy) / scale;
            for (x, o) in row.iter_mut().enumerate() {
                let dx = (x as f32 + 0.5 - cx) / scale;
                let t = ((dx * dx + dy * dy) * inv_m2 * inv).min((LUT - 1) as f32);
                let i = (t as usize).min(LUT - 2);
                let a = t - i as f32;
                let (l0, l1) = (&lut[i], &lut[i + 1]);
                let g = l0[3] + (l1[3] - l0[3]) * a;
                for (c, v) in o.iter_mut().enumerate() {
                    let f = l0[c] + (l1[c] - l0[c]) * a;
                    *v = sample(img, c, cx + dx * f - 0.5, cy + dy * f - 0.5) * g;
                }
            }
        });
        ImageF32 {
            width: w,
            height: h,
            data: out,
        }
    }
}

/// Was für ein Foto korrigiert werden kann (für die Oberfläche).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LensSummary {
    pub distortion: Option<LensSource>,
    pub chromatic: Option<LensSource>,
    pub vignetting: Option<LensSource>,
    /// Objektiv laut Lensfun
    pub lens: Option<String>,
}

/// Kamera, Objektiv, Brennweite und Blende aus den Metadaten (rawler).
fn shot(md: &rawler::decoders::RawMetadata) -> crate::lensfun::Shot {
    let r = |v: Option<rawler::formats::tiff::Rational>| {
        v.filter(|r| r.d != 0).map(|r| r.n as f64 / r.d as f64)
    };
    // Ohne Objektivnamen im EXIF (Nikon, Pentax …): rawlers Objektivliste
    let from_db = md.lens.as_ref();
    let lens_model = md
        .exif
        .lens_model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .or_else(|| from_db.map(|l| l.lens_model.clone()));
    let lens_make = md
        .exif
        .lens_make
        .clone()
        .or_else(|| from_db.map(|l| l.lens_make.clone()));
    crate::lensfun::Shot {
        camera_make: md.make.clone(),
        camera_model: md.model.clone(),
        lens_make,
        lens_model,
        focal: r(md.exif.focal_length),
        aperture: r(md.exif.fnumber),
    }
}

fn max_radius(cx: f32, cy: f32, w: f32, h: f32) -> f32 {
    let d = |x: f32, y: f32| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
    d(0.0, 0.0)
        .max(d(w, 0.0))
        .max(d(0.0, h))
        .max(d(w, h))
        .max(1.0)
}

/// Bilinear, Koordinaten am Rand begrenzt.
fn sample(img: &ImageF32, c: usize, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, (img.width - 1) as f32);
    let y = y.clamp(0.0, (img.height - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(img.width - 1), (y0 + 1).min(img.height - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let p = |x: usize, y: usize| img.data[y * img.width + x][c];
    let a = p(x0, y0) + (p(x1, y0) - p(x0, y0)) * tx;
    let b = p(x0, y1) + (p(x1, y1) - p(x0, y1)) * tx;
    a + (b - a) * ty
}

/// Normierter Punkt im unrotierten Bild → im aufrecht gedrehten (wie
/// `geometry::apply_exif_orientation`).
fn orient_point(o: u16, (x, y): (f32, f32)) -> (f32, f32) {
    match o {
        2 => (1.0 - x, y),
        3 => (1.0 - x, 1.0 - y),
        4 => (x, 1.0 - y),
        5 => (y, x),
        6 => (1.0 - y, x),
        7 => (1.0 - y, 1.0 - x),
        8 => (y, 1.0 - x),
        _ => (x, y),
    }
}

/// DNG-Opcode-Liste (big-endian): Anzahl, dann je Opcode ID, Version,
/// Flags, Länge und Daten.
fn parse_opcodes(b: &[u8], warp: &mut Option<DngWarp>, vignette: &mut Option<DngVignette>) {
    let u32_at = |o: usize| {
        b.get(o..o + 4)
            .map(|s| u32::from_be_bytes(s.try_into().unwrap()))
    };
    let f64_at = |o: usize| {
        b.get(o..o + 8)
            .map(|s| f64::from_be_bytes(s.try_into().unwrap()))
    };
    let Some(count) = u32_at(0) else { return };
    let mut o = 4;
    for _ in 0..count.min(64) {
        let (Some(id), Some(len)) = (u32_at(o), u32_at(o + 12)) else {
            return;
        };
        let d = o + 16;
        let next = d + len as usize;
        if next > b.len() {
            return;
        }
        match id {
            // WarpRectilinear
            1 if warp.is_none() => {
                if let Some(planes) = u32_at(d).filter(|p| (1..=4).contains(p)) {
                    let mut ks = Vec::new();
                    for p in 0..planes as usize {
                        let base = d + 4 + p * 48;
                        let k = [0, 1, 2, 3].map(|i| f64_at(base + i * 8).unwrap_or(0.0));
                        ks.push(k);
                    }
                    let cb = d + 4 + planes as usize * 48;
                    if let (Some(cx), Some(cy)) = (f64_at(cb), f64_at(cb + 8))
                        && ks.iter().flatten().all(|v| v.is_finite())
                        && cx.is_finite()
                        && cy.is_finite()
                    {
                        *warp = Some((ks, (cx, cy)));
                    }
                }
            }
            // FixVignetteRadial
            3 if vignette.is_none() => {
                let k = [0, 1, 2, 3, 4].map(|i| f64_at(d + i * 8).unwrap_or(0.0));
                if let (Some(cx), Some(cy)) = (f64_at(d + 40), f64_at(d + 48))
                    && k.iter().all(|v| v.is_finite())
                    && cx.is_finite()
                    && cy.is_finite()
                {
                    *vignette = Some((k, (cx, cy)));
                }
            }
            _ => {}
        }
        o = next;
    }
}

/// Minimaler TIFF-Leser: IFD0, Folge-IFDs und SubIFDs, einzelne Tags.
struct Tiff<'a> {
    b: &'a [u8],
    le: bool,
}

impl<'a> Tiff<'a> {
    fn new(b: &'a [u8]) -> Option<Self> {
        let le = match b.get(0..4)? {
            // Panasonic RW2 trägt 0x55 statt 42
            [b'I', b'I', 42 | 0x55, 0] => true,
            [b'M', b'M', 0, 42] => false,
            _ => return None,
        };
        Some(Self { b, le })
    }
    fn u16(&self, o: usize) -> Option<u16> {
        let s: [u8; 2] = self.b.get(o..o + 2)?.try_into().ok()?;
        Some(if self.le {
            u16::from_le_bytes(s)
        } else {
            u16::from_be_bytes(s)
        })
    }
    fn u32(&self, o: usize) -> Option<u32> {
        let s: [u8; 4] = self.b.get(o..o + 4)?.try_into().ok()?;
        Some(if self.le {
            u32::from_le_bytes(s)
        } else {
            u32::from_be_bytes(s)
        })
    }
    /// Alle IFD-Offsets: Kette ab IFD0 und deren SubIFDs (eine Ebene tief).
    fn ifds(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut next = self.u32(4).map(|v| v as usize);
        while let Some(o) = next.filter(|o| *o >= 8 && !out.contains(o) && out.len() < 16) {
            out.push(o);
            next = self
                .u16(o)
                .and_then(|n| self.u32(o + 2 + n as usize * 12))
                .map(|v| v as usize)
                .filter(|v| *v != 0);
        }
        let mut subs = Vec::new();
        for ifd in &out {
            if let Some((typ, count, at)) = self.entry(*ifd, 330) {
                for i in 0..count.min(8) as usize {
                    let v = if typ == 3 {
                        self.u16(at + i * 2).map(|v| v as usize)
                    } else {
                        self.u32(at + i * 4).map(|v| v as usize)
                    };
                    if let Some(v) = v.filter(|v| *v >= 8 && !out.contains(v)) {
                        subs.push(v);
                    }
                }
            }
        }
        out.extend(subs);
        out
    }
    /// (Typ, Anzahl, Offset der Werte)
    fn entry(&self, ifd: usize, tag: u16) -> Option<(u16, u32, usize)> {
        let n = self.u16(ifd)? as usize;
        for i in 0..n.min(1000) {
            let e = ifd + 2 + i * 12;
            if self.u16(e)? != tag {
                continue;
            }
            let typ = self.u16(e + 2)?;
            let count = self.u32(e + 4)?;
            let size = match typ {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                5 | 10 | 12 => 8,
                _ => return None,
            } * count as usize;
            let at = if size <= 4 {
                e + 8
            } else {
                self.u32(e + 8)? as usize
            };
            if at.checked_add(size)? > self.b.len() {
                return None;
            }
            return Some((typ, count, at));
        }
        None
    }
    fn shorts(&self, ifd: usize, tag: u16) -> Option<Vec<u16>> {
        let (typ, count, at) = self.entry(ifd, tag)?;
        if typ != 3 && typ != 8 {
            return None;
        }
        (0..count.min(64) as usize)
            .map(|i| self.u16(at + i * 2))
            .collect()
    }
    fn bytes(&self, ifd: usize, tag: u16) -> Option<&'a [u8]> {
        let (typ, count, at) = self.entry(ifd, tag)?;
        if typ != 7 && typ != 1 {
            return None;
        }
        self.b.get(at..at + count as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Werte aus einem echten ARW (ILCA-68, DT 35mm F1.8 SAM bei f/4)
    fn sony() -> LensProfile {
        let dist = [
            11, 4, 0, -13, -29, -53, -81, -117, -156, -204, -254, -312, 0, 0, 0, 0, 0,
        ];
        let ca = [
            22, 640, 640, 640, 512, 512, 512, 384, 384, 384, 384, 256, 0, -128, -128, -128, 0, 0,
            128, 256, 256, 128, -256, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let vig = [
            11, 0, 5, 32, 70, 119, 178, 244, 300, 390, 545, 800, 0, 0, 0, 0, 0,
        ];
        LensProfile::from_sony(&dist, &ca, &vig).unwrap()
    }

    #[test]
    fn vignetting_only_matches_full_resampling() {
        let n = 8;
        let knots: Vec<f32> = (0..n).map(|i| i as f32 / (n - 1) as f32).collect();
        let p = LensProfile {
            center: (0.5, 0.5),
            knots: knots.clone(),
            factor: [vec![1.0; n], vec![1.0; n], vec![1.0; n]],
            vig_knots: knots.clone(),
            vig_gain: knots.iter().map(|r| 1.0 + 0.5 * r * r).collect(),
            distortion: None,
            chromatic: None,
            vignetting: Some(LensSource::Lensfun),
            lens_name: None,
        };
        let img = ImageF32::from_fn(40, 30, |x, y| [x as f32 * 0.01, y as f32 * 0.02, 0.3]);
        let fast = p.apply(&img);
        // Dieselbe Rechnung mit einem Faktor knapp über 1 nimmt den langsamen Weg
        let mut slow = p.clone();
        slow.factor[1][0] = 1.0 + 1e-7;
        let slow = slow.apply(&img);
        for (a, b) in fast.data.iter().zip(&slow.data) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 1e-4, "{a:?} {b:?}");
            }
        }
        // Mitte bleibt, Ecke wird heller
        assert!(fast.get(0, 0)[2] > 0.3 * 1.3);
    }

    #[test]
    fn camera_ca_survives_lensfun_distortion() {
        // Kamera: Farbsäume, aber keine Verzeichnung
        let dist = [11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let ca = [
            22, 640, 640, 640, 512, 512, 512, 384, 384, 384, 384, 256, 0, -128, -128, -128, 0, 0,
            128, 256, 256, 128, -256, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let vig = [11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let cam = LensProfile::from_sony(&dist, &ca, &vig).unwrap();
        assert!(cam.distortion.is_none() && cam.chromatic.is_some());
        let n = 64;
        let knots: Vec<f32> = (0..n).map(|i| i as f32 / (n - 1) as f32).collect();
        // Lensfun: leichte Tonne, ohne Farbsäume
        let green: Vec<f32> = knots.iter().map(|r| 1.0 - 0.02 * r * r).collect();
        let l = crate::lensfun::Profile {
            name: "Test".into(),
            center: (0.5, 0.5),
            knots: knots.clone(),
            factor: Some([green.clone(), green.clone(), green.clone()]),
            chromatic: false,
            vig_gain: None,
        };
        let ratio = |p: &LensProfile, c: usize, r: f32| {
            spline(&p.knots, &p.factor[c], r) / spline(&p.knots, &p.factor[1], r)
        };
        let p = LensProfile::combine(Some(cam.clone()), Some(l), 1).unwrap();
        assert_eq!(p.distortion, Some(LensSource::Lensfun));
        assert_eq!(p.chromatic, Some(LensSource::Sony));
        for r in [0.2f32, 0.5, 0.8] {
            // Grün folgt Lensfun, Rot und Blau behalten das Verhältnis der Kamera
            assert!((spline(&p.knots, &p.factor[1], r) - (1.0 - 0.02 * r * r)).abs() < 1e-3);
            for c in [0, 2] {
                assert!(
                    (ratio(&p, c, r) - ratio(&cam, c, r)).abs() < 2e-4,
                    "Kanal {c}, r {r}"
                );
            }
        }
    }

    #[test]
    fn sony_barrel_distortion_pulls_the_corners_inward() {
        let p = sony();
        assert!(p.distortion.is_some() && p.chromatic.is_some() && p.vignetting.is_some());
        // Tonnenförmig: außen holt das Pixel seinen Wert weiter innen
        let f = |r| spline(&p.knots, &p.factor[1], r);
        assert!((f(0.0) - 1.0).abs() < 1e-3);
        assert!(f(1.0) < 0.985 && f(1.0) > 0.97, "{}", f(1.0));
        // Vignettierung hellt die Ecke etwas auf, die Mitte nicht
        let g = |r| spline(&p.vig_knots, &p.vig_gain, r);
        assert!((g(0.0) - 1.0).abs() < 1e-3 && g(1.0) > 1.01, "{}", g(1.0));
    }

    #[test]
    fn a_straight_line_bent_by_the_lens_comes_out_straight() {
        // Bild mit einer waagerechten Linie, die das Objektiv tonnenförmig
        // gebogen hat (Kehrwert der Korrektur): nach der Korrektur gerade.
        let p = sony();
        let (w, h) = (600usize, 400usize);
        let (cx, cy) = (300.0f32, 200.0f32);
        let m = (cx * cx + cy * cy).sqrt();
        let scale = p.fill_scale(w as f32, h as f32);
        let line_y = 60.0f32;
        // Für jede Spalte: wo liegt die Linie im verzerrten Bild?
        let bent = |x: f32| {
            let (dx, dy) = ((x - cx) / scale, (line_y - cy) / scale);
            let r = (dx * dx + dy * dy).sqrt() / m;
            let f = spline(&p.knots, &p.factor[1], r);
            cy + dy * f
        };
        let img = ImageF32 {
            width: w,
            height: h,
            data: (0..w * h)
                .map(|i| {
                    let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                    let v = (1.0 - (y - bent(x)).abs()).max(0.0);
                    [v, v, v]
                })
                .collect(),
        };
        assert!(
            (bent(5.0) - bent(300.0)).abs() > 1.5,
            "Testbild ist wirklich gebogen"
        );
        let out = p.apply(&img);
        // Schwerpunkt der Linie je Spalte: überall gleich
        let row_of = |x: usize| {
            let (mut s, mut n) = (0.0, 0.0);
            for y in 0..h {
                let v = out.data[y * w + x][1];
                s += v * (y as f32 + 0.5);
                n += v;
            }
            s / n
        };
        for x in [10, 150, 300, 450, 590] {
            assert!(
                (row_of(x) - line_y).abs() < 0.35,
                "Spalte {x}: {}",
                row_of(x)
            );
        }
    }

    #[test]
    fn corrected_image_has_no_empty_border() {
        let p = sony();
        let img = ImageF32 {
            width: 300,
            height: 200,
            data: vec![[0.5; 3]; 300 * 200],
        };
        let out = p.apply(&img);
        // Gleichmäßige Fläche: nur die Vignettierung ändert sie, nichts wird schwarz
        assert!(out.data.iter().all(|p| p[1] >= 0.5 && p[1] < 0.53));
        assert!(
            out.data[0][1] > out.data[100 * 300 + 150][1],
            "Ecke aufgehellt"
        );
    }

    /// Wörter aus einem echten RW2 (DC-FZ80, 9 mm)
    const FZ80: [i16; 16] = [
        -29481, 9426, 36, 0, -5111, 0, 411, 1, 2716, 277, 155, 2468, 3060, 777, -12839, -23076,
    ];

    #[test]
    fn panasonic_distortion_inverts_the_camera_formula() {
        let p = LensProfile::from_panasonic(&FZ80).expect("Profil");
        assert_eq!(p.distortion, Some(LensSource::Panasonic));
        assert_eq!(p.chromatic, None);
        let (a, b, c) = (2716.0 / 32768.0, -5111.0 / 32768.0, 2468.0 / 32768.0);
        for (i, r) in p.knots.iter().enumerate().skip(1) {
            // Der Rohradius, aus dem gelesen wird, ergibt mit der Formel der
            // Kamera wieder den Ausgaberadius
            let rd = (*r * p.factor[1][i]) as f64;
            let ru = rd + a * rd.powi(3) + b * rd.powi(5) + c * rd.powi(7);
            assert!((ru - *r as f64).abs() < 1e-5, "r={r}: {ru}");
        }
        // Korrektur ausgeschaltet (Wort 7) oder zu kurz: keine Daten
        let mut off = FZ80;
        off[7] = 0;
        assert!(LensProfile::from_panasonic(&off).is_none());
        assert!(LensProfile::from_panasonic(&FZ80[..8]).is_none());
    }

    #[test]
    fn rw2_header_and_tag_are_read() {
        // Kleines RW2: Kopf „IIU“, IFD0 mit 0x119 (32 Byte, Typ UNDEFINED)
        let mut b = b"IIU\0\x08\0\0\0".to_vec();
        b.extend(1u16.to_le_bytes());
        b.extend(0x119u16.to_le_bytes());
        b.extend(7u16.to_le_bytes());
        b.extend(32u32.to_le_bytes());
        b.extend(26u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        for w in FZ80 {
            b.extend(w.to_le_bytes());
        }
        let p = LensProfile::read_embedded(&b, 1).expect("aus dem RW2");
        assert_eq!(p.distortion, Some(LensSource::Panasonic));
    }

    #[test]
    fn dng_opcodes_are_parsed() {
        // OpcodeList mit WarpRectilinear (1 Ebene) und FixVignetteRadial
        let mut b = Vec::new();
        b.extend(2u32.to_be_bytes());
        let mut op = |id: u32, data: Vec<u8>| {
            b.extend(id.to_be_bytes());
            b.extend(0x01030000u32.to_be_bytes());
            b.extend(0u32.to_be_bytes());
            b.extend((data.len() as u32).to_be_bytes());
            b.extend(data);
        };
        let f = |v: &[f64]| v.iter().flat_map(|x| x.to_be_bytes()).collect::<Vec<u8>>();
        let mut warp = 1u32.to_be_bytes().to_vec();
        warp.extend(f(&[1.0, -0.02, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5]));
        op(1, warp);
        op(3, f(&[0.3, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5]));
        let (mut w, mut v) = (None, None);
        parse_opcodes(&b, &mut w, &mut v);
        let (planes, c) = w.as_ref().unwrap();
        assert_eq!((planes[0][1], *c), (-0.02, (0.5, 0.5)));
        assert_eq!(v.unwrap().0[0], 0.3);
        let p = LensProfile::from_dng(w.as_ref(), v, 1).unwrap();
        assert!(p.distortion.is_some() && p.vignetting.is_some() && p.chromatic.is_none());
        assert!((spline(&p.vig_knots, &p.vig_gain, 1.0) - 1.3).abs() < 1e-3);
    }

    #[test]
    fn dng_center_turns_with_the_photo() {
        assert_eq!(orient_point(6, (0.4, 0.5)), (0.5, 0.4));
        assert_eq!(orient_point(3, (0.4, 0.5)), (0.6, 0.5));
    }

    #[test]
    fn files_without_data_have_no_profile() {
        let p = std::env::temp_dir().join(format!("lw-lens-{}.bin", std::process::id()));
        std::fs::write(&p, b"keine TIFF-Datei").unwrap();
        assert!(LensProfile::read(&p, 1, (10, 10)).is_none());
        let _ = std::fs::remove_file(&p);
    }
}
