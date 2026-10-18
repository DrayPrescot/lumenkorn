//! Aufnahmeort: Ortssuche, Ortsnamen und Kartenkacheln von OpenStreetMap.
//!
//! Alle Anfragen an OpenStreetMap laufen über das Backend – mit eigener
//! Kennung (User-Agent), Zwischenspeicher und dem Tempo, das die Nutzungs-
//! regeln verlangen:
//! - Nominatim (Ortssuche, Ortsname zu Koordinaten): höchstens eine Anfrage
//!   pro Sekunde, keine Suche während des Tippens.
//!   <https://operations.osmfoundation.org/policies/nominatim/>
//! - Kacheln: höchstens zwei Verbindungen, 7 Tage auf der Platte gehalten.
//!   <https://operations.osmfoundation.org/policies/tiles/>
//!
//! Im Hintergrund trägt ein eigener Thread GPS-Daten aus EXIF nach (Fotos aus
//! der Zeit vor dem Aufnahmeort) und ermittelt Ortsnamen für die Suche – je
//! gerundeter Koordinate einmal, gemerkt im Katalog.

use crate::AppState;
use lumenkorn_core::{Error, LockExt};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Emitter, Manager, State};

type CmdResult<T> = std::result::Result<T, Error>;

const NOMINATIM: &str = "https://nominatim.openstreetmap.org";
const TILES: &str = "https://tile.openstreetmap.org";
/// Abstand zwischen zwei Nominatim-Anfragen (Vorgabe: höchstens eine je Sekunde)
const NOMINATIM_GAP: Duration = Duration::from_millis(1100);
/// So lange gelten Kacheln auf der Platte als aktuell
const TILE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

fn user_agent() -> String {
    format!(
        "Lumenkorn/{} (+https://github.com/DrayPrescot/lumenkorn)",
        env!("CARGO_PKG_VERSION")
    )
}

fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        // Zertifikate des Betriebssystems (Firmen-Proxys, Virenscanner)
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        ureq::Agent::config_builder()
            .tls_config(tls)
            .user_agent(user_agent())
            .timeout_global(Some(Duration::from_secs(20)))
            .build()
            .into()
    })
}

fn lang() -> &'static str {
    if lumenkorn_develop::i18n::is_english() {
        "en"
    } else {
        "de"
    }
}

fn net_err(e: impl std::fmt::Display) -> Error {
    Error::Invalid(lumenkorn_develop::tr!(
        "OpenStreetMap nicht erreichbar: {e}",
        "OpenStreetMap not reachable: {e}"
    ))
}

/// Nominatim-Anfrage im vorgeschriebenen Tempo (prozessweit)
fn nominatim(path: &str, query: &[(&str, &str)]) -> CmdResult<serde_json::Value> {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = *last {
        let wait = NOMINATIM_GAP.saturating_sub(t.elapsed());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }
    let mut req = agent()
        .get(format!("{NOMINATIM}/{path}"))
        .query("format", "jsonv2")
        .query("accept-language", lang());
    for (k, v) in query {
        req = req.query(*k, *v);
    }
    let res = req.call();
    *last = Some(Instant::now());
    drop(last);
    let text = res
        .map_err(net_err)?
        .body_mut()
        .read_to_string()
        .map_err(net_err)?;
    serde_json::from_str(&text).map_err(net_err)
}

/// Kurzer Ortsname aus den Adressteilen einer Nominatim-Antwort:
/// Ort, Region, Land („München, Bayern, Deutschland“).
fn short_name(v: &serde_json::Value) -> String {
    let a = &v["address"];
    let get = |keys: &[&str]| keys.iter().find_map(|k| a[*k].as_str()).map(str::to_string);
    let mut parts: Vec<String> = Vec::new();
    for p in [
        get(LOCALITY),
        get(&["state", "region", "state_district"]),
        get(&["country"]),
    ]
    .into_iter()
    .flatten()
    {
        if !parts.contains(&p) {
            parts.push(p);
        }
    }
    if parts.is_empty() {
        // Ohne Adresse (Meer, Gebirge): Name des Objekts
        v["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| v["display_name"].as_str())
            .unwrap_or("")
            .to_string()
    } else {
        parts.join(", ")
    }
}

/// Adressteile, die einen Ort bezeichnen (zuerst die größeren)
const LOCALITY: &[&str] = &[
    "city",
    "town",
    "village",
    "municipality",
    "hamlet",
    "suburb",
    "county",
];

fn has_locality(v: &serde_json::Value) -> bool {
    LOCALITY.iter().any(|k| v["address"][*k].is_string())
}

/// Treffer der Ortssuche
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceHit {
    /// Kurzer Name für Foto und Suche
    pub name: String,
    /// Ausführliche Bezeichnung (zur Unterscheidung gleichnamiger Orte)
    pub display: String,
    pub lat: f64,
    pub lon: f64,
    /// Ausdehnung [Süd, Nord, West, Ost] zum Einpassen der Karte
    pub bbox: Option<[f64; 4]>,
}

fn parse_hit(v: &serde_json::Value) -> Option<PlaceHit> {
    let num = |x: &serde_json::Value| x.as_str()?.parse::<f64>().ok();
    let bbox = v["boundingbox"].as_array().and_then(|b| {
        let n: Vec<f64> = b.iter().filter_map(num).collect();
        (n.len() == 4).then(|| [n[0], n[1], n[2], n[3]])
    });
    Some(PlaceHit {
        name: short_name(v),
        display: v["display_name"].as_str().unwrap_or_default().to_string(),
        lat: num(&v["lat"])?,
        lon: num(&v["lon"])?,
        bbox,
    })
}

/// Ortssuche („München“, „Eiffelturm“) – erst auf Enter, nicht beim Tippen
#[tauri::command(async)]
pub fn geo_search(query: String) -> CmdResult<Vec<PlaceHit>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let v = nominatim(
        "search",
        &[("q", q), ("addressdetails", "1"), ("limit", "8")],
    )?;
    Ok(v.as_array()
        .map(|a| a.iter().filter_map(parse_hit).collect())
        .unwrap_or_default())
}

/// Umrissfläche eines Ortes (GeoJSON Polygon/MultiPolygon): Liste von
/// Polygonen, je äußerer Ring plus Löcher, Punkte als (Breite, Länge).
#[derive(Debug, Clone, Default)]
struct Area(Vec<Vec<Vec<(f64, f64)>>>);

impl Area {
    fn from_geojson(g: &serde_json::Value) -> Option<Area> {
        let ring = |r: &serde_json::Value| -> Option<Vec<(f64, f64)>> {
            r.as_array()?
                .iter()
                .map(|p| Some((p.get(1)?.as_f64()?, p.get(0)?.as_f64()?)))
                .collect()
        };
        let polygon = |p: &serde_json::Value| -> Option<Vec<Vec<(f64, f64)>>> {
            p.as_array()?.iter().map(ring).collect()
        };
        let coords = g.get("coordinates")?;
        let polys = match g.get("type")?.as_str()? {
            "Polygon" => vec![polygon(coords)?],
            "MultiPolygon" => coords
                .as_array()?
                .iter()
                .map(polygon)
                .collect::<Option<Vec<_>>>()?,
            _ => return None,
        };
        (!polys.is_empty()).then_some(Area(polys))
    }

    /// Liegt der Punkt in der Fläche (in einem äußeren Ring, nicht in einem Loch)?
    fn contains(&self, lat: f64, lon: f64) -> bool {
        fn in_ring(r: &[(f64, f64)], lat: f64, lon: f64) -> bool {
            let mut inside = false;
            let mut j = r.len().wrapping_sub(1);
            for i in 0..r.len() {
                let (ai, oi) = r[i];
                let (aj, oj) = r[j];
                if (ai > lat) != (aj > lat) && lon < (oj - oi) * (lat - ai) / (aj - ai) + oi {
                    inside = !inside;
                }
                j = i;
            }
            inside
        }
        self.0.iter().any(|poly| {
            poly.first().is_some_and(|outer| in_ring(outer, lat, lon))
                && !poly[1..].iter().any(|hole| in_ring(hole, lat, lon))
        })
    }
}

/// Ortsname zu Koordinaten (`""`: keiner, z. B. auf dem Meer) und die
/// Umrissfläche des Ortes – nur auf Gemeindeebene (eine Region wie „Bali“
/// wäre zu grob, um ihren Namen an andere Fotos weiterzugeben).
fn reverse(lat: f64, lon: f64) -> CmdResult<Place> {
    let (la, lo) = (format!("{lat:.6}"), format!("{lon:.6}"));
    let v = nominatim(
        "reverse",
        &[
            ("lat", &la),
            ("lon", &lo),
            ("zoom", "10"),
            ("addressdetails", "1"),
            ("polygon_geojson", "1"),
            // Umriss leicht vereinfacht (~10 m) – genau genug an Grenzen,
            // die Antwort bleibt klein
            ("polygon_threshold", "0.0001"),
        ],
    )?;
    if v.get("error").is_some() {
        return Ok(Place::default());
    }
    // Fläche nur weitergeben, wenn die Antwort einen Ort nennt: Ohne Ort
    // (Nationalpark, Meer) liefert OSM sonst einen ganzen Bezirk, und dessen
    // Fotos hießen alle nur „Bali, Indonesien“.
    let local = has_locality(&v);
    let municipal = v["place_rank"].as_u64().is_some_and(|r| r >= 12);
    let area = (local && municipal)
        .then(|| Area::from_geojson(&v["geojson"]))
        .flatten();
    Ok(Place {
        name: short_name(&v),
        area,
        local,
    })
}

/// Antwort auf eine Ortsabfrage
#[derive(Debug, Clone, Default)]
struct Place {
    name: String,
    /// Umrissfläche (Gemeinde, Dorf), falls bekannt
    area: Option<Area>,
    /// Ein Ort ist bekannt (nicht nur Region und Land)
    local: bool,
}

/// Ohne Umrissfläche bekommen Fotos höchstens so weit (km) vom abgefragten
/// Punkt dessen Namen mit (mit Fläche entscheidet allein die Fläche) …
const NEAR_KM: f64 = 0.3;
/// … und wenn OpenStreetMap dort keinen Ort kennt (nur Region und Land),
/// nur Fotos an praktisch derselben Stelle – daneben kann schon ein Ort liegen.
const SAME_SPOT_KM: f64 = 0.03;

/// Abstand zweier Punkte in km (Großkreis)
fn distance_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, la2) = (a.0.to_radians(), b.0.to_radians());
    let dla = la2 - la1;
    let dlo = (b.1 - a.1).to_radians();
    let h = (dla / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlo / 2.0).sin().powi(2);
    2.0 * 6371.0 * h.sqrt().asin()
}

type Pending = (i64, f64, f64);

/// Ortsnamen für wartende Fotos mit möglichst wenigen Anfragen: Eine Antwort
/// gilt für alle wartenden Fotos innerhalb der Umrissfläche des gefundenen
/// Ortes – 50 Fotos aus München kosten eine Anfrage, auch über die ganze
/// Stadt verteilt, und Fotos jenseits einer Gemeindegrenze bekommen nicht den
/// falschen Namen. Gefragt wird zuerst im ~1-km-Raster (`name_key`) mit den
/// meisten Fotos; gemerkte Namen eines Rasters kosten keine Anfrage.
///
/// `cached`: gemerkter Name eines Rasters; `lookup`: Anfrage an OSM;
/// `apply`: Namen für Fotos speichern und für die Raster `keys` merken.
/// Ergebnis: Zahl der Anfragen.
fn name_places(
    pending: Vec<Pending>,
    mut cached: impl FnMut(&str) -> CmdResult<Option<String>>,
    mut lookup: impl FnMut(f64, f64) -> CmdResult<Place>,
    mut apply: impl FnMut(&[Pending], &str, &[String]) -> CmdResult<()>,
) -> CmdResult<usize> {
    let mut groups: std::collections::BTreeMap<String, Vec<Pending>> = Default::default();
    for p in pending {
        groups
            .entry(lumenkorn_core::geo::name_key(p.1, p.2))
            .or_default()
            .push(p);
    }
    // Schon bekannte Raster ohne Anfrage
    let mut open: Vec<(String, Vec<Pending>)> = Vec::new();
    for (key, items) in groups {
        match cached(&key)? {
            Some(name) => apply(&items, &name, &[])?,
            None => open.push((key, items)),
        }
    }
    let mut requests = 0;
    while !open.is_empty() {
        // Im Raster mit den meisten Fotos fragen: ein Foto nahe dessen Mitte
        let first = (0..open.len())
            .max_by_key(|&i| (open[i].1.len(), std::cmp::Reverse(i)))
            .expect("nicht leer");
        let items = &open[first].1;
        let n = items.len() as f64;
        let center = (
            items.iter().map(|p| p.1).sum::<f64>() / n,
            items.iter().map(|p| p.2).sum::<f64>() / n,
        );
        let at = items
            .iter()
            .map(|p| (p.1, p.2))
            .min_by(|a, b| distance_km(*a, center).total_cmp(&distance_km(*b, center)))
            .expect("nicht leer");
        let Place { name, area, local } = lookup(at.0, at.1)?;
        requests += 1;
        let near = if local && area.is_none() {
            NEAR_KM
        } else {
            SAME_SPOT_KM
        };
        let same = |p: &Pending| {
            distance_km(at, (p.1, p.2)) <= near
                || area.as_ref().is_some_and(|a| a.contains(p.1, p.2))
        };
        let mut covered = Vec::new();
        let mut keys = Vec::new();
        let mut rest = Vec::with_capacity(open.len());
        for (k, group) in open {
            let (hit, miss): (Vec<Pending>, Vec<Pending>) = group.into_iter().partition(same);
            if miss.is_empty() {
                // Ganzes Raster im Ort: Namen dafür merken
                keys.push(k);
            } else {
                rest.push((k, miss));
            }
            covered.extend(hit);
        }
        open = rest;
        // Der abgefragte Punkt selbst ist immer dabei – es geht voran
        apply(&covered, &name, &keys)?;
    }
    Ok(requests)
}

/// Ort für Fotos setzen. Ohne `place` (Klick oder Ziehen auf die Karte)
/// ermittelt der Hintergrund den Ortsnamen.
#[tauri::command(async)]
pub fn geo_set_location(
    app: AppHandle,
    state: State<AppState>,
    ids: Vec<i64>,
    lat: f64,
    lon: f64,
    place: Option<String>,
) -> CmdResult<()> {
    let place = place.filter(|p| !p.trim().is_empty());
    state
        .catalog
        .locked()
        .set_location(&ids, Some((lat, lon)), place.as_deref())?;
    if place.is_none() {
        kick(&app);
    }
    Ok(())
}

#[tauri::command(async)]
pub fn geo_clear_location(state: State<AppState>, ids: Vec<i64>) -> CmdResult<()> {
    state.catalog.locked().set_location(&ids, None, None)
}

// --- Hintergrund: GPS nachtragen, Ortsnamen ermitteln -------------------------

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Startet den Hintergrund-Thread, falls er nicht schon läuft.
pub fn kick(app: &AppHandle) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("lumenkorn-geo".into())
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&app)));
            if !matches!(r, Ok(Ok(()))) {
                log::warn!("Aufnahmeort im Hintergrund: {r:?}");
            }
            RUNNING.store(false, Ordering::SeqCst);
        });
    if spawned.is_err() {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

fn run(app: &AppHandle) -> CmdResult<()> {
    let Some(state) = app.try_state::<AppState>() else {
        return Ok(());
    };
    // 1. GPS aus EXIF nachtragen (lokal, ohne Netz)
    let mut found = 0usize;
    loop {
        let batch = state.catalog.locked().geo_pending(200)?;
        if batch.is_empty() {
            break;
        }
        // Dateien ohne Sperre lesen (NAS, große RAWs)
        let items: Vec<(i64, Option<(f64, f64)>)> = batch
            .into_iter()
            .map(|(id, path)| {
                (
                    id,
                    lumenkorn_core::metadata::read_gps(std::path::Path::new(&path)),
                )
            })
            .collect();
        found += items.iter().filter(|(_, g)| g.is_some()).count();
        state.catalog.locked().set_exif_gps(&items)?;
    }
    if found > 0 {
        log::info!("GPS-Daten von {found} Fotos übernommen");
        let _ = app.emit("geo-changed", ());
    }
    // 2. Ortsnamen ermitteln (OpenStreetMap, gebündelt – siehe `name_places`).
    // Große Stapel, damit nahe Fotos möglichst in dieselbe Anfrage fallen.
    let (mut named, mut requests) = (0usize, 0usize);
    loop {
        let pending = state.catalog.locked().place_pending(20_000)?;
        if pending.is_empty() {
            break;
        }
        // Ohne Netz bricht `lookup` ab: beim nächsten Anstoß weiter
        requests += name_places(
            pending,
            |key| state.catalog.locked().cached_place(key),
            reverse,
            |items, name, keys| {
                let cat = state.catalog.locked();
                for k in keys {
                    cat.cache_place(k, name)?;
                }
                drop(cat);
                state.catalog.locked().set_place(items, name)?;
                named += items.len();
                let _ = app.emit("geo-changed", ());
                Ok(())
            },
        )?;
    }
    if named > 0 {
        log::info!("Ortsnamen für {named} Fotos ermittelt, {requests} Anfragen an OpenStreetMap");
    }
    Ok(())
}

// --- Kartenkacheln ------------------------------------------------------------

/// Kachel `z/x/y` aus dem Zwischenspeicher bzw. von OpenStreetMap (PNG).
/// Veraltete Kacheln werden ohne Netz weiter geliefert.
pub fn tile(state: &AppState, z: u32, x: u32, y: u32) -> Option<Vec<u8>> {
    if z > 19 || x >= (1 << z) || y >= (1 << z) {
        return None;
    }
    let path = state
        .cache
        .base()
        .join("tiles")
        .join(z.to_string())
        .join(x.to_string())
        .join(format!("{y}.png"));
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < TILE_MAX_AGE);
    if fresh && let Ok(b) = std::fs::read(&path) {
        return Some(b);
    }
    match fetch_tile(z, x, y) {
        Ok(bytes) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::write(&path, &bytes);
            }
            Some(bytes)
        }
        Err(e) => {
            log::debug!("Kachel {z}/{x}/{y}: {e}");
            std::fs::read(&path).ok()
        }
    }
}

fn fetch_tile(z: u32, x: u32, y: u32) -> std::result::Result<Vec<u8>, String> {
    let mut res = agent()
        .get(format!("{TILES}/{z}/{x}/{y}.png"))
        .call()
        .map_err(|e| e.to_string())?;
    res.body_mut()
        .with_config()
        .limit(2 << 20)
        .read_to_vec()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearby_photos_share_one_request() {
        // 50 Fotos über ganz München verteilt (viele ~1-km-Raster), dazu ein
        // Foto im Umland (außerhalb des Umrisses) und eines in Bern
        let mut pending: Vec<Pending> = (0..50)
            .map(|i| {
                (
                    i,
                    48.10 + (i % 5) as f64 * 0.02,
                    11.45 + (i / 5) as f64 * 0.02,
                )
            })
            .collect();
        pending.push((100, 48.30, 11.70));
        pending.push((200, 46.948, 7.447));
        let mut asked = Vec::new();
        let mut names = std::collections::HashMap::new();
        let mut remembered = Vec::new();
        // München als Rechteck-Umriss, das Umland liegt außerhalb
        let munich = Area(vec![vec![vec![
            (48.06, 11.36),
            (48.06, 11.72),
            (48.25, 11.72),
            (48.25, 11.36),
            (48.06, 11.36),
        ]]]);
        let requests = name_places(
            pending,
            |_| Ok(None),
            |lat, lon| {
                asked.push((lat, lon));
                let place = |name: &str, area: Option<Area>| Place {
                    name: name.into(),
                    area,
                    local: true,
                };
                Ok(if lat > 48.27 {
                    place("Unterföhring, Bayern, Deutschland", None)
                } else if lat > 47.5 {
                    place("München, Bayern, Deutschland", Some(munich.clone()))
                } else {
                    place("Bern, Schweiz", None)
                })
            },
            |items, name, keys| {
                for it in items {
                    names.insert(it.0, name.to_string());
                }
                remembered.extend(keys.iter().cloned());
                Ok(())
            },
        )
        .unwrap();
        // München einmal, Umland einmal, Bern einmal
        assert_eq!(requests, 3, "{asked:?}");
        assert_eq!(names.len(), 52);
        assert_eq!(names[&100], "Unterföhring, Bayern, Deutschland");
        assert!((0..50).all(|i| names[&i] == "München, Bayern, Deutschland"));
        assert_eq!(names[&200], "Bern, Schweiz");
        // Die Raster der 50 Fotos sind gemerkt
        assert!(remembered.len() >= 25, "{remembered:?}");
    }

    #[test]
    fn photos_across_a_border_get_their_own_name() {
        // Grenze bei Länge 115.275: links Payangan, rechts Tegallalang. Die
        // Fotos liegen nur ~500 m auseinander, gehören aber zu zwei Orten.
        let tegallalang = Area(vec![vec![vec![
            (-8.50, 115.275),
            (-8.50, 115.35),
            (-8.35, 115.35),
            (-8.35, 115.275),
            (-8.50, 115.275),
        ]]]);
        let mut pending: Vec<Pending> = (0..20).map(|i| (i, -8.430, 115.2795)).collect();
        pending.extend((20..25).map(|i| (i, -8.430, 115.2745)));
        let mut names = std::collections::HashMap::new();
        let requests = name_places(
            pending,
            |_| Ok(None),
            |_, lon| {
                Ok(if lon >= 115.275 {
                    Place {
                        name: "Tegallalang".into(),
                        area: Some(tegallalang.clone()),
                        local: true,
                    }
                } else {
                    Place {
                        name: "Payangan".into(),
                        area: None,
                        local: true,
                    }
                })
            },
            |items, name, _| {
                for it in items {
                    names.insert(it.0, name.to_string());
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(requests, 2);
        assert!((0..20).all(|i| names[&i] == "Tegallalang"));
        assert!((20..25).all(|i| names[&i] == "Payangan"));
    }

    #[test]
    fn region_only_answer_is_not_passed_on() {
        // Am ersten Punkt kennt OSM nur die Region; 100 m daneben liegt ein Ort
        let pending: Vec<Pending> = vec![
            (1, -8.2438, 115.1480),
            (2, -8.2438, 115.1480),
            (3, -8.2447, 115.1480),
        ];
        let mut names = std::collections::HashMap::new();
        let requests = name_places(
            pending,
            |_| Ok(None),
            |lat, _| {
                Ok(if lat > -8.244 {
                    Place {
                        name: "Bali, Indonesien".into(),
                        area: None,
                        local: false,
                    }
                } else {
                    Place {
                        name: "Sukasada, Bali, Indonesien".into(),
                        area: None,
                        local: true,
                    }
                })
            },
            |items, name, _| {
                for it in items {
                    names.insert(it.0, name.to_string());
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(requests, 2);
        assert_eq!(names[&3], "Sukasada, Bali, Indonesien");
    }

    #[test]
    fn area_handles_holes_and_multipolygons() {
        let g = serde_json::json!({
            "type": "MultiPolygon",
            "coordinates": [
                [[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0], [0.0, 0.0]],
                 [[4.0, 4.0], [6.0, 4.0], [6.0, 6.0], [4.0, 6.0], [4.0, 4.0]]],
                [[[20.0, 20.0], [21.0, 20.0], [21.0, 21.0], [20.0, 21.0], [20.0, 20.0]]]
            ]
        });
        let a = Area::from_geojson(&g).unwrap();
        // GeoJSON: [Länge, Breite]
        assert!(a.contains(1.0, 1.0));
        assert!(!a.contains(5.0, 5.0), "Loch");
        assert!(a.contains(20.5, 20.5));
        assert!(!a.contains(15.0, 15.0));
    }

    #[test]
    fn cached_cells_need_no_request() {
        let pending = vec![(1, 48.137, 11.575), (2, 48.1371, 11.5751)];
        let n = name_places(
            pending,
            |_| Ok(Some("München".into())),
            |_, _| panic!("keine Anfrage erwartet"),
            |_, _, _| Ok(()),
        )
        .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn short_name_uses_place_region_country() {
        let v = serde_json::json!({
            "name": "München",
            "display_name": "München, Bayern, Deutschland",
            "address": { "city": "München", "state": "Bayern", "country": "Deutschland" },
            "lat": "48.1371", "lon": "11.5754",
            "boundingbox": ["48.06", "48.25", "11.36", "11.72"]
        });
        assert_eq!(short_name(&v), "München, Bayern, Deutschland");
        let h = parse_hit(&v).unwrap();
        assert!((h.lat - 48.1371).abs() < 1e-9 && h.bbox.is_some());
        // Stadtstaat: Name nicht doppelt
        let berlin = serde_json::json!({
            "address": { "city": "Berlin", "state": "Berlin", "country": "Deutschland" }
        });
        assert_eq!(short_name(&berlin), "Berlin, Deutschland");
        // Ohne Adresse
        let sea = serde_json::json!({ "name": "Nordsee" });
        assert_eq!(short_name(&sea), "Nordsee");
    }
}
