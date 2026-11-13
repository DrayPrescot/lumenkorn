//! Semantische Suche: Suchindex aus den CLIP-Vektoren im Katalog, Suche nach
//! Beschreibung, Vektoren und Themenwerte für Stil und Album.

use super::*;

/// Führt `f` mit dem Suchindex aus; baut ihn bei der ersten Suche auf.
///
/// Der Aufbau läuft unter der Schreibsperre des Index: Zwei gleichzeitige
/// Suchen bauen ihn so nur einmal (die zweite wartet und nimmt das Ergebnis),
/// und Änderungen während des Aufbaus gehen nicht verloren – `upsert` und
/// `forget_in_index` warten auf die Sperre und wirken danach auf den fertigen
/// Index (was sie vorher in den Katalog geschrieben haben, liest der Aufbau
/// ohnehin schon mit). Gelesen wird über eine eigene Leseverbindung, die
/// Katalogsperre nur kurz zum Öffnen gehalten. Reihenfolge der Sperren:
/// Index → Katalog; umgekehrt hält niemand den Katalog, während er den Index sperrt.
pub(super) fn with_index<T>(
    state: &AppState,
    f: impl FnOnce(&EmbeddingIndex) -> T,
) -> Result<T, Error> {
    *state.ai.index_used.locked() = std::time::Instant::now();
    {
        let guard = state
            .ai
            .index
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(idx) = guard.as_ref() {
            let r = f(idx);
            *state.ai.index_used.locked() = std::time::Instant::now();
            return Ok(r);
        }
    }
    let mut guard = state
        .ai
        .index
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Hat eine parallele Suche ihn inzwischen gebaut, nur noch benutzen
    let idx = match guard.take() {
        Some(idx) => idx,
        None => {
            // Rund 2 KB je Foto (bei 85 000 Fotos 170 MB): nicht unter der Katalogsperre
            let rows = crate::read_catalog(state, |c| c.embeddings(MODEL_KEY))?;
            let mut idx = EmbeddingIndex::default();
            for (id, bytes) in rows {
                idx.upsert(id, &from_bytes(&bytes));
            }
            idx
        }
    };
    let r = f(guard.insert(idx));
    *state.ai.index_used.locked() = std::time::Instant::now();
    Ok(r)
}

/// Gibt den Suchindex frei, wenn er seit `idle` nicht benutzt wurde (er wird
/// bei der nächsten Suche aus dem Katalog neu aufgebaut). Wartet nicht auf
/// eine laufende Suche oder einen laufenden Aufbau. Liefert, ob er frei wurde.
pub(crate) fn release_index(state: &AppState, idle: std::time::Duration) -> bool {
    if state.ai.index_used.locked().elapsed() < idle {
        return false;
    }
    match state.ai.index.try_write() {
        Ok(mut g) => g.take().is_some(),
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner().take().is_some(),
        Err(std::sync::TryLockError::WouldBlock) => false,
    }
}

/// Entfernt gelöschte Fotos aus dem Suchindex der semantischen Suche (`None`:
/// Index verwerfen, er wird bei der nächsten Suche neu aufgebaut). Sonst
/// belegten sie bis zum Neustart Speicher und Plätze unter den Treffern.
pub(crate) fn forget_in_index(state: &AppState, ids: Option<&[i64]>) {
    let mut index = state
        .ai
        .index
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match (ids, index.as_mut()) {
        (Some(ids), Some(idx)) => ids.iter().for_each(|id| idx.remove(*id)),
        (None, _) => *index = None,
        (Some(_), None) => {}
    }
}

/// Semantische Suche: Bilder nach Ähnlichkeit zur Beschreibung sortiert.
#[tauri::command]
pub async fn semantic_search(
    app: AppHandle,
    query: String,
    scope: Scope,
    min_rating: Option<i64>,
    exts: Option<Vec<String>>,
    edited: Option<bool>,
) -> Result<Vec<ImageSummary>, Error> {
    blocking(app, move |state| {
        let loaded = ensure_loaded(state)?;
        load_search_names(state);
        let english = vocab::translate_query(&query);
        let q = loaded
            .clip
            .embed_texts(&[english.as_str()])
            .map_err(err)?
            .remove(0);
        // Erst den Bereich (Ordner/Projekt/Person, Mindestbewertung, Dateityp) bestimmen
        // und nur darin suchen: Sonst füllen Treffer aus dem übrigen Katalog
        // die SEARCH_MAX_HITS Plätze und setzen `best`, und ein kleiner Ordner findet
        // nichts. Nebenbei fallen gelöschte oder fehlende Fotos heraus, die
        // noch im Index stehen.
        let mut sq = ImageQuery::new(scope);
        sq.min_rating = min_rating;
        sq.exts = exts;
        sq.edited = edited;
        let allowed: HashSet<i64> = state.catalog.locked().query_image_ids(&sq)?;
        // Kosinus-Werte von CLIP liegen eng beieinander; relativ zum besten
        // Treffer filtern ist robuster als eine feste Schwelle.
        let hits = with_index(state, |idx| {
            idx.search_in(&q, SEARCH_MAX_HITS, -1.0, Some(&allowed))
        })?;
        let best = hits.first().map(|h| h.1).unwrap_or(0.0);
        let ids: Vec<i64> = hits
            .iter()
            .take_while(|(_, s)| *s >= best - SEARCH_MARGIN)
            .map(|h| h.0)
            .collect();
        log::info!(
            "Semantische Suche „{query}“ → „{english}“: {} Treffer",
            ids.len()
        );
        state.catalog.locked().images_in_order(&ids, &sq)
    })
    .await
}

/// CLIP-Vektoren einzelner Fotos aus dem Suchindex (für „Auto nach deinem
/// Stil“, Stufe 4: ähnliche Motive). Fehlende fehlen einfach.
pub(crate) fn embeddings_for(
    state: &AppState,
    ids: &[i64],
) -> std::collections::HashMap<i64, Vec<f32>> {
    {
        let guard = state
            .ai
            .index
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(idx) = guard.as_ref() {
            return ids
                .iter()
                .filter_map(|id| Some((*id, idx.get(*id)?.to_vec())))
                .collect();
        }
    }
    // Index nicht geladen: Nur die gefragten Vektoren lesen, nicht alle (bis ~200 MB
    // bei 100 000 Fotos). Das gilt auch bei ausgeschalteter KI, die Vektoren liegen
    // schon im Katalog.
    let rows = state.catalog.locked().embeddings_of(MODEL_KEY, ids);
    rows.unwrap_or_default()
        .into_iter()
        .map(|(id, bytes)| (id, from_bytes(&bytes)))
        .collect()
}

/// CLIP-Werte für das Fotoalbum (siehe `album::select`).
pub(crate) struct ThemeScores {
    /// Kosinus zum Thema (Mittel der Themen-Beschreibungen)
    pub theme: std::collections::HashMap<i64, f32>,
    /// Höchster Kosinus zu einer der unerwünschten Beschreibungen
    /// (Bildschirmfoto, Dokument …)
    pub negative: std::collections::HashMap<i64, f32>,
    /// Bild-Embeddings (für Dubletten und Abwechslung)
    pub vectors: std::collections::HashMap<i64, Vec<f32>>,
}

/// Thema- und Ausschlusswerte für `ids` aus dem Suchindex. `None`, wenn die
/// KI nicht eingerichtet ist oder keins der Fotos schon analysiert wurde – das
/// Album wählt dann nur nach Qualität und Bewertung. Englische Beschreibungen
/// erwartet (CLIP).
pub(crate) fn theme_scores(
    state: &AppState,
    prompts: &[String],
    negatives: &[&str],
    ids: &[i64],
) -> Result<Option<ThemeScores>, Error> {
    // KI aus: nicht erst den Suchindex aufbauen, um dann am Modell zu scheitern
    if ids.is_empty() || !enabled(state) || !models::status(&state.data_dir).model_ready {
        return Ok(None);
    }
    let vectors: std::collections::HashMap<i64, Vec<f32>> = with_index(state, |idx| {
        ids.iter()
            .filter_map(|id| Some((*id, idx.get(*id)?.to_vec())))
            .collect()
    })?;
    if vectors.is_empty() {
        return Ok(None);
    }
    let loaded = ensure_loaded(state)?;
    let mut texts: Vec<&str> = prompts.iter().map(String::as_str).collect();
    texts.extend_from_slice(negatives);
    let emb = loaded.clip.embed_texts(&texts).map_err(err)?;
    let (pos, neg) = emb.split_at(prompts.len());
    let theme_vec = (!pos.is_empty()).then(|| lumenkorn_ai::clip::mean_normalized(pos));
    let mut theme = std::collections::HashMap::new();
    let mut negative = std::collections::HashMap::new();
    for (id, v) in &vectors {
        if let Some(t) = &theme_vec {
            theme.insert(*id, lumenkorn_ai::clip::dot(t, v));
        }
        let n = neg
            .iter()
            .map(|q| lumenkorn_ai::clip::dot(q, v))
            .fold(f32::MIN, f32::max);
        if !neg.is_empty() {
            negative.insert(*id, n);
        }
    }
    Ok(Some(ThemeScores {
        theme,
        negative,
        vectors,
    }))
}
