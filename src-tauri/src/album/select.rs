//! Fotoalbum, Teil 1: die besten Fotos eines Projekts zu einem Thema finden.
//!
//! Bewertet wird jedes nicht abgelehnte Foto nach
//! - Technik: Schärfe (an der schärfsten Stelle, damit Porträts mit unscharfem
//!   Hintergrund nicht verlieren) und Belichtung, gemessen an der Vorschau;
//! - Nutzer: Sterne, Markierung, Bearbeitung;
//! - Thema: CLIP-Ähnlichkeit zu Beschreibungen des Themas, abzüglich
//!   Bildschirmfotos, Dokumenten und Ähnlichem; bei Familienthemen zählen
//!   Gesichter.
//!
//! Ausgewählt wird nicht einfach nach Punkten: Serien und Dubletten (kurz
//! hintereinander, fast gleiches Bild) zählen als ein Motiv, die Fotos werden
//! über die Ereignisse (Tage, Ausflüge) verteilt, und innerhalb eines Ereignisses
//! wird Abwechslung bevorzugt.

use crate::AppState;
use lumenkorn_core::LockExt;
use lumenkorn_core::thumbs::Variant;
use lumenkorn_core::{Error, ImageQuery, Scope, SortOrder};
use lumenkorn_develop::i18n::tr_s;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tauri::{AppHandle, Emitter};

/// Neues Ereignis, wenn zwischen zwei Fotos mehr als so viel Zeit liegt.
const EVENT_GAP_S: i64 = 3 * 3600;
/// Ähnliche Fotos werden innerhalb dieser Zeit (im selben Ereignis) gesucht.
const SIMILAR_WINDOW_S: i64 = 30 * 60;
/// … und höchstens so viele Fotos zurück.
const SIMILAR_LOOKBACK: usize = 80;
/// Ab dieser CLIP-Ähnlichkeit gilt ein Foto als Wiederholung.
const DUPLICATE_COS: f32 = 0.92;
/// Bildvergleich (gemessen an Handy-Serien und Wiederholungen): Korrelation
/// der Helligkeit (16×16) und mittlere Farbabweichung (4×4), oder höchstens so
/// viele abweichende Bits im Differenz-Hash.
const DUPLICATE_CORR: f32 = 0.6;
const DUPLICATE_COLOR: f32 = 0.07;
const DUPLICATE_HASH_BITS: u32 = 10;
/// Dieselbe Szene mit anderem Ausschnitt (Pixelvergleich versagt): Fotos bis
/// zu diesem Abstand, deren Farbverteilung sich mindestens so weit deckt.
/// Gemessen an Urlaubsfotos: Wiederholungen ≥ 0,63 (meist ≥ 0,7), zufällige
/// Paare aus 2–30 Minuten nur zu 4 % ≥ 0,7.
const SCENE_WINDOW_S: i64 = 10 * 60;
const SCENE_HIST: f32 = 0.7;
/// CLIP innerhalb von [`SCENE_WINDOW_S`]: Wiederholungen meist ≥ 0,88,
/// zufällige Paare aus 2–30 Minuten nur zu 4 %.
const SCENE_COS: f32 = 0.88;
/// Fotos mit mindestens so vielen Sternen kommen immer ins Album.
const MUST_RATING: i64 = 4;
/// Kantenlänge, auf der Schärfe und Belichtung gemessen werden.
const MEASURE_EDGE: u32 = 768;

static CANCEL: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumRequest {
    pub scope: Scope,
    /// Schlüssel aus [`THEMES`] oder „custom“ mit `custom_theme`
    pub theme: String,
    #[serde(default)]
    pub custom_theme: Option<String>,
    /// Gewünschte Anzahl Fotos insgesamt
    pub count: usize,
    /// Nur bearbeitete Fotos und fertige Bilddateien (JPEG …): unbearbeitete
    /// RAWs zeigen nur das flaue Kamerabild und kommen nicht in Frage
    #[serde(default)]
    pub skip_unedited_raw: bool,
}

/// Ein bewertetes Foto. Alle nicht abgelehnten Fotos kommen zurück – die nicht
/// gewählten dienen in der Oberfläche als Ersatz beim Tauschen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumCandidate {
    pub id: i64,
    /// Breite/Höhe, wie das Foto entwickelt aussieht (mit Zuschnitt)
    pub aspect: f32,
    pub captured_at: Option<String>,
    /// Gesamtwertung (höher ist besser, etwa −1…2)
    pub score: f32,
    /// Ereignis (0-basiert, chronologisch)
    pub event: usize,
    /// Motiv: Fotos einer Serie bzw. Dubletten teilen sich die Nummer
    pub group: usize,
    pub picked: bool,
    /// Schärfe relativ zu den übrigen Fotos (0…1)
    pub sharpness: f32,
    /// Mitte der Gesichter (normiert), als Ausschnitt-Vorschlag
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<[f32; 2]>,
    /// Lange Kante des Originals in Pixeln (für die Warnung bei zu kleiner
    /// Auflösung im Druck)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pixels: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSuggestion {
    /// Chronologisch
    pub candidates: Vec<AlbumCandidate>,
    /// Wurde das Thema berücksichtigt (KI eingerichtet und Fotos analysiert)?
    pub theme_used: bool,
    /// Fotos ohne KI-Analyse (zählen nur nach Qualität)
    pub unanalyzed: usize,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress {
    done: usize,
    total: usize,
}

/// Themen mit englischen Beschreibungen für CLIP. Personenthemen gewichten
/// Gesichter.
pub(crate) struct Theme {
    pub key: &'static str,
    pub prompts: &'static [&'static str],
    pub people: bool,
}

pub(crate) const THEMES: &[Theme] = &[
    Theme {
        key: "vacation",
        prompts: &[
            "a holiday photo at the beach",
            "a vacation snapshot of people having fun",
            "a scenic holiday landscape",
            "a swimming pool at a holiday resort",
            "a sunset by the sea",
        ],
        people: false,
    },
    Theme {
        key: "travel",
        prompts: &[
            "a travel photo of a famous landmark",
            "a scenic landscape view",
            "a street in an old town",
            "local food at a market",
            "mountains and a lake",
        ],
        people: false,
    },
    Theme {
        key: "wedding",
        prompts: &[
            "a bride and groom",
            "a wedding ceremony",
            "a wedding couple kissing",
            "wedding guests celebrating",
            "a wedding cake",
        ],
        people: true,
    },
    Theme {
        key: "birthday",
        prompts: &[
            "a birthday party",
            "a birthday cake with candles",
            "people celebrating a birthday",
            "a child opening presents",
        ],
        people: true,
    },
    Theme {
        key: "family",
        prompts: &[
            "a happy family",
            "children playing",
            "grandparents with their grandchildren",
            "a family having dinner together",
        ],
        people: true,
    },
    Theme {
        key: "baby",
        prompts: &[
            "a newborn baby",
            "a smiling baby",
            "parents holding their baby",
            "a toddler playing",
        ],
        people: true,
    },
    Theme {
        key: "nature",
        prompts: &[
            "a beautiful landscape",
            "a forest",
            "mountains",
            "a close-up of a flower",
            "a wild animal in nature",
            "a sunset",
        ],
        people: false,
    },
    Theme {
        key: "party",
        prompts: &[
            "a party with friends",
            "people dancing",
            "friends celebrating together",
            "a concert",
        ],
        people: true,
    },
    Theme {
        key: "sport",
        prompts: &[
            "sports action",
            "a person running",
            "a football match",
            "skiing",
            "cycling",
        ],
        people: false,
    },
    Theme {
        key: "animals",
        prompts: &["a dog", "a cat", "a wild animal", "a bird", "a horse"],
        people: false,
    },
];

/// Fotos, die in kaum ein Album gehören.
const NEGATIVES: &[&str] = &[
    "a screenshot",
    "a photo of a document",
    "a receipt",
    "a whiteboard",
    "a photo of a sign with text",
    "an accidental photo of the floor",
    "a very blurry photo",
];

/// Messwerte eines Fotos, bevor gewertet wird.
#[derive(Debug, Clone, Default)]
pub(crate) struct Features {
    pub id: i64,
    pub time: Option<i64>,
    pub captured_at: Option<String>,
    pub aspect: f32,
    /// Schärfe an der schärfsten Stelle (Laplace, beliebige Einheit)
    pub sharp: f32,
    /// Anteil fast weißer bzw. fast schwarzer Pixel
    pub clip_hi: f32,
    pub clip_lo: f32,
    /// mittlere Helligkeit 0…1
    pub mean: f32,
    /// Differenz-Hash 8×8
    pub hash: u64,
    /// Helligkeit 16×16, mittelwertfrei und auf Länge 1 normiert
    pub luma: Vec<f32>,
    /// Farbe 4×4 (RGB, 0…1)
    pub color: Vec<f32>,
    /// Farbverteilung (Farbton × Sättigung × Helligkeit), Summe 1
    pub hist: Vec<f32>,
    pub rating: i64,
    pub flag: i64,
    pub edited: bool,
    /// Gesichter mit nennenswerter Größe, davon benannt
    pub faces: u32,
    pub named: bool,
    /// Erkannte Personen (sortiert); andere Personen machen ein ähnliches Foto
    /// zu einem eigenen Motiv
    pub people: Vec<i64>,
    pub focus: Option<[f32; 2]>,
    pub pixels: Option<u32>,
    pub theme: Option<f32>,
    pub negative: Option<f32>,
    pub vector: Option<Vec<f32>>,
}

/// Schärfe, Belichtung, Hash aus einem Graustufenbild.
pub(crate) fn measure(gray: &image::GrayImage) -> (f32, f32, f32, f32, u64) {
    let (w, h) = gray.dimensions();
    let px = |x: u32, y: u32| gray.get_pixel(x, y)[0] as f32;
    let n = (w * h).max(1) as f32;
    let (mut hi, mut lo, mut sum) = (0u32, 0u32, 0f64);
    for p in gray.pixels() {
        let v = p[0];
        hi += (v >= 250) as u32;
        lo += (v <= 5) as u32;
        sum += v as f64;
    }
    // Laplace-Varianz je Kachel (6×6), Mittel der schärfsten 4 Kacheln
    const T: u32 = 6;
    let mut tiles = Vec::with_capacity((T * T) as usize);
    if w >= 3 * T && h >= 3 * T {
        for ty in 0..T {
            for tx in 0..T {
                let (x0, x1) = ((tx * w / T).max(1), ((tx + 1) * w / T).min(w - 1));
                let (y0, y1) = ((ty * h / T).max(1), ((ty + 1) * h / T).min(h - 1));
                let (mut s, mut s2, mut k) = (0f64, 0f64, 0f64);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let l = 4.0 * px(x, y)
                            - px(x - 1, y)
                            - px(x + 1, y)
                            - px(x, y - 1)
                            - px(x, y + 1);
                        s += l as f64;
                        s2 += (l * l) as f64;
                        k += 1.0;
                    }
                }
                if k > 0.0 {
                    tiles.push((s2 / k - (s / k).powi(2)) as f32);
                }
            }
        }
    }
    tiles.sort_by(|a, b| b.total_cmp(a));
    let top = tiles.len().clamp(1, 4);
    let sharp = tiles.iter().take(top).sum::<f32>() / top as f32;
    (
        sharp,
        hi as f32 / n,
        lo as f32 / n,
        (sum / n as f64 / 255.0) as f32,
        dhash(gray),
    )
}

/// Bildsignatur für den Vergleich ähnlicher Fotos: Helligkeit 16×16
/// (mittelwertfrei, Länge 1) und Farbe 4×4.
pub(crate) fn signature(img: &image::DynamicImage) -> (Vec<f32>, Vec<f32>) {
    use image::imageops::FilterType;
    let g = img.resize_exact(16, 16, FilterType::Triangle).to_luma8();
    let mut luma: Vec<f32> = g.pixels().map(|p| p[0] as f32).collect();
    let mean = luma.iter().sum::<f32>() / luma.len() as f32;
    luma.iter_mut().for_each(|v| *v -= mean);
    let norm = luma.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 1e-3 {
        luma.iter_mut().for_each(|v| *v /= norm);
    }
    let c = img.resize_exact(4, 4, FilterType::Triangle).to_rgb8();
    let color = c
        .pixels()
        .flat_map(|p| p.0.map(|v| v as f32 / 255.0))
        .collect();
    (luma, color)
}

/// Farbverteilung: 12 Farbtöne × 3 Sättigungen × 3 Helligkeiten, Summe 1.
/// Unabhängig vom Ausschnitt – erkennt dieselbe Szene leicht anders
/// fotografiert.
pub(crate) fn color_hist(img: &image::DynamicImage) -> Vec<f32> {
    let small = img.thumbnail(256, 256).to_rgb8();
    let mut h = vec![0f32; 108];
    for p in small.pixels() {
        let [r, g, b] = p.0.map(|v| v as f32 / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let sat = if max > 0.0 { d / max } else { 0.0 };
        let hue = if d <= 0.0 {
            0.0
        } else if max == r {
            ((g - b) / d).rem_euclid(6.0) / 6.0
        } else if max == g {
            ((b - r) / d + 2.0) / 6.0
        } else {
            ((r - g) / d + 4.0) / 6.0
        };
        let bin = |v: f32, n: usize| ((v * n as f32) as usize).min(n - 1);
        h[bin(hue, 12) * 9 + bin(sat, 3) * 3 + bin(max, 3)] += 1.0;
    }
    let n = h.iter().sum::<f32>().max(1.0);
    h.iter_mut().for_each(|v| *v /= n);
    h
}

/// Überdeckung zweier Farbverteilungen (0…1).
fn hist_overlap(a: &Features, b: &Features) -> f32 {
    a.hist.iter().zip(&b.hist).map(|(x, y)| x.min(*y)).sum()
}

/// Differenz-Hash: 9×8 verkleinert, Vergleich benachbarter Pixel.
fn dhash(gray: &image::GrayImage) -> u64 {
    let s = image::imageops::resize(gray, 9, 8, image::imageops::FilterType::Triangle);
    let mut h = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            h = (h << 1) | (s.get_pixel(x, y)[0] > s.get_pixel(x + 1, y)[0]) as u64;
        }
    }
    h
}

/// Rang jedes Wertes in 0…1 (0 = kleinster).
fn ranks(v: &[f32]) -> Vec<f32> {
    let mut idx: Vec<usize> = (0..v.len()).collect();
    idx.sort_by(|a, b| v[*a].total_cmp(&v[*b]));
    let mut r = vec![0.5; v.len()];
    if v.len() > 1 {
        for (k, i) in idx.into_iter().enumerate() {
            r[i] = k as f32 / (v.len() - 1) as f32;
        }
    }
    r
}

/// Gesamtwertung je Foto (gleiche Reihenfolge wie `f`) und relative Schärfe.
pub(crate) fn score(f: &[Features], people: bool) -> (Vec<f32>, Vec<f32>) {
    let sharp = ranks(&f.iter().map(|x| x.sharp).collect::<Vec<_>>());
    // Thema als z-Wert: CLIP-Kosinus liegen eng beieinander, nur der Abstand
    // zu den übrigen Fotos des Projekts sagt etwas
    let th: Vec<f32> = f.iter().filter_map(|x| x.theme).collect();
    let (mean, sd) = if th.len() >= 2 {
        let m = th.iter().sum::<f32>() / th.len() as f32;
        let v = th.iter().map(|t| (t - m).powi(2)).sum::<f32>() / th.len() as f32;
        (m, v.sqrt().max(1e-3))
    } else {
        (0.0, 1.0)
    };
    let s = f
        .iter()
        .zip(&sharp)
        .map(|(x, &q)| {
            let mut s = 0.45 * q;
            if q < 0.1 {
                s -= 0.15;
            }
            // Belichtung: großflächig ausgefressen oder abgesoffen
            s -= ((x.clip_hi - 0.08).max(0.0) * 2.0).min(0.3);
            s -= ((x.clip_lo - 0.20).max(0.0) * 1.5).min(0.3);
            if !(0.1..=0.9).contains(&x.mean) {
                s -= 0.15;
            }
            s += 0.08 * x.rating.clamp(0, 5) as f32;
            if x.flag == 1 {
                s += 0.25;
            }
            if x.edited {
                s += 0.08;
            }
            if let Some(t) = x.theme {
                s += 0.15 * ((t - mean) / sd).clamp(-2.0, 2.0);
                // Eher Bildschirmfoto oder Dokument als Thema
                if x.negative.is_some_and(|n| n > t + 0.01) {
                    s -= 0.4;
                }
            }
            if people && x.faces > 0 {
                s += 0.15 + if x.named { 0.05 } else { 0.0 };
            }
            s
        })
        .collect();
    (s, sharp)
}

/// Gleiche Ausrichtung (beide quer, beide hoch oder ähnlich): Ein Hoch- und
/// ein Querformat sind nie dasselbe Bild, auch wenn die Farben passen.
fn same_shape(a: &Features, b: &Features) -> bool {
    let r = a.aspect.max(0.01) / b.aspect.max(0.01);
    (1.0 / 1.25..=1.25).contains(&r)
}

/// Korrelation der Helligkeit (−1…1).
fn luma_corr(a: &Features, b: &Features) -> f32 {
    a.luma.iter().zip(&b.luma).map(|(x, y)| x * y).sum()
}

fn color_diff(a: &Features, b: &Features) -> f32 {
    let n = a.color.len().min(b.color.len()).max(1) as f32;
    a.color
        .iter()
        .zip(&b.color)
        .map(|(x, y)| (x - y).abs())
        .sum::<f32>()
        / n
}

/// Andere Personen auf dem Bild: andere Zahl von Gesichtern oder andere
/// erkannte Personen. Nur wenn auf beiden Fotos Gesichter gefunden wurden –
/// fehlen sie auf einem, ist es oft nur noch nicht analysiert (oder das
/// Gesicht abgewandt).
fn different_people(a: &Features, b: &Features) -> bool {
    a.faces > 0
        && b.faces > 0
        && (a.faces != b.faces
            || (!a.people.is_empty() && !b.people.is_empty() && a.people != b.people))
}

/// Ähnlichkeit zweier Fotos 0…1 (für Abwechslung innerhalb eines Ereignisses).
fn similarity(a: &Features, b: &Features) -> f32 {
    let pixel = if same_shape(a, b) {
        (luma_corr(a, b) - color_diff(a, b) * 2.0).max(hist_overlap(a, b) - 0.1)
    } else {
        0.0
    };
    let clip = match (&a.vector, &b.vector) {
        (Some(x), Some(y)) => lumenkorn_ai::clip::dot(x, y),
        _ => 0.0,
    };
    pixel.max(clip)
}

/// Fast dasselbe Foto: gleicher Ausschnitt (Bildvergleich) oder laut CLIP
/// derselbe Inhalt – außer es sind andere Personen zu sehen.
pub(crate) fn is_duplicate(a: &Features, b: &Features) -> bool {
    if different_people(a, b) {
        return false;
    }
    let close = matches!((a.time, b.time), (Some(x), Some(y)) if (y - x).abs() <= SCENE_WINDOW_S);
    let cos = match (&a.vector, &b.vector) {
        (Some(x), Some(y)) => lumenkorn_ai::clip::dot(x, y),
        _ => 0.0,
    };
    // Laut CLIP derselbe Inhalt: auch als Hoch- und Querformat dasselbe Motiv
    if close && cos >= DUPLICATE_COS {
        return true;
    }
    if !same_shape(a, b) {
        return false;
    }
    // Kurz hintereinander reicht weniger Übereinstimmung
    let clip = cos >= if close { SCENE_COS } else { DUPLICATE_COS };
    let pixel = (luma_corr(a, b) >= DUPLICATE_CORR && color_diff(a, b) <= DUPLICATE_COLOR)
        || (a.hash ^ b.hash).count_ones() <= DUPLICATE_HASH_BITS;
    let scene = close && hist_overlap(a, b) >= SCENE_HIST;
    clip || pixel || scene
}

fn find(parent: &mut [usize], i: usize) -> usize {
    let mut r = i;
    while parent[r] != r {
        r = parent[r];
    }
    let mut k = i;
    while parent[k] != r {
        let next = parent[k];
        parent[k] = r;
        k = next;
    }
    r
}

/// Ereignisse und Motivgruppen. `f` ist chronologisch sortiert (ohne Zeit am
/// Ende). Eine Gruppe sind fast gleiche Fotos desselben Ereignisses, auch wenn
/// andere dazwischen liegen (z. B. dieselbe Schaukel vor und nach einem
/// anderen Motiv).
pub(crate) fn groups(f: &[Features]) -> (Vec<usize>, Vec<usize>) {
    let mut event = vec![0; f.len()];
    let mut ev = 0;
    for i in 1..f.len() {
        let gap = match (f[i - 1].time, f[i].time) {
            (Some(a), Some(b)) => b - a,
            (None, None) => 0,
            _ => i64::MAX,
        };
        if gap > EVENT_GAP_S {
            ev += 1;
        }
        event[i] = ev;
    }
    let mut parent: Vec<usize> = (0..f.len()).collect();
    for i in 0..f.len() {
        for j in (i.saturating_sub(SIMILAR_LOOKBACK)..i).rev() {
            if event[j] != event[i] {
                break;
            }
            if let (Some(a), Some(b)) = (f[j].time, f[i].time)
                && b - a > SIMILAR_WINDOW_S
            {
                break;
            }
            if is_duplicate(&f[j], &f[i]) {
                let (a, b) = (find(&mut parent, j), find(&mut parent, i));
                parent[b.max(a)] = a.min(b);
            }
        }
    }
    // Gruppennummern fortlaufend in Reihenfolge des ersten Fotos
    let mut ids: HashMap<usize, usize> = HashMap::new();
    let group = (0..f.len())
        .map(|i| {
            let r = find(&mut parent, i);
            let n = ids.len();
            *ids.entry(r).or_insert(n)
        })
        .collect();
    (event, group)
}

/// Wählt `count` Fotos: alle mit mindestens [`MUST_RATING`] Sternen (auch wenn
/// es dadurch mehr werden), dazu je Motivgruppe höchstens eins, verteilt über
/// die Ereignisse, mit Abwechslung. Liefert die Indizes der gewählten Fotos.
pub(crate) fn pick(
    f: &[Features],
    scores: &[f32],
    event: &[usize],
    group: &[usize],
    count: usize,
) -> Vec<usize> {
    if f.is_empty() {
        return vec![];
    }
    let must: Vec<usize> = (0..f.len())
        .filter(|&i| f[i].rating >= MUST_RATING)
        .collect();
    let covered: std::collections::HashSet<usize> = must.iter().map(|&i| group[i]).collect();
    // Bestes Foto je Gruppe
    let mut best: HashMap<usize, usize> = HashMap::new();
    for i in 0..f.len() {
        let e = best.entry(group[i]).or_insert(i);
        if scores[i] > scores[*e] {
            *e = i;
        }
    }
    let mut reps: Vec<usize> = best.into_values().collect();
    reps.sort_unstable();
    let n_events = event.iter().max().map_or(0, |m| m + 1);
    // Gewicht eines Ereignisses: Wurzel der Motivzahl (lange Tage sollen nicht
    // alles verdrängen) mal Qualität der besten drei Motive
    let mut all_by_event: Vec<Vec<usize>> = vec![vec![]; n_events];
    for &i in &reps {
        all_by_event[event[i]].push(i);
    }
    let weight: Vec<f64> = all_by_event
        .iter()
        .map(|c| {
            if c.is_empty() {
                return 0.0;
            }
            let mut s: Vec<f32> = c.iter().map(|&i| scores[i]).collect();
            s.sort_by(|a, b| b.total_cmp(a));
            let top = s.iter().take(3).sum::<f32>() / s.len().min(3) as f32;
            (c.len() as f64).sqrt() * (0.5 + top.max(-0.4) as f64)
        })
        .collect();
    // Frei wählbar: Motive, die noch kein Pflichtfoto haben
    let mut by_event: Vec<Vec<usize>> = vec![vec![]; n_events];
    for &i in reps.iter().filter(|&&i| !covered.contains(&group[i])) {
        by_event[event[i]].push(i);
    }
    let mut must_in = vec![0usize; n_events];
    for &i in &must {
        must_in[event[i]] += 1;
    }
    let free: usize = by_event.iter().map(Vec::len).sum();
    let rest = count.saturating_sub(must.len()).min(free);
    let total: f64 = weight.iter().sum::<f64>().max(1e-9);
    // Anteil je Ereignis an der Gesamtzahl, abzüglich der Pflichtfotos dort
    let mut quota: Vec<usize> = (0..n_events)
        .map(|e| {
            let share = (count as f64 * weight[e] / total).floor() as usize;
            share.saturating_sub(must_in[e]).min(by_event[e].len())
        })
        .collect();
    let mut assigned: usize = quota.iter().sum();
    while assigned > rest {
        // Durch die Pflichtfotos zu viel: beim größten Überschuss kürzen
        let e = (0..n_events)
            .filter(|&e| quota[e] > 0)
            .max_by(|a, b| {
                ((quota[*a] + must_in[*a]) as f64 / weight[*a].max(1e-9))
                    .total_cmp(&((quota[*b] + must_in[*b]) as f64 / weight[*b].max(1e-9)))
            })
            .expect("Quote vorhanden");
        quota[e] -= 1;
        assigned -= 1;
    }
    // Jedes Ereignis ohne Foto bekommt eines, solange es reicht (die besten zuerst)
    let mut order: Vec<usize> = (0..n_events).filter(|&e| !by_event[e].is_empty()).collect();
    order.sort_by(|a, b| weight[*b].total_cmp(&weight[*a]));
    for &e in &order {
        if quota[e] + must_in[e] == 0 && assigned < rest {
            quota[e] = 1;
            assigned += 1;
        }
    }
    // Rest dort, wo das Verhältnis Fotos/Gewicht am kleinsten ist
    while assigned < rest {
        let Some(e) = (0..n_events)
            .filter(|&e| quota[e] < by_event[e].len())
            .min_by(|a, b| {
                ((quota[*a] + must_in[*a]) as f64 / weight[*a].max(1e-9))
                    .total_cmp(&((quota[*b] + must_in[*b]) as f64 / weight[*b].max(1e-9)))
            })
        else {
            break;
        };
        quota[e] += 1;
        assigned += 1;
    }
    // Innerhalb des Ereignisses: Wertung minus Ähnlichkeit zu schon gewählten
    // (auch zu den Pflichtfotos)
    let mut chosen = must.clone();
    for (e, cands) in by_event.iter().enumerate() {
        let mut left = cands.clone();
        let mut mine: Vec<usize> = must.iter().copied().filter(|&i| event[i] == e).collect();
        for _ in 0..quota[e] {
            let (k, _) = left
                .iter()
                .enumerate()
                .map(|(k, &i)| {
                    let sim = mine
                        .iter()
                        .filter(|&&j| !different_people(&f[i], &f[j]))
                        .map(|&j| similarity(&f[i], &f[j]))
                        .fold(0f32, f32::max);
                    // Unter ~0,5 gilt als verschieden, ab ~0,85 als fast gleich
                    let penalty = ((sim - 0.5) / 0.35).clamp(0.0, 1.0) * 0.6;
                    (k, scores[i] - penalty)
                })
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .expect("Quote ≤ Kandidaten");
            let i = left.swap_remove(k);
            mine.push(i);
            chosen.push(i);
        }
    }
    chosen.sort_unstable();
    chosen
}

/// Bewertet die Fotos eines Projekts und wählt die besten zum Thema.
/// Fortschritt per `album-progress`.
#[tauri::command]
pub async fn album_suggest(
    app: AppHandle,
    request: AlbumRequest,
) -> Result<AlbumSuggestion, Error> {
    CANCEL.store(false, Ordering::Relaxed);
    let emitter = app.clone();
    crate::blocking(app, move |state| suggest(state, &emitter, request)).await
}

#[tauri::command]
pub fn album_suggest_cancel() {
    CANCEL.store(true, Ordering::Relaxed);
}

/// Kamera-RAW ohne Bearbeitung.
fn is_unedited_raw(i: &lumenkorn_core::ImageSummary) -> bool {
    i.kind == lumenkorn_core::formats::ImageKind::Raw && !i.edited
}

fn canceled() -> Error {
    Error::Invalid(tr_s("Abgebrochen", "Canceled").into())
}

fn suggest(state: &AppState, app: &AppHandle, req: AlbumRequest) -> Result<AlbumSuggestion, Error> {
    let images = {
        let mut q = ImageQuery::new(req.scope.clone());
        q.sort = SortOrder::CapturedAsc;
        state.catalog.locked().query_images(&q)?
    };
    let images: Vec<_> = images
        .into_iter()
        .filter(|i| i.flag != -1 && !(req.skip_unedited_raw && is_unedited_raw(i)))
        .collect();
    let total = images.len();
    let done = AtomicUsize::new(0);
    let _ = app.emit("album-progress", Progress { done: 0, total });
    // Vorschauen messen (fehlende werden dabei erzeugt). Eigener kleiner Pool:
    // RAW-Vorschauen brauchen viel Speicher.
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .clamp(2, 6),
        )
        .build()
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let measured: Vec<Option<Features>> = pool.install(|| {
        images
            .par_iter()
            .map(|img| {
                if CANCEL.load(Ordering::Relaxed) {
                    return None;
                }
                let r = (|| -> Option<Features> {
                    let job = state.catalog.locked().thumb_job(img.id).ok()?;
                    let bytes = state.cache.get_or_render(Variant::Preview, &job).ok()?;
                    let dyn_img = image::load_from_memory(&bytes).ok()?;
                    let small = dyn_img.thumbnail(MEASURE_EDGE, MEASURE_EDGE);
                    let gray = small.to_luma8();
                    let (sharp, clip_hi, clip_lo, mean, hash) = measure(&gray);
                    let (luma, color) = signature(&small);
                    let hist = color_hist(&small);
                    Some(Features {
                        id: img.id,
                        time: img
                            .captured_at
                            .as_deref()
                            .and_then(lumenkorn_core::geotime::parse_time),
                        captured_at: img.captured_at.clone(),
                        aspect: dyn_img.width().max(1) as f32 / dyn_img.height().max(1) as f32,
                        sharp,
                        clip_hi,
                        clip_lo,
                        mean,
                        hash,
                        luma,
                        color,
                        hist,
                        rating: img.rating,
                        flag: img.flag,
                        edited: img.edited,
                        pixels: img
                            .width
                            .max(img.height)
                            .and_then(|p| u32::try_from(p).ok()),
                        ..Default::default()
                    })
                })();
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n.is_multiple_of(8) || n == total {
                    let _ = app.emit("album-progress", Progress { done: n, total });
                }
                r
            })
            .collect()
    });
    if CANCEL.load(Ordering::Relaxed) {
        return Err(canceled());
    }
    let mut f: Vec<Features> = measured.into_iter().flatten().collect();
    if f.is_empty() {
        return Err(Error::Invalid(
            tr_s(
                "Hier gibt es keine Fotos für ein Album.",
                "There are no photos here for an album.",
            )
            .into(),
        ));
    }
    // Gesichter: Anzahl, benannte, Mitte als Ausschnitt-Vorschlag. Die Lage
    // bezieht sich aufs unbearbeitete Bild – bei zugeschnittenen Fotos nur ein
    // grober Anhaltspunkt, die Oberfläche nimmt ihn nur ohne Zuschnitt.
    let ids: Vec<i64> = f.iter().map(|x| x.id).collect();
    let faces = state.catalog.locked().faces_of_images(&ids)?;
    let mut by_image: HashMap<i64, Vec<&lumenkorn_core::people::FaceInfo>> = HashMap::new();
    for face in &faces {
        // Kleine Gesichter im Hintergrund zählen nicht
        if face.rect[2] * face.rect[3] >= 0.004 {
            by_image.entry(face.image_id).or_default().push(face);
        }
    }
    for x in &mut f {
        if let Some(list) = by_image.get(&x.id) {
            x.faces = list.len() as u32;
            x.named = list.iter().any(|fc| fc.person_name.is_some());
            let mut people: Vec<i64> = list.iter().filter_map(|fc| fc.person_id).collect();
            people.sort_unstable();
            people.dedup();
            x.people = people;
            let n = list.len() as f64;
            let cx = list
                .iter()
                .map(|fc| fc.rect[0] + fc.rect[2] / 2.0)
                .sum::<f64>()
                / n;
            let cy = list
                .iter()
                .map(|fc| fc.rect[1] + fc.rect[3] / 2.0)
                .sum::<f64>()
                / n;
            x.focus = Some([cx as f32, cy as f32]);
        }
    }
    // Thema über CLIP
    let theme = THEMES.iter().find(|t| t.key == req.theme);
    let prompts: Vec<String> = match (theme, req.custom_theme.as_deref().map(str::trim)) {
        (Some(t), _) => t.prompts.iter().map(|s| s.to_string()).collect(),
        (None, Some(c)) if !c.is_empty() => {
            let en = lumenkorn_ai::vocab::translate_query(c);
            vec![format!("a photo of {en}"), en]
        }
        _ => vec![],
    };
    let people = theme.is_some_and(|t| t.people);
    let clip = match crate::ai::theme_scores(state, &prompts, NEGATIVES, &ids) {
        Ok(s) => s,
        Err(e) => {
            // Ohne KI geht es auch, nur ohne Thema
            log::warn!("Fotoalbum: KI nicht nutzbar: {e}");
            None
        }
    };
    let mut unanalyzed = f.len();
    let mut theme_used = false;
    if let Some(mut c) = clip {
        unanalyzed = f.iter().filter(|x| !c.vectors.contains_key(&x.id)).count();
        theme_used = !prompts.is_empty();
        for x in &mut f {
            x.theme = c.theme.get(&x.id).copied();
            x.negative = c.negative.get(&x.id).copied();
            x.vector = c.vectors.remove(&x.id);
        }
    }
    let (scores, sharp) = score(&f, people);
    let (event, group) = groups(&f);
    let chosen = pick(&f, &scores, &event, &group, req.count.max(1));
    let mut picked = vec![false; f.len()];
    for i in chosen {
        picked[i] = true;
    }
    log::info!(
        "Fotoalbum „{}“: {} von {} Fotos gewählt, {} Ereignisse, Thema {}",
        req.theme,
        picked.iter().filter(|p| **p).count(),
        f.len(),
        event.last().map_or(0, |e| e + 1),
        if theme_used { "ja" } else { "nein" }
    );
    Ok(AlbumSuggestion {
        candidates: f
            .into_iter()
            .enumerate()
            .map(|(i, x)| AlbumCandidate {
                id: x.id,
                aspect: x.aspect,
                captured_at: x.captured_at,
                score: scores[i],
                event: event[i],
                group: group[i],
                picked: picked[i],
                sharpness: sharp[i],
                focus: x.focus,
                pixels: x.pixels,
            })
            .collect(),
        theme_used,
        unanalyzed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(id: i64, time: i64, hash: u64, sharp: f32) -> Features {
        // Signatur aus dem Hash: gleiche Hashes ergeben gleiche Bilder
        let mut luma: Vec<f32> = (0..64)
            .map(|b| if hash >> b & 1 == 1 { 1.0 } else { -1.0 })
            .collect();
        let n = luma.iter().map(|v| v * v).sum::<f32>().sqrt();
        luma.iter_mut().for_each(|v| *v /= n);
        Features {
            id,
            time: Some(time),
            aspect: 1.5,
            sharp,
            mean: 0.5,
            hash,
            luma,
            color: vec![0.5; 48],
            // Eine Farbe je Hash: gleiche Hashes decken sich, andere kaum
            hist: (0..108)
                .map(|b| if b == (hash % 108) as usize { 1.0 } else { 0.0 })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn sharpness_prefers_detail() {
        let flat = image::GrayImage::from_pixel(120, 90, image::Luma([128]));
        let mut detail = flat.clone();
        for (x, y, p) in detail.enumerate_pixels_mut() {
            p[0] = if (x / 2 + y / 2) % 2 == 0 { 30 } else { 220 };
        }
        let a = measure(&flat);
        let b = measure(&detail);
        assert!(b.0 > a.0 * 10.0 + 1.0, "{} vs {}", b.0, a.0);
        assert!((a.3 - 128.0 / 255.0).abs() < 0.01);
        assert_eq!(a.1, 0.0);
    }

    #[test]
    fn similar_photos_group_unless_other_people() {
        let a = photo(1, 0, 0xF0F0_F0F0_0F0F_0F0F, 1.0);
        let mut b = photo(2, 600, 0xF0F0_F0F0_0F0F_0F0E, 1.0);
        let c = photo(3, 300, 0x1234_5678_9ABC_DEF0, 1.0);
        // Gleiches Motiv mit einem anderen Foto dazwischen, 10 Minuten später
        let (_, g) = groups(&[a.clone(), c.clone(), b.clone()]);
        assert_eq!(g[0], g[2]);
        assert_ne!(g[0], g[1]);
        // Dasselbe Motiv mit einer anderen Person ist ein eigenes Foto
        let mut a = a;
        a.faces = 1;
        a.people = vec![3];
        b.faces = 1;
        b.people = vec![7];
        let (_, g) = groups(&[a.clone(), c.clone(), b.clone()]);
        assert_ne!(g[0], g[2]);
        // Hoch- statt Querformat ist nie dasselbe Bild
        let mut d = photo(4, 700, 0xF0F0_F0F0_0F0F_0F0F, 1.0);
        d.aspect = 0.66;
        d.faces = 1;
        d.people = vec![3];
        assert!(!is_duplicate(&a, &d));
        // Gesichter nur auf einem Foto gefunden: zählt nicht als andere Person
        let mut e = photo(5, 650, 0xF0F0_F0F0_0F0F_0F0F, 1.0);
        e.faces = 0;
        assert!(is_duplicate(&a, &e));
    }

    #[test]
    fn same_scene_other_framing_counts_once() {
        // Farbverteilung gleich, Pixel verschieden: nur kurz hintereinander
        let red = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, _| {
            if x < 40 {
                image::Rgb([200, 30, 30])
            } else {
                image::Rgb([30, 30, 200])
            }
        }));
        let shifted = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, _| {
            if x >= 24 {
                image::Rgb([200, 30, 30])
            } else {
                image::Rgb([30, 30, 200])
            }
        }));
        let mut a = photo(1, 0, 0x0F0F_0F0F_0F0F_0F0F, 1.0);
        let mut b = photo(2, 120, 0xF0F0_F0F0_F0F0_F0F0, 1.0);
        a.hist = color_hist(&red);
        b.hist = color_hist(&shifted);
        assert!(is_duplicate(&a, &b));
        b.time = Some(3600);
        assert!(
            !is_duplicate(&a, &b),
            "eine Stunde später ist es ein neues Foto"
        );
    }

    #[test]
    fn four_and_five_stars_are_always_in() {
        let day = 86_400;
        let mut f: Vec<Features> = (0..12)
            .map(|k| {
                photo(
                    k,
                    k * 900,
                    (k as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15),
                    1.0,
                )
            })
            .collect();
        // Drei Fotos mit Sternen am zweiten Tag, eins davon Doppel eines anderen
        for k in 0..3 {
            let mut p = photo(
                100 + k,
                day + k * 900,
                (k as u64 + 7).wrapping_mul(0x51_7CC1_B727_220A),
                0.5,
            );
            p.rating = if k == 0 { 5 } else { 4 };
            f.push(p);
        }
        let mut twin = f[13].clone();
        twin.id = 200;
        twin.time = Some(day + 905);
        twin.rating = 4;
        f.push(twin);
        let (event, group) = groups(&f);
        let scores: Vec<f32> = f
            .iter()
            .map(|x| if x.id < 100 { 1.0 } else { 0.1 })
            .collect();
        let chosen = pick(&f, &scores, &event, &group, 6);
        let ids: Vec<i64> = chosen.iter().map(|&i| f[i].id).collect();
        for id in [100, 101, 102, 200] {
            assert!(ids.contains(&id), "{id} fehlt in {ids:?}");
        }
        assert_eq!(chosen.len(), 6);
        // Weniger gewünscht als Sterne-Fotos: trotzdem alle
        assert_eq!(pick(&f, &scores, &event, &group, 2).len(), 4);
    }

    #[test]
    fn bursts_become_one_group_and_days_events() {
        let day = 86_400;
        let f = vec![
            photo(1, 0, 0xFFFF_0000, 1.0),
            photo(2, 5, 0xFFFF_0001, 2.0), // Serie: fast gleicher Hash
            photo(3, 60, 0x0000_FFFF_0000_FFFF, 1.0), // anderes Motiv
            photo(4, day, 0xFFFF_0000, 1.0), // nächster Tag
        ];
        let (event, group) = groups(&f);
        assert_eq!(event, vec![0, 0, 0, 1]);
        assert_eq!(group[0], group[1]);
        assert_ne!(group[1], group[2]);
        assert_ne!(group[2], group[3]);
    }

    #[test]
    fn pick_spreads_over_events_and_skips_duplicates() {
        let day = 86_400;
        // Tag 1: viele gute Fotos, Tag 2: wenige mäßige
        let mut f = vec![];
        for k in 0..10 {
            f.push(photo(
                k,
                k * 600,
                (k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15),
                1.0,
            ));
        }
        f.push(photo(100, day, 0x1234_5678_9ABC_DEF0, 1.0));
        f.push(photo(101, day + 3, 0x1234_5678_9ABC_DEF1, 1.0)); // Serie
        let (event, group) = groups(&f);
        let scores: Vec<f32> = f
            .iter()
            .map(|x| if x.id < 100 { 1.0 } else { 0.2 })
            .collect();
        let chosen = pick(&f, &scores, &event, &group, 5);
        assert_eq!(chosen.len(), 5);
        let day2: Vec<_> = chosen.iter().filter(|&&i| f[i].id >= 100).collect();
        assert_eq!(day2.len(), 1, "Tag 2 kommt vor, die Serie nur einmal");
        // Mehr gewünscht als Motive vorhanden: alle Motive, keine Serie doppelt
        let all = pick(&f, &scores, &event, &group, 50);
        assert_eq!(all.len(), 11);
    }

    #[test]
    fn score_punishes_blur_and_screenshots() {
        let mut a = photo(1, 0, 0, 10.0);
        let mut b = photo(2, 10_000, 1, 1.0);
        let mut c = photo(3, 20_000, 2, 9.0);
        a.theme = Some(0.25);
        b.theme = Some(0.25);
        c.theme = Some(0.25);
        c.negative = Some(0.30);
        let (s, sharp) = score(&[a, b, c], false);
        assert!(sharp[0] > sharp[1]);
        assert!(s[0] > s[1], "schärfer ist besser");
        assert!(s[0] > s[2] + 0.3, "Bildschirmfoto fällt zurück");
    }

    #[test]
    fn every_theme_has_prompts() {
        for t in THEMES {
            assert!(!t.prompts.is_empty(), "{}", t.key);
        }
    }
}
