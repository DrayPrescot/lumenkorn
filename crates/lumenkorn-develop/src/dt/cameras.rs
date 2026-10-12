//! Schwarz- und Weißpunkt der Sensoren aus der Kameradatenbank von rawspeed
//! (darktable, `data/cameras.xml`, LGPL-2.1-or-later) – wie darktables
//! rawprepare. Die Tabelle erzeugt `tools/rawspeed_white.py`.
//!
//! rawler nimmt teils andere Werte (Sony A7 IV: Weiß 15360 aus den EXIF-Daten,
//! die Sensordaten reichen aber bis 16383). Mit zu niedrigem Weißpunkt gelten
//! echte Messwerte als gesättigt, und die Lichter-Rekonstruktion arbeitet auf
//! falschen Daten.

use std::collections::HashMap;
use std::sync::OnceLock;

const TABLE: &str = include_str!("../../data/rawspeed_sensors.tsv");

struct Sensor {
    mode: String,
    iso_min: Option<u32>,
    iso_max: Option<u32>,
    iso_list: Vec<u32>,
    black: Option<u32>,
    white: u32,
}

fn table() -> &'static HashMap<(String, String), Vec<Sensor>> {
    static T: OnceLock<HashMap<(String, String), Vec<Sensor>>> = OnceLock::new();
    T.get_or_init(|| {
        let mut map: HashMap<(String, String), Vec<Sensor>> = HashMap::new();
        for line in TABLE.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = line.split('\t').collect();
            let [make, model, mode, iso_min, iso_max, iso_list, black, white] = f[..] else {
                continue;
            };
            let Ok(white) = white.parse() else {
                continue;
            };
            map.entry((make.to_lowercase(), model.to_lowercase()))
                .or_default()
                .push(Sensor {
                    mode: mode.to_string(),
                    iso_min: iso_min.parse().ok(),
                    iso_max: iso_max.parse().ok(),
                    iso_list: iso_list
                        .split_whitespace()
                        .filter_map(|v| v.parse().ok())
                        .collect(),
                    black: black
                        .parse()
                        .ok()
                        .filter(|b: &i64| *b >= 0)
                        .map(|b| b as u32),
                    white,
                });
        }
        map
    })
}

/// (Schwarz, Weiß) für eine Kamera; Schwarz `None`, wenn rawspeed ihn aus
/// den Daten misst. Wie rawspeed: zuerst ein Eintrag, dessen ISO-Bereich passt,
/// sonst der ohne ISO-Angabe. Der Modus (z. B. „12bit-compressed“) muss
/// passen, falls die Kamera mehrere hat.
pub(crate) fn sensor_levels(
    makes: &[&str],
    models: &[&str],
    mode: &str,
    iso: Option<u32>,
) -> Option<(Option<u32>, u32)> {
    let t = table();
    let list = makes.iter().find_map(|make| {
        models
            .iter()
            .find_map(|model| t.get(&(make.to_lowercase(), model.to_lowercase())))
    })?;
    let modes: Vec<&Sensor> = {
        let exact: Vec<&Sensor> = list.iter().filter(|s| s.mode == mode).collect();
        if !exact.is_empty() {
            exact
        } else {
            let plain: Vec<&Sensor> = list.iter().filter(|s| s.mode.is_empty()).collect();
            if plain.is_empty() {
                list.iter().collect()
            } else {
                plain
            }
        }
    };
    let iso_match = |s: &&&Sensor| match iso {
        Some(i) if !s.iso_list.is_empty() => s.iso_list.contains(&i),
        Some(i) if s.iso_min.is_some() || s.iso_max.is_some() => {
            s.iso_min.is_none_or(|m| i >= m) && s.iso_max.is_none_or(|m| i <= m)
        }
        _ => false,
    };
    let general =
        |s: &&&Sensor| s.iso_list.is_empty() && s.iso_min.is_none() && s.iso_max.is_none();
    let s = modes
        .iter()
        .find(iso_match)
        .or_else(|| modes.iter().find(general))
        .or_else(|| modes.first())?;
    Some((s.black, s.white))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sony_a7_iv_white_is_14_bit_maximum() {
        assert_eq!(
            sensor_levels(&["Sony"], &["ILCE-7M4"], "", Some(100)),
            Some((Some(512), 16383))
        );
        assert_eq!(sensor_levels(&["Nobody"], &["Nothing"], "", None), None);
    }
}
