//! stdio-Brücke für Agenten, die Server als Programm starten (`lumenkorn --mcp`).

use super::*;

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

/// Die Verbindung kam gar nicht zustande (die Anfrage wurde nie gesendet).
pub(super) fn not_reached(e: &ureq::Error) -> bool {
    use std::io::ErrorKind;
    match e {
        ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => true,
        ureq::Error::Io(io) => matches!(
            io.kind(),
            ErrorKind::ConnectionRefused | ErrorKind::AddrNotAvailable | ErrorKind::NotConnected
        ),
        _ => false,
    }
}

pub(super) fn forward(
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
            match res {
                Ok(mut r) => {
                    let status = r.status().as_u16();
                    let body = match r.body_mut().read_to_string() {
                        Ok(b) => b,
                        // Antwort abgebrochen: der Aufruf lief womöglich schon,
                        // also nicht noch einmal senden
                        Err(e) => return Err(format!("Lumenkorn: reading the answer failed: {e}")),
                    };
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
                // Nicht erreichbar: Lumenkorn läuft nicht (mehr) – starten, warten
                Err(e) if not_reached(&e) => {}
                // Zeitüberschreitung oder Abbruch mitten im Aufruf: nicht erneut
                // senden (ein Werkzeug könnte doppelt laufen)
                Err(e) => return Err(format!("Lumenkorn: request failed: {e}")),
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
