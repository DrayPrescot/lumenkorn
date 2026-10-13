//! Protokoll in eine Datei (`<daten>/lumenkorn.log`) und auf stderr.
//!
//! Unter Windows hat die Release-App kein Konsolenfenster – ohne Datei gingen
//! Fehlermeldungen verloren. Rust-Paniken landen mit Rückverfolgung ebenfalls
//! im Protokoll.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static FILE: OnceLock<Mutex<File>> = OnceLock::new();
static PATH: OnceLock<PathBuf> = OnceLock::new();
const MAX_SIZE: u64 = 5 << 20;

pub fn path() -> Option<&'static Path> {
    PATH.get().map(PathBuf::as_path)
}

struct Tee;

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        if let Some(f) = FILE.get() {
            let mut f = f.lock().unwrap_or_else(|e| e.into_inner());
            let _ = f.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(f) = FILE.get() {
            let _ = f.lock().unwrap_or_else(|e| e.into_inner()).flush();
        }
        Ok(())
    }
}

/// Logger und Panik-Aufzeichnung einrichten (so früh wie möglich).
pub fn init() {
    // Ohne Belang und bei jedem Start bzw. RAW wiederholt: Vulkan-Loader
    // (Registry-Einträge für Layer) und rawlers TIFF-Leser (fehlerhafte
    // Hersteller-Tags, die er selbst übergeht – mit Backtrace).
    env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("info,wgpu_hal::vulkan::instance=error,rawler::formats::tiff=off"),
    )
    .target(env_logger::Target::Pipe(Box::new(Tee)))
    .init();

    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current()
            .name()
            .unwrap_or("unbenannt")
            .to_string();
        let bt = std::backtrace::Backtrace::force_capture();
        log::error!("Rust-Panik im Thread „{thread}“: {info}\n{bt}");
        default(info);
    }));
}

/// Ab jetzt auch in die Datei schreiben. Große Protokolle werden einmal
/// rotiert (`lumenkorn.log.1`).
pub fn open(data_dir: &Path) {
    let path = data_dir.join("lumenkorn.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_SIZE) {
        let _ = std::fs::rename(&path, data_dir.join("lumenkorn.log.1"));
    }
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => {
            let _ = FILE.set(Mutex::new(f));
            let _ = PATH.set(path);
            log::info!(
                "Lumenkorn {} startet ({} {})",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH
            );
        }
        Err(e) => log::warn!("Protokolldatei {} nicht nutzbar: {e}", path.display()),
    }
}
