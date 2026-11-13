//! MCP-Schnittstelle: KI-Agenten (Claude, ChatGPT/Codex, …) bedienen
//! Lumenkorn über das Model Context Protocol.
//!
//! - **HTTP** („Streamable HTTP“, nur JSON-Antworten): Die laufende App hört
//!   auf `http://127.0.0.1:<port>/mcp`. Jede Anfrage braucht den Schlüssel aus
//!   `mcp.json` im Datenordner (`Authorization: Bearer …`); Anfragen von
//!   Webseiten (fremder `Origin`) werden abgewiesen.
//! - **stdio**: `lumenkorn --mcp` ist eine Brücke für Agenten, die Server als
//!   Programm starten (Claude Desktop, Codex CLI …). Sie liest Port und
//!   Schlüssel aus `mcp.json`, startet Lumenkorn bei Bedarf und reicht jede
//!   Zeile an die laufende App weiter.
//!
//! Standardmäßig aus; eingeschaltet wird im KI-Dialog. Die Werkzeuge lesen,
//! bewerten, verschlagworten, entwickeln und exportieren – Fotos löschen oder
//! Ordner entfernen können sie nicht. Änderungen erscheinen sofort in der
//! Oberfläche und lassen sich bei geöffnetem Foto mit Strg+Z zurücknehmen.

use crate::AppState;
use lumenkorn_core::Error;
use lumenkorn_core::LockExt;
use lumenkorn_core::model::{ImageQuery, Scope, SortOrder};
use lumenkorn_develop::DevelopParams;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

mod http;
mod stdio;
mod tools;

use http::*;
pub use stdio::*;
use tools::*;

/// Bevorzugter Port; ist er belegt, nimmt die App einen freien.
const PORT: u16 = 47_823;
const PROTOCOL: &str = "2025-06-18";
/// Ältere Protokollstände, die genauso funktionieren (nur Werkzeuge)
const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const FILE: &str = "mcp.json";
const IDENTIFIER: &str = "io.github.drayprescot.lumenkorn";

/// Einstellungen und Zugangsdaten (Datenordner/mcp.json)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct McpConfig {
    pub enabled: bool,
    /// Port, auf dem die App gerade hört (0 = keiner)
    pub port: u16,
    pub token: String,
}

#[derive(Default)]
pub struct McpState {
    server: Mutex<Option<std::sync::Arc<tiny_http::Server>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    enabled: bool,
    running: bool,
    url: Option<String>,
    token: String,
    /// Programm für die stdio-Brücke (`<exe> --mcp`)
    command: String,
}

fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

fn load(data_dir: &Path) -> McpConfig {
    lumenkorn_develop::fsutil::load_json_or_default(&config_path(data_dir))
}

fn save(data_dir: &Path, c: &McpConfig) {
    if let Err(e) = lumenkorn_develop::fsutil::save_json(&config_path(data_dir), c) {
        log::warn!("MCP-Einstellungen nicht gespeichert: {e}");
    }
    // Enthält den Schlüssel: Andere Benutzer des Rechners sollen ihn nicht lesen.
    // Unter Windows schützt das Benutzerprofil (%APPDATA%) die Datei schon.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let only_owner = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(config_path(data_dir), only_owner) {
            log::warn!("Rechte von {FILE} nicht gesetzt: {e}");
        }
    }
}

/// Zufälliger Schlüssel: 256 Bit vom Zufallsgenerator des Betriebssystems, hex.
fn new_token() -> String {
    let mut b = [0u8; 32];
    if getrandom::fill(&mut b).is_err() {
        // Ohne Zufallsquelle gibt es keinen brauchbaren Schlüssel
        log::error!("MCP: keine Zufallsquelle für den Schlüssel");
        return String::new();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Vergleich ohne vorzeitigen Abbruch (Laufzeit hängt nicht vom Treffer ab)
fn same_token(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= (a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0)) as usize;
    }
    diff == 0
}

fn exe() -> String {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "lumenkorn".into())
}

/// Beim Start: Server starten, falls eingeschaltet.
pub fn start_if_enabled(app: &AppHandle) {
    let state = app.state::<AppState>();
    if load(&state.data_dir).enabled
        && let Err(e) = start(app)
    {
        log::warn!("MCP-Server nicht gestartet: {e}");
    }
}

fn start(app: &AppHandle) -> Result<(), Error> {
    let state = app.state::<AppState>();
    let mut slot = state.mcp.server.locked();
    if slot.is_some() {
        return Ok(());
    }
    let server = tiny_http::Server::http(("127.0.0.1", PORT))
        .or_else(|_| tiny_http::Server::http(("127.0.0.1", 0)))
        .map_err(|e| Error::Invalid(format!("MCP: {e}")))?;
    let port = server.server_addr().to_ip().map_or(0, |a| a.port());
    let server = std::sync::Arc::new(server);
    let mut cfg = load(&state.data_dir);
    if cfg.token.is_empty() {
        cfg.token = new_token();
    }
    cfg.enabled = true;
    cfg.port = port;
    save(&state.data_dir, &cfg);
    let (srv, handle, token) = (server.clone(), app.clone(), cfg.token.clone());
    std::thread::Builder::new()
        .name("lumenkorn-mcp".into())
        .spawn(move || {
            for req in srv.incoming_requests() {
                handle_http(&handle, &token, port, req);
            }
        })
        .map_err(|e| Error::Invalid(e.to_string()))?;
    log::info!("MCP-Server auf http://127.0.0.1:{port}/mcp");
    *slot = Some(server);
    Ok(())
}

fn stop(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Some(s) = state.mcp.server.locked().take() {
        s.unblock();
    }
    let mut cfg = load(&state.data_dir);
    cfg.enabled = false;
    cfg.port = 0;
    save(&state.data_dir, &cfg);
}

#[tauri::command]
pub fn mcp_status(app: AppHandle) -> McpStatus {
    let state = app.state::<AppState>();
    let cfg = load(&state.data_dir);
    let running = state.mcp.server.locked().is_some();
    McpStatus {
        enabled: cfg.enabled,
        running,
        url: (running && cfg.port > 0).then(|| format!("http://127.0.0.1:{}/mcp", cfg.port)),
        token: cfg.token,
        command: exe(),
    }
}

#[tauri::command]
pub fn mcp_set_enabled(app: AppHandle, enabled: bool) -> Result<McpStatus, Error> {
    if enabled {
        start(&app)?;
    } else {
        stop(&app);
    }
    Ok(mcp_status(app))
}

/// Neuer Schlüssel – alte Zugänge gelten danach nicht mehr (Neustart des
/// Servers nötig, damit er greift).
#[tauri::command]
pub fn mcp_new_token(app: AppHandle) -> Result<McpStatus, Error> {
    let state = app.state::<AppState>();
    let mut cfg = load(&state.data_dir);
    cfg.token = new_token();
    save(&state.data_dir, &cfg);
    let was = state.mcp.server.locked().is_some();
    if was {
        stop(&app);
        start(&app)?;
    }
    Ok(mcp_status(app))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Die Beschreibung von `set_develop_settings` ist für Agenten die einzige Liste der
    /// Entwicklungswerte. Ein neuer Regler gehört dort hinein.
    #[test]
    fn develop_settings_description_names_every_field() {
        let tool = set_develop_settings_tool();
        let text = tool["description"].as_str().unwrap();
        // Masken über `add_mask`; Rauschreduzierung, Reparaturen und Version nicht über MCP
        let elsewhere = ["version", "masks", "denoise", "repairs"];
        let defaults = serde_json::to_value(DevelopParams::default()).unwrap();
        let missing: Vec<&String> = defaults
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| !elsewhere.contains(&k.as_str()))
            .filter(|k| !text.contains(k.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "in der Beschreibung fehlen: {missing:?}"
        );
    }

    #[test]
    fn token_is_random_hex_and_compared_exactly() {
        let (a, b) = (new_token(), new_token());
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert!(same_token(&a, &a));
        assert!(!same_token(&a, &b));
        assert!(!same_token(&a, &a[..63]));
        assert!(!same_token("", &a));
    }

    #[test]
    fn only_exact_local_origins() {
        for ok in ["http://localhost", "http://127.0.0.1:5173"] {
            assert!(local_origin(ok), "{ok}");
        }
        for bad in [
            "null",
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com:80",
            "http://localhost:80@evil.com",
            "https://evil.com",
        ] {
            assert!(!local_origin(bad), "{bad}");
        }
    }

    #[test]
    fn host_header_must_be_local_with_our_port() {
        for ok in [
            "127.0.0.1:47800",
            "localhost:47800",
            "[::1]:47800",
            "LocalHost:47800",
        ] {
            assert!(local_host(ok, 47800), "{ok}");
        }
        for bad in [
            "evil.com",
            "evil.com:47800",
            "127.0.0.1",
            "127.0.0.1:1",
            "localhost.evil.com:47800",
            "127.0.0.1:47800.evil.com",
        ] {
            assert!(!local_host(bad, 47800), "{bad}");
        }
    }

    #[test]
    fn path_inside_is_component_wise() {
        assert!(path_inside(r"C:\Bilder\Urlaub", r"C:\Bilder"));
        assert!(path_inside(r"C:\Bilder", r"C:\Bilder"));
        assert!(!path_inside(r"C:\Bilder2", r"C:\Bilder"));
        assert!(!path_inside(r"C:\Bilder\..\Windows", r"C:\Bilder"));
        assert!(!path_inside(r"C:\Bilder\..\..\Windows", r"C:\Bilder"));
        assert!(path_inside(r"C:\Bilder\a\..\b", r"C:\Bilder"));
        assert!(!path_inside(r"C:\Bilder", r"C:\Bilder\Urlaub"));
        assert!(!path_inside("/a/b", ""));
        assert!(path_inside(r"\\?\C:\Bilder\x", r"C:\Bilder"));
        if cfg!(windows) {
            assert!(path_inside(r"c:\bilder\x", r"C:\Bilder"));
        }
    }

    #[test]
    fn export_folder_is_checked() {
        let base = std::env::temp_dir().join(format!("lk_mcp_{}", std::process::id()));
        let (root, root2) = (base.join("Bilder"), base.join("Bilder2"));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(&root2).unwrap();
        let allowed = vec![root.clone()];
        let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
        assert!(check_export_folder(&s(&root), &allowed).is_ok());
        assert!(check_export_folder(&s(&root.join("sub")), &allowed).is_ok());
        assert!(check_export_folder(&s(&root.join("sub").join("..")), &allowed).is_ok());
        assert!(check_export_folder(&s(&root2), &allowed).is_err());
        assert!(check_export_folder(&s(&root.join("..").join("Bilder2")), &allowed).is_err());
        assert!(check_export_folder(&s(&root.join("fehlt")), &allowed).is_err());
        assert!(check_export_folder("Export", &allowed).is_err());
        assert!(check_export_folder("..", &allowed).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    fn rpc(body: Value) -> Option<Value> {
        let echo = |name: &str, args: &Value| -> ToolResult {
            if name == "fail" {
                return Err("kaputt".into());
            }
            text(json!({ "tool": name, "args": args }))
        };
        handle_body_with(&body.to_string(), &echo).map(|s| serde_json::from_str(&s).unwrap())
    }

    #[test]
    fn speaks_mcp_initialize_list_and_call() {
        let init = rpc(json!({"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}))
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "lumenkorn");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        // Unbekannte Version: unsere
        let init = rpc(json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}})).unwrap();
        assert_eq!(init["result"]["protocolVersion"], PROTOCOL);
        // Benachrichtigung: keine Antwort
        assert!(rpc(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());

        let list = rpc(json!({"jsonrpc":"2.0","id":3,"method":"tools/list"})).unwrap();
        let tools = list["result"]["tools"].as_array().unwrap();
        assert!(tools.len() >= 15);
        for t in tools {
            assert!(
                t["name"]
                    .as_str()
                    .unwrap()
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_')
            );
            assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
            assert!(t["description"].as_str().unwrap().len() > 20);
        }

        let ok = rpc(json!({"jsonrpc":"2.0","id":"a","method":"tools/call","params":{"name":"rate_photos","arguments":{"ids":[1],"rating":3}}})).unwrap();
        assert_eq!(ok["id"], "a");
        assert_eq!(ok["result"]["isError"], false);
        assert!(
            ok["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("rate_photos")
        );
        let bad =
            rpc(json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"fail"}}))
                .unwrap();
        assert_eq!(bad["result"]["isError"], true);
        let none = rpc(json!({"jsonrpc":"2.0","id":5,"method":"nope"})).unwrap();
        assert_eq!(none["error"]["code"], -32601);
        // Liste von Nachrichten
        let batch = rpc(json!([{"jsonrpc":"2.0","id":6,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/x"}])).unwrap();
        assert_eq!(batch.as_array().unwrap().len(), 1);
        let parse = handle_body_with("{kaputt", &|_, _| Ok(vec![])).unwrap();
        assert!(parse.contains("-32700"));
    }

    #[test]
    fn merge_patch_changes_only_given_keys() {
        let mut p = serde_json::to_value(DevelopParams {
            exposure: 0.5,
            contrast: 10.0,
            ..Default::default()
        })
        .unwrap();
        merge(
            &mut p,
            &json!({ "contrast": 25, "hsl": { "saturation": [0, 0, 0, 0, 0, -50, 0, 0] } }),
        );
        let out: DevelopParams = serde_json::from_value(p).unwrap();
        assert_eq!((out.exposure, out.contrast), (0.5, 25.0));
        assert_eq!(out.hsl.saturation[5], -50.0);
        assert_eq!(out.hsl.hue, [0.0; 8], "Geschwister im Objekt bleiben");
        let changes = non_default(&out);
        assert_eq!(changes["contrast"], 25.0);
        assert!(changes.get("temperature").is_none());
    }

    /// Brücke stdio → HTTP gegen einen echten Server mit Schlüssel
    #[test]
    fn stdio_bridge_forwards_to_the_running_app() {
        let dir = tempfile::tempdir().unwrap();
        let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let port = server.server_addr().to_ip().unwrap().port();
        let cfg = McpConfig {
            enabled: true,
            port,
            token: "geheim".into(),
        };
        save(dir.path(), &cfg);
        let srv = server.clone();
        std::thread::spawn(move || {
            for mut req in srv.incoming_requests() {
                let auth = req.headers().iter().any(|h| {
                    h.field
                        .as_str()
                        .as_str()
                        .eq_ignore_ascii_case("authorization")
                        && h.value.as_str() == "Bearer geheim"
                });
                let mut body = String::new();
                std::io::Read::read_to_string(req.as_reader(), &mut body).unwrap();
                let out = handle_body_with(&body, &|n, _| text(n));
                let (code, body) = match (auth, out) {
                    (false, _) => (401, String::new()),
                    (true, Some(b)) => (200, b),
                    (true, None) => (202, String::new()),
                };
                let _ = req.respond(tiny_http::Response::from_string(body).with_status_code(code));
            }
        });
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let mut launched = true; // nichts starten
        let r = forward(
            &agent,
            dir.path(),
            r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#,
            &mut launched,
        )
        .unwrap();
        assert!(r.unwrap().contains("\"id\":7"));
        let n = forward(
            &agent,
            dir.path(),
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &mut launched,
        )
        .unwrap();
        assert!(n.is_none());
        // Ausgeschaltet: klare Meldung statt Warten
        save(
            dir.path(),
            &McpConfig {
                enabled: false,
                ..cfg
            },
        );
        let off = forward(
            &agent,
            dir.path(),
            r#"{"jsonrpc":"2.0","id":8,"method":"ping"}"#,
            &mut launched,
        );
        assert!(off.unwrap_err().contains("switched off"));
        server.unblock();
    }
}
