// Mock-Backend für die Arbeit an der Oberfläche im normalen Browser.
// Erzeugt eine Beispielbibliothek mit Platzhalterbildern.

import { DEFAULT_PARAMS, NO_GROUPS, transferParams } from "./api";
import { t } from "./i18n";
import type { DevelopParams, Edit, Preset, FolderNode, ImageDetail, ImageMeta, ImageQuery, ImageSummary, ImageTag, MetaPatch, Project, Root } from "./api";

// rating/flag gehören zur Bildliste (ImageSummary), nicht zu den Details
type Img = ImageDetail & {
  rootId: number;
  hue: number;
  rating: number;
  flag: number;
  edit?: DevelopParams;
  snapshots: Edit[];
  /** Metadaten bearbeiten: Dateiwerte der überschriebenen Felder */
  fileMeta?: Record<string, unknown>;
};

const META_TEXT = ["title", "description", "creator", "copyright"] as const;
const META_FILE = ["capturedAt", "camera", "lens", "iso", "exposure", "aperture", "focalLength"] as const;

/** Wie das Backend: Zeit wie „2024-06-01 14:03:22“ oder mit T, Sekunden optional */
function metaFor(i: Img): ImageMeta {
  const rec = i as unknown as Record<string, unknown>;
  const field = (k: string, backed: boolean) => {
    const overridden = backed ? !!i.fileMeta && k in i.fileMeta : rec[k] != null;
    return { value: (rec[k] ?? null) as never, file: (backed ? (overridden ? i.fileMeta![k] : rec[k]) ?? null : null) as never, overridden };
  };
  const o: Record<string, unknown> = { id: i.id };
  for (const k of META_TEXT) o[k] = field(k, false);
  for (const k of META_FILE) o[k] = field(k, true);
  return o as unknown as ImageMeta;
}

function applyMetaPatch(i: Img, patch: MetaPatch) {
  const rec = i as unknown as Record<string, unknown>;
  for (const [k, e] of Object.entries(patch) as [string, "keep" | "reset" | { set: unknown }][]) {
    if (e === "keep") continue;
    const backed = (META_FILE as readonly string[]).includes(k);
    if (e === "reset") {
      if (backed && i.fileMeta && k in i.fileMeta) {
        rec[k] = i.fileMeta[k];
        delete i.fileMeta[k];
      } else if (!backed) rec[k] = null;
      continue;
    }
    if (backed) {
      i.fileMeta ??= {};
      if (!(k in i.fileMeta)) i.fileMeta[k] = rec[k] ?? null;
    }
    rec[k] = e.set;
  }
  i.metaEdited = META_TEXT.some((k) => rec[k] != null) || !!i.fileMeta && Object.keys(i.fileMeta).length > 0;
}

const sep = "/";
const roots: Root[] = [
  { id: 1, path: "/home/anna/Bilder", name: "Bilder", imageCount: 0, lastScan: 0 },
  { id: 2, path: "/media/archiv/Fotos", name: "Fotos", imageCount: 0, lastScan: 0 },
];

const dirs: [number, string, number][] = [
  [1, "/home/anna/Bilder/2025/Lofoten", 46],
  [1, "/home/anna/Bilder/2025/Hochzeit Lena & Tom", 38],
  [1, "/home/anna/Bilder/2026/Porträts Studio", 24],
  [1, "/home/anna/Bilder/2026/Streetfotografie München", 31],
  [2, "/media/archiv/Fotos/Kunde Brauhaus/Produkt", 18],
  [2, "/media/archiv/Fotos/Kunde Brauhaus/Location", 22],
];

const ratios = [1.5, 1.5, 1.5, 0.667, 1.333, 0.75, 1.778, 1, 2.4];
const cameras = ["Sony ILCE-7M4", "Fujifilm X-T5", "Canon EOS R6m2"];
const lenses = ["FE 24-70mm F2.8 GM II", "XF33mmF1.4 R LM WR", "RF35mm F1.8 MACRO IS STM"];
const tagPool = ["Berge", "Meer", "Himmel", "Person", "Gebäude", "Straße", "Nacht", "Schnee", "Bier", "Porträt", "Hund"];

let nextId = 1;
// Beispiel-Presets für die Browser-Vorschau
const demoPreset = (id: number, name: string, p: Partial<DevelopParams>, g: Partial<typeof NO_GROUPS>): Preset =>
  ({ id, name, params: { ...structuredClone(DEFAULT_PARAMS), ...p }, groups: { ...NO_GROUPS, ...g }, createdAt: 0 });
// Erst beim ersten Zugriff anlegen: api.ts und mock.ts importieren sich gegenseitig
let presetStore: Preset[] | null = null;
let mcpOn = false;
const demoPresets = (): Preset[] => [
  demoPreset(1, "Matt und weich", { contrast: -20, blacks: 25, highlights: -30, saturation: -10 }, { light: true, color: true }),
  demoPreset(2, "Schwarzweiß kontrastreich", { saturation: -100, contrast: 35, clarity: 20 }, { light: true, color: true, details: true }),
  demoPreset(3, "Warmer Abend", { temperature: 25, tint: 5, vibrance: 15 }, { whiteBalance: true, color: true }),
];
const images: Img[] = [];

// Beispielorte für Karte und Ortssuche
const mockPlaces = [
  { name: "München, Bayern, Deutschland", display: "München, Bayern, Deutschland", lat: 48.1372, lon: 11.5755, bbox: [48.06, 48.25, 11.36, 11.72] as [number, number, number, number] },
  { name: "Luzern, Schweiz", display: "Luzern, Luzern, Schweiz", lat: 47.0502, lon: 8.3093, bbox: null },
  { name: "Reykjavík, Island", display: "Reykjavík, Hauptstadtregion, Island", lat: 64.1466, lon: -21.9426, bbox: null },
  { name: "Kyoto, Japan", display: "Kyoto, Präfektur Kyoto, Japan", lat: 35.0116, lon: 135.7681, bbox: null },
  { name: "San José, Costa Rica", display: "San José, Provinz San José, Costa Rica", lat: 9.9281, lon: -84.0907, bbox: null },
];

// Beispiel-Gesichter: die ersten zwölf Fotos je ein Gesicht – „Anna“ (bestätigt oder vorgeschlagen) oder ohne Zuordnung
const mockPersons: { id: number; name: string; group?: number | null }[] = [
  { id: 1, name: "Anna", group: 1 },
  { id: 2, name: "Ben", group: 1 },
  { id: 3, name: "Clara", group: 2 },
  { id: 4, name: "David", group: 2 },
  { id: 5, name: "Eva" },
  { id: 6, name: "" }, // unbenannte Gruppe
];
let mockGroups: { id: number; name: string }[] = [{ id: 1, name: "Familie" }, { id: 2, name: "Freunde" }];
const mockFaces: { id: number; imageId: number; rect: [number, number, number, number]; score: number; personId: number | null; personName: string | null; confirmed: boolean }[] = [];
function mockPeople() {
  if (!mockFaces.length) {
    images.slice(0, 12).forEach((im, i) =>
      mockFaces.push({ id: i + 1, imageId: im.id, rect: [0.4, 0.3, 0.2, 0.3], score: 0.9, personId: i % 4 === 3 ? 6 : 1, personName: i % 4 === 3 ? null : "Anna", confirmed: i % 2 === 0 }),
    );
    // weitere benannte Personen für die Gruppen-Ansicht
    images.slice(12, 40).forEach((im, i) => {
      const p = mockPersons[1 + (i % 4)];
      mockFaces.push({ id: 100 + i, imageId: im.id, rect: [0.4, 0.3, 0.2, 0.3], score: 0.9, personId: p.id, personName: p.name, confirmed: true });
    });
  }
  return mockPersons
    .map((p) => {
      const fs = mockFaces.filter((f) => f.personId === p.id);
      return { id: p.id, name: p.name || null, faceCount: fs.length, suggested: fs.filter((f) => !f.confirmed).length, imageCount: fs.length, coverFace: fs[0]?.id ?? null, groupId: p.group ?? null };
    })
    .filter((p) => p.faceCount > 0);
}
let seed = 7;
const rnd = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);

for (const [rootId, dir, n] of dirs) {
  for (let i = 0; i < n; i++) {
    const r = ratios[Math.floor(rnd() * ratios.length)];
    const raw = rnd() > 0.45;
    const ext = raw ? (rootId === 1 ? "arw" : "raf") : "jpg";
    const day = 1 + Math.floor(rnd() * 27);
    const id = nextId++;
    const tags: ImageTag[] = tagPool
      .filter(() => rnd() > 0.8)
      .map((name) => ({ name, source: "ai" as const, confidence: 0.5 + rnd() * 0.5 }));
    images.push({
      id,
      rootId,
      hue: Math.floor(rnd() * 360),
      path: `${dir}${sep}DSC0${1000 + id}.${ext}`,
      dir,
      fileName: `DSC0${1000 + id}.${ext.toUpperCase()}`,
      ext,
      size: 18_000_000 + Math.floor(rnd() * 30_000_000),
      width: Math.round(6000 * Math.min(1, r)),
      height: Math.round((6000 * Math.min(1, r)) / r),
      capturedAt: `20${dir.includes("2026") ? 26 : 25}-0${1 + (id % 9)}-${String(day).padStart(2, "0")}T${String(8 + (id % 12)).padStart(2, "0")}:${String(id % 60).padStart(2, "0")}:12`,
      camera: cameras[id % 3],
      lens: lenses[id % 3],
      iso: [100, 200, 400, 800, 3200][id % 5],
      exposure: ["1/250", "1/1000", "1/60", "1/8000", "2s"][id % 5],
      aperture: [1.4, 2.8, 4, 8, 11][id % 5],
      focalLength: [24, 33, 35, 50, 70][id % 5],
      rating: rnd() > 0.7 ? Math.ceil(rnd() * 5) : 0,
      flag: 0,
      tags,
      snapshots: [],
      // Etwa jedes dritte Foto mit Ort, leicht gestreut um einen der Beispielorte
      location:
        id % 3 === 0
          ? (() => {
              const pl = mockPlaces[id % mockPlaces.length];
              return { lat: pl.lat + (rnd() - 0.5) * 0.08, lon: pl.lon + (rnd() - 0.5) * 0.12, place: pl.name, manual: id % 2 === 0 };
            })()
          : null,
    });
  }
}

const projects: Project[] = [
  {
    id: 1,
    name: "Portfolio 2026",
    description: "",
    folders: [
      { path: "/home/anna/Bilder/2025/Lofoten", recursive: true },
      { path: "/home/anna/Bilder/2026/Porträts Studio", recursive: true },
      { path: "/media/archiv/Fotos/Kunde Brauhaus", recursive: true },
    ],
    imageCount: 0,
    createdAt: 0,
    updatedAt: 0,
  },
];

const within = (p: string, d: string) => p === d || p.startsWith(d + sep);

function inScope(img: Img, q: ImageQuery): boolean {
  const s = q.scope;
  if (s.type === "folder") return s.recursive ? within(img.dir, s.path) : img.dir === s.path;
  if (s.type === "project") {
    const p = projects.find((p) => p.id === s.id);
    return !!p?.folders.some((f) => (f.recursive ? within(img.dir, f.path) : img.dir === f.path));
  }
  if (s.type === "person") return mockFaces.some((f) => f.imageId === img.id && f.personId === s.id);
  return true;
}

/** Fotos der Abfrage, ohne Sortierung (`withTypes`: Dateityp-Filter anwenden) */
function matching(q: ImageQuery, withTypes = true): Img[] {
  const words = (q.search ?? "").toLowerCase().split(/\s+/).filter(Boolean);
  const exts = withTypes && q.exts?.length ? q.exts : null;
  return images.filter(
    (i) =>
      inScope(i, q) &&
      i.rating >= (q.minRating ?? 0) &&
      (!exts || exts.includes(i.ext)) &&
      (q.edited == null || !!i.edit === q.edited) &&
      words.every((w) =>
        [i.fileName, i.camera ?? "", i.lens ?? "", i.location?.place ?? "", ...i.tags.map((tag) => tag.name)].some((s) => s.toLowerCase().includes(w)),
      ),
  );
}

function countBy<T>(list: Img[], key: (i: Img) => T | null | undefined): [T, number][] {
  const m = new Map<T, number>();
  for (const i of list) {
    const k = key(i);
    if (k !== null && k !== undefined && k !== "") m.set(k, (m.get(k) ?? 0) + 1);
  }
  return [...m.entries()];
}

function fileTypes(list: Img[]) {
  return countBy(list, (i) => i.ext)
    .map(([ext, count]) => ({ ext, kind: ext === "jpg" ? "raster" : "raw", count }))
    .sort((a, b) => b.count - a.count);
}

function stats(q: ImageQuery) {
  const list = matching(q);
  const labels = (key: (i: Img) => string | null | undefined) =>
    countBy(list, key)
      .map(([label, count]) => ({ label, count }))
      .sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));
  const values = (key: (i: Img) => number | null | undefined) =>
    countBy(list, key)
      .map(([value, count]) => ({ value, count }))
      .sort((a, b) => a.value - b.value);
  const hours = Array<number>(24).fill(0);
  for (const i of list) if (i.capturedAt) hours[Number(i.capturedAt.slice(11, 13))]++;
  const dates = list.map((i) => i.capturedAt).filter((d): d is string => !!d).sort();
  const tagCount = new Map<string, number>();
  for (const i of list) for (const tag of i.tags) tagCount.set(tag.name, (tagCount.get(tag.name) ?? 0) + 1);
  return {
    total: list.length,
    raw: list.filter((i) => i.ext !== "jpg").length,
    edited: list.filter((i) => i.edit).length,
    withLocation: list.filter((i) => i.location).length,
    first: dates[0] ?? null,
    last: dates[dates.length - 1] ?? null,
    ratings: [0, 1, 2, 3, 4, 5].map((r) => list.filter((i) => i.rating === r).length),
    cameras: labels((i) => i.camera),
    lenses: labels((i) => i.lens),
    focalLengths: values((i) => i.focalLength),
    apertures: values((i) => i.aperture),
    isos: values((i) => i.iso),
    exposures: labels((i) => i.exposure),
    fileTypes: fileTypes(list),
    months: labels((i) => i.capturedAt?.slice(0, 7)).sort((a, b) => a.label.localeCompare(b.label)),
    hours,
    keywords: [...tagCount.entries()].map(([label, count]) => ({ label, count })).sort((a, b) => b.count - a.count).slice(0, 40),
    people: [],
    countries: labels((i) => i.location?.place?.split(",").pop()?.trim()),
  };
}

function query(q: ImageQuery): ImageSummary[] {
  let list = matching(q);
  const by = q.sort ?? "capturedAsc";
  list = [...list].sort((a, b) => {
    if (by === "nameAsc") return a.fileName.localeCompare(b.fileName);
    if (by === "ratingDesc") return b.rating - a.rating;
    const c = (a.capturedAt ?? "").localeCompare(b.capturedAt ?? "");
    return by === "capturedDesc" ? -c : c;
  });
  return list.map((i) => ({
    id: i.id,
    path: i.path,
    fileName: i.fileName,
    kind: i.ext === "jpg" ? "raster" : "raw",
    width: i.width,
    height: i.height,
    capturedAt: i.capturedAt,
    rating: i.rating,
    flag: i.flag,
    thumbReady: true,
    missing: false,
    edited: !!i.edit,
    ...(i.location ? { lat: i.location.lat, lon: i.location.lon } : {}),
  }));
}

function tree(root: Root): FolderNode {
  const counts = new Map<string, number>([[root.path, 0]]);
  for (const i of images.filter((i) => i.rootId === root.id)) {
    counts.set(i.dir, (counts.get(i.dir) ?? 0) + 1);
    let p = i.dir;
    while (p.lastIndexOf(sep) > root.path.length - 1) {
      p = p.slice(0, p.lastIndexOf(sep));
      if (!counts.has(p)) counts.set(p, 0);
    }
  }
  const build = (path: string, name: string): FolderNode => {
    const children = [...counts.keys()]
      .filter((k) => k.startsWith(path + sep) && !k.slice(path.length + 1).includes(sep))
      .sort()
      .map((k) => build(k, k.slice(path.length + 1)));
    const count = counts.get(path) ?? 0;
    return { path, name, count, total: count + children.reduce((s, c) => s + c.total, 0), children };
  };
  return build(root.path, root.name);
}

function withCount(p: Project): Project {
  return { ...p, imageCount: query({ scope: { type: "project", id: p.id } }).length };
}

let mockLearnStyle = true;
let mockMapEnabled = true;
/** Ereignisse der Attrappe (sonst sendet nur das Backend welche) */
const mockListeners = new Map<string, Set<(p: unknown) => void>>();
function mockEmit(event: string, payload: unknown) {
  for (const cb of mockListeners.get(event) ?? []) cb(payload);
}
let mockNc: { server: string; loginName: string; baseFolder: string } | null = null;
const mockShares = new Map<number, string>();
let mockExportSubfolder = "Export";

const delay = <T>(v: T) => new Promise<T>((r) => setTimeout(() => r(structuredClone(v)), 30));

export const mock = {
  async invoke<T>(cmd: string, a: Record<string, any> = {}): Promise<T> {
    const out = (v: unknown) => delay(v) as Promise<T>;
    switch (cmd) {
      case "set_language":
        return out(null);
      case "app_info":
        // ?neuer im Adressfeld: Hinweis „Katalog von neuerer Version“ ansehen
        return out({
          version: "0.1.0",
          platform: "browser",
          dataDir: "(Mock)",
          catalogTooNew: location.search.includes("neuer")
            ? "Der Katalog stammt von einer neueren Lumenkorn-Version (Stand 20, diese Version kennt bis 19). Bitte Lumenkorn aktualisieren."
            : null,
        });
      case "app_exit":
        return out(null);
      case "open_url":
        window.open(String(a.url), "_blank", "noopener");
        return out(null);
      case "list_roots":
        return out(roots.map((r) => ({ ...r, imageCount: images.filter((i) => i.rootId === r.id).length })));
      case "add_root":
        throw t("Im Browser-Modus können keine Ordner hinzugefügt werden. Starte die App mit `npm run tauri dev`.");
      case "remove_root":
      case "rescan_root":
        return out(null);
      case "folder_tree":
        return out(tree(roots.find((r) => r.id === a.rootId)!));
      case "query_images":
        return out(query(a.query));
      case "photo_stats":
        return out(stats(a.query));
      case "file_types":
        return out(fileTypes(matching(a.query, false)));
      case "geo_search": {
        await new Promise((r) => setTimeout(r, 300));
        const q = String(a.query).toLowerCase();
        return out(mockPlaces.filter((pl) => pl.display.toLowerCase().includes(q)));
      }
      case "geo_set_location":
        for (const i of images.filter((i) => a.ids.includes(i.id))) {
          const near = mockPlaces.find((pl) => Math.abs(pl.lat - a.lat) < 1 && Math.abs(pl.lon - a.lon) < 1);
          i.location = { lat: a.lat, lon: a.lon, place: a.place ?? near?.name ?? null, manual: true };
        }
        return out(null);
      case "geo_time_suggest": {
        await new Promise((r) => setTimeout(r, 500));
        const without = images.filter((i) => !i.location);
        const matches = without.slice(0, 14).map((i, k) => {
          const pl = mockPlaces[k % 2];
          return { id: i.id, refId: images.find((j) => j.location)?.id ?? i.id, lat: pl.lat, lon: pl.lon, place: pl.name, deltaS: 40 + k * 17 };
        });
        return out({
          matches,
          offsets: [
            { camera: "Sony ILCE-7M4", offsetS: a.offsetS ?? -3720, estimated: a.offsetS == null, uncertain: false, matched: 9, photos: 30 },
            { camera: "Fujifilm X-T5", offsetS: 0, estimated: false, uncertain: a.offsetS == null, matched: 5, photos: 12 },
          ],
          withoutTime: 2,
          candidates: without.length,
        });
      }
      case "geo_apply":
        for (const it of a.items) {
          const i = images.find((i) => i.id === it.id);
          if (i) i.location = { lat: it.lat, lon: it.lon, place: it.place ?? null, manual: true };
        }
        return out(null);
      case "geo_clear_location":
        for (const i of images.filter((i) => a.ids.includes(i.id))) i.location = null;
        return out(null);
      case "image_detail": {
        const { rootId, hue, edit, snapshots, fileMeta, ...d } = images.find((i) => i.id === a.id)!;
        return out(d);
      }
      case "develop_load": {
        const img = images.find((i) => i.id === a.id)!;
        await new Promise((r) => setTimeout(r, 400)); // RAW-Dekodierung simulieren
        return out({ params: img.edit ?? null, fullWidth: img.width, fullHeight: img.height, raw: img.ext !== "jpg", asShotKelvin: img.ext !== "jpg" ? 5350 : null, snapshots: img.snapshots, lens: img.ext !== "jpg" ? { distortion: "sony", chromatic: "sony", vignetting: "lensfun", lens: "Sony FE 24-70mm f/2.8 GM II" } : null });
      }
      case "develop_auto":
        await new Promise((r) => setTimeout(r, 300));
        return out({ exposure: 0.45, contrast: 18, highlights: -22, shadows: 15, whites: 34, blacks: -21, vibrance: 12 });
      case "develop_auto_wb": {
        const raw = images.find((i) => i.id === a.id)?.ext !== "jpg";
        return out(raw ? { temperature: 0, tint: 6, kelvin: 4380 } : { temperature: -18, tint: 6, kelvin: 0 });
      }
      case "develop_detect_tilt":
        return out(1.5);
      case "develop_params": {
        const img = images.find((i) => i.id === a.id)!;
        return out({ ...DEFAULT_PARAMS, ...(img.edit ?? {}) });
      }
      case "develop_release":
        return out(null);
      case "develop_transfer":
        for (const i of images.filter((i) => a.ids.includes(i.id))) i.edit = transferParams({ ...DEFAULT_PARAMS, ...(i.edit ?? {}) }, a.source, a.groups);
        return out(null);
      case "develop_save":
        for (const i of images.filter((i) => a.ids.includes(i.id))) i.edit = a.params;
        return out(null);
      case "preset_list":
        return out((presetStore ??= demoPresets()));
      case "preset_create": {
        const params = transferParams(structuredClone(DEFAULT_PARAMS), a.params, a.groups);
        presetStore = [...(presetStore ??= demoPresets()), { id: Date.now(), name: a.name.trim(), params, groups: a.groups, createdAt: Date.now() / 1000 }]
          .sort((x, y) => x.name.localeCompare(y.name, "de", { sensitivity: "base" }));
        return out(presetStore);
      }
      case "preset_rename":
        presetStore = (presetStore ??= demoPresets()).map((p) => (p.id === a.id ? { ...p, name: a.name.trim() || p.name } : p))
          .sort((x, y) => x.name.localeCompare(y.name, "de", { sensitivity: "base" }));
        return out(presetStore);
      case "preset_delete":
        presetStore = (presetStore ??= demoPresets()).filter((p) => p.id !== a.id);
        return out(presetStore);
      case "develop_snapshot": {
        const img = images.find((i) => i.id === a.id)!;
        img.snapshots.unshift({ id: Date.now(), imageId: img.id, name: a.name || "Schnappschuss", params: structuredClone(img.edit!), isCurrent: false, createdAt: Date.now() / 1000, updatedAt: 0 });
        return out(img.snapshots);
      }
      case "develop_restore": {
        const img = images.find((i) => i.snapshots.some((s) => s.id === a.editId))!;
        img.edit = structuredClone(img.snapshots.find((s) => s.id === a.editId)!.params);
        return out(img.edit);
      }
      case "develop_delete_snapshot": {
        const img = images.find((i) => i.id === a.id)!;
        img.snapshots = img.snapshots.filter((s) => s.id !== a.editId);
        return out(img.snapshots);
      }
      case "ai_status":
        return out({ runtimePath: null, runtimeDownloadable: true, modelReady: false, missing: ["vision.onnx", "text.onnx", "vocab.json", "merges.txt"], downloadSize: 161_000_000, modelDir: "(Mock)", masksReady: true, skyReady: true, partsReady: true, masksDownloadSize: 0, repairReady: false, repairDownloadSize: 92_600_000, denoiseReady: false, denoiseDownloadSize: 131_000_000, gpuRuntimes: [{ backend: "webgpu", label: "WebGPU (Vulkan)", version: "1.27.0", installed: false, downloadSize: 25_163_729 }], accel: { active: { library: null, gpuEnabled: false, gpuError: null, placements: [] }, devices: [{ vendor: "AMD", name: "Radeon RX 7900 XTX / XT / GRE", memory: 25_753_026_560, discrete: true }], restartRequired: false, develop: { enabled: true, available: true, info: { name: "AMD Radeon RX 7900 XTX (RADV NAVI31)", backend: "Vulkan", kind: "discrete", driver: "radv Mesa" }, error: null } }, crashNotice: null, downloading: false, loaded: false, error: null, analyzed: 0, total: images.length, settings: { autoTag: true, accel: "auto", developGpu: true, faces: true, learnStyle: mockLearnStyle } });
      case "style_status":
        // Beispielhafter Lernstand für die Vorschau
        return out({
          enabled: mockLearnStyle,
          samples: 71,
          toneSamples: 64,
          pending: 3,
          summary: {
            samples: 71,
            stage2: true,
            stage3: true,
            stage4: false,
            groups: [
              { group: "ILCE-7M4 (RAW)", samples: 37, tone: [0.1, 0, 0, 0, 0, -12, 8], extra: [null, 18, null, null], adaptive: [false, false, false, false, false, false, true] },
              { group: "Pixel 8 (JPEG)", samples: 28, tone: [0, -9, 0, 0, 0, 0, 0], extra: [null, null, null, null], adaptive: [false, false, false, false, false, false, false] },
            ],
          },
        });
      case "app_settings":
        return out({ mapEnabled: mockMapEnabled, exportSubfolder: mockExportSubfolder });
      case "app_set_settings": {
        const sub = a.settings?.exportSubfolder;
        if (typeof sub === "string") {
          let s = sub.trim().replace(/\\/g, "/") || "Export";
          if (/^\/+$/.test(s)) s = ".";
          if (/^\/|:|[<>"|?*]/.test(s)) throw new Error(`„${s}“ ist kein gültiger relativer Ordner`);
          mockExportSubfolder = s;
        }
        mockMapEnabled = a.settings?.mapEnabled ?? mockMapEnabled;
        return out({ mapEnabled: mockMapEnabled, exportSubfolder: mockExportSubfolder });
      }
      case "style_reset":
        return out(null);
      case "people_status":
        return out({ ready: true, downloadSize: 38_929_000, enabled: true, analyzed: images.length, total: images.length, faces: 12, error: null });
      case "people_list":
        return out(mockPeople());
      case "person_groups":
        return out([...mockGroups].sort((x, y) => x.name.localeCompare(y.name, "de")));
      case "person_group_create": {
        const name = String(a.name).trim();
        if (!name) throw t("Bitte einen Namen für die Gruppe eingeben");
        let g = mockGroups.find((x) => x.name.toLowerCase() === name.toLowerCase());
        if (!g) mockGroups.push((g = { id: Date.now(), name }));
        for (const p of mockPersons.filter((p) => a.persons.includes(p.id))) p.group = g.id;
        return out(g.id);
      }
      case "person_group_rename": {
        const name = String(a.name).trim();
        const other = mockGroups.find((x) => x.id !== a.id && x.name.toLowerCase() === name.toLowerCase());
        if (other) {
          for (const p of mockPersons.filter((p) => p.group === a.id)) p.group = other.id;
          mockGroups = mockGroups.filter((x) => x.id !== a.id);
          return out(other.id);
        }
        mockGroups = mockGroups.map((x) => (x.id === a.id ? { ...x, name } : x));
        return out(a.id);
      }
      case "person_group_delete":
        for (const p of mockPersons.filter((p) => p.group === a.id)) p.group = null;
        mockGroups = mockGroups.filter((x) => x.id !== a.id);
        return out(null);
      case "person_set_group":
        for (const p of mockPersons.filter((p) => a.persons.includes(p.id))) p.group = a.group;
        return out(null);
      case "person_faces":
        return out(mockFaces.filter((f) => f.personId === a.id));
      case "image_faces":
        return out(mockFaces.filter((f) => f.imageId === a.id));
      case "faces_of_images":
        return out(mockFaces.filter((f) => a.ids.includes(f.imageId)));
      case "person_rename": {
        const name: string = a.name.trim();
        const other = mockPersons.find((p) => p.id !== a.id && p.name.toLowerCase() === name.toLowerCase());
        if (other) {
          for (const f of mockFaces.filter((f) => f.personId === a.id)) {
            f.personId = other.id;
            f.personName = other.name;
          }
          return out(other.id);
        }
        const me = mockPersons.find((p) => p.id === a.id);
        if (me) me.name = name;
        for (const f of mockFaces.filter((f) => f.personId === a.id)) f.personName = name;
        return out(a.id);
      }
      case "faces_confirm": {
        const fs = mockFaces.filter((f) => a.faceIds.includes(f.id) && f.personId !== null && !f.confirmed);
        fs.forEach((f) => (f.confirmed = true));
        return out(fs.length);
      }
      case "face_confirm":
        mockFaces.filter((f) => f.id === a.faceId).forEach((f) => (f.confirmed = true));
        return out(null);
      case "face_reject":
        mockFaces.filter((f) => f.id === a.faceId).forEach((f) => ((f.personId = null), (f.personName = null), (f.confirmed = false)));
        return out(null);
      case "faces_reassign": {
        const name: string | null = a.name;
        let pid: number | null = null;
        if (name) {
          pid = mockPersons.find((p) => p.name.toLowerCase() === name.toLowerCase())?.id ?? null;
          if (pid === null) mockPersons.push({ id: (pid = mockPersons.length + 1), name });
        }
        for (const f of mockFaces.filter((f) => a.faceIds.includes(f.id))) {
          f.personId = pid;
          f.personName = name;
          f.confirmed = pid !== null;
        }
        return out(pid);
      }
      case "update_check":
        // Im Browser nur mit ?update in der Adresse (für die Gestaltung)
        return out(
          location.search.includes("update")
            ? { version: "1.1.0", current: "1.0.0", date: null, notes: "### Added\n- Masks: “Person” selects the whole person or one part.\n- “Animal” selects birds, mammals or reptiles.\n\n### Changed\n- Tone sliders now split the range like in Lightroom." }
            : null,
        );
      case "mcp_status":
      case "mcp_set_enabled":
      case "mcp_new_token": {
        if (cmd === "mcp_set_enabled") mcpOn = !!a.enabled;
        return out({ enabled: mcpOn, running: mcpOn, url: mcpOn ? "http://127.0.0.1:47823/mcp" : null, token: "3f9c1e7a5b2d4c6e8f0a1b2c3d4e5f60", command: "C:\\Program Files\\Lumenkorn\\lumenkorn.exe" });
      }
      case "update_install":
        throw t("Updates sind nur in der App verfügbar.");
      case "ai_benchmark":
        return out({ gpu: false, backend: "CPU", size: 512, seconds: 6.1, seconds24mp: 558 });
      case "mask_subject":
      case "mask_sky":
      case "develop_sample_color":
        return out([0.55, 0.12 * Math.cos(a.x * 6), 0.12 * Math.sin(a.y * 6)]);
      case "mask_person":
      case "mask_animal":
      case "mask_object":
        await new Promise((r) => setTimeout(r, 600));
        return out({ maskId: a.maskId ?? `mock-${Date.now()}`, coverage: 0.3 });
      case "denoise_start":
        throw t("Rauschreduzierung ist nur in der App verfügbar.");
      case "denoise_cancel":
      case "merge_cancel":
        return out(null);
      case "merge_start":
        throw t("Zusammenfügen ist nur in der App verfügbar.");
      case "album_suggest": {
        // Wertung aus Bewertung und einer festen Streuung, Ereignisse je Tag
        await new Promise((r) => setTimeout(r, 400));
        const list = query({ scope: a.request.scope, sort: "capturedAsc" }).filter(
          (i) => i.flag !== -1 && !(a.request.skipUneditedRaw && i.kind === "raw" && !i.edited),
        );
        const days = [...new Set(list.map((i) => (i.capturedAt ?? "").slice(0, 10)))];
        const cands = list.map((i) => ({
          id: i.id,
          aspect: (i.width ?? 3) / (i.height ?? 2),
          capturedAt: i.capturedAt,
          score: i.rating * 0.1 + ((i.id * 37) % 11) / 20,
          event: days.indexOf((i.capturedAt ?? "").slice(0, 10)),
          group: i.id,
          picked: false,
          sharpness: ((i.id * 53) % 100) / 100,
          pixels: Math.max(i.width ?? 0, i.height ?? 0) || undefined,
          ...(i.lat != null && i.lon != null
            ? { lat: i.lat, lon: i.lon, place: images.find((x) => x.id === i.id)?.location?.place ?? undefined }
            : {}),
        }));
        const best = new Set([...cands].sort((x, y) => y.score - x.score).slice(0, a.request.count).map((c) => c.id));
        for (const i of list) if (i.rating >= 4) best.add(i.id);
        for (const c of cands) c.picked = best.has(c.id);
        return out({ candidates: cands, themeUsed: true, unanalyzed: 0 });
      }
      case "album_places":
        return out(Object.fromEntries(images.filter((i) => a.ids.includes(i.id) && i.location?.place).map((i) => [i.id, i.location!.place])));
      case "album_suggest_cancel":
      case "album_export_cancel":
        return out(null);
      case "album_export":
        throw t("Das Speichern des Albums ist nur in der App verfügbar.");
      case "import_defaults":
        return out({ settings: { source: null, target: roots[0].path, deleteAfter: false }, cards: ["E:\\DCIM"] });
      case "import_scan": {
        // Drei Tage Aufnahmen, ein Teil schon importiert
        await new Promise((r) => setTimeout(r, 300));
        const days = ["2026-10-09", "2026-10-10", "2026-10-11"];
        const items = Array.from({ length: 42 }, (_, i) => {
          const day = days[Math.min(2, Math.floor(i / 14))];
          const sec = 10 * 3600 + i * 97;
          const time = [Math.floor(sec / 3600), Math.floor((sec % 3600) / 60), sec % 60].map((v) => String(v).padStart(2, "0")).join(":");
          const nr = 7860 + i;
          const raw = i % 5 !== 4;
          return {
            id: i,
            name: `DSC0${nr}.${raw ? "ARW" : "JPG"}`,
            extra: i % 7 === 0 ? [`DSC0${nr}.JPG`] : [],
            kind: raw ? "raw" : "raster",
            size: raw ? 34_000_000 : 9_000_000,
            capturedAt: `${day}T${time}`,
            dateFromFile: false,
            camera: raw ? "SONY ILCE-7M4" : "Apple iPhone 15",
            already: i < 6,
            newName: `${day.replaceAll("-", "")}_${time.replaceAll(":", "")}_${nr}.${raw ? "ARW" : "JPG"}`,
          };
        });
        return out(items);
      }
      case "import_start":
        throw t("Import ist nur in der App verfügbar.");
      case "import_estimate_shift":
        await new Promise((r) => setTimeout(r, 300));
        return out(a.camera === "SONY ILCE-7M4" ? { offsetS: 7 * 3600 + 4 * 60, matched: 31, photos: 34 } : null);
      case "import_cancel":
        return out(null);
      case "ai_set_settings":
        mockLearnStyle = a.settings?.learnStyle ?? mockLearnStyle;
        return out(null);
      case "ai_retag_all":
      case "ai_cancel_download":
        return out(null);
      case "ai_download":
        throw t("Downloads sind nur in der App verfügbar.");
      case "semantic_search":
        return out(query({ scope: a.scope, search: a.query, minRating: a.minRating, exts: a.exts, edited: a.edited }));
      case "export_conflicts":
        return out(["DSC01153.jpg", "DSC01154.jpg"]);
      case "export_images":
        throw t("Export ist nur in der App verfügbar.");
      case "nc_status":
        return out(mockNc);
      case "nc_login_start":
        setTimeout(() => {
          mockNc = { server: String(a.server).replace(/\/+$/, ""), loginName: "anna", baseFolder: "Lumenkorn" };
          mockEmit("nextcloud-login", { ok: true, error: null });
        }, 1500);
        return out(null);
      case "nc_login_cancel":
        return out(null);
      case "nc_logout":
        mockNc = null;
        mockShares.clear();
        return out(null);
      case "nc_set_base_folder":
        if (mockNc) mockNc.baseFolder = String(a.folder).trim() || "Lumenkorn";
        return out(mockNc?.baseFolder ?? "Lumenkorn");
      case "nc_project_info": {
        const p = projects.find((x) => x.id === a.projectId);
        return out({ files: 42, bytes: 42 * 3_400_000, folder: `${mockNc?.baseFolder ?? "Lumenkorn"}/${p?.name ?? "Projekt"}`, shareUrl: mockShares.get(a.projectId) ?? null });
      }
      case "nc_upload_project": {
        const total = 42;
        let done = 0;
        const base = { uploaded: 0, unchanged: 0, removed: 0, failed: [] as string[], error: null, shareUrl: null };
        const step = () => {
          done += 3;
          if (done < total) {
            mockEmit("nextcloud-progress", { ...base, done, total, current: `P10${String(100 + done)}.jpg`, uploaded: done, finished: false });
            setTimeout(step, 120);
          } else {
            const url = a.options.share ? "https://cloud.example.org/s/k7Qm2xPa9" : null;
            if (url) mockShares.set(a.projectId, url);
            else mockShares.delete(a.projectId);
            mockEmit("nextcloud-progress", { ...base, done: total, total, current: null, uploaded: 38, unchanged: 4, finished: true, shareUrl: url });
          }
        };
        setTimeout(step, 100);
        return out(null);
      }
      case "meta_get_many":
        return out(images.filter((i) => a.ids.includes(i.id)).map(metaFor));
      case "meta_set":
        for (const i of images.filter((i) => a.ids.includes(i.id))) applyMetaPatch(i, a.patch);
        return out({ changed: a.ids, failed: [] });
      case "meta_shift_time":
        for (const i of images.filter((i) => a.ids.includes(i.id))) {
          if (!i.capturedAt) continue;
          const d = new Date(i.capturedAt + "Z");
          d.setUTCSeconds(d.getUTCSeconds() + a.seconds);
          applyMetaPatch(i, { capturedAt: { set: d.toISOString().slice(0, 19) } });
        }
        return out({ changed: a.ids, failed: [] });
      case "set_rating":
        images.filter((i) => a.ids.includes(i.id)).forEach((i) => (i.rating = a.rating));
        return out(null);
      case "rejected_summary": {
        const r = images.filter((i) => a.ids.includes(i.id) && i.flag === -1);
        return out({ count: r.length, bytes: r.reduce((n, i) => n + i.size, 0), trashSupported: true });
      }
      case "rename_preview":
      case "rename_apply": {
        const o = a.options;
        const sel: Img[] = (a.ids as number[]).map((id) => images.find((i) => i.id === id)).filter((i): i is Img => !!i);
        const order = o.byCapture ? [...sel].sort((x, y) => (x.capturedAt ?? "").localeCompare(y.capturedAt ?? "")) : sel;
        // RAW+JPEG-Paare: gleicher Ordner und Namensstamm, eine Nummer
        const stemOf = (i: Img) => i.fileName.replace(/\.[^.]*$/, "");
        const groups = new Map<string, Img[]>();
        for (const i of order) {
          const k = `${i.dir}|${stemOf(i)}`;
          groups.set(k, [...(groups.get(k) ?? []), i]);
        }
        const BAD = /[<>:"/\\|?*\u0000-\u001f]/;
        const clean = (v: string) => v.replace(new RegExp(BAD.source, "g"), "-");
        const items: { id: number; old: string; new: string; conflict: string | null }[] = [];
        [...groups.values()].forEach((g, n) => {
          const c = g[0];
          const d = (c.capturedAt ?? "1970-01-01T00:00:00").padEnd(19, "0");
          let err: string | null = null;
          const stem = (o.pattern as string).replace(/\{([^{}]*)\}/g, (m, tok: string) => {
            const v: Record<string, string> = {
              name: clean(stemOf(c)), yyyy: d.slice(0, 4), mm: d.slice(5, 7), dd: d.slice(8, 10), hh: d.slice(11, 13), min: d.slice(14, 16), ss: d.slice(17, 19),
              n: String(o.start + n).padStart(o.digits, "0"), camera: clean(c.camera ?? ""), lens: clean(c.lens ?? ""), folder: clean(c.dir.split(/[\\/]/).pop() ?? ""),
            };
            if (tok.toLowerCase() in v) return v[tok.toLowerCase()];
            err = t("Unbekannter Platzhalter {tok}", { tok: `{${tok}}` });
            return m;
          });
          if (!err && !stem.trim()) err = t("Der Name ist leer");
          else if (!err && BAD.test(stem)) err = t("Ungültiges Zeichen im Namen");
          else if (!err && /[. ]$/.test(stem)) err = t("Der Name darf nicht mit Punkt oder Leerzeichen enden");
          else if (!err && /^(con|prn|aux|nul|com\d|lpt\d)(\.|$)/i.test(stem)) err = t("„{stem}“ ist ein reservierter Name", { stem });
          for (const i of g) items.push({ id: i.id, old: i.fileName, new: err ? i.fileName : stem + i.fileName.slice(stemOf(i).length), conflict: err });
        });
        // Gleiche Namen im selben Ordner
        const dirOf = (id: number) => images.find((i) => i.id === id)!.dir;
        const key = (it: { id: number; new: string }) => `${dirOf(it.id)}|${it.new.toLowerCase()}`;
        const count = new Map<string, number>();
        for (const it of items) count.set(key(it), (count.get(key(it)) ?? 0) + 1);
        for (const it of items) {
          if (it.conflict) continue;
          if ((count.get(key(it)) ?? 0) > 1) it.conflict = t("Ein anderes Foto bekäme denselben Namen");
          else if (it.new !== it.old && images.some((i) => i.dir === dirOf(it.id) && i.fileName.toLowerCase() === it.new.toLowerCase() && !items.some((x) => x.id === i.id)))
            it.conflict = t("„{name}“ gibt es schon im Ordner", { name: it.new });
        }
        if (cmd === "rename_preview") return out((a.ids as number[]).map((id) => items.find((x) => x.id === id)).filter(Boolean));
        if (items.some((x) => x.conflict)) throw t("Es gibt noch Konflikte bei den neuen Namen.");
        const renamed = [];
        for (const it of items) {
          if (it.new === it.old) continue;
          const img = images.find((i) => i.id === it.id)!;
          img.fileName = it.new;
          img.path = `${img.dir}${sep}${it.new}`;
          renamed.push({ id: img.id, path: img.path, fileName: img.fileName });
        }
        return out({ renamed });
      }
      case "delete_rejected": {
        const r = images.filter((i) => a.ids.includes(i.id) && i.flag === -1);
        for (const i of r) images.splice(images.indexOf(i), 1);
        return out({ deleted: r.map((i) => i.id), bytes: r.reduce((n, i) => n + i.size, 0), failed: [], toTrash: a.toTrash });
      }
      case "set_flag":
        images.filter((i) => a.ids.includes(i.id)).forEach((i) => (i.flag = a.flag));
        return out(null);
      case "add_tag": {
        const img = images.find((i) => i.id === a.imageId)!;
        if (!img.tags.some((tag) => tag.name.toLowerCase() === a.name.toLowerCase()))
          img.tags.unshift({ name: a.name, source: "manual", confidence: null });
        return out(img.tags);
      }
      case "remove_tag": {
        const img = images.find((i) => i.id === a.imageId)!;
        img.tags = img.tags.filter((tag) => tag.name !== a.name);
        return out(img.tags);
      }
      case "list_projects":
        return out(projects.map(withCount));
      case "create_project": {
        const p: Project = { id: Date.now(), name: a.name, description: a.description ?? "", folders: [], imageCount: 0, createdAt: 0, updatedAt: 0 };
        projects.push(p);
        return out(p);
      }
      case "update_project": {
        const p = projects.find((p) => p.id === a.id)!;
        p.name = a.name;
        p.description = a.description;
        return out(withCount(p));
      }
      case "delete_project":
        projects.splice(projects.findIndex((p) => p.id === a.id), 1);
        return out(null);
      case "add_project_folder": {
        const p = projects.find((p) => p.id === a.projectId)!;
        p.folders = p.folders.filter((f) => f.path !== a.path).concat({ path: a.path, recursive: a.recursive });
        return out(withCount(p));
      }
      case "remove_project_folder": {
        const p = projects.find((p) => p.id === a.projectId)!;
        p.folders = p.folders.filter((f) => f.path !== a.path);
        return out(withCount(p));
      }
    }
    throw t("Unbekannter Befehl {cmd}", { cmd });
  },

  listen(event: string, cb: (p: unknown) => void) {
    const set = mockListeners.get(event) ?? new Set();
    set.add(cb);
    mockListeners.set(event, set);
    return Promise.resolve(() => set.delete(cb));
  },

  /** Grobe Annäherung der Entwicklung über SVG-Filter – nur fürs UI-Design im Browser. */
  async developRender(id: number, p: DevelopParams): Promise<Blob> {
    const url = this.imageUrl("preview", id);
    const svg = decodeURIComponent(url.slice(url.indexOf(",") + 1));
    const gain = Math.pow(2, p.exposure);
    const slope = 1 + p.contrast / 150;
    const sat = p.monochrome ? 0 : 1 + p.saturation / 100 + p.vibrance / 200;
    const warm = p.temperature / 400;
    const filter = `<filter id="d"><feColorMatrix type="saturate" values="${Math.max(0, sat)}"/>
      <feComponentTransfer><feFuncR type="linear" slope="${gain * slope * (1 + warm)}" intercept="${(1 - slope) / 2}"/>
      <feFuncG type="linear" slope="${gain * slope}" intercept="${(1 - slope) / 2}"/>
      <feFuncB type="linear" slope="${gain * slope * (1 - warm)}" intercept="${(1 - slope) / 2}"/></feComponentTransfer></filter>`;
    const out = svg.replace("<defs>", `<defs>${filter}`).replace(/<rect /, '<g filter="url(#d)"><rect ').replace("</svg>", "</g></svg>");
    await new Promise((r) => setTimeout(r, 40));
    return new Blob([out], { type: "image/svg+xml" });
  },

  /** Vorschau einer Aufnahme im Import-Dialog (Platzhalter mit Farbverlauf) */
  async importPreview(id: number): Promise<Blob> {
    const url = this.imageUrl("thumb", (id % 40) + 1);
    await new Promise((r) => setTimeout(r, 20 + (id % 5) * 15));
    return new Blob([decodeURIComponent(url.slice(url.indexOf(",") + 1))], { type: "image/svg+xml" });
  },

  imageUrl(_variant: string, id: number): string {
    const img = images.find((i) => i.id === id);
    const h = img?.hue ?? 200;
    const w = img?.width ?? 1500;
    const ht = img?.height ?? 1000;
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${w} ${ht}" preserveAspectRatio="none">
      <defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="hsl(${h},45%,62%)"/><stop offset="0.6" stop-color="hsl(${(h + 30) % 360},35%,38%)"/>
        <stop offset="1" stop-color="hsl(${(h + 50) % 360},30%,16%)"/></linearGradient></defs>
      <rect width="100%" height="100%" fill="url(#g)"/>
      <path d="M0 ${ht * 0.72} L${w * 0.3} ${ht * 0.48} L${w * 0.52} ${ht * 0.66} L${w * 0.75} ${ht * 0.4} L${w} ${ht * 0.62} L${w} ${ht} L0 ${ht}Z" fill="hsl(${(h + 60) % 360},25%,12%)" opacity=".85"/>
      <circle cx="${w * 0.72}" cy="${ht * 0.22}" r="${Math.min(w, ht) * 0.07}" fill="hsl(${h},70%,88%)" opacity=".8"/></svg>`;
    return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
  },
};
