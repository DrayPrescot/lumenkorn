//! EXIF aus fremden Dateien lesen, ohne dass eine präparierte Datei den
//! Speicher sprengt.
//!
//! `kamadak-exif` folgt jedem Zeiger auf ein Exif-, GPS- oder Interop-IFD und legt
//! dessen Felder jedes Mal neu an. N Zeiger auf ein IFD mit M Einträgen ergeben
//! N×M Felder: Gemessen wurden aus einer 70-KB-Datei 1 GB, eine Datei unter 1 MB
//! käme auf viele GB. Scheitert die Anforderung, endet der ganze Prozess, und
//! weil der Scan bei jedem Start läuft, gäbe das eine Absturzschleife
//! (`catch_unwind` hilft dagegen nicht).
//!
//! Deshalb wird der EXIF-Block hier selbst aus der Datei geholt und vor dem
//! Parsen gezählt, was der Parser anlegen würde. Echte Fotos haben einige
//! hundert Felder.

use std::io::{BufRead, Read, Seek};

/// So viele Felder darf ein EXIF-Block haben
const MAX_FIELDS: usize = 20_000;
/// Eine TIFF-Datei wird für EXIF ganz gelesen (die Einträge können überall
/// liegen) – eine größere bleibt ohne EXIF, statt den Speicher zu füllen.
const MAX_TIFF_BYTES: u64 = 1 << 30;
/// Größter EXIF-Block in PNG und WebP (dort begrenzt das Format ihn nicht)
const MAX_CHUNK_BYTES: u32 = 16 << 20;

/// EXIF einer Bilddatei (TIFF und TIFF-basierte RAWs, JPEG, PNG, WebP).
/// `None`, wenn es keins gibt oder es unbrauchbar ist.
pub(crate) fn read_container<R: BufRead + Seek>(r: &mut R) -> Option<exif::Exif> {
    let block = {
        let head = r.fill_buf().ok()?;
        if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
            Block::Tiff
        } else if head.starts_with(&[0xFF, 0xD8]) {
            Block::Jpeg
        } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
            Block::Png
        } else if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
            Block::Webp
        } else {
            return None;
        }
    };
    let tiff = match block {
        Block::Tiff => read_limited(r)?,
        Block::Jpeg => jpeg_block(r)?,
        Block::Png => png_block(r)?,
        Block::Webp => webp_block(r)?,
    };
    read_tiff(tiff)
}

/// EXIF aus einem TIFF-Block. Enthält er unplausibel viele Felder: `None`.
pub(crate) fn read_tiff(tiff: Vec<u8>) -> Option<exif::Exif> {
    if !fields_bounded(&tiff) {
        log::warn!("EXIF-Block mit unplausibel vielen Einträgen übergangen");
        return None;
    }
    exif::Reader::new().read_raw(tiff).ok()
}

enum Block {
    Tiff,
    Jpeg,
    Png,
    Webp,
}

/// Rest der Datei, höchstens [`MAX_TIFF_BYTES`]
pub(crate) fn read_limited<R: Read>(r: &mut R) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    r.take(MAX_TIFF_BYTES + 1).read_to_end(&mut buf).ok()?;
    (buf.len() as u64 <= MAX_TIFF_BYTES).then_some(buf)
}

fn read_byte<R: Read>(r: &mut R) -> Option<u8> {
    let mut b = [0u8];
    r.read_exact(&mut b).ok()?;
    Some(b[0])
}

/// Erstes APP1-Segment mit EXIF. Hinter dem Beginn der Bilddaten (SOS) kann laut
/// Format keins mehr kommen, die übrige Datei muss also nicht gelesen werden.
fn jpeg_block<R: Read + Seek>(r: &mut R) -> Option<Vec<u8>> {
    const APP1: u8 = 0xE1;
    let mut soi = [0u8; 2];
    r.read_exact(&mut soi).ok()?;
    loop {
        // Bis zum nächsten Marker; 0xFF darf sich wiederholen
        while read_byte(r)? != 0xFF {}
        let mut code = read_byte(r)?;
        while code == 0xFF {
            code = read_byte(r)?;
        }
        match code {
            0x00..=0x01 | 0xD0..=0xD7 => continue,
            0xD8..=0xDA => return None,
            _ => {}
        }
        let mut len = [0u8; 2];
        r.read_exact(&mut len).ok()?;
        let len = u16::from_be_bytes(len).checked_sub(2)?;
        if code == APP1 {
            let mut seg = vec![0; usize::from(len)];
            r.read_exact(&mut seg).ok()?;
            if let Some(tiff) = seg.strip_prefix(b"Exif\0\0") {
                return Some(tiff.to_vec());
            }
        } else {
            r.seek_relative(i64::from(len)).ok()?;
        }
    }
}

/// Inhalt des Abschnitts `eXIf`
fn png_block<R: Read + Seek>(r: &mut R) -> Option<Vec<u8>> {
    let mut sig = [0u8; 8];
    r.read_exact(&mut sig).ok()?;
    loop {
        let mut head = [0u8; 8];
        r.read_exact(&mut head).ok()?;
        let len = u32::from_be_bytes(head[0..4].try_into().ok()?);
        match &head[4..8] {
            b"eXIf" => return read_chunk(r, len),
            b"IEND" => return None,
            // Daten und Prüfsumme überspringen
            _ => r.seek_relative(i64::from(len) + 4).ok()?,
        }
    }
}

/// Inhalt des Abschnitts `EXIF`
fn webp_block<R: Read + Seek>(r: &mut R) -> Option<Vec<u8>> {
    let mut sig = [0u8; 12];
    r.read_exact(&mut sig).ok()?;
    loop {
        let mut head = [0u8; 8];
        r.read_exact(&mut head).ok()?;
        let len = u32::from_le_bytes(head[4..8].try_into().ok()?);
        if &head[0..4] == b"EXIF" {
            let data = read_chunk(r, len)?;
            // Manche Programme schreiben den Vorspann aus JPEG mit
            return Some(match data.strip_prefix(b"Exif\0\0") {
                Some(tiff) => tiff.to_vec(),
                None => data,
            });
        }
        // Abschnitte sind auf gerade Längen aufgefüllt
        r.seek_relative(i64::from(len) + i64::from(len & 1)).ok()?;
    }
}

fn read_chunk<R: Read>(r: &mut R, len: u32) -> Option<Vec<u8>> {
    if len > MAX_CHUNK_BYTES {
        return None;
    }
    let mut data = Vec::new();
    r.take(u64::from(len)).read_to_end(&mut data).ok()?;
    (data.len() == len as usize).then_some(data)
}

#[derive(Clone, Copy, PartialEq)]
enum Ctx {
    Tiff,
    Exif,
    Other,
}

/// Abbruch beim Zählen
enum Stop {
    /// Mehr Felder als erlaubt oder ein Zeiger, dem nicht zu trauen ist
    TooMany,
    /// Abgeschnitten: Hier bricht auch der Parser ab, weiter kommt er nicht
    Broken,
}

/// Bleibt der Block unter [`MAX_FIELDS`]? Geht dieselben Wege wie der Parser:
/// bis zu 8 IFDs in der Kette, Zeiger führen eine oder zwei Ebenen tief.
fn fields_bounded(d: &[u8]) -> bool {
    let le = match d.get(0..4) {
        Some(b"II*\0") => true,
        Some(b"MM\0*") => false,
        // Das lehnt der Parser selbst ab
        _ => return true,
    };
    let t = Tiff { d, le };
    let mut total = 0;
    let Some(mut next) = t.u32_at(4) else {
        return true;
    };
    for _ in 0..8 {
        if next == 0 {
            break;
        }
        match t.ifd(next as usize, Ctx::Tiff, &mut total) {
            Ok(n) => next = n,
            Err(Stop::TooMany) => return false,
            Err(Stop::Broken) => break,
        }
    }
    true
}

struct Tiff<'a> {
    d: &'a [u8],
    le: bool,
}

impl Tiff<'_> {
    fn u16_at(&self, o: usize) -> Option<u16> {
        let b: [u8; 2] = self.d.get(o..o.checked_add(2)?)?.try_into().ok()?;
        Some(if self.le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    fn u32_at(&self, o: usize) -> Option<u32> {
        let b: [u8; 4] = self.d.get(o..o.checked_add(4)?)?.try_into().ok()?;
        Some(if self.le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    /// Zählt ein IFD samt Kindern. Liefert den Verweis aufs nächste IFD.
    fn ifd(&self, ofs: usize, ctx: Ctx, total: &mut usize) -> Result<u32, Stop> {
        let n = usize::from(self.u16_at(ofs).ok_or(Stop::Broken)?);
        for i in 0..n {
            let e = ofs + 2 + i * 12;
            let tag = self.u16_at(e).ok_or(Stop::Broken)?;
            *total += 1;
            if *total > MAX_FIELDS {
                return Err(Stop::TooMany);
            }
            let child = match (ctx, tag) {
                (Ctx::Tiff, 0x8769) => Ctx::Exif,
                (Ctx::Tiff, 0x8825) | (Ctx::Exif, 0xA005) => Ctx::Other,
                _ => continue,
            };
            let target = self.pointer(e)?;
            self.ifd(target, child, total)?;
        }
        self.u32_at(ofs + 2 + n * 12).ok_or(Stop::Broken)
    }

    /// Ziel eines Zeigers, gelesen wie im Parser. Laut Norm ist ein Zeiger genau
    /// ein LONG; alles außer den einfachen Fällen wird nicht weiterverfolgt und
    /// gilt als unbrauchbar, sonst zählte dieser Weg anders als der Parser.
    fn pointer(&self, e: usize) -> Result<usize, Stop> {
        let typ = self.u16_at(e + 2).ok_or(Stop::Broken)?;
        let count = self.u32_at(e + 4).ok_or(Stop::Broken)?;
        match (typ, count) {
            (4, 1) => self.u32_at(e + 8).map(|v| v as usize),
            (3, 1) => self.u16_at(e + 8).map(usize::from),
            _ => return Err(Stop::TooMany),
        }
        .ok_or(Stop::Broken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// TIFF mit einem Datum, wie es eine Kamera schreibt
    fn sample_tiff() -> Vec<u8> {
        use exif::{Field, In, Tag, Value};
        let date = Field {
            tag: Tag::DateTimeOriginal,
            ifd_num: In::PRIMARY,
            value: Value::Ascii(vec![b"2026:10:11 14:32:10".to_vec()]),
        };
        let mut w = exif::experimental::Writer::new();
        w.push_field(&date);
        let mut out = Cursor::new(Vec::new());
        w.write(&mut out, false).unwrap();
        out.into_inner()
    }

    fn date_of(file: Vec<u8>) -> Option<String> {
        let exif = read_container(&mut Cursor::new(file))?;
        let f = exif.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)?;
        Some(f.display_value().to_string())
    }

    const DATE: &str = "2026-10-11 14:32:10";

    #[test]
    fn reads_tiff() {
        assert_eq!(date_of(sample_tiff()).as_deref(), Some(DATE));
    }

    #[test]
    fn reads_jpeg_also_behind_other_segments() {
        let tiff = sample_tiff();
        let mut f = vec![0xFF, 0xD8];
        // APP0 davor, wie in JFIF-Dateien
        f.extend_from_slice(&[0xFF, 0xE0, 0, 4, 1, 2]);
        // Fremdes APP1 (XMP) davor
        f.extend_from_slice(&[0xFF, 0xE1, 0, 5, b'x', b'm', b'p']);
        f.extend_from_slice(&[0xFF, 0xE1]);
        f.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
        f.extend_from_slice(b"Exif\0\0");
        f.extend_from_slice(&tiff);
        f.extend_from_slice(&[0xFF, 0xDA, 0, 2, 0xFF, 0xD9]);
        assert_eq!(date_of(f).as_deref(), Some(DATE));
    }

    #[test]
    fn jpeg_without_exif_has_none() {
        assert!(date_of(vec![0xFF, 0xD8, 0xFF, 0xDA, 0, 2, 0xFF, 0xD9]).is_none());
        assert!(date_of(vec![0xFF, 0xD8]).is_none());
    }

    #[test]
    fn reads_png() {
        let tiff = sample_tiff();
        let mut f = b"\x89PNG\r\n\x1a\n".to_vec();
        f.extend_from_slice(&3u32.to_be_bytes());
        f.extend_from_slice(b"IHDR\0\0\0crc!");
        f.extend_from_slice(&(tiff.len() as u32).to_be_bytes());
        f.extend_from_slice(b"eXIf");
        f.extend_from_slice(&tiff);
        f.extend_from_slice(b"crc!");
        assert_eq!(date_of(f).as_deref(), Some(DATE));
    }

    #[test]
    fn reads_webp_with_padding_and_jpeg_prefix() {
        let tiff = sample_tiff();
        let mut body = Vec::new();
        // Abschnitt ungerader Länge: ein Füllbyte folgt
        body.extend_from_slice(b"VP8 ");
        body.extend_from_slice(&3u32.to_le_bytes());
        body.extend_from_slice(b"abc\0");
        body.extend_from_slice(b"EXIF");
        body.extend_from_slice(&((tiff.len() + 6) as u32).to_le_bytes());
        body.extend_from_slice(b"Exif\0\0");
        body.extend_from_slice(&tiff);
        let mut f = b"RIFF".to_vec();
        f.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
        f.extend_from_slice(b"WEBP");
        f.extend_from_slice(&body);
        assert_eq!(date_of(f).as_deref(), Some(DATE));
    }

    /// `n` Zeiger im IFD0 auf ein Exif-IFD mit `m` Einträgen
    fn bomb(n: u32, m: u32, typ: u16, count: u32) -> Vec<u8> {
        let mut b = b"II*\0".to_vec();
        b.extend_from_slice(&8u32.to_le_bytes());
        let exif = 8 + 2 + 12 * n + 4;
        b.extend_from_slice(&(n as u16).to_le_bytes());
        for _ in 0..n {
            b.extend_from_slice(&0x8769u16.to_le_bytes());
            b.extend_from_slice(&typ.to_le_bytes());
            b.extend_from_slice(&count.to_le_bytes());
            b.extend_from_slice(&exif.to_le_bytes());
        }
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&(m as u16).to_le_bytes());
        for i in 0..m {
            b.extend_from_slice(&((0x9000 + i % 1000) as u16).to_le_bytes());
            b.extend_from_slice(&3u16.to_le_bytes());
            b.extend_from_slice(&1u32.to_le_bytes());
            b.extend_from_slice(&1u32.to_le_bytes());
        }
        b.extend_from_slice(&0u32.to_le_bytes());
        b
    }

    /// Ungeprüft ergäbe das 9 Millionen Felder und rund 1 GB aus 70 KB
    #[test]
    fn pointer_bomb_is_refused() {
        let f = bomb(3000, 3000, 4, 1);
        assert!(f.len() < 100_000);
        assert!(!fields_bounded(&f));
        assert!(read_tiff(f).is_none());
    }

    #[test]
    fn pointer_bomb_in_jpeg_is_refused() {
        let tiff = bomb(2700, 2700, 4, 1);
        let mut f = vec![0xFF, 0xD8, 0xFF, 0xE1];
        f.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
        f.extend_from_slice(b"Exif\0\0");
        f.extend_from_slice(&tiff);
        assert!(read_container(&mut Cursor::new(f)).is_none());
    }

    #[test]
    fn unusual_pointer_types_cannot_slip_past_the_count() {
        assert!(!fields_bounded(&bomb(10, 10, 1, 1)), "BYTE als Zeiger");
        assert!(!fields_bounded(&bomb(10, 10, 4, 2)), "mehrere Werte");
    }

    #[test]
    fn ordinary_and_broken_blocks_pass_the_count() {
        assert!(fields_bounded(&sample_tiff()));
        assert!(fields_bounded(&bomb(1, 500, 4, 1)));
        // Abgeschnitten oder Unsinn: das entscheidet der Parser
        assert!(fields_bounded(b"II*\0"));
        assert!(fields_bounded(b"II*\0\xFF\xFF\xFF\xFF"));
        assert!(fields_bounded(b"kein TIFF"));
    }

    /// Echte Fotos (`LUMENKORN_TEST_RAW`): gleiche Felder wie der Parser allein
    #[test]
    fn real_files_match_the_plain_parser() {
        let Ok(dir) = std::env::var("LUMENKORN_TEST_RAW") else {
            return;
        };
        let mut checked = 0;
        for e in walkdir::WalkDir::new(dir).into_iter().flatten() {
            if !e.file_type().is_file() {
                continue;
            }
            let bytes = std::fs::read(e.path()).unwrap();
            let plain = exif::Reader::new().read_from_container(&mut Cursor::new(&bytes));
            let guarded = read_container(&mut Cursor::new(&bytes));
            match (plain, guarded) {
                (Ok(a), Some(b)) => {
                    assert_eq!(a.fields().count(), b.fields().count(), "{:?}", e.path());
                    checked += 1;
                }
                (Err(_), None) => {}
                (a, b) => panic!(
                    "{:?}: Parser {:?}, geprüft {:?}",
                    e.path(),
                    a.is_ok(),
                    b.is_some()
                ),
            }
        }
        eprintln!("{checked} Dateien mit EXIF verglichen");
    }
}
