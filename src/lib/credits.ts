// Open-Source-Bestandteile und KI-Modelle für den Dialog „Über Lumenkorn“.
// Dieselben Angaben stehen in README.md und docs/README.de.md – bei neuen
// Abhängigkeiten oder Modellen alle drei Stellen anpassen.

import { t } from "./i18n";

export interface Credit {
  name: string;
  url: string;
  use: string;
  license: string;
}

export const REPO_URL = "https://github.com/DrayPrescot/lumenkorn";
export const LICENSE_URL = `${REPO_URL}/blob/main/LICENSE`;

/** In die App eingebaute bzw. von ihr geladene Bibliotheken und Daten */
export const LIBRARIES: Credit[] = [
  { name: "darktable", url: "https://github.com/darktable-org/darktable", use: t("RAW-Entwicklung: Lichter-Rekonstruktion, Demosaicing, Farbkalibrierung, sigmoid (nach Rust übertragen)"), license: "GPL-3.0-or-later" },
  { name: "rawler (dnglab)", url: "https://github.com/dnglab/dnglab", use: t("RAW-Dateien lesen"), license: "LGPL-2.1" },
  { name: t("rawspeed-Kameradaten"), url: "https://github.com/darktable-org/rawspeed", use: t("Weiß- und Schwarzpunkte der Sensoren"), license: "LGPL-2.1-or-later" },
  { name: t("Lensfun-Datenbank"), url: "https://github.com/lensfun/lensfun", use: t("Objektivprofile für die Objektivkorrektur"), license: "CC BY-SA 3.0" },
  { name: "Tauri", url: "https://github.com/tauri-apps/tauri", use: t("App-Gerüst, Fenster, Dateidialoge, Updates (mit den Plugins dialog, updater, single-instance)"), license: "MIT / Apache-2.0" },
  { name: "Svelte", url: "https://github.com/sveltejs/svelte", use: t("Oberfläche"), license: "MIT" },
  { name: "Leaflet, Leaflet.markercluster", url: "https://github.com/Leaflet/Leaflet", use: t("Karte und gebündelte Fotomarker"), license: "BSD-2-Clause, MIT" },
  { name: "OpenStreetMap", url: "https://www.openstreetmap.org/copyright", use: t("Kartenkacheln und Ortssuche (Nominatim), online abgerufen"), license: t("Daten: ODbL, Kacheln: CC BY-SA 2.0") },
  { name: "wgpu", url: "https://github.com/gfx-rs/wgpu", use: t("Bildbearbeitung auf der Grafikkarte"), license: "MIT / Apache-2.0" },
  { name: "ONNX Runtime", url: "https://github.com/microsoft/onnxruntime", use: t("KI-Modelle ausführen (wird bei Bedarf geladen)"), license: "MIT" },
  { name: "Dawn, DirectX Shader Compiler", url: "https://dawn.googlesource.com/dawn", use: t("Teil von ONNX Runtime für WebGPU"), license: t("BSD-3-Clause, NCSA (dxil.dll: Microsoft-Lizenz)") },
  { name: "ort", url: "https://github.com/pykeio/ort", use: t("Anbindung an ONNX Runtime"), license: "MIT / Apache-2.0" },
  { name: "SQLite, rusqlite", url: "https://github.com/rusqlite/rusqlite", use: t("Katalog und Suchindex"), license: t("Public Domain / MIT") },
  { name: "image", url: "https://github.com/image-rs/image", use: t("JPEG, PNG, TIFF und WebP lesen und schreiben"), license: "MIT / Apache-2.0" },
  { name: "jpeg-encoder", url: "https://github.com/vstroebel/jpeg-encoder", use: t("Schnelle JPEG-Kodierung"), license: "MIT / Apache-2.0, IJG" },
  { name: "exrs", url: "https://github.com/johannesvollmer/exrs", use: t("OpenEXR-Dateien für zusammengefügte Bilder"), license: "BSD-3-Clause" },
  { name: "kamadak-exif", url: "https://github.com/kamadak/exif-rs", use: t("EXIF-Daten"), license: "BSD-2-Clause" },
  { name: "Rayon", url: "https://github.com/rayon-rs/rayon", use: t("Paralleles Rechnen auf der CPU"), license: "MIT / Apache-2.0" },
  { name: "notify, walkdir", url: "https://github.com/notify-rs/notify", use: t("Ordner beobachten und durchsuchen"), license: "CC0-1.0, MIT / Unlicense" },
  { name: "BLAKE3", url: "https://github.com/BLAKE3-team/BLAKE3", use: t("Prüfsummen für verschobene Dateien"), license: "CC0-1.0 / Apache-2.0" },
  { name: "trash", url: "https://github.com/Byron/trash-rs", use: t("Dateien in den Papierkorb verschieben"), license: "MIT" },
  { name: "ureq, tiny_http", url: "https://github.com/algesten/ureq", use: t("Downloads, Schnittstelle für KI-Agenten (MCP)"), license: "MIT / Apache-2.0" },
  { name: "serde, serde_json", url: "https://github.com/serde-rs/serde", use: t("Einstellungen, Bearbeitungen, Datenaustausch"), license: "MIT / Apache-2.0" },
  { name: "sha2, flate2, tar, zip", url: "https://github.com/RustCrypto/hashes", use: t("Downloads prüfen und entpacken"), license: "MIT / Apache-2.0, MIT" },
  { name: "half, bytemuck, pollster", url: "https://github.com/VoidStarKat/half-rs", use: t("Hilfen für die Grafikkarte"), license: "MIT / Apache-2.0" },
  { name: "regex, log, env_logger, thiserror, libloading, dirs, base64", url: "https://github.com/rust-lang/regex", use: t("Hilfsbibliotheken"), license: "MIT / Apache-2.0, ISC" },
  { name: "Instrument Sans", url: "https://github.com/Instrument/instrument-sans", use: t("Schrift der Oberfläche"), license: "OFL-1.1" },
  { name: "Lora", url: "https://github.com/cyrealtype/Lora-Cyrillic", use: t("Serifenschrift im Fotoalbum"), license: "OFL-1.1" },
  { name: "Natural Earth", url: "https://www.naturalearthdata.com", use: t("Kartendaten im Fotoalbum: Länder, Städte, Berge, Flüsse und Seen"), license: "Public Domain" },
];

/** KI-Modelle – nicht in der App enthalten, werden bei Bedarf heruntergeladen */
export const MODELS: Credit[] = [
  { name: "CLIP ViT-B/32", url: "https://huggingface.co/Xenova/clip-vit-base-patch32", use: t("Schlagworte und Suche nach Bildinhalt"), license: "MIT" },
  { name: "IS-Net (DIS)", url: "https://github.com/xuebinqin/DIS", use: t("Motiv-Maske"), license: "Apache-2.0" },
  { name: "Segment Anything (ViT-B)", url: "https://github.com/facebookresearch/segment-anything", use: t("Objekt-, Personen- und Tiermasken"), license: "Apache-2.0" },
  { name: "skyseg (U²-Net)", url: "https://huggingface.co/JianyuanWang/skyseg", use: t("Himmel-Maske"), license: "MIT" },
  { name: "OWLv2", url: "https://huggingface.co/Xenova/owlv2-base-patch16", use: t("Personen, Gesichter und Tiere finden"), license: "Apache-2.0" },
  { name: "MediaPipe Selfie Multiclass", url: "https://huggingface.co/senty-au/selfie_multiclass_256x256-ONNX", use: t("Haare, Haut und Kleidung"), license: "Apache-2.0" },
  { name: "MediaPipe Face Mesh V2", url: "https://huggingface.co/fernandotonon/QtMeshEditor-facemesh-onnx", use: t("Augen, Brauen und Lippen"), license: "Apache-2.0" },
  { name: "DRUNet", url: "https://github.com/cszn/KAIR", use: t("Rauschreduzierung"), license: "MIT" },
  { name: "LaMa", url: "https://github.com/advimman/lama", use: t("Reparatur (Störendes entfernen)"), license: "Apache-2.0" },
  { name: "YuNet", url: "https://github.com/opencv/opencv_zoo", use: t("Gesichter finden"), license: "MIT" },
  { name: "SFace", url: "https://github.com/opencv/opencv_zoo", use: t("Gesichter wiedererkennen"), license: "Apache-2.0" },
];
