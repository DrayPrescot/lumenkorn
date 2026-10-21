//! Prüft „Auto nach deinem Stil“ an einem echten Katalog, ohne Oberfläche:
//!
//!     cargo run --release -p lumenkorn-core --example style_probe -- <Katalog.db>
//!
//! Berechnet für jedes bearbeitete Foto das Lernbeispiel wie die App
//! (`src-tauri/src/style.rs`), lernt daraus und vergleicht je Regler, wie weit
//! das Standard-Auto bzw. das angepasste Auto von den Endwerten des Nutzers
//! entfernt liegt (jedes Foto ohne sich selbst vorhergesagt). Die Fotos werden
//! nur gelesen. Bitte nur mit einer Kopie des Katalogs aufrufen: Beim Öffnen
//! werden ausstehende Migrationen eingespielt.

use lumenkorn_core::formats::ImageKind;
use lumenkorn_core::*;
use lumenkorn_develop::Source;
use lumenkorn_develop::auto::auto_tone_with_features;
use lumenkorn_develop::style::{Query, Sample, StyleModel, TONE};
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

fn main() -> Result<()> {
    let db = std::env::args()
        .nth(1)
        .expect("Aufruf: style_probe <Katalog.db>");
    let cat = Catalog::open(Path::new(&db))?;
    let edited = cat.edited_overview()?;
    println!("{} bearbeitete Fotos", edited.len());
    let t = Instant::now();
    let mut samples: Vec<Sample> = Vec::new();
    for (id, stamp, camera, raw) in edited {
        let job = cat.thumb_job(id)?;
        let p = cat.develop_params(id)?;
        let src = match Source::load(
            Path::new(&job.path),
            job.kind == ImageKind::Raw,
            job.orientation.map(|o| o as u16),
            Some(1024),
        ) {
            Ok(s) => s,
            Err(e) => {
                println!("  übersprungen ({e})");
                continue;
            }
        };
        let (a, features) = auto_tone_with_features(&src, &p);
        let user = [
            p.exposure,
            p.contrast,
            p.highlights,
            p.shadows,
            p.whites,
            p.blacks,
            p.vibrance,
        ];
        let kind = if raw { "RAW" } else { "JPEG" };
        samples.push(Sample {
            image_id: id,
            group: match camera.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
                Some(c) => format!("{c} ({kind})"),
                None => kind.into(),
            },
            stamp,
            auto: [
                a.exposure,
                a.contrast,
                a.highlights,
                a.shadows,
                a.whites,
                a.blacks,
                a.vibrance,
            ],
            user,
            tone_edited: user.iter().any(|v| v.abs() > 1e-4),
            extra: [p.saturation, p.clarity, p.dehaze, p.sharpen],
            features,
        });
    }
    println!("{} Lernbeispiele in {:.1?}\n", samples.len(), t.elapsed());
    let now = samples.iter().map(|s| s.stamp).max().unwrap_or(0);
    let model = StyleModel::learn(&samples, &HashMap::new(), now);
    let sum = model.summary();
    println!(
        "Stufen aktiv: Ausgleich {} · Bildhelligkeit {} · ähnliche Motive {}",
        sum.stage2, sum.stage3, sum.stage4
    );
    for g in &sum.groups {
        println!(
            "  {} ({} Fotos): Ton {:?}, weitere {:?}",
            g.group, g.samples, g.tone, g.extra
        );
    }
    // Jedes Foto ohne sich selbst vorhersagen
    let tone: Vec<&Sample> = samples.iter().filter(|s| s.tone_edited).collect();
    let mut plain = [0f32; 7];
    let mut learned = [0f32; 7];
    for s in &tone {
        let others: Vec<Sample> = samples
            .iter()
            .filter(|x| x.image_id != s.image_id)
            .cloned()
            .collect();
        let m = StyleModel::learn(&others, &HashMap::new(), now);
        let pred = m.predict(&Query {
            group: &s.group,
            features: s.features,
            embedding: None,
        });
        for k in 0..7 {
            plain[k] += (s.user[k] - s.auto[k]).abs();
            learned[k] += (s.user[k] - s.auto[k] - pred.tone_delta[k]).abs();
        }
    }
    let n = tone.len().max(1) as f32;
    println!(
        "\nMittlerer Abstand zu deinem Endwert (jedes Foto ohne sich selbst gelernt, {} Fotos):",
        tone.len()
    );
    println!("  {:<11} {:>12} {:>12}", "Regler", "Standard", "angepasst");
    for k in 0..7 {
        println!(
            "  {:<11} {:>12.2} {:>12.2}",
            TONE[k],
            plain[k] / n,
            learned[k] / n
        );
    }
    Ok(())
}
