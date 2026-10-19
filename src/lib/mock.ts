// Mock-Backend für die Arbeit an der Oberfläche im normalen Browser.
// Erzeugt eine Beispielbibliothek mit Platzhalterbildern.

import { DEFAULT_PARAMS, NO_GROUPS, transferParams } from "./api";
import { t } from "./i18n";
import type { DevelopParams, Edit, Preset, FolderNode, ImageDetail, ImageQuery, ImageSummary, ImageTag, Project, Root } from "./api";

// rating/flag gehören zur Bildliste (ImageSummary), nicht zu den Details
type Img = ImageDetail & { rootId: number; hue: number; rating: number; flag: number; edit?: DevelopParams; snapshots: Edit[] };

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

function query(q: ImageQuery): ImageSummary[] {
  const words = (q.search ?? "").toLowerCase().split(/\s+/).filter(Boolean);
  let list = images.filter(
    (i) =>
      inScope(i, q) &&
      i.rating >= (q.minRating ?? 0) &&
      words.every((w) =>
        [i.fileName, i.camera ?? "", i.lens ?? "", i.location?.place ?? "", ...i.tags.map((tag) => tag.name)].some((s) => s.toLowerCase().includes(w)),
      ),
  );
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
        const { rootId, hue, edit, snapshots, ...d } = images.find((i) => i.id === a.id)!;
        return out(d);
      }
      case "develop_load": {
        const img = images.find((i) => i.id === a.id)!;
        await new Promise((r) => setTimeout(r, 400)); // RAW-Dekodierung simulieren
        return out({ params: img.edit ?? null, fullWidth: img.width, fullHeight: img.height, raw: img.ext !== "jpg", snapshots: img.snapshots, lens: img.ext !== "jpg" ? { distortion: "sony", chromatic: "sony", vignetting: "lensfun", lens: "Sony FE 24-70mm f/2.8 GM II" } : null });
      }
      case "develop_auto":
        await new Promise((r) => setTimeout(r, 300));
        return out({ exposure: 0.45, contrast: 18, highlights: -22, shadows: 15, whites: 34, blacks: -21, vibrance: 12 });
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
        return out({ runtimePath: null, runtimeDownloadable: true, modelReady: false, missing: ["vision.onnx", "text.onnx", "vocab.json", "merges.txt"], downloadSize: 161_000_000, modelDir: "(Mock)", masksReady: true, skyReady: true, partsReady: true, masksDownloadSize: 0, repairReady: false, repairDownloadSize: 92_600_000, denoiseReady: false, denoiseDownloadSize: 131_000_000, gpuRuntimes: [{ backend: "webgpu", label: "WebGPU (Vulkan)", version: "1.27.0", installed: false, downloadSize: 25_163_729 }], accel: { active: { library: null, gpuEnabled: false, gpuError: null, placements: [] }, devices: [{ vendor: "AMD", name: "Radeon RX 7900 XTX / XT / GRE", memory: 25_753_026_560, discrete: true }], restartRequired: false, develop: { enabled: true, available: true, info: { name: "AMD Radeon RX 7900 XTX (RADV NAVI31)", backend: "Vulkan", kind: "discrete", driver: "radv Mesa" }, error: null } }, crashNotice: null, downloading: false, loaded: false, error: null, analyzed: 0, total: images.length, settings: { autoTag: true, accel: "auto", developGpu: true, faces: true } });
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
      case "ai_retag_all":
      case "ai_cancel_download":
        return out(null);
      case "ai_download":
        throw t("Downloads sind nur in der App verfügbar.");
      case "semantic_search":
        return out(query({ scope: a.scope, search: a.query, minRating: a.minRating }));
      case "export_images":
        throw t("Export ist nur in der App verfügbar.");
      case "set_rating":
        images.filter((i) => a.ids.includes(i.id)).forEach((i) => (i.rating = a.rating));
        return out(null);
      case "rejected_summary": {
        const r = images.filter((i) => a.ids.includes(i.id) && i.flag === -1);
        return out({ count: r.length, bytes: r.reduce((n, i) => n + i.size, 0), trashSupported: true });
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

  listen(_event: string, _cb: (p: unknown) => void) {
    return Promise.resolve(() => {});
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
