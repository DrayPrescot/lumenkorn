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
//!
//! **Lernen ohne den Assistenten:** Auch was der Nutzer sonst in der Bibliothek
//! entscheidet, zählt: Ablehnen, Pick und Sterne (nicht aber, was ein KI-Agent
//! über MCP setzt). `commands.rs` meldet die Fotos, der Worker wertet sie
//! stapelweise aus (`Job::CullLearn`), und vor dem Löschen abgelehnter Fotos
//! holt `learn_before_delete` nach, was noch fehlt. Dabei gilt:
//!
//! - Der Grund ist nur geraten, deshalb zählen solche Beispiele schwächer
//!   ([`Source::weight`]) und bewegen die Schwelle allein nur begrenzt
//!   ([`HAND_SHIFT`]). Ein Foto, das aus ganz anderen Gründen abgelehnt wurde,
//!   zählt gar nicht: Es muss wie im Dialog mindestens ein Grenzfall sein.
//! - Nur aus Ablehnungen würde die Schwelle immer weiter sinken. Darum gelten
//!   die Fotos neben einer Ablehnung als gesehen und behalten
//!   ([`PASSED_NEIGHBORS`]). Das ist die Gegenprobe, und sie zählt am wenigsten.
//! - Gemessen wird wie vor der Entscheidung: Sterne und Markierungen schützen
//!   hier nicht, und abgelehnte Fotos gehören noch zur Serie. Sonst hätten die
//!   Schweregrade einen anderen Maßstab als die Schwellen, mit denen sie
//!   verglichen werden.
//! - Bearbeitete Fotos zählen nicht: Ihre Vorschau zeigt die Bearbeitung und
//!   nicht das Foto, über das entschieden wurde. Im Hintergrund wird nichts
//!   entwickelt (`get_light`), und CLIP wird dafür nicht geladen; „Versehentlich
//!   ausgelöst“ lernt außerhalb des Dialogs daher nur aus schwarzen Bildern.

use crate::AppState;
use crate::album::select::{
    Features, color_diff, different_people, find, luma_corr, measure, same_shape, signature,
};
use lumenkorn_core::thumbs::Variant;
use lumenkorn_core::{Error, ImageQuery, LockExt, Scope, SortOrder};
use lumenkorn_develop::fsutil::unix_now;
use lumenkorn_develop::i18n::tr_s;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

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
/// So weit unter der Schwelle gilt ein Befund als Grenzfall (wie `BORDER` in
/// `lib/cull.ts`). Darunter hat eine Ablehnung von Hand wohl einen anderen Grund.
const BORDER: f32 = 0.15;
/// Ohne genug Entscheidungen im Assistenten bewegt sich die Schwelle höchstens
/// so weit: Was von Hand entschieden wurde, sagt über den Grund nichts Sicheres.
const HAND_SHIFT: f32 = 0.2;
/// Behaltene Fotos zählen ab diesem Schweregrad: Tiefer liegt keine Schwelle.
const MIN_KEPT_SEVERITY: f32 = 0.15;
/// So viele Fotos vor und nach einer Ablehnung gelten als gesehen und behalten
const PASSED_NEIGHBORS: usize = 3;
/// Rund um eine Entscheidung gehören höchstens so viele Fotos je Seite zur Serie
const MAX_CHAIN: usize = 20;
/// Entscheidungen von Hand je Stapel im Hintergrund
const LEARN_BATCH: usize = 16;
/// Längere Warteschlange: die ältesten Entscheidungen fallen weg
const MAX_PENDING: usize = 5000;
/// So lange darf das Lernen das Löschen abgelehnter Fotos aufhalten
const DELETE_WAIT: Duration = Duration::from_secs(5);

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
/// Von Hand entschiedene Fotos, die noch nicht in der Datei stehen. Ein Tastendruck
/// soll keine Datei schreiben; was hier liegt, geht beim Beenden verloren, schadet
/// aber nicht: Es sind höchstens die letzten Sekunden.
static QUEUE: Mutex<Vec<i64>> = Mutex::new(Vec::new());
/// Gibt es etwas auszuwerten? Beim Start ja: Die Datei kann noch etwas enthalten.
static WORK: AtomicBool = AtomicBool::new(true);
/// Zählt jedes Zurücksetzen: Ein Stapel, der gerade läuft, ist danach hinfällig.
static GENERATION: AtomicU64 = AtomicU64::new(0);

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

use crate::album::select::Progress;

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

// --- Messen ---------------------------------------------------------------------

/// Gesichter, die für die Augen-Prüfung groß genug sind, und je Foto ihre Anzahl
/// mit den erkannten Personen.
#[derive(Default)]
struct BigFaces {
    rects: HashMap<i64, Vec<[f64; 4]>>,
    count: HashMap<i64, (u32, Vec<i64>)>,
}

fn big_faces(cat: &lumenkorn_core::Catalog, ids: &[i64]) -> Result<BigFaces, Error> {
    let mut out = BigFaces::default();
    for fc in &cat.faces_of_images(ids)? {
        if fc.rect[2] * fc.rect[3] >= EYE_FACE_AREA {
            out.rects.entry(fc.image_id).or_default().push(fc.rect);
            let e = out.count.entry(fc.image_id).or_default();
            e.0 += 1;
            if let Some(p) = fc.person_id {
                e.1.push(p);
            }
        }
    }
    Ok(out)
}

/// Misst ein Foto an seiner Vorschau. Die Augen nur bei unbearbeiteten Fotos: Die
/// Lage der Gesichter bezieht sich aufs unbearbeitete Bild, und bearbeitete werden
/// ohnehin nicht vorgeschlagen.
fn measure_photo(
    img: &lumenkorn_core::ImageSummary,
    bytes: &[u8],
    faces: &BigFaces,
    mesh: Option<&lumenkorn_ai::parts::FaceMesh>,
) -> Option<Photo> {
    let full = image::load_from_memory(bytes).ok()?;
    let small = full.thumbnail(MEASURE_EDGE, MEASURE_EDGE);
    let (sharp, clip_hi, clip_lo, mean, hash) = measure(&small.to_luma8());
    let (luma, color) = signature(&small);
    let (face_count, mut people) = faces.count.get(&img.id).cloned().unwrap_or_default();
    people.sort_unstable();
    people.dedup();
    let mut eyes = None;
    if let (Some(mesh), Some(rects), false) = (mesh, faces.rects.get(&img.id), img.edited) {
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
            faces: face_count,
            people,
            ..Default::default()
        },
        raw: img.kind == lumenkorn_core::formats::ImageKind::Raw,
        eyes,
        accident: None,
    })
}

// --- Lernen --------------------------------------------------------------------

/// Woher ein Lernbeispiel stammt
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Source {
    /// Im Assistenten entschieden: Der Grund stand dabei
    #[default]
    Assistant,
    /// Von Hand abgelehnt, mit Pick markiert oder mit Sternen bewertet
    Hand,
    /// Neben einer Ablehnung gesehen und nicht abgelehnt
    Passed,
}

impl Source {
    /// Wie viel ein Beispiel zählt: Nur im Assistenten ist der Grund sicher.
    fn weight(self) -> f32 {
        match self {
            Source::Assistant => 1.0,
            Source::Hand => 0.5,
            Source::Passed => 0.25,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Sample {
    id: i64,
    reason: Reason,
    severity: f32,
    /// Abgelehnt (Vorschlag richtig bzw. Grenzfall doch aussortiert)
    reject: bool,
    at: i64,
    #[serde(default)]
    source: Source,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct LearnFile {
    version: u32,
    samples: Vec<Sample>,
    /// Vom Assistenten abgelehnte Fotos: Hebt der Nutzer die Ablehnung
    /// später auf, war der Vorschlag falsch.
    applied: Vec<i64>,
    /// Von Hand entschiedene Fotos, die noch ausgewertet werden müssen
    pending: Vec<i64>,
}

/// Was der Nutzer ohne den Assistenten über ein Foto entschieden hat
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hand {
    Reject,
    Keep,
}

/// Abgelehnt schlägt Sterne: Das Foto soll weg, auch wenn es einmal gefiel.
fn hand_of(flag: i64, rating: i64) -> Option<Hand> {
    match flag {
        -1 => Some(Hand::Reject),
        1 => Some(Hand::Keep),
        _ => (rating > 0).then_some(Hand::Keep),
    }
}

/// Was eine Auswertung über ein Foto ergeben hat: (Grund, Schweregrad, abgelehnt)
#[derive(Debug)]
struct Learned {
    id: i64,
    /// Von Hand entschieden (sonst nur gesehen und behalten). Ohne `samples` heißt
    /// das: Das Foto lehrt nichts (mehr), die Entscheidung ist etwa zurückgenommen.
    hand: bool,
    samples: Vec<(Reason, f32, bool)>,
}

/// Eine Ablehnung von Hand lehrt nur den wahrscheinlichsten Grund – den, den auch
/// der Dialog genannt hätte –, und nur, wenn der mindestens ein Grenzfall ist.
/// Behaltene Fotos lehren alle Gründe: Das Foto blieb trotz aller Befunde.
fn hand_samples(
    hand: Hand,
    findings: &[Finding],
    thresholds: &HashMap<Reason, f32>,
) -> Vec<(Reason, f32, bool)> {
    let margin = |f: &Finding| f.severity - thresholds.get(&f.reason).copied().unwrap_or(BASE);
    match hand {
        Hand::Reject => findings
            .iter()
            .max_by(|a, b| margin(a).total_cmp(&margin(b)))
            .filter(|f| margin(f) >= -BORDER)
            .map(|f| (f.reason, f.severity, true))
            .into_iter()
            .collect(),
        Hand::Keep => kept_samples(findings),
    }
}

fn kept_samples(findings: &[Finding]) -> Vec<(Reason, f32, bool)> {
    findings
        .iter()
        .filter(|f| f.severity >= MIN_KEPT_SEVERITY)
        .map(|f| (f.reason, f.severity, false))
        .collect()
}

/// Wertet gemessene Fotos eines Ordners aus (chronologisch). `decided`: die Fotos
/// aus der Warteschlange; die übrigen sind nur Umgebung.
fn learn_from(
    photos: &mut [Photo],
    decided: &HashSet<i64>,
    thresholds: &HashMap<Reason, f32>,
) -> Vec<Learned> {
    let mut hands: HashMap<i64, Hand> = HashMap::new();
    let mut passed: HashSet<i64> = HashSet::new();
    for (pos, p) in photos.iter().enumerate() {
        if !decided.contains(&p.f.id) {
            continue;
        }
        let Some(hand) = hand_of(p.f.flag, p.f.rating) else {
            continue;
        };
        hands.insert(p.f.id, hand);
        if hand == Hand::Reject {
            let near = pos.saturating_sub(PASSED_NEIGHBORS)
                ..(pos + PASSED_NEIGHBORS + 1).min(photos.len());
            passed.extend(
                photos[near]
                    .iter()
                    .filter(|n| hand_of(n.f.flag, n.f.rating).is_none())
                    .map(|n| n.f.id),
            );
        }
    }
    // Wie vor der Entscheidung messen: Geschützte Fotos hätten sonst keine Befunde
    for p in photos.iter_mut() {
        p.f.rating = 0;
        p.f.flag = 0;
        p.f.edited = false;
    }
    let mut findings: HashMap<i64, Vec<Finding>> = evaluate(photos)
        .into_iter()
        .map(|(_, it)| (it.id, it.findings))
        .collect();
    let mut out = Vec::new();
    for (id, hand) in hands {
        let f = findings.remove(&id).unwrap_or_default();
        out.push(Learned {
            id,
            hand: true,
            samples: hand_samples(hand, &f, thresholds),
        });
    }
    for id in passed {
        if let Some(f) = findings.get(&id) {
            out.push(Learned {
                id,
                hand: false,
                samples: kept_samples(f),
            });
        }
    }
    out
}

/// Positionen der Fotos (chronologisch), die für die Entscheidungen an den
/// Positionen `decided` gemessen werden müssen: die Fotos daneben und die Serie,
/// zu der sie gehören könnten.
fn context_of(times: &[Option<i64>], decided: &[usize]) -> BTreeSet<usize> {
    let close = |a: usize, b: usize| matches!((times[a], times[b]), (Some(x), Some(y)) if y - x <= BURST_GAP_S);
    let n = times.len();
    let mut out = BTreeSet::new();
    for &d in decided {
        out.extend(d.saturating_sub(PASSED_NEIGHBORS)..(d + PASSED_NEIGHBORS + 1).min(n));
        let mut i = d;
        for _ in 0..MAX_CHAIN {
            if i == 0 || !close(i - 1, i) {
                break;
            }
            i -= 1;
            out.insert(i);
        }
        let mut i = d;
        for _ in 0..MAX_CHAIN {
            if i + 1 >= n || !close(i, i + 1) {
                break;
            }
            i += 1;
            out.insert(i);
        }
    }
    out
}

impl LearnFile {
    /// Fotos einreihen, jedes nur einmal: Gezählt wird beim Auswerten, was dann gilt.
    fn enqueue(&mut self, ids: Vec<i64>) {
        let mut seen: HashSet<i64> = self.pending.iter().copied().collect();
        self.pending
            .extend(ids.into_iter().filter(|id| seen.insert(*id)));
        let excess = self.pending.len().saturating_sub(MAX_PENDING);
        self.pending.drain(..excess);
    }

    /// Den nächsten Stapel aus der Warteschlange nehmen
    fn take(&mut self, only: Option<&HashSet<i64>>) -> Vec<i64> {
        let mut taken = Vec::new();
        self.pending.retain(|id| {
            let take = taken.len() < LEARN_BATCH && only.is_none_or(|o| o.contains(id));
            if take {
                taken.push(*id);
            }
            !take
        });
        taken
    }

    fn waiting(&self, only: Option<&HashSet<i64>>) -> bool {
        self.pending
            .iter()
            .any(|id| only.is_none_or(|o| o.contains(id)))
    }

    /// Ergebnisse einer Auswertung übernehmen. Die neuere Entscheidung ersetzt die
    /// ältere; nur gesehen zu sein ersetzt nie eine Entscheidung.
    fn apply(&mut self, learned: Vec<Learned>, at: i64) {
        for l in learned {
            // Hat der Assistent abgelehnt: Dafür gibt es ein Beispiel, und eine
            // Rücknahme erkennt `reconcile`
            if self.applied.contains(&l.id) {
                continue;
            }
            let source = if l.hand {
                self.samples.retain(|s| {
                    s.id != l.id
                        || (s.source == Source::Assistant
                            && !l.samples.iter().any(|x| x.0 == s.reason))
                });
                Source::Hand
            } else {
                if self
                    .samples
                    .iter()
                    .any(|s| s.id == l.id && s.source != Source::Passed)
                {
                    continue;
                }
                self.samples.retain(|s| s.id != l.id);
                Source::Passed
            };
            for (reason, severity, reject) in l.samples {
                self.samples.push(Sample {
                    id: l.id,
                    reason,
                    severity,
                    reject,
                    at,
                    source,
                });
            }
        }
        self.trim();
    }

    /// Älteste Beispiele fallen weg, die aus dem Assistenten zuletzt. Hand und
    /// Gegenprobe gemeinsam nach Alter, sonst kippte das Verhältnis der beiden.
    fn trim(&mut self) {
        let mut excess = self.samples.len().saturating_sub(MAX_SAMPLES);
        for assistant in [false, true] {
            self.samples.retain(|s| {
                let drop = excess > 0 && (s.source == Source::Assistant) == assistant;
                if drop {
                    excess -= 1;
                }
                !drop
            });
        }
    }
}

fn load(state: &AppState) -> LearnFile {
    lumenkorn_develop::fsutil::load_json_or_default(&state.data_dir.join(FILE))
}

fn save(state: &AppState, f: &LearnFile) {
    if let Err(e) = lumenkorn_develop::fsutil::save_json(&state.data_dir.join(FILE), f) {
        log::warn!("{FILE} nicht gespeichert: {e}");
    }
}

/// Schwelle aus Entscheidungen (Schweregrad, abgelehnt, Gewicht): die mit den
/// wenigsten Fehlentscheidungen, wobei jede Verschiebung von `base` weg etwas
/// kostet. Unter [`MIN_SAMPLES`] Beispielen (nach Gewicht) bleibt es bei `base`.
pub(crate) fn fit(samples: &[(f32, bool, f32)], base: f32) -> f32 {
    if samples.iter().map(|s| s.2).sum::<f32>() < MIN_SAMPLES as f32 {
        return base;
    }
    let mut best = (f32::MAX, base);
    for step in 15..=90 {
        let t = step as f32 / 100.0;
        let wrong: f32 = samples
            .iter()
            .filter(|(s, reject, _)| (*s >= t) != *reject)
            .map(|s| s.2)
            .sum();
        let cost = wrong + PRIOR_PER_TENTH * (t - base).abs() * 10.0;
        if cost < best.0 - 1e-4
            || ((cost - best.0).abs() <= 1e-4 && (t - base).abs() < (best.1 - base).abs())
        {
            best = (cost, t);
        }
    }
    best.1
}

fn threshold_of(f: &LearnFile, reason: Reason) -> f32 {
    let mine = || f.samples.iter().filter(|x| x.reason == reason);
    let s: Vec<(f32, bool, f32)> = mine()
        .map(|x| (x.severity, x.reject, x.source.weight()))
        .collect();
    let t = fit(&s, BASE);
    let sure = mine().filter(|x| x.source == Source::Assistant).count();
    if sure >= MIN_SAMPLES {
        t
    } else {
        t.clamp(BASE - HAND_SHIFT, BASE + HAND_SHIFT)
    }
}

fn thresholds(f: &LearnFile) -> HashMap<Reason, f32> {
    REASONS.iter().map(|r| (*r, threshold_of(f, *r))).collect()
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
    let pool = crate::album::select::analysis_pool()?;

    let ids: Vec<i64> = images.iter().map(|i| i.id).collect();
    let faces = big_faces(&state.catalog.locked(), &ids)?;
    let mesh = if faces.rects.is_empty() {
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
                    let bytes = state
                        .cache
                        .get_or_render(&state.catalog, Variant::Preview, &job)
                        .ok()?;
                    measure_photo(img, &bytes, &faces, mesh.as_deref())
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
    record_decisions(&mut f, &decisions, unix_now());
    // Die Oberfläche hat diese Fotos eben selbst markiert: keine Entscheidung von Hand
    let ids: HashSet<i64> = decisions.iter().map(|d| d.id).collect();
    QUEUE.locked().retain(|id| !ids.contains(id));
    f.pending.retain(|id| !ids.contains(id));
    save(&state, &f);
}

fn record_decisions(f: &mut LearnFile, decisions: &[Decision], at: i64) {
    for d in decisions {
        // Die neuere Entscheidung zum selben Foto und Grund ersetzt die alte. Was von
        // Hand oder nur nebenbei über das Foto bekannt war, zählt daneben nicht mehr.
        f.samples
            .retain(|s| s.id != d.id || (s.source == Source::Assistant && s.reason != d.reason));
        f.samples.push(Sample {
            id: d.id,
            reason: d.reason,
            severity: d.severity,
            reject: d.reject,
            at,
            source: Source::Assistant,
        });
        if d.reject && !f.applied.contains(&d.id) {
            f.applied.push(d.id);
        }
    }
    f.trim();
    f.version = 2;
}

// --- Entscheidungen ohne den Assistenten ------------------------------------------

/// Der Nutzer hat Fotos in der Bibliothek abgelehnt, markiert oder bewertet.
pub fn note_decisions(ids: &[i64]) {
    QUEUE.locked().extend_from_slice(ids);
    WORK.store(true, Ordering::Relaxed);
}

/// Für den Hausmeister: Liegen Entscheidungen zum Auswerten bereit?
pub fn has_work() -> bool {
    WORK.load(Ordering::Relaxed)
}

/// Ein Stapel im Hintergrund. `true`: Es wartet noch mehr.
pub fn run_batch(app: &AppHandle) -> Result<bool, Error> {
    batch(&app.state::<AppState>(), None)
}

/// Abgelehnte Fotos werden gleich gelöscht: Was davon noch nicht ausgewertet ist,
/// jetzt auswerten, danach ist es nicht mehr möglich. Ein Fehler hier darf das
/// Löschen nicht verhindern.
pub fn learn_before_delete(state: &AppState, ids: &[i64]) {
    let only: HashSet<i64> = ids.iter().copied().collect();
    let until = Instant::now() + DELETE_WAIT;
    let run = std::panic::AssertUnwindSafe(|| {
        while batch(state, Some(&only))? && Instant::now() < until {}
        Ok::<_, Error>(())
    });
    match std::panic::catch_unwind(run) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => log::warn!("Aussortieren: Lernen vor dem Löschen: {e}"),
        Err(_) => log::error!("Aussortieren: Lernen vor dem Löschen abgebrochen (Panik)"),
    }
}

fn batch(state: &AppState, only: Option<&HashSet<i64>>) -> Result<bool, Error> {
    let generation = GENERATION.load(Ordering::Relaxed);
    let (ids, known, more) = {
        let _g = FILE_LOCK.locked();
        // Vor dem Abholen: Eine Meldung ab jetzt setzt es wieder
        WORK.store(false, Ordering::Relaxed);
        let mut f = load(state);
        let fresh = std::mem::take(&mut *QUEUE.locked());
        let changed = !fresh.is_empty();
        f.enqueue(fresh);
        let ids = f.take(only);
        // Schon vor dem Messen aus der Datei: Bringt ein Foto das Programm zu Fall,
        // ist es beim nächsten Start nicht wieder dran.
        if changed || !ids.is_empty() {
            f.version = 2;
            save(state, &f);
        }
        if !f.pending.is_empty() {
            WORK.store(true, Ordering::Relaxed);
        }
        (ids, thresholds(&f), f.waiting(only))
    };
    if ids.is_empty() {
        return Ok(false);
    }
    let sources = Sources {
        catalog: &state.catalog,
        cache: &state.cache,
        ai: crate::ai::enabled(state),
        mesh: &|| crate::ai::face_mesh(state),
    };
    let learned = evaluate_decisions(&sources, &ids, &known)?;
    let _g = FILE_LOCK.locked();
    if GENERATION.load(Ordering::Relaxed) == generation {
        let mut f = load(state);
        f.apply(learned, unix_now());
        save(state, &f);
    }
    Ok(more)
}

/// Was die Auswertung vom Programm braucht
struct Sources<'a> {
    catalog: &'a Mutex<lumenkorn_core::Catalog>,
    cache: &'a lumenkorn_core::thumbs::ThumbCache,
    /// KI eingeschaltet: Gesichtsmodell und CLIP-Vektoren aus dem Katalog nutzen
    ai: bool,
    mesh: &'a dyn Fn() -> Option<std::sync::Arc<lumenkorn_ai::parts::FaceMesh>>,
}

/// Misst die Fotos `ids` mit ihrer Umgebung, ohne Katalogsperre und nacheinander:
/// Das läuft neben der Arbeit des Nutzers her. Ohne Entwicklung (`get_light`): Was
/// sich nur entwickelt messen ließe, lehrt nichts.
fn evaluate_decisions(
    src: &Sources,
    ids: &[i64],
    thresholds: &HashMap<Reason, f32>,
) -> Result<Vec<Learned>, Error> {
    let reader = src.catalog.locked().reader()?;
    let mut out = Vec::new();
    let mut by_dir: HashMap<String, HashSet<i64>> = HashMap::new();
    for img in reader.images_in_order(ids, &ImageQuery::new(Scope::All))? {
        if hand_of(img.flag, img.rating).is_none() {
            // Zurückgenommen: Was das Foto gelehrt hat, gilt nicht mehr
            out.push(Learned {
                id: img.id,
                hand: true,
                samples: Vec::new(),
            });
        } else if !img.edited
            && let Some(dir) = std::path::Path::new(&img.path).parent()
        {
            by_dir
                .entry(dir.to_string_lossy().into_owned())
                .or_default()
                .insert(img.id);
        }
    }
    for (dir, decided) in by_dir {
        let mut q = ImageQuery::new(Scope::Folder {
            path: dir,
            recursive: false,
        });
        q.sort = SortOrder::CapturedAsc;
        let list = reader.query_images(&q)?;
        let times: Vec<Option<i64>> = list
            .iter()
            .map(|i| {
                i.captured_at
                    .as_deref()
                    .and_then(lumenkorn_core::geotime::parse_time)
            })
            .collect();
        let positions: Vec<usize> = (0..list.len())
            .filter(|p| decided.contains(&list[*p].id))
            .collect();
        let images: Vec<_> = context_of(&times, &positions)
            .into_iter()
            .map(|p| &list[p])
            .filter(|i| !i.edited && !i.missing)
            .collect();
        let image_ids: Vec<i64> = images.iter().map(|i| i.id).collect();
        let faces = big_faces(&reader, &image_ids)?;
        let mesh = if src.ai && !faces.rects.is_empty() {
            (src.mesh)()
        } else {
            None
        };
        let mut vectors: HashMap<i64, Vec<f32>> = if src.ai {
            reader
                .embeddings_of(lumenkorn_ai::models::MODEL_ID, &image_ids)?
                .into_iter()
                .map(|(id, bytes)| (id, lumenkorn_ai::index::from_bytes(&bytes)))
                .collect()
        } else {
            HashMap::new()
        };
        let mut photos: Vec<Photo> = images
            .iter()
            .filter_map(|img| {
                let job = reader.thumb_job(img.id).ok()?;
                let bytes = src
                    .cache
                    .get_light(src.catalog, Variant::Preview, &job)
                    .ok()??;
                let mut photo = measure_photo(img, &bytes, &faces, mesh.as_deref())?;
                photo.f.vector = vectors.remove(&img.id);
                Some(photo)
            })
            .collect();
        out.extend(learn_from(&mut photos, &decided, thresholds));
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasonStatus {
    pub reason: Reason,
    /// Entscheidungen (ohne die nur gesehenen und behaltenen Fotos)
    pub samples: usize,
    /// Davon abgelehnt
    pub rejected: usize,
    /// Gelernte Schwelle (Grundschwelle [`BASE`])
    pub threshold: f32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CullStatus {
    /// Lernbeispiele insgesamt
    pub samples: usize,
    /// Davon Entscheidungen ohne den Assistenten
    pub hand: usize,
    /// Fotos, deren Entscheidung noch ausgewertet wird
    pub pending: usize,
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
    let waiting: HashSet<i64> = f
        .pending
        .iter()
        .chain(QUEUE.locked().iter())
        .copied()
        .collect();
    Ok(CullStatus {
        samples: f.samples.len(),
        hand: f
            .samples
            .iter()
            .filter(|s| s.source == Source::Hand)
            .count(),
        pending: waiting.len(),
        base: BASE,
        reasons: REASONS
            .iter()
            .map(|r| {
                let mine = f
                    .samples
                    .iter()
                    .filter(|s| s.reason == *r && s.source != Source::Passed);
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
    GENERATION.fetch_add(1, Ordering::Relaxed);
    QUEUE.locked().clear();
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
        // Entscheidungen aus dem Assistenten zählen voll
        let fit = |s: &[(f32, bool)], base: f32| {
            let s: Vec<(f32, bool, f32)> = s.iter().map(|x| (x.0, x.1, 1.0)).collect();
            fit(&s, base)
        };
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

    // --- Entscheidungen ohne den Assistenten ---

    /// Einzelfoto mit Unschärfe 0,5: kein gemeinsamer Hash, weit auseinander
    fn blurry(id: i64, time: i64) -> Photo {
        photo(
            id,
            time * 1000,
            (id as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15),
            80.0,
        )
    }

    fn base_thresholds() -> HashMap<Reason, f32> {
        REASONS.iter().map(|r| (*r, BASE)).collect()
    }

    fn sample(id: i64, severity: f32, reject: bool, source: Source) -> Sample {
        Sample {
            id,
            reason: Reason::Blur,
            severity,
            reject,
            at: 0,
            source,
        }
    }

    fn finding(reason: Reason, severity: f32) -> Finding {
        Finding { reason, severity }
    }

    #[test]
    fn what_counts_as_a_decision() {
        assert_eq!(hand_of(-1, 0), Some(Hand::Reject));
        assert_eq!(hand_of(-1, 4), Some(Hand::Reject));
        assert_eq!(hand_of(1, 0), Some(Hand::Keep));
        assert_eq!(hand_of(0, 3), Some(Hand::Keep));
        assert_eq!(hand_of(0, 0), None);
    }

    #[test]
    fn a_rejection_teaches_only_the_likely_reason() {
        let t = base_thresholds();
        let f = [
            finding(Reason::Series, 0.6),
            finding(Reason::Blur, 0.9),
            finding(Reason::Eyes, 0.2),
        ];
        assert_eq!(
            hand_samples(Hand::Reject, &f, &t),
            vec![(Reason::Blur, 0.9, true)]
        );
        // Grenzfall zählt noch …
        let f = [finding(Reason::Blur, 0.36)];
        assert_eq!(hand_samples(Hand::Reject, &f, &t).len(), 1);
        // … was weit darunter liegt, war nicht der Grund
        let f = [finding(Reason::Blur, 0.3)];
        assert!(hand_samples(Hand::Reject, &f, &t).is_empty());
        assert!(hand_samples(Hand::Reject, &[], &t).is_empty());
    }

    #[test]
    fn a_kept_photo_teaches_every_reason() {
        let f = [
            finding(Reason::Blur, 0.8),
            finding(Reason::Eyes, 0.2),
            finding(Reason::Exposure, 0.05),
        ];
        assert_eq!(
            hand_samples(Hand::Keep, &f, &base_thresholds()),
            vec![(Reason::Blur, 0.8, false), (Reason::Eyes, 0.2, false)]
        );
    }

    #[test]
    fn protected_photos_are_measured_as_before_the_decision() {
        // Pick und Sterne schützen im Assistenten: Dort hätten sie keinen Befund
        let mut p = vec![blurry(1, 10), blurry(2, 20), blurry(3, 30)];
        p[0].f.flag = 1;
        p[1].f.rating = 3;
        assert!(
            evaluate(&p)
                .iter()
                .filter(|(_, it)| it.id != 3)
                .all(|(_, it)| it.findings.is_empty())
        );
        let learned = learn_from(&mut p, &HashSet::from([1, 2]), &base_thresholds());
        assert_eq!(learned.len(), 2);
        for l in learned {
            assert!(l.hand);
            assert_eq!(l.samples.len(), 1);
            assert_eq!(l.samples[0].0, Reason::Blur);
            assert!(!l.samples[0].2);
        }
    }

    #[test]
    fn rejected_photos_still_count_for_their_series() {
        // Zehn fast gleiche Fotos, die schwächsten von Hand abgelehnt: Für den Assistenten
        // wären das die Vorschläge. Ohne die abgelehnten wäre die Serie viel kleiner.
        let mut p: Vec<Photo> = (0..10)
            .map(|i| photo(i, 1000 + i, 0xF0F0, 1000.0 - i as f32 * 10.0))
            .collect();
        for x in &mut p[4..] {
            x.f.flag = -1;
        }
        let decided: HashSet<i64> = (4..10).collect();
        let learned = learn_from(&mut p, &decided, &base_thresholds());
        let hand: Vec<_> = learned.iter().filter(|l| l.hand).collect();
        assert_eq!(hand.len(), 6);
        for l in hand {
            assert_eq!(l.samples.len(), 1, "Foto {}", l.id);
            assert_eq!(l.samples[0].0, Reason::Series);
            assert!(l.samples[0].2);
        }
    }

    #[test]
    fn photos_next_to_a_rejection_count_as_kept() {
        let mut p: Vec<Photo> = (0..9).map(|i| blurry(i, i * 10)).collect();
        p[4].f.flag = -1;
        // Daneben, aber schon entschieden: kein stilles Behalten
        p[3].f.rating = 2;
        let learned = learn_from(&mut p, &HashSet::from([4]), &base_thresholds());
        let mut passed: Vec<i64> = learned.iter().filter(|l| !l.hand).map(|l| l.id).collect();
        passed.sort_unstable();
        assert_eq!(passed, vec![1, 2, 5, 6, 7]);
        for l in learned.iter().filter(|l| !l.hand) {
            assert_eq!(l.samples.len(), 1);
            assert_eq!(l.samples[0].0, Reason::Blur);
            assert!(!l.samples[0].2);
        }
        // Sterne allein sagen nichts über die Fotos daneben
        let mut p: Vec<Photo> = (0..9).map(|i| blurry(i, i * 10)).collect();
        p[4].f.rating = 5;
        let learned = learn_from(&mut p, &HashSet::from([4]), &base_thresholds());
        assert!(learned.iter().all(|l| l.hand));
    }

    #[test]
    fn context_covers_neighbours_and_the_series() {
        let t = |v: &[i64]| v.iter().map(|x| Some(*x)).collect::<Vec<_>>();
        // Weit auseinander: nur die Nachbarn, am Rand weniger
        let far = t(&[0, 1000, 2000, 3000, 4000, 5000, 6000, 7000, 8000, 9000]);
        assert_eq!(
            context_of(&far, &[4]).into_iter().collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6, 7]
        );
        assert_eq!(
            context_of(&far, &[0]).into_iter().collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        // Kurz hintereinander: die ganze Kette, aber nicht unbegrenzt
        let burst: Vec<i64> = (0..100).map(|i| i * 10).collect();
        let c = context_of(&t(&burst), &[50]);
        assert_eq!(c.len(), 2 * MAX_CHAIN + 1);
        assert_eq!(c.first(), Some(&(50 - MAX_CHAIN)));
        assert_eq!(c.last(), Some(&(50 + MAX_CHAIN)));
        // Ohne Aufnahmezeit: keine Kette
        let none = vec![None; 20];
        assert_eq!(context_of(&none, &[10]).len(), 2 * PASSED_NEIGHBORS + 1);
        assert!(context_of(&[], &[]).is_empty());
    }

    #[test]
    fn a_newer_decision_replaces_the_older_one() {
        let learned = |id, hand, samples| Learned { id, hand, samples };
        let mut f = LearnFile::default();
        // Nur gesehen und behalten …
        f.apply(vec![learned(1, false, vec![(Reason::Blur, 0.6, false)])], 1);
        assert_eq!(f.samples.len(), 1);
        assert_eq!(f.samples[0].source, Source::Passed);
        // … dann doch von Hand abgelehnt
        f.apply(vec![learned(1, true, vec![(Reason::Blur, 0.6, true)])], 2);
        assert_eq!(f.samples.len(), 1);
        assert_eq!(f.samples[0].source, Source::Hand);
        assert!(f.samples[0].reject);
        // Nebenbei gesehen ändert daran nichts mehr
        f.apply(vec![learned(1, false, vec![(Reason::Blur, 0.6, false)])], 3);
        assert!(f.samples[0].reject);
        // Zurückgenommen: Das Foto lehrt nichts mehr
        f.apply(vec![learned(1, true, vec![])], 4);
        assert!(f.samples.is_empty());

        // Entscheidung im Assistenten: bleibt, außer die neue gilt demselben Grund
        let mut f = LearnFile {
            samples: vec![sample(2, 0.7, false, Source::Assistant)],
            ..Default::default()
        };
        f.samples[0].reason = Reason::Eyes;
        f.apply(vec![learned(2, true, vec![(Reason::Blur, 0.6, true)])], 5);
        assert_eq!(f.samples.len(), 2);
        f.apply(vec![learned(2, true, vec![(Reason::Eyes, 0.7, true)])], 6);
        assert_eq!(f.samples.len(), 1);
        assert!(f.samples[0].reject);
        assert_eq!(f.samples[0].source, Source::Hand);
        // Vom Assistenten selbst abgelehnt: kein Beispiel von Hand, obwohl `set_flag` es meldet
        let mut f = LearnFile {
            applied: vec![3],
            ..Default::default()
        };
        f.apply(vec![learned(3, true, vec![(Reason::Blur, 0.6, true)])], 7);
        assert!(f.samples.is_empty());
    }

    #[test]
    fn a_decision_in_the_assistant_replaces_what_was_guessed() {
        let mut f = LearnFile {
            samples: vec![
                sample(1, 0.6, false, Source::Passed),
                sample(2, 0.6, true, Source::Hand),
                sample(3, 0.6, true, Source::Assistant),
            ],
            ..Default::default()
        };
        f.samples[1].reason = Reason::Eyes;
        f.samples[2].reason = Reason::Eyes;
        let d = |id| Decision {
            id,
            reason: Reason::Series,
            severity: 0.7,
            reject: false,
        };
        record_decisions(&mut f, &[d(1), d(2), d(3)], 1);
        // Bei 1 und 2 ist das Geratene weg, bei 3 bleibt der andere Grund aus dem Assistenten
        assert_eq!(f.samples.iter().filter(|s| s.id == 1).count(), 1);
        assert_eq!(f.samples.iter().filter(|s| s.id == 2).count(), 1);
        assert_eq!(f.samples.iter().filter(|s| s.id == 3).count(), 2);
        assert!(f.samples.iter().all(|s| s.source == Source::Assistant));
    }

    #[test]
    fn decisions_by_hand_count_less() {
        let file = |samples: Vec<Sample>| LearnFile {
            samples,
            ..Default::default()
        };
        let n = |count: i64, severity, reject, source| -> Vec<Sample> {
            (0..count)
                .map(|i| sample(i, severity, reject, source))
                .collect()
        };
        // Acht Entscheidungen im Assistenten genügen, acht von Hand noch nicht
        let f = file(n(8, 0.45, true, Source::Assistant));
        assert!(threshold_of(&f, Reason::Blur) < BASE);
        let f = file(n(8, 0.45, true, Source::Hand));
        assert_eq!(threshold_of(&f, Reason::Blur), BASE);
        let f = file(n(16, 0.45, true, Source::Hand));
        assert!(threshold_of(&f, Reason::Blur) < BASE);
        // Von Hand allein nur begrenzt, in beide Richtungen
        let f = file(n(400, 0.17, true, Source::Hand));
        assert_eq!(threshold_of(&f, Reason::Blur), BASE - HAND_SHIFT);
        let f = file(n(400, 0.88, false, Source::Hand));
        assert_eq!(threshold_of(&f, Reason::Blur), BASE + HAND_SHIFT);
        // Mit genug Entscheidungen im Assistenten gilt die Grenze nicht
        let mut s = n(400, 0.88, false, Source::Hand);
        s.extend(n(8, 0.88, false, Source::Assistant));
        assert!(threshold_of(&file(s), Reason::Blur) > BASE + HAND_SHIFT);
    }

    #[test]
    fn the_control_group_keeps_the_threshold_from_sinking() {
        let at = |i: i64| 0.36 + (i % 14) as f32 * 0.01;
        // Abgelehnte Fotos knapp unter der Schwelle: Ohne Gegenprobe sinkt sie …
        let mut samples: Vec<Sample> = (0..40)
            .map(|i| sample(i, at(i), true, Source::Hand))
            .collect();
        let f = LearnFile {
            samples: samples.clone(),
            ..Default::default()
        };
        assert!(threshold_of(&f, Reason::Blur) < BASE - 0.05);
        // … doch wenn gleich unscharfe Fotos daneben meist behalten wurden, bleibt sie
        samples.extend((100..300).map(|i| sample(i, at(i), false, Source::Passed)));
        let f = LearnFile {
            samples,
            ..Default::default()
        };
        assert_eq!(threshold_of(&f, Reason::Blur), BASE);
    }

    #[test]
    fn the_queue_holds_every_photo_once_and_stays_bounded() {
        let mut f = LearnFile::default();
        f.enqueue(vec![1, 2, 2, 3]);
        f.enqueue(vec![3, 4]);
        assert_eq!(f.pending, vec![1, 2, 3, 4]);
        // Nur die gewünschten Fotos, der Rest wartet weiter
        let only = HashSet::from([2, 4]);
        assert_eq!(f.take(Some(&only)), vec![2, 4]);
        assert_eq!(f.pending, vec![1, 3]);
        assert!(!f.waiting(Some(&only)));
        assert!(f.waiting(None));
        // Stapelgröße und Obergrenze
        f.enqueue((10..10 + MAX_PENDING as i64 * 2).collect());
        assert_eq!(f.pending.len(), MAX_PENDING);
        assert_eq!(f.take(None).len(), LEARN_BATCH);
        assert_eq!(f.pending.len(), MAX_PENDING - LEARN_BATCH);
    }

    #[test]
    fn old_samples_go_first_and_those_from_the_assistant_last() {
        let mut f = LearnFile::default();
        f.samples
            .extend((0..10).map(|i| sample(i, 0.6, true, Source::Assistant)));
        f.samples
            .extend((10..MAX_SAMPLES as i64).map(|i| sample(i, 0.6, true, Source::Hand)));
        f.samples
            .extend((0..5).map(|i| sample(90_000 + i, 0.6, false, Source::Passed)));
        f.trim();
        assert_eq!(f.samples.len(), MAX_SAMPLES);
        let count = |source| f.samples.iter().filter(|s| s.source == source).count();
        assert_eq!(count(Source::Assistant), 10);
        // Weg sind die ältesten von Hand, die jüngsten aus der Gegenprobe bleiben
        assert_eq!(count(Source::Passed), 5);
        assert!(!f.samples.iter().any(|s| s.id == 10));
        // Nur aus dem Assistenten: wie bisher die ältesten
        let mut f = LearnFile {
            samples: (0..MAX_SAMPLES as i64 + 3)
                .map(|i| sample(i, 0.6, true, Source::Assistant))
                .collect(),
            ..Default::default()
        };
        f.trim();
        assert_eq!(f.samples.len(), MAX_SAMPLES);
        assert_eq!(f.samples[0].id, 3);
    }

    #[test]
    fn decisions_are_evaluated_from_files_on_disk() {
        use image::{Rgb, RgbImage};
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("Fotos");
        std::fs::create_dir_all(&dir).unwrap();
        // Scharfes Rauschen, jedes Foto anders …
        let noise = |seed: u32| {
            RgbImage::from_fn(800, 600, |x, y| {
                let v = (x * 7 + y * 13 + seed * 101).wrapping_mul(2_654_435_761) >> 24;
                Rgb([v as u8, v as u8, v as u8])
            })
        };
        for i in 0..8 {
            let name = dir.join(format!("{i}.jpg"));
            match i {
                // … und zwei deutlich unscharfe Fotos, die nichts miteinander zu tun haben
                3 => RgbImage::from_fn(800, 600, |x, _| {
                    let v = (x / 4) as u8;
                    Rgb([v, v, v])
                })
                .save(name)
                .unwrap(),
                4 => RgbImage::from_fn(800, 600, |_, y| {
                    let v = (y / 3) as u8;
                    Rgb([v, v, v])
                })
                .save(name)
                .unwrap(),
                _ => noise(i).save(name).unwrap(),
            }
        }
        let cat =
            std::sync::Mutex::new(lumenkorn_core::Catalog::open(&d.path().join("k.db")).unwrap());
        let root = cat.locked().add_root(&dir).unwrap();
        lumenkorn_core::scanner::scan_root(&cat, root.id, &|_| {}).unwrap();
        let mut q = ImageQuery::new(Scope::All);
        q.sort = SortOrder::NameAsc;
        let ids: Vec<i64> = cat
            .locked()
            .query_images(&q)
            .unwrap()
            .iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(ids.len(), 8);
        cat.locked().set_flag(&[ids[3]], -1).unwrap();
        cat.locked().set_rating(&[ids[0]], 4).unwrap();

        let cache = lumenkorn_core::thumbs::ThumbCache::new(d.path().join("cache"));
        let sources = Sources {
            catalog: &cat,
            cache: &cache,
            ai: false,
            mesh: &|| panic!("KI ist aus"),
        };
        // Abgelehnt, mit Sternen, zurückgenommen (6) und ein gelöschtes Foto
        let wanted = [ids[3], ids[0], ids[6], 999_999];
        let learned = evaluate_decisions(&sources, &wanted, &base_thresholds()).unwrap();
        let of = |id: i64, hand: bool| {
            learned
                .iter()
                .find(|l| l.id == id && l.hand == hand)
                .unwrap_or_else(|| panic!("Foto {id}: {learned:?}"))
        };
        // Abgelehnt: Unschärfe war der Grund
        let rejected = &of(ids[3], true).samples;
        assert_eq!(rejected.len(), 1, "{learned:?}");
        assert_eq!(rejected[0].0, Reason::Blur);
        assert!(rejected[0].2);
        // Daneben: unscharf und trotzdem behalten. Die scharfen lehren nichts.
        let passed = &of(ids[4], false).samples;
        assert_eq!(passed.len(), 1, "{learned:?}");
        assert_eq!(passed[0].0, Reason::Blur);
        assert!(!passed[0].2);
        assert!(!learned.iter().any(|l| l.id == ids[4] && l.hand));
        // Sterne auf einem scharfen Foto: Daran gibt es nichts zu lernen
        assert!(of(ids[0], true).samples.is_empty());
        // Entscheidung zurückgenommen: alte Beispiele werden gelöscht
        assert!(of(ids[6], true).samples.is_empty());
        // Das gelöschte Foto fehlt einfach, und die Datei ist nicht angefasst
        assert!(!learned.iter().any(|l| l.id == 999_999));
        assert!(dir.join("3.jpg").is_file());

        // Bearbeitet: Die Vorschau zeigt nicht mehr das Foto, über das entschieden wurde
        let edited = lumenkorn_develop::DevelopParams {
            exposure: 1.0,
            ..Default::default()
        };
        cat.locked().save_develop(&[ids[3]], &edited).unwrap();
        let learned = evaluate_decisions(&sources, &[ids[3]], &base_thresholds()).unwrap();
        assert!(learned.is_empty(), "{learned:?}");
    }

    #[test]
    fn an_older_file_still_loads() {
        // So hat die Vorgängerversion geschrieben
        let f: LearnFile = serde_json::from_str(
            r#"{"version":1,"samples":[{"id":7,"reason":"blur","severity":0.6,"reject":true,"at":5}],"applied":[7]}"#,
        )
        .unwrap();
        assert_eq!(f.samples[0].source, Source::Assistant);
        assert_eq!(f.applied, vec![7]);
        assert!(f.pending.is_empty());
    }

    #[test]
    fn a_measured_preview_becomes_a_photo() {
        let img = image::RgbImage::from_fn(640, 480, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x ^ y) % 256) as u8])
        });
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let summary: lumenkorn_core::ImageSummary = serde_json::from_value(serde_json::json!({
            "id": 42,
            "path": "x.jpg",
            "fileName": "x.jpg",
            "kind": "raster",
            "width": null,
            "height": null,
            "capturedAt": "2026-11-06T10:00:00",
            "rating": 3,
            "flag": 1,
            "thumbReady": true,
            "missing": false,
            "edited": false,
        }))
        .unwrap();
        let p = measure_photo(&summary, &bytes, &BigFaces::default(), None).unwrap();
        assert_eq!((p.f.id, p.f.rating, p.f.flag), (42, 3, 1));
        assert!(p.f.time.is_some());
        assert!((p.f.aspect - 640.0 / 480.0).abs() < 1e-3);
        assert!(!p.raw);
        assert_eq!(p.eyes, None);
        // Kaputte Vorschau: kein Foto, kein Abbruch
        assert!(measure_photo(&summary, b"kaputt", &BigFaces::default(), None).is_none());
    }
}
