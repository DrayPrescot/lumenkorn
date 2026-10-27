//! Album-Export: jede Seite einmal rendern, als JPEG kodieren und dieselben
//! Bytes sowohl als „Seite 001.jpg“ ablegen als auch in ein druckfertiges PDF
//! (mit Beschnittzugabe) schreiben. Das PDF entsteht streamend, damit ein großes
//! Album nicht komplett im Speicher liegt.

use super::{AlbumCell, AlbumDoc};
use crate::AppState;
use image::{RgbImage, imageops};
use lumenkorn_core::formats::ImageKind;
use lumenkorn_core::{Error, LockExt};
use lumenkorn_develop::i18n::tr_s;
use lumenkorn_develop::{Source, output, tr};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager};

/// Ein Zellrand näher als so viel am Seitenrand gilt als randbündig (mm).
const EDGE_TOL_MM: f64 = 0.5;
const JPEG_QUALITY: u8 = 92;
const MAX_PAGE_PX: f64 = 20000.0;
/// Längste erlaubte Base64-Ebene je Seite (Zeichen).
const MAX_OVERLAY_B64: usize = 64 * 1024 * 1024;

static CANCEL: AtomicBool = AtomicBool::new(false);

// --- Geometrie ------------------------------------------------------------------

fn mm_to_px(mm: f64, dpi: f64) -> f64 {
    mm * dpi / 25.4
}

/// Pixelgröße der ganzen Seite samt Beschnittzugabe.
pub fn page_pixels(doc: &AlbumDoc) -> (u32, u32) {
    let f = |mm: f64| mm_to_px(mm + 2.0 * doc.bleed_mm, doc.dpi as f64).round() as u32;
    (f(doc.width_mm), f(doc.height_mm))
}

/// Zelle → Pixelrechteck (x, y, w, h) auf der Seite mit Beschnittzugabe.
/// Kanten am Seitenrand reichen bis an den Rand der Zugabe.
pub fn cell_rect(
    cell: &AlbumCell,
    page_mm: (f64, f64),
    bleed_mm: f64,
    dpi: f64,
    page_px: (u32, u32),
) -> Option<(u32, u32, u32, u32)> {
    let axis = |pos: f64, len: f64, page: f64, max_px: u32| -> Option<(u32, u32)> {
        let a = if pos < EDGE_TOL_MM { -bleed_mm } else { pos };
        let b = if pos + len > page - EDGE_TOL_MM {
            page + bleed_mm
        } else {
            pos + len
        };
        let pa = mm_to_px(a + bleed_mm, dpi)
            .round()
            .clamp(0.0, max_px as f64) as u32;
        let pb = mm_to_px(b + bleed_mm, dpi)
            .round()
            .clamp(0.0, max_px as f64) as u32;
        (pb > pa).then_some((pa, pb - pa))
    };
    let (x, w) = axis(cell.x, cell.w, page_mm.0, page_px.0)?;
    let (y, h) = axis(cell.y, cell.h, page_mm.1, page_px.1)?;
    Some((x, y, w, h))
}

/// Ausschnitt eines Fotos (pw × ph) für ein Feld (zw × zh) wie CSS
/// `object-fit: cover` mit `object-position`: (links, oben, Breite, Höhe).
pub fn cover_crop(pw: f64, ph: f64, zw: f64, zh: f64, ox: f64, oy: f64) -> (f64, f64, f64, f64) {
    let s = (zw / pw).max(zh / ph);
    let cw = (zw / s).min(pw);
    let ch = (zh / s).min(ph);
    (
        ox.clamp(0.0, 1.0) * (pw - cw),
        oy.clamp(0.0, 1.0) * (ph - ch),
        cw,
        ch,
    )
}

/// Foto auf Zellgröße bringen und in die Seite kopieren.
fn paint_cell(page: &mut RgbImage, photo: &RgbImage, cell: &AlbumCell, rect: (u32, u32, u32, u32)) {
    let (x, y, w, h) = rect;
    let (pw, ph) = photo.dimensions();
    let (l, t, cw, ch) = cover_crop(pw as f64, ph as f64, w as f64, h as f64, cell.ox, cell.oy);
    let l = (l.round() as u32).min(pw - 1);
    let t = (t.round() as u32).min(ph - 1);
    let cw = (cw.round() as u32).clamp(1, pw - l);
    let ch = (ch.round() as u32).clamp(1, ph - t);
    let part = imageops::crop_imm(photo, l, t, cw, ch).to_image();
    let scaled = imageops::resize(&part, w, h, imageops::FilterType::Lanczos3);
    imageops::replace(page, &scaled, x as i64, y as i64);
}

/// Ein Farbkanal: `src` mit Deckkraft `a` (0…255) über `dst`.
fn blend_channel(src: u8, dst: u8, a: u8) -> u8 {
    let a = a as u32;
    ((src as u32 * a + dst as u32 * (255 - a) + 127) / 255) as u8
}

/// Legt ein RGBA-Bild (gleiche Größe wie die Seite) mit Alpha über die Seite.
fn blend_overlay(page: &mut RgbImage, overlay: &image::RgbaImage) {
    for (dst, src) in page.pixels_mut().zip(overlay.pixels()) {
        let a = src.0[3];
        if a == 0 {
            continue;
        }
        for c in 0..3 {
            dst.0[c] = blend_channel(src.0[c], dst.0[c], a);
        }
    }
}

/// Base64-PNG der Ebene lesen; weicht die Größe von `size` ab, wird skaliert.
fn decode_overlay(b64: &str, size: (u32, u32)) -> Result<image::RgbaImage, String> {
    use base64::Engine;
    if b64.len() > MAX_OVERLAY_B64 {
        return Err(tr_s("Ebene zu groß", "Layer too large").into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| e.to_string())?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    if img.dimensions() == size {
        Ok(img)
    } else {
        Ok(imageops::resize(
            &img,
            size.0,
            size.1,
            imageops::FilterType::CatmullRom,
        ))
    }
}

// --- PDF ------------------------------------------------------------------------

/// Schreibt ein PDF 1.4 mit je einem JPEG pro Seite, Seite für Seite.
pub struct PdfWriter {
    out: BufWriter<std::fs::File>,
    pos: u64,
    /// Offset je Objektnummer (Index 0 = frei)
    offsets: Vec<u64>,
    pages: usize,
}

impl PdfWriter {
    pub fn new(path: &Path) -> std::io::Result<Self> {
        let mut w = Self {
            out: BufWriter::new(std::fs::File::create(path)?),
            pos: 0,
            offsets: vec![0; 4],
            pages: 0,
        };
        // Binärkommentar kennzeichnet die Datei als Binärdatei
        w.put(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n")?;
        Ok(w)
    }

    fn put(&mut self, b: &[u8]) -> std::io::Result<()> {
        self.out.write_all(b)?;
        self.pos += b.len() as u64;
        Ok(())
    }

    fn begin(&mut self, num: usize) -> std::io::Result<()> {
        if self.offsets.len() <= num {
            self.offsets.resize(num + 1, 0);
        }
        self.offsets[num] = self.pos;
        self.put(format!("{num} 0 obj\n").as_bytes())
    }

    /// Eine Seite; Maße in Punkten, `jpeg` ist ein fertig kodiertes JPEG.
    pub fn add_page(
        &mut self,
        jpeg: &[u8],
        px_w: u32,
        px_h: u32,
        page_w_pt: f64,
        page_h_pt: f64,
        bleed_pt: f64,
    ) -> std::io::Result<()> {
        let base = 4 + 3 * self.pages;
        let (page, content, image) = (base, base + 1, base + 2);
        let (tx1, ty1) = (page_w_pt - bleed_pt, page_h_pt - bleed_pt);
        self.begin(page)?;
        self.put(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {page_w_pt:.3} {page_h_pt:.3}] \
                 /BleedBox [0 0 {page_w_pt:.3} {page_h_pt:.3}] \
                 /TrimBox [{bleed_pt:.3} {bleed_pt:.3} {tx1:.3} {ty1:.3}] \
                 /Resources << /XObject << /Im0 {image} 0 R >> >> /Contents {content} 0 R >>\nendobj\n"
            )
            .as_bytes(),
        )?;
        let stream = format!("q {page_w_pt:.3} 0 0 {page_h_pt:.3} 0 0 cm /Im0 Do Q\n");
        self.begin(content)?;
        self.put(
            format!(
                "<< /Length {} >>\nstream\n{stream}endstream\nendobj\n",
                stream.len()
            )
            .as_bytes(),
        )?;
        self.begin(image)?;
        self.put(
            format!(
                "<< /Type /XObject /Subtype /Image /Width {px_w} /Height {px_h} \
                 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
                jpeg.len()
            )
            .as_bytes(),
        )?;
        self.put(jpeg)?;
        self.put(b"\nendstream\nendobj\n")?;
        self.pages += 1;
        Ok(())
    }

    /// Katalog, Seitenbaum, Info und Verweistabelle schreiben.
    pub fn finish(mut self) -> std::io::Result<()> {
        self.begin(1)?;
        self.put(b"<< /Type /Catalog /Pages 2 0 R >>\nendobj\n")?;
        let kids: String = (0..self.pages)
            .map(|i| format!("{} 0 R ", 4 + 3 * i))
            .collect();
        self.begin(2)?;
        self.put(
            format!(
                "<< /Type /Pages /Kids [{kids}] /Count {} >>\nendobj\n",
                self.pages
            )
            .as_bytes(),
        )?;
        self.begin(3)?;
        self.put(b"<< /Producer (Lumenkorn) >>\nendobj\n")?;
        let xref = self.pos;
        let n = self.offsets.len();
        let mut table = format!("xref\n0 {n}\n0000000000 65535 f \n");
        for off in &self.offsets[1..] {
            table.push_str(&format!("{off:010} 00000 n \n"));
        }
        table.push_str(&format!(
            "trailer\n<< /Size {n} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        ));
        self.put(table.as_bytes())?;
        self.out.flush()
    }
}

// --- Ausgabeordner --------------------------------------------------------------

/// `<Ordner>/<stamm>`, bei vorhandenem nicht leerem Ordner `stamm-2`, `-3` …
fn unique_dir(parent: &Path, stem: &str) -> PathBuf {
    let free = |p: &Path| {
        !p.exists()
            || p.read_dir()
                .map(|mut d| d.next().is_none())
                .unwrap_or(false)
    };
    let first = parent.join(stem);
    if free(&first) {
        return first;
    }
    (2..)
        .map(|n| parent.join(format!("{stem}-{n}")))
        .find(|p| free(p))
        .expect("unendlich")
}

// --- Foto entwickeln -----------------------------------------------------------

/// Foto entwickeln wie beim normalen Export, mit auf `need_edge` begrenzter
/// Quelle. `name` bekommt den Dateinamen (für Fehlermeldungen).
fn render_photo(
    state: &AppState,
    id: i64,
    need_edge: usize,
    name: &mut String,
) -> Result<RgbImage, Error> {
    let (job, params) = {
        let cat = state.catalog.locked();
        (cat.thumb_job(id)?, cat.develop_params(id)?)
    };
    *name = Path::new(&job.path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| id.to_string());
    // Zuschnitt und Geraderichten verkleinern bzw. vergrößern den Ausschnitt
    let crop = params.crop.map_or(1.0, |c| c.w.min(c.h)).max(0.05);
    let tilt = lumenkorn_develop::geometry::load_margin(&params);
    let load_edge = Some((need_edge as f32 / crop * tilt).ceil() as usize);
    let src = Source::load(
        Path::new(&job.path),
        job.kind == ImageKind::Raw,
        job.orientation.map(|o| o as u16),
        load_edge,
    )?;
    let denoised = params.denoise.as_ref().and_then(|d| {
        let p = lumenkorn_develop::denoised::path(&state.denoised_dir, &d.id)?;
        lumenkorn_develop::denoised::read(&p, Some(src.image.long_edge())).ok()
    });
    let img = lumenkorn_develop::render_auto(
        &src,
        &params,
        &crate::develop::render_opts(state, denoised.as_ref()),
    );
    Ok(output::to_rgb8(&img))
}

// --- Befehle --------------------------------------------------------------------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AlbumProgress {
    done: usize,
    total: usize,
    finished: bool,
    failed: Vec<String>,
    error: Option<String>,
    pdf: String,
    dir: String,
}

fn validate(doc: &AlbumDoc) -> Result<(), Error> {
    let bad = |s: &str| Err(Error::Invalid(s.to_string()));
    if doc.pages.is_empty() {
        return bad(tr_s(
            "Das Album hat keine Seiten.",
            "The album has no pages.",
        ));
    }
    let ok = |v: f64| v.is_finite() && v > 0.0;
    if !ok(doc.width_mm) || !ok(doc.height_mm) || !doc.bleed_mm.is_finite() || doc.bleed_mm < 0.0 {
        return bad(tr_s("Ungültiges Seitenformat.", "Invalid page size."));
    }
    if !(72..=600).contains(&doc.dpi) {
        return bad(tr_s(
            "Die Auflösung muss zwischen 72 und 600 dpi liegen.",
            "The resolution must be between 72 and 600 dpi.",
        ));
    }
    let (pw, ph) = page_pixels(doc);
    if pw as f64 > MAX_PAGE_PX || ph as f64 > MAX_PAGE_PX {
        return bad(tr_s(
            "Die Seite wäre in dieser Auflösung zu groß (max. 20000 Pixel je Kante).",
            "The page would be too large at this resolution (max. 20000 pixels per side).",
        ));
    }
    Ok(())
}

/// Exportiert das Album im Hintergrund als PDF (`path`) und JPG je Seite in
/// einen Ordner neben dem PDF. Fortschritt per `album-export-progress`.
#[tauri::command(async)]
pub fn album_export(app: AppHandle, doc: AlbumDoc, path: String) -> Result<(), Error> {
    validate(&doc)?;
    let pdf = PathBuf::from(&path);
    let (Some(parent), Some(stem)) = (
        pdf.parent().map(Path::to_path_buf),
        pdf.file_stem().map(|s| s.to_string_lossy().into_owned()),
    ) else {
        return Err(Error::Invalid(tr!(
            "Ungültiger Pfad: {path}",
            "Invalid path: {path}"
        )));
    };
    if !parent.is_dir() {
        return Err(Error::Invalid(tr!(
            "{} ist kein Ordner",
            "{} is not a folder",
            parent.display()
        )));
    }
    let Some(running) = crate::busy::claim_in(&app, |s| &s.develop.exporting) else {
        return Err(Error::Invalid(
            tr_s(
                "Es läuft bereits ein Export – bitte warten, bis er fertig ist.",
                "An export is already running – please wait until it has finished.",
            )
            .into(),
        ));
    };
    CANCEL.store(false, Ordering::SeqCst);
    let dir = unique_dir(&parent, &stem);
    std::thread::Builder::new()
        .name("lumenkorn-album".into())
        .spawn(move || {
            let _running = running;
            let total = doc.pages.len();
            let mut failed = Vec::new();
            let emit = |done: usize, finished: bool, failed: &[String], error: Option<String>| {
                let _ = app.emit(
                    "album-export-progress",
                    AlbumProgress {
                        done,
                        total,
                        finished,
                        failed: failed.to_vec(),
                        error,
                        pdf: pdf.display().to_string(),
                        dir: dir.display().to_string(),
                    },
                );
            };
            emit(0, false, &failed, None);
            let result = run_export(&app, &doc, &pdf, &dir, &mut failed, |done, failed| {
                emit(done, false, failed, None)
            });
            lumenkorn_develop::release_large_caches();
            match result {
                Ok(true) => emit(total, true, &failed, None),
                Ok(false) => emit(
                    0,
                    true,
                    &failed,
                    Some(tr_s("Abgebrochen", "Canceled").into()),
                ),
                Err(e) => {
                    log::warn!("Album-Export: {e}");
                    emit(0, true, &failed, Some(e.to_string()))
                }
            }
        })?;
    Ok(())
}

/// Bricht den laufenden Album-Export vor der nächsten Seite ab.
#[tauri::command]
pub fn album_export_cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

/// Rendert alle Seiten. `Ok(true)` = fertig, `Ok(false)` = abgebrochen.
fn run_export(
    app: &AppHandle,
    doc: &AlbumDoc,
    pdf: &Path,
    dir: &Path,
    failed: &mut Vec<String>,
    progress: impl Fn(usize, &[String]),
) -> Result<bool, Error> {
    std::fs::create_dir_all(dir)?;
    let part = PathBuf::from(format!("{}.part", pdf.display()));
    let mut writer = PdfWriter::new(&part)?;
    match write_pages(app, doc, dir, failed, &progress, &mut writer) {
        Ok(true) => {
            writer.finish()?;
            // Rename ersetzt auf Windows eine vorhandene Datei
            std::fs::rename(&part, pdf)?;
            Ok(true)
        }
        other => {
            drop(writer);
            let _ = std::fs::remove_file(&part);
            other
        }
    }
}

fn write_pages(
    app: &AppHandle,
    doc: &AlbumDoc,
    dir: &Path,
    failed: &mut Vec<String>,
    progress: &impl Fn(usize, &[String]),
    writer: &mut PdfWriter,
) -> Result<bool, Error> {
    let state = app.state::<AppState>();
    let dpi = doc.dpi as f64;
    let page_px = page_pixels(doc);
    let page_mm = (doc.width_mm, doc.height_mm);
    let pt = |mm: f64| mm * 72.0 / 25.4;
    let page_label = tr_s("Seite", "Page");
    for (n, page) in doc.pages.iter().enumerate() {
        if CANCEL.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let mut img = RgbImage::from_pixel(page_px.0, page_px.1, image::Rgb(doc.background));
        let rects: Vec<_> = page
            .cells
            .iter()
            .map(|c| cell_rect(c, page_mm, doc.bleed_mm, dpi, page_px))
            .collect();
        // Dasselbe Foto mehrfach auf einer Seite nur einmal entwickeln
        let mut cache: HashMap<i64, Option<RgbImage>> = HashMap::new();
        for (cell, rect) in page.cells.iter().zip(&rects) {
            let Some(rect) = *rect else { continue };
            if let std::collections::hash_map::Entry::Vacant(slot) = cache.entry(cell.id) {
                // Obergrenze für unbekanntes Seitenverhältnis; bei mehreren
                // Zellen mit demselben Foto gilt die größte.
                let need = page
                    .cells
                    .iter()
                    .zip(&rects)
                    .filter(|(c, _)| c.id == cell.id)
                    .filter_map(|(_, r)| *r)
                    .map(|r| (r.2.max(r.3) as f64 * 1.6).ceil() as usize)
                    .max()
                    .unwrap_or(1);
                let mut name = cell.id.to_string();
                // Ein Panic bei einem Foto darf den Export nicht beenden.
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    render_photo(&state, cell.id, need, &mut name)
                }))
                .unwrap_or_else(|_| {
                    Err(Error::Invalid(
                        tr_s("Interner Fehler", "Internal error").into(),
                    ))
                });
                let photo = match res {
                    Ok(p) => Some(p),
                    Err(e) => {
                        log::warn!("Album {}: {e}", cell.id);
                        failed.push(format!("{name}: {e}"));
                        None
                    }
                };
                slot.insert(photo);
            }
            if let Some(Some(photo)) = cache.get(&cell.id) {
                paint_cell(&mut img, photo, cell, rect);
            }
        }
        drop(cache);
        if let Some(b64) = page.overlay.as_deref().filter(|o| !o.is_empty()) {
            match decode_overlay(b64, page_px) {
                Ok(ov) => blend_overlay(&mut img, &ov),
                Err(e) => {
                    log::warn!("Album Seite {}: Ebene: {e}", n + 1);
                    failed.push(format!(
                        "{page_label} {}: {}: {e}",
                        n + 1,
                        tr_s("Text/Karte", "Text/map")
                    ));
                }
            }
        }
        let jpeg = output::encode_jpeg_rgb8(&img, JPEG_QUALITY)?;
        drop(img);
        std::fs::write(dir.join(format!("{page_label} {:03}.jpg", n + 1)), &jpeg)?;
        writer.add_page(
            &jpeg,
            page_px.0,
            page_px.1,
            pt(doc.width_mm + 2.0 * doc.bleed_mm),
            pt(doc.height_mm + 2.0 * doc.bleed_mm),
            pt(doc.bleed_mm),
        )?;
        progress(n + 1, failed);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_b64(img: &image::RgbaImage) -> String {
        use base64::Engine;
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
    }

    #[test]
    fn alpha_blend() {
        let mut page = RgbImage::from_pixel(2, 1, image::Rgb([255, 255, 255]));
        let mut ov = image::RgbaImage::new(2, 1);
        ov.put_pixel(0, 0, image::Rgba([255, 0, 0, 128]));
        ov.put_pixel(1, 0, image::Rgba([0, 0, 0, 0]));
        blend_overlay(&mut page, &ov);
        let p = page.get_pixel(0, 0).0;
        assert_eq!(p[0], 255);
        assert!(
            (120..=135).contains(&p[1]) && (120..=135).contains(&p[2]),
            "{p:?}"
        );
        assert_eq!(page.get_pixel(1, 0).0, [255, 255, 255]);
        assert_eq!(blend_channel(10, 200, 255), 10);
        assert_eq!(blend_channel(10, 200, 0), 200);
    }

    #[test]
    fn overlay_is_scaled_to_page() {
        let ov = image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        let out = decode_overlay(&png_b64(&ov), (10, 6)).unwrap();
        assert_eq!(out.dimensions(), (10, 6));
        let same = decode_overlay(&png_b64(&ov), (4, 4)).unwrap();
        assert_eq!(same.dimensions(), (4, 4));
    }

    #[test]
    fn broken_overlay_is_an_error() {
        assert!(decode_overlay("!!!kein base64!!!", (4, 4)).is_err());
        assert!(decode_overlay("QUJDRA==", (4, 4)).is_err()); // gültig, aber kein PNG
        assert!(decode_overlay("", (4, 4)).is_err());
    }

    fn cell(x: f64, y: f64, w: f64, h: f64) -> AlbumCell {
        AlbumCell {
            id: 1,
            x,
            y,
            w,
            h,
            ox: 0.5,
            oy: 0.5,
        }
    }

    #[test]
    fn page_size_includes_bleed() {
        let doc = AlbumDoc {
            width_mm: 210.0,
            height_mm: 297.0,
            bleed_mm: 3.0,
            dpi: 300,
            background: [255; 3],
            pages: vec![],
        };
        // 216 mm · 300 / 25.4 = 2551,18 ; 303 mm → 3578,74
        assert_eq!(page_pixels(&doc), (2551, 3579));
    }

    #[test]
    fn edge_cells_extend_into_bleed() {
        // 100 × 100 mm, 3 mm Zugabe, 25,4 dpi → 1 px je mm
        let px = (106, 106);
        let rect = |c: AlbumCell| cell_rect(&c, (100.0, 100.0), 3.0, 25.4, px);
        // Ganze Seite: von 0 bis 106
        assert_eq!(rect(cell(0.0, 0.0, 100.0, 100.0)), Some((0, 0, 106, 106)));
        // Linke Hälfte: rechte Kante liegt innen (50 mm → 53 px)
        assert_eq!(rect(cell(0.0, 0.0, 50.0, 100.0)), Some((0, 0, 53, 106)));
        // Rechte Hälfte: beginnt bei 53 px, reicht bis 106
        assert_eq!(rect(cell(50.0, 0.0, 50.0, 100.0)), Some((53, 0, 53, 106)));
        // Innenzelle bleibt; 0,3 mm Abstand oben zählt als Rand
        assert_eq!(rect(cell(10.0, 0.3, 20.0, 20.0)), Some((13, 0, 20, 23)));
        // Ohne Fläche
        assert_eq!(rect(cell(10.0, 10.0, 0.0, 20.0)), None);
    }

    #[test]
    fn cover_matches_css() {
        // Quer-Foto 400×200 in quadratischem Feld: sichtbar 200×200
        let (l, t, w, h) = cover_crop(400.0, 200.0, 100.0, 100.0, 0.0, 0.5);
        assert_eq!((l, t, w, h), (0.0, 0.0, 200.0, 200.0));
        let (l, ..) = cover_crop(400.0, 200.0, 100.0, 100.0, 1.0, 0.5);
        assert_eq!(l, 200.0);
        let (l, ..) = cover_crop(400.0, 200.0, 100.0, 100.0, 0.5, 0.5);
        assert_eq!(l, 100.0);
        // Hochformat-Foto im Querfeld: Höhe wird beschnitten
        let (l, t, w, h) = cover_crop(200.0, 400.0, 100.0, 50.0, 0.5, 1.0);
        assert_eq!((l, w, h), (0.0, 200.0, 100.0));
        assert_eq!(t, 300.0);
    }

    #[test]
    fn unique_dir_skips_non_empty() {
        let base = std::env::temp_dir().join(format!("lk-album-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("a")).unwrap();
        assert_eq!(unique_dir(&base, "a"), base.join("a"));
        std::fs::write(base.join("a").join("x"), b"1").unwrap();
        assert_eq!(unique_dir(&base, "a"), base.join("a-2"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn pdf_structure_is_valid() {
        let dir = std::env::temp_dir().join(format!("lk-album-pdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.pdf");
        let img = RgbImage::from_pixel(8, 6, image::Rgb([200, 100, 50]));
        let jpeg = output::encode_jpeg_rgb8(&img, 90).unwrap();
        let mut w = PdfWriter::new(&path).unwrap();
        w.add_page(&jpeg, 8, 6, 226.0, 170.0, 8.5).unwrap();
        w.add_page(&jpeg, 8, 6, 226.0, 170.0, 8.5).unwrap();
        w.finish().unwrap();
        let b = std::fs::read(&path).unwrap();
        assert!(b.starts_with(b"%PDF-1.4"));
        assert!(b.ends_with(b"%%EOF\n"));
        let sx = b
            .windows(10)
            .rposition(|w| w == b"startxref\n")
            .expect("startxref");
        let tail = String::from_utf8_lossy(&b[sx + 10..]).to_string();
        let xref: usize = tail.lines().next().unwrap().parse().unwrap();
        assert!(b[xref..].starts_with(b"xref\n"));
        let text = String::from_utf8_lossy(&b[xref..]).to_string();
        let mut lines = text.lines().skip(1);
        let count: usize = lines
            .next()
            .unwrap()
            .split(' ')
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(count, 10); // 0..3 plus 2 Seiten à 3 Objekte
        assert_eq!(lines.next().unwrap(), "0000000000 65535 f ");
        for num in 1..count {
            let line = lines.next().unwrap();
            assert_eq!(line.len() + 1, 20);
            let off: usize = line[..10].parse().unwrap();
            assert!(
                b[off..].starts_with(format!("{num} 0 obj").as_bytes()),
                "Objekt {num}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
