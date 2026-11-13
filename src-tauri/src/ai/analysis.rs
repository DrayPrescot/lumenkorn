//! Hintergrundanalyse: Schlagworte, Tierarten (CLIP, BioCLIP) und Vektoren für
//! die Suche, in Stapeln über den Worker.

use super::*;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct AiProgress {
    done: i64,
    total: i64,
    ids: Vec<i64>,
}

/// Schlagworte zusammenführen: gleiche Namen (z. B. „Vogel“ als Motiv und als
/// Tierklasse) einmal, mit der höheren Sicherheit.
pub(super) fn merge_tags(found: Vec<TagSuggestion>) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::with_capacity(found.len());
    for t in found {
        let c = t.confidence as f64;
        match out.iter_mut().find(|(n, _)| *n == t.tag) {
            Some(e) => e.1 = e.1.max(c),
            None => out.push((t.tag, c)),
        }
    }
    out
}

/// Tierarten-Erkennung (Text-Embeddings der Taxonomie), einmal berechnet.
/// `None`, wenn das nicht gelingt – dann gibt es eben keine Tierarten.
pub(super) fn animal_tagger(state: &AppState, loaded: &Loaded) -> Option<Arc<AnimalTagger>> {
    let mut slot = state.ai.animals.locked();
    if slot.is_none() {
        let t = std::time::Instant::now();
        match AnimalTagger::new(&loaded.clip) {
            Ok(a) => {
                log::info!("Tierarten vorbereitet in {:.2?}", t.elapsed());
                *slot = Some(Arc::new(a));
            }
            Err(e) => log::warn!("Tierarten nicht verfügbar: {e}"),
        }
    }
    slot.clone()
}

/// BioCLIP 2 mit seiner Tierarten-Liste
pub(crate) struct BioAnimals {
    model: Bioclip,
    taxa: BioTaxonomy,
}

/// BioCLIP 2 für genauere Tierarten, wenn installiert (sonst `None`: dann
/// bestimmt CLIP die Arten).
pub(super) fn bioclip_model(state: &AppState) -> Option<Arc<BioAnimals>> {
    if !models::status(&state.data_dir).animals_ready {
        return None;
    }
    let dir = models::animals_dir(&state.data_dir);
    lazy_model(state, &state.ai.bioclip, "BioCLIP 2", || {
        Ok(BioAnimals {
            taxa: BioTaxonomy::load(&dir.join(lumenkorn_ai::animals::BIOCLIP_TAXA_FILE))
                .map_err(err)?,
            model: Bioclip::load(&dir.join(lumenkorn_ai::animals::BIOCLIP_FILE)).map_err(err)?,
        })
    })
    .map_err(|e| log::warn!("BioCLIP 2 nicht nutzbar: {e}"))
    .ok()
}

/// BioCLIP-Embeddings für die Bilder mit Tier (laut CLIP); sonst `None`.
pub(super) fn bioclip_embeddings(
    state: &AppState,
    bio: Option<&BioAnimals>,
    animals: &AnimalTagger,
    images: &[image::DynamicImage],
    clip: &[Vec<f32>],
) -> Vec<Option<Vec<f32>>> {
    let mut out = vec![None; images.len()];
    let Some(bio) = bio else { return out };
    let which: Vec<usize> = (0..images.len())
        .filter(|&i| animals.has_animal(&clip[i]))
        .collect();
    if which.is_empty() {
        return out;
    }
    let picked: Vec<image::DynamicImage> = which.iter().map(|&i| images[i].clone()).collect();
    let _g = Guard::begin(
        &state.ai.guard_dir,
        Stage::KiAnalyse,
        Some(model_backend(bio.model.on_gpu())),
        "BioCLIP 2",
    );
    match bio.model.embed(&picked) {
        Ok(emb) => {
            for (i, e) in which.into_iter().zip(emb) {
                out[i] = Some(e);
            }
        }
        Err(e) => log::warn!("BioCLIP 2: {e}"),
    }
    out
}

/// Namen der großen Tierarten-Liste für die Suche bereitstellen (einmalig,
/// ohne das Modell zu laden)
pub(crate) fn load_search_names(state: &AppState) {
    if lumenkorn_ai::animals::search_names_loaded()
        || !models::status(&state.data_dir).animals_ready
    {
        return;
    }
    let path = models::animals_dir(&state.data_dir).join(lumenkorn_ai::animals::BIOCLIP_TAXA_FILE);
    if let Err(e) = BioTaxonomy::load(&path) {
        log::warn!("Tierarten-Liste nicht lesbar: {e}");
    }
}

/// Schlagworte dieser Fotos sind neu (Anzeige auffrischen)
pub(super) fn tags_changed(app: &AppHandle, items: &[(i64, Vec<(String, f64)>)]) {
    let ids: Vec<i64> = items.iter().map(|i| i.0).collect();
    let _ = app.emit("tags-changed", serde_json::json!({ "ids": ids }));
}

/// Ziel des Nachtragens (siehe [`ANIMAL_TAGS_CLIP`]), `None`, wenn erreicht
pub(super) fn animal_target(state: &AppState, bio_ready: bool) -> Option<u32> {
    let path = models::ai_dir(&state.data_dir).join(ANIMAL_TAGS_FILE);
    let done: u32 = lumenkorn_develop::fsutil::load_json_or_default(&path);
    let target = if bio_ready {
        ANIMAL_TAGS_BIOCLIP
    } else {
        ANIMAL_TAGS_CLIP
    };
    (done < target).then_some(target)
}

/// Stand der Tiererkennung mit BioCLIP (durchgesehene Fotos, alle
/// analysierten; nach dem Nachbestimmen alle); `None` ohne BioCLIP.
pub(super) fn animal_progress(state: &AppState) -> Option<Progress> {
    if !models::status(&state.data_dir).animals_ready {
        return None;
    }
    let after = match animal_target(state, true) {
        None => i64::MAX,
        Some(target) => {
            let g = state.ai.animal_backfill.locked();
            if g.0 == target { g.1 } else { 0 }
        }
    };
    let (done, total) = state
        .catalog
        .locked()
        .embeddings_progress(MODEL_KEY, after)
        .ok()?;
    Some(Progress { done, total })
}

#[derive(Serialize, Clone, Copy)]
pub struct Progress {
    done: i64,
    total: i64,
}

/// Tierarten für schon analysierte Fotos nachtragen bzw. nachbestimmen:
/// mit CLIP aus den gespeicherten Embeddings (ergänzt nur), mit BioCLIP 2
/// für die Fotos mit Tier aus ihren Thumbnails (ersetzt nur die
/// Tier-Schlagworte). `true`: es gibt noch mehr.
pub(super) fn animal_backfill(app: &AppHandle, state: &AppState) -> Result<bool, Error> {
    let path = models::ai_dir(&state.data_dir).join(ANIMAL_TAGS_FILE);
    let bio_ready = models::status(&state.data_dir).animals_ready;
    let Some(target) = animal_target(state, bio_ready) else {
        return Ok(false);
    };
    let after = {
        let mut g = state.ai.animal_backfill.locked();
        if g.0 != target {
            // Neues Ziel (BioCLIP kam dazu): von vorn
            *g = (target, 0);
        }
        g.1
    };
    let rows = state
        .catalog
        .locked()
        .embeddings_after(MODEL_KEY, after, ANIMAL_BACKFILL_BATCH)?;
    let Some(&(last_row, _)) = rows.last() else {
        // Fertig: Stand merken
        if let Err(e) = lumenkorn_develop::fsutil::save_json(&path, &target) {
            log::warn!("{ANIMAL_TAGS_FILE} nicht gespeichert: {e}");
        }
        let _ = std::fs::remove_file(models::ai_dir(&state.data_dir).join(ANIMAL_BACKFILL_FILE));
        log::info!(
            "Tierarten für bereits analysierte Fotos {}",
            if bio_ready {
                "mit BioCLIP 2 nachbestimmt"
            } else {
                "nachgetragen"
            }
        );
        return Ok(false);
    };
    let loaded = ensure_loaded(state)?;
    let Some(animals) = animal_tagger(state, &loaded) else {
        return Ok(false);
    };
    let bio = if bio_ready {
        bioclip_model(state)
    } else {
        None
    };
    let Some(bio) = bio else {
        // Nur CLIP: aus den gespeicherten Embeddings ergänzen
        let items: Vec<(i64, Vec<(String, f64)>)> = rows
            .iter()
            .filter_map(|(id, bytes)| {
                let tags = merge_tags(animals.tag(&from_bytes(bytes), None));
                (!tags.is_empty()).then_some((*id, tags))
            })
            .collect();
        if !items.is_empty() {
            state.catalog.locked().add_ai_tags(&items)?;
            tags_changed(app, &items);
        }
        set_backfill_pos(state, target, last_row);
        return Ok(true);
    };
    // BioCLIP: nur Fotos mit Tier, höchstens BIOCLIP_BACKFILL_BATCH je Durchgang
    let mut last = after;
    let mut picked: Vec<(i64, Vec<f32>)> = Vec::new();
    for (id, bytes) in &rows {
        last = *id;
        let e = from_bytes(bytes);
        if animals.has_animal(&e) {
            picked.push((*id, e));
            if picked.len() >= BIOCLIP_BACKFILL_BATCH {
                break;
            }
        }
    }
    let ids: Vec<i64> = picked.iter().map(|p| p.0).collect();
    let jobs = state.catalog.locked().thumb_jobs(&ids)?;
    let mut images = Vec::new();
    let mut clips = Vec::new();
    let mut ok = Vec::new();
    for (id, e) in picked {
        let img = jobs
            .get(&id)
            .and_then(|j| {
                state
                    .cache
                    .get_or_render(&state.catalog, Variant::Thumb, j)
                    .ok()
            })
            .and_then(|b| image::load_from_memory(&b).ok());
        if let Some(img) = img {
            images.push(img);
            clips.push(e);
            ok.push(id);
        }
    }
    let bios = bioclip_embeddings(state, Some(&bio), &animals, &images, &clips);
    let mut names = lumenkorn_ai::animals::all_names();
    names.extend(bio.taxa.names());
    let items: Vec<(i64, Vec<(String, f64)>)> = ok
        .iter()
        .zip(clips.iter().zip(&bios))
        .map(|(id, (e, b))| {
            // Auch die Motiv-Schlagworte mit Tiernamen („Vogel“, „Hund“) neu,
            // denn sie werden mit ersetzt
            let mut found: Vec<TagSuggestion> = loaded
                .tagger
                .tag(e)
                .into_iter()
                .filter(|t| names.contains(&t.tag.as_str()))
                .collect();
            found.extend(animals.tag(e, b.as_deref().map(|b| (b, &bio.taxa))));
            (*id, merge_tags(found))
        })
        .collect();
    if !items.is_empty() {
        state
            .catalog
            .locked()
            .replace_ai_tags_named(&items, &names)?;
        tags_changed(app, &items);
    }
    set_backfill_pos(state, target, last);
    Ok(true)
}

/// Nachtragen bis `pos` erledigt – auch auf der Platte, für den nächsten Start
pub(super) fn set_backfill_pos(state: &AppState, target: u32, pos: i64) {
    let mut g = state.ai.animal_backfill.locked();
    *g = (target, pos);
    let path = models::ai_dir(&state.data_dir).join(ANIMAL_BACKFILL_FILE);
    if let Err(e) = lumenkorn_develop::fsutil::save_json(&path, &*g) {
        log::warn!("{ANIMAL_BACKFILL_FILE} nicht gespeichert: {e}");
    }
}

/// Analysiert einen Stapel Bilder. `Ok(true)`, wenn noch weitere ausstehen.
pub fn run_batch(app: &AppHandle) -> Result<bool, Error> {
    let state = app.state::<AppState>();
    let on = {
        let s = state.ai.settings.locked();
        s.enabled && s.auto_tag
    };
    if !on || state.tasks.download.is_running() {
        return Ok(false);
    }
    if !models::status(&state.data_dir).model_ready || state.ai.load_error.locked().is_some() {
        return Ok(false);
    }
    let ids = state.catalog.locked().pending_ai(BATCH)?;
    if ids.is_empty() {
        // Alles analysiert: ggf. Tierarten für ältere Analysen nachtragen
        let more = animal_backfill(app, &state)?;
        if models::status(&state.data_dir).animals_ready {
            // Stand für die Einstellungen
            let _ = app.emit("animals-progress", animal_progress(&state));
        }
        return Ok(more);
    }
    let loaded = ensure_loaded(&state)?;
    let animals = animal_tagger(&state, &loaded);
    let bio = bioclip_model(&state);

    // Thumbnails (512 px) reichen für CLIP (224 px) völlig.
    let mut images = Vec::new();
    let mut ok_ids = Vec::new();
    let jobs: Vec<_> = {
        let cat = state.catalog.locked();
        ids.iter().map(|id| cat.thumb_job(*id)).collect()
    };
    let decoded: Vec<Option<image::DynamicImage>> = {
        use rayon::prelude::*;
        jobs.into_par_iter()
            .map(|job| {
                job.and_then(|j| {
                    state
                        .cache
                        .get_or_render(&state.catalog, Variant::Thumb, &j)
                })
                .ok()
                .and_then(|bytes| image::load_from_memory(&bytes).ok())
            })
            .collect()
    };
    for (id, img) in ids.iter().zip(decoded) {
        match img {
            Some(i) => {
                images.push(i);
                ok_ids.push(*id);
            }
            None => {
                if let Err(e) = state.catalog.locked().set_ai_failed(*id) {
                    log::warn!("Foto {id}: KI-Fehlschlag nicht vermerkt: {e}");
                }
            }
        }
    }

    let embeddings = {
        let _g = Guard::begin(
            &state.ai.guard_dir,
            Stage::KiAnalyse,
            Some(model_backend(loaded.clip.on_gpu())),
            "CLIP",
        );
        match loaded.clip.embed_images(&images) {
            Ok(e) => e,
            // Stapel gescheitert: einzeln, damit ein Foto, das immer scheitert, nicht
            // jedes Mal den ganzen Stapel (und alle Fotos danach) aufhält
            Err(batch) => {
                log::warn!("KI-Stapel gescheitert ({batch}), einzeln weiter");
                let mut kept = (Vec::new(), Vec::new(), Vec::new());
                let mut failed = Vec::new();
                for (id, img) in ok_ids.iter().zip(images.drain(..)) {
                    match loaded.clip.embed_images(std::slice::from_ref(&img)) {
                        Ok(mut e) if e.len() == 1 => {
                            kept.0.push(*id);
                            kept.1.push(img);
                            kept.2.push(e.remove(0));
                        }
                        _ => failed.push(*id),
                    }
                }
                // Scheitern alle, liegt es nicht an den Fotos (Laufzeit, Grafikkarte):
                // dann keins als gescheitert vermerken
                if kept.0.is_empty() {
                    return Err(err(batch));
                }
                for id in failed {
                    if let Err(e) = state.catalog.locked().set_ai_failed(id) {
                        log::warn!("Foto {id}: KI-Fehlschlag nicht vermerkt: {e}");
                    }
                }
                ok_ids = kept.0;
                images = kept.1;
                kept.2
            }
        }
    };
    // BioCLIP nur für die Fotos mit Tier
    let bios = match &animals {
        Some(a) => bioclip_embeddings(&state, bio.as_deref(), a, &images, &embeddings),
        None => vec![None; embeddings.len()],
    };
    for ((id, emb), b) in ok_ids.iter().zip(&embeddings).zip(&bios) {
        let mut found = loaded.tagger.tag(emb);
        if let Some(a) = &animals {
            found.extend(a.tag(emb, b.as_deref().zip(bio.as_deref().map(|x| &x.taxa))));
        }
        let tags = merge_tags(found);
        state
            .catalog
            .locked()
            .save_ai_result(*id, MODEL_KEY, &to_bytes(emb), &tags)?;
        if let Some(idx) = state
            .ai
            .index
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            idx.upsert(*id, emb);
        }
    }

    let (done, total) = state.catalog.locked().ai_counts()?;
    let _ = app.emit(
        "ai-progress",
        AiProgress {
            done,
            total,
            ids: ok_ids,
        },
    );
    Ok(true)
}

#[tauri::command(async)]
pub fn ai_retag_all(state: State<AppState>) -> Result<(), Error> {
    state.catalog.locked().reset_ai()?;
    *state.ai.load_error.locked() = None;
    state.worker.submit(Job::Ai);
    Ok(())
}
