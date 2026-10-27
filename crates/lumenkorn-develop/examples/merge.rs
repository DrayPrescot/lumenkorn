//! Prüft HDR, Panorama und Focus Stacking an einem echten Foto, aus dem
//! passende Aufnahmereihen erzeugt werden:
//!
//!     cargo run --release -p lumenkorn-develop --example merge -- <RAW/JPEG> <Ausgabeordner>
//!
//! - HDR: drei Belichtungen (−2/0/+2 EV) mit Rauschen, Abschneiden bei Weiß
//!   und verwackelter heller Aufnahme
//! - Panorama: das Foto auf einen Zylinder gelegt, drei geschwenkte Aufnahmen
//! - Focus Stacking: vier Aufnahmen mit wandernder Schärfezone und Objektiv-Atmen
//!
//! Ergebnisse als EXR und entwickeltes JPEG im Ausgabeordner.

use lumenkorn_develop::color::luma;
use lumenkorn_develop::merge::align::{self, Mat3};
use lumenkorn_develop::merge::{focus, hdr, pano};
use lumenkorn_develop::output::{ExportFormat, ExportOptions};
use lumenkorn_develop::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

/// Deterministisches Rauschen
fn noise(i: usize, seed: u32) -> f32 {
    let mut v = (i as u32).wrapping_mul(2654435761) ^ seed;
    v = (v ^ (v >> 15)).wrapping_mul(2246822519);
    v = (v ^ (v >> 13)).wrapping_mul(3266489917);
    ((v >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 3.46 // Streuung ≈ 1
}

fn save(img: &ImageF32, dir: &Path, name: &str, scene: bool) -> Result<()> {
    let exr = dir.join(format!("{name}.exr"));
    let t = Instant::now();
    exr_io::write(img, &exr, scene)?;
    let tw = t.elapsed();
    let t = Instant::now();
    let mb = std::fs::metadata(&exr)?.len() as f64 / 1e6;
    let src = Source::load(&exr, false, None, Some(2048))?;
    let out = render(&src, &DevelopParams::default());
    output::write(
        &out,
        &dir.join(format!("{name}.jpg")),
        &ExportOptions {
            format: ExportFormat::Jpeg,
            formats: Vec::new(),
            dng_edited: true,
            quality: 88,
            long_edge: None,
        },
    )?;
    println!(
        "  gespeichert: {name}.exr ({mb:.1} MB, geschrieben in {tw:.2?}, gelesen + entwickelt in {:.2?}) + {name}.jpg",
        t.elapsed()
    );
    Ok(())
}

fn blur(img: &ImageF32, r: usize) -> ImageF32 {
    if r == 0 {
        return img.clone();
    }
    // Drei Kastenfilter ≈ Gauß; getrennt waagerecht/senkrecht
    let mut cur = img.clone();
    for _ in 0..3 {
        let (w, h) = (cur.width, cur.height);
        let src = cur.clone();
        let hor = ImageF32::from_fn(w, h, |x, y| {
            let (a, b) = (x.saturating_sub(r), (x + r + 1).min(w));
            let mut s = [0f32; 3];
            for xx in a..b {
                let p = src.get(xx, y);
                for c in 0..3 {
                    s[c] += p[c];
                }
            }
            s.map(|v| v / (b - a) as f32)
        });
        cur = ImageF32::from_fn(w, h, |x, y| {
            let (a, b) = (y.saturating_sub(r), (y + r + 1).min(h));
            let mut s = [0f32; 3];
            for yy in a..b {
                let p = hor.get(x, yy);
                for c in 0..3 {
                    s[c] += p[c];
                }
            }
            s.map(|v| v / (b - a) as f32)
        });
    }
    cur
}

fn warp(img: &ImageF32, h_out_to_in: &Mat3) -> ImageF32 {
    ImageF32::from_fn(img.width, img.height, |x, y| {
        let (sx, sy) = align::apply(h_out_to_in, x as f64, y as f64);
        align::sample(img, sx, sy).unwrap_or([0.0; 3])
    })
}

/// Prüfsumme über die Bitmuster (gleich = bitidentisch)
fn checksum(img: &ImageF32) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for p in &img.data {
        for v in p {
            hash = (hash ^ v.to_bits() as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    hash
}

/// Spitzenspeicher des Prozesses (nur Linux), in MB.
fn peak_rss_mb() -> Option<f64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let l = s.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: f64 = l.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}

/// Lastprobe Focus Stacking in voller Auflösung (`FOKUS_SERIE=<n>`): Jedes Bild
/// wird wie in der App beim Laden neu dekodiert (RAW → linear) und dann
/// verändert (scharfer Streifen wandert, Rest weich; Objektiv-Atmen). Gibt
/// Zeit, Spitzenspeicher und eine Prüfsumme des Ergebnisses aus;
/// `FOKUS_AUSRICHTEN=0` schaltet die Ausrichtung ab.
fn focus_series(input: &Path, raw: bool, n: usize, dir: &Path) -> Result<()> {
    use rayon::prelude::*;
    let load = |i: usize| -> Result<ImageF32> {
        let img = Source::load(input, raw, None, None)?.image;
        let (w, h) = (img.width, img.height);
        // Weich: Kastenfilter 5×5, getrennt und parallel
        let r = 2usize;
        let mut hor = vec![[0f32; 3]; w * h];
        hor.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let src = &img.data[y * w..(y + 1) * w];
            for (x, o) in row.iter_mut().enumerate() {
                let (a, b) = (x.saturating_sub(r), (x + r + 1).min(w));
                let mut s = [0f32; 3];
                for p in &src[a..b] {
                    for c in 0..3 {
                        s[c] += p[c];
                    }
                }
                *o = s.map(|v| v / (b - a) as f32);
            }
        });
        let band = |x: usize| x * n / w == i;
        let mut shot = vec![[0f32; 3]; w * h];
        shot.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let (a, b) = (y.saturating_sub(r), (y + r + 1).min(h));
            for (x, o) in row.iter_mut().enumerate() {
                if band(x) {
                    *o = img.data[y * w + x];
                    continue;
                }
                let mut s = [0f32; 3];
                for yy in a..b {
                    let p = hor[yy * w + x];
                    for c in 0..3 {
                        s[c] += p[c];
                    }
                }
                *o = s.map(|v| v / (b - a) as f32);
            }
        });
        drop(hor);
        let shot = ImageF32 {
            width: w,
            height: h,
            data: shot,
        };
        let s = i as f64 - (n / 2) as f64;
        if s == 0.0 {
            return Ok(shot);
        }
        let k = 1.0 + 0.002 * s;
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
        let m = [
            [k, 0.0, cx * (1.0 - k) + s * 0.8],
            [0.0, k, cy * (1.0 - k) - s * 0.5],
            [0.0, 0.0, 1.0],
        ];
        let inv = align::invert(&m).unwrap();
        Ok(ImageF32::from_fn(w, h, |x, y| {
            let (sx, sy) = align::apply(&inv, x as f64, y as f64);
            align::sample(&shot, sx, sy).unwrap_or([0.0; 3])
        }))
    };
    let cancel = AtomicBool::new(false);
    let opts = focus::FocusOptions {
        align: std::env::var("FOKUS_AUSRICHTEN").map_or(true, |v| v != "0"),
    };
    let t = Instant::now();
    let out = focus::merge(n, &load, opts, &|_, _, _| {}, &cancel)?;
    let el = t.elapsed();
    let hash = checksum(&out);
    println!(
        "Focus-Serie: {n} Bilder {}×{} in {el:.2?}, Spitzenspeicher {:.0} MB, Prüfsumme {hash:016x}",
        out.width,
        out.height,
        peak_rss_mb().unwrap_or(f64::NAN)
    );
    // Verkleinert ablegen, zum Vergleich zweier Läufe
    let small = out.downscale_to(1500);
    let bytes: Vec<u8> = small
        .data
        .iter()
        .flat_map(|p| p.iter().flat_map(|v| v.to_le_bytes()))
        .collect();
    std::fs::write(dir.join("fokus_serie_1500.f32"), bytes)?;
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let input = PathBuf::from(args.next().expect("Eingabedatei fehlt"));
    let dir = PathBuf::from(args.next().expect("Ausgabeordner fehlt"));
    std::fs::create_dir_all(&dir)?;
    let raw = !matches!(
        input
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "jpg" | "jpeg" | "png" | "tif" | "tiff"
    );
    if let Some(n) = std::env::var("FOKUS_SERIE")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        return focus_series(&input, raw, n, &dir);
    }
    let src = Source::load(&input, raw, None, None)?;
    let img = src.image;
    println!("Vorlage {}×{}", img.width, img.height);
    let cancel = AtomicBool::new(false);
    let quiet = |_: &str, _: usize, _: usize| {};

    // --- HDR ---------------------------------------------------------------------
    {
        // „Wahre“ Szene: so skaliert, dass das hellste Prozent bei 2,0 liegt
        let mut l: Vec<f32> = img.data.iter().step_by(37).map(|p| luma(*p)).collect();
        l.sort_by(|a, b| a.total_cmp(b));
        let k = 2.0 / l[l.len() * 99 / 100].max(1e-3);
        let scene = ImageF32 {
            width: img.width,
            height: img.height,
            data: img.data.iter().map(|p| p.map(|v| v * k)).collect(),
        };
        let shift = [[1.0, 0.0, 6.5], [0.0, 1.0, -4.0], [0.0, 0.0, 1.0]];
        let shots: Vec<ImageF32> = [0.25f32, 1.0, 4.0]
            .iter()
            .enumerate()
            .map(|(s, &ev)| {
                let base = if ev > 2.0 {
                    warp(&scene, &shift)
                } else {
                    scene.clone()
                };
                let data = base
                    .data
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        p.map(|v| {
                            let v = v * ev;
                            let n = (0.0008 + 0.004 * v.max(0.0)).sqrt()
                                * 0.05
                                * noise(i, s as u32 + 1);
                            ((v + n).clamp(0.0, 1.0) * 4095.0).round() / 4095.0
                        })
                    })
                    .collect();
                ImageF32 {
                    width: base.width,
                    height: base.height,
                    data,
                }
            })
            .collect();
        let t = Instant::now();
        let out = hdr::merge(
            3,
            &|i| Ok(shots[i].clone()),
            hdr::HdrOptions::default(),
            &quiet,
            &cancel,
        )?
        .image;
        println!(
            "HDR: {}×{} in {:.2?}, Prüfsumme {:016x}",
            out.width,
            out.height,
            t.elapsed(),
            checksum(&out)
        );
        // Abweichung von der Szene (im Maßstab der Referenz ×1), ohne Rand
        let mut errs: Vec<f32> = Vec::new();
        let mut hi = Vec::new();
        for y in (40..out.height - 40).step_by(7) {
            for x in (40..out.width - 40).step_by(7) {
                let (a, b) = (luma(out.get(x, y)), luma(scene.get(x, y)));
                let sp = scene.get(x, y);
                if b > 0.02 && sp[0].max(sp[1]).max(sp[2]) * 0.25 <= 0.8 {
                    let e = (a / b - 1.0).abs();
                    errs.push(e);
                    if b > 1.2 {
                        hi.push(e);
                    }
                }
            }
        }
        // Verhältnis Ergebnis/Szene je Helligkeitsbereich (systematische Fehler)
        let edges = [0.02f32, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 100.0];
        let mut bins: Vec<Vec<f32>> = vec![vec![]; edges.len() - 1];
        for y in (40..out.height - 40).step_by(5) {
            for x in (40..out.width - 40).step_by(5) {
                let (a, b) = (luma(out.get(x, y)), luma(scene.get(x, y)));
                // Nur, was die dunkelste Aufnahme noch ohne Abschneiden enthält
                let sp = scene.get(x, y);
                if sp[0].max(sp[1]).max(sp[2]) * 0.25 > 0.8 {
                    continue;
                }
                if let Some(k) = edges.windows(2).position(|e| b >= e[0] && b < e[1]) {
                    bins[k].push(a / b);
                }
            }
        }
        let summary: Vec<String> = bins
            .iter_mut()
            .zip(edges.windows(2))
            .filter(|(b, _)| !b.is_empty())
            .map(|(b, e)| {
                b.sort_by(|x, y| x.total_cmp(y));
                format!("{}–{}: {:.3} (n={})", e[0], e[1], b[b.len() / 2], b.len())
            })
            .collect();
        println!("  Ergebnis/Szene: {}", summary.join(", "));
        errs.sort_by(|a, b| a.total_cmp(b));
        hi.sort_by(|a, b| a.total_cmp(b));
        println!(
            "  rel. Fehler: Median {:.2} %, 95 % {:.2} %; Lichter über Weiß ({}): Median {:.2} %",
            100.0 * errs[errs.len() / 2],
            100.0 * errs[errs.len() * 95 / 100],
            hi.len(),
            100.0 * hi.get(hi.len() / 2).copied().unwrap_or(f32::NAN)
        );
        save(&shots[1], &dir, "hdr_eingang_mitte", true)?;
        save(&out, &dir, "hdr", true)?;
    }

    if std::env::var("NUR_HDR").is_ok() {
        return Ok(());
    }
    // --- Panorama ------------------------------------------------------------------
    {
        let fs = img.width as f64 / 2.0; // Vorlage deckt 2 rad ≈ 115° ab
        let (w, h, f) = (2000usize, 1500usize, 1800.0f64);
        let shoot = |yaw: f64, pitch: f64, exposure: f32| -> ImageF32 {
            let (sy, cy) = yaw.sin_cos();
            let (sp, cp) = pitch.sin_cos();
            ImageF32::from_fn(w, h, |x, y| {
                let d = [
                    (x as f64 - (w - 1) as f64 / 2.0) / f,
                    (y as f64 - (h - 1) as f64 / 2.0) / f,
                    1.0,
                ];
                // erst nicken, dann schwenken
                let d = [d[0], cp * d[1] - sp * d[2], sp * d[1] + cp * d[2]];
                let r = [cy * d[0] + sy * d[2], d[1], -sy * d[0] + cy * d[2]];
                let theta = r[0].atan2(r[2]);
                let hh = r[1] / (r[0] * r[0] + r[2] * r[2]).sqrt();
                let u = theta * fs + img.width as f64 / 2.0;
                let v = hh * fs + img.height as f64 / 2.0;
                align::sample(&img, u, v)
                    .unwrap_or([0.0; 3])
                    .map(|v| v * exposure)
            })
        };
        let shots = [
            shoot(-0.5, 0.02, 1.0),
            shoot(0.0, -0.01, 1.15),
            shoot(0.5, 0.015, 0.9),
        ];
        for (projection, name) in [
            (pano::Projection::Cylindrical, "pano_zylinder"),
            (pano::Projection::Spherical, "pano_kugel"),
            (pano::Projection::Perspective, "pano_perspektive"),
        ] {
            let t = Instant::now();
            let opts = pano::PanoOptions {
                projection,
                ..Default::default()
            };
            match pano::merge(
                3,
                &|i| Ok(shots[[1usize, 2, 0][i]].clone()),
                opts,
                &quiet,
                &cancel,
            ) {
                Ok(out) => {
                    let empty = out.data.iter().filter(|p| luma(**p) <= 0.0).count();
                    println!(
                        "Panorama {name}: {}×{} in {:.2?}, {} leere Pixel, Prüfsumme {:016x}",
                        out.width,
                        out.height,
                        t.elapsed(),
                        empty,
                        checksum(&out)
                    );
                    save(&out, &dir, name, raw)?;
                }
                Err(e) => println!("Panorama {name}: {e}"),
            }
        }
        // Erwartet: 1,0 rad + 2·atan(1000/1800) ≈ 2,01 rad → bei f = 1800 ≈ 3620 px breit
        println!(
            "  erwartete Breite (zylindrisch) ≈ {:.0} px",
            (1.0 + 2.0 * (1000.0f64 / 1800.0).atan()) * f
        );
        save(&shots[0], &dir, "pano_eingang_links", raw)?;
    }

    // --- Focus Stacking ------------------------------------------------------------
    {
        let sharp = img.downscale_to(3000);
        let (w, h) = (sharp.width, sharp.height);
        let levels: Vec<ImageF32> = [0usize, 2, 4, 8].iter().map(|&r| blur(&sharp, r)).collect();
        let n = 4;
        let shots: Vec<ImageF32> = (0..n)
            .map(|s| {
                // Schärfezone wandert von links nach rechts; Unschärfe wächst mit Abstand
                let shot = ImageF32::from_fn(w, h, |x, y| {
                    let zone = (x as f32 / w as f32 * n as f32 - 0.5 - s as f32).abs();
                    let b = (zone * 1.6).min(3.0);
                    let (i, t) = (b.floor() as usize, b.fract());
                    let a = levels[i].get(x, y);
                    let c = levels[(i + 1).min(3)].get(x, y);
                    [0, 1, 2].map(|k| a[k] * (1.0 - t) + c[k] * t)
                });
                // Objektiv-Atmen: Maßstab ändert sich leicht, dazu etwas Versatz
                let s = s as f64;
                let k = 1.0 + 0.004 * (s - 1.5);
                let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
                let m = [
                    [k, 0.0, cx * (1.0 - k) + s * 0.8],
                    [0.0, k, cy * (1.0 - k) - s * 0.5],
                    [0.0, 0.0, 1.0],
                ];
                warp(&shot, &align::invert(&m).unwrap())
            })
            .collect();
        let t = Instant::now();
        let out = focus::merge(
            n,
            &|i| Ok(shots[i].clone()),
            focus::FocusOptions::default(),
            &quiet,
            &cancel,
        )?;
        println!(
            "Focus Stacking: {}×{} in {:.2?}, Prüfsumme {:016x}",
            out.width,
            out.height,
            t.elapsed(),
            checksum(&out)
        );
        // Das Ergebnis liegt im Koordinatensystem der Referenz (Nr. 2) –
        // Vergleich mit der ebenso verzerrten scharfen Vorlage
        let k = 1.0 + 0.004 * 0.5;
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
        let m = [
            [k, 0.0, cx * (1.0 - k) + 1.6],
            [0.0, k, cy * (1.0 - k) - 1.0],
            [0.0, 0.0, 1.0],
        ];
        let truth = warp(&sharp, &align::invert(&m).unwrap());
        let err = |a: &ImageF32| {
            let (mut e, mut c) = (0.0f64, 0.0f64);
            for y in (30..h - 30).step_by(3) {
                for x in (30..w - 30).step_by(3) {
                    e += (luma(a.get(x, y)) - luma(truth.get(x, y))).abs() as f64;
                    c += 1.0;
                }
            }
            e / c
        };
        println!(
            "  mittl. Abweichung von der scharfen Vorlage: Ergebnis {:.4}, Einzelbilder {}",
            err(&out),
            shots
                .iter()
                .map(|s| format!("{:.4}", err(s)))
                .collect::<Vec<_>>()
                .join(" / ")
        );
        save(&shots[0], &dir, "fokus_eingang_1", raw)?;
        save(&out, &dir, "fokus", raw)?;
    }
    Ok(())
}
