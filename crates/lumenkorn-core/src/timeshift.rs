//! Aufnahmezeit verschieben, z. B. wenn die Kamerauhr noch auf der Zeitzone
//! daheim stand. Die Zeiten stehen in EXIF als Text fester Länge
//! (`JJJJ:MM:TT hh:mm:ss`), in XMP als `JJJJ-MM-TTThh:mm:ss`. Beide lassen
//! sich beim Kopieren im Datenstrom ersetzen, ohne dass sich Größe oder
//! Aufbau der Datei ändern – das gilt für JPEG, TIFF-basierte RAWs (ARW, CR2,
//! NEF, DNG …), CR3, HEIF und XMP-Begleitdateien, samt der Kopien in
//! eingebetteten Vorschauen.
//!
//! Verschoben werden nur Zeiten nahe der Aufnahmezeit (± 2 Tage), damit
//! andere Datumsangaben (Firmware, Urheberrecht) bleiben, wie sie sind.
//! GPS-Zeiten sind UTC und als Zahlen gespeichert – sie bleiben unberührt.
//! Ebenso bleiben die Zeitzonenfelder (`OffsetTime*`) unverändert: Sie
//! beschreiben die Zone der Kamera, nicht den Zeitpunkt.

use crate::geotime::parse_time;

/// Länge einer Zeitangabe (`JJJJ:MM:TT hh:mm:ss`)
const LEN: usize = 19;
/// Nur Zeiten so nah (s) an der Aufnahmezeit werden verschoben
const NEAR_S: i64 = 2 * 86_400;

/// Sekunden seit 1970 als `JJJJ-MM-TTThh:mm:ss`
pub fn format_time(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Tage seit 1970 → Kalenderdatum (Howard Hinnant, „civil_from_days“)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// `2024-10-10T12:41:45` um `by` Sekunden verschoben (gleiches Format)
pub fn shift_iso(s: &str, by: i64) -> Option<String> {
    Some(format_time(parse_time(s)?.checked_add(by)?))
}

/// Ersetzt beim Durchlaufen eines Datenstroms alle Zeitangaben nahe
/// `around` durch die um `by` verschobenen. Ausgabe ist genauso lang wie die
/// Eingabe.
pub struct DateShifter {
    by: i64,
    around: i64,
    /// Zurückgehaltenes Ende des letzten Stücks (eine Zeitangabe kann über
    /// die Grenze zweier Stücke reichen)
    carry: Vec<u8>,
    /// Ab dieser Stelle in `carry` darf ein neuer Treffer beginnen
    skip: usize,
    /// Zahl der ersetzten Zeitangaben
    pub replaced: usize,
}

impl DateShifter {
    /// `around`: Aufnahmezeit (Sekunden seit 1970, wie [`parse_time`])
    pub fn new(by: i64, around: i64) -> Self {
        DateShifter {
            by,
            around,
            carry: Vec::new(),
            skip: 0,
            replaced: 0,
        }
    }

    /// Verarbeitet das nächste Stück; hängt an `out` an, was fertig ist
    /// (das Ende bleibt bis zum nächsten Stück zurück).
    pub fn feed(&mut self, chunk: &[u8], out: &mut Vec<u8>) {
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(chunk);
        let mut i = self.skip;
        let mut next_free = 0;
        while i + LEN <= buf.len() {
            if let Some(new) = self.shifted_at(&buf[i..i + LEN]) {
                buf[i..i + LEN].copy_from_slice(&new);
                self.replaced += 1;
                i += LEN;
                next_free = i;
            } else {
                i += 1;
            }
        }
        // Die letzten LEN-1 Bytes zurückhalten: dort kann eine Zeitangabe
        // beginnen, die erst im nächsten Stück endet
        let keep = buf.len().min(LEN - 1);
        let split = buf.len() - keep;
        out.extend_from_slice(&buf[..split]);
        self.skip = next_free.saturating_sub(split);
        self.carry = buf[split..].to_vec();
    }

    /// Gibt den zurückgehaltenen Rest aus (kann keine Zeitangabe mehr sein).
    pub fn finish(&mut self, out: &mut Vec<u8>) {
        out.append(&mut self.carry);
        self.skip = 0;
    }

    /// Verschobene Zeitangabe, wenn `s` eine gültige ist und nahe der
    /// Aufnahmezeit liegt
    fn shifted_at(&self, s: &[u8]) -> Option<[u8; LEN]> {
        let exif = match (s[4], s[7], s[10], s[13], s[16]) {
            (b':', b':', b' ', b':', b':') => true,
            (b'-', b'-', b'T', b':', b':') => false,
            _ => return None,
        };
        let digits = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
        if !digits.iter().all(|&k| s[k].is_ascii_digit()) {
            return None;
        }
        let mut iso = [0u8; LEN];
        iso.copy_from_slice(s);
        if exif {
            iso[4] = b'-';
            iso[7] = b'-';
            iso[10] = b'T';
        }
        let t = parse_time(std::str::from_utf8(&iso).ok()?)?;
        if (t - self.around).abs() > NEAR_S {
            return None;
        }
        let shifted = format_time(t.checked_add(self.by)?);
        // Jahr außerhalb 0000–9999 ergäbe eine andere Länge
        if shifted.len() != LEN {
            return None;
        }
        let mut new = [0u8; LEN];
        new.copy_from_slice(shifted.as_bytes());
        if exif {
            new[4] = b':';
            new[7] = b':';
            new[10] = b' ';
        }
        Some(new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_and_shifts() {
        assert_eq!(format_time(0), "1970-01-01T00:00:00");
        let t = parse_time("2024-10-10T12:41:45").unwrap();
        assert_eq!(format_time(t), "2024-10-10T12:41:45");
        assert_eq!(
            shift_iso("2024-12-31T20:00:00", 6 * 3600).as_deref(),
            Some("2025-01-01T02:00:00")
        );
        assert_eq!(
            shift_iso("2024-03-01T01:00:00", -2 * 3600).as_deref(),
            Some("2024-02-29T23:00:00")
        );
    }

    fn run(data: &[u8], by: i64, around: &str, chunk: usize) -> (Vec<u8>, usize) {
        let mut s = DateShifter::new(by, parse_time(around).unwrap());
        let mut out = Vec::new();
        for c in data.chunks(chunk.max(1)) {
            s.feed(c, &mut out);
        }
        s.finish(&mut out);
        (out, s.replaced)
    }

    #[test]
    fn replaces_exif_and_xmp_times_across_chunk_borders() {
        let data = b"\0\x02DateTime\x002024:10:10 12:41:45\0xx\
            <xmp:CreateDate>2024-10-10T12:41:45.123+02:00</xmp:CreateDate>\
            firmware 2019:01:01 00:00:00 gps 2024:10:10 \0\0";
        let want = b"\0\x02DateTime\x002024:10:10 19:46:45\0xx\
            <xmp:CreateDate>2024-10-10T19:46:45.123+02:00</xmp:CreateDate>\
            firmware 2019:01:01 00:00:00 gps 2024:10:10 \0\0";
        // Jede Stückgröße: Treffer über Stückgrenzen hinweg
        for chunk in 1..=data.len() + 1 {
            let (out, n) = run(data, 7 * 3600 + 300, "2024-10-10T12:41:45", chunk);
            assert_eq!(out.len(), data.len());
            assert_eq!(
                String::from_utf8_lossy(&out),
                String::from_utf8_lossy(want),
                "Stück {chunk}"
            );
            assert_eq!(n, 2, "Stück {chunk}");
        }
    }

    #[test]
    fn rejects_impossible_days_and_overflow() {
        assert!(parse_time("2023-02-29T00:00:00").is_none());
        assert!(parse_time("2024-02-29T00:00:00").is_some());
        assert!(parse_time("2024-04-31T00:00:00").is_none());
        assert!(parse_time("2100-02-29T00:00:00").is_none());
        // Ende des Jahres 9999: Ergebnis hätte 5 Stellen, keine Panik
        let (out, n) = run(b"9999:12:31 23:00:00", 7200, "9999-12-31T23:00:00", 4);
        assert_eq!(&out[..], b"9999:12:31 23:00:00");
        assert_eq!(n, 0);
        let mut s = DateShifter::new(i64::MAX, parse_time("2024-01-01T00:00:00").unwrap());
        let mut out = Vec::new();
        s.feed(b"2024:01:01 00:00:00", &mut out);
        s.finish(&mut out);
        assert_eq!(s.replaced, 0);
        assert!(shift_iso("2024-01-01T00:00:00", i64::MAX).is_none());
    }

    #[test]
    fn leaves_invalid_and_distant_values() {
        // Ungültiger Monat, Ziffern fehlen, zu weit weg
        let data = b"2024:13:10 12:41:45 2024:1a:10 12:41:45 2020:10:10 12:41:45";
        let (out, n) = run(data, 3600, "2024-10-10T12:00:00", 7);
        assert_eq!(&out[..], &data[..]);
        assert_eq!(n, 0);
    }

    #[test]
    fn adjacent_times_are_both_replaced() {
        let data = b"2024:10:10 12:41:452024:10:10 12:41:46";
        let (out, n) = run(data, 60, "2024-10-10T12:41:45", 5);
        assert_eq!(&out[..], b"2024:10:10 12:42:452024:10:10 12:42:46");
        assert_eq!(n, 2);
    }
}
