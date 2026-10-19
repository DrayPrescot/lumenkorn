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
    std::fs::read_to_string(config_path(data_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(data_dir: &Path, c: &McpConfig) {
    if let Ok(s) = serde_json::to_string_pretty(c)
        && let Err(e) = std::fs::write(config_path(data_dir), s)
    {
        log::warn!("MCP-Einstellungen nicht gespeichert: {e}");
    }
}

/// Zufälliger Schlüssel (128 Bit aus den Zufallsschlüsseln von `RandomState`,
/// die das Betriebssystem liefert).
fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    (0..2)
        .map(|i| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(i);
            h.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos()),
            );
            format!("{:016x}", h.finish())
        })
        .collect()
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
                handle_http(&handle, &token, req);
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

// --- HTTP ------------------------------------------------------------------------

/// Origin einer lokalen Seite: genau 127.0.0.1 bzw. localhost, mit oder ohne
/// Port – nicht etwa `http://localhost.example.com`.
fn local_origin(origin: &str) -> bool {
    if origin == "null" {
        return true;
    }
    let Some(rest) = origin.strip_prefix("http://") else {
        return false;
    };
    let host = match rest.split_once(':') {
        Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
        Some(_) => return false,
        None => rest,
    };
    host == "127.0.0.1" || host == "localhost"
}

fn handle_http(app: &AppHandle, token: &str, mut req: tiny_http::Request) {
    use tiny_http::{Header, Method, Response};
    let header = |name: &str| {
        req.headers()
            .iter()
            .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str().to_string())
    };
    let json_header = Header::from_bytes("Content-Type", "application/json").unwrap();
    let reply = |req: tiny_http::Request, code: u16, body: String| {
        let _ = req.respond(
            Response::from_string(body)
                .with_status_code(code)
                .with_header(json_header.clone()),
        );
    };
    // Nur lokale Webseiten dürfen (Schutz vor DNS-Rebinding)
    if let Some(origin) = header("Origin")
        && !local_origin(&origin)
    {
        return reply(req, 403, r#"{"error":"origin not allowed"}"#.into());
    }
    let auth_ok = header("Authorization")
        .is_some_and(|a| a.strip_prefix("Bearer ").is_some_and(|t| t.trim() == token));
    if !auth_ok {
        return reply(req, 401, r#"{"error":"missing or wrong token"}"#.into());
    }
    if req.url().split('?').next() != Some("/mcp") {
        return reply(req, 404, r#"{"error":"use /mcp"}"#.into());
    }
    match req.method() {
        Method::Post => {}
        // Kein Ereignisstrom vom Server aus (erlaubt laut Protokoll)
        Method::Get => return reply(req, 405, String::new()),
        Method::Delete => return reply(req, 200, String::new()),
        _ => return reply(req, 405, String::new()),
    }
    let mut body = String::new();
    if std::io::Read::read_to_string(req.as_reader(), &mut body).is_err() {
        return reply(req, 400, String::new());
    }
    match handle_body(app, &body) {
        Some(out) => reply(req, 200, out),
        // Nur Benachrichtigungen: angenommen, keine Antwort
        None => reply(req, 202, String::new()),
    }
}

/// JSON-RPC (einzeln oder als Liste). `None`, wenn nichts zu antworten ist.
pub(crate) fn handle_body(app: &AppHandle, body: &str) -> Option<String> {
    handle_body_with(body, &|name, args| {
        tauri::async_runtime::block_on(call(app, name, args))
    })
}

/// Werkzeugaufruf: Name und Argumente → Inhalt
type Caller<'a> = &'a dyn Fn(&str, &Value) -> ToolResult;

fn handle_body_with(body: &str, call: Caller) -> Option<String> {
    let msg: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => {
            return Some(
                json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}})
                    .to_string(),
            );
        }
    };
    if let Value::Array(list) = msg {
        let out: Vec<Value> = list
            .into_iter()
            .filter_map(|m| handle_msg(m, call))
            .collect();
        return (!out.is_empty()).then(|| Value::Array(out).to_string());
    }
    handle_msg(msg, call).map(|v| v.to_string())
}

fn handle_msg(msg: Value, call: Caller) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    // Benachrichtigungen (ohne id) und Antworten des Clients
    let id = id?;
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            let asked = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL);
            let version = if PROTOCOLS.contains(&asked) {
                asked
            } else {
                PROTOCOL
            };
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": {
                    "name": "lumenkorn",
                    "title": "Lumenkorn",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let out = call(name, &args);
            Ok(match out {
                Ok(content) => json!({ "content": content, "isError": false }),
                Err(e) => json!({
                    "content": [{ "type": "text", "text": e }],
                    "isError": true,
                }),
            })
        }
        "resources/list" => Ok(json!({ "resources": [] })),
        "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, m)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": m } })
        }
    })
}

const INSTRUCTIONS: &str = "Lumenkorn is a local photo library and RAW developer. \
Find photos with search_photos (text, folder, project, person, rating), look at them with \
view_photo, then rate, flag, tag, develop (set_develop_settings, auto_tone, apply_preset, \
add_mask) or export them. Photo ids come from search_photos. Develop values use the same \
ranges as the app's sliders (most −100…100, exposure −5…5 EV). Changes show up in the \
app immediately. Nothing here deletes files.";

// --- Werkzeuge ------------------------------------------------------------------

fn tool(name: &str, title: &str, description: &str, schema: Value, read_only: bool) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": schema,
        "annotations": { "readOnlyHint": read_only, "destructiveHint": false, "openWorldHint": false },
    })
}

fn ids_schema() -> Value {
    json!({ "type": "array", "items": { "type": "integer" }, "minItems": 1, "description": "Photo ids" })
}

fn tools() -> Vec<Value> {
    vec![
        tool(
            "list_library",
            "List folders, projects and people",
            "Source folders (with photo counts per subfolder), projects, named people and people groups, and presets. Use the ids and paths for search_photos.",
            json!({ "type": "object", "properties": {
                "max_folders": { "type": "integer", "description": "Maximum number of folders listed (default 300)" }
            } }),
            true,
        ),
        tool(
            "search_photos",
            "Search photos",
            "Find photos. All filters are optional and combined. `text` searches file name, folder, camera, lens, keywords and people; with `by_content` it searches what is in the picture (AI, e.g. \"sunset over the sea\"). Returns id, file, date, rating (0–5), flag (1 pick, -1 rejected, 0 none) and whether it is edited.",
            json!({ "type": "object", "properties": {
                "text": { "type": "string" },
                "by_content": { "type": "boolean", "description": "Search by image content (needs the AI keyword model)" },
                "folder": { "type": "string", "description": "Folder path from list_library" },
                "include_subfolders": { "type": "boolean", "default": true },
                "project_id": { "type": "integer" },
                "person_id": { "type": "integer" },
                "min_rating": { "type": "integer", "minimum": 0, "maximum": 5 },
                "flag": { "type": "string", "enum": ["pick", "rejected", "none"] },
                "edited": { "type": "boolean" },
                "sort": { "type": "string", "enum": ["oldest", "newest", "name", "rating"], "default": "oldest" },
                "limit": { "type": "integer", "default": 50, "maximum": 500 },
                "offset": { "type": "integer", "default": 0 }
            } }),
            true,
        ),
        tool(
            "get_photo",
            "Photo details",
            "Metadata of one photo: file, folder, date, camera, lens, exposure, size, rating, flag, keywords, recognized people and the develop settings that differ from the defaults.",
            json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] }),
            true,
        ),
        tool(
            "view_photo",
            "Look at a photo",
            "Returns the photo as a JPEG image with its current edits, so you can judge it. `size` is the longest edge in pixels (default 1024, up to 2048).",
            json!({ "type": "object", "properties": {
                "id": { "type": "integer" },
                "size": { "type": "integer", "minimum": 128, "maximum": 2048, "default": 1024 }
            }, "required": ["id"] }),
            true,
        ),
        tool(
            "rate_photos",
            "Rate photos",
            "Set the star rating (0 removes it).",
            json!({ "type": "object", "properties": {
                "ids": ids_schema(),
                "rating": { "type": "integer", "minimum": 0, "maximum": 5 }
            }, "required": ["ids", "rating"] }),
            false,
        ),
        tool(
            "flag_photos",
            "Flag photos",
            "Mark photos as pick, rejected, or remove the flag. Rejecting does not delete anything.",
            json!({ "type": "object", "properties": {
                "ids": ids_schema(),
                "flag": { "type": "string", "enum": ["pick", "rejected", "none"] }
            }, "required": ["ids", "flag"] }),
            false,
        ),
        tool(
            "tag_photos",
            "Add or remove keywords",
            "Add and/or remove manual keywords on photos.",
            json!({ "type": "object", "properties": {
                "ids": ids_schema(),
                "add": { "type": "array", "items": { "type": "string" } },
                "remove": { "type": "array", "items": { "type": "string" } }
            }, "required": ["ids"] }),
            false,
        ),
        tool(
            "get_develop_settings",
            "Read develop settings",
            "All develop settings of a photo as JSON (the format set_develop_settings accepts).",
            json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] }),
            true,
        ),
        tool(
            "set_develop_settings",
            "Change develop settings",
            "Merge the given settings into the photos' current ones (JSON merge: only the keys you pass change; nested objects merge, arrays are replaced). Keys and ranges: temperature, tint (−100…100, relative to the camera); exposure (−5…5 EV); contrast, highlights, shadows, whites, blacks, clarity, dehaze, vibrance, saturation (−100…100); sharpen (0…100); monochrome (bool); lensCorrection (bool); hsl {hue, saturation, luminance: 8 values each for red, orange, yellow, green, aqua, blue, purple, magenta, −100…100}; colorGrade {shadows, midtones, highlights, global: {hue 0…360, saturation 0…100, luminance −100…100}, blending, balance}; toneCurve {rgb, red, green, blue: points [[x, y], …] 0…1 on sRGB-encoded values, sorted by x, at most 16, [] = unchanged; preserveColors (bool, default true: the rgb curve acts on luminance)}; crop {x, y, w, h normalized 0…1} or null; straighten (−45…45°); rotate (quarter turns 0…3); flipHorizontal (bool). Unknown keys are reported and ignored.",
            json!({ "type": "object", "properties": {
                "ids": ids_schema(),
                "settings": { "type": "object", "additionalProperties": true }
            }, "required": ["ids", "settings"] }),
            false,
        ),
        tool(
            "auto_tone",
            "Auto tone",
            "Like the Auto button: sets exposure, contrast, highlights, shadows, whites, blacks and vibrance for a balanced result. Returns the values.",
            json!({ "type": "object", "properties": { "ids": ids_schema() }, "required": ["ids"] }),
            false,
        ),
        tool(
            "reset_photos",
            "Reset to original",
            "Remove all edits of the photos (they look like the camera original again).",
            json!({ "type": "object", "properties": { "ids": ids_schema() }, "required": ["ids"] }),
            false,
        ),
        tool(
            "apply_preset",
            "Apply a preset",
            "Apply a saved preset (name or id from list_library) to photos.",
            json!({ "type": "object", "properties": {
                "ids": ids_schema(),
                "preset": { "description": "Preset name or id", "type": ["string", "integer"] }
            }, "required": ["ids", "preset"] }),
            false,
        ),
        tool(
            "copy_settings",
            "Copy develop settings",
            "Copy parts of one photo's edit to other photos. `groups` chooses what (default: everything except geometry): whiteBalance, light, color, hsl, colorGrade, details, masks, geometry.",
            json!({ "type": "object", "properties": {
                "from_id": { "type": "integer" },
                "to_ids": ids_schema(),
                "groups": { "type": "array", "items": { "type": "string", "enum": ["whiteBalance", "light", "color", "hsl", "colorGrade", "details", "masks", "geometry"] } }
            }, "required": ["from_id", "to_ids"] }),
            false,
        ),
        tool(
            "add_mask",
            "Add an AI mask with local adjustments",
            "Detect an area with AI and adjust only there. kind: subject, sky, person (whole person), person:face (facial skin), person:hair, person:eyes, person:iris, person:brows, person:lips, person:bodySkin, person:clothes, animal (all animals), animal:bird, animal:mammal, animal:reptile. adjust: exposure (−4…4 EV), contrast, highlights, shadows, clarity, temperature, tint, saturation (−100…100). `invert` adjusts everything except the area. Needs the AI masks to be installed in the app.",
            json!({ "type": "object", "properties": {
                "id": { "type": "integer" },
                "kind": { "type": "string" },
                "adjust": { "type": "object", "properties": {
                    "exposure": { "type": "number" }, "contrast": { "type": "number" },
                    "highlights": { "type": "number" }, "shadows": { "type": "number" },
                    "clarity": { "type": "number" }, "temperature": { "type": "number" },
                    "tint": { "type": "number" }, "saturation": { "type": "number" }
                } },
                "invert": { "type": "boolean" },
                "name": { "type": "string" }
            }, "required": ["id", "kind", "adjust"] }),
            false,
        ),
        tool(
            "export_photos",
            "Export photos",
            "Export developed photos in the background. Without `folder`, each goes into an \"Export\" subfolder next to its original. format jpeg (quality 1–100), png or tiff; long_edge limits the size in pixels.",
            json!({ "type": "object", "properties": {
                "ids": ids_schema(),
                "folder": { "type": "string" },
                "format": { "type": "string", "enum": ["jpeg", "png", "tiff"], "default": "jpeg" },
                "quality": { "type": "integer", "minimum": 1, "maximum": 100, "default": 90 },
                "long_edge": { "type": "integer", "minimum": 64 }
            }, "required": ["ids"] }),
            false,
        ),
        tool(
            "show_photo",
            "Show a photo in the app",
            "Open a photo in Lumenkorn's window (library or develop view) so the user sees it.",
            json!({ "type": "object", "properties": {
                "id": { "type": "integer" },
                "develop": { "type": "boolean", "default": false }
            }, "required": ["id"] }),
            false,
        ),
    ]
}

type ToolResult = Result<Vec<Value>, String>;

fn text(v: impl Serialize) -> ToolResult {
    Ok(vec![json!({
        "type": "text",
        "text": serde_json::to_string_pretty(&v).unwrap_or_default(),
    })])
}

fn arg<'a>(a: &'a Value, k: &str) -> Option<&'a Value> {
    a.get(k).filter(|v| !v.is_null())
}
fn int(a: &Value, k: &str) -> Result<i64, String> {
    arg(a, k)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("`{k}` (integer) is required"))
}
fn ids(a: &Value, k: &str) -> Result<Vec<i64>, String> {
    let v: Vec<i64> = arg(a, k)
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    if v.is_empty() {
        return Err(format!("`{k}` (list of photo ids) is required"));
    }
    Ok(v)
}
fn e(err: Error) -> String {
    err.to_string()
}

/// Oberfläche über Änderungen informieren
fn changed(app: &AppHandle, develop: &[i64]) {
    let _ = app.emit("catalog-updated", ());
    if !develop.is_empty() {
        let _ = app.emit("develop-external", json!({ "ids": develop }));
    }
}

async fn call(app: &AppHandle, name: &str, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    log::info!("MCP: {name}");
    match name {
        "list_library" => {
            let max = arg(a, "max_folders").and_then(Value::as_u64).unwrap_or(300) as usize;
            let roots = crate::commands::list_roots(state.clone()).map_err(e)?;
            let mut folders = Vec::new();
            for r in &roots {
                if let Ok(tree) = crate::commands::folder_tree(state.clone(), r.id) {
                    flatten(&tree, 0, &mut folders, max);
                }
            }
            let projects = crate::commands::list_projects(state.clone()).map_err(e)?;
            let people = crate::people::people_list(state.clone()).map_err(e)?;
            let groups = crate::people::person_groups(state.clone()).map_err(e)?;
            let presets = crate::develop::preset_list(state.clone()).map_err(e)?;
            text(json!({
                "folders": folders,
                "projects": projects.iter().map(|p| json!({ "id": p.id, "name": p.name })).collect::<Vec<_>>(),
                "people": people.iter().filter(|p| p.name.is_some()).map(|p| json!({
                    "id": p.id, "name": p.name, "photos": p.image_count,
                    "group": p.group_id.and_then(|g| groups.iter().find(|x| x.id == g)).map(|g| g.name.clone()),
                })).collect::<Vec<_>>(),
                "unnamedPeopleGroups": people.iter().filter(|p| p.name.is_none()).count(),
                "presets": presets.iter().map(|p| json!({ "id": p.id, "name": p.name })).collect::<Vec<_>>(),
            }))
        }
        "search_photos" => search(app, a).await,
        "get_photo" => {
            let id = int(a, "id")?;
            let detail = crate::commands::image_detail(state.clone(), id).map_err(e)?;
            let faces = crate::people::image_faces(state.clone(), id).unwrap_or_default();
            let params = crate::develop::develop_params(state.clone(), id).map_err(e)?;
            let mut v = serde_json::to_value(&detail).unwrap_or_default();
            v["people"] = json!(
                faces
                    .iter()
                    .filter_map(|f| f.person_name.clone())
                    .collect::<Vec<_>>()
            );
            v["developChanges"] = non_default(&params);
            text(v)
        }
        "view_photo" => {
            let id = int(a, "id")?;
            let size = arg(a, "size")
                .and_then(Value::as_u64)
                .unwrap_or(1024)
                .clamp(128, 2048) as u32;
            let job = state.catalog.locked().thumb_job(id).map_err(e)?;
            let bytes = state
                .cache
                .get_or_render(lumenkorn_core::thumbs::Variant::Preview, &job)
                .map_err(e)?;
            let img = image::load_from_memory(&bytes).map_err(|x| x.to_string())?;
            let img = if img.width().max(img.height()) > size {
                img.resize(size, size, image::imageops::FilterType::Triangle)
            } else {
                img
            };
            let mut out = Vec::new();
            img.to_rgb8()
                .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(
                    &mut out, 85,
                ))
                .map_err(|x| x.to_string())?;
            use base64::Engine;
            Ok(vec![
                json!({ "type": "image", "mimeType": "image/jpeg",
                        "data": base64::engine::general_purpose::STANDARD.encode(&out) }),
                json!({ "type": "text", "text": format!("Photo {id}: {} ({}×{} shown)", job.path, img.width(), img.height()) }),
            ])
        }
        "rate_photos" => {
            let ids = ids(a, "ids")?;
            let r = int(a, "rating")?.clamp(0, 5);
            crate::commands::set_rating(state.clone(), ids.clone(), r).map_err(e)?;
            changed(app, &[]);
            text(json!({ "rated": ids.len(), "rating": r }))
        }
        "flag_photos" => {
            let ids = ids(a, "ids")?;
            let f = match arg(a, "flag").and_then(Value::as_str) {
                Some("pick") => 1,
                Some("rejected") => -1,
                Some("none") => 0,
                _ => return Err("`flag` must be pick, rejected or none".into()),
            };
            crate::commands::set_flag(state.clone(), ids.clone(), f).map_err(e)?;
            changed(app, &[]);
            text(json!({ "flagged": ids.len() }))
        }
        "tag_photos" => {
            let ids = ids(a, "ids")?;
            let list = |k| -> Vec<String> {
                arg(a, k)
                    .and_then(Value::as_array)
                    .map(|l| {
                        l.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let (add, remove) = (list("add"), list("remove"));
            for id in &ids {
                for t in &add {
                    crate::commands::add_tag(state.clone(), *id, t.clone()).map_err(e)?;
                }
                for t in &remove {
                    crate::commands::remove_tag(state.clone(), *id, t.clone()).map_err(e)?;
                }
            }
            let _ = app.emit("tags-changed", json!({ "ids": ids }));
            changed(app, &[]);
            text(json!({ "photos": ids.len(), "added": add, "removed": remove }))
        }
        "get_develop_settings" => {
            let id = int(a, "id")?;
            text(crate::develop::develop_params(state.clone(), id).map_err(e)?)
        }
        "set_develop_settings" => {
            let ids = ids(a, "ids")?;
            let patch = arg(a, "settings")
                .filter(|v| v.is_object())
                .ok_or("`settings` (object) is required")?;
            let known = serde_json::to_value(DevelopParams::default()).unwrap_or_default();
            let unknown: Vec<&String> = patch
                .as_object()
                .unwrap()
                .keys()
                .filter(|k| known.get(k.as_str()).is_none())
                .collect();
            for id in &ids {
                let cur = crate::develop::develop_params(state.clone(), *id).map_err(e)?;
                let mut v = serde_json::to_value(&cur).unwrap_or_default();
                merge(&mut v, patch);
                let p: DevelopParams =
                    serde_json::from_value(v).map_err(|x| format!("settings: {x}"))?;
                crate::develop::develop_save(app.clone(), state.clone(), vec![*id], p.sanitized())
                    .map_err(e)?;
            }
            changed(app, &ids);
            text(json!({ "updated": ids.len(), "ignoredUnknownKeys": unknown }))
        }
        "auto_tone" => {
            let ids = ids(a, "ids")?;
            let mut out = Vec::new();
            for id in &ids {
                let mut p = crate::develop::develop_params(state.clone(), *id).map_err(e)?;
                let t = crate::develop::develop_auto(app.clone(), *id, p.clone())
                    .await
                    .map_err(e)?;
                p.exposure = t.exposure;
                p.contrast = t.contrast;
                p.highlights = t.highlights;
                p.shadows = t.shadows;
                p.whites = t.whites;
                p.blacks = t.blacks;
                p.vibrance = t.vibrance;
                crate::develop::develop_save(app.clone(), state.clone(), vec![*id], p)
                    .map_err(e)?;
                out.push(json!({ "id": id, "values": t }));
            }
            changed(app, &ids);
            text(out)
        }
        "reset_photos" => {
            let ids = ids(a, "ids")?;
            crate::develop::develop_save(
                app.clone(),
                state.clone(),
                ids.clone(),
                DevelopParams::default(),
            )
            .map_err(e)?;
            changed(app, &ids);
            text(json!({ "reset": ids.len() }))
        }
        "apply_preset" => {
            let ids = ids(a, "ids")?;
            let presets = crate::develop::preset_list(state.clone()).map_err(e)?;
            let want = arg(a, "preset").ok_or("`preset` is required")?;
            let p = presets
                .iter()
                .find(|p| {
                    want.as_i64() == Some(p.id)
                        || want
                            .as_str()
                            .is_some_and(|n| n.eq_ignore_ascii_case(&p.name))
                })
                .ok_or_else(|| {
                    format!(
                        "No preset {want}. Available: {}",
                        presets
                            .iter()
                            .map(|p| p.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?;
            let mut groups = p.groups;
            // `basic` würde alle feinen Gruppen einschließen
            groups.basic = false;
            crate::develop::develop_transfer(state.clone(), ids.clone(), p.params.clone(), groups)
                .map_err(e)?;
            changed(app, &ids);
            text(json!({ "preset": p.name, "applied": ids.len() }))
        }
        "copy_settings" => {
            let from = int(a, "from_id")?;
            let to = ids(a, "to_ids")?;
            let src = crate::develop::develop_params(state.clone(), from).map_err(e)?;
            let mut g = lumenkorn_develop::TransferGroups {
                basic: false,
                ..Default::default()
            };
            let names: Vec<String> = arg(a, "groups")
                .and_then(Value::as_array)
                .map(|l| {
                    l.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let all = names.is_empty();
            let on = |n: &str| all && n != "geometry" || names.iter().any(|x| x == n);
            g.white_balance = on("whiteBalance");
            g.light = on("light");
            g.color = on("color");
            g.hsl = on("hsl");
            g.color_grade = on("colorGrade");
            g.details = on("details");
            g.masks = on("masks");
            g.geometry = on("geometry");
            crate::develop::develop_transfer(state.clone(), to.clone(), src, g).map_err(e)?;
            changed(app, &to);
            text(json!({ "copiedTo": to.len() }))
        }
        "add_mask" => add_mask(app, a).await,
        "export_photos" => {
            let ids = ids(a, "ids")?;
            let format = match arg(a, "format").and_then(Value::as_str).unwrap_or("jpeg") {
                "png" => "png",
                "tiff" | "tif" => "tiff16",
                _ => "jpeg",
            };
            let options: lumenkorn_develop::output::ExportOptions = serde_json::from_value(json!({
                "format": format,
                "quality": arg(a, "quality").and_then(Value::as_u64).unwrap_or(90).clamp(1, 100),
                "longEdge": arg(a, "long_edge").and_then(Value::as_u64),
            }))
            .map_err(|x| x.to_string())?;
            let dir = arg(a, "folder").and_then(Value::as_str).map(str::to_string);
            crate::develop::export_images(app.clone(), ids.clone(), dir.clone(), options)
                .map_err(e)?;
            text(json!({
                "exporting": ids.len(),
                "to": dir.unwrap_or_else(|| "\"Export\" subfolder next to each original".into()),
                "note": "Runs in the background; progress is shown in the app.",
            }))
        }
        "show_photo" => {
            let id = int(a, "id")?;
            state.catalog.locked().thumb_job(id).map_err(e)?;
            let develop = arg(a, "develop").and_then(Value::as_bool).unwrap_or(false);
            let _ = app.emit("mcp-show", json!({ "id": id, "develop": develop }));
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
            text(json!({ "shown": id }))
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

fn flatten(n: &lumenkorn_core::model::FolderNode, depth: usize, out: &mut Vec<Value>, max: usize) {
    if out.len() >= max {
        return;
    }
    out.push(
        json!({ "path": n.path, "photos": n.count, "withSubfolders": n.total, "depth": depth }),
    );
    for c in &n.children {
        flatten(c, depth + 1, out, max);
    }
}

/// JSON Merge Patch (RFC 7396): Objekte zusammenführen, alles andere ersetzen
fn merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(t), Value::Object(p)) => {
            for (k, v) in p {
                if v.is_null() && k != "crop" && k != "denoise" {
                    t.remove(k);
                } else if let Some(slot) = t.get_mut(k)
                    && slot.is_object()
                    && v.is_object()
                {
                    merge(slot, v);
                } else {
                    t.insert(k.clone(), v.clone());
                }
            }
        }
        (t, p) => *t = p.clone(),
    }
}

/// Nur die Einstellungen, die vom Standard abweichen (Masken als Anzahl)
fn non_default(p: &DevelopParams) -> Value {
    let cur = serde_json::to_value(p).unwrap_or_default();
    let def = serde_json::to_value(DevelopParams::default()).unwrap_or_default();
    let mut out = serde_json::Map::new();
    if let (Some(c), Some(d)) = (cur.as_object(), def.as_object()) {
        for (k, v) in c {
            if d.get(k) == Some(v) || k == "version" {
                continue;
            }
            let shown = match k.as_str() {
                "masks" | "repairs" => json!(v.as_array().map_or(0, Vec::len)),
                _ => v.clone(),
            };
            out.insert(k.clone(), shown);
        }
    }
    Value::Object(out)
}

async fn search(app: &AppHandle, a: &Value) -> ToolResult {
    let state = app.state::<AppState>();
    let scope = if let Some(f) = arg(a, "folder").and_then(Value::as_str) {
        Scope::Folder {
            path: f.to_string(),
            recursive: arg(a, "include_subfolders")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        }
    } else if let Some(id) = arg(a, "project_id").and_then(Value::as_i64) {
        Scope::Project { id }
    } else if let Some(id) = arg(a, "person_id").and_then(Value::as_i64) {
        Scope::Person { id }
    } else {
        Scope::All
    };
    let min_rating = arg(a, "min_rating")
        .and_then(Value::as_i64)
        .filter(|r| *r > 0);
    let text_q = arg(a, "text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let by_content = arg(a, "by_content")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut list = if let (Some(q), true) = (text_q, by_content) {
        crate::ai::semantic_search(app.clone(), q.to_string(), scope, min_rating)
            .await
            .map_err(e)?
    } else {
        let mut q = ImageQuery::new(scope);
        q.sort = match arg(a, "sort").and_then(Value::as_str) {
            Some("newest") => SortOrder::CapturedDesc,
            Some("name") => SortOrder::NameAsc,
            Some("rating") => SortOrder::RatingDesc,
            _ => SortOrder::CapturedAsc,
        };
        q.search = text_q.map(str::to_string);
        q.min_rating = min_rating;
        crate::commands::query_images(state.clone(), q).map_err(e)?
    };
    if let Some(f) = arg(a, "flag").and_then(Value::as_str) {
        let want = match f {
            "pick" => 1,
            "rejected" => -1,
            _ => 0,
        };
        list.retain(|i| i.flag == want);
    }
    if let Some(ed) = arg(a, "edited").and_then(Value::as_bool) {
        list.retain(|i| i.edited == ed);
    }
    let total = list.len();
    let offset = arg(a, "offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let limit = arg(a, "limit")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .clamp(1, 500) as usize;
    let page: Vec<Value> = list
        .iter()
        .skip(offset)
        .take(limit)
        .map(|i| {
            json!({
                "id": i.id, "file": i.file_name, "path": i.path, "date": i.captured_at,
                "rating": i.rating, "flag": i.flag, "edited": i.edited,
                "size": i.width.zip(i.height).map(|(w, h)| format!("{w}×{h}")),
            })
        })
        .collect();
    text(json!({ "total": total, "offset": offset, "photos": page }))
}

async fn add_mask(app: &AppHandle, a: &Value) -> ToolResult {
    use lumenkorn_develop::params::{LocalAdjust, MaskLayer, MaskShape};
    let state = app.state::<AppState>();
    let id = int(a, "id")?;
    let kind = arg(a, "kind")
        .and_then(Value::as_str)
        .ok_or("`kind` is required")?;
    let (stored_kind, result) = match kind {
        "subject" => (
            "subject".to_string(),
            crate::ai::mask_subject(app.clone(), id).await,
        ),
        "sky" => (
            "sky".to_string(),
            crate::ai::mask_sky(app.clone(), id).await,
        ),
        k if k == "person" || k.starts_with("person:") => {
            let part = k.strip_prefix("person:").unwrap_or("person");
            let p: lumenkorn_ai::parts::PersonPart = serde_json::from_value(json!(part))
                .map_err(|_| format!("unknown person part: {part}"))?;
            (
                format!("person:{part}"),
                crate::ai::mask_person(app.clone(), id, p).await,
            )
        }
        k if k == "animal" || k.starts_with("animal:") => {
            let which = k.strip_prefix("animal:").unwrap_or("all");
            let p: lumenkorn_ai::parts::AnimalKind = serde_json::from_value(json!(which))
                .map_err(|_| format!("unknown animal kind: {which}"))?;
            (
                format!("animal:{which}"),
                crate::ai::mask_animal(app.clone(), id, p).await,
            )
        }
        other => return Err(format!("unknown mask kind: {other}")),
    };
    let r = result.map_err(e)?;
    if r.coverage <= 0.0 {
        return Err(format!(
            "Nothing found for `{kind}` in photo {id}; no mask added."
        ));
    }
    let adjust: LocalAdjust =
        serde_json::from_value(arg(a, "adjust").cloned().unwrap_or(json!({})))
            .map_err(|x| format!("adjust: {x}"))?;
    let mut p = crate::develop::develop_params(state.clone(), id).map_err(e)?;
    let name = arg(a, "name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("KI: {kind}"));
    p.masks.push(MaskLayer {
        id: format!(
            "m{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis())
        ),
        name,
        enabled: true,
        shapes: vec![MaskShape::Raster {
            mask_id: r.mask_id,
            kind: stored_kind,
            prompt: vec![],
            invert: arg(a, "invert").and_then(Value::as_bool).unwrap_or(false),
            subtract: false,
        }],
        adjust,
    });
    crate::develop::develop_save(app.clone(), state.clone(), vec![id], p).map_err(e)?;
    changed(app, &[id]);
    text(json!({ "added": kind, "coverage": (r.coverage * 1000.0).round() / 1000.0 }))
}

// --- stdio-Brücke ---------------------------------------------------------------

/// `lumenkorn --mcp`: reicht JSON-RPC-Zeilen von stdin an die laufende App
/// weiter und deren Antworten an stdout. Startet Lumenkorn, falls es nicht
/// läuft. Kehrt zurück, wenn stdin endet.
pub fn run_stdio() {
    use std::io::{BufRead, Write};
    let Some(data) = dirs::data_dir().map(|d| d.join(IDENTIFIER)) else {
        eprintln!("Lumenkorn: no data folder");
        return;
    };
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(600)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut launched = false;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = forward(&agent, &data, &line, &mut launched);
        match reply {
            Ok(Some(body)) => {
                let _ = writeln!(out, "{}", body.trim());
                let _ = out.flush();
            }
            Ok(None) => {}
            Err(msg) => {
                // Fehler als JSON-RPC-Antwort, damit der Agent ihn anzeigt
                if let Ok(v) = serde_json::from_str::<Value>(&line)
                    && let Some(id) = v.get("id").filter(|i| !i.is_null())
                {
                    let err = json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": msg } });
                    let _ = writeln!(out, "{err}");
                    let _ = out.flush();
                } else {
                    eprintln!("Lumenkorn: {msg}");
                }
            }
        }
    }
}

fn forward(
    agent: &ureq::Agent,
    data: &Path,
    line: &str,
    launched: &mut bool,
) -> Result<Option<String>, String> {
    for attempt in 0..60 {
        let cfg = load(data);
        // Ausgeschaltet: nichts starten (die App kann schon laufen)
        if !cfg.enabled {
            return Err(
                "The MCP interface is switched off. Turn it on in Lumenkorn: AI (bottom left) → AI agents (MCP)."
                    .into(),
            );
        }
        if cfg.port > 0 {
            let res = agent
                .post(&format!("http://127.0.0.1:{}/mcp", cfg.port))
                .header("Authorization", &format!("Bearer {}", cfg.token))
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .send(line);
            // Fehler heißt: nicht erreichbar, Lumenkorn läuft nicht (mehr) – starten
            if let Ok(mut r) = res {
                {
                    let status = r.status().as_u16();
                    let body = r.body_mut().read_to_string().unwrap_or_default();
                    return match status {
                        200 => Ok(Some(body)),
                        202 => Ok(None),
                        401 => {
                            // Schlüssel geändert: gleich noch einmal mit dem neuen
                            if attempt == 0 {
                                continue;
                            }
                            Err("Lumenkorn rejected the access key.".into())
                        }
                        s => Err(format!("Lumenkorn answered HTTP {s}: {body}")),
                    };
                }
            }
        }
        if !*launched {
            *launched = true;
            if let Ok(me) = std::env::current_exe() {
                let _ = std::process::Command::new(me)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err("Lumenkorn is not reachable. Start Lumenkorn and turn on AI → AI agents (MCP).".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_local_origins() {
        for ok in ["null", "http://localhost", "http://127.0.0.1:5173"] {
            assert!(local_origin(ok), "{ok}");
        }
        for bad in [
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com:80",
            "http://localhost:80@evil.com",
            "https://evil.com",
        ] {
            assert!(!local_origin(bad), "{bad}");
        }
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
