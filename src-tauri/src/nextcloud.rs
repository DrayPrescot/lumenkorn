//! Nextcloud: die Exporte eines Projekts in einen Ordner hochladen und als
//! Link teilen.
//!
//! - **Anmeldung:** Login Flow v2. Der Browser zeigt die Anmeldeseite der
//!   Nextcloud, Lumenkorn bekommt ein eigenes App-Passwort (in Nextcloud
//!   jederzeit widerrufbar). Es liegt im Anmeldespeicher des Systems (unter
//!   Windows die Anmeldeinformationsverwaltung), nie im Datenordner.
//! - **Hochladen:** WebDAV. Ein erneutes Hochladen gleicht ab: Neue und
//!   geänderte Dateien (Größe oder Änderungszeit) werden übertragen, der Rest
//!   bleibt; auf Wunsch werden Dateien entfernt, die es nicht mehr gibt.
//! - **Teilen:** OCS-Share-API, ein öffentlicher Link auf den Ordner, mit
//!   Passwort und Ablaufdatum nach Wahl. Derselbe Link bleibt beim Abgleich.
//!
//! Verbindungen gehen nur an den Server, den der Nutzer selbst einträgt.

use crate::AppState;
use lumenkorn_core::{Error, LockExt};
use lumenkorn_develop::{i18n::tr_s, tr};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

const FILE: &str = "nextcloud.json";
const KEYRING_SERVICE: &str = "Lumenkorn Nextcloud";
const DEFAULT_BASE: &str = "Lumenkorn";

/// Verbindung und je Projekt Zielordner und Link (`nextcloud.json`)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Config {
    server: String,
    login_name: String,
    /// Benutzer-ID für die WebDAV-Pfade (kann vom Anmeldenamen abweichen)
    user_id: String,
    /// Ordner, unter dem die Projektordner angelegt werden
    base_folder: String,
    /// Projekt-ID → zuletzt benutzter Ordner und Link
    projects: HashMap<String, ProjectLink>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ProjectLink {
    folder: String,
    share_url: Option<String>,
}

impl Config {
    fn base(&self) -> &str {
        if self.base_folder.is_empty() {
            DEFAULT_BASE
        } else {
            &self.base_folder
        }
    }
}

fn load(data_dir: &Path) -> Config {
    std::fs::read(data_dir.join(FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(data_dir: &Path, c: &Config) -> Result<(), Error> {
    let tmp = data_dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_string_pretty(c)?)?;
    std::fs::rename(tmp, data_dir.join(FILE))?;
    Ok(())
}

fn keyring_entry(c: &Config) -> Result<keyring::Entry, Error> {
    keyring::Entry::new(KEYRING_SERVICE, &format!("{}@{}", c.login_name, c.server))
        .map_err(|e| Error::Invalid(e.to_string()))
}

fn app_password(c: &Config) -> Result<String, Error> {
    if c.server.is_empty() {
        return Err(not_connected());
    }
    keyring_entry(c)?
        .get_password()
        .map_err(|_| not_connected())
}

fn not_connected() -> Error {
    Error::Invalid(
        tr_s(
            "Nicht mit Nextcloud verbunden (Einstellungen → Nextcloud).",
            "Not connected to Nextcloud (Settings → Nextcloud).",
        )
        .into(),
    )
}

// ----------------------------------------------------------------------
// Hilfen: Adressen, Pfade, Namen
// ----------------------------------------------------------------------

/// Serveradresse bereinigen: ohne Schema https, ohne `/index.php` und
/// abschließenden Schrägstrich. Unverschlüsseltes http nur im eigenen Netz.
fn normalize_server(s: &str) -> Result<String, Error> {
    let input = s.trim();
    let bad = || {
        Error::Invalid(tr!(
            "„{input}“ ist keine gültige Nextcloud-Adresse",
            "“{input}” is not a valid Nextcloud address"
        ))
    };
    let (scheme, rest) = match input.split_once("://") {
        Some((a, b)) => (a.to_ascii_lowercase(), b),
        None => ("https".to_string(), input),
    };
    let mut rest = rest.trim_end_matches('/');
    for suffix in ["/index.php", "/login"] {
        rest = rest.strip_suffix(suffix).unwrap_or(rest);
    }
    let s = format!("{scheme}://{rest}");
    let host = rest
        .split(['/', ':'])
        .next()
        .filter(|h| !h.is_empty())
        .ok_or_else(bad)?
        .to_ascii_lowercase();
    if rest.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(bad());
    }
    match scheme.as_str() {
        "https" => Ok(s),
        "http" if is_local(&host) => Ok(s),
        "http" => Err(Error::Invalid(
            tr_s(
                "Unverschlüsselt (http) nur im eigenen Netz – bitte https verwenden.",
                "Unencrypted (http) only on your local network – please use https.",
            )
            .into(),
        )),
        _ => Err(bad()),
    }
}

fn is_local(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".local") || host.ends_with(".lan") {
        return true;
    }
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => ip.is_private() || ip.is_loopback(),
        Err(_) => false,
    }
}

/// Ein Pfadabschnitt für URLs: alles außer A–Z, a–z, 0–9 und `-._~` kodiert
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn dec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Ordnerpfad prüfen und in Abschnitte zerlegen (`a/b/c`, keine `..`)
fn folder_parts(folder: &str) -> Result<Vec<String>, Error> {
    let parts: Vec<String> = folder
        .replace('\\', "/")
        .split('/')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect();
    if parts.is_empty()
        || parts
            .iter()
            .any(|p| p == "." || p == ".." || p.chars().any(char::is_control))
    {
        return Err(Error::Invalid(tr!(
            "„{folder}“ ist kein gültiger Ordner",
            "“{folder}” is not a valid folder"
        )));
    }
    Ok(parts)
}

/// Name eines Projekts als Ordnername (ohne Schrägstriche)
fn folder_name(name: &str) -> String {
    let n: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\') || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let n = n.trim().trim_matches('.').to_string();
    if n.is_empty() { "Projekt".into() } else { n }
}

/// Eindeutige Zielnamen (gleiche Namen aus verschiedenen Ordnern → `-2`)
fn unique_names(files: &[PathBuf]) -> Vec<String> {
    let mut used = HashSet::new();
    files
        .iter()
        .map(|p| {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let stem = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let ext = p
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            let mut cand = name;
            let mut i = 2;
            while !used.insert(cand.to_lowercase()) {
                cand = format!("{stem}-{i}{ext}");
                i += 1;
            }
            cand
        })
        .collect()
}

fn mtime_secs(p: &Path) -> Option<i64> {
    let t = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64)
}

// ----------------------------------------------------------------------
// HTTP
// ----------------------------------------------------------------------

fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        ureq::Agent::config_builder()
            .tls_config(tls)
            .user_agent(format!("Lumenkorn/{}", env!("CARGO_PKG_VERSION")))
            .timeout_connect(Some(Duration::from_secs(10)))
            // Große Dateien dürfen dauern; hängt die Verbindung, greift dies
            .timeout_recv_response(Some(Duration::from_secs(120)))
            .http_status_as_error(false)
            .allow_non_standard_methods(true)
            .build()
            .into()
    })
}

/// Verbindung mit Anmeldung
struct Conn {
    server: String,
    user: String,
    auth: String,
}

impl Conn {
    fn new(c: &Config) -> Result<Self, Error> {
        use base64::Engine;
        let pw = app_password(c)?;
        let token =
            base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", c.login_name, pw));
        Ok(Self {
            server: c.server.clone(),
            user: if c.user_id.is_empty() {
                c.login_name.clone()
            } else {
                c.user_id.clone()
            },
            auth: format!("Basic {token}"),
        })
    }

    fn dav(&self, parts: &[String]) -> String {
        let mut u = format!("{}/remote.php/dav/files/{}", self.server, enc(&self.user));
        for p in parts {
            u.push('/');
            u.push_str(&enc(p));
        }
        u
    }

    fn request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: impl ureq::AsSendBody,
    ) -> Result<(u16, String), Error> {
        let mut b = ureq::http::Request::builder()
            .method(method)
            .uri(url)
            .header("Authorization", &self.auth)
            .header("OCS-APIRequest", "true");
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let req = b.body(body).map_err(|e| Error::Invalid(e.to_string()))?;
        let mut resp = agent().run(req).map_err(net_err)?;
        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .with_config()
            .limit(16 << 20)
            .read_to_string()
            .unwrap_or_default();
        if status == 401 {
            return Err(Error::Invalid(
                tr_s(
                    "Nextcloud lehnt die Anmeldung ab – bitte unter Einstellungen → Nextcloud neu verbinden.",
                    "Nextcloud rejected the login – please reconnect under Settings → Nextcloud.",
                )
                .into(),
            ));
        }
        Ok((status, text))
    }

    /// OCS-Aufruf (JSON); Fehler mit der Meldung des Servers
    fn ocs(
        &self,
        method: &str,
        path: &str,
        form: &[(&str, &str)],
    ) -> Result<serde_json::Value, Error> {
        let sep = if path.contains('?') { '&' } else { '?' };
        let url = format!("{}/ocs/v2.php/{path}{sep}format=json", self.server);
        let body = form
            .iter()
            .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
            .collect::<Vec<_>>()
            .join("&");
        let (status, text) = self.request(
            method,
            &url,
            &[("Content-Type", "application/x-www-form-urlencoded")],
            body,
        )?;
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let meta = &v["ocs"]["meta"];
        let ok = (200..300).contains(&status)
            && meta["statuscode"]
                .as_i64()
                .is_none_or(|c| (100..300).contains(&c));
        if !ok {
            let msg = meta["message"].as_str().filter(|m| !m.is_empty());
            return Err(Error::Invalid(match msg {
                Some(m) => format!("Nextcloud: {m}"),
                None => format!("Nextcloud: HTTP {status}"),
            }));
        }
        Ok(v["ocs"]["data"].clone())
    }

    /// Ordner samt übergeordneten anlegen (vorhandene sind kein Fehler)
    fn mkdirs(&self, parts: &[String]) -> Result<(), Error> {
        for i in 1..=parts.len() {
            let (status, _) = self.request("MKCOL", &self.dav(&parts[..i]), &[], "")?;
            if !(matches!(status, 201 | 405) || (200..300).contains(&status)) {
                return Err(Error::Invalid(tr!(
                    "Ordner „{}“ ließ sich nicht anlegen (HTTP {status})",
                    "Could not create folder “{}” (HTTP {status})",
                    parts[..i].join("/")
                )));
            }
        }
        Ok(())
    }

    /// Dateien im Ordner: Name → (Größe, Änderungszeit in Sekunden)
    fn list(&self, parts: &[String]) -> Result<HashMap<String, (u64, i64)>, Error> {
        const BODY: &str = r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:getcontentlength/><d:getlastmodified/><d:resourcetype/></d:prop></d:propfind>"#;
        let (status, text) = self.request(
            "PROPFIND",
            &(self.dav(parts) + "/"),
            &[("Depth", "1"), ("Content-Type", "application/xml")],
            BODY,
        )?;
        if status != 207 {
            return Err(Error::Invalid(format!("Nextcloud: PROPFIND HTTP {status}")));
        }
        Ok(parse_propfind(&text))
    }

    fn upload(&self, parts: &[String], name: &str, file: &Path) -> Result<(), Error> {
        let mut path = parts.to_vec();
        path.push(name.to_string());
        let f = std::fs::File::open(file)?;
        let mtime = mtime_secs(file).unwrap_or(0).to_string();
        let (status, _) = self.request("PUT", &self.dav(&path), &[("X-OC-Mtime", &mtime)], f)?;
        if (200..300).contains(&status) {
            Ok(())
        } else if status == 507 {
            Err(Error::Invalid(
                tr_s(
                    "Kein Speicherplatz mehr in Nextcloud",
                    "No storage space left in Nextcloud",
                )
                .into(),
            ))
        } else {
            Err(Error::Invalid(format!("HTTP {status}")))
        }
    }

    fn delete(&self, parts: &[String], name: &str) -> Result<(), Error> {
        let mut path = parts.to_vec();
        path.push(name.to_string());
        let (status, _) = self.request("DELETE", &self.dav(&path), &[], "")?;
        if (200..300).contains(&status) || status == 404 {
            Ok(())
        } else {
            Err(Error::Invalid(format!("HTTP {status}")))
        }
    }

    /// Öffentlichen Link auf den Ordner anlegen oder anpassen; `None`:
    /// vorhandenen Link entfernen. Liefert die Adresse des Links.
    fn share(
        &self,
        parts: &[String],
        want: Option<&ShareOptions>,
    ) -> Result<Option<String>, Error> {
        let path = format!("/{}", parts.join("/"));
        let api = "apps/files_sharing/api/v1/shares";
        let list = self.ocs(
            "GET",
            &format!("{api}?path={}&reshares=false", enc(&path)),
            &[],
        )?;
        let existing = public_share(&list);
        let Some(o) = want else {
            if let Some((id, _)) = existing {
                self.ocs("DELETE", &format!("{api}/{id}"), &[])?;
            }
            return Ok(None);
        };
        let pw = o.password.as_deref().unwrap_or("");
        let exp = o.expire.as_deref().unwrap_or("");
        match existing {
            Some((id, url)) => {
                // Einzeln setzen: Nextcloud nimmt je Aufruf eine Änderung
                self.ocs("PUT", &format!("{api}/{id}"), &[("password", pw)])?;
                self.ocs("PUT", &format!("{api}/{id}"), &[("expireDate", exp)])?;
                Ok(Some(url))
            }
            None => {
                let mut form = vec![
                    ("path", path.as_str()),
                    ("shareType", "3"),
                    ("permissions", "1"),
                ];
                if !pw.is_empty() {
                    form.push(("password", pw));
                }
                if !exp.is_empty() {
                    form.push(("expireDate", exp));
                }
                let d = self.ocs("POST", api, &form)?;
                Ok(d["url"].as_str().map(String::from))
            }
        }
    }
}

fn net_err(e: ureq::Error) -> Error {
    Error::Invalid(tr!(
        "Nextcloud nicht erreichbar: {e}",
        "Cannot reach Nextcloud: {e}"
    ))
}

/// PROPFIND-Antwort: Dateien (keine Ordner) mit Größe und Änderungszeit
fn parse_propfind(xml: &str) -> HashMap<String, (u64, i64)> {
    use quick_xml::events::Event;
    let mut out = HashMap::new();
    let mut r = quick_xml::Reader::from_str(xml);
    let (mut href, mut len, mut modified, mut dir) = (String::new(), 0u64, 0i64, false);
    let mut field = String::new();
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => {
                let n = e.local_name().as_ref().to_string();
                if n == "response" {
                    (href, len, modified, dir) = (String::new(), 0, 0, false);
                } else if n == "collection" {
                    dir = true;
                }
                field = n;
            }
            Ok(Event::Empty(e)) if e.local_name().as_ref() == "collection" => dir = true,
            Ok(Event::Text(t)) => {
                let v = t.xml_content(quick_xml::XmlVersion::Implicit1_0);
                match field.as_str() {
                    "href" => href.push_str(v.trim()),
                    "getcontentlength" => len = v.trim().parse().unwrap_or(0),
                    "getlastmodified" => modified = http_date(v.trim()).unwrap_or(0),
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                if e.local_name().as_ref() == "response" && !dir && !href.ends_with('/') {
                    let name = dec(href.rsplit('/').next().unwrap_or(""));
                    if !name.is_empty() {
                        out.insert(name, (len, modified));
                    }
                }
                field.clear();
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// „Wed, 28 Oct 2026 08:15:00 GMT“ → Sekunden seit 1970
fn http_date(s: &str) -> Option<i64> {
    let p: Vec<&str> = s.split_whitespace().collect();
    if p.len() < 5 {
        return None;
    }
    let day: i64 = p[1].parse().ok()?;
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|m| *m == p[2])? as i64
        + 1;
    let year: i64 = p[3].parse().ok()?;
    let t: Vec<i64> = p[4].split(':').filter_map(|v| v.parse().ok()).collect();
    if t.len() != 3 {
        return None;
    }
    // Tage seit 1970 (Howard Hinnant, days_from_civil)
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + t[0] * 3600 + t[1] * 60 + t[2])
}

/// Öffentlicher Link (share_type 3) in einer Share-Liste: (ID, Adresse)
fn public_share(list: &serde_json::Value) -> Option<(String, String)> {
    list.as_array()?.iter().find_map(|s| {
        (s["share_type"].as_i64() == Some(3)).then(|| {
            let id = match &s["id"] {
                serde_json::Value::String(v) => v.clone(),
                v => v.to_string(),
            };
            (id, s["url"].as_str().unwrap_or_default().to_string())
        })
    })
}

// ----------------------------------------------------------------------
// Befehle
// ----------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NcStatus {
    server: String,
    login_name: String,
    base_folder: String,
}

fn status_of(c: &Config) -> Option<NcStatus> {
    (!c.server.is_empty() && app_password(c).is_ok()).then(|| NcStatus {
        server: c.server.clone(),
        login_name: c.login_name.clone(),
        base_folder: c.base().to_string(),
    })
}

#[tauri::command]
pub fn nc_status(state: State<AppState>) -> Option<NcStatus> {
    status_of(&load(&state.data_dir))
}

/// Zähler der Anmeldeversuche: ein neuer (oder Abbrechen) beendet den alten
static LOGIN: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginEvent {
    ok: bool,
    error: Option<String>,
}

/// Anmeldung beginnen: öffnet die Anmeldeseite im Browser und wartet im
/// Hintergrund auf die Freigabe (Ereignis `nextcloud-login`).
#[tauri::command]
pub async fn nc_login_start(app: AppHandle, server: String) -> Result<(), Error> {
    let server = normalize_server(&server)?;
    let attempt = LOGIN.fetch_add(1, Ordering::SeqCst) + 1;
    tauri::async_runtime::spawn_blocking(move || -> Result<(), Error> {
        let url = format!("{server}/index.php/login/v2");
        let mut resp = agent()
            .post(&url)
            .header("Content-Length", "0")
            .send_empty()
            .map_err(net_err)?;
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|_| {
            Error::Invalid(tr!(
                "Unter {server} antwortet keine Nextcloud (HTTP {})",
                "No Nextcloud answers at {server} (HTTP {})",
                resp.status().as_u16()
            ))
        })?;
        let (Some(login), Some(token), Some(endpoint)) = (
            v["login"].as_str(),
            v["poll"]["token"].as_str(),
            v["poll"]["endpoint"].as_str(),
        ) else {
            return Err(Error::Invalid(tr!(
                "Unter {server} antwortet keine Nextcloud",
                "No Nextcloud answers at {server}"
            )));
        };
        open_browser(login)?;
        let (token, endpoint) = (token.to_string(), endpoint.to_string());
        std::thread::spawn(move || {
            let result = poll_login(&app, attempt, &endpoint, &token);
            if LOGIN.load(Ordering::SeqCst) != attempt {
                return; // abgebrochen oder ersetzt
            }
            let ev = match result {
                Ok(()) => LoginEvent {
                    ok: true,
                    error: None,
                },
                Err(e) => LoginEvent {
                    ok: false,
                    error: Some(e.to_string()),
                },
            };
            let _ = app.emit("nextcloud-login", ev);
        });
        Ok(())
    })
    .await
    .map_err(|e| Error::Invalid(e.to_string()))?
}

/// Bis zu 20 Minuten alle 2 s fragen, ob die Anmeldung freigegeben ist
fn poll_login(app: &AppHandle, attempt: u64, endpoint: &str, token: &str) -> Result<(), Error> {
    for _ in 0..600 {
        std::thread::sleep(Duration::from_secs(2));
        if LOGIN.load(Ordering::SeqCst) != attempt {
            return Ok(());
        }
        let Ok(mut resp) = agent()
            .post(endpoint)
            .content_type("application/x-www-form-urlencoded")
            .send(format!("token={}", enc(token)))
        else {
            continue; // Netz kurz weg: weiter versuchen
        };
        if resp.status().as_u16() != 200 {
            continue;
        }
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let (Some(server), Some(login), Some(pw)) = (
            v["server"].as_str(),
            v["loginName"].as_str(),
            v["appPassword"].as_str(),
        ) else {
            continue;
        };
        let state = app.state::<AppState>();
        let mut c = load(&state.data_dir);
        // Alten Zugang (anderer Server oder Nutzer) aus dem Speicher nehmen
        if !c.server.is_empty()
            && let Ok(e) = keyring_entry(&c)
        {
            let _ = e.delete_credential();
        }
        c.server = normalize_server(server)?;
        c.login_name = login.to_string();
        keyring_entry(&c)?
            .set_password(pw)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        // Benutzer-ID für WebDAV (Anmeldung per E-Mail-Adresse u. Ä.)
        let conn = Conn::new(&c)?;
        if let Ok(d) = conn.ocs("GET", "cloud/user", &[]) {
            c.user_id = d["id"].as_str().unwrap_or(login).to_string();
        }
        save(&state.data_dir, &c)?;
        log::info!("Nextcloud verbunden: {} als {}", c.server, c.login_name);
        return Ok(());
    }
    Err(Error::Invalid(
        tr_s(
            "Die Anmeldung wurde nicht innerhalb von 20 Minuten bestätigt.",
            "The login was not confirmed within 20 minutes.",
        )
        .into(),
    ))
}

fn open_browser(url: &str) -> Result<(), Error> {
    let ok = (url.starts_with("https://") || url.starts_with("http://"))
        && url.len() < 2048
        && !url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"');
    if !ok {
        return Err(Error::Invalid(url.into()));
    }
    #[cfg(windows)]
    std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn()?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(url).spawn()?;
    #[cfg(not(any(windows, target_os = "macos")))]
    std::process::Command::new("xdg-open").arg(url).spawn()?;
    Ok(())
}

#[tauri::command]
pub fn nc_login_cancel() {
    LOGIN.fetch_add(1, Ordering::SeqCst);
}

/// Trennen: App-Passwort in Nextcloud widerrufen (wenn erreichbar) und aus
/// dem Anmeldespeicher löschen.
#[tauri::command]
pub async fn nc_logout(app: AppHandle) -> Result<(), Error> {
    crate::blocking(app, |state| {
        let mut c = load(&state.data_dir);
        if c.server.is_empty() {
            return Ok(());
        }
        if let Ok(conn) = Conn::new(&c)
            && let Err(e) = conn.ocs("DELETE", "core/apppassword", &[])
        {
            log::warn!("App-Passwort nicht widerrufen: {e}");
        }
        if let Ok(e) = keyring_entry(&c) {
            let _ = e.delete_credential();
        }
        c.server.clear();
        c.login_name.clear();
        c.user_id.clear();
        // Links gehören zu diesem Konto
        c.projects.clear();
        save(&state.data_dir, &c)
    })
    .await
}

/// Projekte mit Freigabelink (Projekt-ID → Link), für das Symbol in der
/// Seitenleiste; leer, solange nicht verbunden.
#[tauri::command]
pub fn nc_shared_projects(state: State<AppState>) -> HashMap<i64, String> {
    let c = load(&state.data_dir);
    if c.server.is_empty() {
        return HashMap::new();
    }
    c.projects
        .iter()
        .filter_map(|(id, l)| Some((id.parse().ok()?, l.share_url.clone()?)))
        .collect()
}

#[tauri::command]
pub fn nc_set_base_folder(state: State<AppState>, folder: String) -> Result<String, Error> {
    let mut c = load(&state.data_dir);
    c.base_folder = if folder.trim().is_empty() {
        String::new()
    } else {
        folder_parts(&folder)?.join("/")
    };
    save(&state.data_dir, &c)?;
    Ok(c.base().to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    files: usize,
    bytes: u64,
    /// Zielordner (zuletzt benutzt, sonst Basisordner/Projektname)
    folder: String,
    share_url: Option<String>,
}

#[tauri::command]
pub async fn nc_project_info(app: AppHandle, project_id: i64) -> Result<ProjectInfo, Error> {
    crate::blocking(app, move |state| {
        let (files, name) = {
            let cat = state.catalog.locked();
            (
                cat.project_exports(project_id)?,
                cat.project(project_id)?.name,
            )
        };
        let c = load(&state.data_dir);
        let link = c.projects.get(&project_id.to_string());
        Ok(ProjectInfo {
            bytes: files
                .iter()
                .filter_map(|p| std::fs::metadata(p).ok())
                .map(|m| m.len())
                .sum(),
            files: files.len(),
            folder: link
                .map(|l| l.folder.clone())
                .unwrap_or_else(|| format!("{}/{}", c.base(), folder_name(&name))),
            share_url: link.and_then(|l| l.share_url.clone()),
        })
    })
    .await
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareOptions {
    /// Leer: ohne Passwort
    password: Option<String>,
    /// `JJJJ-MM-TT`, leer: ohne Ablauf
    expire: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadOptions {
    folder: String,
    /// Dateien im Ordner löschen, die es im Projekt nicht mehr gibt
    #[serde(default)]
    remove_missing: bool,
    /// Öffentlicher Link; `None`: einen vorhandenen entfernen
    share: Option<ShareOptions>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    done: usize,
    total: usize,
    current: Option<String>,
    uploaded: usize,
    unchanged: usize,
    removed: usize,
    failed: Vec<String>,
    finished: bool,
    error: Option<String>,
    share_url: Option<String>,
}

static UPLOADING: AtomicBool = AtomicBool::new(false);

/// Lädt die Exporte des Projekts hoch (gleicht ab) und teilt den Ordner.
/// Fortschritt und Ergebnis per `nextcloud-progress`.
#[tauri::command]
pub fn nc_upload_project(
    app: AppHandle,
    project_id: i64,
    options: UploadOptions,
) -> Result<(), Error> {
    let parts = folder_parts(&options.folder)?;
    if let Some(e) = options.share.as_ref().and_then(|s| s.expire.as_deref())
        && !e.is_empty()
        && !(e.len() == 10 && e.as_bytes()[4] == b'-' && e.as_bytes()[7] == b'-')
    {
        return Err(Error::Invalid(tr!(
            "Ungültiges Datum: {e}",
            "Invalid date: {e}"
        )));
    }
    if UPLOADING.swap(true, Ordering::SeqCst) {
        return Err(Error::Invalid(
            tr_s(
                "Es läuft bereits ein Hochladen.",
                "An upload is already running.",
            )
            .into(),
        ));
    }
    std::thread::spawn(move || {
        let mut p = Progress::default();
        let res = upload(&app, project_id, &options, &parts, &mut p);
        p.finished = true;
        p.current = None;
        if let Err(e) = res {
            p.error = Some(e.to_string());
        }
        UPLOADING.store(false, Ordering::SeqCst);
        let _ = app.emit("nextcloud-progress", p);
    });
    Ok(())
}

fn upload(
    app: &AppHandle,
    project_id: i64,
    o: &UploadOptions,
    parts: &[String],
    p: &mut Progress,
) -> Result<(), Error> {
    let state = app.state::<AppState>();
    let files = state.catalog.locked().project_exports(project_id)?;
    let c = load(&state.data_dir);
    let conn = Conn::new(&c)?;
    let names = unique_names(&files);
    p.total = files.len();
    let _ = app.emit("nextcloud-progress", p.clone());

    conn.mkdirs(parts)?;
    let remote = conn.list(parts)?;
    for (file, name) in files.iter().zip(&names) {
        p.current = Some(name.clone());
        let _ = app.emit("nextcloud-progress", p.clone());
        let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
        let same = remote
            .get(name)
            .is_some_and(|(len, m)| *len == size && Some(*m) == mtime_secs(file));
        if same {
            p.unchanged += 1;
        } else {
            match conn.upload(parts, name, file) {
                Ok(()) => p.uploaded += 1,
                Err(e) => {
                    log::warn!("Hochladen von {} fehlgeschlagen: {e}", file.display());
                    p.failed.push(format!("{name}: {e}"));
                }
            }
        }
        p.done += 1;
    }
    if o.remove_missing {
        let keep: HashSet<&str> = names.iter().map(String::as_str).collect();
        for name in remote.keys().filter(|n| !keep.contains(n.as_str())) {
            match conn.delete(parts, name) {
                Ok(()) => p.removed += 1,
                Err(e) => p.failed.push(format!("{name}: {e}")),
            }
        }
    }
    p.current = None;
    p.share_url = conn.share(parts, o.share.as_ref())?;

    let mut c = load(&state.data_dir);
    c.projects.insert(
        project_id.to_string(),
        ProjectLink {
            folder: parts.join("/"),
            share_url: p.share_url.clone(),
        },
    );
    save(&state.data_dir, &c)?;
    log::info!(
        "Nextcloud: {} hochgeladen, {} unverändert, {} entfernt, {} fehlgeschlagen",
        p.uploaded,
        p.unchanged,
        p.removed,
        p.failed.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_addresses_are_normalized() {
        assert_eq!(
            normalize_server("cloud.example.org/").unwrap(),
            "https://cloud.example.org"
        );
        assert_eq!(
            normalize_server(" https://example.org/nextcloud/index.php ").unwrap(),
            "https://example.org/nextcloud"
        );
        assert_eq!(
            normalize_server("http://192.168.1.5:8080").unwrap(),
            "http://192.168.1.5:8080"
        );
        assert_eq!(
            normalize_server("http://nas.local").unwrap(),
            "http://nas.local"
        );
        assert!(normalize_server("http://cloud.example.org").is_err());
        assert!(normalize_server("ftp://example.org").is_err());
        assert!(normalize_server("https://").is_err());
    }

    #[test]
    fn paths_and_names() {
        assert_eq!(enc("Reise Süd/1"), "Reise%20S%C3%BCd%2F1");
        assert_eq!(dec("Reise%20S%C3%BCd%2F1"), "Reise Süd/1");
        assert_eq!(
            folder_parts(" Lumenkorn / Reise ").unwrap(),
            ["Lumenkorn", "Reise"]
        );
        assert!(folder_parts("a/../b").is_err());
        assert!(folder_parts("  ").is_err());
        assert_eq!(folder_name("Urlaub/2026"), "Urlaub-2026");
        let names = unique_names(&[
            PathBuf::from("a/x.jpg"),
            PathBuf::from("b/X.jpg"),
            PathBuf::from("c/x.jpg"),
            PathBuf::from("c/y.dng"),
        ]);
        assert_eq!(names, ["x.jpg", "X-2.jpg", "x-3.jpg", "y.dng"]);
    }

    #[test]
    fn propfind_lists_files_only() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns">
 <d:response><d:href>/remote.php/dav/files/anna/Lumenkorn/Reise/</d:href>
  <d:propstat><d:prop><d:getlastmodified>Wed, 28 Oct 2026 08:00:00 GMT</d:getlastmodified>
  <d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response>
 <d:response><d:href>/remote.php/dav/files/anna/Lumenkorn/Reise/S%C3%BCd%20eins.jpg</d:href>
  <d:propstat><d:prop><d:getlastmodified>Wed, 28 Oct 2026 08:15:30 GMT</d:getlastmodified>
  <d:getcontentlength>12345</d:getcontentlength><d:resourcetype/></d:prop></d:propstat></d:response>
 <d:response><d:href>/remote.php/dav/files/anna/Lumenkorn/Reise/Unter/</d:href>
  <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response>
</d:multistatus>"#;
        let m = parse_propfind(xml);
        assert_eq!(m.len(), 1);
        let (len, t) = m["Süd eins.jpg"];
        assert_eq!(len, 12345);
        // 2026-10-28 08:15:30 UTC
        assert_eq!(t, 1_793_175_330);
    }

    #[test]
    fn finds_the_public_link() {
        let list: serde_json::Value = serde_json::json!([
            {"id": "7", "share_type": 0, "url": null},
            {"id": 12, "share_type": 3, "url": "https://c.example/s/AbC"}
        ]);
        assert_eq!(
            public_share(&list),
            Some(("12".into(), "https://c.example/s/AbC".into()))
        );
        assert_eq!(public_share(&serde_json::json!([])), None);
    }
}
