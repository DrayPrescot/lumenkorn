//! Personenerkennung mit den echten Modellen:
//!
//!     LUMENKORN_TEST_ORT=/pfad/libonnxruntime.so LUMENKORN_TEST_FACES=/pfad cargo test -p lumenkorn-ai --test faces
//!
//! `LUMENKORN_TEST_FACES` enthält yunet.onnx, sface.onnx und die OpenCV-
//! Beispielbilder lena.jpg, messi5.jpg und largest_selfie.jpg.

use image::RgbImage;
use lumenkorn_ai::clip::dot as cosine;
use lumenkorn_ai::faces::{FaceDetector, FaceEmbedder, SAME_PERSON};
// Gruppiert wird im Katalog – dieselbe Schwelle wie in der App
use lumenkorn_ai::runtime;
use lumenkorn_core::people::GROUP;
use std::path::PathBuf;

fn setup() -> Option<(PathBuf, FaceDetector, FaceEmbedder)> {
    let lib = std::env::var("LUMENKORN_TEST_ORT").ok()?;
    let dir = PathBuf::from(std::env::var("LUMENKORN_TEST_FACES").ok()?);
    runtime::init(std::path::Path::new(&lib)).unwrap();
    let det = FaceDetector::load(&dir.join("yunet.onnx")).unwrap();
    let emb = FaceEmbedder::load(&dir.join("sface.onnx")).unwrap();
    Some((dir, det, emb))
}

fn load(dir: &std::path::Path, name: &str) -> RgbImage {
    image::open(dir.join(name)).unwrap().to_rgb8()
}

#[test]
fn finds_and_recognizes_faces() {
    let Some((dir, det, emb)) = setup() else {
        eprintln!("übersprungen: LUMENKORN_TEST_ORT/LUMENKORN_TEST_FACES fehlen");
        return;
    };

    // Einzelporträts
    let lena = load(&dir, "lena.jpg");
    let lf = det.detect(&lena).unwrap();
    assert_eq!(lf.len(), 1, "Lena: {lf:?}");
    // OpenCV FaceDetectorYN liefert [207.8, 182.5, 145.9, 206.9]
    eprintln!("Lena: {:?} ({:.3})", lf[0].bbox, lf[0].score);
    for (a, b) in lf[0].bbox.iter().zip([207.8, 182.5, 145.9, 206.9]) {
        assert!(
            (a - b).abs() < 6.0,
            "Lena-Kasten weicht von OpenCV ab: {:?}",
            lf[0].bbox
        );
    }
    let messi = load(&dir, "messi5.jpg");
    let mf = det.detect(&messi).unwrap();
    assert!(!mf.is_empty(), "Messi: kein Gesicht");

    // Gruppenfoto: viele, auch kleine Gesichter. OpenCVs FaceDetectorYN findet
    // bei voller Auflösung und Schwelle 0,8 hier 58 (kleinstes 15 px breit).
    let crowd = load(&dir, "largest_selfie.jpg");
    let t = std::time::Instant::now();
    let cf = det.detect(&crowd).unwrap();
    eprintln!(
        "Gruppenfoto {}×{}: {} Gesichter in {:.2?}",
        crowd.width(),
        crowd.height(),
        cf.len(),
        t.elapsed()
    );
    assert!(cf.len() >= 35, "nur {} Gesichter", cf.len());
    // Landmarken liegen im Kasten
    for f in &cf {
        let [x, y, w, h] = f.bbox;
        for p in f.landmarks {
            assert!(
                p[0] > x - w * 0.2
                    && p[0] < x + w * 1.2
                    && p[1] > y - h * 0.2
                    && p[1] < y + h * 1.2
            );
        }
    }

    // Wiedererkennen: dieselbe Person, verkleinert und aufgehellt
    let e_lena = emb.embed(&lena, &lf[0]).unwrap();
    let mut changed =
        image::imageops::resize(&lena, 330, 330, image::imageops::FilterType::Triangle);
    for p in changed.pixels_mut() {
        for c in 0..3 {
            p[c] = (p[c] as f32 * 0.8 + 30.0).min(255.0) as u8;
        }
    }
    let cf2 = det.detect(&changed).unwrap();
    assert_eq!(cf2.len(), 1);
    let e_lena2 = emb.embed(&changed, &cf2[0]).unwrap();
    let same = cosine(&e_lena, &e_lena2);

    // Verschiedene Personen
    let e_messi = emb.embed(&messi, &mf[0]).unwrap();
    let diff = cosine(&e_lena, &e_messi);
    eprintln!(
        "Ähnlichkeit: gleiche Person {same:.3}, verschiedene {diff:.3} (Schwelle {SAME_PERSON})"
    );
    assert!(same > 0.6, "gleiche Person nur {same}");
    assert!(diff < SAME_PERSON, "verschiedene Personen {diff}");

    // Im Gruppenfoto sind alle Paare verschiedene Personen. Kleine, ähnlich
    // beleuchtete Gesichter liegen öfter über 0,363 – OpenCVs eigene Pipeline
    // (alignCrop + feature) kommt hier auf 59 von 435 Paaren, höchstens 0,545.
    // Lumenkorn gruppiert deshalb erst ab GROUP (0,5) automatisch.
    let big: Vec<_> = cf.iter().filter(|f| f.bbox[2] >= 40.0).take(30).collect();
    let embs: Vec<Vec<f32>> = big.iter().map(|f| emb.embed(&crowd, f).unwrap()).collect();
    let (mut pairs, mut over, mut grouped) = (0, 0, 0);
    for i in 0..embs.len() {
        for j in i + 1..embs.len() {
            pairs += 1;
            let s = cosine(&embs[i], &embs[j]);
            over += usize::from(s >= SAME_PERSON);
            grouped += usize::from(s >= GROUP);
        }
    }
    eprintln!(
        "{} Gesichter ≥ 40 px: {over} von {pairs} Paaren ≥ {SAME_PERSON}, {grouped} ≥ {GROUP}",
        embs.len()
    );
    assert!(
        over <= 59,
        "schlechter als OpenCVs Referenz: {over}/{pairs}"
    );
    assert!(
        grouped * 100 <= pairs * 2,
        "würde zu oft falsch gruppieren: {grouped}/{pairs}"
    );

    if let Ok(o) = std::env::var("LUMENKORN_TEST_OUT") {
        let mut img = crowd.clone();
        for f in &cf {
            let [x, y, w, h] = f.bbox.map(|v| v.max(0.0) as u32);
            for dx in 0..w {
                for yy in [y, (y + h).min(img.height() - 1)] {
                    if x + dx < img.width() {
                        img.put_pixel(x + dx, yy, image::Rgb([0, 255, 0]));
                    }
                }
            }
            for dy in 0..h {
                for xx in [x, (x + w).min(img.width() - 1)] {
                    if y + dy < img.height() {
                        img.put_pixel(xx, y + dy, image::Rgb([0, 255, 0]));
                    }
                }
            }
        }
        img.save(PathBuf::from(&o).join("gesichter.jpg")).unwrap();
        lumenkorn_ai::faces::align(&lena, &lf[0])
            .save(PathBuf::from(&o).join("lena_ausgerichtet.png"))
            .unwrap();
    }
}
