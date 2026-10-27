//! Vorschaubilder: kleine Thumbnails fürs Raster und größere Vorschauen
//! für die Einzelansicht. Beide werden als JPEG im Cache-Ordner abgelegt.
//!
//! Unbearbeitete RAWs nutzen das eingebettete Kamera-JPEG – das ist um ein
//! Vielfaches schneller als eine Entwicklung. Sobald ein Bild bearbeitet ist,
//! entstehen Thumbnail und Vorschau über die Entwicklungspipeline und zeigen
//! damit die Bearbeitung.

use crate::LockExt;
use crate::catalog::{Catalog, ThumbJob};
use crate::error::{Error, Result};
use crate::formats::ImageKind;
use crate::maskstore::MaskFiles;
use image::metadata::Orientation;
use image::{DynamicImage, ImageFormat};
use lumenkorn_develop::repair::{NoRepairs, RepairFiles, RepairSource};
use lumenkorn_develop::{DevelopParams, MaskSource, NoMasks, RenderOptions, Source};
use rayon::prelude::*;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;

/// Längste Kante der Raster-Thumbnails.
pub const THUMB_SIZE: u32 = 512;
/// Längste Kante der Vorschau in der Einzelansicht.
pub const PREVIEW_SIZE: u32 = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Thumb,
    Preview,
}

impl Variant {
    fn max_size(self) -> u32 {
        match self {
            Variant::Thumb => THUMB_SIZE,
            Variant::Preview => PREVIEW_SIZE,
        }
    }
    fn dir(self) -> &'static str {
        match self {
            Variant::Thumb => "thumbs",
            Variant::Preview => "previews",
        }
    }
}

/// Ablage der erzeugten Vorschauen, z. B. `<cache>/thumbs/2a/42.jpg`.
#[derive(Debug, Clone)]
pub struct ThumbCache {
    base: PathBuf,
    /// KI-Masken, damit Vorschauen lokale Anpassungen damit zeigen.
    masks: Option<Arc<MaskFiles>>,
    /// Ordner mit entrauschten Zwischenstufen
    denoise_dir: Option<PathBuf>,
    /// Flicken der KI-Reparaturen
    repairs: Option<Arc<RepairFiles>>,
}

impl ThumbCache {
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self {
            base: base.into(),
            masks: None,
            denoise_dir: None,
            repairs: None,
        }
    }

    pub fn with_masks(mut self, masks: Arc<MaskFiles>) -> Self {
        self.masks = Some(masks);
        self
    }

    pub fn with_denoise_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.denoise_dir = Some(dir.into());
        self
    }

    pub fn with_repairs(mut self, repairs: Arc<RepairFiles>) -> Self {
        self.repairs = Some(repairs);
        self
    }

    fn repair_source(&self) -> &dyn RepairSource {
        match &self.repairs {
            Some(r) => r.as_ref(),
            None => &NoRepairs,
        }
    }

    fn mask_source(&self) -> &dyn MaskSource {
        match &self.masks {
            Some(m) => m.as_ref(),
            None => &NoMasks,
        }
    }

    /// Wurzel des Vorschau-Speichers
    pub fn base(&self) -> &Path {
        &self.base
    }

    fn path_for(&self, variant: Variant, id: i64) -> PathBuf {
        self.base
            .join(variant.dir())
            .join(format!("{:02x}", id.rem_euclid(256)))
            .join(format!("{id}.jpg"))
    }

    pub fn read(&self, variant: Variant, id: i64) -> Option<Vec<u8>> {
        std::fs::read(self.path_for(variant, id)).ok()
    }

    pub fn write(&self, variant: Variant, id: i64, jpeg: &[u8]) -> Result<()> {
        let p = self.path_for(variant, id);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Erst temporär schreiben, dann umbenennen: nie halbe Dateien im Cache.
        // Eindeutiger Name: Worker und Bildprotokoll können dasselbe Bild
        // gleichzeitig schreiben (unter Windows schlägt sonst das Umbenennen fehl)
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = p.with_extension(format!("{}.{n}.tmp", std::process::id()));
        std::fs::write(&tmp, jpeg)?;
        std::fs::rename(tmp, p)?;
        Ok(())
    }

    /// Erster Teil von [`Self::write`]: Verzeichnis anlegen und die Bytes in
    /// eine eindeutig benannte Temp-Datei neben dem Ziel schreiben. Langsam,
    /// darf deshalb ohne Katalogsperre laufen. Danach [`Self::commit_staged`]
    /// oder [`Self::discard_staged`].
    fn stage(&self, variant: Variant, id: i64, jpeg: &[u8]) -> Result<PathBuf> {
        let p = self.path_for(variant, id);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = p.with_extension(format!("{}.{n}.tmp", std::process::id()));
        if let Err(e) = std::fs::write(&tmp, jpeg) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(tmp)
    }

    /// Zweiter Teil: Temp-Datei an ihren Platz umbenennen (schnell).
    fn commit_staged(&self, variant: Variant, id: i64, tmp: &Path) -> Result<()> {
        std::fs::rename(tmp, self.path_for(variant, id))?;
        Ok(())
    }

    fn discard_staged(tmp: &Path) {
        let _ = std::fs::remove_file(tmp);
    }

    pub fn remove(&self, id: i64) {
        for v in [Variant::Thumb, Variant::Preview] {
            let _ = std::fs::remove_file(self.path_for(v, id));
        }
    }

    /// Liefert die Vorschau aus dem Cache oder erzeugt sie bei Bedarf.
    pub fn get_or_render(&self, variant: Variant, job: &ThumbJob) -> Result<Vec<u8>> {
        if let Some(bytes) = self.read(variant, job.id) {
            return Ok(bytes);
        }
        let bytes = render_job(
            job,
            variant.max_size(),
            self.mask_source(),
            self.denoise_dir.as_deref(),
            self.repair_source(),
        )?;
        self.write(variant, job.id, &bytes)?;
        Ok(bytes)
    }

    /// Ganzes Bild in voller Auflösung (Zoom in Bibliothek und Vergleich).
    /// Wird nicht auf der Platte zwischengespeichert.
    pub fn render_full(&self, job: &ThumbJob) -> Result<Vec<u8>> {
        render_full(
            job,
            self.mask_source(),
            self.denoise_dir.as_deref(),
            self.repair_source(),
        )
    }
}

/// Volle Auflösung: Bearbeitete Bilder und RAWs werden entwickelt (das
/// eingebettete JPEG eines RAWs ist oft kleiner als das Original),
/// unbearbeitete JPEGs ohne Drehung unverändert geliefert, andere
/// Rasterbilder dekodiert.
fn render_full(
    job: &ThumbJob,
    masks: &dyn MaskSource,
    denoise_dir: Option<&Path>,
    repairs: &dyn RepairSource,
) -> Result<Vec<u8>> {
    let edit = job
        .edit
        .as_ref()
        .map(DevelopParams::from_json)
        .filter(|p| !p.is_default());
    if let Some(p) = edit {
        return render_developed(job, &p, u32::MAX, masks, denoise_dir, repairs);
    }
    if job.kind == ImageKind::Raw {
        if let Some(bytes) = full_size_preview(job)? {
            return Ok(bytes);
        }
        return render_developed(
            job,
            &DevelopParams::default(),
            u32::MAX,
            masks,
            None,
            repairs,
        );
    }
    if serves_original(job) {
        return Ok(std::fs::read(&job.path)?);
    }
    render(Path::new(&job.path), job.kind, job.orientation, u32::MAX)
}

/// Liefert [`ThumbCache::render_full`] die Datei unverändert? Für ein unbearbeitetes,
/// nicht zu drehendes JPEG ist die Datei selbst das Ergebnis – Dekodieren und
/// neu Kodieren kostete ~0,5 s je 24 MP und Qualität (q82). Wie bei
/// `full_size_preview` für RAWs. (Zwischenspeichern lohnt dann nicht.)
pub fn serves_original(job: &ThumbJob) -> bool {
    job.kind == ImageKind::Raster
        && job.orientation.is_none_or(|o| o == 1)
        && job
            .edit
            .as_ref()
            .is_none_or(|e| DevelopParams::from_json(e).is_default())
        && is_jpeg(Path::new(&job.path))
        && within_read_cap(Path::new(&job.path))
}

/// Obergrenze für Dateien, die ganz in den Speicher gelesen werden (Original-
/// JPEG, RAW mit eingebetteter Vorschau). Eine präparierte Riesendatei soll
/// keine fehlgeschlagene Allokation (= Prozessabbruch) auslösen.
const MAX_WHOLE_FILE: u64 = 1 << 30;

/// Ist die Datei klein genug, um sie ganz zu lesen? (Unlesbar: ja – der
/// anschließende Lesevorgang meldet den eigentlichen Fehler.)
fn within_read_cap(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() <= MAX_WHOLE_FILE)
}

/// Beginnt die Datei mit einem JPEG-Kopf (SOI + Marker)?
fn is_jpeg(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 3];
    std::fs::File::open(path).is_ok_and(|mut f| f.read_exact(&mut head).is_ok())
        && head == [0xFF, 0xD8, 0xFF]
}

/// Die meisten Kameras betten eine JPEG-Vorschau in voller Sensorgröße ein.
/// Für unbearbeitete RAWs ist das die volle Auflösung: Sekunden schneller als
/// Entwickeln und farblich gleich zur Vorschau in der Bibliothek (die auch
/// aus dieser Datei stammt). Ohne Drehung werden die Bytes unverändert
/// geliefert. `None`, wenn die Vorschau deutlich kleiner als der Sensor ist.
fn full_size_preview(job: &ThumbJob) -> Result<Option<Vec<u8>>> {
    let Some(sensor) = job.long_edge else {
        return Ok(None);
    };
    if !within_read_cap(Path::new(&job.path)) {
        return Ok(None);
    }
    let data = std::fs::read(&job.path)?;
    let Some((jpeg, edge)) = embedded_jpegs(&data)
        .into_iter()
        .filter_map(|c| jpeg_size(c).map(|(w, h)| (c, w.max(h))))
        .max_by_key(|(_, edge)| *edge)
    else {
        return Ok(None);
    };
    if (edge as f32) < sensor as f32 * 0.9 {
        return Ok(None);
    }
    let orientation = job
        .orientation
        .and_then(|o| Orientation::from_exif(o as u8))
        .filter(|o| *o != Orientation::NoTransforms);
    let Some(o) = orientation else {
        return Ok(Some(jpeg.to_vec()));
    };
    let mut img = image::load(Cursor::new(jpeg), ImageFormat::Jpeg)?;
    img.apply_orientation(o);
    Ok(Some(encode_jpeg(&img, 90)?))
}

/// Vorschau für einen Katalogeintrag – mit Bearbeitung, falls vorhanden.
fn render_job(
    job: &ThumbJob,
    max: u32,
    masks: &dyn MaskSource,
    denoise_dir: Option<&Path>,
    repairs: &dyn RepairSource,
) -> Result<Vec<u8>> {
    let edit = job
        .edit
        .as_ref()
        .map(DevelopParams::from_json)
        .filter(|p| !p.is_default());
    if let Some(p) = edit {
        return render_developed(job, &p, max, masks, denoise_dir, repairs);
    }
    match render(Path::new(&job.path), job.kind, job.orientation, max) {
        Ok(bytes) => Ok(bytes),
        // RAW ohne brauchbares eingebettetes JPEG: selbst entwickeln.
        Err(e) if job.kind == ImageKind::Raw => {
            render_developed(job, &DevelopParams::default(), max, masks, None, repairs)
                .map_err(|_| e)
        }
        Err(e) => Err(e),
    }
}

fn render_developed(
    job: &ThumbJob,
    params: &DevelopParams,
    max: u32,
    masks: &dyn MaskSource,
    denoise_dir: Option<&Path>,
    repairs: &dyn RepairSource,
) -> Result<Vec<u8>> {
    // Fehlt die Entrauschungsdatei oder ist sie defekt, wird ohne gerendert.
    // Das Vorschaubild gilt trotzdem als fertig (ein Fehlerzustand ließe es
    // dauerhaft fehlen, obwohl das Foto selbst lesbar ist) – daher nur warnen.
    let denoised = match (&params.denoise, denoise_dir) {
        (Some(d), Some(dir)) => {
            let read = lumenkorn_develop::denoised::path(dir, &d.id)
                .and_then(|p| lumenkorn_develop::denoised::read(&p, Some(max as usize)).ok());
            if read.is_none() {
                log::warn!(
                    "Foto {}: Entrauschungsdatei {} fehlt oder ist defekt, Vorschau ohne",
                    job.id,
                    d.id
                );
            }
            read
        }
        _ => None,
    };
    let src = Source::load(
        Path::new(&job.path),
        job.kind == ImageKind::Raw,
        job.orientation.map(|o| o as u16),
        Some(max as usize),
    )?;
    let rgb = lumenkorn_develop::render_auto_rgb8(
        &src,
        params,
        &RenderOptions {
            masks,
            overlay: None,
            denoised: denoised.as_ref(),
            region: None,
            repairs,
            keep_prepared: false,
        },
    );
    Ok(lumenkorn_develop::output::encode_jpeg_rgb8(&rgb, 84)?)
}

/// Dekodiert ein Bild, skaliert es auf `max` und liefert ein JPEG.
fn render(path: &Path, kind: ImageKind, orientation: Option<i64>, max: u32) -> Result<Vec<u8>> {
    encode_jpeg(&decode_unedited(path, kind, orientation, max)?, 82)
}

/// Unbearbeitetes, aufrecht gedrehtes Bild mit höchstens `max` Pixeln Kante.
/// RAWs liefern ihr eingebettetes JPEG – schnell, ohne Entwicklung.
fn decode_unedited(
    path: &Path,
    kind: ImageKind,
    orientation: Option<i64>,
    max: u32,
) -> Result<DynamicImage> {
    // Lineares EXR (Ergebnis von HDR/Panorama/Focus Stacking): wie ein RAW
    // mit Standardeinstellungen entwickeln
    if lumenkorn_develop::exr_io::is_exr(path) {
        return develop_default(path, false, orientation, max);
    }
    let img = match kind {
        ImageKind::Raster => lumenkorn_develop::raster::open(path)?,
        ImageKind::Raw => match decode_embedded_preview(path, max) {
            Ok(img) => img,
            // Ohne eingebettete Vorschau (z. B. DNGs aus Entrauschungs-
            // programmen): mit Standardeinstellungen entwickeln, statt bei
            // jedem Start erneut zu scheitern
            Err(Error::NoPreview(_)) => return develop_default(path, true, orientation, max),
            Err(e) => return Err(e),
        },
    };
    Ok(fit(img, orientation, max))
}

/// Mit Standardeinstellungen entwickeln (RAW ohne Vorschau, EXR).
fn develop_default(
    path: &Path,
    raw: bool,
    orientation: Option<i64>,
    max: u32,
) -> Result<DynamicImage> {
    let src = Source::load(path, raw, orientation.map(|o| o as u16), Some(max as usize))?;
    let rgb = lumenkorn_develop::render_auto_rgb8(
        &src,
        &DevelopParams::default(),
        &RenderOptions {
            masks: &NoMasks,
            overlay: None,
            denoised: None,
            region: None,
            repairs: &NoRepairs,
            keep_prepared: false,
        },
    );
    Ok(DynamicImage::ImageRgb8(rgb))
}

/// Auf `max` verkleinern und aufrichten.
fn fit(img: DynamicImage, orientation: Option<i64>, max: u32) -> DynamicImage {
    let mut img = if img.width() > max || img.height() > max {
        img.thumbnail(max, max)
    } else {
        img
    };
    if let Some(o) = orientation.and_then(|o| Orientation::from_exif(o as u8)) {
        img.apply_orientation(o);
    }
    img
}

/// JPEG-Vorschau einer Datei, die (noch) nicht im Katalog ist – für den
/// Import von einer Speicherkarte. Bei RAWs zuerst nur das erste MB: Bei
/// Sony liegt die 1616-px-Vorschau in den ersten 200 KB, und die Karte ist
/// langsam (der übliche Weg liest bis zu 16 MB je Datei).
pub fn file_preview(
    path: &Path,
    kind: ImageKind,
    orientation: Option<i64>,
    max: u32,
) -> Result<Vec<u8>> {
    let from_head = (kind == ImageKind::Raw)
        .then(|| {
            use std::io::Read;
            let mut head = Vec::new();
            std::fs::File::open(path)
                .ok()?
                .take(1 << 20)
                .read_to_end(&mut head)
                .ok()?;
            best_preview(&head, max, true)
        })
        .flatten();
    let img = match from_head {
        Some(img) => fit(img, orientation, max),
        None => decode_unedited(path, kind, orientation, max)?,
    };
    encode_jpeg(&img, 80)
}

/// Bild für die Analyse (Gesichter): unbearbeitet, damit Positionen auch nach
/// späterem Zuschneiden gelten. RAWs ohne brauchbares eingebettetes JPEG
/// werden mit Standardeinstellungen entwickelt.
pub fn decode_for_analysis(job: &ThumbJob, max: u32) -> Result<DynamicImage> {
    match decode_unedited(Path::new(&job.path), job.kind, job.orientation, max) {
        Ok(img) => Ok(img),
        Err(e) if job.kind == ImageKind::Raw => {
            let bytes = render_developed(
                job,
                &DevelopParams::default(),
                max,
                &NoMasks,
                None,
                &NoRepairs,
            )
            .map_err(|_| e)?;
            Ok(image::load_from_memory(&bytes)?)
        }
        Err(e) => Err(e),
    }
}

fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    // JPEG-Quellen sind schon RGB8: keine Kopie (bei 24 MP 72 MB)
    let converted;
    let rgb = match img {
        DynamicImage::ImageRgb8(rgb) => rgb,
        other => {
            converted = other.to_rgb8();
            &converted
        }
    };
    Ok(lumenkorn_develop::output::encode_jpeg_rgb8(rgb, quality)?)
}

/// Dekodiert eine eingebettete JPEG-Vorschau des RAWs. Funktioniert
/// unabhängig vom Containerformat (TIFF-basierte RAWs wie NEF/CR2/ARW/DNG,
/// aber auch CR3 und RAF).
///
/// Gewählt wird die *kleinste* Vorschau, deren längere Kante mindestens
/// `min_edge` misst – für ein 512-px-Thumbnail reicht meist die mittlere
/// Vorschau, statt das 24–60-MP-JPEG voll zu dekodieren. Gibt es keine
/// passende, die größte.
fn decode_embedded_preview(path: &Path, min_edge: u32) -> Result<DynamicImage> {
    // Die Vorschauen liegen fast immer vorn in der Datei (TIFF-IFDs, CR3-PRVW,
    // RAF-Kopf). Erst den Anfang lesen, die ganze Datei (24–60 MB, bei 50 000
    // RAWs Terabytes auf dem NAS) nur, wenn dort nichts Passendes steckt.
    use std::io::Read;
    const HEAD: u64 = 16 << 20;
    let mut head = Vec::new();
    let file_len = {
        let mut f = std::fs::File::open(path)?;
        let len = f.metadata()?.len();
        f.by_ref().take(HEAD).read_to_end(&mut head)?;
        len
    };
    if file_len > HEAD
        && let Some(img) = best_preview(&head, min_edge, true)
    {
        return Ok(img);
    }
    let data = if file_len > HEAD {
        if file_len > MAX_WHOLE_FILE {
            return Err(Error::NoPreview(path.display().to_string()));
        }
        std::fs::read(path)?
    } else {
        head
    };
    best_preview(&data, min_edge, false).ok_or_else(|| Error::NoPreview(path.display().to_string()))
}

/// Dekodiert die kleinste eingebettete Vorschau mit mindestens `min_edge`
/// Pixeln Kante, sonst die größte. `only_sufficient`: nur eine ausreichend
/// große akzeptieren (für den Versuch mit dem Dateianfang).
fn best_preview(data: &[u8], min_edge: u32, only_sufficient: bool) -> Option<DynamicImage> {
    let mut candidates: Vec<(&[u8], u32)> = embedded_jpegs(data)
        .into_iter()
        // Größe aus dem Kopf lesen; „JPEGs“ mit verlustfreien Sensordaten
        // (SOF3) lassen sich nicht als Bild dekodieren und fallen hier weg.
        .filter_map(|c| jpeg_size(c).map(|(w, h)| (c, w.max(h))))
        // EXIF-Thumbnails (160 × 120) sind fürs Raster zu klein
        .filter(|(_, edge)| *edge >= 160)
        .filter(|(_, edge)| !only_sufficient || *edge >= min_edge)
        .collect();
    // Passende aufsteigend, danach die zu kleinen absteigend
    candidates.sort_by_key(|(_, edge)| {
        if *edge >= min_edge {
            (0, *edge as i64)
        } else {
            (1, -(*edge as i64))
        }
    });
    candidates
        .into_iter()
        .find_map(|(c, _)| image::load(Cursor::new(c), ImageFormat::Jpeg).ok())
}

/// Breite und Höhe eines JPEG aus seinem Frame-Kopf (SOF0/1/2), ohne zu
/// dekodieren. `None` für verlustfreie oder unbekannte Varianten.
fn jpeg_size(d: &[u8]) -> Option<(u32, u32)> {
    let mut pos = 2;
    while pos + 9 < d.len() {
        if d[pos] != 0xFF {
            return None;
        }
        let marker = d[pos + 1];
        if marker == 0xFF {
            pos += 1;
            continue;
        }
        let len = u16::from_be_bytes([d[pos + 2], d[pos + 3]]) as usize;
        match marker {
            0xC0..=0xC2 => {
                let h = u16::from_be_bytes([d[pos + 5], d[pos + 6]]) as u32;
                let w = u16::from_be_bytes([d[pos + 7], d[pos + 8]]) as u32;
                return (w > 0 && h > 0).then_some((w, h));
            }
            // Andere SOF-Varianten (verlustfrei, arithmetisch) oder Scanbeginn
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF | 0xDA => return None,
            _ => pos += 2 + len,
        }
    }
    None
}

/// Findet vollständige JPEG-Datenströme (SOI…EOI) in einem Puffer.
fn embedded_jpegs(data: &[u8]) -> Vec<&[u8]> {
    // Kürzere Ströme sind nur EXIF-Thumbnails, nie eine brauchbare Vorschau
    const MIN_LEN: usize = 8 * 1024;
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < data.len() {
        // Zum nächsten 0xFF springen statt jedes Byte einzeln zu prüfen
        // (bis 16 MB je RAW)
        match find_ff(&data[i..data.len() - 3]) {
            Some(k) => i += k,
            None => break,
        }
        if data[i] == 0xFF
            && data[i + 1] == 0xD8
            && data[i + 2] == 0xFF
            && let Some(len) = jpeg_len(&data[i..])
        {
            if len >= MIN_LEN {
                out.push(&data[i..i + len]);
            }
            // Über das gefundene JPEG springen – darin eingebettete
            // EXIF-Thumbnails interessieren uns nicht.
            i += len;
            continue;
        }
        i += 1;
    }
    out
}

/// Position des ersten 0xFF. Blockweise ohne frühen Abbruch je Byte, damit
/// der Compiler den Vergleich vektorisiert (memchr ist keine direkte
/// Abhängigkeit).
fn find_ff(d: &[u8]) -> Option<usize> {
    const BLOCK: usize = 32;
    let mut chunks = d.chunks_exact(BLOCK);
    let mut base = 0;
    for c in chunks.by_ref() {
        if c.iter().fold(false, |acc, &b| acc | (b == 0xFF)) {
            return c.iter().position(|&b| b == 0xFF).map(|k| base + k);
        }
        base += BLOCK;
    }
    chunks
        .remainder()
        .iter()
        .position(|&b| b == 0xFF)
        .map(|k| base + k)
}

/// Länge eines JPEG-Streams anhand seiner Marker-Struktur.
fn jpeg_len(d: &[u8]) -> Option<usize> {
    let mut pos = 2; // nach SOI
    loop {
        // Füllbytes 0xFF überspringen
        while pos < d.len() && d[pos] == 0xFF && d.get(pos + 1) == Some(&0xFF) {
            pos += 1;
        }
        if pos + 1 >= d.len() || d[pos] != 0xFF {
            return None;
        }
        let marker = d[pos + 1];
        match marker {
            0xD9 => return Some(pos + 2), // EOI
            0xD0..=0xD7 | 0x01 => {
                pos += 2;
                continue;
            }
            _ => {}
        }
        if pos + 3 >= d.len() {
            return None;
        }
        let seg_len = u16::from_be_bytes([d[pos + 2], d[pos + 3]]) as usize;
        if seg_len < 2 {
            return None;
        }
        pos += 2 + seg_len;
        if marker == 0xDA {
            // Entropiecodierte Daten bis zum nächsten echten Marker überspringen.
            while pos + 1 < d.len() {
                if d[pos] == 0xFF {
                    let m = d[pos + 1];
                    if m == 0x00 || (0xD0..=0xD7).contains(&m) {
                        pos += 2;
                        continue;
                    }
                    if m == 0xFF {
                        pos += 1;
                        continue;
                    }
                    break;
                }
                pos += 1;
            }
        }
        if pos > d.len() {
            return None;
        }
    }
}

/// Bearbeitete Bilder, die gleichzeitig für Thumbnails entwickelt werden
const EDITED_PARALLEL: usize = 2;

/// Erzeugt alle ausstehenden Thumbnails parallel.
///
/// `on_done(id, ok)` wird pro Bild aufgerufen, damit die Oberfläche das
/// Thumbnail sofort nachladen kann. Liefert die Anzahl verarbeiteter Bilder.
pub fn generate_pending(
    catalog: &Mutex<Catalog>,
    cache: &ThumbCache,
    on_done: &(dyn Fn(i64, bool) + Sync),
) -> Result<usize> {
    let mut processed = 0;
    loop {
        let (n, more) = generate_some(catalog, cache, on_done, 256)?;
        processed += n;
        if !more {
            return Ok(processed);
        }
    }
}

/// Wie [`generate_pending`], aber höchstens etwa `limit` Bilder. Liefert
/// (verarbeitet, weitere ausstehend). Für den Hintergrund-Worker: Zwischen den
/// Portionen kommen andere Aufträge (Ordneränderungen, Scans) an die Reihe,
/// statt beim ersten Import stundenlang zu warten.
pub fn generate_some(
    catalog: &Mutex<Catalog>,
    cache: &ThumbCache,
    on_done: &(dyn Fn(i64, bool) + Sync),
    limit: usize,
) -> Result<(usize, bool)> {
    let mut processed = 0;
    while processed < limit {
        let jobs = catalog.locked().pending_thumbs(64)?;
        if jobs.is_empty() {
            return Ok((processed, false));
        }
        // Decoder laufen auf beliebigen Dateien: Ein Absturz darin wird zum
        // Fehler dieses Fotos. Sonst ginge die ganze Portion verloren, das Foto
        // bliebe ausstehend und bräche jeden weiteren Durchlauf erneut ab.
        let one = |job: &ThumbJob| -> Result<Vec<u8>> {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                render_job(
                    job,
                    THUMB_SIZE,
                    cache.mask_source(),
                    cache.denoise_dir.as_deref(),
                    cache.repair_source(),
                )
            }))
            .unwrap_or_else(|_| {
                let path = &job.path;
                Err(Error::Invalid(lumenkorn_develop::tr!(
                    "{path}: Absturz beim Dekodieren",
                    "{path}: crashed while decoding"
                )))
            })
        };
        // Unbearbeitete Bilder (eingebettete Vorschau, klein) voll parallel.
        // Bearbeitete werden ganz entwickelt – ein RAW belegt dabei bis ~1,5 GB.
        // Deshalb höchstens zwei gleichzeitig; die Pipeline selbst nutzt
        // ohnehin alle Kerne.
        let (edited, plain): (Vec<&ThumbJob>, Vec<&ThumbJob>) =
            jobs.iter().partition(|j| j.edit.is_some());
        let mut results: Vec<(&ThumbJob, Result<Vec<u8>>)> =
            plain.par_iter().map(|j| (*j, one(j))).collect();
        for pair in edited.chunks(EDITED_PARALLEL) {
            results.extend(pair.par_iter().map(|j| (*j, one(j))).collect::<Vec<_>>());
        }

        processed += results.len();

        // Langsamer Teil ohne Katalogsperre: Verzeichnis anlegen und Bytes in
        // Temp-Dateien neben dem Ziel schreiben, sonst friert die Oberfläche
        // (teilt sich die Sperre) während der Datei-E/A ein.
        let staged: Vec<(&ThumbJob, Result<PathBuf>)> = results
            .into_iter()
            .map(|(job, res)| {
                let res = res.and_then(|bytes| cache.stage(Variant::Thumb, job.id, &bytes));
                (job, res)
            })
            .collect();

        // Kurze Sperre: Veraltung für alle prüfen, die Temp-Dateien umbenennen
        // und alle Zustände in einer Transaktion setzen. Wurde das Bild während
        // des Renderns bearbeitet (Entwickeln speichert eigene Thumbnails, und
        // zwar prüfend und schreibend unter derselben Sperre), ist dieses
        // Ergebnis veraltet und darf das neue nicht überschreiben – es bleibt
        // ausstehend und wird mit der neuen Bearbeitung erzeugt. Das Umbenennen
        // gehört noch unter die Sperre: Danach käme Entwickeln dazwischen und
        // würde von uns überschrieben. Es ist billig, weil die Bytes schon
        // geschrieben sind.
        let ids: Vec<i64> = staged.iter().map(|(j, _)| j.id).collect();
        let outcome = {
            let mut cat = catalog.locked();
            match cat.thumb_jobs(&ids) {
                Err(e) => Err(e),
                Ok(current) => {
                    let mut states = Vec::new();
                    for (job, res) in &staged {
                        // Nicht im Ergebnis: inzwischen gelöscht
                        let Some(cur) = current.get(&job.id) else {
                            continue;
                        };
                        if cur.edit != job.edit || cur.path != job.path {
                            continue;
                        }
                        let ok = match res {
                            Ok(tmp) => cache
                                .commit_staged(Variant::Thumb, job.id, tmp)
                                .map_err(|e| {
                                    log::warn!("Thumbnail für {} fehlgeschlagen: {e}", job.path)
                                })
                                .is_ok(),
                            Err(e) => {
                                log::warn!("Thumbnail für {} fehlgeschlagen: {e}", job.path);
                                false
                            }
                        };
                        states.push((job.id, ok));
                    }
                    cat.set_thumb_states(&states).map(|()| states)
                }
            }
        };

        // Aufräumen außerhalb der Sperre: übrig gebliebene Temp-Dateien
        // (veraltet, gelöscht, Fehler) entfernen; nach dem Umbenennen existiert
        // der Pfad nicht mehr, das Löschen ist dann wirkungslos.
        for (_, res) in &staged {
            if let Ok(tmp) = res {
                ThumbCache::discard_staged(tmp);
            }
        }
        // Echte Datenbankfehler (nicht „gelöscht“) melden und weitergeben
        let done = outcome.inspect_err(|e| log::warn!("Thumbnail-Zustand nicht gesetzt: {e}"))?;
        for (id, ok) in done {
            // Alte Vorschau verwerfen, falls sich die Datei geändert hat.
            let _ = std::fs::remove_file(cache.path_for(Variant::Preview, id));
            on_done(id, ok);
        }
    }
    Ok((processed, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn sample_jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = RgbImage::from_fn(w, h, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
        encode_jpeg(&DynamicImage::ImageRgb8(img), 90).unwrap()
    }

    #[test]
    fn picks_smallest_sufficient_embedded_jpeg() {
        let small = sample_jpeg(400, 300);
        let big = sample_jpeg(1200, 800);
        let mut fake_raw = b"II*\0 fake raw header ".to_vec();
        fake_raw.extend(std::iter::repeat_n(0x42u8, 5000));
        fake_raw.extend(&small);
        fake_raw.extend(std::iter::repeat_n(0x17u8, 3000));
        fake_raw.extend(&big);
        fake_raw.extend(std::iter::repeat_n(0x00u8, 1000));

        let found = embedded_jpegs(&fake_raw);
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].len(), big.len());

        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.nef");
        std::fs::write(&p, &fake_raw).unwrap();
        let size = |min| {
            let img = decode_embedded_preview(&p, min).unwrap();
            (img.width(), img.height())
        };
        assert_eq!(size(300), (400, 300), "kleine reicht");
        assert_eq!(size(512), (1200, 800), "kleine zu klein");
        assert_eq!(size(5000), (1200, 800), "keine reicht: größte");
        assert_eq!(jpeg_size(&small), Some((400, 300)));
    }

    #[test]
    fn reads_file_head_first_and_falls_back_to_whole_file() {
        let small = sample_jpeg(400, 300);
        let big = sample_jpeg(1200, 800);
        let mut raw = b"II*\0".to_vec();
        raw.extend(&small);
        raw.resize(17 << 20, 0x11); // große Vorschau erst hinter den ersten 16 MB
        raw.extend(&big);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("gross.nef");
        std::fs::write(&p, &raw).unwrap();
        let size = |min| {
            let img = decode_embedded_preview(&p, min).unwrap();
            (img.width(), img.height())
        };
        assert_eq!(size(300), (400, 300), "aus dem Dateianfang");
        assert_eq!(size(1000), (1200, 800), "Rückfall: ganze Datei");
    }

    #[test]
    fn renders_with_orientation() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.jpg");
        std::fs::write(&p, sample_jpeg(1600, 900)).unwrap();
        let bytes = render(&p, ImageKind::Raster, Some(6), 512).unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        // 90° gedreht: hochkant
        assert_eq!((img.width(), img.height()), (288, 512));
    }

    /// Bisherige Suche Byte für Byte – Referenz für `embedded_jpegs`.
    fn embedded_jpegs_ref(data: &[u8]) -> Vec<&[u8]> {
        let mut out = Vec::new();
        let mut i = 0;
        while i + 3 < data.len() {
            if data[i] == 0xFF
                && data[i + 1] == 0xD8
                && data[i + 2] == 0xFF
                && let Some(len) = jpeg_len(&data[i..])
            {
                if len >= 8 * 1024 {
                    out.push(&data[i..i + len]);
                }
                i += len;
                continue;
            }
            i += 1;
        }
        out
    }

    #[test]
    fn fast_scan_finds_the_same_jpegs() {
        let jpeg = sample_jpeg(400, 300);
        let mut seed = 1u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 33) as u8
        };
        for case in 0..40 {
            let mut d: Vec<u8> = (0..case * 997).map(|_| rnd()).collect();
            // 0xFF-Häufungen, abgeschnittene Köpfe, JPEGs an Blockgrenzen und am Ende
            d.extend([0xFF, 0xFF, 0xD8, 0xFF, 0xFF, 0xD8]);
            d.extend(&jpeg);
            d.extend((0..case * 31).map(|_| 0xFF));
            d.extend(&jpeg[..jpeg.len() / 2]);
            d.extend(&jpeg);
            d.extend([0xFF, 0xD8, 0xFF][..case % 4].iter());
            for cut in [0, 1, 2, 3, 33] {
                let part = &d[..d.len().saturating_sub(cut)];
                let a: Vec<(usize, usize)> = embedded_jpegs(part)
                    .iter()
                    .map(|j| (j.as_ptr() as usize - part.as_ptr() as usize, j.len()))
                    .collect();
                let b: Vec<(usize, usize)> = embedded_jpegs_ref(part)
                    .iter()
                    .map(|j| (j.as_ptr() as usize - part.as_ptr() as usize, j.len()))
                    .collect();
                assert_eq!(a, b, "Fall {case}, gekürzt um {cut}");
            }
        }
        assert_eq!(find_ff(&[]), None);
        assert_eq!(find_ff(&[0; 64]), None);
        let mut v = vec![0u8; 70];
        v[69] = 0xFF;
        assert_eq!(find_ff(&v), Some(69));
        v[32] = 0xFF;
        assert_eq!(find_ff(&v), Some(32));
    }

    /// Unbearbeitetes JPEG ohne Drehung: Originalbytes; mit Drehung dekodiert.
    #[test]
    fn full_size_serves_plain_jpeg_unchanged() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.jpg");
        let original = sample_jpeg(640, 480);
        std::fs::write(&p, &original).unwrap();
        let job = |orientation, edit| ThumbJob {
            id: 1,
            path: p.to_string_lossy().into_owned(),
            kind: ImageKind::Raster,
            orientation,
            edit,
            long_edge: Some(640),
        };
        let full = |j: &ThumbJob| render_full(j, &NoMasks, None, &NoRepairs).unwrap();
        assert_eq!(full(&job(None, None)), original);
        assert_eq!(full(&job(Some(1), None)), original);
        let neutral = serde_json::to_value(DevelopParams::default()).unwrap();
        assert_eq!(
            full(&job(None, Some(neutral))),
            original,
            "neutrale Bearbeitung"
        );
        let rotated = image::load_from_memory(&full(&job(Some(6), None))).unwrap();
        assert_eq!((rotated.width(), rotated.height()), (480, 640));
        // PNG wird weiterhin dekodiert und als JPEG geliefert
        let png = d.path().join("b.png");
        image::RgbImage::new(64, 32).save(&png).unwrap();
        let j = ThumbJob {
            path: png.to_string_lossy().into_owned(),
            ..job(None, None)
        };
        assert!(full(&j).starts_with(&[0xFF, 0xD8, 0xFF]));
    }
}
