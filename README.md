<div align="center">

<img src="src-tauri/icons/128x128.png" width="96" alt="Lumenkorn icon">

# Lumenkorn

**A local, privacy-friendly photo manager and RAW developer — an open-source alternative to Lightroom.**

[![CI](https://github.com/DrayPrescot/lumenkorn/actions/workflows/ci.yml/badge.svg)](https://github.com/DrayPrescot/lumenkorn/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20Linux-lightgrey)

[Features](#features) · [Getting started](#getting-started) · [Building](#building-from-source) · [Architecture](#architecture) · [Deutsch](docs/README.de.md)

<img src="docs/screenshots/develop.png" alt="Develop view with a RAW photo" width="900">

</div>

Lumenkorn manages your photo folders in place, develops RAW files non-destructively and
runs all of its AI features on your own machine. It has no cloud, no account and no
subscription. It is built with a Rust core, a Svelte 5 interface and [Tauri 2](https://tauri.app),
and uses the GPU (AMD, NVIDIA, Intel) through Vulkan or DirectX 12 where available.

> [!NOTE]
> **Early stage.** Lumenkorn is under active development (version 0.1).
> The user interface is currently **German only**; an English translation is planned.
> Back up your photos as you would with any new tool. Lumenkorn never modifies your originals.

## Features

### Library
- **Folders instead of imports.** Add a folder and Lumenkorn indexes it. Nothing is copied or moved.
- **Live sync.** File watching picks up new, changed, renamed and moved files.
  Moves are recognized by content hash, so ratings and edits stay with the photo.
- **Projects** group any number of folders, even across drives.
- **Fast thumbnails** for JPEG, PNG, TIFF, WebP and RAW (NEF, CR2, CR3, ARW, RAF, DNG, ORF, RW2, …).
  For unedited RAW files Lumenkorn uses the camera's embedded preview.
- **Search** by file name, folder, camera, lens, keywords and people, backed by a
  full-text index (SQLite FTS5), or by image content with local AI.
- **Culling:** `X` rejects a photo, `Shift+X` rejects it and moves to the next.
  Rejected photos can then be moved to the trash in one step.
- **Compare** up to 12 photos side by side with linked zoom, and view any photo at 1:1.
- **People:** confirm many suggested faces at once (click to select, `Shift`-click for a range,
  `Ctrl+A` for all). "‹ Personen" in a person's photo view leads back to the people overview.
  Fix assignments for many photos at once. Select them and reassign or remove
  a person in the info panel. To rename a person (e.g. "Max" → "Max Mustermann"), double-click
  or right-click their name in the sidebar.
- **Collapsible sidebar:** click the *Projekte*, *Personen* or *Ordner* heading to collapse
  that section.

### Develop (non-destructive)
- **Linear floating-point RAW pipeline** ([rawler](https://github.com/dnglab/dnglab)) with:
  - white balance and exposure;
  - highlights, shadows, whites and blacks;
  - contrast, clarity, vibrance and saturation;
  - sharpening.
- **HSL** for eight color ranges, computed in Oklch. An **eyedropper** adjusts the color
  you click on.
- **Crop and straighten**, with automatic horizon detection.
- **Local adjustments**:
  - linear and radial gradients;
  - brush;
  - color range with eyedropper;
  - AI masks for **subject**, **sky** and **object** (click to select).
- **Before/after:** "Original zeigen" in the toolbar (click toggles, press and hold for a quick look) or `\`.
- **Keyboard entry:** `Tab` moves from slider to slider with the value selected, ready to type;
  arrow keys nudge it (`Shift` ×10).
- **History, snapshots**, and **copy/paste of basic settings** (`Ctrl+Shift+C` / `Ctrl+Shift+V`).
- **Export** to JPEG, 16-bit TIFF or PNG.
- **GPU processing** with [wgpu](https://wgpu.rs). A test checks that GPU and CPU produce
  matching pixels, and Lumenkorn falls back to the CPU automatically when needed.

### Local AI (optional, runs offline after a one-time download)
- **Automatic keywords** and **semantic search** ("dog on the beach") with CLIP.
- **AI noise reduction** with DRUNet, with a noise estimate on two scales and an extra pass
  against color blotches. Lumenkorn downloads the original weights and converts
  them to ONNX locally.
- **AI repair** removes objects, dust spots, power lines and other distractions (LaMa).
- **People:** finds faces, groups them and suggests names (YuNet and SFace). The models used
  are licensed for commercial use.

### Merge
<img src="docs/screenshots/merge.png" alt="Panorama merge dialog" width="560" align="right">

Select photos, right-click, and choose **Merge**:

- **HDR from exposure brackets** (`Ctrl+H`)
  - Aligns handheld brackets.
  - Measures the exposure ratios from the pixels, so EXIF data is not needed.
  - Removes ghosting from moving subjects.
  - Handles brackets of up to ±6 EV.
- **Panorama** (`Ctrl+M`)
  - Works with any shot order, including multiple rows.
  - Projections: cylindrical, spherical or perspective.
  - Compensates exposure differences and crops automatically.
- **Focus stacking** (`Ctrl+Shift+F`)
  - Aligns frames, correcting for focus breathing.
  - Fuses them with Laplacian pyramids.

Results are written as linear 16-bit **OpenEXR** files next to the originals and can be
developed like a RAW file, with recoverable highlights.

<br clear="right">

## Getting started

### Download
Installers for Windows (`.msi` / `.exe`) and Linux (`.deb`, `.AppImage`) are published on the
[Releases](https://github.com/DrayPrescot/lumenkorn/releases) page once a version is tagged.

### First steps
1. Click **+** next to *Ordner* (folders) and choose a folder with photos.
2. Rate with `0`–`5`, pick or reject with `P` / `X`, and press `D` to develop.
3. Optional: open **KI einrichten** (bottom left) to download the AI models.
   Everything else works without them.

### Keyboard shortcuts

| Key | Action |
|---|---|
| Arrow keys | Navigate |
| `Enter` / `E` | Show large (several selected: side by side) |
| `D` / `G` | Develop / back to grid |
| `0`–`5` | Rating |
| `P` / `X` / `U` | Pick / reject / clear flag |
| `Shift+X` | Reject and go to the next photo |
| `Ctrl+Del` | Delete rejected photos (with confirmation) |
| `C` | Compare selected photos (also in Develop and via right-click) |
| `Z`, mouse wheel | Zoom (fit ↔ 100 %) |
| `\` | Before / after |
| `R` | Crop |
| `[` / `]` | Rotate left / right |
| `Ctrl+Z` / `Ctrl+Shift+Z` | Undo / redo |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy / paste basic settings |
| `Ctrl+Shift+E` | Export selection |
| `Ctrl+H` / `Ctrl+M` / `Ctrl+Shift+F` | Merge to HDR / panorama / focus stack |

## Building from source

**Requirements**
- Rust 1.90 or newer (stable).
- Node.js 20 or newer.
- The [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform:
  - **Windows:** Microsoft C++ Build Tools and WebView2. WebView2 is usually preinstalled on Windows 10 and 11.
  - **Debian/Ubuntu:**
    ```bash
    sudo apt install libwebkit2gtk-4.1-dev build-essential libxdo-dev libssl-dev \
      libayatana-appindicator3-dev librsvg2-dev
    ```

**Commands**
```bash
npm install
npm run tauri dev          # run the app with hot reload
npm run tauri build        # build installers
npm run dev                # UI only, in the browser, with demo data

cargo test --workspace     # all tests
cargo run --release -p lumenkorn-develop --example bench    # CPU vs. GPU benchmark
```

GPU tests on machines without a GPU need Mesa's software Vulkan driver (lavapipe) and
`LUMENKORN_GPU_SOFTWARE=1`.

Some tests use real model files or photos and are skipped unless the matching
environment variables (`LUMENKORN_TEST_*`) are set. The
[German documentation](docs/README.de.md#entwickeln-am-code) lists them all.

## Architecture

```
lumenkorn/
├── crates/lumenkorn-core/      catalog (SQLite), folder scanner, file watcher, thumbnails, people
├── crates/lumenkorn-develop/   RAW decoding and the editing pipeline (CPU + wgpu compute shaders),
│                               masks, repair patches, merge (HDR, panorama, focus stacking), export
├── crates/lumenkorn-ai/        ONNX Runtime (loaded dynamically): CLIP, segmentation, DRUNet,
│                               LaMa, face detection/recognition, model downloads
├── src-tauri/                  Tauri 2 shell: commands, background worker, image protocol
├── src/                        Svelte 5 user interface
└── docs/                       German documentation, screenshots
```

The three crates do not depend on Tauri, so everything can be tested without a GUI.

### Your data
- **Catalog, edits and AI data** live in the app data folder. Lumenkorn writes to your photo folders only
  when you ask it to: deleting rejected photos, saving merge results (`*.exr`) next to their sources,
  or exporting into a folder you choose.
  - Windows: `%APPDATA%\io.github.drayprescot.lumenkorn\`
  - Linux: `~/.local/share/io.github.drayprescot.lumenkorn/`
- **Thumbnails** are stored in the system cache folder.
- **Network access** happens only when you choose to download the AI runtime or models. Downloads
  are verified by SHA-256 where the source publishes fixed files.

## Roadmap
- English user interface (internationalization).
- XMP sidecars, hierarchical keywords, and a multilingual text model for search.
- Luminance range masks, and transferring masks between photos.
- Further GPU tuning based on measurements on real hardware.

See the [changelog](CHANGELOG.md) for what is already included.

## Contributing
Issues and pull requests are welcome. Please run these before submitting:
```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run check
```

## License

Lumenkorn is dual-licensed under either of

- [MIT license](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option. Unless you explicitly state otherwise, any contribution you submit for
inclusion in this project is dual-licensed in the same way, without any additional terms.

**Third-party components.**
- **RAW decoding:** Lumenkorn uses [rawler](https://github.com/dnglab/dnglab), licensed **LGPL-2.1**.
  Release binaries link rawler statically. To use a modified rawler, change the `rawler`
  dependency in `crates/lumenkorn-develop/Cargo.toml` and build from source as described above.
- **AI models** are downloaded at runtime, not distributed with Lumenkorn:
  - CLIP (MIT);
  - IS-Net (Apache-2.0);
  - Segment Anything (Apache-2.0);
  - DRUNet/KAIR (MIT);
  - LaMa (Apache-2.0);
  - YuNet (MIT);
  - SFace (Apache-2.0);
  - ONNX Runtime (MIT).
