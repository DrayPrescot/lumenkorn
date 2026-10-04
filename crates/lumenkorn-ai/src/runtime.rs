//! Dynamisches Laden von ONNX Runtime.

use crate::accel::{self, Backend, Preference};
use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static LOADED: OnceLock<std::result::Result<(PathBuf, Backend), String>> = OnceLock::new();

/// Dateiname der Laufzeitbibliothek auf dieser Plattform.
pub fn library_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    }
}

/// Sucht eine Bibliothek (GPU-Variante bevorzugt), siehe [`accel::choose_library`].
pub fn find_library(app_dir: &Path) -> Option<PathBuf> {
    accel::choose_library(app_dir, Preference::Auto).map(|(p, _)| p)
}

/// Lädt eine CPU-Laufzeit (bzw. eine unbekannte Bibliothek ohne GPU-Nutzung).
pub fn init(lib: &Path) -> Result<()> {
    init_with(lib, Backend::Cpu)
}

/// Lädt ONNX Runtime genau einmal pro Prozess. Weitere Aufrufe liefern das
/// Ergebnis des ersten – eine einmal geladene Bibliothek lässt sich nicht tauschen.
pub fn init_with(lib: &Path, backend: Backend) -> Result<()> {
    let res = LOADED.get_or_init(|| {
        #[cfg(windows)]
        preload_companions(lib);
        let builder = ort::init_from(lib).map_err(|e| {
            if cfg!(windows) {
                lumenkorn_develop::tr!(
                    "{e} – Ist das Microsoft Visual C++ Redistributable (x64) installiert? \
                     https://aka.ms/vs/17/release/vc_redist.x64.exe",
                    "{e} – Is the Microsoft Visual C++ Redistributable (x64) installed? \
                     https://aka.ms/vs/17/release/vc_redist.x64.exe"
                )
            } else {
                e.to_string()
            }
        })?;
        builder.with_name("lumenkorn").commit();
        accel::set_library(backend);
        log::info!(
            "ONNX Runtime geladen ({}): {}",
            backend.label(),
            lib.display()
        );
        Ok((lib.to_path_buf(), backend))
    });
    res.as_ref().map(|_| ()).map_err(|e| Error::Ort(e.clone()))
}

pub fn is_loaded() -> bool {
    matches!(LOADED.get(), Some(Ok(_)))
}

/// Pfad und Variante der geladenen Bibliothek.
pub fn loaded() -> Option<(PathBuf, Backend)> {
    LOADED.get().and_then(|r| r.as_ref().ok().cloned())
}

/// Windows sucht abhängige DLLs nicht im Ordner der nachgeladenen Bibliothek.
/// Ist eine DLL gleichen Namens aber schon geladen, wird diese verwendet –
/// daher die Begleiter vorab mit vollem Pfad laden und geladen lassen.
#[cfg(windows)]
fn preload_companions(lib: &Path) {
    let Some(dir) = lib.parent() else { return };
    // Reihenfolge: Abhängigkeiten vor ihren Nutzern
    for name in [
        "DirectML.dll",
        "dxil.dll",
        "dxcompiler.dll",
        "onnxruntime_providers_shared.dll",
    ] {
        let p = dir.join(name);
        if p.is_file() {
            match unsafe { libloading::Library::new(&p) } {
                Ok(l) => std::mem::forget(l),
                Err(e) => log::warn!("{} nicht geladen: {e}", p.display()),
            }
        }
    }
}
