// Verhindert unter Windows ein zusätzliches Konsolenfenster im Release-Build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Native Bibliotheken (ONNX Runtime, Grafiktreiber) brauchen beim Laden
    // großer Modelle teils viel Stapelspeicher. Windows gibt neuen Threads
    // weniger als Linux – daher für alle Rust-Threads (auch Tokio, Rayon)
    // großzügig vorgeben. Muss vor dem ersten Thread geschehen.
    if std::env::var_os("RUST_MIN_STACK").is_none() {
        // SAFETY: Noch läuft nur dieser eine Thread.
        unsafe { std::env::set_var("RUST_MIN_STACK", (16usize << 20).to_string()) };
    }
    // KI-Agenten starten Lumenkorn als MCP-Server über stdin/stdout
    if std::env::args().any(|a| a == "--mcp") {
        return lumenkorn_lib::run_mcp_stdio();
    }
    lumenkorn_lib::run()
}
