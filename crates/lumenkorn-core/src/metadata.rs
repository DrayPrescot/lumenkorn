//! Metadaten: EXIF lesen und einen schnellen Inhalts-Hash bilden.

use crate::formats::ImageKind;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata {
    pub width: Option<i64>,
    pub height: Option<i64>,
    /// EXIF-Orientierung 1–8.
    pub orientation: Option<i64>,
    /// ISO-8601 ohne Zeitzone, z. B. `2024-06-01T14:03:22`.
    pub captured_at: Option<String>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<i64>,
    pub exposure: Option<String>,
    pub aperture: Option<f64>,
    pub focal_length: Option<f64>,
    /// Aufnahmeort aus den GPS-Daten (Breite, Länge in Grad, WGS 84)
    pub gps: Option<(f64, f64)>,
}

const HASH_CHUNK: u64 = 64 * 1024;

/// Hash über Dateigröße sowie Anfang und Ende der Datei.
///
/// Liest höchstens 128 KiB und erkennt damit verschobene oder umbenannte
/// Dateien zuverlässig genug, ohne große RAWs komplett lesen zu müssen.
pub(crate) fn quick_hash(path: &Path) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let mut hasher = blake3::Hasher::new();
    hasher.update(&len.to_le_bytes());

    let mut buf = vec![0u8; HASH_CHUNK as usize];
    let n = read_up_to(&mut f, &mut buf)?;
    hasher.update(&buf[..n]);

    if len > HASH_CHUNK * 2 {
        f.seek(SeekFrom::End(-(HASH_CHUNK as i64)))?;
        let n = read_up_to(&mut f, &mut buf)?;
        hasher.update(&buf[..n]);
    } else if len > HASH_CHUNK {
        let n = read_up_to(&mut f, &mut buf)?;
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex()[..32].to_string())
}

fn read_up_to(f: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match f.read(&mut buf[total..])? {
            0 => break,
            n => total += n,
        }
    }
    Ok(total)
}

/// Liest EXIF und Abmessungen. Fehler führen zu leeren Feldern, nie zum Abbruch.
pub fn read(path: &Path, kind: ImageKind) -> Metadata {
    let mut m = read_exif(path).unwrap_or_default();

    if lumenkorn_develop::exr_io::is_exr(path) {
        if let Some((w, h)) = lumenkorn_develop::exr_io::dimensions(path) {
            m.width = Some(w as i64);
            m.height = Some(h as i64);
        }
        return m;
    }
    // Für Rasterbilder sind die echten Pixelmaße billig und verlässlicher als EXIF.
    if kind == ImageKind::Raster
        && let Ok((w, h)) = image::image_dimensions(path)
    {
        m.width = Some(w as i64);
        m.height = Some(h as i64);
    }

    // Bei manchen RAWs (NEF, ARW …) beschreibt IFD0 nur das Mini-Thumbnail
    // (z. B. 160×120) – dann stimmten Seitenverhältnis im Raster und
    // Größenangabe nicht. Die Sensormaße stehen in den Unter-IFDs.
    if kind == ImageKind::Raw
        && m.width.max(m.height).unwrap_or(0) < 1000
        && let Some((w, h)) = tiff_raw_dims(path)
    {
        m.width = Some(w as i64);
        m.height = Some(h as i64);
    }

    // Bei 90°-Drehung Breite und Höhe tauschen, damit das Raster korrekt layoutet.
    if matches!(m.orientation, Some(5..=8)) {
        std::mem::swap(&mut m.width, &mut m.height);
    }
    m
}

/// Größtes Vollbild (NewSubfileType 0) in den IFDs und SubIFDs eines
/// TIFF-basierten RAWs. Liest nur den Dateianfang.
pub(crate) fn tiff_raw_dims(path: &Path) -> Option<(u32, u32)> {
    let mut buf = Vec::new();
    File::open(path)
        .ok()?
        .take(1 << 20)
        .read_to_end(&mut buf)
        .ok()?;
    tiff_raw_dims_in(&buf)
}

fn tiff_raw_dims_in(d: &[u8]) -> Option<(u32, u32)> {
    let le = match d.get(0..4)? {
        [b'I', b'I', 42, 0] | [b'I', b'I', 0x52, 0x4F] | [b'I', b'I', 0x55, 0] => true, // TIFF, ORF, RW2
        [b'M', b'M', 0, 42] => false,
        _ => return None,
    };
    let u16_at = |o: usize| -> Option<u32> {
        let b = d.get(o..o + 2)?;
        Some(if le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        } as u32)
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b: [u8; 4] = d.get(o..o + 4)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    // Wert eines Eintrags (SHORT oder LONG, erster Wert)
    let value = |e: usize| -> Option<u32> {
        match u16_at(e + 2)? {
            3 => u16_at(e + 8),
            4 => u32_at(e + 8),
            _ => None,
        }
    };
    let mut best: Option<(u32, u32)> = None;
    let mut queue = vec![u32_at(4)? as usize];
    // Grenzen hier und unten: Schutz vor kaputten oder zyklischen IFDs (echte
    // RAWs haben nur wenige IFDs mit höchstens einigen Dutzend Einträgen)
    let mut seen = std::collections::HashSet::new();
    while let Some(ifd) = queue.pop() {
        if ifd == 0 || !seen.insert(ifd) || seen.len() > 64 {
            continue;
        }
        let Some(n) = u16_at(ifd) else { continue };
        let (mut w, mut h, mut sub) = (None, None, 0);
        for i in 0..n.min(500) as usize {
            let e = ifd + 2 + i * 12;
            match u16_at(e) {
                Some(0x00FE) => sub = value(e).unwrap_or(0),
                Some(0x0100) => w = value(e),
                Some(0x0101) => h = value(e),
                Some(0x014A) => {
                    // SubIFDs: ein Offset direkt oder eine Liste von Offsets
                    let count = u32_at(e + 4).unwrap_or(0) as usize;
                    if count == 1 {
                        queue.extend(u32_at(e + 8).map(|o| o as usize));
                    } else if let Some(list) = u32_at(e + 8) {
                        for k in 0..count.min(16) {
                            queue.extend(u32_at(list as usize + 4 * k).map(|o| o as usize));
                        }
                    }
                }
                _ => {}
            }
        }
        if sub == 0
            && let (Some(w), Some(h)) = (w, h)
            && w.max(h) > best.map_or(0, |(a, b)| a.max(b))
        {
            best = Some((w, h));
        }
        // nächstes IFD in der Kette
        if let Some(next) = u32_at(ifd + 2 + n as usize * 12) {
            queue.push(next as usize);
        }
    }
    // Kleiner sind nur EXIF-Thumbnails (160 × 120), nie das Sensorbild
    best.filter(|(w, h)| *w >= 160 && *h >= 160)
}

/// Aufnahmezeit und EXIF-Ausrichtung, zuerst nur aus dem ersten MB der Datei
/// (Import von einer langsamen Speicherkarte): `kamadak-exif` liest
/// TIFF-basierte RAWs sonst vollständig ein – bei 500 Fotos über 10 GB von
/// der Karte. Steht dort kein Datum, wird die ganze Datei gelesen.
pub fn read_capture(path: &Path) -> (Option<String>, Option<i64>) {
    let mut head = Vec::new();
    if let Ok(f) = File::open(path) {
        let _ = f.take(1 << 20).read_to_end(&mut head);
    }
    exif_from(&mut std::io::Cursor::new(&head))
        .filter(|m| m.captured_at.is_some())
        .or_else(|| read_exif(path))
        .map_or((None, None), |m| (m.captured_at, m.orientation))
}

fn read_exif(path: &Path) -> Option<Metadata> {
    let file = File::open(path).ok()?;
    exif_from(&mut BufReader::new(file))
}

fn exif_from<R: BufRead + Seek>(reader: &mut R) -> Option<Metadata> {
    use exif::{In, Tag, Value};

    let exif = exif::Reader::new().read_from_container(reader).ok()?;

    let text = |tag: Tag| -> Option<String> {
        let f = exif.get_field(tag, In::PRIMARY)?;
        let s = match &f.value {
            Value::Ascii(v) => v
                .iter()
                .map(|b| {
                    String::from_utf8_lossy(b)
                        .trim_matches(char::from(0))
                        .trim()
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join(" "),
            _ => f.display_value().to_string(),
        };
        let s = s.trim().to_string();
        (!s.is_empty()).then_some(s)
    };
    let uint = |tag: Tag| -> Option<i64> {
        exif.get_field(tag, In::PRIMARY)
            .and_then(|f| f.value.get_uint(0))
            .map(i64::from)
    };
    let rational = |tag: Tag| -> Option<f64> {
        match &exif.get_field(tag, In::PRIMARY)?.value {
            Value::Rational(v) if !v.is_empty() && v[0].denom != 0 => Some(v[0].to_f64()),
            _ => None,
        }
    };

    let make = text(Tag::Make);
    let model = text(Tag::Model);
    let camera = match (make, model) {
        // Viele Hersteller wiederholen die Marke im Modellnamen („Canon Canon EOS R5“).
        (Some(mk), Some(md)) if md.to_lowercase().starts_with(&mk.to_lowercase()) => Some(md),
        (Some(mk), Some(md)) => Some(format!("{mk} {md}")),
        (a, b) => a.or(b),
    };

    let captured_at = text(Tag::DateTimeOriginal)
        .or_else(|| text(Tag::DateTime))
        .and_then(|s| exif_date_to_iso(&s));

    let exposure = match &exif
        .get_field(Tag::ExposureTime, In::PRIMARY)
        .map(|f| &f.value)
    {
        Some(Value::Rational(v)) if !v.is_empty() && v[0].denom != 0 => {
            let r = v[0];
            let secs = r.to_f64();
            Some(if secs < 1.0 && secs > 0.0 {
                format!("1/{}", (1.0 / secs).round() as i64)
            } else {
                format!("{secs}s")
            })
        }
        _ => None,
    };

    Some(Metadata {
        width: uint(Tag::PixelXDimension).or_else(|| uint(Tag::ImageWidth)),
        height: uint(Tag::PixelYDimension).or_else(|| uint(Tag::ImageLength)),
        orientation: uint(Tag::Orientation).filter(|o| (1..=8).contains(o)),
        captured_at,
        camera,
        lens: text(Tag::LensModel),
        iso: uint(Tag::PhotographicSensitivity),
        exposure,
        aperture: rational(Tag::FNumber),
        focal_length: rational(Tag::FocalLength),
        gps: gps(&exif),
    })
}

/// GPS-Position aus EXIF: Grad, Minuten, Sekunden und Himmelsrichtung.
/// Nullpunkt (0, 0) und unmögliche Werte gelten als „kein Ort“ – manche
/// Kameras schreiben ohne Empfang Nullen.
fn gps(exif: &exif::Exif) -> Option<(f64, f64)> {
    use exif::{In, Tag, Value};
    let dms = |tag: Tag| -> Option<f64> {
        match &exif.get_field(tag, In::PRIMARY)?.value {
            Value::Rational(v) if v.len() >= 3 && v.iter().all(|r| r.denom != 0) => {
                Some(v[0].to_f64() + v[1].to_f64() / 60.0 + v[2].to_f64() / 3600.0)
            }
            Value::Rational(v) if !v.is_empty() && v[0].denom != 0 => Some(v[0].to_f64()),
            _ => None,
        }
    };
    let reference = |tag: Tag| -> Option<u8> {
        match &exif.get_field(tag, In::PRIMARY)?.value {
            Value::Ascii(v) => v.first()?.first().map(u8::to_ascii_uppercase),
            _ => None,
        }
    };
    let mut lat = dms(Tag::GPSLatitude)?;
    let mut lon = dms(Tag::GPSLongitude)?;
    if reference(Tag::GPSLatitudeRef) == Some(b'S') {
        lat = -lat;
    }
    if reference(Tag::GPSLongitudeRef) == Some(b'W') {
        lon = -lon;
    }
    let valid = lat.is_finite()
        && lon.is_finite()
        && lat.abs() <= 90.0
        && lon.abs() <= 180.0
        && (lat.abs() > 1e-6 || lon.abs() > 1e-6);
    valid.then_some((lat, lon))
}

/// Nur den Aufnahmeort lesen (Nachtragen für Fotos, die vor dem Lesen von
/// GPS-Daten in den Katalog kamen).
pub fn read_gps(path: &Path) -> Option<(f64, f64)> {
    let file = File::open(path).ok()?;
    let exif = exif::Reader::new()
        .read_from_container(&mut BufReader::new(file))
        .ok()?;
    gps(&exif)
}

/// `2024:06:01 14:03:22` → `2024-06-01T14:03:22`
fn exif_date_to_iso(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() < 19 || s.starts_with("0000") {
        return None;
    }
    // Kaputte EXIF-Daten können Nicht-ASCII-Zeichen enthalten; dann fielen die
    // Byte-Grenzen unten mitten in ein Zeichen (Panik, Scan des Ordners bricht ab)
    let b = s.as_bytes();
    if !b[..19].is_ascii() || b[4] != b':' || b[7] != b':' {
        return None;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if !(digits(0..4)
        && digits(5..7)
        && digits(8..10)
        && digits(11..13)
        && digits(14..16)
        && digits(17..19))
    {
        return None;
    }
    Some(format!(
        "{}-{}-{}T{}",
        &s[0..4],
        &s[5..7],
        &s[8..10],
        &s[11..19]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exif_dates_never_panic() {
        assert_eq!(
            exif_date_to_iso("2024:06:01 14:03:22").as_deref(),
            Some("2024-06-01T14:03:22")
        );
        assert_eq!(exif_date_to_iso("2024:06:\u{FFFD}1 14:03:22"), None);
        assert_eq!(exif_date_to_iso("2024:06:01 14:0\u{FFFD}:22xx"), None);
        assert_eq!(exif_date_to_iso("    :  :     :  :  "), None);
        assert_eq!(exif_date_to_iso("0000:00:00 00:00:00"), None);
    }

    /// TIFF mit Mini-Thumbnail in IFD0 und Vollbild im SubIFD (wie NEF).
    fn fake_raw(le: bool) -> Vec<u8> {
        let w16 = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let w32 = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let entry = |tag: u16, typ: u16, val: u32| {
            let mut e = Vec::new();
            e.extend(w16(tag));
            e.extend(w16(typ));
            e.extend(w32(1));
            if typ == 3 {
                e.extend(w16(val as u16));
                e.extend([0, 0]);
            } else {
                e.extend(w32(val));
            }
            e
        };
        let mut d = if le { b"II".to_vec() } else { b"MM".to_vec() };
        d.extend(w16(42));
        d.extend(w32(8));
        // IFD0 bei 8: Thumbnail 160×120 (Subfile 1), SubIFD → 100
        d.extend(w16(4));
        d.extend(entry(0x00FE, 4, 1));
        d.extend(entry(0x0100, 3, 160));
        d.extend(entry(0x0101, 3, 120));
        d.extend(entry(0x014A, 4, 100));
        d.extend(w32(0));
        d.resize(100, 0);
        // SubIFD: 6048×4024, Subfile 0
        d.extend(w16(3));
        d.extend(entry(0x00FE, 4, 0));
        d.extend(entry(0x0100, 4, 6048));
        d.extend(entry(0x0101, 4, 4024));
        d.extend(w32(0));
        d
    }

    #[test]
    fn raw_dims_come_from_sub_ifds() {
        assert_eq!(tiff_raw_dims_in(&fake_raw(true)), Some((6048, 4024)));
        assert_eq!(tiff_raw_dims_in(&fake_raw(false)), Some((6048, 4024)));
        assert_eq!(tiff_raw_dims_in(b"kein tiff"), None);
        // Abgeschnittene Datei: kein Absturz
        assert_eq!(tiff_raw_dims_in(&fake_raw(true)[..60]), None);
    }

    #[test]
    fn exif_dates() {
        assert_eq!(
            exif_date_to_iso("2024:06:01 14:03:22").as_deref(),
            Some("2024-06-01T14:03:22")
        );
        assert_eq!(exif_date_to_iso("0000:00:00 00:00:00"), None);
        assert_eq!(exif_date_to_iso("kaputt"), None);
    }

    #[test]
    fn hash_detects_changes_but_not_renames() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.bin");
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&a, &data).unwrap();
        let h1 = quick_hash(&a).unwrap();

        let b = d.path().join("b.bin");
        std::fs::rename(&a, &b).unwrap();
        assert_eq!(quick_hash(&b).unwrap(), h1);

        let mut changed = data.clone();
        changed[10] ^= 0xff;
        std::fs::write(&b, &changed).unwrap();
        assert_ne!(quick_hash(&b).unwrap(), h1);
    }
}
