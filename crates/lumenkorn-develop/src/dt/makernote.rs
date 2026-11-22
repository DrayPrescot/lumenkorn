//! Lichterschutz der Kamera aus den Herstellerdaten (Makernotes), wie darktable
//! 5.6 `src/common/exif.cc` (`_check_highlight_preservation`).
//!
//! Modi wie Canon „Auto Lighting Optimizer“, Fuji DR200/DR400, Nikon HLG oder
//! Pentax „Dynamic Range Expansion“ belichten knapper, um Lichter zu schützen,
//! und hellen das JPEG in der Kamera wieder auf. darktable gleicht das in
//! exposure aus (`compensate_hilite_pres`); hier wird nur ermittelt, um wie
//! viele Blendenstufen.
//!
//! rawler gibt die Makernotes nicht heraus, deshalb ein eigener, kleiner Leser
//! für genau diese Tags. Die Datei kommt von außen: Jeder Zugriff ist auf
//! Grenzen geprüft, Einträge und Boxen sind gezählt, verschachtelt wird nur bis
//! zu einer festen Tiefe. Was nicht passt, ergibt 0 (kein Ausgleich).
//!
//! Abweichung von darktable: Fujis EXR-Kompakte (F600EXR, HS20EXR …) erreichen
//! DR400 über ihren Sensormodus, nicht durch knappere Belichtung – mit Ausgleich
//! war das Beispielbild der F600EXR zwei Blenden zu hell. Für Modelle mit „EXR“
//! im Namen gibt es daher keinen Fuji-Ausgleich.
//!
//! Wie darktable gilt bei Nikon: Steht `ColorSpace` in den Makernotes (fast
//! immer), entscheidet nur der HLG-Modus, Active D-Lighting wird dann nicht
//! betrachtet.

/// Höchstens so viele Einträge je IFD (echte Makernotes: einige hundert)
const MAX_ENTRIES: usize = 2000;
/// Höchstens so viele Boxen bzw. JPEG-Segmente werden durchsucht
const MAX_STEPS: usize = 4096;

/// Blendenstufen, um die die Kamera wegen ihres Lichterschutzes knapper
/// belichtet hat (0 = keiner oder unbekannt). Wie darktable begrenzt auf 0…4.
pub(crate) fn highlight_preservation(buf: &[u8], make: &str, model: &str) -> f32 {
    let v = detect(buf, make, model).unwrap_or(0.0);
    if v > 0.0 { v.min(4.0) } else { 0.0 }
}

fn detect(buf: &[u8], make: &str, model: &str) -> Option<f32> {
    let make = make.to_ascii_lowercase();
    // Canon CR3: Makernotes als eigenes TIFF in der Box CMT3
    if buf.get(4..8) == Some(b"ftyp") {
        if !make.contains("canon") {
            return None;
        }
        let cmt3 = cr3_box(buf, b"CMT3")?;
        let t = Tiff::new(cmt3)?;
        return canon(&t, &t.ifd(t.first_ifd()?)?);
    }
    // Fuji RAF: EXIF steckt im eingebetteten JPEG
    let tiff = if buf.starts_with(b"FUJIFILMCCD-RAW") {
        let off = be32(buf, 84)? as usize;
        let len = be32(buf, 88)? as usize;
        jpeg_exif(buf.get(off..off.checked_add(len)?)?)?
    } else if buf.starts_with(&[0xFF, 0xD8]) {
        jpeg_exif(buf)?
    } else {
        buf
    };
    let t = Tiff::new(tiff)?;
    let ifd0 = t.ifd(t.first_ifd()?)?;

    // DNG (Pentax/Ricoh): Makernotes im Tag DNGPrivateData – bei Pentax-DNGs
    // direkt („PENTAX \0“), bei konvertierten DNGs in Adobes Hülle
    if let Some(e) = ifd0.get(0xC634)
        && let Some(mn) = t.data(&e)
    {
        if mn.starts_with(b"Adobe\0MakN") {
            // "Adobe\0" "MakN", Länge (BE), Bytefolge, ursprünglicher Offset, Makernote
            let len = be32(mn, 10)? as usize;
            let body = mn.get(20..)?;
            let note = body.get(..len.saturating_sub(6).min(body.len()))?;
            return pentax(note, t.le);
        }
        if mn.starts_with(b"PENTAX \0") || mn.starts_with(b"AOC\0") {
            return pentax(mn, t.le);
        }
    }

    let exif = t.ifd(t.sub_ifd(&ifd0, 0x8769)?)?;
    let e = exif.get(0x927C)?;
    let note_off = e.offset(&t)?;
    let note = t.data(&e)?;
    if note.starts_with(b"FUJIFILM") {
        if model.to_ascii_uppercase().contains("EXR") {
            return None;
        }
        fuji(note)
    } else if note.starts_with(b"Nikon\0") {
        nikon(note)
    } else if note.starts_with(b"OLYMP") || note.starts_with(b"OM SYSTEM") {
        olympus(&t, note, note_off)
    } else if note.starts_with(b"AOC\0") || note.starts_with(b"PENTAX ") {
        pentax(note, t.le)
    } else if make.contains("canon") {
        // Canon: IFD ohne Kopf, Offsets relativ zum TIFF-Anfang
        canon(&t, &t.ifd(note_off)?)
    } else {
        None
    }
}

// --- Hersteller -------------------------------------------------------------

/// `Exif.Canon.LightingOpt` (0x4018): LONG-Feld, Wert [2] ist der ALO-Zustand
/// (0 Standard, 1 Niedrig, 2 Stark, 3 Aus).
fn canon(t: &Tiff, ifd: &Ifd) -> Option<f32> {
    let e = ifd.get(0x4018)?;
    let state = t.uint(&e, 2)?;
    Some(match state {
        0 => 0.50,
        1 => 0.33,
        2 => 0.66,
        _ => 0.0,
    })
}

/// Fuji: Little Endian, IFD-Offset bei 8, Offsets relativ zur Makernote.
/// `DevelopmentDynamicRange` (0x1403), sonst `AutoDynamicRange` (0x140B).
fn fuji(note: &[u8]) -> Option<f32> {
    let t = Tiff {
        data: note,
        le: true,
    };
    let ifd = t.ifd(le32(note, 8)? as usize)?;
    let e = ifd.get(0x1403).or_else(|| ifd.get(0x140B))?;
    Some(match t.uint(&e, 0)? {
        200 => 1.0,
        400 => 2.0,
        _ => 0.0,
    })
}

/// Nikon (Typ 3): eigener TIFF-Kopf ab Byte 10. `ColorSpace` (0x001E) = 4 ist
/// HLG (+2 EV); nur ohne `ColorSpace` zählt `ActiveDLighting` (0x0022).
fn nikon(note: &[u8]) -> Option<f32> {
    let t = Tiff::new(note.get(10..)?)?;
    let ifd = t.ifd(t.first_ifd()?)?;
    if let Some(e) = ifd.get(0x001E) {
        return Some(if t.uint(&e, 0)? == 4 { 2.0 } else { 0.0 });
    }
    let e = ifd.get(0x0022)?;
    Some(match t.uint(&e, 0)? {
        3 => 0.33,
        5 => 0.66,
        7 => 1.00,
        8 => 1.10,
        9 => 1.20,
        10 => 1.30,
        11 => 1.33,
        _ => 0.0,
    })
}

/// Olympus/OM: `CameraSettings` (0x2020) → `Gradation` (0x050F), drei oder vier
/// SSHORT. Neue Makernotes („OLYMPUS\0II“, „OM SYSTEM“) rechnen Offsets ab ihrem
/// Anfang, alte („OLYMP\0“) ab dem TIFF-Anfang.
fn olympus(t: &Tiff, note: &[u8], note_off: usize) -> Option<f32> {
    let (mt, ifd_at) = if note.starts_with(b"OLYMPUS\0") {
        (Tiff::with_order(note, note.get(8..10)?)?, 12)
    } else if note.starts_with(b"OM SYSTEM\0") {
        (Tiff::with_order(note, note.get(12..14)?)?, 16)
    } else {
        // alt: Offsets relativ zum TIFF
        let ifd = t.ifd(note_off.checked_add(8)?)?;
        return olympus_gradation(t, &ifd);
    };
    let ifd = mt.ifd(ifd_at)?;
    olympus_gradation(&mt, &ifd)
}

fn olympus_gradation(t: &Tiff, mn: &Ifd) -> Option<f32> {
    let cs = t.ifd(t.sub_ifd(mn, 0x2020)?)?;
    let e = cs.get(0x050F)?;
    let s = |i| t.sint(&e, i);
    let (s1, s2, s3) = (s(0)?, s(1)?, s(2)?);
    let s4 = if e.count > 3 { s(3)? } else { 0 };
    Some(if s4 == 1 {
        // „Auto“ belichtet etwa 0,3 EV knapper
        0.33
    } else if (s1, s2, s3) == (-1, -1, 1) {
        // „Low Key“: gemeldet −1,0 EV, 0,66 passt besser zum JPEG
        0.66
    } else {
        0.0
    })
}

/// Pentax/Ricoh: „AOC\0“ + Bytefolge (IFD ab 6) oder „PENTAX \0“ + Bytefolge
/// (IFD ab 10). `DynamicRangeExpansion` (0x0069): vier Bytes im Eintrag,
/// Byte 0 = 1 heißt an (eine Blende knapper).
fn pentax(note: &[u8], tiff_le: bool) -> Option<f32> {
    let (order, ifd_at) = if note.starts_with(b"AOC\0") {
        (note.get(4..6)?, 6)
    } else if note.starts_with(b"PENTAX \0") {
        (note.get(8..10)?, 10)
    } else {
        return None;
    };
    let t = Tiff::with_order(note, order).unwrap_or(Tiff {
        data: note,
        le: tiff_le,
    });
    let ifd = t.ifd(ifd_at)?;
    let e = ifd.get(0x0069)?;
    // Höchstens 4 Bytes: stehen im Eintrag selbst, kein Offset nötig
    if e.size()? > 4 {
        return None;
    }
    Some(if e.raw[0] == 1 { 1.0 } else { 0.0 })
}

// --- Container --------------------------------------------------------------

/// TIFF-Block aus dem APP1-Segment eines JPEG
fn jpeg_exif(jpeg: &[u8]) -> Option<&[u8]> {
    let mut p = 2usize;
    for _ in 0..MAX_STEPS {
        if *jpeg.get(p)? != 0xFF {
            return None;
        }
        let marker = *jpeg.get(p + 1)?;
        if marker == 0xD9 || marker == 0xDA {
            return None;
        }
        let len = be16(jpeg, p + 2)? as usize;
        let seg = jpeg.get(p + 4..p.checked_add(2 + len)?)?;
        if marker == 0xE1 && seg.starts_with(b"Exif\0\0") {
            return seg.get(6..);
        }
        p = p.checked_add(2 + len)?;
    }
    None
}

/// Inhalt einer Box in CR3 (ISO BMFF): `moov` → Canon-`uuid` → `name`
fn cr3_box<'a>(buf: &'a [u8], name: &[u8; 4]) -> Option<&'a [u8]> {
    const CANON_UUID: [u8; 16] = [
        0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a,
        0x48,
    ];
    let moov = find_box(buf, b"moov", |_| true)?;
    let uuid = find_box(moov, b"uuid", |b| b.starts_with(&CANON_UUID))?;
    find_box(uuid.get(16..)?, name, |_| true)
}

/// Erste Box `name` auf dieser Ebene, deren Inhalt `accept` erfüllt
fn find_box<'a>(
    mut data: &'a [u8],
    name: &[u8; 4],
    accept: impl Fn(&[u8]) -> bool,
) -> Option<&'a [u8]> {
    for _ in 0..MAX_STEPS {
        if data.len() < 8 {
            return None;
        }
        let size = be32(data, 0)? as u64;
        let (head, size) = match size {
            1 => (
                16usize,
                u64::from_be_bytes(data.get(8..16)?.try_into().ok()?),
            ),
            0 => (8, data.len() as u64),
            s => (8, s),
        };
        let size = usize::try_from(size).ok()?;
        if size < head || size > data.len() {
            return None;
        }
        let body = &data[head..size];
        if &data[4..8] == name && accept(body) {
            return Some(body);
        }
        data = &data[size..];
    }
    None
}

// --- TIFF -------------------------------------------------------------------

struct Tiff<'a> {
    data: &'a [u8],
    le: bool,
}

#[derive(Clone, Copy)]
struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    /// die vier Wert- bzw. Offset-Bytes
    raw: [u8; 4],
}

struct Ifd {
    entries: Vec<Entry>,
}

impl Ifd {
    fn get(&self, tag: u16) -> Option<Entry> {
        self.entries.iter().copied().find(|e| e.tag == tag)
    }
}

impl Entry {
    fn type_size(&self) -> Option<usize> {
        Some(match self.typ {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 => 8,
            _ => return None,
        })
    }
    fn size(&self) -> Option<usize> {
        self.type_size()?.checked_mul(self.count as usize)
    }
    fn offset(&self, t: &Tiff) -> Option<usize> {
        Some(t.u32_of(self.raw) as usize)
    }
}

impl<'a> Tiff<'a> {
    /// TIFF mit Kopf („II“/„MM“ und beliebiger Kennzahl, z. B. ORF „IIRO“)
    fn new(data: &'a [u8]) -> Option<Self> {
        Self::with_order(data, data.get(0..2)?)
    }

    fn with_order(data: &'a [u8], order: &[u8]) -> Option<Self> {
        match order {
            b"II" => Some(Tiff { data, le: true }),
            b"MM" => Some(Tiff { data, le: false }),
            _ => None,
        }
    }

    fn first_ifd(&self) -> Option<usize> {
        Some(self.u32_at(4)? as usize)
    }

    fn u16_at(&self, p: usize) -> Option<u16> {
        let b: [u8; 2] = self.data.get(p..p.checked_add(2)?)?.try_into().ok()?;
        Some(if self.le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    fn u32_at(&self, p: usize) -> Option<u32> {
        let b: [u8; 4] = self.data.get(p..p.checked_add(4)?)?.try_into().ok()?;
        Some(self.u32_of(b))
    }

    fn u32_of(&self, b: [u8; 4]) -> u32 {
        if self.le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        }
    }

    fn ifd(&self, at: usize) -> Option<Ifd> {
        let n = self.u16_at(at)? as usize;
        if n == 0 || n > MAX_ENTRIES {
            return None;
        }
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let p = at.checked_add(2 + 12 * i)?;
            entries.push(Entry {
                tag: self.u16_at(p)?,
                typ: self.u16_at(p + 2)?,
                count: self.u32_at(p + 4)?,
                raw: self.data.get(p + 8..p + 12)?.try_into().ok()?,
            });
        }
        Some(Ifd { entries })
    }

    /// Offset eines Unter-IFD (Typ LONG oder IFD)
    fn sub_ifd(&self, ifd: &Ifd, tag: u16) -> Option<usize> {
        let e = ifd.get(tag)?;
        matches!(e.typ, 4 | 13).then(|| e.offset(self))?
    }

    /// Daten eines Eintrags (im Eintrag selbst oder am Offset)
    fn data(&self, e: &Entry) -> Option<&'a [u8]> {
        let size = e.size()?;
        if size <= 4 {
            // Liegt im Eintrag – für Makernotes und DNGPrivateData nie der Fall
            return None;
        }
        let off = e.offset(self)?;
        self.data.get(off..off.checked_add(size)?)
    }

    /// Element `i` eines BYTE/SHORT/LONG-Feldes als Zahl
    fn uint(&self, e: &Entry, i: usize) -> Option<u32> {
        if i >= e.count as usize {
            return None;
        }
        let ts = e.type_size()?;
        let bytes: &[u8] = if e.size()? <= 4 {
            &e.raw
        } else {
            let off = e.offset(self)?;
            self.data.get(off..off.checked_add(e.size()?)?)?
        };
        let p = i.checked_mul(ts)?;
        let b = bytes.get(p..p + ts)?;
        Some(match (e.typ, ts) {
            (1 | 6 | 7, 1) => b[0] as u32,
            (3 | 8, 2) => {
                let a = [b[0], b[1]];
                (if self.le {
                    u16::from_le_bytes(a)
                } else {
                    u16::from_be_bytes(a)
                }) as u32
            }
            (4 | 9, 4) => self.u32_of([b[0], b[1], b[2], b[3]]),
            _ => return None,
        })
    }

    /// Element `i` als vorzeichenbehaftete Zahl (SSHORT/SLONG; SHORT wie SSHORT)
    fn sint(&self, e: &Entry, i: usize) -> Option<i32> {
        let v = self.uint(e, i)?;
        Some(match e.typ {
            3 | 8 => v as u16 as i16 as i32,
            1 | 6 => v as u8 as i8 as i32,
            _ => v as i32,
        })
    }
}

fn be16(b: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        b.get(p..p.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn be32(b: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        b.get(p..p.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn le32(b: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(p..p.checked_add(4)?)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Baut TIFF-Daten: IFDs werden hinten angehängt, Kinder vor den Eltern.
    struct B {
        buf: Vec<u8>,
        le: bool,
    }

    impl B {
        fn new(le: bool, head: &[u8]) -> Self {
            B {
                buf: head.to_vec(),
                le,
            }
        }
        fn u16(&self, v: u16) -> [u8; 2] {
            if self.le {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        }
        fn u32(&self, v: u32) -> [u8; 4] {
            if self.le {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        }
        /// Hängt Rohdaten an, liefert ihren Offset
        fn blob(&mut self, d: &[u8]) -> u32 {
            if self.buf.len() % 2 == 1 {
                self.buf.push(0);
            }
            let at = self.buf.len() as u32;
            self.buf.extend_from_slice(d);
            at
        }
        /// IFD mit (Tag, Typ, Anzahl, Wertbytes); Werte > 4 Bytes kommen davor
        fn ifd(&mut self, entries: &[(u16, u16, u32, Vec<u8>)]) -> u32 {
            let vals: Vec<[u8; 4]> = entries
                .iter()
                .map(|(_, _, _, v)| {
                    if v.len() <= 4 {
                        let mut r = [0u8; 4];
                        r[..v.len()].copy_from_slice(v);
                        r
                    } else {
                        let at = self.blob(v);
                        self.u32(at)
                    }
                })
                .collect();
            let at = self.blob(&[]);
            let n = self.u16(entries.len() as u16);
            self.buf.extend_from_slice(&n);
            for ((tag, typ, count, _), v) in entries.iter().zip(vals) {
                let (t, y, c) = (self.u16(*tag), self.u16(*typ), self.u32(*count));
                self.buf.extend_from_slice(&t);
                self.buf.extend_from_slice(&y);
                self.buf.extend_from_slice(&c);
                self.buf.extend_from_slice(&v);
            }
            self.buf.extend_from_slice(&[0; 4]);
            at
        }
        fn shorts(&self, v: &[u16]) -> Vec<u8> {
            v.iter().flat_map(|x| self.u16(*x)).collect()
        }
        fn longs(&self, v: &[u32]) -> Vec<u8> {
            v.iter().flat_map(|x| self.u32(*x)).collect()
        }
        /// Fertiges TIFF mit Kopf und erstem IFD
        fn finish_tiff(mut self, first: u32) -> Vec<u8> {
            let f = self.u32(first);
            self.buf[4..8].copy_from_slice(&f);
            self.buf
        }
    }

    fn tiff_head(le: bool) -> &'static [u8] {
        if le {
            b"II*\0\0\0\0\0"
        } else {
            b"MM\0*\0\0\0\0"
        }
    }

    /// TIFF-RAW: IFD0 → Exif-IFD → MakerNote (fertiger Block)
    fn raw_with_note(le: bool, note: &[u8]) -> Vec<u8> {
        let mut b = B::new(le, tiff_head(le));
        let exif = b.ifd(&[(0x927C, 7, note.len() as u32, note.to_vec())]);
        let p = b.u32(exif).to_vec();
        let ifd0 = b.ifd(&[(0x8769, 4, 1, p)]);
        b.finish_tiff(ifd0)
    }

    #[test]
    fn canon_cr2_alo() {
        for (state, ev) in [(0, 0.5), (1, 0.33), (2, 0.66), (3, 0.0)] {
            let mut b = B::new(true, tiff_head(true));
            // Canon: IFD ohne Kopf, Offsets relativ zum TIFF
            let lo = b.longs(&[56, 0, state, 0, 0, 0, 1, 0, 1, 1, 1, 0, 1, u32::MAX]);
            let canon = b.ifd(&[(0x0001, 3, 1, b.shorts(&[0])), (0x4018, 4, 14, lo)]);
            let canon_len = b.buf.len() as u32 - canon;
            let exif = b.ifd(&[(0x927C, 7, canon_len, vec![])]);
            // MakerNote zeigt auf das Canon-IFD
            let e = exif as usize + 2 + 8;
            let v = b.u32(canon);
            b.buf[e..e + 4].copy_from_slice(&v);
            let p = b.u32(exif).to_vec();
            let ifd0 = b.ifd(&[(0x8769, 4, 1, p)]);
            let f = b.finish_tiff(ifd0);
            assert_eq!(
                highlight_preservation(&f, "Canon", ""),
                ev,
                "Zustand {state}"
            );
            // Ohne Hersteller Canon kein Ausgleich (IFD ohne Kennung)
            assert_eq!(highlight_preservation(&f, "Sony", ""), 0.0);
        }
    }

    #[test]
    fn canon_cr3_alo() {
        let mut t = B::new(true, tiff_head(true));
        let lo = t.longs(&[64, 0, 0, 0, 1, 2]);
        let ifd = t.ifd(&[(0x4018, 4, 6, lo)]);
        let cmt3 = t.finish_tiff(ifd);
        let bx = |name: &[u8], body: &[u8]| {
            let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(name);
            v.extend_from_slice(body);
            v
        };
        let mut uuid_body = vec![
            0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b,
            0x6a, 0x48,
        ];
        uuid_body.extend(bx(b"CMT1", b"xx"));
        uuid_body.extend(bx(b"CMT3", &cmt3));
        let mut file = bx(b"ftyp", b"crx \0\0\0\x01");
        file.extend(bx(b"moov", &bx(b"uuid", &uuid_body)));
        assert_eq!(highlight_preservation(&file, "Canon", ""), 0.5);
    }

    fn fuji_note(tag: u16, dr: u16) -> Vec<u8> {
        let mut n = B::new(true, b"FUJIFILM\x0c\0\0\0");
        let ifd = n.ifd(&[
            (0x1000, 2, 4, b"0100".to_vec()),
            (tag, 3, 1, n.shorts(&[dr])),
        ]);
        assert_eq!(ifd, 12);
        n.buf
    }

    #[test]
    fn fuji_dynamic_range_in_raf() {
        for (tag, dr, ev) in [
            (0x1403, 400, 2.0),
            (0x1403, 200, 1.0),
            (0x140B, 200, 1.0),
            (0x1403, 100, 0.0),
        ] {
            let exif = raw_with_note(false, &fuji_note(tag, dr));
            let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0];
            jpeg.extend_from_slice(&[0xFF, 0xE1]);
            jpeg.extend_from_slice(&((exif.len() + 8) as u16).to_be_bytes());
            jpeg.extend_from_slice(b"Exif\0\0");
            jpeg.extend_from_slice(&exif);
            jpeg.extend_from_slice(&[0xFF, 0xDA, 0, 2, 0xFF, 0xD9]);
            let mut raf = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
            raf.resize(100, 0);
            raf[84..88].copy_from_slice(&100u32.to_be_bytes());
            raf[88..92].copy_from_slice(&(jpeg.len() as u32).to_be_bytes());
            raf.extend_from_slice(&jpeg);
            assert_eq!(
                highlight_preservation(&raf, "Fujifilm", ""),
                ev,
                "{tag:x} {dr}"
            );
            // EXR-Sensoren: DR über den Sensormodus, kein Ausgleich
            assert_eq!(
                highlight_preservation(&raf, "Fujifilm", "FinePix F600EXR"),
                0.0
            );
        }
    }

    fn nikon_note(entries: &[(u16, u16)]) -> Vec<u8> {
        let mut inner = B::new(false, tiff_head(false));
        let list: Vec<_> = entries
            .iter()
            .map(|(t, v)| (*t, 3, 1, inner.shorts(&[*v])))
            .collect();
        let ifd = inner.ifd(&list);
        let mut note = b"Nikon\0\x02\x10\0\0".to_vec();
        note.extend(inner.finish_tiff(ifd));
        note
    }

    #[test]
    fn nikon_hlg_and_active_d_lighting() {
        let f = |e: &[(u16, u16)]| {
            highlight_preservation(&raw_with_note(false, &nikon_note(e)), "Nikon", "")
        };
        assert_eq!(f(&[(0x001E, 4)]), 2.0);
        assert_eq!(
            f(&[(0x001E, 1), (0x0022, 5)]),
            0.0,
            "wie darktable: ColorSpace entscheidet"
        );
        assert_eq!(f(&[(0x0022, 5)]), 0.66);
        assert_eq!(f(&[(0x0022, 11)]), 1.33);
        assert_eq!(f(&[(0x0022, 0xFFFF)]), 0.0);
    }

    #[test]
    fn olympus_gradation() {
        let note = |g: &[u16]| {
            // Das Haupt-IFD muss bei 12 liegen: Platz dafür (1 Eintrag = 18 Bytes)
            // freihalten, CameraSettings dahinter, dann das Haupt-IFD einsetzen
            let mut out = B::new(true, b"OLYMPUS\0II\x03\0");
            out.buf.resize(12 + 18, 0);
            let gr: Vec<u8> = g.iter().flat_map(|v| v.to_le_bytes()).collect();
            let cs = out.ifd(&[
                (0x0100, 3, 1, out.shorts(&[0])),
                (0x050F, 8, g.len() as u32, gr),
            ]);
            let main = {
                let mut m = B::new(true, &[]);
                m.ifd(&[(0x2020, 13, 1, out.u32(cs).to_vec())]);
                m.buf
            };
            out.buf[12..12 + main.len()].copy_from_slice(&main);
            out.buf
        };
        let ev = |g: &[u16]| highlight_preservation(&raw_with_note(true, &note(g)), "Olympus", "");
        let m1 = (-1i16) as u16;
        assert_eq!(ev(&[m1, m1, 1]), 0.66, "Low Key");
        assert_eq!(ev(&[0, 0, 0, 1]), 0.33, "Auto-Override");
        assert_eq!(ev(&[0, 0, 0, 0]), 0.0);
        assert_eq!(ev(&[1, m1, 1]), 0.0, "High Key: nicht ausgeglichen");
    }

    #[test]
    fn pentax_pef_and_dng() {
        let note = |head: &[u8], ifd_at: usize, dre: [u8; 4]| {
            let mut n = B::new(false, head);
            assert_eq!(n.buf.len(), ifd_at);
            n.ifd(&[
                (0x0000, 1, 4, vec![4, 0, 0, 0]),
                (0x0069, 7, 4, dre.to_vec()),
            ]);
            n.buf
        };
        for (dre, ev) in [
            ([1, 0, 0, 0], 1.0),
            ([1, 2, 0, 0], 1.0),
            ([0, 2, 0, 0], 0.0),
        ] {
            // PEF: Makernote im Exif-IFD
            let pef = raw_with_note(false, &note(b"AOC\0MM", 6, dre));
            assert_eq!(highlight_preservation(&pef, "Pentax", ""), ev);
            // DNG: Makernote direkt als DNGPrivateData
            let mut b = B::new(false, tiff_head(false));
            let pn = note(b"PENTAX \0MM", 10, dre);
            let ifd0 = b.ifd(&[(0xC634, 1, pn.len() as u32, pn)]);
            assert_eq!(
                highlight_preservation(&b.finish_tiff(ifd0), "Pentax", ""),
                ev
            );
        }
    }

    /// Kaputte Daten: nie ein Absturz, nie ein Wert außerhalb 0…4
    #[test]
    fn broken_data_is_harmless() {
        let good = raw_with_note(false, &nikon_note(&[(0x001E, 4)]));
        for cut in 0..good.len() {
            let v = highlight_preservation(&good[..cut], "Nikon", "");
            assert!((0.0..=4.0).contains(&v));
        }
        // Jedes Byte einmal verändern
        for i in 0..good.len() {
            let mut b = good.clone();
            b[i] ^= 0xFF;
            let v = highlight_preservation(&b, "Nikon", "");
            assert!((0.0..=4.0).contains(&v));
        }
        // IFD mit riesiger Eintragszahl, Offsets ins Leere
        let mut b = tiff_head(true).to_vec();
        b[4] = 8;
        b.extend_from_slice(&[
            0xFF, 0xFF, 0x69, 0x87, 4, 0, 1, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF,
        ]);
        assert_eq!(highlight_preservation(&b, "Canon", ""), 0.0);
        // Box-Größen, die nicht passen
        let mut cr3 = vec![0, 0, 0, 16];
        cr3.extend_from_slice(b"ftypcrx \0\0\0\0");
        cr3.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xF0]);
        cr3.extend_from_slice(b"moov");
        assert_eq!(highlight_preservation(&cr3, "Canon", ""), 0.0);
        // Werte außerhalb werden begrenzt (darktable: höchstens 4)
        assert_eq!(highlight_preservation(&[], "Canon", ""), 0.0);
    }

    /// Echte Dateien: `LUMENKORN_TEST_MAKERNOTE=<Ordner>` gibt je Datei den Wert
    /// aus; abgeschnittene Fassungen dürfen nicht abstürzen
    #[test]
    fn real_files_from_env() {
        let Ok(dir) = std::env::var("LUMENKORN_TEST_MAKERNOTE") else {
            return;
        };
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        names.sort();
        for p in names {
            let buf = std::fs::read(&p).unwrap();
            let src =
                rawler::rawsource::RawSource::new_from_shared_vec(std::sync::Arc::new(buf.clone()));
            let (make, model) = rawler::get_decoder(&src)
                .ok()
                .and_then(|d| d.raw_metadata(&src, &Default::default()).ok())
                .map(|m| (m.make, m.model))
                .unwrap_or_default();
            eprintln!(
                "{:>5.2} EV  {:<10} {}",
                highlight_preservation(&buf, &make, &model),
                make,
                p.file_name().unwrap().to_string_lossy()
            );
            for cut in (0..buf.len().min(400_000)).step_by(97) {
                let v = highlight_preservation(&buf[..cut], &make, &model);
                assert!((0.0..=4.0).contains(&v));
            }
        }
    }
}
