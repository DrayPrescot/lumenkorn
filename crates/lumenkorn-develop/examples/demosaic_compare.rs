//! Demosaicing-Vergleich mit Referenz: Ein RAW wird entwickelt und 3× verkleinert
//! (rauscharm, alle Farben je Pixel) – das ist die „Wahrheit“. Daraus wird ein
//! RGGB-Mosaik gebildet und mit PPG (rawler) bzw. RCD zurückgerechnet.
//!
//!     cargo run --release -p lumenkorn-develop --example demosaic_compare -- <raw> <ausgabe.png>

use lumenkorn_develop::demosaic::rcd;
use lumenkorn_develop::source::{DemosaicMethod, decode_raw_with};
use rawler::imgop::sensor::Demosaic;
use std::path::Path;

fn enc(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn psnr(a: &[[f32; 3]], b: &[[f32; 3]], w: usize, h: usize, m: usize) -> f64 {
    let mut se = 0f64;
    let mut n = 0usize;
    for y in m..h - m {
        for x in m..w - m {
            let (p, q) = (a[y * w + x], b[y * w + x]);
            for c in 0..3 {
                let d = (enc(p[c]) - enc(q[c])) as f64;
                se += d * d;
                n += 1;
            }
        }
    }
    10.0 * (1.0 / (se / n as f64)).log10()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("RAW fehlt");
    let output = args.next().expect("Ausgabe fehlt");
    let raster = matches!(
        Path::new(&input).extension().and_then(|e| e.to_str()),
        Some("png" | "jpg" | "jpeg")
    );
    // Rasterbild (sauberes Foto): direkt als Wahrheit, linearisiert
    let (w, h, truth) = if raster {
        let im = image::open(&input).unwrap().to_rgb8();
        let dec = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let (w, h) = (im.width() as usize, im.height() as usize);
        (
            w,
            h,
            im.pixels()
                .map(|p| [dec(p[0]), dec(p[1]), dec(p[2])])
                .collect::<Vec<_>>(),
        )
    } else {
        raw_truth(&input)
    };
    run(&input, w, h, truth, &output);
}

fn raw_truth(input: &str) -> (usize, usize, Vec<[f32; 3]>) {
    let (img, _) = decode_raw_with(Path::new(input), DemosaicMethod::Ppg).unwrap();
    // Belichtung normieren (Mittelwert ~0,18), dann 3×3-Box verkleinern
    let mean: f32 = img.data.iter().map(|p| p[1]).sum::<f32>() / img.data.len() as f32;
    let gain = 0.18 / mean.max(1e-6);
    let (w, h) = (img.width / 3, img.height / 3);
    let truth: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w * 3, i / w * 3);
            let mut s = [0f32; 3];
            for dy in 0..3 {
                for dx in 0..3 {
                    let p = img.data[(y + dy) * img.width + x + dx];
                    for c in 0..3 {
                        s[c] += p[c] * gain / 9.0;
                    }
                }
            }
            s.map(|v| v.min(1.0))
        })
        .collect();
    (w, h, truth)
}

fn run(input: &str, w: usize, h: usize, truth: Vec<[f32; 3]>, output: &str) {
    // RGGB-Mosaik
    let pattern = [[0usize, 1], [1, 2]];
    let mosaic: Vec<f32> = (0..w * h)
        .map(|i| truth[i][pattern[(i / w) & 1][(i % w) & 1]])
        .collect();

    let t = std::time::Instant::now();
    let ours = rcd(&mosaic, w, (0, 0, w, h), pattern);
    let t_rcd = t.elapsed();
    let cfa = rawler::CFA::new("RGGB");
    let pix = rawler::pixarray::PixF32::new_with(mosaic.clone(), w, h);
    let t = std::time::Instant::now();
    let ppg = rawler::imgop::sensor::bayer::ppg::PPGDemosaic::new().demosaic(
        &pix,
        &cfa,
        &rawler::cfa::PlaneColor::new("RGB"),
        pix.rect(),
    );
    let t_ppg = t.elapsed();
    println!(
        "{}: {w}×{h}  PPG {:.2} dB ({:.0?})  RCD {:.2} dB ({:.0?})",
        Path::new(input).file_name().unwrap().to_string_lossy(),
        psnr(&ppg.data, &truth, w, h, 4),
        t_ppg,
        psnr(&ours, &truth, w, h, 4),
        t_rcd
    );

    // Ausschnitt der Bildmitte: Wahrheit | PPG | RCD, 2× vergrößert
    let (cw, ch) = (220usize.min(w), 220usize.min(h));
    let (x0, y0) = ((w - cw) / 2, (h - ch) / 2);
    let mut canvas = image::RgbImage::new((cw * 3 * 2) as u32, (ch * 2) as u32);
    for (k, src) in [&truth, &ppg.data, &ours].iter().enumerate() {
        for y in 0..ch * 2 {
            for x in 0..cw * 2 {
                let p = src[(y0 + y / 2) * w + x0 + x / 2];
                canvas.put_pixel(
                    (k * cw * 2 + x) as u32,
                    y as u32,
                    image::Rgb(p.map(|v| (enc(v) * 255.0 + 0.5) as u8)),
                );
            }
        }
    }
    canvas.save(output).unwrap();
}
