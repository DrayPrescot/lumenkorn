//! KI-Reparatur in der App: Auswahl in voller Auflösung auswerten, mit LaMa
//! füllen, Flicken speichern.

use crate::AppState;
use crate::ai::{analysis_guard, err, lazy_model};
use lumenkorn_ai::inpaint::{self, Inpainter};
use lumenkorn_ai::models;
use lumenkorn_core::Error;
use lumenkorn_core::LockExt;
use lumenkorn_core::formats::ImageKind;
use lumenkorn_core::maskstore::MaskFiles;
use lumenkorn_develop::i18n::tr_s;
use lumenkorn_develop::{DenoiseRef, MaskLayer, MaskShape, Source};
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

#[derive(Default)]
pub struct RepairState {
    model: Mutex<Option<Arc<Inpainter>>>,
    cancel: AtomicBool,
    busy: AtomicBool,
}

/// Entlädt LaMa, wenn gerade keine Reparatur läuft.
pub(crate) fn release_model(state: &AppState) -> bool {
    !state.repair.busy.load(Ordering::SeqCst) && crate::ai::drop_if_unused(&state.repair.model)
}

fn inpainter(state: &AppState) -> Result<Arc<Inpainter>, Error> {
    state.usage.touch(crate::memory::Slot::Interactive);
    if !models::status(&state.data_dir).repair_ready {
        return Err(Error::Invalid(
            tr_s(
                "Die KI-Reparatur ist noch nicht installiert. Lade sie über „KI“ unten links.",
                "AI repair is not installed yet. Download it via “AI” at the bottom left.",
            )
            .into(),
        ));
    }
    lazy_model(
        state,
        &state.repair.model,
        tr_s("LaMa (Reparatur)", "LaMa (repair)"),
        || {
            Inpainter::load(&models::repair_dir(&state.data_dir).join(inpaint::MODEL_FILE))
                .map_err(err)
        },
    )
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RepairProgress {
    image_id: i64,
    done: usize,
    total: usize,
}

/// Berechnet eine Reparatur in voller Auflösung und liefert die ID des Flickens.
/// `denoise`: aktuelle Rauschreduzierung – der Flicken wird auf dem
/// entrauschten Bild berechnet, damit er zur Umgebung passt.
#[tauri::command]
pub async fn repair_compute(
    app: AppHandle,
    image_id: i64,
    shapes: Vec<MaskShape>,
    denoise: Option<DenoiseRef>,
    lens: Option<bool>,
) -> Result<String, Error> {
    // Fortschritt wird über `app` gemeldet
    let events = app.clone();
    crate::blocking(app, move |state| {
        // Gibt `busy` auch nach Fehlern und Panics in `compute` wieder frei
        let Some(_busy) = crate::busy::claim(&state.repair.busy) else {
            return Err(Error::Invalid(
                tr_s(
                    "Es läuft bereits eine Reparatur.",
                    "A repair is already running.",
                )
                .into(),
            ));
        };
        state.repair.cancel.store(false, Ordering::SeqCst);
        compute(
            &events,
            state,
            image_id,
            shapes,
            denoise,
            lens.unwrap_or(false),
        )
    })
    .await
}

fn compute(
    app: &AppHandle,
    state: &AppState,
    image_id: i64,
    shapes: Vec<MaskShape>,
    denoise: Option<DenoiseRef>,
    lens: bool,
) -> Result<String, Error> {
    let model = inpainter(state)?;
    let job = state.catalog.locked().thumb_job(image_id)?;
    let t = std::time::Instant::now();
    let mut src = Source::load(
        Path::new(&job.path),
        job.kind == ImageKind::Raw,
        job.orientation.map(|o| o as u16),
        None,
    )?;
    // Rauschreduzierung einmischen wie in der Pipeline
    if let Some(d) = denoise.filter(|d| d.amount > 0.0)
        && let Some(p) = lumenkorn_develop::denoised::path(&state.denoised_dir, &d.id)
        && let Ok(dn) = lumenkorn_develop::denoised::read(&p, None)
        && dn.width == src.image.width
        && dn.height == src.image.height
    {
        let a = (d.amount / 100.0).clamp(0.0, 1.0);
        for (o, n) in src.image.data.iter_mut().zip(&dn.data) {
            for k in 0..3 {
                o[k] += (n[k] - o[k]) * a;
            }
        }
    }
    // Auf dem objektivkorrigierten Bild, wie es angezeigt wird (die Pipeline
    // setzt solche Flicken nach der Korrektur ein)
    if lens && let Some(l) = &src.lens {
        src.image = l.apply(&src.image);
    }
    let (w, h) = (src.image.width, src.image.height);
    let layer = MaskLayer {
        shapes,
        ..Default::default()
    };
    let alpha =
        lumenkorn_develop::masks::evaluate(&layer, w, h, state.masks.as_ref(), Some(&src.image));
    let progress = |done: usize, total: usize| {
        let _ = app.emit(
            "repair-progress",
            RepairProgress {
                image_id,
                done,
                total,
            },
        );
    };
    let patch = {
        let _g = analysis_guard(state, tr_s("Reparatur", "Repair"));
        model
            .repair(&src.image, &alpha, &progress, &state.repair.cancel)
            .map_err(err)?
    };
    let id = MaskFiles::new_id(image_id, "rep");
    let parts = patch.parts.len();
    state.repairs.put(&id, patch)?;
    log::info!(
        "Reparatur für Bild {image_id}: {parts} Bereiche in {:.2?} ({w}×{h})",
        t.elapsed()
    );
    // Thumbnail und Vorschau neu erzeugen lassen
    state.cache.remove(image_id);
    Ok(id)
}

#[tauri::command]
pub fn repair_cancel(state: State<AppState>) {
    state.repair.cancel.store(true, Ordering::SeqCst);
}
