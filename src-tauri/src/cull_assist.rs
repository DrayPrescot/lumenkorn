//! Aussortier-Assistent: schlägt Fotos zum Ablehnen vor – aus Serien die
//! schwächeren, dazu unscharfe, mit geschlossenen Augen, stark falsch
//! belichtete und versehentlich ausgelöste.
//!
//! Der Assistent markiert nichts selbst: Er liefert je Foto und Grund einen
//! Schweregrad (0…1), die Oberfläche schlägt vor, was über der Schwelle des
//! Grundes liegt (verschoben um die gewählte Strenge), und der Nutzer
//! entscheidet. Fotos mit Sternen, Pick-Markierung oder Bearbeitung werden nie
//! vorgeschlagen, das beste Foto einer Serie auch nicht.
//!
//! **Lernen:** Jede Entscheidung in der Prüfansicht (Vorschlag übernommen
//! oder abgewählt, Grenzfall doch abgelehnt) wird in `aussortieren.json` im
//! Datenordner festgehalten, ebenso eine spätere Rücknahme einer Ablehnung.
//! Daraus wird je Grund die Schwelle verschoben – vorsichtig, mit der
//! Grundschwelle als Vorwissen. „Zurücksetzen“ löscht alles Gelernte.

use crate::AppState;
use crate::album::select::{
    Features, color_diff, different_people, find, luma_corr, measure, same_shape, signature,
};
use lumenkorn_core::thumbs::Variant;
use lumenkorn_core::{Error, ImageQuery, LockExt, Scope, SortOrder};
use lumenkorn_develop::i18n::tr_s;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tauri::{AppHandle, Emitter, State};

const FILE: &str = "aussortieren.json";
/// Kantenlänge, auf der gemessen wird (die Vorschau hat 2048)
const MEASURE_EDGE: u32 = 1024;
/// Serie: Fotos höchstens so weit auseinander (zum vorigen der Serie) …
const BURST_GAP_S: i64 = 90;
/// … und fast gleich: Helligkeit/Farbe, Differenz-Hash oder CLIP
const BURST_CORR: f32 = 0.75;
const BURST_COLOR: f32 = 0.06;
const BURST_HASH_BITS: u32 = 8;
const BURST_COS: f32 = 0.95;
/// Mittlere Helligkeit um mehr als diesen Faktor verschieden: keine Serie
const BRACKET_RATIO: f32 = 1.5;
/// Absolute Schärfe (Laplace auf 1024 px): darunter sicher unscharf, darüber
/// unauffällig. Gemessen an Testfotos: scharf 450–5800 je nach Motiv, stark
/// weichgezeichnet 17–75 – nur klare Fälle, den Rest erkennt der Vergleich
/// in der Serie.
const SHARP_BAD: f32 = 40.0;
const SHARP_OK: f32 = 160.0;
/// Gesichter, die für die Augen-Prüfung groß genug sind (Fläche normiert,
/// Breite in Pixeln der Vorschau)
const EYE_FACE_AREA: f64 = 0.004;
const EYE_FACE_PX: f64 = 60.0;
/// Grundschwelle bei Strenge „normal“
pub const BASE: f32 = 0.5;
/// So viele Lernbeispiele je Grund, bevor sich die Schwelle bewegt
const MIN_SAMPLES: usize = 8;
/// Vorwissen: Eine Verschiebung um 0,1 „kostet“ so viele Fehlentscheidungen
const PRIOR_PER_TENTH: f32 = 2.0;
/// Älteste Lernbeispiele fallen darüber hinaus weg
const MAX_SAMPLES: usize = 5000;

/// CLIP: Beschreibungen gewöhnlicher Fotos und versehentlicher Auslösungen
const PHOTO_PROMPTS: &[&str] = &[
    "a photo of a scene",
    "a photo of people",
    "a landscape photo",
    "a photo of an object",
];
const ACCIDENT_PROMPTS: &[&str] = &[
    "an accidental photo of the floor",
    "a photo taken by accident inside a bag",
    "a blurry photo of nothing in particular",
    "a completely black photo",
    "an accidental photo of feet and the ground",
];

static CANCEL: AtomicBool = AtomicBool::new(false);
/// Lesen und Schreiben von `aussortieren.json` nacheinander
static FILE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    /// Serie: bessere Fotos vorhanden
    Series,
    Blur,
    /// Augen geschlossen
    Eyes,
    Exposure,
    /// Versehentlich ausgelöst
    Accident,
}

pub const REASONS: [Reason; 5] = [
    Reason::Series,
    Reason::Blur,
    Reason::Eyes,
    Reason::Exposure,
    Reason::Accident,
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub reason: Reason,
    pub severity: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CullItem {
    pub id: i64,
    pub captured_at: Option<String>,
    /// Serie (nur bei mindestens zwei Fotos) und Platz darin (0 = bestes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series: Option<usize>,
    pub rank: usize,
    /// Bleibt in der Serie (unter den besten)
    pub keep: bool,
    /// Sterne, Pick oder bearbeitet – wird nie vorgeschlagen
    pub protected: bool,
    /// Gründe mit Schweregrad > 0, der schwerste zuerst
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CullResult {
    /// Fotos mit Befund und alle Fotos von Serien, chronologisch
    pub items: Vec<CullItem>,
    /// Gelernte Schwelle je Grund (Strenge „normal“)
    pub thresholds: HashMap<Reason, f32>,
    pub analyzed: usize,
    /// Augen geprüft (KI-Masken für Personen und Gesichter vorhanden)
    pub eyes_checked: bool,
    /// CLIP-Analyse genutzt (Serien inhaltlich, versehentliche Fotos)
    pub clip_used: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress {
    done: usize,
    total: usize,
}

// --- Bewertung (ohne Ein-/Ausgabe) --------------------------------------------

/// Ein gemessenes Foto.
#[derive(Debug, Clone, Default)]
pub(crate) struct Photo {
    pub f: Features,
    pub raw: bool,
    /// Lidöffnung des Gesichts mit den geschlossensten Augen
    pub eyes: Option<f32>,
    /// CLIP: Vorsprung „versehentlich“ vor „gewöhnliches Foto“
    pub accident: Option<f32>,
}

impl Photo {
    fn protected(&self) -> bool {
        self.f.rating > 0 || self.f.flag == 1 || self.f.edited
    }
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// Unschärfe ohne Vergleich: 0,5 bei 80, logarithmisch zwischen
/// [`SHARP_OK`] und [`SHARP_BAD`].
pub(crate) fn blur_absolute(sharp: f32) -> f32 {
    let (ok, bad) = (SHARP_OK.ln(), SHARP_BAD.ln());
    clamp01((ok - sharp.max(1e-3).ln()) / (ok - bad))
}

/// Unschärfe im Vergleich zum schärfsten Foto der Serie: ab 75 % unauffällig,
/// bei der Hälfte 0,5, bei einem Viertel 1 (leicht weichgezeichnet ≈ 40 %,
/// verwackelt ≈ 25 %).
pub(crate) fn blur_relative(sharp: f32, best: f32) -> f32 {
    if best <= 0.0 {
        return 0.0;
    }
    clamp01((0.75 - sharp / best) / 0.5)
}

/// Stark falsch belichtet. RAWs haben Reserven, dort nur extreme Fälle.
pub(crate) fn exposure(p: &Photo) -> f32 {
    let (hi_from, dark_from) = if p.raw { (0.45, 0.04) } else { (0.35, 0.06) };
    let bright = clamp01((p.f.clip_hi - hi_from) / 0.3);
    let dark = clamp01((dark_from - p.f.mean) / (dark_from * 0.6));
    bright.max(dark)
}

/// Geschlossene Augen: Lidöffnung offen etwa 0,25–0,35, 0,5 bei 0,13.
pub(crate) fn eyes_closed(openness: f32) -> f32 {
    clamp01((0.18 - openness) / 0.1)
}

/// Versehentlich ausgelöst: schwarzes Bild, oder CLIP hält es dafür – und
/// dann umso eher, je unschärfer bzw. falscher belichtet.
pub(crate) fn accident(p: &Photo) -> f32 {
    if p.f.mean < 0.02 && p.f.clip_lo > 0.9 {
        return 1.0;
    }
    let Some(m) = p.accident else {
        return 0.0;
    };
    let clip = clamp01((m - 0.02) / 0.04);
    clip * (0.5 + 0.5 * blur_absolute(p.f.sharp).max(exposure(p)))
}

/// Wie viele Fotos einer Serie bleiben: 3 bis 5 je nach Größe.
pub(crate) fn keep_count(n: usize) -> usize {
    match n {
        0..=8 => 3,
        9..=16 => 4,
        _ => 5,
    }
}

/// Serie: kurz hintereinander, fast dasselbe Bild, dieselben Personen.
fn is_burst(a: &Photo, b: &Photo) -> bool {
    let (a, b) = (&a.f, &b.f);
    let Some((ta, tb)) = a.time.zip(b.time) else {
        return false;
    };
    if (tb - ta).abs() > BURST_GAP_S || different_people(a, b) || !same_shape(a, b) {
        return false;
    }
    // Deutlich andere Helligkeit: eher eine Belichtungsreihe (für HDR), deren
    // dunkle und helle Aufnahmen gebraucht werden
    let (lo, hi) = (a.mean.min(b.mean).max(0.02), a.mean.max(b.mean).max(0.02));
    if hi / lo > BRACKET_RATIO {
        return false;
    }
    let cos = match (&a.vector, &b.vector) {
        (Some(x), Some(y)) => lumenkorn_ai::clip::dot(x, y),
        _ => 0.0,
    };
    cos >= BURST_COS
        || (luma_corr(a, b) >= BURST_CORR && color_diff(a, b) <= BURST_COLOR)
        || (a.hash ^ b.hash).count_ones() <= BURST_HASH_BITS
}

/// Serien bilden: Fotos sind chronologisch; jedes wird mit den Fotos der
/// letzten [`BURST_GAP_S`] Sekunden verglichen. Liefert je Foto eine
/// Gruppennummer.
fn bursts(p: &[Photo]) -> Vec<usize> {
    let mut parent: Vec<usize> = (0..p.len()).collect();
    for i in 0..p.len() {
        for j in (0..i).rev() {
            match (p[j].f.time, p[i].f.time) {
                (Some(a), Some(b)) if b - a <= BURST_GAP_S => {}
                _ => break,
            }
            if is_burst(&p[j], &p[i]) {
                let (a, b) = (find(&mut parent, j), find(&mut parent, i));
                parent[b.max(a)] = a.min(b);
            }
        }
    }
    (0..p.len()).map(|i| find(&mut parent, i)).collect()
}

/// Bewertet alle Fotos (chronologisch). Ergebnis in derselben Reihenfolge;
/// Fotos ohne Befund und ohne Serie fehlen.
pub(crate) fn evaluate(p: &[Photo]) -> Vec<(usize, CullItem)> {
    let root = bursts(p);
    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, r) in root.iter().enumerate() {
        members.entry(*r).or_default().push(i);
    }
    // Serien in Reihenfolge ihres ersten Fotos nummerieren
    let mut series_no: HashMap<usize, usize> = HashMap::new();
    for r in &root {
        if members[r].len() >= 2 {
            let n = series_no.len();
            series_no.entry(*r).or_insert(n);
        }
    }

    let base: Vec<[f32; 3]> = p
        .iter()
        .map(|x| [exposure(x), x.eyes.map_or(0.0, eyes_closed), accident(x)])
        .collect();

    let mut series = vec![None; p.len()];
    let mut rank = vec![0usize; p.len()];
    let mut keep = vec![true; p.len()];
    let mut series_sev = vec![0f32; p.len()];
    let mut best_sharp: Vec<f32> = p.iter().map(|x| x.f.sharp).collect();
    for (r, list) in &members {
        let Some(&no) = series_no.get(r) else {
            continue;
        };
        let best = list.iter().map(|&i| p[i].f.sharp).fold(0f32, f32::max);
        // Güte in der Serie: Schärfe relativ zur schärfsten, abzüglich der
        // übrigen Befunde; Geschütztes vorn
        let quality = |i: usize| {
            let x = &p[i];
            let [exp, eyes, acc] = base[i];
            let mut q = if best > 0.0 { x.f.sharp / best } else { 1.0 };
            q -= 0.5 * exp + 0.6 * eyes + 0.5 * acc;
            q += 0.1 * x.f.rating.clamp(0, 5) as f32;
            if x.f.flag == 1 {
                q += 0.5;
            }
            if x.f.edited {
                q += 0.2;
            }
            q
        };
        let mut order: Vec<(usize, f32)> = list.iter().map(|&i| (i, quality(i))).collect();
        order.sort_by(|a, b| {
            p[b.0]
                .protected()
                .cmp(&p[a.0].protected())
                .then(b.1.total_cmp(&a.1))
        });
        let k = keep_count(list.len());
        let bar = order[(k - 1).min(order.len() - 1)].1;
        for (pos, &(i, q)) in order.iter().enumerate() {
            series[i] = Some(no);
            rank[i] = pos;
            best_sharp[i] = best;
            keep[i] = pos < k || p[i].protected();
            if !keep[i] {
                series_sev[i] = clamp01(0.55 + (bar - q).max(0.0) * 1.5);
            }
        }
    }

    let mut out = Vec::new();
    for (i, x) in p.iter().enumerate() {
        let protected = x.protected();
        let mut findings = Vec::new();
        // Geschützte Fotos und das beste einer Serie: nie vorschlagen
        let best_of_series = series[i].is_some() && rank[i] == 0;
        if !protected && !best_of_series {
            let blur = if series[i].is_some() {
                blur_absolute(x.f.sharp).max(blur_relative(x.f.sharp, best_sharp[i]))
            } else {
                blur_absolute(x.f.sharp)
            };
            let [exp, eyes, acc] = base[i];
            for (reason, sev) in [
                (Reason::Series, series_sev[i]),
                (Reason::Blur, blur),
                (Reason::Eyes, eyes),
                (Reason::Exposure, exp),
                (Reason::Accident, acc),
            ] {
                if sev > 0.01 {
                    findings.push(Finding {
                        reason,
                        severity: sev,
                    });
                }
            }
            findings.sort_by(|a, b| b.severity.total_cmp(&a.severity));
        }
        if findings.is_empty() && series[i].is_none() {
            continue;
        }
        out.push((
            i,
            CullItem {
                id: x.f.id,
                captured_at: x.f.captured_at.clone(),
                series: series[i],
                rank: rank[i],
                keep: keep[i],
                protected,
                findings,
            },
        ));
    }
    out
}

// --- Lernen --------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Sample {
    id: i64,
    reason: Reason,
    severity: f32,
    /// Abgelehnt (Vorschlag richtig bzw. Grenzfall doch aussortiert)
    reject: bool,
    at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct LearnFile {
    version: u32,
    samples: Vec<Sample>,
    /// Vom Assistenten abgelehnte Fotos: Hebt der Nutzer die Ablehnung
    /// später auf, war der Vorschlag falsch.
    applied: Vec<i64>,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn load(state: &AppState) -> LearnFile {
    std::fs::read_to_string(state.data_dir.join(FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(state: &AppState, f: &LearnFile) {
    let path = state.data_dir.join(FILE);
    let tmp = path.with_extension("json.tmp");
    let r = serde_json::to_string(f)
        .map_err(std::io::Error::other)
        .and_then(|s| std::fs::write(&tmp, s))
        .and_then(|_| std::fs::rename(&tmp, &path));
    if let Err(e) = r {
        log::warn!("{FILE} nicht gespeichert: {e}");
    }
}

/// Schwelle aus Entscheidungen (Schweregrad, abgelehnt): die mit den wenigsten
/// Fehlentscheidungen, wobei jede Verschiebung von `base` weg etwas kostet.
/// Unter [`MIN_SAMPLES`] Beispielen bleibt es bei `base`.
pub(crate) fn fit(samples: &[(f32, bool)], base: f32) -> f32 {
    if samples.len() < MIN_SAMPLES {
        return base;
    }
    let mut best = (f32::MAX, base);
    for step in 15..=90 {
        let t = step as f32 / 100.0;
        let wrong = samples
            .iter()
            .filter(|(s, reject)| (*s >= t) != *reject)
            .count() as f32;
        let cost = wrong + PRIOR_PER_TENTH * (t - base).abs() * 10.0;
        if cost < best.0 - 1e-4
            || ((cost - best.0).abs() <= 1e-4 && (t - base).abs() < (best.1 - base).abs())
        {
            best = (cost, t);
        }
    }
    best.1
}

fn thresholds(f: &LearnFile) -> HashMap<Reason, f32> {
    REASONS
        .iter()
        .map(|r| {
            let s: Vec<(f32, bool)> = f
                .samples
                .iter()
                .filter(|x| x.reason == *r)
                .map(|x| (x.severity, x.reject))
                .collect();
            (*r, fit(&s, BASE))
        })
        .collect()
}

/// Vom Assistenten abgelehnte Fotos, deren Ablehnung inzwischen aufgehoben
/// ist, als Fehlvorschlag vermerken.
fn reconcile(state: &AppState, f: &mut LearnFile) -> Result<bool, Error> {
    if f.applied.is_empty() {
        return Ok(false);
    }
    let now: HashMap<i64, i64> = state
        .catalog
        .locked()
        .images_in_order(&f.applied, &ImageQuery::new(Scope::All))?
        .into_iter()
        .map(|i| (i.id, i.flag))
        .collect();
    let before = f.applied.len();
    let mut taken_back = Vec::new();
    f.applied.retain(|id| match now.get(id) {
        Some(-1) => true,
        Some(_) => {
            taken_back.push(*id);
            false
        }
        // Gelöscht oder nicht mehr vorhanden: die Entscheidung bleibt
        None => false,
    });
    for s in &mut f.samples {
        if s.reject && taken_back.contains(&s.id) {
            s.reject = false;
        }
    }
    Ok(f.applied.len() != before)
}

// --- Befehle ---------------------------------------------------------------------

fn canceled() -> Error {
    Error::Invalid(tr_s("Abgebrochen", "Canceled").into())
}

/// Fotos des Bereichs analysieren. Fortschritt per `cull-progress`.
#[tauri::command]
pub async fn cull_suggest(app: AppHandle, scope: Scope) -> Result<CullResult, Error> {
    CANCEL.store(false, Ordering::Relaxed);
    let emitter = app.clone();
    crate::blocking(app, move |state| suggest(state, &emitter, scope)).await
}

#[tauri::command]
pub fn cull_suggest_cancel() {
    CANCEL.store(true, Ordering::Relaxed);
}

fn suggest(state: &AppState, app: &AppHandle, scope: Scope) -> Result<CullResult, Error> {
    let images = {
        let mut q = ImageQuery::new(scope);
        q.sort = SortOrder::CapturedAsc;
        state.catalog.locked().query_images(&q)?
    };
    let images: Vec<_> = images.into_iter().filter(|i| i.flag != -1).collect();
    let total = images.len();
    let done = AtomicUsize::new(0);
    let _ = app.emit("cull-progress", Progress { done: 0, total });
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .clamp(2, 6),
        )
        .build()
        .map_err(|e| Error::Invalid(e.to_string()))?;

    // Gesichter, die für die Augen groß genug sind (nur bei unbearbeiteten
    // Fotos: Die Lage bezieht sich aufs unbearbeitete Bild, und bearbeitete
    // werden ohnehin nicht vorgeschlagen)
    let ids: Vec<i64> = images.iter().map(|i| i.id).collect();
    let faces = state.catalog.locked().faces_of_images(&ids)?;
    let mut face_rects: HashMap<i64, Vec<[f64; 4]>> = HashMap::new();
    let mut face_count: HashMap<i64, (u32, Vec<i64>)> = HashMap::new();
    for fc in &faces {
        if fc.rect[2] * fc.rect[3] >= EYE_FACE_AREA {
            face_rects.entry(fc.image_id).or_default().push(fc.rect);
            let e = face_count.entry(fc.image_id).or_default();
            e.0 += 1;
            if let Some(p) = fc.person_id {
                e.1.push(p);
            }
        }
    }
    let mesh = if face_rects.is_empty() {
        None
    } else {
        crate::ai::face_mesh(state)
    };
    let eyes_checked = mesh.is_some();

    let measured: Vec<Option<Photo>> = pool.install(|| {
        images
            .par_iter()
            .map(|img| {
                if CANCEL.load(Ordering::Relaxed) {
                    return None;
                }
                let r = (|| -> Option<Photo> {
                    let job = state.catalog.locked().thumb_job(img.id).ok()?;
                    let bytes = state.cache.get_or_render(Variant::Preview, &job).ok()?;
                    let full = image::load_from_memory(&bytes).ok()?;
                    let small = full.thumbnail(MEASURE_EDGE, MEASURE_EDGE);
                    let (sharp, clip_hi, clip_lo, mean, hash) = measure(&small.to_luma8());
                    let (luma, color) = signature(&small);
                    let (faces, mut people) = face_count.get(&img.id).cloned().unwrap_or_default();
                    people.sort_unstable();
                    people.dedup();
                    let mut eyes = None;
                    if let (Some(mesh), Some(rects), false) =
                        (&mesh, face_rects.get(&img.id), img.edited)
                    {
                        let rgb = full.to_rgb8();
                        let (w, h) = (rgb.width() as f64, rgb.height() as f64);
                        let boxes: Vec<[f32; 4]> = rects
                            .iter()
                            .filter(|r| r[2] * w >= EYE_FACE_PX)
                            .map(|r| {
                                [
                                    (r[0] * w) as f32,
                                    (r[1] * h) as f32,
                                    ((r[0] + r[2]) * w) as f32,
                                    ((r[1] + r[3]) * h) as f32,
                                ]
                            })
                            .collect();
                        match lumenkorn_ai::parts::eye_openness(&rgb, &boxes, mesh) {
                            Ok(v) => eyes = v.into_iter().flatten().reduce(f32::min),
                            Err(e) => log::warn!("Augen-Prüfung {}: {e}", img.id),
                        }
                    }
                    Some(Photo {
                        f: Features {
                            id: img.id,
                            time: img
                                .captured_at
                                .as_deref()
                                .and_then(lumenkorn_core::geotime::parse_time),
                            captured_at: img.captured_at.clone(),
                            aspect: full.width().max(1) as f32 / full.height().max(1) as f32,
                            sharp,
                            clip_hi,
                            clip_lo,
                            mean,
                            hash,
                            luma,
                            color,
                            rating: img.rating,
                            flag: img.flag,
                            edited: img.edited,
                            faces,
                            people,
                            ..Default::default()
                        },
                        raw: img.kind == lumenkorn_core::formats::ImageKind::Raw,
                        eyes,
                        accident: None,
                    })
                })();
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n.is_multiple_of(8) || n == total {
                    let _ = app.emit("cull-progress", Progress { done: n, total });
                }
                r
            })
            .collect()
    });
    if CANCEL.load(Ordering::Relaxed) {
        return Err(canceled());
    }
    let mut photos: Vec<Photo> = measured.into_iter().flatten().collect();

    // CLIP: Serien auch inhaltlich, versehentliche Fotos
    let ids: Vec<i64> = photos.iter().map(|x| x.f.id).collect();
    let prompts: Vec<String> = PHOTO_PROMPTS.iter().map(|s| s.to_string()).collect();
    let clip =
        crate::ai::theme_scores(state, &prompts, ACCIDENT_PROMPTS, &ids).unwrap_or_else(|e| {
            log::warn!("Aussortieren: KI nicht nutzbar: {e}");
            None
        });
    let clip_used = clip.is_some();
    if let Some(mut c) = clip {
        for x in &mut photos {
            if let (Some(n), Some(t)) = (c.negative.get(&x.f.id), c.theme.get(&x.f.id)) {
                x.accident = Some(n - t);
            }
            x.f.vector = c.vectors.remove(&x.f.id);
        }
    }

    let items: Vec<CullItem> = evaluate(&photos).into_iter().map(|(_, it)| it).collect();
    let learned = {
        let _g = FILE_LOCK.locked();
        let mut f = load(state);
        if reconcile(state, &mut f)? {
            save(state, &f);
        }
        thresholds(&f)
    };
    log::info!(
        "Aussortieren: {} Fotos analysiert, {} mit Befund oder in Serien, Augen {}, CLIP {}",
        photos.len(),
        items.len(),
        if eyes_checked { "ja" } else { "nein" },
        if clip_used { "ja" } else { "nein" }
    );
    Ok(CullResult {
        items,
        thresholds: learned,
        analyzed: photos.len(),
        eyes_checked,
        clip_used,
    })
}

/// Eine Entscheidung in der Prüfansicht (ausschlaggebender Grund).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub id: i64,
    pub reason: Reason,
    pub severity: f32,
    pub reject: bool,
}

/// Entscheidungen merken. Abgelehnt markiert die Oberfläche selbst.
#[tauri::command(async)]
pub fn cull_learn(state: State<AppState>, decisions: Vec<Decision>) {
    let _g = FILE_LOCK.locked();
    let mut f = load(&state);
    let at = unix_now();
    for d in &decisions {
        // Neuere Entscheidung zum selben Foto und Grund ersetzt die alte
        f.samples
            .retain(|s| !(s.id == d.id && s.reason == d.reason));
        f.samples.push(Sample {
            id: d.id,
            reason: d.reason,
            severity: d.severity,
            reject: d.reject,
            at,
        });
        if d.reject && !f.applied.contains(&d.id) {
            f.applied.push(d.id);
        }
    }
    if f.samples.len() > MAX_SAMPLES {
        let drop = f.samples.len() - MAX_SAMPLES;
        f.samples.drain(..drop);
    }
    f.version = 1;
    save(&state, &f);
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasonStatus {
    pub reason: Reason,
    pub samples: usize,
    /// Davon abgelehnt
    pub rejected: usize,
    /// Gelernte Schwelle (Grundschwelle [`BASE`])
    pub threshold: f32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CullStatus {
    pub samples: usize,
    pub base: f32,
    pub reasons: Vec<ReasonStatus>,
}

#[tauri::command(async)]
pub fn cull_status(state: State<AppState>) -> Result<CullStatus, Error> {
    let _g = FILE_LOCK.locked();
    let mut f = load(&state);
    if reconcile(&state, &mut f)? {
        save(&state, &f);
    }
    let t = thresholds(&f);
    Ok(CullStatus {
        samples: f.samples.len(),
        base: BASE,
        reasons: REASONS
            .iter()
            .map(|r| {
                let mine = f.samples.iter().filter(|s| s.reason == *r);
                ReasonStatus {
                    reason: *r,
                    samples: mine.clone().count(),
                    rejected: mine.filter(|s| s.reject).count(),
                    threshold: t[r],
                }
            })
            .collect(),
    })
}

/// Gelerntes vergessen: Der Assistent schlägt wieder vor wie ab Werk.
#[tauri::command(async)]
pub fn cull_reset(state: State<AppState>) {
    let _g = FILE_LOCK.locked();
    save(&state, &LearnFile::default());
    log::info!("Aussortier-Assistent zurückgesetzt");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(id: i64, time: i64, hash: u64, sharp: f32) -> Photo {
        Photo {
            f: Features {
                id,
                time: Some(time),
                aspect: 1.5,
                sharp,
                hash,
                mean: 0.45,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn sev(items: &[(usize, CullItem)], id: i64, reason: Reason) -> f32 {
        items
            .iter()
            .find(|(_, it)| it.id == id)
            .and_then(|(_, it)| it.findings.iter().find(|f| f.reason == reason))
            .map_or(0.0, |f| f.severity)
    }

    #[test]
    fn keeps_three_to_five_of_a_series() {
        assert_eq!(keep_count(2), 3);
        assert_eq!(keep_count(8), 3);
        assert_eq!(keep_count(12), 4);
        assert_eq!(keep_count(40), 5);
        // Zehn fast gleiche Fotos im Sekundenabstand, eins davon das schärfste
        let p: Vec<Photo> = (0..10)
            .map(|i| photo(i, 1000 + i, 0xF0F0, 1000.0 - i as f32 * 10.0))
            .collect();
        let r = evaluate(&p);
        assert_eq!(r.len(), 10);
        let kept = r.iter().filter(|(_, it)| it.keep).count();
        assert_eq!(kept, 4);
        assert!(r.iter().all(|(_, it)| it.series == Some(0)));
        // Die schärfsten bleiben, die übrigen werden vorgeschlagen
        for (_, it) in &r {
            let s = it.findings.iter().any(|f| f.reason == Reason::Series);
            assert_eq!(s, !it.keep, "Foto {}", it.id);
            assert_eq!(it.keep, it.id < 4, "Foto {}", it.id);
        }
    }

    #[test]
    fn small_series_and_protected_photos_stay() {
        // Drei Fotos: alle bleiben (Serie kleiner als die Mindestzahl)
        let p: Vec<Photo> = (0..3).map(|i| photo(i, 1000 + i, 0xAA, 900.0)).collect();
        assert!(
            evaluate(&p)
                .iter()
                .all(|(_, it)| it.keep && it.findings.is_empty())
        );
        // Sechs Fotos, das unschärfste hat Sterne: bleibt und belegt einen Platz
        let mut p: Vec<Photo> = (0..6)
            .map(|i| photo(i, 1000 + i, 0xAA, 900.0 - i as f32 * 100.0))
            .collect();
        p[5].f.rating = 3;
        let r = evaluate(&p);
        let keep: Vec<i64> = r
            .iter()
            .filter(|(_, it)| it.keep)
            .map(|(_, it)| it.id)
            .collect();
        assert_eq!(keep, vec![0, 1, 5]);
        let protected = &r.iter().find(|(_, it)| it.id == 5).unwrap().1;
        assert!(protected.protected && protected.findings.is_empty());
    }

    #[test]
    fn blur_relative_to_the_series_and_absolute() {
        // Scharf, leicht weich (40 %), verwackelt (20 %)
        let p = vec![
            photo(1, 1000, 0x55, 2000.0),
            photo(2, 1001, 0x55, 800.0),
            photo(3, 1002, 0x55, 400.0),
        ];
        let r = evaluate(&p);
        assert_eq!(sev(&r, 1, Reason::Blur), 0.0);
        assert!(sev(&r, 2, Reason::Blur) >= 0.5);
        assert_eq!(sev(&r, 3, Reason::Blur), 1.0);
        // Einzelfoto: nur deutlich unscharfe
        assert!(blur_absolute(500.0) == 0.0);
        assert!((blur_absolute(80.0) - 0.5).abs() < 0.01);
        assert!(blur_absolute(30.0) == 1.0);
        let r = evaluate(&[photo(9, 5000, 0x1, 60.0)]);
        assert!(sev(&r, 9, Reason::Blur) > 0.5);
    }

    #[test]
    fn different_scenes_or_far_apart_are_no_series() {
        let p = vec![
            photo(1, 1000, 0x0000_0000_FFFF_FFFF, 900.0),
            photo(2, 1001, 0xFFFF_FFFF_0000_0000, 500.0),
            photo(3, 1500, 0x0000_0000_FFFF_FFFF, 400.0),
        ];
        assert!(evaluate(&p).is_empty());
        // Belichtungsreihe (−2, 0, +2 EV): gleiches Motiv, aber keine Serie
        let mut p: Vec<Photo> = (0..3).map(|i| photo(i, 1000 + i, 0xAA, 900.0)).collect();
        p[0].f.mean = 0.12;
        p[2].f.mean = 0.8;
        p.push(photo(3, 1003, 0xAA, 900.0));
        p[3].f.mean = 0.12;
        let r = evaluate(&p);
        assert!(
            r.iter()
                .all(|(_, it)| it.series.is_none() || it.id == 0 || it.id == 3)
        );
    }

    #[test]
    fn eyes_exposure_and_accidents() {
        assert_eq!(eyes_closed(0.3), 0.0);
        assert!((eyes_closed(0.13) - 0.5).abs() < 0.01);
        assert_eq!(eyes_closed(0.05), 1.0);
        let mut x = photo(1, 0, 0, 900.0);
        x.f.clip_hi = 0.7;
        assert_eq!(exposure(&x), 1.0);
        x.raw = true;
        assert!(exposure(&x) < 1.0);
        let mut black = photo(2, 0, 0, 1.0);
        black.f.mean = 0.01;
        black.f.clip_lo = 0.98;
        assert_eq!(accident(&black), 1.0);
        let mut floor = photo(3, 0, 0, 900.0);
        floor.accident = Some(0.06);
        assert!((accident(&floor) - 0.5).abs() < 0.01);
    }

    #[test]
    fn learning_moves_the_threshold_only_with_evidence() {
        // Zu wenig Beispiele: Grundschwelle
        assert_eq!(fit(&[(0.6, false); 5], BASE), BASE);
        // Vorschläge um 0,55–0,65 werden regelmäßig abgewählt: Schwelle steigt
        let mut s: Vec<(f32, bool)> = (0..12).map(|i| (0.52 + i as f32 * 0.01, false)).collect();
        s.extend((0..8).map(|i| (0.8 + i as f32 * 0.02, true)));
        let t = fit(&s, BASE);
        assert!(t > 0.62 && t <= 0.8, "{t}");
        // Grenzfälle um 0,4 werden doch abgelehnt: Schwelle sinkt
        let s: Vec<(f32, bool)> = (0..12).map(|i| (0.38 + i as f32 * 0.01, true)).collect();
        let t = fit(&s, BASE);
        assert!(t <= 0.39, "{t}");
        // Gemischt um die Grundschwelle: bleibt
        let s: Vec<(f32, bool)> = (0..20)
            .map(|i| (0.305 + i as f32 * 0.02, i % 2 == 0))
            .collect();
        assert_eq!(fit(&s, BASE), BASE);
    }
}
