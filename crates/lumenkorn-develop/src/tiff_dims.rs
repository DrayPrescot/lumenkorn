//! Bildmaße aus den Einträgen einer TIFF-basierten Datei (die meisten RAWs), ohne
//! sie zu dekodieren. Für die Abmessungen im Katalog und als Größenprüfung vor
//! dem Entwickeln.

/// Ein Bild in der Datei (IFD mit Breite und Höhe)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// NewSubfileType: 0 ist das Vollbild, 1 eine Vorschau
    pub subfile: u32,
    pub width: u32,
    pub height: u32,
}

/// Alle Bilder in den IFDs und SubIFDs. Leer, wenn es kein TIFF ist.
pub fn frames(d: &[u8]) -> Vec<Frame> {
    let le = match d.get(0..4) {
        Some([b'I', b'I', 42, 0] | [b'I', b'I', 0x52, 0x4F] | [b'I', b'I', 0x55, 0]) => true, // TIFF, ORF, RW2
        Some([b'M', b'M', 0, 42]) => false,
        _ => return Vec::new(),
    };
    let u16_at = |o: usize| -> Option<u32> {
        let b = d.get(o..o.checked_add(2)?)?;
        Some(if le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        } as u32)
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b: [u8; 4] = d.get(o..o.checked_add(4)?)?.try_into().ok()?;
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
    let mut out = Vec::new();
    let Some(first) = u32_at(4) else {
        return out;
    };
    let mut queue = vec![first as usize];
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
        if let (Some(width), Some(height)) = (w, h) {
            out.push(Frame {
                subfile: sub,
                width,
                height,
            });
        }
        // nächstes IFD in der Kette
        if let Some(next) = u32_at(ifd + 2 + n as usize * 12) {
            queue.push(next as usize);
        }
    }
    out
}

/// Rohdaten belegen beim Entwickeln 2 B/Pixel, bevor das erste Pixel gelesen wird.
const RAW_BYTES_PER_PIXEL: u64 = 2;
/// Stärker komprimieren auch verlustbehaftete DNGs nicht (Maße gegen Dateigröße)
const MAX_COMPRESSION: u64 = 64;

/// Passen die Maße in der Datei zu ihrer Größe und zum Speicher? rawler legt den
/// Puffer unbesehen nach diesen Maßen an: Eine kleine Datei mit Breite und Höhe
/// 60 000 forderte 7 GB, und eine fehlgeschlagene Anforderung beendet den Prozess.
/// RAWs, die kein TIFF sind (CR3, RAF …), sind hiermit nicht geprüft.
pub(crate) fn raw_size_plausible(d: &[u8]) -> bool {
    frames(d).iter().all(|f| {
        let (w, h) = (f.width as usize, f.height as usize);
        let bytes = u64::from(f.width) * u64::from(f.height) * RAW_BYTES_PER_PIXEL;
        bytes / MAX_COMPRESSION <= d.len() as u64
            && crate::raster::dims_allowed(w, h, RAW_BYTES_PER_PIXEL)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiff(w: u32, h: u32, padding: usize) -> Vec<u8> {
        let mut d = b"II*\0".to_vec();
        d.extend_from_slice(&8u32.to_le_bytes());
        d.extend_from_slice(&2u16.to_le_bytes());
        for (tag, v) in [(0x0100u16, w), (0x0101, h)] {
            d.extend_from_slice(&tag.to_le_bytes());
            d.extend_from_slice(&4u16.to_le_bytes());
            d.extend_from_slice(&1u32.to_le_bytes());
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&0u32.to_le_bytes());
        d.resize(d.len() + padding, 0);
        d
    }

    #[test]
    fn finds_dimensions() {
        assert_eq!(
            frames(&tiff(6048, 4024, 0)),
            vec![Frame {
                subfile: 0,
                width: 6048,
                height: 4024
            }]
        );
        assert!(frames(b"kein tiff").is_empty());
        assert!(frames(b"II*\0\xFF\xFF\xFF\xFF").is_empty());
    }

    #[test]
    fn tiny_file_with_huge_dimensions_is_refused() {
        assert!(!raw_size_plausible(&tiff(60_000, 60_000, 200_000)));
        // Echtes 24-MP-RAW: 25 MB; auch stark komprimiert plausibel
        assert!(raw_size_plausible(&tiff(6048, 4024, 25 << 20)));
        assert!(raw_size_plausible(&tiff(6048, 4024, 3 << 20)));
        // Nicht-TIFF wird hier nicht beurteilt
        assert!(raw_size_plausible(b"FUJIFILMCCD-RAW"));
    }
}
