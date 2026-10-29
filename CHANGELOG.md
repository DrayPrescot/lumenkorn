# Changelog

All notable changes to Lumenkorn are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [1.1.0] – 2026-10-29

### Changed
- With the optional BioCLIP 2 download, animal keywords now cover about 10,000 species instead of
  300: the species most observed on iNaturalist in Europe, birds worldwide, all primates, and the
  mammals, birds and reptiles of Kenya, Tanzania and South Africa. German and English names come
  from Wikidata. Species that are common in their region get a small head start, so a grey heron
  is not named as a great blue heron. The list (about 9 MB) is downloaded together with the model; if BioCLIP 2 is
  already installed, Settings → AI offers the missing file. Animal keywords of photos analysed
  before are determined again in the background. Without BioCLIP 2 the built-in list of about
  300 animals stays.
- Settings → AI: sections in a new order (subject recognition, animal recognition, people, noise
  reduction, masks); animal recognition shows how many photos it has analysed, like the others.
- Settings: the culling assistant has its own page "Cull", and the data folder moved from
  "General" to "Folders".

## [1.0.29] – 2026-10-28

### Added
- AI keywords name animals from coarse to fine, e.g. Bird → Tit → Blue tit or Insect → Beetle →
  Ladybird (about 300 animals, German and English), and stop at the level they are sure about.
  CLIP checks whether an animal is in the photo. For the species there is an optional download
  under Settings → AI: BioCLIP 2 (about 610 MB, MIT licence), which is trained on animals and
  plants and also names small birds or insects far in the frame; without it, CLIP names the
  species and often stops at "Bird". Photos analysed before get the animal keywords in the
  background; only animal keywords are replaced, everything else stays. Search by content
  understands the animal names too.
- A book icon next to projects that have a photo album; clicking it opens the album.
- People: each group shows at most 200 people per page, with page buttons beyond that.

### Changed
- Updated getrandom to 0.4 and rusqlite to 0.41 (bundled SQLite 3.54).

## [1.0.28] – 2026-10-28

### Added
- Nextcloud upload: choose between the project's exports and all of its photos as originals
  (RAWs included, rejected photos left out). The choice is remembered per project.
- Photo album: besides Instrument Sans and Lora, every font installed on the system can be used
  for captions and texts. An album whose font is missing on another computer shows it as
  "not installed" and falls back to a standard font there.
- Settings → General: a limit for the preview cache on disk (5 GB by default). Previews not
  viewed for the longest time are deleted and rendered again when needed.

### Changed
- Long sessions use less memory: the search index for "search by content" is released after
  10 minutes without use, import previews are freed when the dialog closes, and the GPU only
  reserves as much readback memory as a render needs.
- Dragging the noise reduction amount with lens correction or repairs active no longer
  re-corrects the whole photo on every step; the corrected photo and its denoised version are
  kept and only mixed.
- Re-exporting over existing files no longer scans the whole catalog for every file (new index).
- Settings files are written atomically everywhere; a damaged file is logged and kept as `.bad`
  instead of silently falling back to defaults.

## [1.0.27] – 2026-10-28

### Added
- Culling assistant (right-click a folder or project → "Aussortieren …"): suggests photos to reject. That covers the
  weaker shots of a burst (the best 3 to 5 stay, depending on its size), blurry photos, closed
  eyes, badly exposed photos and photos taken by accident. You go through the suggestions and
  borderline cases; only what you tick is flagged as rejected, nothing is deleted. Photos with
  stars, a pick flag or edits are never suggested. A strictness switch (cautious, normal, strict)
  takes effect right away. The assistant learns from your decisions, including rejections you
  take back later, and can be reset under Settings → AI.

### Fixed
- Linux: the Nextcloud app password is now kept in the keyring (Secret Service: GNOME Keyring,
  KWallet, KeePassXC) and survives logging out and restarting. Before, it was only held in the
  session's kernel keyring and you had to log in again after every restart.

## [1.0.26] – 2026-10-28

### Added
- Leave subfolders out of the library, per folder: right-click a folder → "Unterordner
  einbeziehen". The default (include subfolders) can be changed under Settings → Folders. Photos
  in excluded folders disappear from the library, but their ratings and edits are kept and come
  back when the subfolders are included again.
- Projects shared in Nextcloud show a cloud icon in the sidebar; its tooltip shows the link and a
  click opens the Nextcloud dialog.

## [1.0.25] – 2026-10-28

### Added
- Nextcloud: right-click a project → "Zu Nextcloud hochladen …" uploads the project's exported
  photos to a folder in your Nextcloud and shares it as a public link, optionally with a password
  and an expiry date. Uploading again syncs: new and changed files go up, unchanged ones are
  skipped, the link stays the same, and files no longer in the project can be removed. You connect
  under Settings → Nextcloud by logging in in the browser; Lumenkorn gets its own app password,
  kept in the Windows Credential Manager. Exports are now remembered in the catalog, so files
  exported to a fixed folder count too.

## [1.0.24] – 2026-10-28

### Added
- Perspective correction in the crop tab: vertical and horizontal sliders straighten converging
  lines, for example buildings photographed from below. Lumenkorn zooms in just enough to avoid
  empty corners, as with straightening.
- Guide lines ("Lines" next to straighten): drag up to four lines along edges that should be
  vertical or horizontal. One line straightens the photo; two vertical lines also set the
  vertical perspective (converging verticals), two horizontal ones the horizontal perspective.
  The lines stay on the photo while you adjust; right-click removes one.
- Edit metadata in the info panel: title, description, author, copyright, capture time (set or
  shift), camera, lens, focal length, aperture, exposure time and ISO, for one or many photos.
  Changes are stored in the catalog and in an XMP sidecar next to the photo; existing sidecars
  from Lightroom or darktable are updated, not replaced. The photo file itself is not changed.
  Each field can be reset to the value from the file. Title, description and author are
  searchable.
- Exported files (JPEG, TIFF, PNG and DNG) carry the photo's metadata: EXIF (camera, lens, exposure, aperture,
  ISO, focal length, capture time, author, copyright, description) and XMP with title, your own
  keywords and the rating. Edited values win over those from the file. AI keywords and the
  location are not written.
- The export subfolder next to the original can be set in Settings → Folders, for example
  `Export/JPEG` or `../Exports` (default `Export`), or exports can go into the original's own
  folder. It also applies to exports by AI agents.
- Batch rename (`F2` or right-click → "Umbenennen …"): a name pattern with date, time, number,
  camera, lens, folder and the old name, with a preview of every new name. Conflicts are shown
  before anything happens, existing files are never overwritten, XMP sidecars move along, and
  RAW+JPEG pairs keep a shared name.

### Fixed
- GPU editing works on systems that run DirectX 12 instead of Vulkan: the RAW tone mapping
  shader no longer fails to compile there. If a driver cannot compile the shaders, Lumenkorn now
  notices at startup and uses the CPU right away.
- CI runs again: a mask limit test no longer allocates about 12 GB.
- Lens correction for cameras with a built-in lens (bridge and compact cameras such as the
  Panasonic FZ1000, FZ2000/2500, FZ200/300/330, TZ/ZS and LX series): Lensfun lists their lens
  under the camera's own mount, a name that never appears in the photo's metadata, so no profile
  was found. Lumenkorn now uses the single lens of that camera.
- Distortion correction for all Panasonic cameras from the data the camera writes into every
  RW2 file, including models Lensfun does not know (FZ80/FZ82, TZ95 and newer). It also replaces
  Lensfun's distortion profile on Lumix system cameras: checked against the camera's own JPEGs it
  is clearly closer (on a strongly distorted G9 photo 0.4 px instead of 2 px off).
- The clipping percentages under the histogram count the whole preview. A reduced copy averaged
  small blown highlights away and showed 0 % where 0.2 % of the photo was clipped.

## [1.0.23] – 2026-10-27

### Changed
- Smoother work with large catalogs and masks:
  - Thumbnail generation no longer holds the catalog lock while writing files, so the library
    stays responsive during imports.
  - Develop serializes the settings once per change instead of about eight times, and the undo
    history is stored compactly. Brush painting no longer gets slower as strokes pile up.
  - Saving in Develop renders its thumbnails on one background thread, newest state wins,
    instead of a new thread per save competing with the live preview.
  - On network drives, Develop checks the original file at most every 1.5 s instead of on every
    slider move.
- AI:
  - Person and animal masks reuse the detection for the same photo, so every part after the
    first appears almost at once.
  - Tagging prepares images in parallel.
  - Repair handles nearby dust spots in one pass instead of one model run per spot.
- Panorama, HDR and focus stacking decode the next photo while the current one is processed.

### Fixed
- After a crash inside the AI index, semantic search and tagging stayed broken until restart,
  and the background chain thumbnails → AI → faces stopped. Both now recover.
- The edited badge and the new tile shape could be undone by a library reload that ran at the
  same moment.

## [1.0.22] – 2026-10-27

### Added
- While straightening, the crop shows a fine grid (about 30 px squares) for lining up edges and
  the horizon. It fades back to the thirds shortly after the last change.

### Changed
- White balance for RAW files is set in **Kelvin** (2000–25000 K), starting from the camera's
  white balance, instead of a relative −100…100 that often ran out of range. The slider moves
  evenly in mired. **Automatic** suggests a Kelvin value too. Older edits are shown in Kelvin
  and look the same. JPEGs and other finished images keep the relative temperature.
- When an exported file already exists, Lumenkorn asks once for the whole export: overwrite,
  export as well (`name-2`, as before) or skip. Originals in the library are never overwritten,
  and photos of the same export never overwrite each other.

### Fixed
- Exported DNGs now carry a large JPEG preview (2560 px) like Adobe's DNGs. Windows and image
  viewers that only show the embedded preview showed a tiny 256-px picture before.

### Dependencies
- Patch updates for Tauri (api, cli, build, dialog and single-instance plugins), vite,
  svelte-check, serde_json and sha2; base64 0.23.

## [1.0.21] – 2026-10-27

### Added
- Export to **DNG**, slightly lossy like Lightroom's lossy compression (linear data in JPEG tiles,
  a quarter to a third of the RAW's size). Either the finished photo with all edits, or the
  original image data without edits for developing in other programs.
- Export several formats at once, for example DNG to archive and JPEG to share.
- AI downloads queue up: clicking a second download while one is running starts it afterwards
  instead of doing nothing. **Download all AI components** gets everything in one go.

### Fixed
- Panasonic RW2 and Olympus ORF files lost their orientation, capture date and camera, because
  their EXIF data was not read. Portrait photos now stand upright. Photos already in the catalog
  are read again on the next start.
- After rotating a photo between landscape and portrait (or cropping it), the library grid and the
  filmstrip kept the old shape and cut the thumbnail off. They now follow the new shape.
- White balance **Automatic** stayed disabled after opening a photo, so it did nothing. Buttons that
  wait for the photo's settings to load now become active as soon as they are loaded.

## [1.0.20] – 2026-10-27

### Added
- White balance: **Automatic** estimates the color of the light and makes it neutral.
- The **Filter** button (formerly File type) can also show only edited or only unedited photos.

### Fixed
- In Develop, `Enter` takes the focus away from a slider, so the arrow keys move through the
  filmstrip again instead of changing the slider.
- `Enter` applies the crop, also right after typing a straighten angle or picking an aspect ratio.
- The Add/Subtract menus of a mask no longer get cut off at the left edge of the panel.

## [1.0.19] – 2026-10-27

### Fixed
- Panorama: objects close to the camera, such as a tree in the foreground, no longer appear as
  see-through double images. Each part of the panorama now comes from one shot, with the seams
  placed where neighbouring shots differ least, and only a narrow strip along the seams is blended.
- HDR, panorama and focus stacking apply the lens correction (distortion, color fringes,
  vignetting) before aligning the photos. Shots now line up at their edges, where they overlap.

## [1.0.18] – 2026-10-26

### Fixed
- The folder menu (right-click in the folder list) no longer runs off the bottom of the window for
  folders near the end of the list; it moves up, and scrolls if there are very many projects.

## [1.0.17] – 2026-10-26

### Added
- File type filter: **File type** in the toolbar lists the types in the current view (ARW, CR3,
  JPEG, PNG …) with their number, grouped into RAW and other formats; JPG and JPEG count as one.
  The filter works in the library and in Develop (filmstrip), together with search and stars,
  and also for the content search and the AI agent tool `search_photos` (`file_types`).
- Statistics: **Statistics** above the grid shows the photos of the current view (folder,
  project, person, with all filters): cameras, lenses, focal length, aperture, shutter speed,
  ISO, photos per month or year, time of day, keywords, people, countries, file types and
  ratings, plus the share of RAW, edited and located photos. Clicking a camera, lens,
  keyword, person, country or file type filters by it, and the statistics follow.

## [1.0.16] – 2026-10-21

### Added
- Settings → General: a switch turns off the map and place names. Then nothing is sent to
  OpenStreetMap: the Map tab is hidden, no map tiles are loaded, place search and the background
  look-up of place names stop. GPS data from the photos, place names already known and the map in
  the photo album (built in) keep working.
- Settings: the button at the bottom left is now **Settings** and opens a menu with sections for
  General (language, check for updates, data folder), Folders (default destinations for import,
  export and photo albums), Develop (Auto in your style), AI, Graphics card, Keyboard shortcuts and
  AI agents (MCP). The AI progress is still shown next to the button while it runs.
- Keyboard shortcuts can be changed: every command (rating, flags, views, develop tools, zoom,
  import/export, merging) can get other or additional keys; a key already in use is reported, and
  each command or all of them can be reset. Tooltips show the current key. Arrow keys, Enter,
  Space and Escape stay fixed.
- Auto in your style: Auto learns from your edits how you differ from its suggestions and adapts
  step by step, separately for each camera and for RAW/JPEG – first a fixed adjustment per camera,
  then depending on the brightness of the photo (from about 50 edits), later from similar subjects
  (from about 300). A slider is only changed once that demonstrably gets closer to your own results
  on your photos (each photo is predicted without itself). Everything stays local in `stil.json`;
  switch it off or reset it under Settings → Develop.

## [1.0.15] – 2026-10-21

### Added
- Photo album map: larger cities, major mountains with their height, rivers and lakes (Natural Earth,
  built in, works offline). How many appear depends on the area shown; your own places come first.
  Each layer can be switched off on the map page.

### Fixed
- Photo album: the map page now counts towards the chosen number of pages instead of being added on
  top, so the album keeps an even number of pages.

## [1.0.14] – 2026-10-20

### Added
- Photo album: alignment, font size, colour and bold for the date and place captions can be set when
  creating the album (and changed later for all captions at once); they are also the default for
  new texts.

### Fixed
- Photo album: typing in a text (for example the date and place caption) froze the window for a long
  time in larger albums, because every key press laid out and redrew all pages. Now only the page
  that changed is updated (about 250 times faster).

## [1.0.13] – 2026-10-20

### Added
- Photo album: the places on the map can be renamed or their names hidden.

### Fixed
- Place names are always written in Latin script. For places without a German name OpenStreetMap
  returned the local script (Japan: kanji); now the English or romanised name is used, or the
  part is left out. Names already in the catalog are looked up again once, and a saved album takes
  them over when opened. Japanese prefectures now appear in place names.

## [1.0.12] – 2026-10-20

### Added
- Photo album: date and place are written onto the page where a new day or place begins (date and
  place, date only, place only, or none). Your own texts can go above or below the photos (the
  photos move together) or anywhere on the page, with size, alignment, colour and bold; two fonts
  (Instrument Sans, Lora).
- Photo album: a map page (first or last page) with country outlines, the visited countries
  highlighted and a pin for every place, nearby places sharing one pin. It works offline.

## [1.0.11] – 2026-10-20

### Fixed
- Photo album: replacing a photo put the new one into a different spot on the page (often the
  first), so it looked as if the wrong photo had been replaced. The page now keeps its layout.

## [1.0.10] – 2026-10-20

### Added
- Photo album: the **Fotoalbum** button above a project or folder picks the best photos for a
  theme (vacation, travel, wedding, birthday, family and more, or your own description) and lays
  them out on pages in the chosen size (30×30, 20×20, A4, A3 or your own). Photos are rated by
  sharpness, exposure, stars and, with the AI set up, by how well they fit the theme; bursts and
  near-identical shots count once unless other people are in them, photos with 4 or 5 stars are
  always included, and the photos are spread over all days. The pages can then be edited:
  swap, remove or move photos, change a page's layout, drag a photo to change its crop, add or
  delete pages. The album is saved as a print-ready PDF (300 dpi, 3 mm bleed) plus one JPG per
  page. By default only edited photos and JPEGs are used, no unedited RAW files.

### Security
- Crafted image files (PNG, TIFF, EXR) whose header claims a huge size no longer make Lumenkorn
  run out of memory and close; the limit now depends on the free memory. Originals over 1 GB are
  no longer read into memory as a whole.
- AI agents (MCP): exports only go to folders inside the library or the Pictures folder, at most
  500 photos per call; requests with `Origin: null` or a foreign `Host` header are refused.
- Place names: coordinates sent to OpenStreetMap are rounded to about 100 m.
- Import never replaces a file that appeared in the target folder while copying.
- Stricter content security policy; a damaged saved album is ignored instead of loaded.
- Dependencies: rustls 0.23.46 (TLS fixes), zip 9, updater plugin 2.14; GitHub Actions are
  pinned to commit hashes.

## [1.0.9] – 2026-10-20

### Fixed
- Develop: closing the window right after a change (within half a second) lost that change, and
  an open crop was not applied. Pending edits are now saved before the window closes.
- Develop: "Straighten" and restoring a snapshot could land on the next photo when you switched
  photos while they were still running.
- Repair: patches computed with lens correction were placed off by the lens distortion once the
  correction was turned off. They are now left out and shown as needing a recompute.
- Develop: a rare crash when a photo changed on disk while it was being loaded.
- Import: checking whether photos are already in the library was slow in large catalogs; a
  missing index is back (catalog migration 22).
- People: when a photo file changed on disk (another editor, a sync tool), face detection ran
  again and dropped confirmed people, hidden faces and "not this person" decisions. These now
  carry over to the newly detected faces, and a file whose content is unchanged (only its date)
  is not analyzed again.
- A crash while loading an AI model, in the GPU pipeline or while exporting one photo no longer
  disables that part until the app restarts; an export carries on with the next photo.
- Masks: undoing a refinement of an object mask could show the refined mask or the earlier one,
  depending on what was still cached. Every refinement is now stored as a new mask.
- Merge: HDR, panorama and focus stacking now say which photos could not be aligned instead of
  silently stacking them unaligned.
- RAW: files with unusable color matrix or crop data in their metadata no longer crash the
  decoder; they fall back to the generic path.
- AI models are downloaded from fixed versions and checked against SHA-256; cancelling a
  download keeps the part already loaded and reacts within seconds. Searching in English no
  longer has words like "an" or "die" translated as German.
- Robustness: corrupt repair or noise-reduction files, oversized brush data, a source folder
  that suddenly appears empty (sleeping network drive), impossible dates and huge clock shifts
  on import are handled instead of crashing or marking all photos missing.

### Changed
- Library: the loupe now shows photos at full resolution right away when the screen has more
  pixels than the 2048 px preview, not only after zooming in. While you flip through photos it
  prepares the next one in the direction you are going, so it is sharp as soon as it appears.
- Performance: removing a large source folder, the daily clean-up and AI search no longer hold
  the catalog for seconds; scrolling quickly through many photos no longer queues thousands of
  thumbnail jobs; a photo is decoded once when several views ask for it at the same time; the
  histogram is computed once per preview; the GPU pipeline reuses buffers better and falls back
  to the CPU straight away for sizes it could not handle; RAW files are opened once instead of
  three times.
- Map: tiles are written atomically, and offline the map and place names back off instead of
  retrying constantly.
- AI agents (MCP): the access token comes from the system's random source, and long tool calls
  are no longer sent twice after a timeout.

## [1.0.8] – 2026-10-19

### Added
- Map: "Suggest locations…" gives photos without a location the location of a photo with GPS
  (e.g. from a phone) taken within a few minutes. It runs only when asked, on the photos of the
  view or the selection, and saves only the suggestions you confirm. A camera clock that is off
  is estimated per camera from the matching times, or entered by hand.
- Import: the capture time can be corrected per camera before copying, for cameras that still
  ran on home time or were simply off. Folders, file names and the EXIF and XMP time stamps of
  the copies get the corrected time; the files on the card stay unchanged, and each copy is
  still verified byte for byte (against the shifted original). "Estimate" derives the
  difference from photos with a location already in the library, e.g. from a phone.
- Develop: tone curve below color grading, modeled on darktable's RGB curve. One curve for RGB
  and one each for red, green and blue, drawn over the channel's histogram; up to 16 points
  joined by a monotone spline (no overshoot). "Preserve colors" (default on) applies the RGB
  curve to brightness and keeps hue and saturation; off, it works per channel as in Lightroom.
  CPU and GPU evaluate the same lookup table. Copying the "Light" settings includes the curve.

### Fixed
- Develop: on large or high-DPI screens the fitted view showed a 2048 px preview scaled up, so
  it only looked sharp after zooming in. When the screen needs more pixels than the preview
  has, the fitted view now loads the whole image at screen resolution from the full file, as
  zooming already did; the before view as well. While a slider moves, the preview stands in.
- Map: right-click → "Move N selected photos here" did nothing. The menu sat inside the map,
  where Leaflet stops clicks before they reach the interface; it now sits next to it.

## [1.0.7] – 2026-10-18

### Added
- Map view (third tab next to Library and Develop, shortcut `M`): the photos of the current view
  appear as clustered markers on an OpenStreetMap world map. Clicking a marker or cluster
  selects those photos in the filmstrip. A location can be set by searching for a place, by
  dragging photos from the filmstrip onto the map, or from the map's context menu, and removed
  again. The filmstrip marks photos that have a location.
- GPS data from EXIF is read when scanning; photos already in the catalog are filled in once in
  the background. Locations set by hand are stored only in the catalog, the files are not changed,
  and a rescan does not overwrite them (catalog migration 21).
- Every location gets a place name (place, region, country), looked up in the background
  through OpenStreetMap's Nominatim service (at most one request per second). Requests are
  bundled: one answer covers every waiting photo inside the outline of the place it names, so 50
  photos from the same town cost one request, and photos across a town border still get their
  own name. Names are remembered per area of about 1 km. The info panel shows the location with a
  link to the map.
- Map tiles are fetched from OpenStreetMap and cached on disk for 7 days. Leaflet and
  Leaflet.markercluster are used for the map and listed in the credits.

### Changed
- The library search also covers place names, so “Ubud” finds the photos taken there.
- The map and place names need an internet connection and contact OpenStreetMap; coordinates
  are sent to Nominatim to look up names. Everything else stays local.

## [1.0.6] – 2026-10-13

### Changed
- If the catalog was created by a newer Lumenkorn version, the app no longer quits with an
  error. It opens a screen that checks for the update right away and installs it; without a
  connection it links to the releases page. The catalog is not touched.

### Added
- `LUMENKORN_DATA_DIR` lets the app use its own catalog and cache, for trying out development
  builds without upgrading the real catalog.

## [1.0.5] – 2026-10-13

### Changed
- RAW development now follows darktable 5.0's default scene-referred workflow with the sigmoid
  tone mapper, translated from darktable's C code to Rust (`crates/lumenkorn-develop/src/dt/`):
  sensor white and black levels from rawspeed's camera database, white balance as shot by the
  camera, highlight reconstruction “inpaint opposed”, RCD demosaicing (X-Trans: Markesteijn,
  1 pass), exposure +0.7 EV minus the camera's exposure compensation, camera matrix to linear
  Rec.2020, color calibration with CAT16 from the camera's illuminant, and sigmoid tone mapping
  (contrast 1.5, per channel, 100 % hue preservation) on CPU and GPU. Lumenkorn's own default
  contrast, saturation and vibrance for RAWs stay on top. Lumenkorn's own base curve and
  highlight reconstruction for RAWs are removed.
- License: from this version on Lumenkorn is licensed under GPL-3.0-or-later (`LICENSE`),
  because it contains code translated from darktable. Releases up to and including 1.0.4 remain
  available under MIT OR Apache-2.0. `LICENSE-MIT` and `LICENSE-APACHE` are removed.

### Added
- "About Lumenkorn" (ⓘ at the bottom left of the sidebar): version, license, a note that the code
  is largely written with an AI assistant, links to the source code and license, and all
  open-source projects and AI models Lumenkorn uses.
- A compact table of sensor white and black levels from rawspeed's `cameras.xml`
  (LGPL-2.1-or-later), generated by `tools/rawspeed_white.py`.

### Fixed
- RAW: color casts in clipped highlights are gone: orange patches with hard edges on sunlit
  skin, pink and green streaks in clouds, pink cores of blown clouds. Causes were a too low
  sensor white level (rawler uses 15360 for the Sony A7 IV, the data reach 16383, so real values
  counted as clipped), rawler's clipping step that mixed every pixel above 1.0 halfway with
  grey, and Lumenkorn's own heuristics. With Highlights −100, detail above the old curve's white
  point can now be recovered, because the sigmoid never clips hard.
- Vibrance protected skin tones with a hard switch (R > G > B), which could create edges where
  two channels are equal; the transition is now smooth.
- Thumbnails of edited RAW photos are created again once (catalog migration v20).
- Masks and repairs: brush, object click, gradients and the HSL picker now also work in the
  zoomed view. Until now the tools were only shown in the fitted view; when zoomed in, clicks
  and strokes moved the image instead.
- RAW files without an embedded preview get a thumbnail developed with default settings instead
  of failing at every start.
- Log: no more Vulkan loader messages and rawler TIFF warnings with backtraces at every start.

## [1.0.4] – 2026-10-12

### Fixed
- RAW: sunlit skin no longer shows flat yellow-green patches when highlights or exposure are
  pulled down. Partly clipped areas now estimate the clipped channel from the measured ones and
  the colour of the nearly clipped surroundings, so skin stays skin-coloured; areas where all
  three channels are clipped become neutral white without brightness steps. Thumbnails of
  edited RAW photos are created again once.

## [1.0.3] – 2026-10-12

### Added
- Import from a memory card (`Ctrl+Shift+I`): choose photos by day or one by one with previews,
  copy them to `Target\YYYY\YYYY.MM.DD[_Occasion]` and rename them to
  `YYYYMMDD_HHMMSS_Number` (capture time, camera image number). RAW/JPEG pairs and `.xmp`
  files stay together; photos already in the library are recognized. Copies are verified
  byte for byte, and the originals can be deleted from the card afterwards.

### Fixed
- RAW: blown-out highlights (a white wall in the sun) no longer turn pink or magenta. Channels
  that hit the sensor's maximum are reconstructed instead of being lifted by the white balance
  on their own; areas where several channels are clipped become neutral white. Thumbnails of
  edited RAW photos are created again once.

### Changed
- RAW photos look like the camera's own JPEGs by default instead of washed out: a new base tone
  curve (brighter midtones, deeper shadows, white at the sensor's clipping point with a little
  room left for highlight recovery) plus a stronger base contrast, saturation and vibrance, all
  matched against the embedded JPEGs of 14 Sony ARW files (midtone contrast about 99 % and
  colourfulness about 98 % of the camera, before 71 % and 75 %). The sliders still start at 0;
  −100 and +100 keep their full effect (before, contrast above about +80 did nothing for RAWs).
  Black-and-white edits keep their grey tones. Edited RAW photos look brighter than before.

## [1.0.2] – 2026-10-12

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
