# Changelog

All notable changes to Lumenkorn are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed
- Masks: “Person”, “Animal”, “Object” (click) and the sky refinement no
  longer come out squeezed and shifted towards the bottom left in portrait
  photos and any aspect ratio other than 3:2 landscape. The Segment Anything
  export had the crop for 3:2 baked in; Lumenkorn now crops the mask itself.
- Masks: “Person” and “Animal” leave a small margin around the detected box,
  so feet and tails are no longer cut off at the box edge.
- Masks: the cached Segment Anything analysis is redone after switching lens
  correction on or off, so clicks match the shown geometry.
- Library: a subfolder that cannot be read for a moment (e.g. on a NAS) no
  longer marks all photos in it as missing.
- People: rejecting a face twice no longer forgets the first “not this
  person”.
- Develop: switching photos back and forth quickly could open the wrong
  photo; AI mask results arriving after a photo switch went to the wrong
  layer.
- Updates: a failed download can be retried without restarting the app.
- AI agents (MCP): the HTTP server accepts only exact local origins.
- Deleting rejected photos: a file on a disconnected drive or network share
  no longer counts as “already deleted”, so the photo and its edits stay.
- Thumbnails: a file that crashes the decoder is marked as failed instead
  of stopping thumbnails for every photo after it.
- A crash inside one task no longer leaves the catalog locked until the
  app is restarted.
- Library: two scans adding the same new file at once (e.g. after an HDR
  merge while the folder is watched) no longer drop the whole batch.
- AI downloads: truncated model files are rejected instead of being kept as
  broken models; the ONNX Runtime archive is checked against its SHA-256.
- Starting Lumenkorn a second time brings the open window to the front
  instead of opening the same catalog twice.
- A catalog from a newer Lumenkorn version is not opened (and not changed);
  errors opening the catalog are shown instead of quitting silently.
- Develop: a photo changed in another program is reloaded instead of
  showing the old content until it drops out of memory.
- Lens correction: when a Sony lens reports colour fringing but no
  distortion, the camera's fringe correction is kept together with the
  Lensfun distortion profile instead of being replaced.
- Folder watching: constant writing in a source folder (tethering, cloud
  sync) no longer delays new photos forever; changes are picked up at least
  every 15 seconds.
- Develop: switching photos could briefly show the preview of the previous
  photo's settings.

### Changed
- Portrait RAWs open faster and need less memory: they are scaled down
  before being turned upright, and lens data is read from the file header
  instead of the whole file.
- Zooming in Develop no longer blocks other requests while the full
  resolution is loading.
- Folder watching only looks up the photos in the reported folders instead
  of the whole source folder on every change.
- Export needs less memory: no extra full-size copy before lens correction
  or of the prepared image; lens profiles with only vignetting are applied
  without resampling. Only one export runs at a time.
- HDR and panorama keep at most half of the free memory for cached frames on
  Windows too (before: a fixed 3 GB); focus stacking keeps its reference in
  half precision.
- Person and animal masks, face parts and hair edges use all CPU cores;
  Segment Anything no longer copies the photo analysis on every click.
- Large libraries: rating or flagging no longer recalculates the layout of
  every photo in the grid and filmstrip; reloads triggered in the background
  are bundled, and a reload can no longer undo a rating made a moment ago.

## [1.0.1] – 2026-10-11

### Added
- AI agents (MCP): a built-in Model Context Protocol server lets Claude,
  ChatGPT/Codex and other agents search, view, rate, tag, develop, mask and
  export photos. Off by default (AI → AI agents); stdio via `lumenkorn --mcp`
  (starts the app if needed) and local HTTP with an access key. No tool
  deletes anything; changes show up live and can be undone in Develop.
- Updates: shortly after starting, Lumenkorn checks once for a newer release
  on GitHub, shows what changed and asks. “Install now” downloads it, checks
  its signature, installs and restarts; “Later” asks again at the next start.
- Masks: “Person” selects the whole person or one part (facial skin, hair,
  eyes, iris, eyebrows, lips, body skin, clothes) of everyone in the photo;
  “Animal” selects all animals, birds, mammals or reptiles. Uses OWLv2 to
  find them, MediaPipe Selfie Multiclass and Face Mesh for the parts and
  Segment Anything for outlines (all Apache 2.0, downloaded with the AI
  masks; about 177 MB more).

### Changed
- Develop: the tone sliders now split the range like in Lightroom.
  Highlights and Shadows work on the bright and dark half of the image,
  reaching into the midtones; Whites and Blacks only move the ends of the
  scale (white and black point) and leave the midtones nearly untouched.
  Before, Highlights reached only the brightest areas while Whites lifted
  almost the whole image. Existing edits that use these sliders look
  different; their thumbnails are rebuilt once.
- Auto: sets Whites and Blacks before Contrast and goes only part of the way
  to the targets, so the values depend on the photo instead of sticking to
  the limits (Whites +70, Blacks −50, Contrast +25).

### Fixed
- After a restart the large view could show an older, unedited preview
  from the WebView cache while the filmstrip showed the edit.

## [1.0.0] – 2026-10-11

### Added
- Develop: “Schwarzweiß” next to Auto (`V`) turns the photo black and white.
  The conversion runs after HSL and the local adjustments, so the HSL
  luminance sliders work as the gray mix (darker blue sky, brighter skin)
  and masks can't tint it again. Color grading applies afterwards and tones
  the result (split toning, sepia). Presets and copied settings carry it in
  the color group.
- Develop: “Auto” at the top of the Basis tab (`Ctrl+U`) sets exposure,
  contrast, highlights, shadows, whites, blacks and vibrance so the photo
  looks balanced. It develops a small copy with the real pipeline and brings
  each slider in turn to a target (median brightness, no new clipping,
  visible shadows, full tonal range), so it fits the base curve, lens
  correction and crop. Night shots and scenes with large bright areas (snow,
  sky) are brightened only a little. `Ctrl+Z` undoes it.
- People: groups such as “Familie” or “Freunde”. “+ Gruppe” in the people
  overview creates one; each group is a collapsible section like “Unbenannt”
  (remembered across restarts), with its people inside. Drag a person's card
  onto a group, or use the folder icon on the card, to assign it; named
  people without a group are collected under the collapsible “Sonstige”, all
  unnamed groups under “Unbenannt”. Groups can be renamed (double-click) and
  deleted without losing the people. The sidebar lists people by group too,
  each group collapsible on its own; drag a person onto a group there, or
  onto “Sonstige” to take it out. Catalog update to version 14.
- Develop: lens correction for RAW files, on by default (“Objektiv” in the
  Basis tab, below Color Grading). It removes distortion, color fringes and
  vignetting using the correction data the camera writes into the RAW (Sony
  ARW, DNG opcodes) or a profile from the bundled Lensfun database (about
  1,500 lenses, CC BY-SA 3.0). Distortion and color fringes come preferably
  from the camera, vignetting preferably from Lensfun (Sony's own data only
  compensates part of it). The panel shows where the data comes from; the
  image is enlarged just enough to avoid empty borders. Masks, repairs and
  the horizon detection work on the corrected image. Older edits with brush
  or AI masks keep the correction off so the masks do not shift. Thumbnails of
  edited RAWs are regenerated once (catalog version 13).
- Presets: save settings under a name and apply them to photos, from the
  right-click menu (grid, filmstrip, single view, Develop) → “Preset
  anwenden”. In Develop, hovering a preset in the menu shows its effect on
  the open photo; a click applies it (`Ctrl+Z` undoes it), also to all
  selected photos at once. “Neues Preset aus diesem Foto …” saves the edit of
  the clicked photo and lets you choose what belongs to it: white balance,
  light, color, HSL, color grading, details, masks (without AI masks) and
  crop/rotate. “Presets verwalten …” renames and deletes them. Catalog update
  to version 12.
- AI sky detection: “Himmel” uses the open-source sky model skyseg (U²-Net,
  MIT license), downloaded with the AI masks (about 176 MB more). It also
  finds dark cloud banks, haze and dusk skies and does not take walls or
  foliage for sky; uncertain edges (tree tops, branches) follow the colors of
  the photo. Without the model the previous detection is used. After the
  update, “KI-Masken herunterladen” in the AI dialog loads only the missing
  sky model.
- Develop: Color Grading below HSL, as in Lightroom – color wheels for
  shadows, midtones, highlights and global (drag for hue and saturation,
  double-click resets), luminance per range, blending and balance. Tints
  never brighten or darken; black and white stay neutral.
- Develop: “Dunst entfernen” (Dehaze) in Details, below Sharpening.
  Removes haze with a dark-channel estimate but a neutral haze color, so
  colors keep their hue instead of drifting blue or yellow; the lost
  brightness is given back in the midtones. Negative values add haze.
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
- Develop: “Auf Original zurücksetzen …” at the bottom of the Basis tab
  removes all edits of the photo (sliders, masks, repairs, noise reduction,
  crop) after a warning; snapshots stay, `Ctrl+Z` undoes it. It replaces the
  “Zurücksetzen” button above Export, which did the same without asking.
- Compare shows the filmstrip too. Clicking a photo there replaces the
  highlighted one; `Ctrl`/`Shift`-click adds or removes a photo.
- The filmstrip (single view, Compare, Develop) scrolls through all photos,
  not just the 30 before and after the current one; the mouse wheel scrolls
  it sideways.
- Develop: HSL moved below Details and noise reduction.
- People: all unnamed groups are collected under a collapsed “Unbenannt”
  (Unnamed) section, largest first, so the overview stays short; single faces
  are shown from there on request.
- People: the name of an opened person or group (face review and photo view)
  can be changed by clicking it; entering an existing name assigns the group
  to that person.
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
- Performance, measured with 50 000 photos and 24 MP images:
  - **Library:** first face analysis ~9× faster per photo; saving AI results
    ~4× faster; project filters and the project list ~10× faster; naming or
    assigning people ~30× faster. Unedited JPEGs open at full size instantly
    (the original file is shown instead of being re-encoded).
  - **Develop:** brush masks up to ~40× faster; RAWs load ~30 % faster with
    ~40 % less memory; local adjustments about twice as fast.
  - **Merge:** focus stacking needs about half the memory, 10–25 % faster;
    HDR loads each photo at most twice.
  - **AI:** faster semantic search; noise reduction needs half the memory.
  - **Interface:** the info panel no longer reloads on every rating, and
    compare loads at most two full-size photos at a time.

  Catalog update to version 11 runs automatically on first start.
- Code cleanup: duplicated logic merged, unused code removed, comments brought
  up to date. No visible changes; leftovers of the withdrawn DirectML backend
  are gone.

### Fixed
- People: the face review of a person or group has “‹ Personen” at the top
  left, like a person's photos (it showed “‹ Raster” there plus a second back
  button). `Esc` also closes the review.
- Sky detection selected far too much (walls, grass, foliage, studio
  backdrops) – and after a first fix found hardly any sky. Each area must now
  look like sky as a whole: clearly blue (also with clouds, haze or at dusk),
  or bright grey with a level horizon below it. Clouds and strips of sky that
  do not reach the top edge join the sky they touch, unless they lie below
  it (rocks, snowfields, buildings). SAM only refines the edge. The log
  records why an area was or was not taken as sky. Muted dusk skies (grey-blue
  above, yellowish-grey below, with contrails) are found too.
  Existing sky masks keep their old result until created again.
- Choosing another folder, project or person in Develop no longer jumps back
  to the Library: the open photo stays if it is part of the new view,
  otherwise the first photo there opens.
- Very large photos got no thumbnail and could not be edited (“Memory limit
  exceeded”): panoramas, upscaled JPEGs, 16-bit TIFFs from about 30 MP and
  32-bit float TIFFs from astro stacking. Huge images are decoded one at a
  time, and the editing preview averages them down while converting instead
  of first building a full-size float copy (420 MP panorama: 1.3 GB instead
  of about 6 GB). Failed thumbnails are retried automatically on the next
  start.
- Crash during AI noise reduction (or other AI functions) on the GPU while
  face recognition ran in the background: the WebGPU runtime does not allow
  two models to compute at the same time, so GPU runs now take turns.
- A project containing a drive root (e.g. `D:\`) showed no photos.
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
