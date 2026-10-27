//! Metadaten in exportierte Dateien schreiben: EXIF (IFD0 und EXIF-IFD) und
//! ein XMP-Paket, als APP1-Segmente im JPEG bzw. als Tags im TIFF.

use crate::dng::Ifd;

/// Gültige Metadaten eines Fotos für den Export (bearbeitet oder aus der Datei)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExportMeta {
    pub title: Option<String>,
    pub description: Option<String>,
    pub creator: Option<String>,
    pub copyright: Option<String>,
    /// `JJJJ-MM-TTThh:mm:ss`
    pub captured_at: Option<String>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<u32>,
    /// Belichtungszeit in Sekunden
    pub exposure: Option<f64>,
    pub aperture: Option<f64>,
    pub focal_length: Option<f64>,
    /// Vollständiges XMP-Paket (mit Schlagworten und Bewertung)
    pub xmp: Option<String>,
}

/// Wert eines EXIF-Tags
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Val {
    Ascii(String),
    Short(u16),
    Rational(u32, u32),
    Undefined(Vec<u8>),
}

/// EXIF-Tags: (IFD0, EXIF-IFD)
pub(crate) type Tags = (Vec<(u16, Val)>, Vec<(u16, Val)>);

/// `JJJJ-MM-TTThh:mm:ss` → `JJJJ:MM:TT hh:mm:ss`
fn exif_date(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let ok = b.len() >= 19
        && b[4] == b'-'
        && b[7] == b'-'
        && matches!(b[10], b'T' | b' ')
        && b[13] == b':'
        && b[16] == b':';
    ok.then(|| format!("{}:{}:{} {}", &s[0..4], &s[5..7], &s[8..10], &s[11..19]))
}

/// Belichtungszeit als Bruch: kurze Zeiten als 1/n, sofern das genau genug ist
fn exposure_rational(s: f64) -> Option<(u32, u32)> {
    if !(s.is_finite() && s > 0.0 && s < 100_000.0) {
        return None;
    }
    if s < 1.0 {
        let d = (1.0 / s).round();
        if d >= 1.0 && ((1.0 / d) - s).abs() <= s * 0.01 {
            return Some((1, d as u32));
        }
        return Some(((s * 100_000.0).round() as u32, 100_000));
    }
    Some(((s * 10.0).round() as u32, 10))
}

fn tenths(v: f64) -> Option<(u32, u32)> {
    (v.is_finite() && v > 0.0 && v < 400_000_000.0).then(|| ((v * 10.0).round() as u32, 10))
}

/// Die Tags, die geschrieben werden. Ausrichtung 1: Die Pixel sind schon
/// gedreht. Farbraum sRGB, wie alle Exporte außer DNG.
fn text(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

pub(crate) fn tags(m: &ExportMeta) -> Tags {
    let mut ifd0 = vec![(274, Val::Short(1)), (305, Val::Ascii("Lumenkorn".into()))];
    if let Some(d) = text(&m.description).or(text(&m.title)) {
        ifd0.push((270, Val::Ascii(d.into())));
    }
    if let Some(c) = text(&m.camera) {
        ifd0.push((272, Val::Ascii(c.into())));
    }
    if let Some(a) = text(&m.creator) {
        ifd0.push((315, Val::Ascii(a.into())));
    }
    if let Some(c) = text(&m.copyright) {
        ifd0.push((33432, Val::Ascii(c.into())));
    }
    let mut exif = vec![
        (36864, Val::Undefined(b"0232".to_vec())),
        (40961, Val::Short(1)),
    ];
    if let Some((n, d)) = m.exposure.and_then(exposure_rational) {
        exif.push((33434, Val::Rational(n, d)));
    }
    if let Some((n, d)) = m.aperture.and_then(tenths) {
        exif.push((33437, Val::Rational(n, d)));
    }
    if let Some(iso) = m.iso.filter(|v| *v > 0) {
        exif.push((34855, Val::Short(iso.min(65535) as u16)));
    }
    if let Some(d) = m.captured_at.as_deref().and_then(exif_date) {
        exif.push((36867, Val::Ascii(d.clone())));
        // Ohne Zeitzone gilt die Ortszeit der Aufnahme
        exif.push((36868, Val::Ascii(d)));
    }
    if let Some((n, d)) = m.focal_length.and_then(tenths) {
        exif.push((37386, Val::Rational(n, d)));
    }
    if let Some(l) = text(&m.lens) {
        exif.push((42036, Val::Ascii(l.into())));
    }
    (ifd0, exif)
}

fn to_ifd(tags: Vec<(u16, Val)>) -> Ifd {
    let mut ifd = Ifd::default();
    for (tag, v) in tags {
        match v {
            Val::Ascii(s) => ifd.ascii(tag, &s),
            Val::Short(s) => ifd.short(tag, &[s]),
            Val::Rational(n, d) => ifd.rational(tag, &[(n, d)]),
            Val::Undefined(b) => ifd.add(tag, 7, b.len(), b),
        }
    }
    ifd
}

/// EXIF als TIFF-Block (little endian), wie ihn das JPEG-APP1-Segment nach
/// `Exif\0\0` erwartet
pub(crate) fn exif_tiff(m: &ExportMeta) -> Vec<u8> {
    let (ifd0, exif) = tags(m);
    let mut out = b"II*\0\0\0\0\0".to_vec();
    let exif_off = to_ifd(exif).write(&mut out);
    let mut d0 = to_ifd(ifd0);
    d0.long(34665, &[exif_off]);
    let off0 = d0.write(&mut out);
    out[4..8].copy_from_slice(&off0.to_le_bytes());
    out
}

/// APP1-Segmente für ein JPEG: EXIF, dann XMP (sofern es in ein Segment passt)
pub(crate) fn jpeg_segments(m: &ExportMeta) -> Vec<Vec<u8>> {
    let mut exif = b"Exif\0\0".to_vec();
    exif.extend(exif_tiff(m));
    let mut out = vec![exif];
    if let Some(x) = &m.xmp {
        let mut seg = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        seg.extend(x.as_bytes());
        if seg.len() <= 65533 {
            out.push(seg);
        } else {
            // Erweitertes XMP über mehrere Segmente lohnt sich hier nicht
            log::warn!("XMP zu groß für das JPEG ({} Byte), ausgelassen", seg.len());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ExportMeta {
        ExportMeta {
            title: Some("Abend am See".into()),
            description: None,
            creator: Some("Jörg Müller".into()),
            copyright: Some("© 2026 Jörg Müller".into()),
            captured_at: Some("2025-06-01T19:42:07".into()),
            camera: Some("Sony ILCE-7M4".into()),
            lens: Some("FE 24-70mm F2.8 GM II".into()),
            iso: Some(400),
            exposure: Some(1.0 / 250.0),
            aperture: Some(2.8),
            focal_length: Some(35.0),
            xmp: Some("<x:xmpmeta/>".into()),
        }
    }

    #[test]
    fn converts_values() {
        assert_eq!(
            exif_date("2025-06-01T19:42:07").as_deref(),
            Some("2025:06:01 19:42:07")
        );
        assert_eq!(exif_date("2025-06"), None);
        assert_eq!(exposure_rational(1.0 / 250.0), Some((1, 250)));
        assert_eq!(exposure_rational(2.0), Some((20, 10)));
        assert_eq!(exposure_rational(0.0), None);
        assert_eq!(tenths(2.8), Some((28, 10)));
    }

    #[test]
    fn tags_cover_all_fields() {
        let (ifd0, exif) = tags(&sample());
        let has = |v: &[(u16, Val)], t: u16| v.iter().any(|e| e.0 == t);
        for t in [270, 272, 274, 305, 315, 33432] {
            assert!(has(&ifd0, t), "IFD0 {t}");
        }
        for t in [33434, 33437, 34855, 36867, 37386, 42036] {
            assert!(has(&exif, t), "EXIF {t}");
        }
        // Ohne Werte nur das Nötigste
        let (ifd0, exif) = tags(&ExportMeta::default());
        assert_eq!(ifd0.len(), 2);
        assert_eq!(exif.len(), 2);
    }

    /// Das JPEG-Segment lässt sich mit einem unabhängigen EXIF-Leser lesen
    #[test]
    fn jpeg_exif_is_readable() {
        let img = image::RgbImage::from_pixel(16, 8, image::Rgb([120, 90, 60]));
        let jpg = crate::output::encode_jpeg_rgb8_meta(&img, 90, Some(&sample())).unwrap();
        let exif = exif::Reader::new()
            .read_from_container(&mut std::io::Cursor::new(&jpg))
            .unwrap();
        let get = |t| {
            exif.get_field(t, exif::In::PRIMARY)
                .map(|f| f.display_value().to_string())
        };
        assert_eq!(get(exif::Tag::Model).as_deref(), Some("\"Sony ILCE-7M4\""));
        assert_eq!(get(exif::Tag::ExposureTime).as_deref(), Some("1/250"));
        assert_eq!(get(exif::Tag::FNumber).as_deref(), Some("2.8"));
        assert_eq!(
            get(exif::Tag::PhotographicSensitivity).as_deref(),
            Some("400")
        );
        assert_eq!(
            get(exif::Tag::DateTimeOriginal).as_deref(),
            Some("2025-06-01 19:42:07")
        );
        assert!(get(exif::Tag::LensModel).unwrap().contains("24-70mm"));
        // UTF-8 im ASCII-Feld (wie viele Programme es schreiben); Unicode steht vollständig im XMP
        let artist = exif
            .get_field(exif::Tag::Artist, exif::In::PRIMARY)
            .unwrap();
        assert!(matches!(&artist.value, exif::Value::Ascii(v) if v[0] == "Jörg Müller".as_bytes()));
        // XMP folgt als zweites APP1-Segment
        let marker = b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta/>";
        assert!(jpg.windows(marker.len()).any(|w| w == marker));
        // Das Bild selbst bleibt lesbar
        assert_eq!(image::load_from_memory(&jpg).unwrap().width(), 16);
    }
}
