# Changelog

All notable changes to Lumenkorn are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- English user interface. Lumenkorn follows the system language; **DE / EN** at
  the bottom left of the sidebar switches it. Backend messages, AI keywords
  for newly analyzed photos and the Windows installers (German and English
  MSI) follow too. `npm run i18n` checks that every text is translated.
- People: reassign or remove a person for all selected photos at once
  (info panel → “Personen in der Auswahl”).
- Confirm several suggested faces at once: select faces in the review
  (click, `Shift`-click, `Ctrl+A`) or photos in a person's grid (info panel).
- “‹ Personen” in a person's photo view returns to the people overview, or
  to the face review it was opened from.
- Rename people who already have a name, e.g. “Max” → “Max Mustermann”:
  double-click or right-click the name in the sidebar, or click it under
  “Personen verwalten”. Open suggestions stay open, and an existing name
  merges both people after confirmation.
- Collapsible sidebar sections (Projects, People, Folders), remembered
  across restarts.
- Compare is reachable from everywhere: `C` also in Develop, `Enter`/`E` with
  several photos selected, and the context menu.
- Develop: “Original zeigen” button in the toolbar shows the photo without
  edits (click toggles, press and hold for a quick look; `\` still works).
- Develop: `Tab` moves through the sliders with the value selected, ready to
  type; `Enter` applies, `Esc` reverts, arrow keys nudge (`Shift` ×10).
- Select several folders in the sidebar with `Ctrl`-click (or a range with
  `Shift`-click) and add them to a project at once, or create a new project
  from them, via right-click. `Esc` clears the selection.
- Automatic dependency updates: Dependabot checks weekly; small updates are
  merged after green CI and produce a new nightly MSI, breaking ones stay open
  for review. CI tests rawler against real RAWs from seven cameras.
- Release workflow started by hand builds the installers as a test without
  creating a release; tauri-action updated to v1.
- CI builds a Windows MSI after every green push to `main` and publishes it
  as the rolling “nightly” pre-release.

### Changed
- The window starts maximized; `F11` toggles fullscreen.
- RAW detail: Bayer sensors are demosaiced with RCD instead of PPG (sharper
  fine detail, far fewer maze and false-color artifacts), Fujifilm X-Trans with
  Markesteijn instead of bilinear. RAWs also get capture sharpening at sensor
  resolution (luminance only, halo-limited, threshold adapts to the measured
  noise), as Lightroom applies by default. Thumbnails of already edited RAWs
  are regenerated once after the update.
- Export defaults to an `Export` subfolder next to each original. Folders
  Lumenkorn creates for this are marked (`.lumenkorn-export`) and not added to
  the library.
- Crop opens with the aspect ratio “Original” (or the ratio of an existing crop).
- AI noise reduction is much stronger. The noise estimate now also looks at
  half resolution (camera noise is spatially correlated and was underestimated
  about 2×), color noise gets an extra half-resolution pass, and the strength
  slider maps 0 / 50 / 100 to 0.75× / 1.5× / 3× the estimate (default 50).
  Already denoised photos keep their old result until recomputed.

### Fixed
- Panoramas with a tilted pan kept a wavy horizon: leveling was undone by the
  final centering step.
- AI model downloads failed on slow connections (total time limit); they now
  only stop when no data arrives for 60 s and resume interrupted downloads.
- The GPU preview could show a stale source (e.g. without the latest repair).
- The GPU mask cache discarded the most recently used masks first.
- HDR results took the capture info (exposure, ISO) from the first selected
  photo instead of the middle exposure.
- Semantic search in a folder, project or person found too little: ranking
  now happens within the current scope; deleted photos leave the index.
- Repair, denoise and downloads stayed blocked until restart after an internal
  error.
- Face thumbnails could show another face after faces were deleted.
- Keys pressed while the AI dialog was open changed the photos behind it.
- People view showed a success message after an error; several actions failed
  without any message.
- New folders always appear in the folder tree, also when they are empty or
  contain only unsupported files. Previously a folder only showed up once it
  held a supported photo.
- Scans report skipped files in unsupported formats (e.g. HEIC from phones)
  and unreadable entries; “Neu einlesen” always confirms that it ran.
- `.jfif` / `.jpe` files are recognized as JPEG.
- File-watcher updates also apply when a reported path is spelled differently
  from the source folder (Windows 8.3 short names, symlinks).
- Person view now refreshes after changing assignments, so wrongly assigned
  photos disappear immediately; if a person ends up without photos, the view
  switches to the new person (or to all photos).

## [0.1.0] – 2026-10-02

First public version. The project was developed under the working title
“Lichtwerk”; existing catalogs from that version are taken over automatically
on first start.

### Library
- Source folders with live file watching (only changed paths are re-scanned),
  folder tree, projects, star ratings and pick/reject flags.
- Full-text search (SQLite FTS5 with trigrams) across file names, folders,
  camera, lens, keywords and people.
- Fast thumbnails from embedded RAW previews; edited photos show their edit.
- Culling: `X` rejects, `Shift+X` rejects and advances, rejected photos can be
  moved to the trash or deleted permanently.
- Side-by-side compare and 1:1 zoom.

### Develop (non-destructive)
- Linear RAW pipeline (rawler) with white balance, exposure, tone, clarity,
  color, HSL with eyedropper, sharpening, crop and straighten with automatic
  horizon detection.
- Local adjustments: linear and radial gradients, brush, color range with
  eyedropper, and AI masks (subject, sky, object).
- GPU processing via wgpu (Vulkan, DirectX 12, Metal) with CPU fallback.
- Copy/paste of basic settings with `Ctrl+Shift+C` / `Ctrl+Shift+V` or the
  context menu.
- Export to JPEG, 16-bit TIFF and PNG.

### Local AI (optional download, runs offline)
- Automatic keywords and semantic search (CLIP).
- AI noise reduction (DRUNet), AI repair / object removal (LaMa).
- Face detection and grouping into people (YuNet, SFace).

### Merge
- HDR from exposure brackets (alignment, measured exposure ratios, de-ghosting,
  chained alignment for wide brackets up to ±6 EV).
- Panorama (any shot order, multi-row, cylindrical / spherical / perspective
  projection, exposure compensation, auto-crop).
- Focus stacking (alignment including focus breathing, Laplacian pyramid fusion).
- Results are stored as linear 16-bit OpenEXR next to the source photos and
  develop like RAW files.
