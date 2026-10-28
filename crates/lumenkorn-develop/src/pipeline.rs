//! Die Entwicklungspipeline.
//!
//! Reihenfolge und Arbeitsraum jeder Stufe:
//!
//! | Stufe                  | Raum                         |
//! |------------------------|------------------------------|
//! | Weißabgleich           | linear, szenenbezogen        |
//! | Belichtung             | linear, szenenbezogen        |
//! | Basiskurve (nur RAW)   | szenen- → anzeigebezogen     |
//! | Dunst entfernen        | sRGB-kodiert                 |
//! | Weiß/Schwarz, Lichter/Tiefen, Kontrast, Klarheit | sRGB-kodiert, auf der Helligkeit |
//! | Dynamik, Sättigung, HSL, Color Grading | sRGB-kodiert |
//! | Gradationskurve (RGB, je Kanal) | sRGB-kodiert            |
//! | Lokale Anpassungen (Masken) | wie oben, je Ebene überblendet |
//! | Schärfen               | sRGB-kodiert, Helligkeit     |
//! | Drehen, Spiegeln, Crop | zuletzt – Masken und lokale Karten bleiben am Motiv |
//!
//! Mit Standardparametern ist die Pipeline für Rasterbilder (bis auf Rundung)
//! die Identität – ein unbearbeitetes JPEG sieht exakt aus wie vorher.

use crate::color::{linear_to_srgb, luma, smoothstep, srgb_to_linear};
use crate::geometry;
use crate::image_f32::ImageF32;
use crate::local::LumaMap;
use crate::masks::{self, MaskSource, NoMasks};
use crate::params::{Crop, DevelopParams, LocalAdjust};
use crate::repair::{NoRepairs, RepairSource};
use crate::source::{Source, SourceKind};
use rayon::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// Feste Zahlen der Formeln. Der GPU-Shader bekommt sie beim Laden als
// WGSL-Konstanten vorangestellt (`gpu::shader_source`) – so gibt es jede nur hier.

/// Grundabstimmung von RAWs (wie das Standardprofil in Lightroom): Kontrast,
/// Sättigung und Dynamik, wenn die Regler auf 0 stehen – zusätzlich zur
/// Tonwertabbildung von darktable (sigmoid), die für sich flau wirkt.
pub(crate) const RAW_BASE_CONTRAST: f32 = 0.38;
pub(crate) const RAW_BASE_SATURATION: f32 = 0.15;
pub(crate) const RAW_BASE_VIBRANCE: f32 = 0.15;

/// Reglerwert (−1…1) mit Grundwert: 0 ergibt `base`, die Enden ±1 bleiben
/// erreichbar (Kontrast +100 wirkt voll, Sättigung −100 ist grau).
#[inline]
pub(crate) fn with_base(v: f32, base: f32) -> f32 {
    if v >= 0.0 {
        base + v * (1.0 - base)
    } else {
        base + v * (1.0 + base)
    }
}

/// Kontrast, Sättigung und Dynamik, wie sie gerechnet werden (−1…1): bei
/// RAWs mit Grundabstimmung. Bei Schwarzweiß keine Farbanhebung – sie
/// verschöbe die Graustufen vorhandener Bearbeitungen. Für CPU und GPU.
pub(crate) fn base_look(p: &DevelopParams, raw: bool) -> (f32, f32, f32) {
    let (c, s, v) = (p.contrast / 100.0, p.saturation / 100.0, p.vibrance / 100.0);
    if !raw {
        return (c, s, v);
    }
    let contrast = with_base(c, RAW_BASE_CONTRAST);
    if p.monochrome {
        return (contrast, s, v);
    }
    (
        contrast,
        with_base(s, RAW_BASE_SATURATION),
        with_base(v, RAW_BASE_VIBRANCE),
    )
}

/// Dynamik: Breite des Übergangs zur Schonung der Hauttöne (Abstand der
/// Kanäle in Kodierungseinheiten)
/// Helligkeitskarten (lange Kante in px, Weichzeichner-Radius) für Lichter/
/// Tiefen, Klarheit und Dunst – CPU und GPU (`gpu::MAP_CAP` hängt daran)
pub(crate) const BASE_MAP: (usize, usize) = (128, 3);
pub(crate) const DETAIL_MAP: (usize, usize) = (640, 3);
pub(crate) const DEHAZE_MAP: (usize, usize) = (256, 2);
/// Darunter gilt ein Pixel in `apply_luma` als schwarz (Helligkeit addieren
/// statt skalieren)
pub(crate) const APPLY_LUMA_EPS: f32 = 1e-4;
pub(crate) const SKIN_EDGE: f32 = 0.02;
/// Beginn der weichen Schulter von [`soft_clip`].
pub(crate) const SOFT_CLIP_KNEE: f32 = 0.8;
/// Stärke von Lichter/Tiefen bei ±100: Exponent der Anhebung bzw. Absenkung.
pub(crate) const SHADOW_HIGHLIGHT_GAIN: f32 = 1.1;
/// Lichter/Tiefen: Bereiche (Rand unten, Rand oben) der lokalen Helligkeit,
/// in denen die Wirkung ausläuft (Tiefen: bis 0,75 abnehmend; Lichter: ab 0,25 zunehmend).
pub(crate) const SHADOW_RANGE: (f32, f32) = (0.1, 0.75);
pub(crate) const HIGHLIGHT_RANGE: (f32, f32) = (0.25, 0.9);
/// Weiß/Schwarz: Bereiche der Tonwerte, in denen der Regler wirkt
pub(crate) const WHITES_RANGE: (f32, f32) = (0.4, 1.0);
pub(crate) const BLACKS_RANGE: (f32, f32) = (0.0, 0.6);
/// Dynamik: Anteil, um den Hauttöne geschont werden
pub(crate) const SKIN_PROTECT: f32 = 0.5;
/// Verschiebung von Weiß bzw. Schwarz am Ende der Tonwertskala bei ±100.
pub(crate) const WHITES_GAIN: f32 = 0.4;
pub(crate) const BLACKS_GAIN: f32 = 0.35;
/// Stärke der Klarheit bei ±100 (Anteil des lokalen Kontrasts).
pub(crate) const CLARITY_GAIN: f32 = 0.8;
/// Anteil des geschätzten Dunsts, der bei Dehaze +100 entfernt wird (ω im
/// Dark-Channel-Prior; unter 1, damit Fernes etwas Tiefe behält).
pub(crate) const DEHAZE_GAIN: f32 = 0.9;
/// Untergrenze der Transmission: begrenzt die Verstärkung in sehr dunstigen
/// Bereichen (sonst Rauschen und ausbrechender Himmel).
pub(crate) const DEHAZE_MIN_T: f32 = 0.3;
/// Anteil des Dunsts, der bei Dehaze −100 eingemischt wird.
pub(crate) const HAZE_ADD: f32 = 0.6;
/// Anteil der durch Dehaze verlorenen mittleren Helligkeit, der über die
/// Mitteltöne zurückgegeben wird (Kontrast bleibt, das Bild „säuft nicht ab“).
pub(crate) const DEHAZE_LIFT: f32 = 0.7;
/// Farbverstärkung bei Dehaze: Abstand zum Grau × t^−DEHAZE_CHROMA (bei
/// voller Wirkung ×1,4). Die physikalisch „richtigen“ 1/t wirken grell.
/// Die Dunstfarbe wird bewusst *nicht* abgezogen: Gemessen an Testfotos
/// verschob das Abziehen (auch nur zur Hälfte) klare Bilder ins Blaue, weil
/// die hellsten Flächen dort warm sind und kein Dunst.
pub(crate) const DEHAZE_CHROMA: f32 = 0.3;
/// Color Grading: Tönung bei Sättigung 100 (Abstand zum Grau, sRGB-kodiert).
pub(crate) const GRADE_TINT: f32 = 0.2;
/// Color Grading: Helligkeitsverschiebung bei Luminanz ±100.
pub(crate) const GRADE_LUM: f32 = 0.2;
/// Anteil, mit dem die S-Kurve bei ±100 Kontrast höchstens eingemischt wird.
pub(crate) const CONTRAST_MIX: f32 = 0.85;
/// Verstärkung des Unscharf-Maskierens bei 100.
pub(crate) const SHARPEN_GAIN: f32 = 1.6;
/// Helligkeitsunterschiede darunter gelten als Rauschen und werden nicht geschärft.
pub(crate) const SHARPEN_THRESHOLD: f32 = 0.004;
/// Deckkraft und Farbe der roten Maskenüberlagerung.
pub(crate) const OVERLAY_OPACITY: f32 = 0.55;
pub(crate) const OVERLAY_COLOR: [f32; 3] = [0.95, 0.12, 0.12];

/// Zusätzliche Eingaben für [`render_with`].
pub struct RenderOptions<'a> {
    /// Liefert die Pixel von Modellmasken (Motiv, Himmel, Objekt).
    pub masks: &'a dyn MaskSource,
    /// Maskenebene, die rot überlagert angezeigt werden soll.
    pub overlay: Option<usize>,
    /// Entrauschte Fassung des Quellbilds (gleiche Größe), falls vorhanden.
    pub denoised: Option<&'a ImageF32>,
    /// Liefert die Flicken der KI-Reparaturen.
    pub repairs: &'a dyn RepairSource,
    /// Nur diesen Ausschnitt des fertigen Bildes ausgeben (normiert, nach dem
    /// Zuschnitt) – für die Detailansicht beim Hineinzoomen.
    pub region: Option<Crop>,
    /// Die vorbereitete Quelle (Objektivkorrektur, Reparaturen) für weitere
    /// Renderings desselben Bildes behalten – nur für die Bearbeitungsansicht,
    /// in der Reglerzüge dasselbe Bild immer wieder entwickeln. Einmalige
    /// Renderings (Vorschaubilder, Export, KI) würden sonst deren Einträge
    /// verdrängen und Bilder in voller Auflösung im Speicher halten.
    pub keep_prepared: bool,
}

impl Default for RenderOptions<'_> {
    fn default() -> Self {
        Self {
            masks: &NoMasks,
            overlay: None,
            denoised: None,
            region: None,
            repairs: &NoRepairs,
            keep_prepared: false,
        }
    }
}

/// Wendet Rauschreduzierung, Objektivkorrektur und Reparaturen auf das
/// lineare Quellbild an – die Stufen vor dem Weißabgleich. Reparaturen, die
/// auf dem korrigierten Bild berechnet wurden, kommen nach der Korrektur,
/// ältere (aus dem Original) davor.
fn prepare_source(
    img: &mut ImageF32,
    p: &DevelopParams,
    opts: &RenderOptions,
    lens: Option<&crate::lens::LensProfile>,
) {
    let (w, h) = (img.width, img.height);
    if let (Some(d), Some(dn)) = (&p.denoise, opts.denoised) {
        if dn.width == w && dn.height == h {
            let a = d.amount / 100.0;
            img.data
                .par_iter_mut()
                .zip(dn.data.par_iter())
                .for_each(|(o, n)| {
                    for k in 0..3 {
                        o[k] += (n[k] - o[k]) * a;
                    }
                });
        } else {
            log::warn!(
                "Entrauschte Fassung passt nicht ({}×{} statt {w}×{h})",
                dn.width,
                dn.height
            );
        }
    }
    // Ohne Korrektur (abgeschaltet oder keine Profildaten) bleiben Flicken, die
    // auf dem korrigierten Bild berechnet wurden, weg: Sie säßen versetzt und
    // müssen neu berechnet werden.
    apply_repairs(img, p, opts, false);
    if let Some(l) = lens {
        *img = l.apply(img);
        apply_repairs(img, p, opts, true);
    }
}

/// Entrauschte Fassung, die eingemischt wird (Rauschreduzierung an und die
/// Fassung passt zum Bild).
fn denoised_for<'a>(
    src: &Source,
    p: &DevelopParams,
    opts: &RenderOptions<'a>,
) -> Option<&'a ImageF32> {
    p.denoise.as_ref()?;
    opts.denoised
        .filter(|d| d.width == src.image.width && d.height == src.image.height)
}

/// `a` und `b` mischen: `a + (b − a)·t`, wie in [`prepare_source`]
fn blend(a: &ImageF32, b: &ImageF32, t: f32) -> ImageF32 {
    let mut out = a.clone();
    out.data
        .par_iter_mut()
        .zip(b.data.par_iter())
        .for_each(|(o, n)| {
            for k in 0..3 {
                o[k] += (n[k] - o[k]) * t;
            }
        });
    out
}

/// Reparaturen einsetzen. `corrected`: nur die vor (`false`) bzw. nach
/// (`true`) der Objektivkorrektur berechneten.
fn apply_repairs(img: &mut ImageF32, p: &DevelopParams, opts: &RenderOptions, corrected: bool) {
    for r in p
        .repairs
        .iter()
        .filter(|r| r.enabled && r.lens == corrected)
    {
        if let Some(patch) = r.result.as_deref().and_then(|id| opts.repairs.get(id)) {
            patch.apply(img);
        }
    }
}

/// Objektivkorrektur, die für dieses Bild gilt (eingeschaltet und Daten da).
fn lens_of<'a>(src: &'a Source, p: &DevelopParams) -> Option<&'a crate::lens::LensProfile> {
    p.lens_correction.then_some(src.lens.as_deref()).flatten()
}

/// Gibt es Reparaturen, die tatsächlich eingesetzt werden? Flicken nach der
/// Objektivkorrektur zählen nur, wenn die Korrektur gilt.
fn has_repairs(p: &DevelopParams, lens: bool) -> bool {
    p.repairs
        .iter()
        .any(|r| r.enabled && r.result.is_some() && (lens || !r.lens))
}

/// Braucht das Bild eine vorbereitete (zwischengespeicherte) Quelle?
fn needs_prepared(src: &Source, p: &DevelopParams) -> bool {
    lens_of(src, p).is_some() || has_repairs(p, false)
}

/// Schlüssel einer vorbereiteten Fassung: das Bild wie für die GPU (Adresse,
/// Größe und Stichproben – eine freigegebene Quelle kann an derselben Adresse
/// durch ein anderes Bild ersetzt werden) plus Flicken und Objektivkorrektur.
/// Die Stärke der Rauschreduzierung gehört nicht dazu: Quelle und entrauschte
/// Fassung werden getrennt vorbereitet und erst danach gemischt.
type RepairedKey = (crate::gpu::ImageKey, String);
/// Für GPU und CPU: Quellbild bzw. entrauschte Fassung mit eingesetzten
/// Reparaturen und Objektivkorrektur, zwischengespeichert – beim Ziehen eines
/// Reglers (auch der Stärke der Rauschreduzierung) ändert sich daran nichts.
/// Vier Einträge: Quelle und entrauschte Fassung, jeweils für Vorschau (Proxy)
/// und Zoomansicht (volle Auflösung), die sich abwechseln. Die Zahl ist die
/// Kennung der Fassung für den GPU-Puffer-Cache.
static REPAIRED: Mutex<Vec<(RepairedKey, Arc<Source>, u64)>> = Mutex::new(Vec::new());
/// Fortlaufende Kennung reparierter Fassungen. Eine neue Fassung kann an der
/// Adresse einer verworfenen liegen und sich nur im Flicken unterscheiden –
/// die GPU erkennt sie daran sicher als neu (siehe `gpu::ImageKey`).
static REPAIRED_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
const REPAIRED_ENTRIES: usize = 4;

/// Verwirft die zwischengespeicherten reparierten Quellbilder.
pub(crate) fn release_cache() {
    REPAIRED.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Nur Einträge in voller Auflösung verwerfen (nach einem Export).
pub(crate) fn release_large() {
    REPAIRED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|(_, s, _)| s.image.data.len() <= crate::LARGE_PIXELS);
}

/// Liefert die reparierte Fassung und ihre Kennung (nie 0).
/// Vorbereitete Fassung von `image` (die Quelle oder ihre entrauschte
/// Fassung): Reparaturen und Objektivkorrektur von `src`, ohne Entrauschen.
fn prepared_image(
    src: &Source,
    image: &ImageF32,
    p: &DevelopParams,
    opts: &RenderOptions,
) -> (Arc<Source>, u64) {
    type Key = RepairedKey;
    let ids: Vec<&str> = p
        .repairs
        .iter()
        .filter(|r| r.enabled)
        .filter_map(|r| r.result.as_deref())
        .collect();
    let key: Key = (
        crate::gpu::image_key(image, 0),
        format!("{}|{}", ids.join(","), lens_of(src, p).is_some()),
    );
    {
        let mut c = REPAIRED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pos) = c.iter().position(|(k, _, _)| *k == key) {
            let entry = c.remove(pos);
            let s = (entry.1.clone(), entry.2);
            c.insert(0, entry);
            return s;
        }
    }
    // Ohne Sperre rechnen: `prepare_source` arbeitet parallel (rayon), und
    // parallele Arbeit unter einer Sperre kann sich verklemmen.
    let lens = lens_of(src, p);
    let before_lens = p
        .repairs
        .iter()
        .any(|r| r.enabled && !r.lens && r.result.is_some());
    // Ohne Entrauschen vorbereiten (gemischt wird danach)
    let plain = RenderOptions {
        denoised: None,
        ..*opts
    };
    let image = match lens {
        // Nur Objektivkorrektur (und Reparaturen danach): Sie liefert ohnehin
        // ein neues Bild – die Kopie der Quelle vorher wäre umsonst (bei 60 MP
        // 720 MB und ein voller Kopiervorgang je Export)
        Some(l) if !before_lens => {
            let mut img = l.apply(image);
            apply_repairs(&mut img, p, &plain, true);
            img
        }
        _ => {
            let mut img = image.clone();
            prepare_source(&mut img, p, &plain, lens);
            img
        }
    };
    let out = Arc::new(Source {
        kind: src.kind,
        image,
        full_width: src.full_width,
        full_height: src.full_height,
        lens: src.lens.clone(),
        as_shot_kelvin: src.as_shot_kelvin,
    });
    let id = REPAIRED_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if !opts.keep_prepared {
        return (out, id);
    }
    let mut c = REPAIRED.lock().unwrap_or_else(|e| e.into_inner());
    c.retain(|(k, _, _)| *k != key);
    c.insert(0, (key, out.clone(), id));
    c.truncate(REPAIRED_ENTRIES);
    (out, id)
}

/// Entwickelt das Quellbild mit den Parametern und liefert sRGB-kodierte Werte in 0..1.
pub fn render(src: &Source, params: &DevelopParams) -> ImageF32 {
    render_with(src, params, &RenderOptions::default())
}

/// Versucht, auf der GPU zu entwickeln (falls eingeschaltet und das Bild
/// passt); `None` heißt: auf der CPU rechnen.
/// Mit Reparaturen oder Objektivkorrektur bekommt die GPU die zwischengespeicherte
/// vorbereitete Quelle und Parameter ohne Entrauschen/Reparaturen – alles steckt
/// schon darin.
fn try_gpu(
    src: &Source,
    p: &DevelopParams,
    opts: &RenderOptions,
    rgb8: bool,
) -> Result<crate::gpu::Rendered, Option<Arc<Source>>> {
    let n = src.image.width * src.image.height;
    let Some(g) = crate::gpu::active().filter(|g| g.fits(n)) else {
        return Err(None);
    };
    if !gpu_allowed(n) {
        return Err(None);
    }
    let original = &src.image;
    let mut prepared_src = None;
    let result = if needs_prepared(src, p) {
        let (prepared, ident) = prepared_image(src, &src.image, p, opts);
        // Die entrauschte Fassung ebenso vorbereitet; gemischt wird auf der
        // GPU (lineare Schritte: Mischen vorher oder nachher ist dasselbe)
        let dn = denoised_for(src, p, opts).map(|d| prepared_image(src, d, p, opts).0);
        let local = RenderOptions {
            denoised: dn.as_ref().map(|d| &d.image),
            repairs: &NoRepairs,
            ..*opts
        };
        let mut gp = p.clone();
        if dn.is_none() {
            gp.denoise = None;
        }
        gp.repairs.clear();
        let r = g.render_sanitized(&prepared, &gp, &local, rgb8, original, ident);
        // Nach einem Fehler nur die ungemischte Quelle weitergeben, wenn sie
        // schon das Ergebnis ist (sonst rechnet die CPU selbst)
        prepared_src = dn.is_none().then_some(prepared);
        r
    } else {
        g.render_sanitized(src, p, opts, rgb8, original, 0)
    };
    match result {
        Ok(r) => {
            gpu_succeeded(n);
            Ok(r)
        }
        Err(e) => {
            crate::gpu::note_error(&e);
            gpu_failed(n);
            // Die vorbereitete Quelle (Reparaturen, Objektiv) gleich weitergeben
            Err(prepared_src)
        }
    }
}

/// Ab dieser Pixelzahl scheiterte die GPU zuletzt (`usize::MAX`: noch nicht).
/// Größere Bilder rechnen dann direkt auf der CPU, statt es jedes Mal erneut
/// (und womöglich nach langer Wartezeit) zu versuchen.
static GPU_FAIL_PIXELS: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Zählt übersprungene Versuche; ab und zu wird trotzdem probiert.
static GPU_SKIPPED: AtomicUsize = AtomicUsize::new(0);
/// Jeder wievielte übersprungene Versuch doch auf die GPU geht (der Fehler
/// kann vorübergehend gewesen sein, z. B. wenig freier Grafikspeicher).
const GPU_RETRY_EVERY: usize = 16;

fn gpu_allowed(n: usize) -> bool {
    if n < GPU_FAIL_PIXELS.load(Ordering::Relaxed) {
        return true;
    }
    GPU_SKIPPED.fetch_add(1, Ordering::Relaxed) % GPU_RETRY_EVERY == GPU_RETRY_EVERY - 1
}

fn gpu_failed(n: usize) {
    GPU_FAIL_PIXELS.fetch_min(n, Ordering::Relaxed);
}

fn gpu_succeeded(n: usize) {
    // Ein Erfolg bei oder über der Grenze hebt sie an
    GPU_FAIL_PIXELS.fetch_max(n.saturating_add(1), Ordering::Relaxed);
}

/// Vergisst gemerkte GPU-Fehler (beim Ein- und Ausschalten der GPU).
pub(crate) fn reset_gpu_failures() {
    GPU_FAIL_PIXELS.store(usize::MAX, Ordering::Relaxed);
    GPU_SKIPPED.store(0, Ordering::Relaxed);
}

/// Entwickelt auf der GPU, wenn eingeschaltet und möglich, sonst auf der CPU.
/// Das Ergebnis ist (bis auf Rundung) dasselbe.
pub fn render_auto(src: &Source, params: &DevelopParams, opts: &RenderOptions) -> ImageF32 {
    let p = params.sanitized();
    match try_gpu(src, &p, opts, false) {
        Ok(crate::gpu::Rendered::Float(img)) => img,
        Ok(crate::gpu::Rendered::Rgb8(_)) => render_sanitized(src, &p, opts, None),
        Err(prepared) => render_sanitized(src, &p, opts, prepared),
    }
}

/// Wie [`render_auto`], aber gleich als 8-Bit-RGB (Vorschau, Thumbnails) –
/// spart auf der GPU das Rücklesen von Float-Daten.
pub fn render_auto_rgb8(
    src: &Source,
    params: &DevelopParams,
    opts: &RenderOptions,
) -> image::RgbImage {
    let p = params.sanitized();
    match try_gpu(src, &p, opts, true) {
        Ok(crate::gpu::Rendered::Rgb8(img)) => img,
        Ok(crate::gpu::Rendered::Float(_)) => {
            crate::output::to_rgb8(&render_sanitized(src, &p, opts, None))
        }
        Err(prepared) => crate::output::to_rgb8(&render_sanitized(src, &p, opts, prepared)),
    }
}

pub fn render_with(src: &Source, params: &DevelopParams, opts: &RenderOptions) -> ImageF32 {
    render_sanitized(src, &params.sanitized(), opts, None)
}

/// Wie [`render_with`] mit bereits bereinigten Parametern (`sanitized`);
/// `prepared`: schon berechnete vorbereitete Quelle (nach einem GPU-Fehler).
fn render_sanitized(
    src: &Source,
    p: &DevelopParams,
    opts: &RenderOptions,
    prepared: Option<Arc<Source>>,
) -> ImageF32 {
    let raw = src.kind == SourceKind::Raw;

    // --- Rauschreduzierung, Objektivkorrektur, Reparaturen (linear) ----------
    // Mit Reparaturen oder Objektivkorrektur aus demselben Zwischenspeicher wie
    // die GPU: Entzerren und Flicken einsetzen kostet sonst bei jedem Rendern
    // (Reglerzug) erneut, obwohl sich an der vorbereiteten Quelle nichts ändert.
    let repaired = prepared.or_else(|| {
        needs_prepared(src, p).then(|| {
            let base = prepared_image(src, &src.image, p, opts).0;
            match (denoised_for(src, p, opts), &p.denoise) {
                (Some(d), Some(dn)) => {
                    let d = prepared_image(src, d, p, opts).0;
                    Arc::new(Source {
                        kind: base.kind,
                        image: blend(&base.image, &d.image, dn.amount / 100.0),
                        full_width: base.full_width,
                        full_height: base.full_height,
                        lens: base.lens.clone(),
                        as_shot_kelvin: base.as_shot_kelvin,
                    })
                }
                _ => base,
            }
        })
    });
    let (mut img, repaired) = match repaired {
        // Nicht zwischengespeichert (Export) und ohne Dunst, der sie noch
        // braucht: die vorbereitete Fassung übernehmen statt kopieren
        Some(r) if p.dehaze == 0.0 => {
            let img = Arc::try_unwrap(r).map_or_else(|a| a.image.clone(), |s| s.image);
            (img, None)
        }
        Some(r) => (r.image.clone(), Some(r)),
        None => {
            let mut img = src.image.clone();
            prepare_source(&mut img, p, opts, None);
            (img, None)
        }
    };
    let (w, h) = (img.width, img.height);

    // --- Szenenbezogene Stufen und Kodierung -------------------------------
    let wb = global_white_balance(p, src.as_shot_kelvin);
    let gain = 2f32.powf(p.exposure);
    img.data.par_iter_mut().for_each(|px| {
        let mut c = [
            px[0] * wb[0] * gain,
            px[1] * wb[1] * gain,
            px[2] * wb[2] * gain,
        ];
        c = if raw {
            raw_tone(c)
        } else {
            [c[0].min(1.0), c[1].min(1.0), c[2].min(1.0)]
        };
        *px = [
            linear_to_srgb(c[0]),
            linear_to_srgb(c[1]),
            linear_to_srgb(c[2]),
        ];
    });

    // --- Dunst --------------------------------------------------------------
    // Dunsthelligkeit aus derselben Quelle wie auf der GPU (dort bekommt
    // `render_masked` mit Reparaturen die reparierte Fassung)
    if p.dehaze != 0.0 {
        let est = repaired.as_ref().map_or(&src.image, |r| &r.image);
        let amount = p.dehaze / 100.0;
        let wb = global_white_balance(p, src.as_shot_kelvin);
        dehaze(&mut img, amount, haze_level(est, p, wb, raw, amount));
    }

    // --- Tonwerte und Farbe ------------------------------------------------
    let (contrast, saturation, vibrance) = base_look(p, raw);
    tone(
        &mut img,
        &Tone {
            contrast,
            highlights: p.highlights / 100.0,
            shadows: p.shadows / 100.0,
            whites: p.whites / 100.0,
            blacks: p.blacks / 100.0,
            clarity: p.clarity / 100.0,
        },
    );
    color(&mut img, saturation, vibrance);
    hsl(&mut img, &p.hsl);
    let grade = grade_setup(&p.color_grade);
    // Bei Schwarzweiß kommt das Color Grading erst nach der Umwandlung (Tonen)
    if let (Some(g), false) = (&grade, p.monochrome) {
        img.data
            .par_iter_mut()
            .for_each(|px| *px = grade_px(*px, g));
    }
    // Gradationskurve nach dem Color Grading (wie im Bedienfeld darunter)
    if let Some(l) = crate::curve::curve_setup(&p.tone_curve) {
        img.data
            .par_iter_mut()
            .for_each(|px| *px = crate::curve::curve_px(*px, &l));
    }

    // --- Lokale Anpassungen ------------------------------------------------
    let mut overlay_alpha = None;
    for (i, layer) in p.masks.iter().enumerate() {
        let want_overlay = opts.overlay == Some(i);
        let active = layer.enabled && !layer.adjust.is_neutral();
        if !active && !want_overlay {
            continue;
        }
        let alpha = masks::evaluate_cached(layer, w, h, opts.masks, Some(&src.image));
        if layer.enabled && !layer.adjust.is_neutral() {
            apply_local_masked(&mut img, &layer.adjust, &alpha);
        }
        if want_overlay {
            overlay_alpha = Some(alpha.clone());
        }
    }

    // --- Schwarzweiß ------------------------------------------------------
    // Nach den lokalen Anpassungen, damit deren Temperatur und Sättigung das
    // Bild nicht wieder einfärben; danach tont das Color Grading
    if p.monochrome {
        img.data.par_iter_mut().for_each(|px| {
            let y = luma(*px);
            *px = [y, y, y];
        });
        if let Some(g) = &grade {
            img.data
                .par_iter_mut()
                .for_each(|px| *px = grade_px(*px, g));
        }
    }

    // --- Schärfen ----------------------------------------------------------
    if p.sharpen > 0.0 {
        img = sharpen(&img, p.sharpen / 100.0);
    }

    // Maske sichtbar machen: rot überlagert, wie in gängigen Editoren.
    if let Some(alpha) = overlay_alpha {
        img.data
            .par_iter_mut()
            .zip(alpha.par_iter())
            .for_each(|(px, a)| {
                let k = a * OVERLAY_OPACITY;
                for (v, c) in px.iter_mut().zip(OVERLAY_COLOR) {
                    *v += (c - *v) * k;
                }
            });
    }

    // --- Geometrie ---------------------------------------------------------
    img = geometry::rotate_quarters(img, p.rotate);
    if p.flip_horizontal {
        img = geometry::flip_h(&img);
    }
    if let Some(w) = geometry::Warp::new(img.width, img.height, p) {
        img = geometry::warp(&img, &w.m);
    }
    if let Some(c) = p.crop {
        img = geometry::crop(&img, c);
    }
    if let Some(r) = opts.region.and_then(sanitize_region) {
        img = geometry::crop(&img, r);
    }
    img
}

/// Gültiger Ausschnitt (innerhalb 0…1, nicht leer) oder `None`.
pub(crate) fn sanitize_region(r: Crop) -> Option<Crop> {
    let ok = [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite());
    if !ok {
        return None;
    }
    let x = r.x.clamp(0.0, 1.0);
    let y = r.y.clamp(0.0, 1.0);
    let w = r.w.clamp(0.0, 1.0 - x);
    let h = r.h.clamp(0.0, 1.0 - y);
    (w > 0.0 && h > 0.0).then_some(Crop { x, y, w, h })
}

/// Kenngrößen für Dehaze, je Bild einmal geschätzt.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Haze {
    /// Helligkeit des Dunsts (Airlight), sRGB-kodiert, neutral
    pub level: f32,
    /// Aufhellung der Mitteltöne, die den Helligkeitsverlust ausgleicht
    pub lift: f32,
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Schätzt [`Haze`] für Dehaze mit Stärke `amount`.
///
/// Die Dunsthelligkeit kommt wie beim Dark-Channel-Prior aus den dunstigsten
/// Pixeln (größter kleinster Farbanteil), aber als *neutraler* Grauwert: Die
/// übliche farbige Schätzung übernimmt meist das Blau des Himmels, und das
/// Abziehen dieser Farbe lässt das ganze Bild ins Gelbe bzw. Blaue kippen.
/// Gerechnet wird auf einer Stichprobe der Quelle mit Weißabgleich, Belichtung
/// und Basiskurve – für CPU und GPU dieselben Zahlen.
pub(crate) fn haze_level(
    src: &ImageF32,
    p: &DevelopParams,
    wb: [f32; 3],
    raw: bool,
    amount: f32,
) -> Haze {
    let n = src.data.len();
    if n == 0 {
        return Haze {
            level: 1.0,
            lift: 0.0,
        };
    }
    let gain = 2f32.powf(p.exposure);
    // Etwa 65 000 gleichmäßig verteilte Stichproben genügen. Die Schrittweite
    // teilerfremd zur Breite, sonst fielen alle Proben auf dieselben Spalten
    // (z. B. Schrittweite 8 bei durch 8 teilbarer Breite).
    let mut step = (n / 65_536).max(1);
    while step > 1 && gcd(step, src.width.max(1)) != 1 {
        step += 1;
    }
    let samples: Vec<[f32; 3]> = src
        .data
        .iter()
        .step_by(step)
        .map(|px| {
            let c = [
                px[0] * wb[0] * gain,
                px[1] * wb[1] * gain,
                px[2] * wb[2] * gain,
            ];
            let c = if raw {
                raw_tone(c)
            } else {
                c.map(|v| v.min(1.0))
            };
            c.map(linear_to_srgb)
        })
        .collect();
    let mut dark: Vec<(f32, f32)> = samples
        .iter()
        .map(|e| (e[0].min(e[1]).min(e[2]), luma(*e)))
        .collect();
    // Die dunstigsten 0,5 % (mindestens eins), davon die mittlere Helligkeit
    let top = (dark.len() / 200).max(1);
    dark.select_nth_unstable_by(top - 1, |a, b| b.0.total_cmp(&a.0));
    let level = (dark[..top].iter().map(|v| v.1).sum::<f32>() / top as f32)
        // Untergrenze: auch in dunklen Bildern kein Dunst „aus Schwarz“
        .clamp(0.5, 1.0);

    // Helligkeitsverlust schätzen (mit dem Dunkelkanal des Pixels selbst –
    // die Karte liegt darunter, der Verlust also eher etwas überschätzt)
    let mut lift = 0.0;
    if amount > 0.0 {
        let (mut before, mut after, mut weight) = (0f64, 0f64, 0f64);
        for e in &samples {
            let d = e[0].min(e[1]).min(e[2]);
            let (yn, _) = dehaze_luma(luma(*e), d, amount, level);
            before += luma(*e) as f64;
            after += yn.clamp(0.0, 1.0) as f64;
            weight += mid_weight(yn) as f64;
        }
        if weight > 0.0 {
            lift = ((before - after) / weight) as f32 * DEHAZE_LIFT;
        }
    }
    Haze {
        level,
        lift: lift.clamp(0.0, 0.3),
    }
}

/// Gewicht der Mitteltöne (0 bei Schwarz und Weiß, 1 bei 0,5). Der Shader
/// hat dieselbe Formel (`mid_weight`) mit `MID_WEIGHT_K`.
pub(crate) const MID_WEIGHT_K: f32 = 4.0;
#[inline]
fn mid_weight(y: f32) -> f32 {
    let v = y.clamp(0.0, 1.0);
    MID_WEIGHT_K * v * (1.0 - v)
}

/// J = (I − A) / t + A auf der Helligkeit, mit neutralem A = `level`.
/// Liefert neue Helligkeit und Transmission t.
#[inline]
fn dehaze_luma(y: f32, d: f32, amount: f32, level: f32) -> (f32, f32) {
    let t = (1.0 - amount * DEHAZE_GAIN * d / level).max(DEHAZE_MIN_T);
    ((y - level) / t + level, t)
}

/// Entfernt (`amount` > 0) oder ergänzt (< 0) Dunst, sRGB-kodiert, in place.
fn dehaze(img: &mut ImageF32, amount: f32, haze: Haze) {
    if amount < 0.0 {
        let k = -amount * HAZE_ADD;
        img.data
            .par_iter_mut()
            .for_each(|px| *px = px.map(|v| v + (haze.level - v) * k));
        return;
    }
    let (w, h) = (img.width, img.height);
    // Dunkelkanal: Dunst hebt den kleinsten Farbanteil an. Weichgezeichnet
    // (~1 % der Bildbreite) wird daraus die Dunstdichte der Umgebung.
    let dark: Vec<f32> = img
        .data
        .par_iter()
        .map(|c| c[0].min(c[1]).min(c[2]))
        .collect();
    let map = LumaMap::build(&dark, w, h, DEHAZE_MAP.0, DEHAZE_MAP.1);
    img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let d = dark[i].min(map.sample(i % w, i / w));
        *px = dehaze_px(*px, d, amount, haze);
    });
}

/// Dunst aus einem Pixel herausrechnen.
/// `d`: Dunkelkanal – nie über dem des Pixels selbst, sonst bekämen dunkle
/// Motive vor hellem Himmel einen dunklen Rand (der weichgezeichnete Wert
/// reicht über die Kante).
/// Die Helligkeit wird voll entdunstet; der Farbabstand zum Grau wächst nur
/// gedämpft (siehe [`DEHAZE_CHROMA`]), der Farbton bleibt – keine kippenden,
/// grellen Farben.
#[inline]
fn dehaze_px(c: [f32; 3], d: f32, amount: f32, haze: Haze) -> [f32; 3] {
    let y = luma(c);
    let (yn, t) = dehaze_luma(y, d, amount, haze.level);
    let yn = yn + haze.lift * mid_weight(yn);
    let k = t.powf(-DEHAZE_CHROMA);
    let ch = |i: usize| yn + (c[i] - y) * k;
    fit_gamut([ch(0), ch(1), ch(2)], yn)
}

/// Tonwert-Parameter, normiert auf −1…1 (auch für die GPU).
pub(crate) struct Tone {
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub clarity: f32,
}

impl Tone {
    pub(crate) fn active(&self) -> bool {
        self.highlights != 0.0
            || self.shadows != 0.0
            || self.whites != 0.0
            || self.blacks != 0.0
            || self.clarity != 0.0
            || self.contrast != 0.0
    }

    /// Braucht die Stufe weichgezeichnete Helligkeitskarten (Nachbarschaft)?
    fn needs_maps(&self) -> bool {
        self.highlights != 0.0 || self.shadows != 0.0 || self.clarity != 0.0
    }
}

/// Helligkeitskarten für [`tone_px`], aus der Helligkeit des *ganzen* Bildes.
struct ToneMaps {
    base: Option<LumaMap>,
    detail: Option<LumaMap>,
}

impl ToneMaps {
    fn build(y: &[f32], w: usize, h: usize, t: &Tone) -> Self {
        // Grobe Karte (~ 5 % der Bildbreite) für Lichter/Tiefen, feinere für Klarheit.
        Self {
            base: (t.highlights != 0.0 || t.shadows != 0.0)
                .then(|| LumaMap::build(y, w, h, BASE_MAP.0, BASE_MAP.1)),
            detail: (t.clarity != 0.0).then(|| LumaMap::build(y, w, h, DETAIL_MAP.0, DETAIL_MAP.1)),
        }
    }
}

fn tone(img: &mut ImageF32, t: &Tone) {
    if !t.active() {
        return;
    }
    let (w, h) = (img.width, img.height);
    let y: Vec<f32> = img.data.par_iter().map(|c| luma(*c)).collect();
    let maps = ToneMaps::build(&y, w, h, t);
    img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        *px = tone_px(*px, y[i], i % w, i / w, t, &maps);
    });
}

/// Tonwerte für ein Pixel an (x, yy) mit Helligkeit `y0`.
#[inline]
fn tone_px(px: [f32; 3], y0: f32, x: usize, yy: usize, t: &Tone, maps: &ToneMaps) -> [f32; 3] {
    let (sh, hi) = (t.shadows, t.highlights);
    let mut v = y0;

    // Lichter und Tiefen: die helle bzw. dunkle Hälfte der Tonwerte, bis in
    // die Mitteltöne hinein. Gewicht nach der *lokalen* Helligkeit, damit
    // Strukturen in Schatten und Lichtern erhalten bleiben.
    if let Some(m) = &maps.base {
        let b = m.sample(x, yy);
        if sh != 0.0 {
            let ws = 1.0 - smoothstep(SHADOW_RANGE.0, SHADOW_RANGE.1, b);
            v *= (sh * SHADOW_HIGHLIGHT_GAIN * ws * (1.0 - v).max(0.0)).exp();
        }
        if hi != 0.0 {
            let wh = smoothstep(HIGHLIGHT_RANGE.0, HIGHLIGHT_RANGE.1, b);
            v = 1.0 - (1.0 - v) * (-hi * SHADOW_HIGHLIGHT_GAIN * wh * v.max(0.0)).exp();
        }
    }

    // Weiß und Schwarz: nur die Enden der Skala – Weiß verschiebt die
    // hellsten Töne (bis zum Ausfressen bzw. weg von Weiß), Schwarz die
    // dunkelsten; die Mitteltöne bleiben nahezu, wo sie sind.
    v = tone_ends(v, t.whites, t.blacks);

    v = s_curve(v.clamp(0.0, 1.0), t.contrast);

    if let Some(m) = &maps.detail {
        let d = m.sample(x, yy);
        let mid = MID_WEIGHT_K * v * (1.0 - v);
        v += t.clarity * CLARITY_GAIN * (y0 - d) * mid;
    }

    apply_luma(px, y0, v.clamp(0.0, 1.0))
}

fn color(img: &mut ImageF32, sat: f32, vib: f32) {
    if sat == 0.0 && vib == 0.0 {
        return;
    }
    img.data
        .par_iter_mut()
        .for_each(|px| *px = color_px(*px, sat, vib));
}

#[inline]
fn color_px(px: [f32; 3], sat: f32, vib: f32) -> [f32; 3] {
    let y = luma(px);
    let max = px[0].max(px[1]).max(px[2]);
    let min = px[0].min(px[1]).min(px[2]);
    let chroma = max - min;
    // Dynamik wirkt stark auf blasse und schwach auf kräftige Farben.
    // Hauttöne (R > G > B) werden zusätzlich geschont – mit weichem Übergang,
    // ein harter Schalter ergab an R = G bzw. G = B Kanten im Bild.
    let skin = 1.0 - SKIN_PROTECT * smoothstep(0.0, SKIN_EDGE, (px[0] - px[1]).min(px[1] - px[2]));
    let f = 1.0 + sat + vib * (1.0 - chroma).powi(2) * if vib > 0.0 { skin } else { 1.0 };
    let c = [
        y + (px[0] - y) * f,
        y + (px[1] - y) * f,
        y + (px[2] - y) * f,
    ];
    fit_gamut(c, y)
}

/// HSL-Werkzeug (siehe `color::hsl_pixel`), in place auf dem kodierten Bild.
fn hsl(img: &mut ImageF32, h: &crate::params::Hsl) {
    if h.is_neutral() {
        return;
    }
    let adj = hsl_adjust(h);
    img.data
        .par_iter_mut()
        .for_each(|px| *px = crate::color::hsl_pixel(*px, &adj));
}

/// Color Grading, vorbereitet für [`grade_px`] (und den GPU-Shader, der
/// dieselben Zahlen bekommt).
#[derive(Clone, Copy, Debug)]
pub(crate) struct GradeVals {
    /// Tiefen, Mitteltöne, Lichter, Global: Tönung (RGB, ohne eigene
    /// Helligkeit) und Helligkeitsverschiebung
    pub wheels: [[f32; 4]; 4],
    /// Grenze zwischen Tiefen und Lichtern (Helligkeit 0…1)
    pub pivot: f32,
    /// Breite der Übergänge
    pub width: f32,
}

/// `None`, wenn Color Grading nichts ändert.
pub(crate) fn grade_setup(g: &crate::params::ColorGrade) -> Option<GradeVals> {
    if g.is_neutral() {
        return None;
    }
    let wheel = |w: &crate::params::GradeWheel| {
        // Reine Farbe des Farbtons, ohne ihre Helligkeit: die Tönung färbt,
        // ohne aufzuhellen oder abzudunkeln
        let c = hue_rgb(w.hue);
        let y = luma(c);
        let s = w.saturation / 100.0 * GRADE_TINT;
        [
            (c[0] - y) * s,
            (c[1] - y) * s,
            (c[2] - y) * s,
            w.luminance / 100.0 * GRADE_LUM,
        ]
    };
    Some(GradeVals {
        wheels: [
            wheel(&g.shadows),
            wheel(&g.midtones),
            wheel(&g.highlights),
            wheel(&g.global),
        ],
        // Abgleich +100 schiebt die Grenze nach unten: mehr Lichter
        pivot: 0.5 - 0.3 * g.balance / 100.0,
        width: 0.1 + 0.4 * g.blending / 100.0,
    })
}

/// Voll gesättigte Farbe eines Farbtons (Grad, 0 = Rot), sRGB.
fn hue_rgb(h: f32) -> [f32; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    match h as u32 {
        0 => [1.0, x, 0.0],
        1 => [x, 1.0, 0.0],
        2 => [0.0, 1.0, x],
        3 => [0.0, x, 1.0],
        4 => [x, 0.0, 1.0],
        _ => [1.0, 0.0, x],
    }
}

/// Color Grading für ein Pixel: Gewichte der Tonwertbereiche nach der
/// Helligkeit, dann Tönung und Helligkeit je Bereich addieren. Schwarz bleibt
/// schwarz und Weiß weiß – `fit_gamut` nimmt dort die Farbe zurück.
#[inline]
fn grade_px(c: [f32; 3], g: &GradeVals) -> [f32; 3] {
    let y = luma(c);
    let ws = 1.0 - smoothstep(g.pivot - g.width, g.pivot, y);
    let wh = smoothstep(g.pivot, g.pivot + g.width, y);
    let wm = (1.0 - ws - wh).max(0.0);
    let mut o = c;
    let mut yn = y;
    for (w, v) in [ws, wm, wh, 1.0].into_iter().zip(&g.wheels) {
        for k in 0..3 {
            o[k] += w * (v[k] + v[3]);
        }
        yn += w * v[3];
    }
    fit_gamut(o, yn)
}

pub(crate) fn hsl_adjust(h: &crate::params::Hsl) -> crate::color::HslAdjust {
    crate::color::HslAdjust {
        hue: h.hue.map(|v| v / 100.0),
        sat: h.saturation.map(|v| v / 100.0),
        lum: h.luminance.map(|v| v / 100.0),
    }
}

/// Regler einer Maskenebene auf ein (bereits entwickeltes) Bild anwenden und
/// mit der Deckkraft `alpha` überblenden.
///
/// Gerechnet wird nur im Rahmen der Pixel mit Deckkraft > 0 – eine kleine
/// Radialmaske kostete sonst eine volle Bildkopie und alle Stufen auf allen
/// Pixeln. Bis auf eine Ausnahme wirken die Stufen pixelweise: Lichter/Tiefen
/// und Klarheit tasten weichgezeichnete Helligkeitskarten ab, deren Größe und
/// Weichzeichner relativ zum *ganzen* Bild sind und die auch Helligkeit von
/// außerhalb der Maske einbeziehen. Diese Karten entstehen deshalb wie bisher
/// aus dem ganzen Bild (nach Belichtung/Weißabgleich der Ebene – dafür genügt
/// die Helligkeit, keine Bildkopie). So bleibt das Ergebnis bitgleich mit der
/// Rechnung über das ganze Bild.
fn apply_local_masked(img: &mut ImageF32, a: &LocalAdjust, alpha: &[f32]) {
    let (w, h) = (img.width, img.height);
    let Some((x0, y0, x1, y1)) = alpha_bounds(alpha, w, h) else {
        return;
    };
    let expo = a.exposure != 0.0 || a.temperature != 0.0 || a.tint != 0.0;
    let wb = white_balance(a.temperature / 100.0, a.tint / 100.0);
    let gain = 2f32.powf(a.exposure);
    let exposure = |px: [f32; 3]| -> [f32; 3] {
        let lin = [0, 1, 2].map(|k| soft_clip(srgb_to_linear(px[k]) * wb[k] * gain));
        let y = luma(lin);
        fit_gamut(
            [
                linear_to_srgb(lin[0]),
                linear_to_srgb(lin[1]),
                linear_to_srgb(lin[2]),
            ],
            linear_to_srgb(y),
        )
    };
    let t = Tone {
        contrast: a.contrast / 100.0,
        highlights: a.highlights / 100.0,
        shadows: a.shadows / 100.0,
        whites: 0.0,
        blacks: 0.0,
        clarity: a.clarity / 100.0,
    };
    let maps = if t.needs_maps() {
        let y: Vec<f32> = img
            .data
            .par_iter()
            .map(|p| luma(if expo { exposure(*p) } else { *p }))
            .collect();
        ToneMaps::build(&y, w, h, &t)
    } else {
        ToneMaps {
            base: None,
            detail: None,
        }
    };
    let (tone_on, sat) = (t.active(), a.saturation / 100.0);
    img.data[y0 * w..y1 * w]
        .par_chunks_mut(w)
        .zip(alpha[y0 * w..y1 * w].par_chunks(w))
        .enumerate()
        .for_each(|(r, (row, arow))| {
            let yy = y0 + r;
            for x in x0..x1 {
                let al = arow[x];
                if al.is_nan() || al <= 0.0 {
                    continue;
                }
                let o = &mut row[x];
                let mut l = *o;
                if expo {
                    l = exposure(l);
                }
                if tone_on {
                    l = tone_px(l, luma(l), x, yy, &t, &maps);
                }
                if sat != 0.0 {
                    l = color_px(l, sat, 0.0);
                }
                for k in 0..3 {
                    o[k] += (l[k] - o[k]) * al;
                }
            }
        });
}

/// Rahmen (x0, y0, x1, y1; Ende exklusiv) aller Pixel mit Deckkraft > 0.
fn alpha_bounds(alpha: &[f32], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    if w == 0 {
        return None;
    }
    let rows: Vec<Option<(usize, usize)>> = alpha[..w * h]
        .par_chunks(w)
        .map(|row| {
            let first = row.iter().position(|a| *a > 0.0)?;
            let last = row.iter().rposition(|a| *a > 0.0)?;
            Some((first, last + 1))
        })
        .collect();
    let y0 = rows.iter().position(Option::is_some)?;
    let y1 = rows.iter().rposition(Option::is_some)? + 1;
    let (x0, x1) = rows[y0..y1]
        .iter()
        .flatten()
        .fold((usize::MAX, 0), |(a, b), (f, l)| (a.min(*f), b.max(*l)));
    Some((x0, y0, x1, y1))
}

/// Weiche Schulter über [`SOFT_CLIP_KNEE`]: helle Bereiche laufen nicht abrupt ins Weiß.
#[inline]
fn soft_clip(v: f32) -> f32 {
    const K: f32 = SOFT_CLIP_KNEE;
    if v <= K {
        v.max(0.0)
    } else {
        K + (1.0 - K) * (1.0 - (-(v - K) / (1.0 - K)).exp())
    }
}

/// Weißabgleich des ganzen Bildes: Kelvin (wenn gesetzt und die Quelle ihre
/// Aufnahme-Temperatur kennt) und relativ. Für CPU und GPU dieselben Zahlen.
pub(crate) fn global_white_balance(p: &DevelopParams, as_shot: Option<f32>) -> [f32; 3] {
    let rel = white_balance(p.temperature / 100.0, p.tint / 100.0);
    match as_shot {
        Some(k0) if p.kelvin > 0.0 => {
            let k = crate::wb::kelvin_multipliers(k0, p.kelvin);
            let m = [rel[0] * k[0], rel[1] * k[1], rel[2] * k[2]];
            let y = luma(m);
            m.map(|v| v / y)
        }
        _ => rel,
    }
}

/// Multiplikatoren für einen relativen Weißabgleich. Die Gesamthelligkeit bleibt gleich.
pub(crate) fn white_balance(temperature: f32, tint: f32) -> [f32; 3] {
    let r = (0.35 * temperature).exp();
    let b = (-0.35 * temperature).exp();
    let g = (-0.25 * tint).exp();
    let y = luma([r, g, b]);
    [r / y, g / y, b / y]
}

/// Tonwertabbildung für RAWs wie darktable: sigmoid in Rec.2020 (siehe
/// [`crate::dt::sigmoid`]). Ein- und Ausgabe lineares sRGB; Werte außerhalb
/// von sRGB beschneidet erst die Kodierung (wie darktables colorout).
pub(crate) struct RawTone {
    pub(crate) to_rec2020: crate::dt::color::M3f,
    pub(crate) to_srgb: crate::dt::color::M3f,
    pub(crate) sigmoid: crate::dt::sigmoid::Sigmoid,
}

pub(crate) fn raw_tone_setup() -> &'static RawTone {
    static T: std::sync::OnceLock<RawTone> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let (to, back) = crate::dt::color::srgb_rec2020();
        RawTone {
            to_rec2020: crate::dt::color::to_f32(&to),
            to_srgb: crate::dt::color::to_f32(&back),
            sigmoid: crate::dt::sigmoid::Sigmoid::default_params(),
        }
    })
}

#[inline]
fn raw_tone(c: [f32; 3]) -> [f32; 3] {
    use crate::dt::color::apply;
    let t = raw_tone_setup();
    apply(&t.to_srgb, t.sigmoid.apply(apply(&t.to_rec2020, c)))
}

/// Weiß und Schwarz (−1…1) auf den Tonwert `v` anwenden.
#[inline]
fn tone_ends(v: f32, whites: f32, blacks: f32) -> f32 {
    let mut v = v;
    if whites != 0.0 {
        let w = smoothstep(WHITES_RANGE.0, WHITES_RANGE.1, v);
        v += whites * WHITES_GAIN * w * w;
    }
    if blacks != 0.0 {
        let b = 1.0 - smoothstep(BLACKS_RANGE.0, BLACKS_RANGE.1, v);
        v += blacks * BLACKS_GAIN * b * b;
    }
    v
}

/// S-Kurve um die Mitte. Positive Werte erhöhen, negative senken den Kontrast.
fn s_curve(v: f32, amount: f32) -> f32 {
    if amount == 0.0 {
        return v;
    }
    let target = if amount > 0.0 {
        v * v * (3.0 - 2.0 * v) // smoothstep: steiler in der Mitte
    } else {
        0.5 - ((1.0 - 2.0 * v).asin() / 3.0).sin() // Umkehrung von smoothstep
    };
    v + (target - v) * amount.abs().min(1.0) * CONTRAST_MIX
}

/// Überträgt eine Helligkeitsänderung auf RGB, ohne den Farbton zu verschieben.
#[inline]
pub(crate) fn apply_luma(c: [f32; 3], y_old: f32, y_new: f32) -> [f32; 3] {
    let out = if y_old > APPLY_LUMA_EPS {
        let k = y_new / y_old;
        [c[0] * k, c[1] * k, c[2] * k]
    } else {
        [c[0] + y_new, c[1] + y_new, c[2] + y_new]
    };
    fit_gamut(out, y_new)
}

/// Bringt Werte außerhalb 0..1 zurück, indem die Farbe in Richtung Grau
/// verschoben wird – Helligkeit und Farbton bleiben dabei erhalten.
#[inline]
fn fit_gamut(c: [f32; 3], y: f32) -> [f32; 3] {
    let y = y.clamp(0.0, 1.0);
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let mut k: f32 = 1.0;
    if max > 1.0 && max > y {
        k = k.min((1.0 - y) / (max - y));
    }
    if min < 0.0 && min < y {
        k = k.min(y / (y - min));
    }
    if k < 1.0 {
        [y + (c[0] - y) * k, y + (c[1] - y) * k, y + (c[2] - y) * k]
    } else {
        c
    }
    .map(|v| v.clamp(0.0, 1.0))
}

/// Unscharf maskieren auf der Helligkeit mit kleiner Schwelle gegen Rauschen.
fn sharpen(img: &ImageF32, amount: f32) -> ImageF32 {
    let (w, h) = (img.width, img.height);
    let y: Vec<f32> = img.data.par_iter().map(|c| luma(*c)).collect();
    let at = |x: isize, yy: isize| {
        y[(yy.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize]
    };
    let data = img
        .data
        .par_iter()
        .enumerate()
        .map(|(i, c)| {
            let (x, yy) = ((i % w) as isize, (i / w) as isize);
            // 3×3-Gauß
            let blur = (4.0 * at(x, yy)
                + 2.0 * (at(x - 1, yy) + at(x + 1, yy) + at(x, yy - 1) + at(x, yy + 1))
                + at(x - 1, yy - 1)
                + at(x + 1, yy - 1)
                + at(x - 1, yy + 1)
                + at(x + 1, yy + 1))
                / 16.0;
            let d = y[i] - blur;
            let d = if d.abs() < SHARPEN_THRESHOLD { 0.0 } else { d };
            let delta = amount * SHARPEN_GAIN * d;
            fit_gamut([c[0] + delta, c[1] + delta, c[2] + delta], y[i] + delta)
        })
        .collect();
    ImageF32 {
        width: w,
        height: h,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_base_look_keeps_slider_ends() {
        let p = |c: f32, s: f32, v: f32, mono: bool| DevelopParams {
            contrast: c,
            saturation: s,
            vibrance: v,
            monochrome: mono,
            ..Default::default()
        };
        // Regler auf 0: Grundabstimmung, nur bei RAWs
        assert_eq!(
            base_look(&p(0.0, 0.0, 0.0, false), true),
            (RAW_BASE_CONTRAST, RAW_BASE_SATURATION, RAW_BASE_VIBRANCE)
        );
        assert_eq!(base_look(&p(0.0, 0.0, 0.0, false), false), (0.0, 0.0, 0.0));
        // Enden bleiben erreichbar: +100 voller Kontrast, −100 grau
        let (c, s, _) = base_look(&p(100.0, -100.0, 0.0, false), true);
        assert!((c - 1.0).abs() < 1e-6 && (s + 1.0).abs() < 1e-6);
        let (c, s, v) = base_look(&p(-100.0, 100.0, 100.0, false), true);
        assert!((c + 1.0).abs() < 1e-6 && (s - 1.0).abs() < 1e-6 && (v - 1.0).abs() < 1e-6);
        // Schwarzweiß: Kontrast ja, keine Farbanhebung
        assert_eq!(
            base_look(&p(0.0, 0.0, 0.0, true), true),
            (RAW_BASE_CONTRAST, 0.0, 0.0)
        );
    }
    use crate::color::srgb_to_linear;

    fn raster(values: &[[f32; 3]], w: usize) -> Source {
        let data = values
            .iter()
            .map(|c| c.map(srgb_to_linear))
            .collect::<Vec<_>>();
        let h = data.len() / w;
        Source {
            kind: SourceKind::Raster,
            image: ImageF32 {
                width: w,
                height: h,
                data,
            },
            full_width: w,
            full_height: h,
            lens: None,
            as_shot_kelvin: None,
        }
    }

    fn gradient() -> Source {
        let v: Vec<[f32; 3]> = (0..256)
            .map(|i| {
                let t = i as f32 / 255.0;
                [t, (t * 0.7 + 0.1).min(1.0), 1.0 - t]
            })
            .collect();
        raster(&v, 16)
    }

    /// Szene mit Struktur unter neutralem Dunst (sRGB-Werte).
    fn hazy(haze: f32) -> Source {
        let v: Vec<[f32; 3]> = (0..64 * 64)
            .map(|i| {
                let (x, y) = (i % 64, i / 64);
                let scene = if (x / 8 + y / 8) % 2 == 0 {
                    [0.55, 0.35, 0.2]
                } else {
                    [0.15, 0.3, 0.45]
                };
                scene.map(|c| c * (1.0 - haze) + 0.85 * haze)
            })
            .collect();
        raster(&v, 64)
    }

    fn spread(img: &ImageF32) -> f32 {
        let y: Vec<f32> = img.data.iter().map(|c| luma(*c)).collect();
        let m = y.iter().sum::<f32>() / y.len() as f32;
        (y.iter().map(|v| (v - m).powi(2)).sum::<f32>() / y.len() as f32).sqrt()
    }

    #[test]
    fn dehaze_restores_contrast_and_keeps_hue() {
        let src = hazy(0.5);
        let p = |dehaze| DevelopParams {
            dehaze,
            ..Default::default()
        };
        let before = render(&src, &p(0.0));
        let after = render(&src, &p(80.0));
        assert!(
            spread(&after) > spread(&before) * 1.3,
            "kein Kontrastgewinn"
        );
        // Farbton je Pixel bleibt: Reihenfolge der Kanäle unverändert
        for (a, b) in before.data.iter().zip(&after.data) {
            let order = |c: &[f32; 3]| (c[0] > c[1], c[1] > c[2], c[0] > c[2]);
            assert_eq!(order(a), order(b), "{a:?} → {b:?}");
        }
        // Neutrales bleibt neutral (kein Blau- oder Gelbstich)
        let gray = raster(&[[0.6, 0.6, 0.6]; 16], 4);
        let g = render(&gray, &p(100.0));
        for c in &g.data {
            assert!(
                (c[0] - c[2]).abs() < 1e-5 && (c[0] - c[1]).abs() < 1e-5,
                "{c:?}"
            );
        }
        // Negativ: Dunst kommt dazu
        let more = render(&hazy(0.1), &p(-80.0));
        assert!(spread(&more) < spread(&render(&hazy(0.1), &p(0.0))) * 0.7);
    }

    /// Wie stark ein Regler bei +100 einen gleichmäßigen Ton `v` verschiebt.
    fn tone_shift(v: f32, set: fn(&mut DevelopParams)) -> f32 {
        let src = raster(&[[v; 3]; 64], 8);
        let mut p = DevelopParams::default();
        set(&mut p);
        render(&src, &p).data[27][1] - render(&src, &DevelopParams::default()).data[27][1]
    }

    #[test]
    fn tone_sliders_work_on_their_part_of_the_range() {
        let lights = |p: &mut DevelopParams| p.highlights = 100.0;
        let darks = |p: &mut DevelopParams| p.shadows = 100.0;
        let whites = |p: &mut DevelopParams| p.whites = 100.0;
        let blacks = |p: &mut DevelopParams| p.blacks = -100.0;
        let row = |f: fn(&mut DevelopParams)| [0.05, 0.2, 0.5, 0.7, 0.9].map(|v| tone_shift(v, f));
        let (l, d, w, b) = (row(lights), row(darks), row(whites), row(blacks));
        // Lichter: helle Töne und obere Mitteltöne, Schatten kaum
        assert!(l[3] > 0.05 && l[2] > 0.03 && l[0].abs() < 0.01, "{l:?}");
        // Tiefen: dunkle Töne und untere Mitteltöne, Lichter kaum
        assert!(d[1] > 0.05 && d[2] > 0.02 && d[4].abs() < 0.01, "{d:?}");
        // Weiß: nur das obere Ende, Mitteltöne fast unverändert
        assert!(
            w[4] > 0.05 && w[2].abs() < 0.015 && w[0].abs() < 1e-3,
            "{w:?}"
        );
        assert!(
            w[2] < l[2] * 0.2,
            "Weiß wirkt kaum auf die Mitte, Lichter schon"
        );
        // Schwarz: nur das untere Ende
        assert!(
            b[0] < -0.03 && b[2].abs() < 0.015 && b[4].abs() < 1e-3,
            "{b:?}"
        );
    }

    #[test]
    fn monochrome_is_gray_mixes_with_hsl_and_tones_with_grading() {
        use crate::params::{ColorGrade, GradeWheel, Hsl};
        let src = raster(
            &[[0.2, 0.3, 0.8], [0.8, 0.3, 0.2], [0.5; 3], [0.3, 0.7, 0.3]],
            4,
        );
        let bw = DevelopParams {
            monochrome: true,
            ..Default::default()
        };
        let out = render(&src, &bw);
        for c in &out.data {
            assert!(
                (c[0] - c[1]).abs() < 1e-5 && (c[1] - c[2]).abs() < 1e-5,
                "nicht grau: {c:?}"
            );
        }
        // HSL-Luminanz Blau wirkt als Grau-Mix: Blau wird dunkler, Rot bleibt
        let mut hsl = Hsl::default();
        hsl.luminance[5] = -60.0;
        let darker = render(&src, &DevelopParams { hsl, ..bw.clone() });
        assert!(
            darker.data[0][0] < out.data[0][0] - 0.02,
            "Blau nicht dunkler"
        );
        assert!(
            (darker.data[1][0] - out.data[1][0]).abs() < 0.01,
            "Rot verändert"
        );
        // Color Grading tont das Schwarzweißbild
        let toned = render(
            &src,
            &DevelopParams {
                color_grade: ColorGrade {
                    shadows: GradeWheel {
                        hue: 240.0,
                        saturation: 80.0,
                        luminance: 0.0,
                    },
                    ..Default::default()
                },
                ..bw.clone()
            },
        );
        assert!(
            toned.data.iter().any(|c| c[2] > c[0] + 0.02),
            "keine Tonung: {:?}",
            toned.data
        );
    }

    #[test]
    fn color_grading_tints_ranges_and_keeps_black_and_white() {
        use crate::params::{ColorGrade, GradeWheel};
        let blue_shadows = DevelopParams {
            color_grade: ColorGrade {
                shadows: GradeWheel {
                    hue: 240.0,
                    saturation: 100.0,
                    luminance: 0.0,
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let src = raster(&[[0.0; 3], [0.15; 3], [0.5; 3], [0.9; 3], [1.0; 3]], 5);
        let out = render(&src, &blue_shadows);
        let d = &out.data;
        assert!(
            d[0].iter().all(|v| *v < 1e-5),
            "Schwarz bleibt schwarz: {:?}",
            d[0]
        );
        assert!(d[1][2] > d[1][0] + 0.05, "Tiefen werden blau: {:?}", d[1]);
        assert!(
            (luma(d[1]) - 0.15).abs() < 1e-4,
            "Helligkeit bleibt: {}",
            luma(d[1])
        );
        assert!(
            (d[3][2] - d[3][0]).abs() < 1e-4,
            "Lichter unberührt: {:?}",
            d[3]
        );
        assert!(
            d[4].iter().all(|v| (*v - 1.0).abs() < 1e-5),
            "Weiß bleibt weiß"
        );
        // Neutral (nur Überblenden/Abgleich verstellt) ändert nichts
        let neutral = DevelopParams {
            color_grade: ColorGrade {
                blending: 10.0,
                balance: 80.0,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            render(&src, &neutral).data,
            render(&src, &DevelopParams::default()).data
        );
    }

    #[test]
    fn default_is_identity_for_raster() {
        let src = gradient();
        let out = render(&src, &DevelopParams::default());
        for (a, b) in out.data.iter().zip(src.image.data.iter()) {
            for k in 0..3 {
                assert!((a[k] - linear_to_srgb(b[k])).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn exposure_one_stop_doubles_linear() {
        let src = raster(&[[0.3, 0.3, 0.3]; 4], 2);
        let out = render(
            &src,
            &DevelopParams {
                exposure: 1.0,
                ..Default::default()
            },
        );
        let lin = srgb_to_linear(out.data[0][0]);
        assert!((lin - 2.0 * srgb_to_linear(0.3)).abs() < 2e-3);
    }

    #[test]
    fn tonal_controls_move_in_the_right_direction() {
        let src = gradient();
        let mean = |p: DevelopParams, lo: usize, hi: usize| {
            let o = render(&src, &p);
            o.data[lo..hi].iter().map(|c| luma(*c)).sum::<f32>() / (hi - lo) as f32
        };
        let base_dark = mean(DevelopParams::default(), 0, 64);
        let base_bright = mean(DevelopParams::default(), 192, 256);
        assert!(
            mean(
                DevelopParams {
                    shadows: 80.0,
                    ..Default::default()
                },
                0,
                64
            ) > base_dark
        );
        assert!(
            mean(
                DevelopParams {
                    blacks: -80.0,
                    ..Default::default()
                },
                0,
                64
            ) < base_dark
        );
        assert!(
            mean(
                DevelopParams {
                    highlights: -80.0,
                    ..Default::default()
                },
                192,
                256
            ) < base_bright
        );
        assert!(
            mean(
                DevelopParams {
                    contrast: 80.0,
                    ..Default::default()
                },
                0,
                64
            ) < base_dark
        );
        assert!(
            mean(
                DevelopParams {
                    contrast: 80.0,
                    ..Default::default()
                },
                192,
                256
            ) > base_bright
        );
    }

    #[test]
    fn saturation_and_wb() {
        let src = raster(&[[0.6, 0.4, 0.3]; 4], 2);
        let chroma = |p: DevelopParams| {
            let c = render(&src, &p).data[0];
            c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
        };
        let base = chroma(DevelopParams::default());
        assert!(
            chroma(DevelopParams {
                saturation: 50.0,
                ..Default::default()
            }) > base
        );
        let grey = chroma(DevelopParams {
            saturation: -100.0,
            ..Default::default()
        });
        assert!(grey < 1e-3);

        let warm = render(
            &src,
            &DevelopParams {
                temperature: 60.0,
                ..Default::default()
            },
        )
        .data[0];
        let cool = render(
            &src,
            &DevelopParams {
                temperature: -60.0,
                ..Default::default()
            },
        )
        .data[0];
        assert!(warm[0] - warm[2] > cool[0] - cool[2]);
    }

    #[test]
    fn output_is_always_in_range() {
        let src = gradient();
        let wild = DevelopParams {
            exposure: 3.0,
            contrast: 100.0,
            highlights: 100.0,
            shadows: 100.0,
            whites: 100.0,
            blacks: -100.0,
            clarity: 100.0,
            vibrance: 100.0,
            saturation: 100.0,
            sharpen: 150.0,
            temperature: 100.0,
            tint: -100.0,
            ..Default::default()
        };
        for c in render(&src, &wild).data {
            assert!(c.iter().all(|v| (0.0..=1.0).contains(v) && v.is_finite()));
        }
    }

    #[test]
    fn raw_tone_is_darktable_sigmoid() {
        // Szenenwerte weit über 1 mit hohem Rot: weich abgerollt, nie weiß
        let src = Source {
            kind: SourceKind::Raw,
            image: ImageF32 {
                width: 3,
                height: 1,
                data: vec![[0.1845; 3], [3.0, 1.0, 2.6], [50.0; 3]],
            },
            full_width: 3,
            full_height: 1,
            lens: None,
            as_shot_kelvin: None,
        };
        let lin = |v: f32| crate::color::srgb_to_linear(v);
        // Ohne Grundabstimmung: Mittelgrau bleibt Mittelgrau (sigmoid)
        let flat = DevelopParams {
            contrast: -100.0 * RAW_BASE_CONTRAST / (1.0 + RAW_BASE_CONTRAST),
            saturation: -100.0 * RAW_BASE_SATURATION / (1.0 + RAW_BASE_SATURATION),
            vibrance: -100.0 * RAW_BASE_VIBRANCE / (1.0 + RAW_BASE_VIBRANCE),
            ..Default::default()
        };
        let out = render(&src, &flat);
        let mid = lin(out.data[0][1]);
        assert!((mid - 0.1845).abs() < 0.01, "Mittelgrau bei {mid}");
        let hl = out.data[1];
        assert!(hl.iter().all(|v| *v < 1.0), "Lichter abgerollt: {hl:?}");
        assert!(hl[0] > hl[2] && hl[2] > hl[1], "Farbton bleibt: {hl:?}");
        let white = out.data[2];
        assert!(white.iter().all(|v| *v > 0.98), "{white:?}");
    }

    /// Bisherige Rechnung einer Maskenebene: alle Stufen auf einer Kopie des
    /// ganzen Bildes, danach überblenden.
    fn apply_local_full(img: &mut ImageF32, a: &LocalAdjust, alpha: &[f32]) {
        let mut local = img.clone();
        if a.exposure != 0.0 || a.temperature != 0.0 || a.tint != 0.0 {
            let wb = white_balance(a.temperature / 100.0, a.tint / 100.0);
            let gain = 2f32.powf(a.exposure);
            local.data.iter_mut().for_each(|px| {
                let lin = [0, 1, 2].map(|k| soft_clip(srgb_to_linear(px[k]) * wb[k] * gain));
                let y = luma(lin);
                *px = fit_gamut(
                    [
                        linear_to_srgb(lin[0]),
                        linear_to_srgb(lin[1]),
                        linear_to_srgb(lin[2]),
                    ],
                    linear_to_srgb(y),
                );
            });
        }
        tone(
            &mut local,
            &Tone {
                contrast: a.contrast / 100.0,
                highlights: a.highlights / 100.0,
                shadows: a.shadows / 100.0,
                whites: 0.0,
                blacks: 0.0,
                clarity: a.clarity / 100.0,
            },
        );
        color(&mut local, a.saturation / 100.0, 0.0);
        for ((o, l), a) in img.data.iter_mut().zip(&local.data).zip(alpha) {
            if *a > 0.0 {
                for k in 0..3 {
                    o[k] += (l[k] - o[k]) * a;
                }
            }
        }
    }

    /// Kleine Radialmaske: Rechnung nur im Rahmen der Maske ist bitgleich mit
    /// der über das ganze Bild – auch mit Klarheit/Lichtern/Tiefen, deren
    /// Helligkeitskarten Nachbarschaft außerhalb der Maske einbeziehen.
    #[test]
    fn masked_local_matches_full_image() {
        use crate::params::{MaskLayer, MaskShape};
        let (w, h) = (300, 200);
        // Entwickeltes (sRGB-kodiertes) Bild mit Struktur und Kanten
        let img = ImageF32::from_fn(w, h, |x, y| {
            let n = ((x * 7 + y * 13) % 17) as f32 / 17.0 * 0.1;
            let edge = if (x / 40 + y / 30) % 2 == 0 {
                0.25
            } else {
                0.0
            };
            [
                (0.2 + x as f32 / w as f32 * 0.6 + n + edge).min(1.0),
                (0.3 + y as f32 / h as f32 * 0.4 + n).min(1.0),
                (0.5 - edge + n).max(0.0),
            ]
        });
        let adjusts = [
            LocalAdjust {
                exposure: 0.7,
                contrast: 25.0,
                highlights: -40.0,
                shadows: 50.0,
                clarity: 60.0,
                saturation: 30.0,
                temperature: 20.0,
                tint: -10.0,
            },
            LocalAdjust {
                clarity: -50.0,
                ..Default::default()
            },
            LocalAdjust {
                contrast: -30.0,
                saturation: -60.0,
                ..Default::default()
            },
        ];
        for (cx, cy, invert) in [(0.3, 0.4, false), (0.02, 0.97, false), (0.5, 0.5, true)] {
            let layer = MaskLayer {
                shapes: vec![MaskShape::Radial {
                    cx,
                    cy,
                    rx: 0.06,
                    ry: 0.04,
                    angle: 25.0,
                    feather: 0.5,
                    invert,
                    subtract: false,
                }],
                ..Default::default()
            };
            let alpha = masks::evaluate(&layer, w, h, &NoMasks, None);
            assert!(alpha.iter().any(|a| *a > 0.0) && alpha.contains(&0.0));
            for a in &adjusts {
                let mut fast = img.clone();
                let mut full = img.clone();
                apply_local_masked(&mut fast, a, &alpha);
                apply_local_full(&mut full, a, &alpha);
                for (p, q) in fast.data.iter().zip(&full.data) {
                    assert_eq!(p.map(f32::to_bits), q.map(f32::to_bits), "{cx},{cy}: {a:?}");
                }
                assert!(fast.data != img.data, "Testaufbau: Ebene wirkt");
            }
        }
        // Leere Maske: nichts geschieht
        let mut same = img.clone();
        apply_local_masked(&mut same, &adjusts[0], &vec![0.0; w * h]);
        assert!(same.data == img.data);
    }

    #[test]
    fn local_adjustment_follows_mask_and_survives_rotation() {
        use crate::params::{LocalAdjust, MaskLayer, MaskShape};
        // 40×20 mittelgraues Bild; Verlauf hellt nur die obere Hälfte auf
        let src = raster(&vec![[0.4, 0.4, 0.4]; 40 * 20], 40);
        let layer = MaskLayer {
            shapes: vec![MaskShape::Linear {
                x0: 0.5,
                y0: 0.0,
                x1: 0.5,
                y1: 0.45,
                invert: false,
                subtract: false,
            }],
            adjust: LocalAdjust {
                exposure: 1.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let p = DevelopParams {
            masks: vec![layer],
            ..Default::default()
        };
        let out = render(&src, &p);
        let top = out.data[40 + 20][0];
        let bottom = out.data[18 * 40 + 20][0];
        assert!(
            top > 0.5 && (bottom - 0.4).abs() < 0.01,
            "oben {top}, unten {bottom}"
        );

        // Gedreht: die aufgehellte Seite liegt jetzt rechts (90° im Uhrzeigersinn)
        let rotated = render(
            &src,
            &DevelopParams {
                rotate: 1,
                ..p.clone()
            },
        );
        assert_eq!((rotated.width, rotated.height), (20, 40));
        let right = rotated.data[20 * 20 + 18][0];
        let left = rotated.data[20 * 20 + 1][0];
        assert!(
            right > 0.5 && (left - 0.4).abs() < 0.01,
            "rechts {right}, links {left}"
        );

        // Deaktivierte Ebene wirkt nicht, Überlagerung färbt rot
        let mut off = p.clone();
        off.masks[0].enabled = false;
        assert!((render(&src, &off).data[40 + 20][0] - 0.4).abs() < 0.01);
        let ov = render_with(
            &src,
            &off,
            &RenderOptions {
                overlay: Some(0),
                ..Default::default()
            },
        );
        let c = ov.data[40 + 20];
        assert!(c[0] > c[1] + 0.2, "rot überlagert: {c:?}");
    }

    #[test]
    fn repairs_after_lens_need_the_correction() {
        use crate::repair::{Patch, PatchPart};
        struct One(Arc<Patch>);
        impl RepairSource for One {
            fn get(&self, _: &str) -> Option<Arc<Patch>> {
                Some(self.0.clone())
            }
        }
        let patch = Arc::new(Patch {
            width: 8,
            height: 8,
            parts: vec![PatchPart {
                x: 2,
                y: 2,
                w: 4,
                h: 4,
                rgb: vec![[1.0, 0.0, 0.0]; 16],
                alpha: vec![255; 16],
            }],
        });
        let source = One(patch);
        let opts = RenderOptions {
            repairs: &source,
            ..Default::default()
        };
        let run = |lens_flag: bool| {
            let p = DevelopParams {
                repairs: vec![crate::params::RepairLayer {
                    enabled: true,
                    result: Some("a".into()),
                    lens: lens_flag,
                    ..Default::default()
                }],
                ..Default::default()
            };
            let mut img = ImageF32::from_fn(8, 8, |_, _| [0.2; 3]);
            prepare_source(&mut img, &p, &opts, None);
            (img.data[3 * 8 + 3], has_repairs(&p, false))
        };
        // Auf dem korrigierten Bild berechnet, aber ohne Korrektur: übersprungen
        let (px, any) = run(true);
        assert!((px[0] - 0.2).abs() < 1e-6 && !any, "{px:?}");
        // Aus dem Original: eingesetzt
        let (px, any) = run(false);
        assert!(px[0] > 0.9 && any, "{px:?}");
    }
}
