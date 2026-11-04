//! Beenden ohne Hängenbleiben.
//!
//! Unter Windows beendet `ExitProcess` erst alle anderen Threads und lässt dann
//! die geladenen Bibliotheken aufräumen. Dabei blieb Lumenkorn nach
//! „Lumenkorn beendet“ mit einem einzigen Thread hängen (ONNX Runtime mit
//! WebGPU-Sitzung, Radeon RX 7900 XTX) – und weil nur eine Instanz laufen darf,
//! hätte ein neuer Start dann nur den hängenden Prozess geweckt. Deshalb endet
//! der Prozess nach dem eigenen Aufräumen sofort. Daten gehen dabei nicht
//! verloren: Der Katalog schreibt jede Transaktion vollständig (WAL),
//! Einstellungen werden atomar gespeichert.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// Ein Neustart ist angefordert: beim Beenden die App neu starten.
static RESTART: AtomicBool = AtomicBool::new(false);

/// Startet Lumenkorn neu (über das normale Beenden, siehe [`finish`]).
pub fn restart(app: &AppHandle) -> ! {
    RESTART.store(true, Ordering::SeqCst);
    app.restart()
}

/// Läuft gerade etwas, das Dateien nicht halb fertig zurücklassen darf?
static CRITICAL: AtomicUsize = AtomicUsize::new(0);
/// So lange wartet das Beenden darauf höchstens
const CRITICAL_WAIT: Duration = Duration::from_secs(30);

/// Solange der Wert lebt, wartet das Beenden (z. B. Umbenennen: Mittendrin
/// abgebrochen, lägen Fotos unter Zwischennamen).
pub struct Critical;

pub fn critical() -> Critical {
    CRITICAL.fetch_add(1, Ordering::SeqCst);
    Critical
}

impl Drop for Critical {
    fn drop(&mut self) {
        CRITICAL.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Letzter Schritt bei `RunEvent::Exit`, nach dem eigenen Aufräumen.
pub fn finish(app: &AppHandle) {
    let start = Instant::now();
    while CRITICAL.load(Ordering::SeqCst) > 0 && start.elapsed() < CRITICAL_WAIT {
        std::thread::sleep(Duration::from_millis(20));
    }
    log::logger().flush();
    #[cfg(windows)]
    {
        // Was Tauri sonst nach diesem Ereignis erledigt (u. a. gibt das
        // Single-Instance-Plugin seine Sperre frei, damit der Neustart angeht)
        app.cleanup_before_exit();
        if RESTART.load(Ordering::SeqCst)
            && let Ok(exe) = std::env::current_exe()
            && let Err(e) = std::process::Command::new(exe)
                .args(std::env::args_os().skip(1))
                .spawn()
        {
            log::warn!("Neustart fehlgeschlagen: {e}");
            log::logger().flush();
        }
        terminate();
    }
    #[cfg(not(windows))]
    let _ = app;
}

#[cfg(windows)]
fn terminate() -> ! {
    use std::ffi::c_void;
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn TerminateProcess(process: *mut c_void, exit_code: u32) -> i32;
    }
    // SAFETY: Pseudo-Handle des eigenen Prozesses, immer gültig
    unsafe {
        TerminateProcess(GetCurrentProcess(), 0);
    }
    // Nur falls TerminateProcess scheitert
    std::process::exit(0)
}
