//! Tests mit echter ONNX Runtime und einem Modellordner:
//!
//!     LUMENKORN_TEST_ORT=/pfad/libonnxruntime.so \
//!     LUMENKORN_TEST_MODEL=/pfad/zum/modell cargo test -p lumenkorn-ai
//!
//! Der Modellordner muss das CLIP-Format haben (vision.onnx, text.onnx,
//! vocab.json, merges.txt). Für die Ranking-Tests reicht ein Testmodell, das
//! „red/green/blue“ auf die Farbe des Bildes abbildet; mit einem echten CLIP
//! funktionieren sie ebenso. Ohne Variablen werden die Tests übersprungen.

use image::{DynamicImage, Rgb, RgbImage};
use lumenkorn_ai::tokenizer::ClipTokenizer;
use lumenkorn_ai::vocab::{Concept, Group};
use lumenkorn_ai::*;
use std::path::PathBuf;

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var(name).ok().map(PathBuf::from)
}

fn solid(r: u8, g: u8, b: u8) -> DynamicImage {
    // Leichtes Rauschen, damit es kein völlig künstliches Bild ist
    DynamicImage::ImageRgb8(RgbImage::from_fn(320, 240, |x, y| {
        let n = ((x * 7 + y * 13) % 9) as u8;
        Rgb([
            r.saturating_add(n),
            g.saturating_add(n),
            b.saturating_add(n),
        ])
    }))
}

#[test]
fn tokenizer_matches_openai_reference() {
    let Some(dir) = env_path("LUMENKORN_TEST_MODEL") else {
        eprintln!("übersprungen: LUMENKORN_TEST_MODEL fehlt");
        return;
    };
    let tok = ClipTokenizer::load(&dir).unwrap();
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("token_cases.json")).unwrap())
            .unwrap();
    for c in cases {
        let text = c["text"].as_str().unwrap();
        let want: Vec<i64> = c["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap())
            .collect();
        assert_eq!(tok.encode(text), want, "{text:?}");
    }
    let (ids, mask) = tok.encode_padded("a photo of a dog");
    assert_eq!(ids.len(), 77);
    assert_eq!(&ids[..7], &[tok.sot, 320, 1125, 539, 320, 1929, tok.eot]);
    assert_eq!(mask.iter().sum::<i64>(), 7);
}

#[test]
fn embeds_tags_and_searches() {
    let (Some(lib), Some(dir)) = (
        env_path("LUMENKORN_TEST_ORT"),
        env_path("LUMENKORN_TEST_MODEL"),
    ) else {
        eprintln!("übersprungen: LUMENKORN_TEST_ORT/LUMENKORN_TEST_MODEL fehlen");
        return;
    };
    runtime::init(&lib).unwrap();
    let clip = Clip::load(&dir).unwrap();

    let images = [solid(220, 30, 30), solid(30, 200, 40), solid(30, 40, 220)];
    let emb = clip.embed_images(&images).unwrap();
    assert_eq!(emb.len(), 3);
    assert!(
        (emb[0].iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-4,
        "normiert"
    );

    // Semantische Suche: „red“ findet das rote Bild zuerst
    let mut idx = EmbeddingIndex::default();
    for (i, e) in emb.iter().enumerate() {
        idx.upsert(i as i64 + 1, e);
    }
    for (query, want) in [("a photo of red", 1), ("green", 2), ("something blue", 3)] {
        let q = &clip.embed_texts(&[query]).unwrap()[0];
        let hits = idx.search(q, 3, -1.0);
        assert_eq!(hits[0].0, want, "{query}: {hits:?}");
    }

    // Verschlagwortung mit eigenem Vokabular
    let groups = vec![Group {
        name: "Farbe",
        neutral: "nothing",
        min_p: 0.3,
        max_tags: 1,
        concepts: vec![
            Concept {
                tag: "Rot",
                en: "Red",
                phrase: "red",
            },
            Concept {
                tag: "Grün",
                en: "Green",
                phrase: "green",
            },
            Concept {
                tag: "Blau",
                en: "Blue",
                phrase: "blue",
            },
        ],
    }];
    let tagger = Tagger::new(&clip, groups).unwrap();
    let names: Vec<String> = emb
        .iter()
        .map(|e| {
            tagger
                .tag(e)
                .first()
                .map(|t| t.tag.clone())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(names, ["Rot", "Grün", "Blau"]);

    // Das Standardvokabular lässt sich berechnen (Inhalt nur mit echtem CLIP sinnvoll)
    let t = std::time::Instant::now();
    let full = Tagger::new(&clip, vocab::default_groups()).unwrap();
    eprintln!("Vokabular in {:.2?}", t.elapsed());
    let _ = full.tag(&emb[0]);
}
