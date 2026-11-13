//! KI-Masken: Motiv, Himmel, Person, Tier und Objekt per Klick.

use super::*;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaskResult {
    pub(crate) mask_id: String,
    /// Anteil der Bildfläche, 0…1 – 0 heißt „nichts gefunden“
    pub(crate) coverage: f32,
}

/// Quellbild für die Segmentierung: unbearbeitet entwickelt, Originalausrichtung
/// (Masken beziehen sich auf das aufrechte Original, nicht auf Drehung/Zuschnitt).
pub(super) fn segmentation_input(state: &AppState, id: i64) -> Result<image::DynamicImage, Error> {
    segmentation_input_at(state, id, segment::MASK_EDGE)
}

/// Wie [`segmentation_input`], mit eigener längster Kante.
pub(super) fn segmentation_input_at(
    state: &AppState,
    id: i64,
    edge: u32,
) -> Result<image::DynamicImage, Error> {
    let src = crate::develop::load_proxy(state, id, edge as usize)?;
    // Masken entstehen auf dem Bild, wie es angezeigt wird – mit oder ohne
    // Objektivkorrektur
    let p = DevelopParams {
        lens_correction: crate::develop::lens_setting(state, id),
        ..Default::default()
    };
    let img = lumenkorn_develop::render(&src, &p);
    Ok(image::DynamicImage::ImageRgb8(
        lumenkorn_develop::output::to_rgb8(&img),
    ))
}

pub(super) fn masks_needed(state: &AppState) -> Result<(), Error> {
    state.usage.touch(Slot::Interactive);
    if !models::status(&state.data_dir).masks_ready {
        return Err(Error::Invalid(
            tr_s(
                "Die KI-Masken sind noch nicht installiert. Lade sie über „KI“ unten links.",
                "The AI masks are not installed yet. Download them via “AI” at the bottom left.",
            )
            .into(),
        ));
    }
    Ok(())
}

pub(super) fn subject_model(state: &AppState) -> Result<Arc<SubjectModel>, Error> {
    masks_needed(state)?;
    lazy_model(
        state,
        &state.ai.subject,
        tr_s("IS-Net (Motiv)", "IS-Net (subject)"),
        || {
            SubjectModel::load(&models::masks_dir(&state.data_dir).join("subject.onnx"))
                .map_err(err)
        },
    )
}

pub(super) fn sam_model(state: &AppState) -> Result<Arc<Sam>, Error> {
    masks_needed(state)?;
    lazy_model(
        state,
        &state.ai.sam,
        tr_s("Segment Anything (Objekt)", "Segment Anything (object)"),
        || {
            let dir = models::masks_dir(&state.data_dir);
            Sam::load(&dir.join("sam-encoder.onnx"), &dir.join("sam-decoder.onnx")).map_err(err)
        },
    )
}

/// (Bild, Objektivkorrektur): Die Eingabe des Embeddings hängt von beidem ab
pub(super) type SamKey = (i64, bool);

/// Zwischenspeicher der Funde für dieses Bild entnehmen (leer, wenn er zu einem
/// anderen Bild bzw. einer anderen Objektivkorrektur gehört).
pub(super) fn take_detect_cache(state: &AppState, id: i64) -> (SamKey, parts::DetectCache) {
    let key = (id, crate::develop::lens_setting(state, id));
    match state.ai.detect_cache.locked().take() {
        Some((k, c)) if k == key => (key, c),
        _ => (key, parts::DetectCache::default()),
    }
}

pub(super) fn sam_embedding(
    state: &AppState,
    sam: &Sam,
    id: i64,
) -> Result<Arc<SamEmbedding>, Error> {
    let key = (id, crate::develop::lens_setting(state, id));
    if let Some((cached, e)) = state.ai.sam_embedding.locked().as_ref()
        && *cached == key
    {
        return Ok(e.clone());
    }
    let t = std::time::Instant::now();
    // Erst das Foto laden (RAW-Dekodierung): Bricht das ab, ist es kein KI-Absturz
    let input = segmentation_input(state, id)?;
    let _g = analysis_guard(state, "Segment Anything");
    let e = Arc::new(sam.embed(&input).map_err(err)?);
    log::info!("SAM-Embedding für Bild {id} in {:.2?}", t.elapsed());
    *state.ai.sam_embedding.locked() = Some((key, e.clone()));
    Ok(e)
}

/// Legt eine Maske unter einer **neuen** ID ab. Bestehende Dateien werden nie
/// überschrieben: „Rückgängig“ und Schnappschüsse verweisen weiter auf die
/// alte ID und müssen dieselbe Maske wiederfinden. Unbenutzte Dateien räumt
/// der Hausmeister (`memory::sweep_data`) nach 24 Stunden weg.
pub(super) fn store(
    state: &AppState,
    image_id: i64,
    kind: &str,
    mask: &image::GrayImage,
) -> Result<MaskResult, Error> {
    let mask_id = MaskFiles::new_id(image_id, kind);
    state.masks.put(&mask_id, mask)?;
    let coverage = mask.pixels().filter(|p| p[0] > 127).count() as f32
        / (mask.width() * mask.height()).max(1) as f32;
    Ok(MaskResult { mask_id, coverage })
}

/// „Motiv auswählen“
#[tauri::command]
pub async fn mask_subject(app: AppHandle, id: i64) -> Result<MaskResult, Error> {
    blocking(app, move |state| {
        let model = subject_model(state)?;
        let t = std::time::Instant::now();
        let input = segmentation_input(state, id)?;
        let _g = analysis_guard(state, tr_s("Motiv", "Subject"));
        let mask = model.predict(&input).map_err(err)?;
        log::info!("Motivmaske für Bild {id} in {:.2?}", t.elapsed());
        store(state, id, "subject", &mask)
    })
    .await
}

/// „Himmel auswählen“. Mit dem Himmelsmodell (skyseg) entscheidet die KI;
/// fehlt es, greift die Farb- und Flächenheuristik, verfeinert durch SAM,
/// wenn installiert.
#[tauri::command]
pub async fn mask_sky(app: AppHandle, id: i64) -> Result<MaskResult, Error> {
    blocking(app, move |state| {
        state.usage.touch(Slot::Interactive);
        let input = segmentation_input(state, id)?;
        let dir = models::masks_dir(&state.data_dir);
        let status = models::status(&state.data_dir);
        let t = std::time::Instant::now();
        // Ohne KI bleibt die Erkennung aus Farbe und Helligkeit
        let ai = enabled(state);
        let mask = if ai && status.sky_ready {
            let sky = lazy_model(state, &state.ai.sky, "skyseg (Himmel)", || {
                SkyModel::load(&dir.join(models::SKY_MODEL)).map_err(err)
            })?;
            let _g = analysis_guard(state, tr_s("Himmel", "Sky"));
            segment::sky_mask(&input, Some(&sky), None).map_err(err)?
        } else if ai && status.masks_ready {
            let s = sam_model(state)?;
            let emb = sam_embedding(state, &s, id)?;
            segment::sky_mask(&input, None, Some((&s, &emb))).map_err(err)?
        } else {
            segment::sky_mask(&input, None, None).map_err(err)?
        };
        log::info!("Himmelsmaske für Bild {id} in {:.2?}", t.elapsed());
        store(state, id, "sky", &mask)
    })
    .await
}

/// Modelle für Personen und Tiere (mit den KI-Masken nachgeladen).
pub(super) fn parts_needed(state: &AppState) -> Result<(), Error> {
    masks_needed(state)?;
    if !models::status(&state.data_dir).parts_ready {
        return Err(Error::Invalid(parts::missing_hint().into()));
    }
    Ok(())
}

/// Face Mesh für die Augen-Prüfung des Aussortier-Assistenten; `None`, wenn
/// die KI-Masken für Personen nicht eingerichtet sind.
pub(crate) fn face_mesh(state: &AppState) -> Option<Arc<parts::FaceMesh>> {
    masks_needed(state).ok()?;
    let dir = models::masks_dir(&state.data_dir);
    // Nur diese Datei: Fehlt nach einem Update das neue OWLv2, läuft die Prüfung weiter
    if !dir.join(parts::MESH_FILE).is_file() {
        return None;
    }
    lazy_model(state, &state.ai.mesh, "Face Mesh", || {
        parts::FaceMesh::load(&dir.join(parts::MESH_FILE)).map_err(err)
    })
    .map_err(|e| log::warn!("Face Mesh nicht nutzbar: {e}"))
    .ok()
}

pub(super) fn owl_model(state: &AppState) -> Result<Arc<parts::Owl>, Error> {
    parts_needed(state)?;
    lazy_model(state, &state.ai.owl, "OWLv2", || {
        parts::Owl::load(&models::masks_dir(&state.data_dir)).map_err(err)
    })
}

/// „Person“: die ganze Person oder ein Teil davon, bei allen Personen im Foto
#[tauri::command]
pub async fn mask_person(
    app: AppHandle,
    id: i64,
    part: parts::PersonPart,
) -> Result<MaskResult, Error> {
    blocking(app, move |state| {
        let owl = owl_model(state)?;
        let dir = models::masks_dir(&state.data_dir);
        let seg = lazy_model(state, &state.ai.parts_seg, "Selfie Multiclass", || {
            parts::PartsSeg::load(&dir.join(parts::SEG_FILE)).map_err(err)
        })?;
        let mesh = lazy_model(state, &state.ai.mesh, "Face Mesh", || {
            parts::FaceMesh::load(&dir.join(parts::MESH_FILE)).map_err(err)
        })?;
        // SAM nur für die ganze Person (Umriss)
        let sam = if part == parts::PersonPart::Person {
            let s = sam_model(state)?;
            let e = sam_embedding(state, &s, id)?;
            Some((s, e))
        } else {
            None
        };
        let input = segmentation_input_at(state, id, parts::PARTS_EDGE)?;
        let t = std::time::Instant::now();
        let _g = analysis_guard(state, tr_s("Personen", "People"));
        let m = parts::PartModels {
            owl: &owl,
            seg: &seg,
            mesh: &mesh,
            sam: sam.as_ref().map(|(s, e)| (s.as_ref(), e.as_ref())),
        };
        let (key, mut cache) = take_detect_cache(state, id);
        let mask = parts::person_mask(&input, part, &m, &mut cache).map_err(err)?;
        *state.ai.detect_cache.locked() = Some((key, cache));
        log::info!(
            "Personenmaske ({part:?}) für Bild {id} in {:.2?}",
            t.elapsed()
        );
        store(state, id, "person", &mask)
    })
    .await
}

/// „Tier“: alle Tiere einer Art (Vögel, Säugetiere, Reptilien oder alle)
#[tauri::command]
pub async fn mask_animal(
    app: AppHandle,
    id: i64,
    kind: parts::AnimalKind,
) -> Result<MaskResult, Error> {
    blocking(app, move |state| {
        let owl = owl_model(state)?;
        let sam = sam_model(state)?;
        let emb = sam_embedding(state, &sam, id)?;
        let input = segmentation_input(state, id)?;
        let t = std::time::Instant::now();
        let _g = analysis_guard(state, tr_s("Tiere", "Animals"));
        let (key, mut cache) = take_detect_cache(state, id);
        let mask = parts::animal_mask(&input, kind, &owl, (&sam, &emb), &mut cache).map_err(err)?;
        *state.ai.detect_cache.locked() = Some((key, cache));
        log::info!("Tiermaske ({kind:?}) für Bild {id} in {:.2?}", t.elapsed());
        store(state, id, "animal", &mask)
    })
    .await
}

/// Objekt per Klick. Mit `mask_id` (die bisherige Maske) wird die Auswahl
/// mit den neuen Punkten verfeinert; das Ergebnis bekommt immer eine neue ID,
/// die alte Datei bleibt für „Rückgängig“ erhalten.
#[tauri::command]
pub async fn mask_object(
    app: AppHandle,
    id: i64,
    points: Vec<PromptPoint>,
    mask_id: Option<String>,
) -> Result<MaskResult, Error> {
    blocking(app, move |state| {
        let sam = sam_model(state)?;
        let emb = sam_embedding(state, &sam, id)?;
        let pts: Vec<segment::Point> = points
            .iter()
            .map(|p| segment::Point {
                x: p.x,
                y: p.y,
                positive: p.positive,
            })
            .collect();
        let mask = {
            let _g = analysis_guard(state, "Segment Anything");
            sam.predict(&emb, &pts).map_err(err)?
        };
        // Die alte Datei bleibt für „Rückgängig“: Frist des Aufräumens neu starten
        if let Some(old) = mask_id
            .filter(|i| i.starts_with(&format!("{id}-")))
            .and_then(|i| state.masks.path(&i))
            && let Ok(f) = std::fs::OpenOptions::new().write(true).open(&old)
        {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
        store(state, id, "object", &mask)
    })
    .await
}
