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
        get(&[
            "city",
            "town",
            "village",
            "municipality",
            "hamlet",
            "suburb",
            "county",
        ]),
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

/// Ortsname zu Koordinaten (`""`: keiner, z. B. auf dem Meer)
fn reverse(lat: f64, lon: f64) -> CmdResult<String> {
    let (la, lo) = (format!("{lat:.6}"), format!("{lon:.6}"));
    let v = nominatim(
        "reverse",
        &[
            ("lat", &la),
            ("lon", &lo),
            ("zoom", "10"),
            ("addressdetails", "1"),
        ],
    )?;
    if v.get("error").is_some() {
        return Ok(String::new());
    }
    Ok(short_name(&v))
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
    // 2. Ortsnamen ermitteln (OpenStreetMap, je Koordinate einmal)
    let mut named = 0usize;
    loop {
        let pending = state.catalog.locked().place_pending(500)?;
        if pending.is_empty() {
            break;
        }
        let mut groups: std::collections::BTreeMap<String, Vec<(i64, f64, f64)>> =
            Default::default();
        for p in pending {
            groups
                .entry(lumenkorn_core::geo::name_key(p.1, p.2))
                .or_default()
                .push(p);
        }
        for (key, items) in groups {
            let cached = state.catalog.locked().cached_place(&key)?;
            let place = match cached {
                Some(p) => p,
                None => {
                    let (lat, lon) = (items[0].1, items[0].2);
                    // Ohne Netz: beim nächsten Anstoß weiter
                    let p = reverse(lat, lon)?;
                    state.catalog.locked().cache_place(&key, &p)?;
                    p
                }
            };
            state.catalog.locked().set_place(&items, &place)?;
            named += items.len();
        }
        let _ = app.emit("geo-changed", ());
    }
    if named > 0 {
        log::info!("Ortsnamen für {named} Fotos ermittelt");
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
