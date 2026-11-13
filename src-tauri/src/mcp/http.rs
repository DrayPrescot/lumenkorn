//! HTTP-Seite: nur lokale Hosts und Origins, Schlüssel prüfen, JSON-RPC-Nachrichten
//! (initialize, tools/list, tools/call) beantworten.

use super::*;

/// `Host` nur 127.0.0.1, localhost oder [::1] mit dem gebundenen Port
/// (zweite Verteidigung gegen DNS-Rebinding).
pub(super) fn local_host(host: &str, port: u16) -> bool {
    let host = host.trim().to_ascii_lowercase();
    ["127.0.0.1", "localhost", "[::1]"]
        .iter()
        .any(|h| host == format!("{h}:{port}"))
}

/// Origin einer lokalen Seite: genau 127.0.0.1 bzw. localhost, mit oder ohne
/// Port – nicht etwa `http://localhost.example.com`. `Origin: null` (Datei,
/// Sandbox-iframe) kann jede fremde Seite erzeugen und gilt nicht als lokal.
pub(super) fn local_origin(origin: &str) -> bool {
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

/// Größte angenommene Anfrage (Bytes)
pub(super) const MAX_BODY: usize = 4 << 20;

pub(super) fn handle_http(app: &AppHandle, token: &str, port: u16, mut req: tiny_http::Request) {
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
    // Host muss auf uns selbst zeigen (DNS-Rebinding)
    if let Some(host) = header("Host")
        && !local_host(&host, port)
    {
        return reply(req, 403, r#"{"error":"host not allowed"}"#.into());
    }
    // Nur lokale Webseiten dürfen (Schutz vor DNS-Rebinding)
    if let Some(origin) = header("Origin")
        && !local_origin(&origin)
    {
        return reply(req, 403, r#"{"error":"origin not allowed"}"#.into());
    }
    // Ein leerer Schlüssel (keine Zufallsquelle) lässt nie jemanden herein
    let auth_ok = !token.is_empty()
        && header("Authorization").is_some_and(|a| {
            a.strip_prefix("Bearer ")
                .is_some_and(|t| same_token(t.trim(), token))
        });
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
    let limit = MAX_BODY as u64 + 1;
    if std::io::Read::read_to_string(&mut std::io::Read::take(req.as_reader(), limit), &mut body)
        .is_err()
    {
        return reply(req, 400, String::new());
    }
    if body.len() > MAX_BODY {
        return reply(req, 413, r#"{"error":"request too large"}"#.into());
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
pub(super) type Caller<'a> = &'a dyn Fn(&str, &Value) -> ToolResult;

pub(super) fn handle_body_with(body: &str, call: Caller) -> Option<String> {
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

pub(super) fn handle_msg(msg: Value, call: Caller) -> Option<Value> {
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

pub(super) const INSTRUCTIONS: &str = "Lumenkorn is a local photo library and RAW developer. \
Find photos with search_photos (text, folder, project, person, rating), look at them with \
view_photo, then rate, flag, tag, develop (set_develop_settings, auto_tone, apply_preset, \
add_mask) or export them. Photo ids come from search_photos. Develop values use the same \
ranges as the app's sliders (most −100…100, exposure −5…5 EV). Changes show up in the \
app immediately. Before develop settings change, the previous ones are kept as a snapshot \
that the user can restore in the app. Nothing here deletes files.";
