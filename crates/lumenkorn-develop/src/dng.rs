//! Verlustbehaftetes DNG (DNG 1.4, Kompression „Lossy JPEG“ wie Lightrooms
//! „Verlustbehaftete Komprimierung“): lineare RGB-Daten in JPEG-Kacheln mit
//! 8 Bit je Kanal, die sRGB-Kurve steckt als Linearisierungstabelle in der
//! Datei. RAW-Programme lesen das wie ein RAW mit fertigem Demosaicing.

use crate::color::linear_to_srgb;
use crate::error::Result;
use crate::image_f32::ImageF32;
use crate::output::to_rgb8;
use crate::wb::XYZ_TO_SRGB;
use rayon::prelude::*;

const TILE: usize = 256;
/// Leicht verlustbehaftet: kaum sichtbar, etwa ein Viertel eines verlustfreien DNG
const QUALITY: u8 = 92;
/// Kantenlänge des eingebetteten Vorschaubilds (IFD0)
const THUMB: usize = 256;
/// Große JPEG-Vorschau wie bei Adobe: Viele Programme (Windows, Bildbetrachter)
/// zeigen von einem DNG nur sie
const PREVIEW: usize = 2560;
const PREVIEW_QUALITY: u8 = 85;

/// XYZ (D65) → Rec.2020 bzw. lineares sRGB (`wb::XYZ_TO_SRGB`): `ColorMatrix1`
/// der DNG-Datei. Der „Kamera-Farbraum“ ist hier der Farbraum der
/// gespeicherten Daten.
const XYZ_TO_REC2020: [[f64; 3]; 3] = [
    [1.7166512, -0.3556708, -0.2533663],
    [-0.6666844, 1.6164812, 0.0157685],
    [0.0176399, -0.0427706, 0.9421031],
];
/// Lineares Rec.709 → Rec.2020 (beide D65)
const REC709_TO_REC2020: [[f32; 3]; 3] = [
    [0.627404, 0.329283, 0.043313],
    [0.069097, 0.919540, 0.011362],
    [0.016391, 0.088013, 0.895595],
];
/// Darüber wird abgeschnitten (16 = vier Blenden über Weiß)
const MAX_PEAK: f32 = 16.0;

/// Was ins DNG kommt
pub enum DngContent<'a> {
    /// Fertig entwickeltes Bild (sRGB-kodiert, 0…1) – sieht in anderen
    /// Programmen so aus wie in Lumenkorn
    Developed(&'a ImageF32),
    /// Szenenbezogene Daten ohne Bearbeitung (lineares Rec.709, Werte über 1
    /// möglich) – zum Weiterentwickeln in anderen Programmen
    Scene(&'a ImageF32),
}

/// Schreibt das DNG in den Speicher.
pub fn encode_lossy(content: DngContent) -> Result<Vec<u8>> {
    let developed = matches!(content, DngContent::Developed(_));
    let (img, codes, peak) = match content {
        DngContent::Developed(img) => (img, to_rgb8(img).into_raw(), 1.0),
        DngContent::Scene(img) => {
            let wide: Vec<[f32; 3]> = img
                .data
                .par_iter()
                .map(|c| mul(&REC709_TO_REC2020, *c).map(|v| v.max(0.0)))
                .collect();
            let peak = wide
                .par_iter()
                .map(|c| c[0].max(c[1]).max(c[2]))
                .reduce(|| 0.0, f32::max)
                .clamp(1.0, MAX_PEAK);
            let mut codes = vec![0u8; wide.len() * 3];
            codes
                .par_chunks_mut(3)
                .zip(wide.par_iter())
                .for_each(|(o, c)| {
                    for k in 0..3 {
                        o[k] = (linear_to_srgb(c[k] / peak) * 255.0 + 0.5) as u8;
                    }
                });
            (img, codes, peak)
        }
    };
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return Err(crate::error::Error::Params("leeres Bild".into()));
    }

    // Kacheln parallel kodieren; Randkacheln mit wiederholtem Randpixel auffüllen
    let (tx, ty) = (w.div_ceil(TILE), h.div_ceil(TILE));
    let tiles: Vec<Vec<u8>> = (0..tx * ty)
        .into_par_iter()
        .map(|i| {
            let (x0, y0) = ((i % tx) * TILE, (i / tx) * TILE);
            let mut buf = vec![0u8; TILE * TILE * 3];
            for y in 0..TILE {
                let sy = (y0 + y).min(h - 1);
                for x in 0..TILE {
                    let sx = (x0 + x).min(w - 1);
                    let s = (sy * w + sx) * 3;
                    buf[(y * TILE + x) * 3..][..3].copy_from_slice(&codes[s..s + 3]);
                }
            }
            let mut out = Vec::new();
            let mut enc = jpeg_encoder::Encoder::new(&mut out, QUALITY);
            enc.set_sampling_factor(jpeg_encoder::SamplingFactor::F_1_1);
            enc.encode(&buf, TILE as u16, TILE as u16, jpeg_encoder::ColorType::Rgb)?;
            Ok(out)
        })
        .collect::<Result<_>>()?;

    // Vorschauen (sRGB): das fertige Bild bzw. beim Original Lumenkorns
    // Standardentwicklung – lineare Daten sähen flau aus
    let preview = if developed {
        img.downscale_to(PREVIEW)
    } else {
        let small = img.downscale_to(PREVIEW);
        let src = crate::Source {
            kind: crate::SourceKind::Raw,
            full_width: small.width,
            full_height: small.height,
            image: small,
            lens: None,
            as_shot_kelvin: None,
        };
        crate::render(&src, &crate::DevelopParams::default())
    };
    let preview_jpeg = crate::output::encode_jpeg(&preview, PREVIEW_QUALITY)?;
    // Kleines Vorschaubild für Dateimanager (unkomprimiertes RGB, IFD0)
    let small = preview.downscale_to(THUMB);
    let thumb = to_rgb8(&small).into_raw();

    let mut out = b"II*\0\0\0\0\0".to_vec();
    let thumb_off = out.len() as u32;
    out.extend(&thumb);
    let mut offsets = Vec::with_capacity(tiles.len());
    let mut counts = Vec::with_capacity(tiles.len());
    for t in &tiles {
        if out.len() % 2 == 1 {
            out.push(0);
        }
        offsets.push(out.len() as u32);
        counts.push(t.len() as u32);
        out.extend(t);
    }
    if out.len() % 2 == 1 {
        out.push(0);
    }
    let preview_off = out.len() as u32;
    out.extend(&preview_jpeg);

    // Hauptbild (SubIFD)
    let table: Vec<u16> = (0..256)
        .map(|i| (crate::color::srgb_to_linear(i as f32 / 255.0) * 65535.0).round() as u16)
        .collect();
    let mut raw = Ifd::default();
    raw.long(254, &[0]);
    raw.long(256, &[w as u32]);
    raw.long(257, &[h as u32]);
    raw.short(258, &[8, 8, 8]);
    raw.short(259, &[34892]);
    raw.short(262, &[34892]);
    raw.short(277, &[3]);
    raw.short(284, &[1]);
    raw.long(322, &[TILE as u32]);
    raw.long(323, &[TILE as u32]);
    raw.long(324, &offsets);
    raw.long(325, &counts);
    raw.short(50712, &table);
    raw.long(50717, &[65535, 65535, 65535]);
    let sub_off = raw.write(&mut out);

    // Große Vorschau (zweites SubIFD), aufgebaut wie bei Adobe
    let mut prev = Ifd::default();
    prev.long(254, &[1]);
    prev.long(256, &[preview.width as u32]);
    prev.long(257, &[preview.height as u32]);
    prev.short(258, &[8, 8, 8]);
    prev.short(259, &[7]); // JPEG
    prev.short(262, &[6]); // YCbCr
    prev.long(273, &[preview_off]);
    prev.short(277, &[3]);
    prev.long(278, &[preview.height as u32]);
    prev.long(279, &[preview_jpeg.len() as u32]);
    prev.short(284, &[1]);
    prev.long(50970, &[2]); // PreviewColorSpace: sRGB
    let prev_off = prev.write(&mut out);

    // IFD0: Vorschaubild und DNG-Angaben
    let (tw, th) = (small.width as u32, small.height as u32);
    let matrix = if developed {
        XYZ_TO_SRGB
    } else {
        XYZ_TO_REC2020
    };
    let mut ifd0 = Ifd::default();
    ifd0.long(254, &[1]);
    ifd0.long(256, &[tw]);
    ifd0.long(257, &[th]);
    ifd0.short(258, &[8, 8, 8]);
    ifd0.short(259, &[1]);
    ifd0.short(262, &[2]);
    ifd0.ascii(271, "Lumenkorn");
    ifd0.ascii(272, "Lumenkorn");
    ifd0.long(273, &[thumb_off]);
    ifd0.short(274, &[1]);
    ifd0.short(277, &[3]);
    ifd0.long(278, &[th]);
    ifd0.long(279, &[thumb.len() as u32]);
    ifd0.short(284, &[1]);
    ifd0.ascii(305, &format!("Lumenkorn {}", env!("CARGO_PKG_VERSION")));
    ifd0.long(330, &[sub_off, prev_off]);
    ifd0.bytes(50706, &[1, 4, 0, 0]);
    ifd0.bytes(50707, &[1, 4, 0, 0]);
    ifd0.ascii(50708, "Lumenkorn");
    ifd0.srational(
        50721,
        &matrix
            .iter()
            .flatten()
            .map(|v| ((v * 10000.0).round() as i32, 10000))
            .collect::<Vec<_>>(),
    );
    ifd0.rational(50728, &[(1, 1), (1, 1), (1, 1)]);
    // Werte wurden durch `peak` geteilt – so stimmt die Helligkeit wieder
    ifd0.srational(50730, &[((peak.log2() * 10000.0).round() as i32, 10000)]);
    ifd0.short(50778, &[21]); // D65
    ifd0.ascii(50936, "Lumenkorn");
    if developed {
        // Fertig entwickelt: keine zusätzliche Gradationskurve
        ifd0.float(50940, &[0.0, 0.0, 1.0, 1.0]);
    }
    ifd0.long(51110, &[1]); // DefaultBlackRender: None
    let ifd0_off = ifd0.write(&mut out);
    out[4..8].copy_from_slice(&ifd0_off.to_le_bytes());
    Ok(out)
}

fn mul(m: &[[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|r| m[r][0] * c[0] + m[r][1] * c[1] + m[r][2] * c[2])
}

/// Ein TIFF-Verzeichnis (little endian). Einträge werden nach Tag sortiert
/// geschrieben, längere Werte folgen direkt dahinter.
#[derive(Default)]
struct Ifd {
    entries: Vec<(u16, u16, u32, Vec<u8>)>,
}

impl Ifd {
    fn add(&mut self, tag: u16, typ: u16, count: usize, data: Vec<u8>) {
        self.entries.push((tag, typ, count as u32, data));
    }
    fn bytes(&mut self, tag: u16, v: &[u8]) {
        self.add(tag, 1, v.len(), v.to_vec());
    }
    fn ascii(&mut self, tag: u16, s: &str) {
        let mut d = s.as_bytes().to_vec();
        d.push(0);
        self.add(tag, 2, d.len(), d);
    }
    fn short(&mut self, tag: u16, v: &[u16]) {
        self.add(
            tag,
            3,
            v.len(),
            v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        );
    }
    fn long(&mut self, tag: u16, v: &[u32]) {
        self.add(
            tag,
            4,
            v.len(),
            v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        );
    }
    fn rational(&mut self, tag: u16, v: &[(u32, u32)]) {
        let d = v
            .iter()
            .flat_map(|(n, d)| [n.to_le_bytes(), d.to_le_bytes()].concat())
            .collect();
        self.add(tag, 5, v.len(), d);
    }
    fn srational(&mut self, tag: u16, v: &[(i32, i32)]) {
        let d = v
            .iter()
            .flat_map(|(n, d)| [n.to_le_bytes(), d.to_le_bytes()].concat())
            .collect();
        self.add(tag, 10, v.len(), d);
    }
    fn float(&mut self, tag: u16, v: &[f32]) {
        self.add(
            tag,
            11,
            v.len(),
            v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        );
    }

    /// Hängt das Verzeichnis an `out` an und liefert seinen Offset.
    fn write(mut self, out: &mut Vec<u8>) -> u32 {
        if out.len() % 2 == 1 {
            out.push(0);
        }
        self.entries.sort_by_key(|e| e.0);
        let start = out.len();
        let n = self.entries.len();
        let mut extra_off = start + 2 + 12 * n + 4;
        let mut extra = Vec::new();
        out.extend((n as u16).to_le_bytes());
        for (tag, typ, count, data) in &self.entries {
            out.extend(tag.to_le_bytes());
            out.extend(typ.to_le_bytes());
            out.extend(count.to_le_bytes());
            if data.len() <= 4 {
                let mut v = data.clone();
                v.resize(4, 0);
                out.extend(v);
            } else {
                out.extend((extra_off as u32).to_le_bytes());
                extra.extend(data);
                if data.len() % 2 == 1 {
                    extra.push(0);
                }
                extra_off = start + 2 + 12 * n + 4 + extra.len();
            }
        }
        out.extend(0u32.to_le_bytes());
        out.extend(extra);
        start as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient() -> ImageF32 {
        ImageF32::from_fn(600, 300, |x, y| [x as f32 / 600.0, y as f32 / 300.0, 0.25])
    }

    /// rawler liest das DNG wieder: Größe, Linearisierung, Farben
    #[test]
    fn developed_dng_round_trips() {
        let img = gradient();
        let data = encode_lossy(DngContent::Developed(&img)).unwrap();
        let raw = rawler::decode(
            &rawler::rawsource::RawSource::new_from_slice(&data),
            &Default::default(),
        )
        .unwrap();
        assert_eq!((raw.width, raw.height, raw.cpp), (600, 300, 3));
        let px = |x: usize, y: usize| -> [f32; 3] {
            match &raw.data {
                rawler::RawImageData::Integer(d) => {
                    let i = (y * raw.width + x) * 3;
                    [0, 1, 2].map(|k| d[i + k] as f32 / 65535.0)
                }
                rawler::RawImageData::Float(d) => {
                    let i = (y * raw.width + x) * 3;
                    [d[i], d[i + 1], d[i + 2]]
                }
            }
        };
        for (x, y) in [(10, 10), (300, 150), (590, 290)] {
            let want = img.get(x, y).map(crate::color::srgb_to_linear);
            let got = px(x, y);
            for k in 0..3 {
                assert!(
                    (got[k] - want[k]).abs() < 0.02,
                    "{x},{y}: {got:?} vs {want:?}"
                );
            }
        }
    }

    /// Lumenkorns eigene RAW-Entwicklung (Farbmatrix, Weißabgleich) liefert
    /// dieselben Farben zurück
    #[test]
    fn developed_dng_keeps_colors_in_lumenkorn() {
        let img = ImageF32::from_fn(512, 256, |x, y| {
            // Flächen: grau, rot, grün, blau
            let c = [
                [0.5, 0.5, 0.5],
                [0.8, 0.2, 0.2],
                [0.2, 0.7, 0.3],
                [0.2, 0.3, 0.8],
            ];
            c[(x / 128) % 4].map(|v: f32| v * (0.6 + 0.4 * y as f32 / 256.0))
        });
        let path = std::env::temp_dir().join(format!("lw-dng-colors-{}.dng", std::process::id()));
        std::fs::write(&path, encode_lossy(DngContent::Developed(&img)).unwrap()).unwrap();
        let back = crate::Source::load(&path, true, None, None).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!((back.image.width, back.image.height), (512, 256));
        for x in [64, 192, 320, 448] {
            let want = img.get(x, 128).map(crate::color::srgb_to_linear);
            let got = back.image.get(x, 128);
            // Farbverhältnisse (die Helligkeit kann der RAW-Weg skalieren)
            for k in [0, 2] {
                let (rw, rg) = (want[k] / want[1], got[k] / got[1]);
                assert!(
                    (rw - rg).abs() < 0.05 * rw.max(1.0),
                    "x={x}: {got:?} vs {want:?}"
                );
            }
        }
    }

    #[test]
    fn scene_dng_keeps_highlights() {
        // Werte bis 4 (zwei Blenden über Weiß)
        let img = ImageF32::from_fn(300, 200, |x, _| [x as f32 / 75.0, 0.5, 0.2]);
        let data = encode_lossy(DngContent::Scene(&img)).unwrap();
        let raw = rawler::decode(
            &rawler::rawsource::RawSource::new_from_slice(&data),
            &Default::default(),
        )
        .unwrap();
        assert_eq!((raw.width, raw.height), (300, 200));
    }
}
