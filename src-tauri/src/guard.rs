//! Absturzschutz für riskante native Schritte (ONNX Runtime, DirectML,
//! Grafiktreiber).
//!
//! Solche Bibliotheken können den Prozess ohne Rust-Fehler beenden
//! (Zugriffsverletzung, Abbruch im Treiber). Vor dem Schritt wird eine
//! Markierungsdatei geschrieben und danach wieder gelöscht. Liegt sie beim
//! nächsten Start noch da, ist Lumenkorn genau dabei abgestürzt – dann wird
//! dieser Schritt nicht automatisch wiederholt, sondern abgeschaltet und
//! gemeldet. So startet die App in jedem Fall wieder.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    /// ONNX Runtime laden
    KiLaufzeit,
    /// Ein KI-Modell öffnen
    KiModell,
    /// Bilder mit der KI analysieren
    KiAnalyse,
    /// Grafikkarte für die Bildbearbeitung öffnen
    GpuBearbeitung,
}

impl Stage {
    fn file(self) -> &'static str {
        match self {
            Stage::KiLaufzeit => "absturz-ki-laufzeit.json",
            Stage::KiModell => "absturz-ki-modell.json",
            Stage::KiAnalyse => "absturz-ki-analyse.json",
            Stage::GpuBearbeitung => "absturz-gpu-bearbeitung.json",
        }
    }

    const ALL: [Stage; 4] = [
        Stage::KiLaufzeit,
        Stage::KiModell,
        Stage::KiAnalyse,
        Stage::GpuBearbeitung,
    ];
}

/// Was beim letzten Start gerade lief, als Lumenkorn abstürzte.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Crash {
    pub stage: Stage,
    /// z. B. „directml“, „webgpu“, „cpu“
    pub backend: Option<String>,
    /// Lesbare Beschreibung, z. B. Pfad der Bibliothek oder Modellname
    pub detail: String,
}

pub struct Guard {
    path: PathBuf,
}

impl Guard {
    /// Markiert den Beginn eines riskanten Schritts. Endet er (auch mit einem
    /// Fehler), wird die Markierung beim Verlassen des Gültigkeitsbereichs gelöscht.
    pub fn begin(dir: &Path, stage: Stage, backend: Option<&str>, detail: &str) -> Guard {
        let path = dir.join(stage.file());
        let crash = Crash {
            stage,
            backend: backend.map(str::to_string),
            detail: detail.to_string(),
        };
        if let Ok(json) = serde_json::to_vec(&crash) {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(&path, json);
        }
        // Nur für Tests: Absturz an genau dieser Stelle simulieren
        if std::env::var("LUMENKORN_TEST_CRASH").is_ok_and(|v| v == stage.file()) {
            log::error!("Simulierter Absturz: {}", stage.file());
            std::process::abort();
        }
        Guard { path }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Liest und entfernt Markierungen des letzten Starts.
pub fn take_previous(dir: &Path) -> Vec<Crash> {
    let mut out = Vec::new();
    for stage in Stage::ALL {
        let path = dir.join(stage.file());
        if let Ok(bytes) = std::fs::read(&path) {
            let crash = serde_json::from_slice(&bytes).unwrap_or(Crash {
                stage,
                backend: None,
                detail: String::new(),
            });
            log::error!("Beim letzten Start abgestürzt: {crash:?}");
            out.push(crash);
            let _ = std::fs::remove_file(&path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_survives_only_a_crash() {
        let dir = std::env::temp_dir().join(format!("lw-guard-{}", std::process::id()));
        {
            let _g = Guard::begin(&dir, Stage::KiModell, Some("cpu"), "vision.onnx");
            assert!(dir.join(Stage::KiModell.file()).is_file());
        }
        assert!(
            take_previous(&dir).is_empty(),
            "sauber beendet → keine Markierung"
        );

        let g = Guard::begin(&dir, Stage::KiLaufzeit, Some("directml"), "onnxruntime.dll");
        std::mem::forget(g); // wie ein Absturz: Drop läuft nie
        let prev = take_previous(&dir);
        assert_eq!(prev.len(), 1);
        assert_eq!(prev[0].stage, Stage::KiLaufzeit);
        assert_eq!(prev[0].backend.as_deref(), Some("directml"));
        assert!(take_previous(&dir).is_empty(), "nur einmal gemeldet");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
