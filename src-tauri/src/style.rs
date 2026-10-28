//! „Auto nach deinem Stil“: Auto lernt aus den Bearbeitungen des Nutzers.
//!
//! Für jedes bearbeitete Foto entsteht im Hintergrund ein Lernbeispiel: was
//! Auto vorgeschlagen hätte, was der Nutzer eingestellt hat, und ein paar
//! Bildmerkmale. Gelernt wird nur die Abweichung vom Auto-Vorschlag (siehe
//! `lumenkorn_develop::style`); Auto bleibt also unverändert, solange es nichts
//! Belastbares zu lernen gibt. Die Beispiele liegen in `stil.json` im
//! Datenordner – keine Änderung am Katalog, nichts verlässt den Rechner.

use crate::AppState;
use lumenkorn_core::formats::ImageKind;
use lumenkorn_core::{Error, LockExt};
use lumenkorn_develop::auto::{AutoTone, auto_tone_with_features};
use lumenkorn_develop::fsutil::unix_now;
use lumenkorn_develop::style::{Query, Sample, StyleModel, Summary};
use lumenkorn_develop::{DevelopParams, Source};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, State};

const FILE: &str = "stil.json";
/// Nach einer Bearbeitung so lange warten, bevor Beispiele entstehen – beim
/// Bearbeiten wird laufend gespeichert, und das Foto ist gerade in Arbeit.
const SETTLE: Duration = Duration::from_secs(20);
/// Erster Abgleich nach dem Start
const FIRST: Duration = Duration::from_secs(60);
/// Danach regelmäßig (fängt Änderungen ab, die nicht über das Speichern im
/// Entwickeln kamen: Einstellungen übertragen, Vorgaben, KI-Agenten)
const EVERY: Duration = Duration::from_secs(30 * 60);
/// Kantenlänge der Quelle für ein Lernbeispiel (Auto misst auf 320 px)
const EDGE: usize = 1024;

/// Format von `stil.json`
const VERSION: u32 = 1;

/// Zum Schreiben, ohne die Beispiele zu kopieren
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StyleFileRef<'a> {
    version: u32,
    reset_at: i64,
    samples: &'a [Sample],
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct StyleFile {
    version: u32,
    /// Bearbeitungen vor diesem Zeitpunkt zählen nicht („zurücksetzen“)
    reset_at: i64,
    samples: Vec<Sample>,
}

pub struct StyleState {
    path: PathBuf,
    file: Mutex<StyleFile>,
    model: Mutex<Option<Arc<StyleModel>>>,
    /// Nächster Abgleich spätestens dann; `notify` weckt den Hintergrund-Thread
    due: Mutex<Instant>,
    wake: Condvar,
}

impl StyleState {
    pub fn new(data_dir: &Path) -> Self {
        let path = data_dir.join(FILE);
        let file: StyleFile = lumenkorn_develop::fsutil::load_json_or_default(&path);
        Self {
            path,
            file: Mutex::new(file),
            model: Mutex::new(None),
            due: Mutex::new(Instant::now() + FIRST),
            wake: Condvar::new(),
        }
    }

    fn save(&self, f: &StyleFile) {
        let res = lumenkorn_develop::fsutil::save_json(
            &self.path,
            &StyleFileRef {
                version: VERSION,
                reset_at: f.reset_at,
                samples: &f.samples,
            },
        );
        if let Err(e) = res {
            log::warn!("Stil speichern: {e}");
        }
    }
}

fn enabled(state: &AppState) -> bool {
    state.ai.settings.locked().learn_style
}

/// Nach dem Speichern einer Bearbeitung: Beispiele bald (nach einer Pause)
/// nachführen.
pub fn nudge(state: &AppState) {
    *state.style.due.locked() = Instant::now() + SETTLE;
    state.style.wake.notify_all();
}

/// Hintergrund-Thread: Lernbeispiele mit den Bearbeitungen abgleichen.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("lumenkorn-style".into())
        .spawn(move || {
            let state = app.state::<AppState>();
            loop {
                {
                    let st = &state.style;
                    let mut due = st.due.locked();
                    loop {
                        let now = Instant::now();
                        if *due <= now {
                            break;
                        }
                        let wait = *due - now;
                        due = st
                            .wake
                            .wait_timeout(due, wait)
                            .unwrap_or_else(|e| e.into_inner())
                            .0;
                    }
                    *due = Instant::now() + EVERY;
                }
                if enabled(&state)
                    && let Err(e) = reconcile(&state)
                {
                    log::warn!("Stil lernen: {e}");
                }
            }
        });
}

/// Gruppe eines Fotos: Kamera, getrennt nach RAW und JPEG
fn group(camera: Option<&str>, raw: bool) -> String {
    let kind = if raw { "RAW" } else { "JPEG" };
    match camera.map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => format!("{c} ({kind})"),
        None => kind.to_string(),
    }
}

fn tone_of(p: &DevelopParams) -> [f32; 7] {
    [
        p.exposure,
        p.contrast,
        p.highlights,
        p.shadows,
        p.whites,
        p.blacks,
        p.vibrance,
    ]
}

/// Ein Lernbeispiel berechnen: Auto auf dem Foto mit den übrigen
/// Einstellungen des Nutzers (Zuschnitt, Farbe …), dazu seine Endwerte.
fn sample(
    state: &AppState,
    id: i64,
    stamp: i64,
    camera: Option<&str>,
    raw: bool,
) -> Result<Sample, Error> {
    let (job, params) = {
        let cat = state.catalog.locked();
        (cat.thumb_job(id)?, cat.develop_params(id)?)
    };
    let src = Source::load(
        Path::new(&job.path),
        job.kind == ImageKind::Raw,
        job.orientation.map(|o| o as u16),
        Some(EDGE),
    )?;
    let (auto, features) = auto_tone_with_features(&src, &params);
    let user = tone_of(&params);
    Ok(Sample {
        image_id: id,
        group: group(camera, raw),
        stamp,
        auto: [
            auto.exposure,
            auto.contrast,
            auto.highlights,
            auto.shadows,
            auto.whites,
            auto.blacks,
            auto.vibrance,
        ],
        user,
        tone_edited: user.iter().any(|v| v.abs() > 1e-4),
        extra: [
            params.saturation,
            params.clarity,
            params.dehaze,
            params.sharpen,
        ],
        features,
    })
}

/// Beispiele nachführen: neue und geänderte Bearbeitungen auswerten, nicht
/// mehr vorhandene entfernen.
fn reconcile(state: &AppState) -> Result<(), Error> {
    let overview = state.catalog.locked().edited_overview()?;
    let reset_at = state.style.file.locked().reset_at;
    let current: HashMap<i64, (i64, Option<String>, bool)> = overview
        .into_iter()
        .filter(|(_, stamp, _, _)| *stamp >= reset_at)
        .map(|(id, stamp, cam, raw)| (id, (stamp, cam, raw)))
        .collect();
    let mut todo: Vec<(i64, i64)> = {
        let mut f = state.style.file.locked();
        let before = f.samples.len();
        f.samples
            .retain(|s| current.get(&s.image_id).is_some_and(|c| c.0 == s.stamp));
        let removed = before != f.samples.len();
        let have: std::collections::HashSet<i64> = f.samples.iter().map(|s| s.image_id).collect();
        if removed {
            state.style.save(&f);
            *state.style.model.locked() = None;
        }
        current
            .iter()
            .filter(|(id, _)| !have.contains(id))
            .map(|(id, c)| (*id, c.0))
            .collect()
    };
    if todo.is_empty() {
        return Ok(());
    }
    // Neueste zuerst: Sie sagen am meisten über den heutigen Stil
    todo.sort_by_key(|t| std::cmp::Reverse(t.1));
    let t0 = Instant::now();
    let mut added = 0usize;
    for (id, stamp) in todo {
        if !enabled(state) {
            break;
        }
        // Inzwischen wieder gespeichert (Anstoß setzt den Termin nah statt in
        // EVERY): Es wird gerade bearbeitet – später weiter
        if *state.style.due.locked() < Instant::now() + EVERY / 2 {
            break;
        }
        let (cam, raw) = {
            let c = &current[&id];
            (c.1.clone(), c.2)
        };
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sample(state, id, stamp, cam.as_deref(), raw)
        }));
        match res {
            Ok(Ok(s)) => {
                let mut f = state.style.file.locked();
                f.samples.retain(|x| x.image_id != id);
                f.samples.push(s);
                added += 1;
                if added.is_multiple_of(10) {
                    state.style.save(&f);
                }
            }
            Ok(Err(e)) => log::debug!("Stil: Foto {id} übersprungen: {e}"),
            Err(_) => log::warn!("Stil: Foto {id} übersprungen (Panik)"),
        }
        // Rechner nicht auslasten
        std::thread::sleep(Duration::from_millis(150));
    }
    if added > 0 {
        let f = state.style.file.locked();
        state.style.save(&f);
        *state.style.model.locked() = None;
        log::info!(
            "Stil: {added} Lernbeispiele in {:.1?} ermittelt, {} insgesamt",
            t0.elapsed(),
            f.samples.len()
        );
    }
    Ok(())
}

/// Gelerntes Modell (zwischengespeichert, nach neuen Beispielen neu gelernt)
fn model(state: &AppState) -> Arc<StyleModel> {
    if let Some(m) = state.style.model.locked().clone() {
        return m;
    }
    let samples = state.style.file.locked().samples.clone();
    // CLIP-Merkmale nur, wenn Stufe 4 überhaupt in Frage kommt
    let tone = samples.iter().filter(|s| s.tone_edited).count();
    let embeddings = if tone >= 300 {
        let ids: Vec<i64> = samples.iter().map(|s| s.image_id).collect();
        crate::ai::embeddings_for(state, &ids)
    } else {
        HashMap::new()
    };
    let t = Instant::now();
    let m = Arc::new(StyleModel::learn(&samples, &embeddings, unix_now()));
    log::info!(
        "Stil gelernt aus {} Beispielen in {:.1?}",
        samples.len(),
        t.elapsed()
    );
    *state.style.model.locked() = Some(m.clone());
    m
}

/// Hinweis, dass Auto nach dem Stil des Nutzers angepasst wurde
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StyleNote {
    pub stage: u8,
    pub samples: usize,
    pub group: String,
}

/// Ergebnis von „Auto“: Tonwerte, ggf. weitere Regler und Hinweis
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoResult {
    #[serde(flatten)]
    pub tone: AutoTone,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saturation: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clarity: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dehaze: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sharpen: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<StyleNote>,
}

impl AutoResult {
    /// Alle Werte in die Einstellungen übernehmen
    pub fn apply(&self, p: &mut DevelopParams) {
        let t = &self.tone;
        p.exposure = t.exposure;
        p.contrast = t.contrast;
        p.highlights = t.highlights;
        p.shadows = t.shadows;
        p.whites = t.whites;
        p.blacks = t.blacks;
        p.vibrance = t.vibrance;
        if let Some(v) = self.saturation {
            p.saturation = v;
        }
        if let Some(v) = self.clarity {
            p.clarity = v;
        }
        if let Some(v) = self.dehaze {
            p.dehaze = v;
        }
        if let Some(v) = self.sharpen {
            p.sharpen = v;
        }
    }
}

/// Auto für ein Foto, nach dem gelernten Stil angepasst (wenn eingeschaltet
/// und etwas gelernt ist).
pub fn auto(state: &AppState, id: i64, src: &Source, base: &DevelopParams) -> AutoResult {
    let (tone, features) = auto_tone_with_features(src, base);
    let plain = AutoResult {
        tone,
        saturation: None,
        clarity: None,
        dehaze: None,
        sharpen: None,
        style: None,
    };
    if !enabled(state) {
        return plain;
    }
    let m = model(state);
    let (camera, raw) = {
        let cat = state.catalog.locked();
        let raw = cat
            .thumb_job(id)
            .map(|j| j.kind == ImageKind::Raw)
            .unwrap_or(false);
        (cat.image(id).ok().and_then(|d| d.camera), raw)
    };
    let g = group(camera.as_deref(), raw);
    let emb = if m.summary().stage4 {
        crate::ai::embeddings_for(state, &[id]).remove(&id)
    } else {
        None
    };
    let pred = m.predict(&Query {
        group: &g,
        features,
        embedding: emb.as_deref(),
    });
    if pred.stage == 0 {
        return plain;
    }
    // Auf die Reglerbereiche begrenzen
    let mut p = base.clone();
    let d = pred.tone_delta;
    let mut t = tone;
    t.exposure += d[0];
    t.contrast += d[1];
    t.highlights += d[2];
    t.shadows += d[3];
    t.whites += d[4];
    t.blacks += d[5];
    t.vibrance += d[6];
    let mut r = AutoResult {
        tone: t,
        saturation: pred.extra[0],
        clarity: pred.extra[1],
        dehaze: pred.extra[2],
        sharpen: pred.extra[3],
        style: Some(StyleNote {
            stage: pred.stage,
            samples: pred.samples,
            group: g,
        }),
    };
    r.apply(&mut p);
    let p = p.sanitized();
    r.tone = AutoTone {
        exposure: (p.exposure / 0.05).round() * 0.05,
        contrast: p.contrast.round(),
        highlights: p.highlights.round(),
        shadows: p.shadows.round(),
        whites: p.whites.round(),
        blacks: p.blacks.round(),
        vibrance: p.vibrance.round(),
    };
    r.saturation = r.saturation.map(|_| p.saturation.round());
    r.clarity = r.clarity.map(|_| p.clarity.round());
    r.dehaze = r.dehaze.map(|_| p.dehaze.round());
    r.sharpen = r.sharpen.map(|_| p.sharpen.round());
    r
}

/// Stand des Lernens für die Einstellungen
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StyleStatus {
    pub enabled: bool,
    /// ausgewertete Bearbeitungen
    pub samples: usize,
    /// davon mit Änderungen am Ton
    pub tone_samples: usize,
    /// noch nicht ausgewertete Bearbeitungen
    pub pending: usize,
    pub summary: Summary,
}

#[tauri::command]
pub async fn style_status(app: AppHandle) -> Result<StyleStatus, Error> {
    crate::blocking(app, |state| {
        let (samples, tone, reset_at, have) = {
            let f = state.style.file.locked();
            (
                f.samples.len(),
                f.samples.iter().filter(|s| s.tone_edited).count(),
                f.reset_at,
                f.samples
                    .iter()
                    .map(|s| (s.image_id, s.stamp))
                    .collect::<std::collections::HashSet<_>>(),
            )
        };
        let pending = state
            .catalog
            .locked()
            .edited_overview()?
            .into_iter()
            .filter(|(id, stamp, _, _)| *stamp >= reset_at && !have.contains(&(*id, *stamp)))
            .count();
        Ok(StyleStatus {
            enabled: enabled(state),
            samples,
            tone_samples: tone,
            pending,
            summary: model(state).summary(),
        })
    })
    .await
}

/// Gelernten Stil vergessen: Bisherige Bearbeitungen zählen nicht mehr, Auto
/// verhält sich wieder wie ab Werk.
#[tauri::command]
pub fn style_reset(state: State<AppState>) {
    let mut f = state.style.file.locked();
    f.samples.clear();
    f.reset_at = unix_now() + 1;
    state.style.save(&f);
    *state.style.model.locked() = None;
    log::info!("Stil zurückgesetzt");
}

/// Einstellung geändert: Beim Einschalten gleich mit dem Sammeln beginnen
pub fn settings_changed(state: &AppState) {
    if enabled(state) {
        *state.style.due.locked() = Instant::now() + Duration::from_secs(2);
        state.style.wake.notify_all();
    }
}
