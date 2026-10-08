//! GPU-Pipeline gegen CPU-Pipeline, Pixel für Pixel.
//!
//! Läuft nur, wenn eine Grafikkarte da ist. Ohne Grafikkarte (CI, Bauumgebung)
//! genügt ein Software-Vulkan (Mesa lavapipe):
//!
//!     LUMENKORN_GPU_SOFTWARE=1 cargo test -p lumenkorn-develop --test gpu

use lumenkorn_develop::gpu::{self, Rendered};
use lumenkorn_develop::{
    Crop, DenoiseRef, DevelopParams, GrayMask, ImageF32, LocalAdjust, MaskLayer, MaskShape,
    MaskSource, RenderOptions, Source, SourceKind, Stroke, render_with,
};
use std::sync::Arc;

struct OneMask(Arc<GrayMask>);
impl MaskSource for OneMask {
    fn get(&self, id: &str) -> Option<Arc<GrayMask>> {
        (id == "motiv").then(|| self.0.clone())
    }
}

/// Testbild mit Verläufen, Kanten und feiner Struktur (linear, teils > 1 für RAW).
fn source(kind: SourceKind, w: usize, h: usize) -> Source {
    let scale = if kind == SourceKind::Raw { 2.5 } else { 1.0 };
    let image = ImageF32::from_fn(w, h, |x, y| {
        let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
        let n = ((x * 7 + y * 13) % 17) as f32 / 17.0 - 0.5;
        let disk = ((fx - 0.6).powi(2) + (fy - 0.4).powi(2)).sqrt() < 0.2;
        let base = if disk {
            [0.8, 0.25, 0.1]
        } else {
            [fx * 0.9, 0.15 + fy * 0.6, 0.5 * (1.0 - fx)]
        };
        base.map(|v: f32| ((v + 0.04 * n).max(0.0)).powf(2.2) * scale)
    });
    Source {
        kind,
        full_width: w,
        full_height: h,
        image,
    }
}

/// Die GPU – oder `None`, wenn es schlicht keine gibt. Andere Fehler (Shader
/// nicht übersetzbar) lassen den Test scheitern.
fn gpu_or_skip() -> Option<&'static gpu::Gpu> {
    match gpu::init() {
        Ok(g) => Some(g),
        Err(e) if e.starts_with("keine Grafikkarte") || e.starts_with("nur Software") => {
            eprintln!("übersprungen – keine GPU: {e}");
            None
        }
        Err(e) => panic!("GPU-Pipeline defekt: {e}"),
    }
}

fn cases() -> Vec<(&'static str, DevelopParams)> {
    let d = DevelopParams::default;
    let linear = MaskLayer {
        shapes: vec![MaskShape::Linear {
            x0: 0.5,
            y0: 0.0,
            x1: 0.5,
            y1: 0.6,
            invert: false,
            subtract: false,
        }],
        adjust: LocalAdjust {
            exposure: 0.8,
            contrast: 30.0,
            shadows: 40.0,
            clarity: 50.0,
            saturation: -40.0,
            temperature: 20.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let radial_brush = MaskLayer {
        shapes: vec![
            MaskShape::Radial {
                cx: 0.4,
                cy: 0.5,
                rx: 0.3,
                ry: 0.2,
                angle: 20.0,
                feather: 0.5,
                invert: false,
                subtract: false,
            },
            MaskShape::Brush {
                strokes: vec![Stroke {
                    points: vec![[0.1, 0.1], [0.5, 0.3], [0.8, 0.2]],
                    radius: 0.05,
                    feather: 0.6,
                    opacity: 0.9,
                    erase: false,
                }],
                subtract: false,
            },
            MaskShape::Raster {
                mask_id: "motiv".into(),
                kind: "subject".into(),
                prompt: vec![],
                invert: true,
                subtract: true,
            },
        ],
        adjust: LocalAdjust {
            highlights: -60.0,
            tint: -30.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let color_mask = MaskLayer {
        shapes: vec![MaskShape::Color {
            samples: vec![lumenkorn_develop::ColorSample {
                x: 0.3,
                y: 0.4,
                lab: [0.6, 0.1, 0.05],
            }],
            range: 40.0,
            invert: false,
            subtract: false,
        }],
        adjust: LocalAdjust {
            exposure: -0.5,
            ..Default::default()
        },
        ..Default::default()
    };
    vec![
        ("Standard", d()),
        (
            "HSL",
            DevelopParams {
                hsl: lumenkorn_develop::Hsl {
                    hue: [40.0, -30.0, 0.0, 60.0, 0.0, -50.0, 20.0, 100.0],
                    saturation: [-100.0, 50.0, 30.0, -20.0, 80.0, 40.0, 0.0, -60.0],
                    luminance: [30.0, 0.0, -40.0, 0.0, 20.0, -80.0, 50.0, 0.0],
                },
                saturation: 20.0,
                ..d()
            },
        ),
        (
            "Farbmaske",
            DevelopParams {
                masks: vec![color_mask.clone()],
                ..d()
            },
        ),
        (
            "Grundregler",
            DevelopParams {
                exposure: 0.7,
                temperature: 35.0,
                tint: -20.0,
                contrast: 40.0,
                ..d()
            },
        ),
        (
            "Lichter/Tiefen/Weiß/Schwarz",
            DevelopParams {
                highlights: -80.0,
                shadows: 70.0,
                whites: 30.0,
                blacks: -40.0,
                ..d()
            },
        ),
        (
            "Klarheit, Kontrast negativ",
            DevelopParams {
                clarity: 60.0,
                contrast: -50.0,
                ..d()
            },
        ),
        (
            "Dehaze",
            DevelopParams {
                dehaze: 70.0,
                exposure: 0.4,
                ..d()
            },
        ),
        (
            "Color Grading",
            DevelopParams {
                color_grade: lumenkorn_develop::ColorGrade {
                    shadows: lumenkorn_develop::GradeWheel {
                        hue: 210.0,
                        saturation: 60.0,
                        luminance: -20.0,
                    },
                    midtones: lumenkorn_develop::GradeWheel {
                        hue: 120.0,
                        saturation: 30.0,
                        luminance: 0.0,
                    },
                    highlights: lumenkorn_develop::GradeWheel {
                        hue: 40.0,
                        saturation: 70.0,
                        luminance: 15.0,
                    },
                    global: lumenkorn_develop::GradeWheel {
                        hue: 300.0,
                        saturation: 10.0,
                        luminance: 5.0,
                    },
                    blending: 70.0,
                    balance: -30.0,
                },
                ..d()
            },
        ),
        (
            "Dehaze negativ (Dunst)",
            DevelopParams {
                dehaze: -60.0,
                ..d()
            },
        ),
        (
            "Farbe",
            DevelopParams {
                vibrance: 60.0,
                saturation: -30.0,
                ..d()
            },
        ),
        (
            "Schärfen",
            DevelopParams {
                sharpen: 120.0,
                ..d()
            },
        ),
        (
            "Masken",
            DevelopParams {
                masks: vec![linear.clone(), radial_brush],
                ..d()
            },
        ),
        (
            "Geometrie 90° + Spiegeln + Crop",
            DevelopParams {
                rotate: 1,
                flip_horizontal: true,
                crop: Some(Crop {
                    x: 0.1,
                    y: 0.2,
                    w: 0.7,
                    h: 0.5,
                }),
                ..d()
            },
        ),
        (
            "Geraderichten 180°/270°",
            DevelopParams {
                rotate: 3,
                straighten: -7.5,
                sharpen: 40.0,
                ..d()
            },
        ),
        (
            "Alles zusammen",
            DevelopParams {
                exposure: 0.4,
                contrast: 25.0,
                highlights: -40.0,
                shadows: 30.0,
                clarity: 30.0,
                vibrance: 30.0,
                sharpen: 60.0,
                rotate: 2,
                straighten: 3.0,
                masks: vec![linear],
                denoise: Some(DenoiseRef {
                    id: "x".into(),
                    amount: 60.0,
                }),
                ..d()
            },
        ),
    ]
}

#[test]
fn gpu_matches_cpu() {
    let Some(g) = gpu_or_skip() else { return };
    eprintln!("GPU: {} ({})", g.info.name, g.info.backend);
    let mask = GrayMask {
        width: 64,
        height: 40,
        data: (0..64 * 40)
            .map(|i| {
                if (i % 64) > 20 && (i / 64) > 10 {
                    230
                } else {
                    10
                }
            })
            .collect(),
    };
    let store = OneMask(Arc::new(mask));

    for kind in [SourceKind::Raster, SourceKind::Raw] {
        let src = source(kind, 301, 203);
        let dn = ImageF32 {
            data: src.image.data.iter().map(|p| p.map(|v| v * 0.9)).collect(),
            ..src.image.clone()
        };
        for (name, params) in cases() {
            for overlay in [None, Some(0)] {
                if overlay.is_some() && params.masks.is_empty() {
                    continue;
                }
                let opts = RenderOptions {
                    masks: &store,
                    overlay,
                    denoised: Some(&dn),
                    region: None,
                    repairs: &lumenkorn_develop::repair::NoRepairs,
                };
                let cpu = render_with(&src, &params, &opts);
                if name != "Standard" {
                    let base = render_with(&src, &DevelopParams::default(), &opts);
                    let changed = cpu.data.len() != base.data.len()
                        || cpu.data.iter().zip(&base.data).any(|(a, b)| a != b);
                    assert!(changed, "{name}: Testfall verändert das Bild nicht");
                }
                let Rendered::Float(gpu_img) = g.render(&src, &params, &opts, false).unwrap()
                else {
                    panic!("Float erwartet")
                };
                assert_eq!(
                    (gpu_img.width, gpu_img.height),
                    (cpu.width, cpu.height),
                    "{name}: Größe"
                );
                let mut max = 0f32;
                let mut sum = 0f64;
                let mut off = 0usize;
                for (a, b) in cpu.data.iter().zip(&gpu_img.data) {
                    for k in 0..3 {
                        let d = (a[k] - b[k]).abs();
                        max = max.max(d);
                        sum += d as f64;
                        if d > 2.0 / 255.0 {
                            off += 1;
                        }
                    }
                }
                let mean = sum / (cpu.data.len() * 3) as f64;
                eprintln!(
                    "{kind:?} {name}{}: max {max:.5}, Mittel {mean:.6}, >2/255: {off}",
                    if overlay.is_some() {
                        " (Überlagerung)"
                    } else {
                        ""
                    }
                );
                assert!(mean < 2e-4, "{name}: mittlere Abweichung {mean}");
                // Einzelne Ausreißer an harten Kanten (Rundung im Kastenfilter)
                // sind erlaubt, aber nur sehr wenige.
                assert!(
                    off <= cpu.data.len() / 1000 && max < 0.03,
                    "{name}: {off} Werte > 2/255, max {max}"
                );

                // 8-Bit-Ausgabe passt zur 8-Bit-Wandlung der CPU
                let Rendered::Rgb8(rgb) = g.render(&src, &params, &opts, true).unwrap() else {
                    panic!("RGB8 erwartet")
                };
                let cpu8 = lumenkorn_develop::output::to_rgb8(&cpu);
                let max8 = rgb
                    .as_raw()
                    .iter()
                    .zip(cpu8.as_raw())
                    .map(|(a, b)| (*a as i32 - *b as i32).abs())
                    .max()
                    .unwrap();
                assert!(max8 <= 8, "{name}: 8-Bit-Abweichung {max8}");
            }
        }
    }
}

#[test]
fn reuses_buffers_across_sizes() {
    let Some(g) = gpu_or_skip() else { return };
    // Wechsel zwischen Vorschau- und Thumbnail-Größe, dann wieder zurück
    for (w, h) in [(640, 480), (120, 90), (640, 480), (97, 211)] {
        let src = source(SourceKind::Raster, w, h);
        let p = DevelopParams {
            exposure: 0.3,
            ..Default::default()
        };
        let cpu = render_with(&src, &p, &RenderOptions::default());
        let Rendered::Float(img) = g
            .render(&src, &p, &RenderOptions::default(), false)
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(img.data.len(), cpu.data.len());
        let max = cpu
            .data
            .iter()
            .zip(&img.data)
            .map(|(a, b)| (a[0] - b[0]).abs())
            .fold(0f32, f32::max);
        assert!(max < 2e-3, "{w}×{h}: {max}");
    }
}

/// Detail-Ausschnitt (Zoom): GPU und CPU liefern denselben Ausschnitt, und er
/// entspricht dem entsprechenden Teil des ganzen Bildes.
#[test]
fn region_matches_cpu_and_full_image() {
    let src = source(SourceKind::Raw, 400, 300);
    let params = DevelopParams {
        rotate: 1,
        straighten: 4.0,
        crop: Some(Crop {
            x: 0.1,
            y: 0.1,
            w: 0.8,
            h: 0.7,
        }),
        clarity: 40.0,
        sharpen: 60.0,
        ..Default::default()
    };
    let region = Crop {
        x: 0.25,
        y: 0.5,
        w: 0.5,
        h: 0.25,
    };
    let full = render_with(&src, &params, &RenderOptions::default());
    let opts = RenderOptions {
        region: Some(region),
        ..Default::default()
    };
    let cpu = render_with(&src, &params, &opts);
    // Ausschnitt = passender Teil des ganzen Bildes
    let (x0, y0, w, h) = lumenkorn_develop::geometry::crop_rect(full.width, full.height, region);
    assert_eq!((cpu.width, cpu.height), (w, h));
    for y in 0..h {
        for x in 0..w {
            assert_eq!(cpu.get(x, y), full.get(x0 + x, y0 + y));
        }
    }
    let Some(g) = gpu_or_skip() else { return };
    let Rendered::Float(gi) = g.render(&src, &params, &opts, false).unwrap() else {
        panic!()
    };
    assert_eq!((gi.width, gi.height), (w, h));
    let max = cpu
        .data
        .iter()
        .zip(&gi.data)
        .flat_map(|(a, b)| (0..3).map(move |k| (a[k] - b[k]).abs()))
        .fold(0f32, f32::max);
    assert!(max < 1e-3, "GPU-Ausschnitt weicht ab: {max}");
}

/// Reparaturen: Der GPU-Weg (Flicken vorab eingesetzt) liefert dasselbe wie die CPU.
#[test]
fn repairs_match_on_gpu() {
    use lumenkorn_develop::repair::{Patch, PatchPart, RepairSource};
    use lumenkorn_develop::{RepairLayer, render_auto};
    struct One(Arc<Patch>);
    impl RepairSource for One {
        fn get(&self, id: &str) -> Option<Arc<Patch>> {
            (id == "p1").then(|| self.0.clone())
        }
    }
    let Some(_) = gpu_or_skip() else { return };
    gpu::set_enabled(true);
    let src = source(SourceKind::Raw, 301, 203);
    // Flicken in voller Auflösung (doppelt so groß wie die Quelle)
    let patch = Patch {
        width: 602,
        height: 406,
        parts: vec![PatchPart {
            x: 100,
            y: 80,
            w: 120,
            h: 90,
            rgb: vec![[0.05, 0.4, 0.1]; 120 * 90],
            alpha: (0..120 * 90)
                .map(|i| ((i % 120) * 2).min(255) as u8)
                .collect(),
        }],
    };
    let store = One(Arc::new(patch));
    let dn = ImageF32 {
        data: src.image.data.iter().map(|p| p.map(|v| v * 0.9)).collect(),
        ..src.image.clone()
    };
    let params = DevelopParams {
        contrast: 30.0,
        rotate: 1,
        denoise: Some(DenoiseRef {
            id: "x".into(),
            amount: 50.0,
        }),
        repairs: vec![RepairLayer {
            id: "r".into(),
            enabled: true,
            result: Some("p1".into()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let opts = RenderOptions {
        repairs: &store,
        denoised: Some(&dn),
        ..Default::default()
    };
    let cpu = render_with(&src, &params, &opts);
    let without = render_with(
        &src,
        &DevelopParams {
            repairs: vec![],
            ..params.clone()
        },
        &opts,
    );
    assert!(cpu.data != without.data, "Flicken wirkt nicht");
    let g = render_auto(&src, &params, &opts);
    let max = cpu
        .data
        .iter()
        .zip(&g.data)
        .flat_map(|(a, b)| (0..3).map(move |k| (a[k] - b[k]).abs()))
        .fold(0f32, f32::max);
    assert!(max < 1e-3, "GPU weicht ab: {max}");
    gpu::set_enabled(false);
}
