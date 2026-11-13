//! KI-Rauschreduzierung im Hintergrund, Fortschritt per `denoise-progress`.

use super::*;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct DenoiseEvent {
    image_id: i64,
    done: usize,
    total: usize,
    finished: bool,
    error: Option<String>,
    /// Datei-ID des Ergebnisses (bei Erfolg)
    denoise_id: Option<String>,
}

pub(super) fn denoiser(state: &AppState) -> Result<Arc<lumenkorn_ai::denoise::Denoiser>, Error> {
    state.usage.touch(Slot::Interactive);
    if !models::status(&state.data_dir).denoise_ready {
        return Err(Error::Invalid(
            tr_s(
                "Die KI-Rauschreduzierung ist noch nicht installiert. Lade sie über „KI“ unten links.",
                "AI denoise is not installed yet. Download it via “AI” at the bottom left.",
            )
            .into(),
        ));
    }
    lazy_model(
        state,
        &state.ai.denoiser,
        tr_s("DRUNet (Rauschreduzierung)", "DRUNet (denoise)"),
        || {
            let path = models::denoise_dir(&state.data_dir).join(models::DENOISE_MODEL);
            lumenkorn_ai::denoise::Denoiser::load(&path).map_err(err)
        },
    )
}

/// Entrauscht ein Bild in voller Auflösung im Hintergrund. Fortschritt und
/// Ergebnis kommen als `denoise-progress`. `strength` skaliert die geschätzte
/// Rauschstärke (1,0 = automatisch).
#[tauri::command(async)]
pub fn denoise_start(app: AppHandle, id: i64, strength: Option<f32>) -> Result<(), Error> {
    // Solange die Anmeldung besteht, wird DRUNet nicht entladen
    let claim = crate::task::claim(&app, |t| &t.denoise)?.full_res()?;
    // Reglerwert „Stärke“ (0–100) → Faktor auf die geschätzte Rauschstärke
    let strength = lumenkorn_ai::denoise::strength_factor(strength.unwrap_or(50.0));
    claim.spawn("denoise", move |app, claim| {
        let state = app.state::<AppState>();
        let emit = |done, total, finished, error: Option<String>, denoise_id: Option<String>| {
            let _ = app.emit(
                "denoise-progress",
                DenoiseEvent {
                    image_id: id,
                    done,
                    total,
                    finished,
                    error,
                    denoise_id,
                },
            );
        };
        // Ein Panic soll als Fehler gemeldet werden (sonst zeigte die
        // Oberfläche weiter „läuft“), nicht nur `denoising` freigeben
        let res = crate::task::catch_panic(|| -> Result<String, Error> {
            let d = denoiser(&state)?;
            let job = state.catalog.locked().thumb_job(id)?;
            let t = std::time::Instant::now();
            let src = Source::load(
                Path::new(&job.path),
                job.kind == lumenkorn_core::formats::ImageKind::Raw,
                job.orientation.map(|o| o as u16),
                None,
            )?;
            let img = &src.image;
            let _g = analysis_guard(&state, tr_s("Rauschreduzierung", "Denoise"));
            let out = d
                .denoise(
                    &lumenkorn_ai::denoise::Rgb {
                        width: img.width,
                        height: img.height,
                        data: &img.data,
                    },
                    strength,
                    &|done, total| emit(done, total, false, None, None),
                    claim.task().cancel_flag(),
                )
                .map_err(err)?;
            let denoise_id = MaskFiles::new_id(id, "dn");
            std::fs::create_dir_all(&state.denoised_dir)?;
            let path = lumenkorn_develop::denoised::path(&state.denoised_dir, &denoise_id)
                .ok_or_else(|| Error::Invalid(tr_s("ungültige ID", "invalid ID").into()))?;
            lumenkorn_develop::denoised::write(&path, img.width, img.height, &out)?;
            log::info!(
                "Bild {id} entrauscht in {:.1?} ({}×{})",
                t.elapsed(),
                img.width,
                img.height
            );
            Ok(denoise_id)
        });
        // Vor der Meldung freigeben: Die Oberfläche darf gleich neu starten
        drop(claim);
        match res {
            Ok(did) => emit(1, 1, true, None, Some(did)),
            Err(e) => emit(0, 0, true, Some(e.to_string()), None),
        }
    })
}

#[tauri::command]
pub fn denoise_cancel(state: State<AppState>) {
    state.tasks.denoise.cancel();
}
