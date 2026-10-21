//! „Auto nach deinem Stil“: lernt aus den manuellen Bearbeitungen des Nutzers,
//! wie weit er vom Auto-Vorschlag abweicht („Difference Learning“), und
//! addiert diese Abweichung künftig zum Vorschlag.
//!
//! Reine Mathematik ohne Ein-/Ausgabe. Das Modell wächst in Stufen, jede
//! Stufe wird je Regler nur freigeschaltet, wenn sie im Leave-one-out-Test
//! nachweislich besser ist als die vorherige (ohne Lernen bleibt Auto, wie es
//! ist):
//!
//! 1. **Stufe 2 – Ausgleich je Gruppe:** gewichteter, gegen 0 geschrumpfter
//!    Mittelwert der Abweichung (global, dann je Kamera/Gruppe).
//! 2. **Stufe 3 – Bildmerkmale:** Ridge-Regression der übrigen Abweichung auf
//!    die Bildmerkmale (ab [`STAGE3_MIN`] Beispielen).
//! 3. **Stufe 4 – ähnliche Motive:** kNN über CLIP-Vektoren (ab
//!    [`STAGE4_MIN`] Beispielen mit Vektor).
//!
//! Weitere Regler ([`EXTRA`]) setzt Auto nicht; sie werden nur als absoluter
//! Wert vorgeschlagen, wenn der Nutzer sie regelmäßig verwendet.

// Indexschleifen über mehrere parallele Felder (Regler k) sind hier am lesbarsten.
#![allow(clippy::needless_range_loop)]

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Tonregler, die Auto setzt (Reihenfolge fest)
pub const TONE: [&str; 7] = [
    "exposure",
    "contrast",
    "highlights",
    "shadows",
    "whites",
    "blacks",
    "vibrance",
];
/// Weitere Regler, die Auto nur setzt, wenn der Nutzer sie regelmäßig nutzt
pub const EXTRA: [&str; 4] = ["saturation", "clarity", "dehaze", "sharpen"];
/// Bildmerkmale aus auto.rs (Helligkeitsperzentile usw., Werte grob 0…1)
pub const N_FEATURES: usize = 8;
// Gleiche Merkmale wie in auto.rs (Reihenfolge und Skalierung)
const _: () = assert!(crate::auto::FEATURES == N_FEATURES);

/// Halbwertszeit eines Beispiels in Tagen: w = 0.5^(Alter / Halbwertszeit)
const HALF_LIFE_DAYS: f64 = 365.0;
/// Schrumpfung des globalen Mittels gegen 0 (wie viele „Null-Beispiele“)
const K0: f64 = 5.0;
/// Schrumpfung des Gruppenmittels gegen das globale Mittel
const K1: f64 = 8.0;
/// Mindestzahl Beispiele, bevor ein Regler freigeschaltet werden kann
const MIN_SAMPLES: usize = 10;
/// Das Modell muss im Leave-one-out-Fehler unter diesem Anteil der Basis liegen
const GATE_FACTOR: f64 = 0.95;
/// Anteil der Beispiele, in denen ein EXTRA-Regler benutzt sein muss
const EXTRA_MIN_USAGE: f64 = 0.3;
/// Ab so vielen Beispielen zählt der Nutzungsanteil der Gruppe (sonst global)
const EXTRA_GROUP_MIN: usize = 10;
/// Ein Wert gilt als „benutzt“, wenn er mindestens so weit vom Standard (0) liegt
const USED_EPS: f32 = 1e-3;
/// Stufe 3: Mindestzahl Ton-Beispiele (je Gruppe bzw. global)
const STAGE3_MIN: usize = 50;
/// Stufe 3: höchstens so viele (neueste) Beispiele (Rechenzeit)
const STAGE3_MAX: usize = 500;
/// Stufe 3: Stärke der Ridge-Strafe (Merkmale standardisiert)
const RIDGE_LAMBDA: f64 = 10.0;
/// Stufe 4: Mindestzahl Ton-Beispiele mit Embedding
const STAGE4_MIN: usize = 300;
/// Stufe 4: höchstens so viele (neueste) Beispiele (Rechenzeit)
const STAGE4_MAX: usize = 2000;
/// Stufe 4: Anzahl der Nachbarn
const KNN_K: usize = 10;
/// Stufe 4: Ähnlichkeit, ab der ein Nachbar überhaupt zählt
const SIM_FLOOR: f64 = 0.6;
/// Obergrenze der Tonregler (Belichtung in EV, sonst Reglerpunkte)
const TONE_LIMIT: [f32; 7] = [3.0, 60.0, 60.0, 60.0, 60.0, 60.0, 60.0];
/// Wertebereiche der EXTRA-Regler (wie in `DevelopParams::clamped`)
const EXTRA_RANGE: [(f32, f32); 4] = [
    (-100.0, 100.0),
    (-100.0, 100.0),
    (-100.0, 100.0),
    (0.0, 150.0),
];

const P: usize = N_FEATURES + 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub image_id: i64,
    /// Gruppe: Kamera (bzw. "raw"/"jpeg", wenn unbekannt)
    pub group: String,
    /// Zeitpunkt der Bearbeitung (Unix-Sekunden)
    pub stamp: i64,
    /// Was Auto für dieses Foto vorgeschlagen hätte
    pub auto: [f32; 7],
    /// Endwerte des Nutzers
    pub user: [f32; 7],
    /// Mindestens ein Tonregler weicht vom Standard (0) ab – sonst hat der
    /// Nutzer am Ton nichts gemacht und das Foto lehrt über den Ton nichts
    pub tone_edited: bool,
    /// Endwerte der weiteren Regler (EXTRA)
    pub extra: [f32; 4],
    pub features: [f32; N_FEATURES],
}

pub struct Query<'a> {
    pub group: &'a str,
    pub features: [f32; N_FEATURES],
    pub embedding: Option<&'a [f32]>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prediction {
    /// auf den Auto-Vorschlag zu addieren
    pub tone_delta: [f32; 7],
    /// absolute Werte für EXTRA (None: nicht setzen)
    pub extra: [Option<f32>; 4],
    /// höchste Stufe, die zum Ergebnis beigetragen hat (0: nichts gelernt,
    /// 2: Ausgleich je Gruppe, 3: abhängig von Bildmerkmalen, 4: ähnliche Motive)
    pub stage: u8,
    /// Anzahl Lernbeispiele (Ton) in der verwendeten Gruppe bzw. insgesamt
    pub samples: usize,
}

/// Zusammenfassung für die Einstellungen
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSummary {
    pub group: String,
    pub samples: usize,
    /// je TONE-Regler gelernter mittlerer Ausgleich (0 wenn nicht aktiv)
    pub tone: [f32; 7],
    /// je EXTRA-Regler gelernter Wert (None wenn nicht aktiv)
    pub extra: [Option<f32>; 4],
    /// je TONE-Regler: passt sich zusätzlich dem einzelnen Foto an (Stufe 3
    /// oder 4) – dafür gibt es keinen festen Wert
    pub adaptive: [bool; 7],
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub samples: usize,
    pub groups: Vec<GroupSummary>,
    /// welche Stufen aktiv sind (true/false für 2, 3, 4)
    pub stage2: bool,
    pub stage3: bool,
    pub stage4: bool,
}

// ---------------------------------------------------------------------------
// Bausteine
// ---------------------------------------------------------------------------

/// Geschrumpfter Mittelwert: global gegen 0, je Gruppe gegen das globale Mittel.
struct Hier<const D: usize> {
    global: [f64; D],
    groups: HashMap<String, [f64; D]>,
}

#[derive(Clone, Copy)]
struct Sums<const D: usize> {
    w: f64,
    r: [f64; D],
}

impl<const D: usize> Sums<D> {
    fn zero() -> Self {
        Sums {
            w: 0.0,
            r: [0.0; D],
        }
    }
    fn add(&mut self, w: f64, r: &[f64; D]) {
        self.w += w;
        for k in 0..D {
            self.r[k] += w * r[k];
        }
    }
}

type Item<'a, const D: usize> = (&'a str, f64, [f64; D]);

fn sums_of<'a, const D: usize>(items: &[Item<'a, D>]) -> (Sums<D>, HashMap<&'a str, Sums<D>>) {
    let mut tot = Sums::zero();
    let mut per: HashMap<&str, Sums<D>> = HashMap::new();
    for (g, w, r) in items {
        tot.add(*w, r);
        per.entry(g).or_insert_with(Sums::zero).add(*w, r);
    }
    (tot, per)
}

fn shrunk_global<const D: usize>(tot: &Sums<D>) -> [f64; D] {
    let mut g = [0.0; D];
    for k in 0..D {
        g[k] = tot.r[k] / (tot.w + K0);
    }
    g
}

fn shrunk_group<const D: usize>(s: &Sums<D>, g: &[f64; D]) -> [f64; D] {
    let mut m = [0.0; D];
    for k in 0..D {
        m[k] = (s.r[k] + K1 * g[k]) / (s.w + K1);
    }
    m
}

fn hier_fit<const D: usize>(items: &[Item<D>]) -> Hier<D> {
    let (tot, per) = sums_of(items);
    let global = shrunk_global(&tot);
    let groups = per
        .iter()
        .map(|(g, s)| (g.to_string(), shrunk_group(s, &global)))
        .collect();
    Hier { global, groups }
}

impl<const D: usize> Hier<D> {
    fn get(&self, group: &str) -> &[f64; D] {
        self.groups.get(group).unwrap_or(&self.global)
    }
}

/// Leave-one-out-Vorhersage je Beispiel, exakt und ohne Neuberechnung:
/// die Summen werden einfach um das Beispiel vermindert.
fn hier_loo<const D: usize>(items: &[Item<D>]) -> Vec<[f64; D]> {
    let (tot, per) = sums_of(items);
    items
        .iter()
        .map(|(g, w, r)| {
            let mut t = tot;
            t.w -= w;
            let mut s = per[g];
            s.w -= w;
            for k in 0..D {
                t.r[k] -= w * r[k];
                s.r[k] -= w * r[k];
            }
            shrunk_group(&s, &shrunk_global(&t))
        })
        .collect()
}

/// Je Regler: Ist die Vorhersage `p` im gewichteten mittleren Fehler
/// nachweislich besser als „nichts“ (0) für die Ziele `t`?
fn gate<const D: usize>(t: &[[f64; D]], p: &[[f64; D]], w: &[f64]) -> [bool; D] {
    let mut active = [false; D];
    for (k, a) in active.iter_mut().enumerate() {
        let (mut em, mut eb) = (0.0, 0.0);
        for i in 0..t.len() {
            em += w[i] * (t[i][k] - p[i][k]).abs();
            eb += w[i] * t[i][k].abs();
        }
        *a = eb > 0.0 && em < GATE_FACTOR * eb;
    }
    active
}

fn mask<const D: usize>(v: &mut [f64; D], active: &[bool; D]) {
    for k in 0..D {
        if !active[k] {
            v[k] = 0.0;
        }
    }
}

struct Ridge {
    mean: [f64; N_FEATURES],
    std: [f64; N_FEATURES],
    /// Zeile 0: Achsenabschnitt, danach je Merkmal; Spalte je Regler
    beta: [[f64; 7]; P],
}

impl Ridge {
    fn design(&self, f: &[f32; N_FEATURES]) -> [f64; P] {
        let mut z = [1.0; P];
        for j in 0..N_FEATURES {
            z[j + 1] = (f[j] as f64 - self.mean[j]) / self.std[j];
        }
        z
    }
    fn predict(&self, f: &[f32; N_FEATURES]) -> [f64; 7] {
        let z = self.design(f);
        let mut out = [0.0; 7];
        for (j, zj) in z.iter().enumerate() {
            for k in 0..7 {
                out[k] += zj * self.beta[j][k];
            }
        }
        out
    }
}

fn invert(mut a: [[f64; P]; P]) -> Option<[[f64; P]; P]> {
    let mut inv = [[0.0; P]; P];
    for (i, row) in inv.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for c in 0..P {
        let piv = (c..P).max_by(|&x, &y| a[x][c].abs().total_cmp(&a[y][c].abs()))?;
        if a[piv][c].abs() < 1e-12 {
            return None;
        }
        a.swap(c, piv);
        inv.swap(c, piv);
        let d = a[c][c];
        for j in 0..P {
            a[c][j] /= d;
            inv[c][j] /= d;
        }
        for r in 0..P {
            if r != c {
                let f = a[r][c];
                if f != 0.0 {
                    for j in 0..P {
                        a[r][j] -= f * a[c][j];
                        inv[r][j] -= f * inv[c][j];
                    }
                }
            }
        }
    }
    Some(inv)
}

/// Gewichtete Ridge-Regression; liefert das Modell und die Leave-one-out-
/// Vorhersagen (über die Hat-Matrix, ohne Neuberechnung).
fn ridge_fit(x: &[[f32; N_FEATURES]], y: &[[f64; 7]], w: &[f64]) -> Option<(Ridge, Vec<[f64; 7]>)> {
    let n = x.len();
    let sw: f64 = w.iter().sum();
    if n == 0 || sw <= 0.0 {
        return None;
    }
    // Gewichte auf Mittelwert 1, damit λ unabhängig vom Alter wirkt
    let wn: Vec<f64> = w.iter().map(|v| v * n as f64 / sw).collect();
    let mut mean = [0.0; N_FEATURES];
    let mut std = [1.0; N_FEATURES];
    for j in 0..N_FEATURES {
        let m: f64 = (0..n).map(|i| wn[i] * x[i][j] as f64).sum::<f64>() / n as f64;
        let var: f64 = (0..n)
            .map(|i| wn[i] * (x[i][j] as f64 - m).powi(2))
            .sum::<f64>()
            / n as f64;
        mean[j] = m;
        std[j] = if var > 1e-12 { var.sqrt() } else { 1.0 };
    }
    let mut ridge = Ridge {
        mean,
        std,
        beta: [[0.0; 7]; P],
    };
    let z: Vec<[f64; P]> = x.iter().map(|f| ridge.design(f)).collect();
    let mut a = [[0.0; P]; P];
    let mut b = [[0.0; 7]; P];
    for i in 0..n {
        for p in 0..P {
            for q in 0..P {
                a[p][q] += wn[i] * z[i][p] * z[i][q];
            }
            for k in 0..7 {
                b[p][k] += wn[i] * z[i][p] * y[i][k];
            }
        }
    }
    // Achsenabschnitt (Index 0) wird nicht bestraft
    for (p, row) in a.iter_mut().enumerate().skip(1) {
        row[p] += RIDGE_LAMBDA;
    }
    let ainv = invert(a)?;
    for p in 0..P {
        for k in 0..7 {
            ridge.beta[p][k] = (0..P).map(|q| ainv[p][q] * b[q][k]).sum();
        }
    }
    let loo = (0..n)
        .map(|i| {
            let mut az = [0.0; P];
            for p in 0..P {
                az[p] = (0..P).map(|q| ainv[p][q] * z[i][q]).sum();
            }
            let h = (wn[i] * (0..P).map(|p| z[i][p] * az[p]).sum::<f64>()).min(0.999);
            let mut out = [0.0; 7];
            for k in 0..7 {
                let fit: f64 = (0..P).map(|p| z[i][p] * ridge.beta[p][k]).sum();
                out[k] = y[i][k] - (y[i][k] - fit) / (1.0 - h);
            }
            out
        })
        .collect();
    Some((ridge, loo))
}

struct Stage3 {
    ridge: Ridge,
    active: [bool; 7],
}

/// Die neuesten `max` Einträge von `idx` (nach Zeitstempel).
fn newest(mut idx: Vec<usize>, stamps: &[i64], max: usize) -> Vec<usize> {
    if idx.len() > max {
        idx.sort_by(|&a, &b| stamps[b].cmp(&stamps[a]).then(b.cmp(&a)));
        idx.truncate(max);
    }
    idx
}

struct Stage4 {
    dim: usize,
    /// normierte Vektoren, zeilenweise
    emb: Vec<f32>,
    /// Rest-Residuum je Beispiel
    y: Vec<[f64; 7]>,
    active: [bool; 7],
}

fn normalized(v: &[f32]) -> Option<Vec<f32>> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    (n > 1e-9 && v.iter().all(|x| x.is_finite())).then(|| v.iter().map(|x| x / n).collect())
}

/// kNN-Vorhersage (bereits mit α gemischt) für `q` über `emb`; `skip` lässt
/// ein Beispiel aus (Leave-one-out).
fn knn(emb: &[f32], dim: usize, y: &[[f64; 7]], q: &[f32], skip: Option<usize>) -> [f64; 7] {
    let mut near: Vec<(f64, usize)> = Vec::new();
    for (j, row) in emb.chunks_exact(dim).enumerate() {
        if Some(j) == skip {
            continue;
        }
        let sim = row.iter().zip(q).map(|(a, b)| a * b).sum::<f32>() as f64;
        if sim > SIM_FLOOR {
            near.push((sim, j));
        }
    }
    near.sort_by(|a, b| b.0.total_cmp(&a.0));
    near.truncate(KNN_K);
    let mut s = 0.0;
    let mut acc = [0.0; 7];
    for (sim, j) in near {
        let w = (sim - SIM_FLOOR).powi(2);
        s += w;
        for k in 0..7 {
            acc[k] += w * y[j][k];
        }
    }
    if s <= 0.0 {
        return [0.0; 7];
    }
    let alpha = s / (s + 1.0);
    acc.map(|v| alpha * v / s)
}

fn age_weight(now: i64, stamp: i64) -> f64 {
    let days = (now - stamp).max(0) as f64 / 86_400.0;
    0.5f64.powf(days / HALF_LIFE_DAYS)
}

// ---------------------------------------------------------------------------
// Modell
// ---------------------------------------------------------------------------

struct ExtraModel {
    hier: Hier<4>,
    active: [bool; 4],
    /// Nutzungsanteil je Regler, global
    global_frac: [f64; 4],
    /// je Gruppe: Anzahl Beispiele und Nutzungsanteil je Regler
    group_frac: HashMap<String, (usize, [f64; 4])>,
}

pub struct StyleModel {
    n_tone: usize,
    tone_groups: HashMap<String, usize>,
    /// Gruppen aller Beispiele (auch solche ohne Tonbearbeitung)
    all_groups: Vec<String>,
    tone: Hier<7>,
    active2: [bool; 7],
    extra: ExtraModel,
    s3_global: Option<Stage3>,
    s3_groups: HashMap<String, Stage3>,
    s4: Option<Stage4>,
}

impl StyleModel {
    /// `embeddings`: CLIP-Vektoren (normiert, Länge 512) zu image_id, soweit
    /// vorhanden. `now` Unix-Sekunden.
    pub fn learn(samples: &[Sample], embeddings: &HashMap<i64, Vec<f32>>, now: i64) -> Self {
        let tone_s: Vec<&Sample> = samples.iter().filter(|s| s.tone_edited).collect();
        let nt = tone_s.len();
        let w: Vec<f64> = tone_s.iter().map(|s| age_weight(now, s.stamp)).collect();
        let stamps: Vec<i64> = tone_s.iter().map(|s| s.stamp).collect();
        let r: Vec<[f64; 7]> = tone_s
            .iter()
            .map(|s| std::array::from_fn(|k| (s.user[k] - s.auto[k]) as f64))
            .collect();

        let mut tone_groups: HashMap<String, usize> = HashMap::new();
        for s in &tone_s {
            *tone_groups.entry(s.group.clone()).or_default() += 1;
        }
        let mut all_groups: Vec<String> = samples.iter().map(|s| s.group.clone()).collect();
        all_groups.sort();
        all_groups.dedup();

        // Stufe 2
        let items: Vec<Item<7>> = (0..nt)
            .map(|i| (tone_s[i].group.as_str(), w[i], r[i]))
            .collect();
        let tone = hier_fit(&items);
        let mut active2 = [false; 7];
        let mut s2loo = vec![[0.0; 7]; nt];
        if nt >= MIN_SAMPLES {
            let loo = hier_loo(&items);
            active2 = gate(&r, &loo, &w);
            s2loo = loo;
            for v in &mut s2loo {
                mask(v, &active2);
            }
        }

        // Stufe 3: Ziel ist das Residuum nach Stufe 2 (ohne das Beispiel selbst)
        let y3: Vec<[f64; 7]> = (0..nt)
            .map(|i| std::array::from_fn(|k| r[i][k] - s2loo[i][k]))
            .collect();
        let feats: Vec<[f32; N_FEATURES]> = tone_s.iter().map(|s| s.features).collect();
        let mut pred23 = s2loo.clone();
        let mut s3_groups = HashMap::new();
        let mut s3_global = None;
        let fit3 = |idx: Vec<usize>| -> Option<(Stage3, Vec<usize>, Vec<[f64; 7]>)> {
            let idx = newest(idx, &stamps, STAGE3_MAX);
            let x: Vec<_> = idx.iter().map(|&i| feats[i]).collect();
            let y: Vec<_> = idx.iter().map(|&i| y3[i]).collect();
            let ww: Vec<_> = idx.iter().map(|&i| w[i]).collect();
            let (ridge, loo) = ridge_fit(&x, &y, &ww)?;
            let active = gate(&y, &loo, &ww);
            if !active.contains(&true) {
                return None;
            }
            Some((Stage3 { ridge, active }, idx, loo))
        };
        let group_names: Vec<String> = tone_groups.keys().cloned().collect();
        let mut s3_members: HashMap<String, (Vec<usize>, Vec<[f64; 7]>)> = HashMap::new();
        for g in &group_names {
            let idx: Vec<usize> = (0..nt).filter(|&i| tone_s[i].group == *g).collect();
            if idx.len() >= STAGE3_MIN
                && let Some((m, idx, loo)) = fit3(idx)
            {
                s3_members.insert(g.clone(), (idx, loo));
                s3_groups.insert(g.clone(), m);
            }
        }
        let mut global_members = None;
        if nt >= STAGE3_MIN
            && let Some((m, idx, loo)) = fit3((0..nt).collect())
        {
            global_members = Some((idx, loo));
            s3_global = Some(m);
        }
        // Rest nach Stufe 2/3 je Beispiel, mit dem Modell, das es später bedienen würde
        for i in 0..nt {
            let g = &tone_s[i].group;
            let (model, members) = if let Some(m) = s3_groups.get(g) {
                (Some(m), s3_members.get(g))
            } else if let Some(m) = &s3_global {
                (Some(m), global_members.as_ref())
            } else {
                (None, None)
            };
            let Some(model) = model else { continue };
            let loo = members
                .and_then(|(idx, loo)| idx.iter().position(|&j| j == i).map(|p| loo[p]))
                .unwrap_or_else(|| model.ridge.predict(&feats[i]));
            for k in 0..7 {
                if model.active[k] {
                    pred23[i][k] += loo[k];
                }
            }
        }

        // Stufe 4
        let mut s4 = None;
        let with_emb: Vec<(usize, Vec<f32>)> = (0..nt)
            .filter_map(|i| {
                let e = embeddings.get(&tone_s[i].image_id)?;
                Some((i, normalized(e)?))
            })
            .collect();
        if let Some(dim) = with_emb.first().map(|(_, e)| e.len()) {
            let cand: Vec<usize> = with_emb
                .iter()
                .filter(|(_, e)| e.len() == dim)
                .map(|(i, _)| *i)
                .collect();
            let cand = newest(cand, &stamps, STAGE4_MAX);
            if cand.len() >= STAGE4_MIN {
                let by_i: HashMap<usize, &Vec<f32>> =
                    with_emb.iter().map(|(i, e)| (*i, e)).collect();
                let mut emb = Vec::with_capacity(cand.len() * dim);
                for i in &cand {
                    emb.extend_from_slice(by_i[i]);
                }
                let y: Vec<[f64; 7]> = cand
                    .iter()
                    .map(|&i| std::array::from_fn(|k| r[i][k] - pred23[i][k]))
                    .collect();
                let ww: Vec<f64> = cand.iter().map(|&i| w[i]).collect();
                let loo: Vec<[f64; 7]> = (0..cand.len())
                    .into_par_iter()
                    .map(|j| knn(&emb, dim, &y, &emb[j * dim..(j + 1) * dim], Some(j)))
                    .collect();
                let active = gate(&y, &loo, &ww);
                if active.contains(&true) {
                    s4 = Some(Stage4 {
                        dim,
                        emb,
                        y,
                        active,
                    });
                }
            }
        }

        // EXTRA-Regler: absolute Werte aus allen Beispielen
        let n_all = samples.len();
        let ew: Vec<f64> = samples.iter().map(|s| age_weight(now, s.stamp)).collect();
        let ev: Vec<[f64; 4]> = samples.iter().map(|s| s.extra.map(|v| v as f64)).collect();
        let eitems: Vec<Item<4>> = (0..n_all)
            .map(|i| (samples[i].group.as_str(), ew[i], ev[i]))
            .collect();
        let ehier = hier_fit(&eitems);
        let mut eactive = [false; 4];
        if n_all >= MIN_SAMPLES {
            eactive = gate(&ev, &hier_loo(&eitems), &ew);
        }
        let used = |s: &Sample, k: usize| s.extra[k].abs() > USED_EPS;
        let mut global_frac = [0.0; 4];
        let mut counts: HashMap<String, (usize, [usize; 4])> = HashMap::new();
        for s in samples {
            let c = counts.entry(s.group.clone()).or_default();
            c.0 += 1;
            for k in 0..4 {
                if used(s, k) {
                    c.1[k] += 1;
                    global_frac[k] += 1.0;
                }
            }
        }
        for f in &mut global_frac {
            *f /= n_all.max(1) as f64;
        }
        let group_frac = counts
            .into_iter()
            .map(|(g, (n, u))| (g, (n, u.map(|c| c as f64 / n as f64))))
            .collect();

        StyleModel {
            n_tone: nt,
            tone_groups,
            all_groups,
            tone,
            active2,
            extra: ExtraModel {
                hier: ehier,
                active: eactive,
                global_frac,
                group_frac,
            },
            s3_global,
            s3_groups,
            s4,
        }
    }

    /// Wert eines EXTRA-Reglers, falls aktiv und in der Gruppe (bzw. global)
    /// oft genug benutzt.
    fn extra_value(&self, group: &str, k: usize) -> Option<f32> {
        let e = &self.extra;
        if !e.active[k] {
            return None;
        }
        let frac = match e.group_frac.get(group) {
            Some((n, f)) if *n >= EXTRA_GROUP_MIN => f[k],
            _ => e.global_frac[k],
        };
        if frac < EXTRA_MIN_USAGE {
            return None;
        }
        let (lo, hi) = EXTRA_RANGE[k];
        Some((e.hier.get(group)[k] as f32).clamp(lo, hi))
    }

    fn stage3_for(&self, group: &str) -> Option<&Stage3> {
        self.s3_groups.get(group).or(self.s3_global.as_ref())
    }

    pub fn predict(&self, q: &Query) -> Prediction {
        let mut delta = *self.tone.get(q.group);
        mask(&mut delta, &self.active2);
        let mut stage = if self.active2.contains(&true) { 2 } else { 0 };

        if let Some(m) = self.stage3_for(q.group) {
            let p = m.ridge.predict(&q.features);
            for k in 0..7 {
                if m.active[k] {
                    delta[k] += p[k];
                }
            }
            stage = 3;
        }
        if let (Some(m), Some(e)) = (&self.s4, q.embedding)
            && e.len() == m.dim
            && let Some(e) = normalized(e)
        {
            let p = knn(&m.emb, m.dim, &m.y, &e, None);
            if p.iter().any(|v| *v != 0.0) {
                for k in 0..7 {
                    if m.active[k] {
                        delta[k] += p[k];
                    }
                }
                stage = 4;
            }
        }

        let extra: [Option<f32>; 4] = std::array::from_fn(|k| self.extra_value(q.group, k));
        if stage == 0 && extra.iter().any(|v| v.is_some()) {
            stage = 2;
        }
        Prediction {
            tone_delta: std::array::from_fn(|k| {
                (delta[k] as f32).clamp(-TONE_LIMIT[k], TONE_LIMIT[k])
            }),
            extra,
            stage,
            samples: self
                .tone_groups
                .get(q.group)
                .copied()
                .unwrap_or(self.n_tone),
        }
    }

    pub fn summary(&self) -> Summary {
        let mut groups: Vec<GroupSummary> = self
            .all_groups
            .iter()
            .map(|g| {
                let mut t = *self.tone.get(g);
                mask(&mut t, &self.active2);
                let s3 = self.s3_groups.get(g).or(self.s3_global.as_ref());
                GroupSummary {
                    group: g.clone(),
                    samples: self.tone_groups.get(g).copied().unwrap_or(0),
                    tone: t.map(|v| v as f32),
                    extra: std::array::from_fn(|k| self.extra_value(g, k)),
                    adaptive: std::array::from_fn(|k| {
                        s3.is_some_and(|m| m.active[k])
                            || self.s4.as_ref().is_some_and(|m| m.active[k])
                    }),
                }
            })
            .collect();
        groups.sort_by(|a, b| b.samples.cmp(&a.samples).then(a.group.cmp(&b.group)));
        Summary {
            samples: self.n_tone,
            groups,
            stage2: self.active2.contains(&true) || self.extra.active.contains(&true),
            // Nur, wenn die Stufe bei mindestens einem Regler freigeschaltet ist
            stage3: self
                .s3_global
                .iter()
                .chain(self.s3_groups.values())
                .any(|m| m.active.contains(&true)),
            stage4: self.s4.as_ref().is_some_and(|m| m.active.contains(&true)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;
    const DAY: i64 = 86_400;

    /// Kleiner deterministischer Zufallsgenerator (xorshift64*)
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545F4914F6CDD1D) >> 11) as f64 / (1u64 << 53) as f64
        }
        /// ungefähr normalverteilt (Mittel 0, Streuung 1)
        fn gauss(&mut self) -> f64 {
            (0..12).map(|_| self.next()).sum::<f64>() - 6.0
        }
    }

    /// Ton-Beispiel mit Ausgleich `off` plus Rauschen (Streuung `noise` Punkte,
    /// bei Belichtung 1/50 davon).
    fn sample(
        rng: &mut Rng,
        id: i64,
        group: &str,
        age_days: i64,
        off: [f64; 7],
        noise: f64,
        features: [f32; N_FEATURES],
    ) -> Sample {
        let mut auto = [0.0f32; 7];
        let mut user = [0.0f32; 7];
        for k in 0..7 {
            let scale = if k == 0 { 0.02 } else { 1.0 };
            auto[k] = (rng.gauss() * 5.0 * scale) as f32;
            user[k] = auto[k] + (off[k] + rng.gauss() * noise * scale) as f32;
        }
        Sample {
            image_id: id,
            group: group.into(),
            stamp: NOW - age_days * DAY,
            auto,
            user,
            tone_edited: true,
            extra: [0.0; 4],
            features,
        }
    }

    fn feats(rng: &mut Rng) -> [f32; N_FEATURES] {
        std::array::from_fn(|_| rng.next() as f32)
    }

    fn query<'a>(group: &'a str, features: [f32; N_FEATURES]) -> Query<'a> {
        Query {
            group,
            features,
            embedding: None,
        }
    }

    fn learn(s: &[Sample]) -> StyleModel {
        StyleModel::learn(s, &HashMap::new(), NOW)
    }

    const BLACKS: usize = 5;

    #[test]
    fn keine_beispiele() {
        let m = learn(&[]);
        let p = m.predict(&query("x", [0.5; N_FEATURES]));
        assert_eq!(p.stage, 0);
        assert_eq!(p.tone_delta, [0.0; 7]);
        assert_eq!(p.extra, [None; 4]);
        assert!(!m.summary().stage2);
    }

    #[test]
    fn konstanter_ausgleich_wird_gelernt() {
        let mut rng = Rng(1);
        let off = [0.3, 0.0, 0.0, 0.0, 0.0, -15.0, 0.0];
        let s: Vec<_> = (0..30)
            .map(|i| {
                let f = feats(&mut rng);
                sample(&mut rng, i, "cam", 0, off, 0.5, f)
            })
            .collect();
        let p = learn(&s).predict(&query("cam", [0.5; N_FEATURES]));
        assert_eq!(p.stage, 2);
        assert_eq!(p.samples, 30);
        let b = p.tone_delta[BLACKS];
        assert!((-15.5..-11.0).contains(&b), "blacks {b}");
        let e = p.tone_delta[0];
        assert!((0.2..0.31).contains(&e), "exposure {e}");
        for k in [1, 2, 3, 4, 6] {
            assert_eq!(p.tone_delta[k], 0.0, "regler {k}");
        }
    }

    #[test]
    fn rauschen_schaltet_nichts_frei() {
        let mut rng = Rng(2);
        let s: Vec<_> = (0..40)
            .map(|i| {
                let f = feats(&mut rng);
                sample(&mut rng, i, "cam", 0, [0.0; 7], 4.0, f)
            })
            .collect();
        let m = learn(&s);
        let p = m.predict(&query("cam", [0.5; N_FEATURES]));
        assert_eq!(p.stage, 0);
        assert_eq!(p.tone_delta, [0.0; 7]);
        assert!(!m.summary().stage2);
    }

    #[test]
    fn zwei_gruppen_gegensaetzlich() {
        let mut rng = Rng(3);
        let mut s = Vec::new();
        for i in 0..40 {
            let (g, o) = if i % 2 == 0 {
                ("A", 15.0)
            } else {
                ("B", -15.0)
            };
            let f = feats(&mut rng);
            s.push(sample(
                &mut rng,
                i,
                g,
                0,
                [0.0, 0.0, 0.0, 0.0, 0.0, o, 0.0],
                1.0,
                f,
            ));
        }
        let m = learn(&s);
        let a = m.predict(&query("A", [0.5; N_FEATURES])).tone_delta[BLACKS];
        let b = m.predict(&query("B", [0.5; N_FEATURES])).tone_delta[BLACKS];
        let u = m.predict(&query("unbekannt", [0.5; N_FEATURES]));
        assert!(a > 8.0, "A {a}");
        assert!(b < -8.0, "B {b}");
        assert!(u.tone_delta[BLACKS].abs() < 3.0, "unbekannt");
        assert_eq!(u.samples, 40);
        let sum = m.summary();
        assert_eq!(sum.groups.len(), 2);
        assert_eq!(sum.groups[0].samples, 20);
    }

    #[test]
    fn neun_beispiele_reichen_nicht() {
        let mut rng = Rng(4);
        let off = [0.0, 0.0, 0.0, 0.0, 0.0, -15.0, 0.0];
        let s: Vec<_> = (0..9)
            .map(|i| {
                let f = feats(&mut rng);
                sample(&mut rng, i, "cam", 0, off, 0.5, f)
            })
            .collect();
        let p = learn(&s).predict(&query("cam", [0.5; N_FEATURES]));
        assert_eq!(p.stage, 0);
        assert_eq!(p.tone_delta, [0.0; 7]);
    }

    #[test]
    fn stufe_3_folgt_dem_merkmal() {
        let mut rng = Rng(5);
        let s: Vec<_> = (0..120)
            .map(|i| {
                let f = feats(&mut rng);
                let off = 40.0 * (f[0] as f64 - 0.5);
                sample(
                    &mut rng,
                    i,
                    "cam",
                    0,
                    [0.0, 0.0, 0.0, 0.0, 0.0, off, 0.0],
                    1.0,
                    f,
                )
            })
            .collect();
        let m = learn(&s);
        let mut hi = [0.5; N_FEATURES];
        hi[0] = 0.9;
        let mut lo = [0.5; N_FEATURES];
        lo[0] = 0.1;
        let ph = m.predict(&query("cam", hi));
        let pl = m.predict(&query("cam", lo));
        assert_eq!(ph.stage, 3);
        assert!(m.summary().stage3);
        assert!(ph.tone_delta[BLACKS] > 8.0, "{}", ph.tone_delta[BLACKS]);
        assert!(pl.tone_delta[BLACKS] < -8.0, "{}", pl.tone_delta[BLACKS]);
    }

    /// Embedding um die Achse `axis` mit kleinem Rauschen
    fn embedding(rng: &mut Rng, axis: usize) -> Vec<f32> {
        let mut v: Vec<f32> = (0..32).map(|_| ((rng.next() - 0.5) * 0.1) as f32).collect();
        v[axis] += 1.0;
        normalized(&v).unwrap()
    }

    #[test]
    fn stufe_4_aehnliche_motive() {
        let mut rng = Rng(6);
        let mut s = Vec::new();
        let mut emb = HashMap::new();
        for i in 0..400 {
            let a = i % 2 == 0;
            let off = if a { 20.0 } else { -20.0 };
            let f = feats(&mut rng);
            s.push(sample(
                &mut rng,
                i,
                "cam",
                0,
                [0.0, 0.0, 0.0, 0.0, 0.0, off, 0.0],
                1.0,
                f,
            ));
            emb.insert(i, embedding(&mut rng, if a { 0 } else { 1 }));
        }
        let m = StyleModel::learn(&s, &emb, NOW);
        let qa = embedding(&mut rng, 0);
        let qb = embedding(&mut rng, 1);
        let pa = m.predict(&Query {
            group: "cam",
            features: [0.5; N_FEATURES],
            embedding: Some(&qa),
        });
        let pb = m.predict(&Query {
            group: "cam",
            features: [0.5; N_FEATURES],
            embedding: Some(&qb),
        });
        assert_eq!(pa.stage, 4);
        assert!(m.summary().stage4);
        assert!(pa.tone_delta[BLACKS] > 8.0, "{}", pa.tone_delta[BLACKS]);
        assert!(pb.tone_delta[BLACKS] < -8.0, "{}", pb.tone_delta[BLACKS]);
        // ohne Embedding der Anfrage entfällt Stufe 4
        assert!(m.predict(&query("cam", [0.5; N_FEATURES])).stage < 4);
    }

    #[test]
    fn extra_regler_nur_bei_regelmaessiger_nutzung() {
        let mut rng = Rng(7);
        let s: Vec<_> = (0..50)
            .map(|i| {
                let f = feats(&mut rng);
                let mut x = sample(&mut rng, i, "cam", 0, [0.0; 7], 1.0, f);
                if i % 5 < 3 {
                    x.extra[1] = 25.0 + rng.gauss() as f32; // clarity in 60 %
                }
                if i % 10 == 0 {
                    x.extra[0] = 25.0; // saturation in 10 %
                }
                x
            })
            .collect();
        let m = learn(&s);
        let p = m.predict(&query("cam", [0.5; N_FEATURES]));
        let c = p.extra[1].expect("clarity aktiv");
        assert!((8.0..25.0).contains(&c), "clarity {c}");
        assert_eq!(p.extra[0], None);
        assert_eq!(p.extra[2], None);
        assert_eq!(p.extra[3], None);
        assert_eq!(p.stage, 2);
        assert_eq!(m.summary().groups[0].extra[1], Some(c));
    }

    #[test]
    fn aeltere_beispiele_zaehlen_weniger() {
        let mut rng = Rng(8);
        let mut s = Vec::new();
        for i in 0..40 {
            let (age, o) = if i < 20 { (3 * 365, -20.0) } else { (0, 10.0) };
            let f = feats(&mut rng);
            s.push(sample(
                &mut rng,
                i,
                "cam",
                age,
                [0.0, 0.0, 0.0, 0.0, 0.0, o, 0.0],
                1.0,
                f,
            ));
        }
        let b = learn(&s)
            .predict(&query("cam", [0.5; N_FEATURES]))
            .tone_delta[BLACKS];
        assert!((3.0..10.0).contains(&b), "blacks {b}");
    }

    #[test]
    fn ausgabe_ist_begrenzt() {
        let mut rng = Rng(9);
        let s: Vec<_> = (0..40)
            .map(|i| {
                let f = feats(&mut rng);
                sample(
                    &mut rng,
                    i,
                    "cam",
                    0,
                    [9.0, 0.0, 0.0, 0.0, 0.0, 200.0, 0.0],
                    0.1,
                    f,
                )
            })
            .collect();
        let p = learn(&s).predict(&query("cam", [0.5; N_FEATURES]));
        assert!(p.tone_delta[0] <= 3.0);
        assert!(p.tone_delta[BLACKS] <= 60.0);
        assert!(p.tone_delta[BLACKS] > 50.0);
    }

    /// Laufzeit mit 2000 Beispielen; nur im Release aussagekräftig:
    /// `cargo test -p lumenkorn-develop --release style -- --ignored`
    #[test]
    #[ignore]
    fn laufzeit_2000_beispiele() {
        let mut rng = Rng(10);
        let mut s = Vec::new();
        let mut emb = HashMap::new();
        for i in 0..2000 {
            let g = if i % 3 == 0 { "A" } else { "B" };
            let f = feats(&mut rng);
            let off = 30.0 * (f[1] as f64 - 0.5);
            s.push(sample(
                &mut rng,
                i,
                g,
                i % 700,
                [0.1, 0.0, 0.0, 0.0, 0.0, off, 0.0],
                2.0,
                f,
            ));
            let v: Vec<f32> = (0..512).map(|_| (rng.next() - 0.5) as f32).collect();
            emb.insert(i, normalized(&v).unwrap());
        }
        let t = std::time::Instant::now();
        let m = StyleModel::learn(&s, &emb, NOW);
        let dt = t.elapsed();
        eprintln!("learn mit 2000 Beispielen: {dt:?}, Stufen {:?}", {
            let u = m.summary();
            (u.stage2, u.stage3, u.stage4)
        });
        assert!(dt.as_secs_f64() < 2.0, "zu langsam: {dt:?}");
    }
}
