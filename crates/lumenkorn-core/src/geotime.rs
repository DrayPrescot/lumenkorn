//! Ort aus zeitgleichen Fotos: Fotos ohne Ort (z. B. von einer Kamera ohne
//! GPS) bekommen den Ort eines Fotos mit Ort (z. B. vom Handy), das wenige
//! Minuten davor oder danach aufgenommen wurde. Die Uhren der Geräte gehen
//! oft falsch (Minuten, Zeitzone, Sommerzeit); der Versatz wird deshalb
//! angewendet und bei Bedarf aus den Aufnahmezeiten geschätzt.
//!
//! Das Modul schreibt nichts in den Katalog, es liefert nur Vorschläge.

use crate::catalog::Catalog;
use crate::error::Result;
use crate::model::ImageQuery;
use rusqlite::params_from_iter;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Sekunden seit 1970-01-01 für `JJJJ-MM-TTTHH:MM:SS` (ohne Zeitzone; auch
/// mit Leerzeichen statt `T`; Sekundenbruchteile und Zonenzusatz werden
/// ignoriert). `None`, wenn der Text nicht passt.
pub fn parse_time(s: &str) -> Option<i64> {
    let b = s.trim().as_bytes();
    if b.len() < 19 || !(b[10] == b'T' || b[10] == b' ') {
        return None;
    }
    if b[4] != b'-' || b[7] != b'-' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |a: usize, e: usize| -> Option<i64> {
        let part = &b[a..e];
        if part.iter().all(u8::is_ascii_digit) {
            std::str::from_utf8(part).ok()?.parse().ok()
        } else {
            None
        }
    };
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let month_days = match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if d > month_days {
        return None;
    }
    // Tage seit 1970 (Howard Hinnant, days_from_civil)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + se)
}

/// Versätze, die so weit (s) vom besten entfernt liegen, gelten als
/// Konkurrenz (siehe [`estimate_offset`]).
const RIVAL_DISTANCE_S: i64 = 3600;
/// Suchbereich der Schätzung: Kameras laufen oft noch auf der Zeit daheim,
/// das sind bis zu ±14 Stunden.
pub const MAX_OFFSET_S: i64 = 14 * 3600;

/// Schätzt den Uhrenversatz `o` (Sekunden, in `[-max, max]` im Raster
/// `step_s`), sodass möglichst viele Zeiten aus `targets` nach `t + o` eine
/// Referenzzeit aus `refs` im Abstand von höchstens `tolerance_s` haben.
/// Rückgabe: (Versatz, Zahl der Treffer).
///
/// Ein Ergebnis gilt nur als eindeutig, wenn
/// * mindestens 3 Treffer vorliegen,
/// * der Versatz 0 ist (bei Gleichstand gewinnt der Versatz nächst 0) oder
///   die Trefferzahl deutlich über der bei Versatz 0 liegt, nämlich
///   `best >= 1,5 * treffer(0) + 2`, und
/// * kein anderer Versatz, der mehr als eine Stunde daneben liegt, auf mehr
///   als die Hälfte der Treffer kommt. Fotos entstehen in Serien; liegen
///   zwei Serien zufällig übereinander, gibt das auch bei falschem Versatz
///   viele Treffer. Ein echter Versatz hebt sich davon deutlich ab.
///
/// Liegt der Gipfel auf einem Plateau (Toleranz größer als das Raster), wird
/// dessen Mitte genommen. Sonst `None` (der Aufrufer nimmt dann 0).
pub fn estimate_offset(
    targets: &[i64],
    refs: &[i64],
    max_offset_s: i64,
    step_s: i64,
    tolerance_s: i64,
) -> Option<(i64, usize)> {
    if targets.is_empty() || refs.is_empty() || step_s <= 0 {
        return None;
    }
    let mut t: Vec<i64> = targets.to_vec();
    t.sort_unstable();
    let mut r: Vec<i64> = refs.to_vec();
    r.sort_unstable();
    r.dedup();

    // Treffer bei einem Versatz; Ziele sind sortiert, der Zeiger wandert nur vor.
    let count = |o: i64| -> usize {
        let mut j = 0;
        let mut n = 0;
        for &x in &t {
            let lo = x + o - tolerance_s;
            while j < r.len() && r[j] < lo {
                j += 1;
            }
            if j < r.len() && r[j] <= x + o + tolerance_s {
                n += 1;
            }
        }
        n
    };

    let k_max = (max_offset_s / step_s).max(0);
    let counts: Vec<usize> = (-k_max..=k_max).map(|k| count(k * step_s)).collect();
    let at = |k: i64| counts[(k + k_max) as usize];
    let c0 = at(0);
    let top = *counts.iter().max().unwrap_or(&0);
    // Gipfel mit der höchsten Trefferzahl, bei Gleichstand der nächste an 0
    let k_best = (-k_max..=k_max)
        .filter(|&k| at(k) == top)
        .min_by_key(|k| k.abs())
        .unwrap_or(0);
    // Liegt der Gipfel auf einem Plateau (Toleranz größer als das Raster),
    // wird dessen Mitte genommen.
    let (mut lo, mut hi) = (k_best, k_best);
    while lo > -k_max && at(lo - 1) == top {
        lo -= 1;
    }
    while hi < k_max && at(hi + 1) == top {
        hi += 1;
    }
    let best = (((lo + hi) / 2) * step_s, top);
    if best.1 < 3 {
        return None;
    }
    if best.0 != 0 && (best.1 as f64) < 1.5 * c0 as f64 + 2.0 {
        return None;
    }
    let rival = (-k_max..=k_max)
        .filter(|&k| (k * step_s - best.0).abs() > RIVAL_DISTANCE_S)
        .map(at)
        .max()
        .unwrap_or(0);
    if best.1 < 2 * rival {
        return None;
    }
    Some(best)
}

/// Ein Foto mit Ort als Quelle.
#[derive(Debug, Clone)]
pub struct Ref {
    pub id: i64,
    pub time: i64,
    pub lat: f64,
    pub lon: f64,
    pub place: Option<String>,
}

/// Vorschlag: Foto `id` bekommt den Ort von `ref_id`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimeMatch {
    pub id: i64,
    pub ref_id: i64,
    pub lat: f64,
    pub lon: f64,
    pub place: Option<String>,
    /// Zeit der Referenz minus (Zeit des Fotos + Versatz), in Sekunden
    pub delta_s: i64,
}

/// Für jedes Ziel (ID, Zeit) die Referenz mit der Zeit nächst `Zeit + offset_s`,
/// wenn sie höchstens `window_s` entfernt liegt. Bei gleichem Abstand gewinnt
/// die frühere.
pub fn match_by_time(
    targets: &[(i64, i64)],
    refs: &[Ref],
    offset_s: i64,
    window_s: i64,
) -> Vec<TimeMatch> {
    let mut sorted: Vec<&Ref> = refs.iter().collect();
    sorted.sort_by_key(|r| r.time);
    let mut out = Vec::new();
    for &(id, time) in targets {
        let want = time + offset_s;
        let p = sorted.partition_point(|r| r.time < want);
        let cands = [p.checked_sub(1), (p < sorted.len()).then_some(p)];
        let best = cands
            .into_iter()
            .flatten()
            .map(|i| sorted[i])
            .min_by_key(|r| (r.time - want).abs());
        if let Some(r) = best {
            let delta = r.time - want;
            if delta.abs() <= window_s {
                out.push(TimeMatch {
                    id,
                    ref_id: r.id,
                    lat: r.lat,
                    lon: r.lon,
                    place: r.place.clone(),
                    delta_s: delta,
                });
            }
        }
    }
    out
}

/// Uhrenversatz einer Kamera
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraOffset {
    pub camera: Option<String>,
    /// Sekunden, die zur Aufnahmezeit der Kamera addiert werden
    pub offset_s: i64,
    /// aus den Zeiten geschätzt (sonst vorgegeben oder 0)
    pub estimated: bool,
    /// Schätzung gewünscht, aber nicht eindeutig: Fotos anderer Geräte werden
    /// dann nicht zugeordnet, der Versatz muss angegeben werden.
    pub uncertain: bool,
    pub matched: usize,
    pub photos: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeSuggestions {
    pub matches: Vec<TimeMatch>,
    pub offsets: Vec<CameraOffset>,
    /// Fotos ohne Ort und ohne lesbare Aufnahmezeit
    pub without_time: usize,
    /// Fotos ohne Ort mit Aufnahmezeit
    pub candidates: usize,
}

struct CamRef {
    r: Ref,
    camera: Option<String>,
}

impl Catalog {
    /// Aufnahmezeiten aller Fotos mit Ort, außer denen der Kamera `camera`
    /// (Referenz für [`estimate_offset`], z. B. beim Import).
    pub fn reference_times(&self, camera: Option<&str>) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare(
            "SELECT captured_at FROM images
             WHERE missing = 0 AND gps_lat IS NOT NULL AND gps_lon IS NOT NULL
               AND captured_at IS NOT NULL AND camera IS NOT ?1",
        )?;
        let rows = st.query_map([camera], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            if let Some(t) = parse_time(&row?) {
                out.push(t);
            }
        }
        Ok(out)
    }

    /// Orte für Fotos ohne Ort aus zeitgleichen Fotos mit Ort vorschlagen.
    /// `offset_s`: fester Uhrenversatz für alle Kameras, sonst wird er je
    /// Kamera geschätzt. Schreibt nichts in den Katalog.
    pub fn time_suggestions(
        &self,
        q: &ImageQuery,
        offset_s: Option<i64>,
        window_s: i64,
    ) -> Result<TimeSuggestions> {
        // Ziele: Fotos der Auswahl ohne Ort
        let (where_sql, args) = self.where_clause(q)?;
        let mut st = self.conn.prepare(&format!(
            "SELECT i.id, i.captured_at, i.camera FROM images i
             WHERE {where_sql} AND (i.gps_lat IS NULL OR i.gps_lon IS NULL)"
        ))?;
        let rows = st.query_map(params_from_iter(args), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        let mut groups: BTreeMap<Option<String>, Vec<(i64, i64)>> = BTreeMap::new();
        let mut without_time = 0;
        let mut candidates = 0;
        for row in rows {
            let (id, at, cam) = row?;
            match at.as_deref().and_then(parse_time) {
                Some(t) => {
                    groups.entry(cam).or_default().push((id, t));
                    candidates += 1;
                }
                None => without_time += 1,
            }
        }
        drop(st);

        // Referenzen: alle Fotos mit Ort
        let mut st = self.conn.prepare(
            "SELECT id, captured_at, camera, gps_lat, gps_lon, place FROM images
             WHERE missing = 0 AND gps_lat IS NOT NULL AND gps_lon IS NOT NULL
               AND captured_at IS NOT NULL",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, f64>(3)?,
                r.get::<_, f64>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })?;
        let mut all_refs: Vec<CamRef> = Vec::new();
        for row in rows {
            let (id, at, camera, lat, lon, place) = row?;
            if let Some(time) = parse_time(&at) {
                all_refs.push(CamRef {
                    r: Ref {
                        id,
                        time,
                        lat,
                        lon,
                        place: place.filter(|p| !p.is_empty()),
                    },
                    camera,
                });
            }
        }
        drop(st);

        let mut matches = Vec::new();
        let mut offsets = Vec::new();
        for (camera, targets) in groups {
            let same: Vec<Ref> = all_refs
                .iter()
                .filter(|c| c.camera == camera)
                .map(|c| c.r.clone())
                .collect();
            let other: Vec<Ref> = all_refs
                .iter()
                .filter(|c| c.camera != camera)
                .map(|c| c.r.clone())
                .collect();
            let (offset, estimated, uncertain) = match offset_s {
                Some(o) => (o, false, false),
                None => {
                    let times: Vec<i64> = targets.iter().map(|t| t.1).collect();
                    let ref_times: Vec<i64> = other.iter().map(|r| r.time).collect();
                    match estimate_offset(&times, &ref_times, MAX_OFFSET_S, 60, 120) {
                        Some((o, _)) => (o, true, false),
                        None => (0, false, !other.is_empty()),
                    }
                }
            };
            let a = match_by_time(&targets, &same, 0, window_s);
            let b = if uncertain {
                Vec::new()
            } else {
                match_by_time(&targets, &other, offset, window_s)
            };
            let mut best: BTreeMap<i64, TimeMatch> = BTreeMap::new();
            for m in a.into_iter().chain(b) {
                // gleiche Kamera zuerst: bei Gleichstand bleibt sie
                match best.get(&m.id) {
                    Some(old) if old.delta_s.abs() <= m.delta_s.abs() => {}
                    _ => {
                        best.insert(m.id, m);
                    }
                }
            }
            offsets.push(CameraOffset {
                camera,
                offset_s: offset,
                estimated,
                uncertain,
                matched: best.len(),
                photos: targets.len(),
            });
            matches.extend(best.into_values());
        }
        Ok(TimeSuggestions {
            matches,
            offsets,
            without_time,
            candidates,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Scope;

    #[test]
    fn parses_times() {
        assert_eq!(parse_time("1970-01-01T00:00:00"), Some(0));
        assert_eq!(parse_time("2000-03-01T00:00:00"), Some(951_868_800));
        assert_eq!(parse_time("2024-06-01 14:03:22"), Some(1_717_250_602));
        assert_eq!(
            parse_time("2024-06-01T14:03:22.5+02:00"),
            Some(1_717_250_602)
        );
        assert_eq!(
            parse_time("2024-06-01T15:03:22").unwrap() - parse_time("2024-06-01T14:03:22").unwrap(),
            3600
        );
        assert_eq!(parse_time("2024-06-01"), None);
        assert_eq!(parse_time("kaputt"), None);
        assert_eq!(parse_time("2024-13-01T00:00:00"), None);
    }

    #[test]
    fn estimates_offset_with_noise() {
        // Kamera geht eine Stunde nach: Referenz = Ziel + 3600 (+ Rauschen)
        let targets: Vec<i64> = (0..40).map(|i| i * i * 53 + i * 17).collect();
        let refs: Vec<i64> = targets
            .iter()
            .enumerate()
            .map(|(i, t)| t + 3600 + (i as i64 % 5) * 15 - 30)
            .collect();
        let (o, n) = estimate_offset(&targets, &refs, 3 * 3600, 60, 120).unwrap();
        assert_eq!(o, 3600);
        assert!(n >= 38);
    }

    #[test]
    fn estimate_prefers_zero_on_ties() {
        let targets: Vec<i64> = (0..30).map(|i| i * 60).collect();
        let (o, n) = estimate_offset(&targets, &targets, 3600, 60, 120).unwrap();
        assert_eq!((o, n), (0, 30));
    }

    #[test]
    fn estimate_rejects_unrelated_and_weak() {
        let targets: Vec<i64> = (0..30).map(|i| i * 100).collect();
        let refs: Vec<i64> = (0..30).map(|i| 10_000_000 + i * 100).collect();
        assert_eq!(estimate_offset(&targets, &refs, 3 * 3600, 60, 120), None);
        assert_eq!(estimate_offset(&[], &refs, 3600, 60, 120), None);
        // nur 2 Treffer: zu schwach
        assert_eq!(
            estimate_offset(&[0, 5000], &[3600, 8600], 3 * 3600, 60, 120),
            None
        );
    }

    #[test]
    fn estimate_rejects_chance_bursts_and_finds_home_time() {
        // Eine Serie der Kamera (10 Fotos in 90 s) passt zufällig auf zwei
        // Handy-Serien, 2 h und 9 h daneben: nicht entscheidbar
        let burst: Vec<i64> = (0..10).map(|i| i * 10).collect();
        let refs: Vec<i64> = (0..10)
            .flat_map(|i| [7200 + i * 10, 32_400 + i * 10])
            .collect();
        assert_eq!(estimate_offset(&burst, &refs, MAX_OFFSET_S, 60, 120), None);

        // Kamera auf der Zeit daheim (7 h nach), Fotos unregelmäßig über einen
        // Tag verteilt (Abstände 1–30 min, Pseudozufall)
        let mut seed = 12345u64;
        let mut t = 0i64;
        let targets: Vec<i64> = (0..40)
            .map(|_| {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                t += 60 + ((seed >> 33) % 1740) as i64;
                t
            })
            .collect();
        let refs: Vec<i64> = targets.iter().map(|t| t + 7 * 3600 + 30).collect();
        let (o, n) = estimate_offset(&targets, &refs, MAX_OFFSET_S, 60, 120).unwrap();
        assert!((o - 7 * 3600).abs() <= 120, "{o}");
        assert_eq!(n, 40);
    }

    fn rf(id: i64, time: i64) -> Ref {
        Ref {
            id,
            time,
            lat: id as f64,
            lon: 0.0,
            place: None,
        }
    }

    #[test]
    fn matches_nearest_within_window() {
        let refs = [rf(1, 1000), rf(2, 1100), rf(3, 5000)];
        let m = match_by_time(
            &[(10, 1060), (11, 3000), (12, 4900), (13, 1050)],
            &refs,
            0,
            120,
        );
        assert_eq!(m.len(), 3);
        assert_eq!((m[0].id, m[0].ref_id, m[0].delta_s), (10, 2, 40));
        assert_eq!((m[1].id, m[1].ref_id, m[1].delta_s), (12, 3, 100));
        // Gleichstand: die frühere Referenz
        assert_eq!((m[2].id, m[2].ref_id), (13, 1));
        // mit Versatz
        let m = match_by_time(&[(10, 2000)], &refs, -1000, 10);
        assert_eq!((m[0].ref_id, m[0].delta_s), (1, 0));
    }

    #[test]
    #[ignore]
    fn timing_estimate_offset() {
        let targets: Vec<i64> = (0..20_000).map(|i| i * 37 + (i * i) % 29).collect();
        let refs: Vec<i64> = (0..1000).map(|i| i * 700 + 3600).collect();
        let t = std::time::Instant::now();
        let r = estimate_offset(&targets, &refs, 3 * 3600, 60, 120);
        eprintln!("estimate_offset: {:?} in {:?}", r, t.elapsed());
    }

    #[test]
    fn catalog_suggestions() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = Catalog::open(&tmp.path().join("k.db")).unwrap();
        cat.conn
            .execute(
                "INSERT INTO roots (id, path, added_at) VALUES (1, '/f', 0)",
                [],
            )
            .unwrap();
        type Gps<'a> = Option<(f64, f64, &'a str)>;
        let add = |id: i64, at: Option<&str>, cam: &str, gps: Gps, place: Option<&str>| {
            cat.conn
                .execute(
                    "INSERT INTO images (id, root_id, path, dir, file_name, ext, kind, size, mtime,
                                         quick_hash, added_at, geo_state, captured_at, camera,
                                         gps_lat, gps_lon, gps_source, place)
                     VALUES (?1, 1, ?2, '/f', ?2, 'jpg', 'raster', 1, 0, 'h', 0, 1, ?3, ?4,
                             ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        id,
                        format!("/f/{id}.jpg"),
                        at,
                        cam,
                        gps.map(|g| g.0),
                        gps.map(|g| g.1),
                        gps.map(|g| g.2),
                        place
                    ],
                )
                .unwrap();
        };
        // Handy mit GPS: alle 7 Minuten, Uhr stimmt
        for k in 0..8 {
            let at = format!("2024-06-01T14:{:02}:00", k * 7);
            add(
                100 + k,
                Some(&at),
                "Pixel 7",
                Some((48.0 + k as f64 * 0.001, 11.0, "exif")),
                Some("München"),
            );
        }
        // Kamera, Uhr eine Stunde nach: gleiche Minute wie das Handy-Foto, eine Stunde früher
        for k in 0..8 {
            let at = format!("2024-06-01T13:{:02}:00", k * 7);
            add(k + 1, Some(&at), "ILCE-7M4", None, None);
        }
        // Kamera ohne Aufnahmezeit
        add(50, None, "ILCE-7M4", None, None);
        // Weit weg in der Zeit: kein Treffer
        add(60, Some("2024-07-01T10:00:00"), "ILCE-7M4", None, None);
        // Manuell gesetztes Foto derselben Kamera, ohne Versatz nutzbar
        add(
            70,
            Some("2024-08-01T09:00:00"),
            "ILCE-7M4",
            Some((47.0, 8.0, "manual")),
            Some("Luzern"),
        );
        add(71, Some("2024-08-01T09:02:00"), "ILCE-7M4", None, None);

        let s = cat
            .time_suggestions(&ImageQuery::new(Scope::All), None, 600)
            .unwrap();
        assert_eq!(s.without_time, 1);
        assert_eq!(s.candidates, 10);
        let cam = s
            .offsets
            .iter()
            .find(|o| o.camera.as_deref() == Some("ILCE-7M4"))
            .unwrap();
        assert_eq!(cam.offset_s, 3600);
        assert!(cam.estimated);
        assert_eq!(cam.photos, 10);
        assert_eq!(cam.matched, 9);
        for k in 0..8 {
            let m = s.matches.iter().find(|m| m.id == k + 1).unwrap();
            assert_eq!(m.ref_id, 100 + k);
            assert_eq!(m.delta_s, 0);
            assert_eq!(m.place.as_deref(), Some("München"));
        }
        assert!(s.matches.iter().all(|m| m.id != 60));
        let m = s.matches.iter().find(|m| m.id == 71).unwrap();
        assert_eq!((m.ref_id, m.delta_s, m.lat), (70, -120, 47.0));

        // Fester Versatz 0: Kamera-Fotos finden nichts vom Handy
        let s = cat
            .time_suggestions(&ImageQuery::new(Scope::All), Some(0), 600)
            .unwrap();
        assert_eq!(s.matches.len(), 1);
        assert!(!s.offsets[0].estimated);
    }
}
