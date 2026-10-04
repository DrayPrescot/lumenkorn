// Schnittstelle zum Rust-Kern. Läuft die Oberfläche ohne Tauri im Browser
// (`npm run dev`), springt ein Mock-Backend ein – praktisch fürs UI-Design.

import { invoke as tauriInvoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import { mock } from "./mock";

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// --- Typen (spiegeln lumenkorn-core/src/model.rs) ---------------------------

export interface Root {
  id: number;
  path: string;
  name: string;
  imageCount: number;
  lastScan: number | null;
}

export interface FolderNode {
  path: string;
  name: string;
  count: number;
  total: number;
  children: FolderNode[];
}

export interface ProjectFolder {
  path: string;
  recursive: boolean;
}

export interface Project {
  id: number;
  name: string;
  description: string;
  folders: ProjectFolder[];
  imageCount: number;
  createdAt: number;
  updatedAt: number;
}

export interface ImageSummary {
  id: number;
  path: string;
  fileName: string;
  kind: "raster" | "raw";
  width: number | null;
  height: number | null;
  capturedAt: string | null;
  rating: number;
  flag: number;
  thumbReady: boolean;
  missing: boolean;
  edited: boolean;
}

export interface RejectedSummary {
  count: number;
  bytes: number;
  trashSupported: boolean;
}

export interface DeleteReport {
  deleted: number[];
  bytes: number;
  failed: { path: string; error: string }[];
  toTrash: boolean;
}

export interface ImageTag {
  name: string;
  source: "manual" | "ai";
  confidence: number | null;
}

export interface ImageDetail {
  id: number;
  path: string;
  dir: string;
  fileName: string;
  ext: string;
  size: number;
  width: number | null;
  height: number | null;
  capturedAt: string | null;
  camera: string | null;
  lens: string | null;
  iso: number | null;
  exposure: string | null;
  aperture: number | null;
  focalLength: number | null;
  rating: number;
  flag: number;
  tags: ImageTag[];
}

export type Scope =
  | { type: "all" }
  | { type: "folder"; path: string; recursive: boolean }
  | { type: "project"; id: number }
  | { type: "person"; id: number };

export interface Person {
  id: number;
  /** null: automatisch gebildete, unbenannte Gruppe */
  name: string | null;
  faceCount: number;
  /** davon nur vorgeschlagen */
  suggested: number;
  imageCount: number;
  coverFace: number | null;
}
export interface FaceInfo {
  id: number;
  imageId: number;
  /** x, y, Breite, Höhe – normiert auf das unbearbeitete Bild */
  rect: [number, number, number, number];
  score: number;
  personId: number | null;
  personName: string | null;
  confirmed: boolean;
}
export interface PeopleStatus {
  ready: boolean;
  downloadSize: number;
  enabled: boolean;
  analyzed: number;
  total: number;
  faces: number;
  error: string | null;
}

export type SortOrder = "capturedAsc" | "capturedDesc" | "nameAsc" | "ratingDesc";

export interface ImageQuery {
  scope: Scope;
  sort?: SortOrder;
  search?: string | null;
  minRating?: number | null;
  limit?: number | null;
  offset?: number | null;
}

export interface ScanReport {
  rootId: number;
  added: number;
  updated: number;
  moved: number;
  missing: number;
  unchanged: number;
  /** Ordnerstruktur geändert (auch Ordner ohne Fotos) */
  foldersChanged: boolean;
  /** Übersprungene Bilder in nicht unterstützten Formaten: [Endung, Anzahl] */
  unsupported: [string, number][];
  /** Nicht lesbare Ordner/Dateien (z. B. fehlende Berechtigung) */
  unreadable: number;
  /** Vom Nutzer ausgelöst („Neu einlesen“) */
  requested: boolean;
}

export interface ScanProgress {
  rootId: number;
  phase: "walking" | "reading" | "writing";
  done: number;
  total: number;
}

export interface Hsl {
  hue: number[];
  saturation: number[];
  luminance: number[];
}

/** Spiegelt lumenkorn-develop/src/params.rs */
export interface DevelopParams {
  version: number;
  temperature: number;
  tint: number;
  exposure: number;
  contrast: number;
  highlights: number;
  shadows: number;
  whites: number;
  blacks: number;
  clarity: number;
  vibrance: number;
  saturation: number;
  sharpen: number;
  /** HSL je Farbbereich (Rot, Orange, Gelb, Grün, Aquamarin, Blau, Lila, Magenta), −100…100 */
  hsl: Hsl;
  rotate: number;
  flipHorizontal: boolean;
  /** Geraderichten in Grad, im Uhrzeigersinn positiv */
  straighten: number;
  crop: { x: number; y: number; w: number; h: number } | null;
  masks: MaskLayer[];
  /** KI-Rauschreduzierung: Verweis auf die zwischengespeicherte Fassung */
  denoise: { id: string; amount: number } | null;
  repairs: RepairLayer[];
}

/** Welche Teile einer Bearbeitung beim Kopieren/Übertragen mitgehen. */
export interface TransferGroups {
  /** Weißabgleich, Licht, Farbe, Klarheit, Schärfen */
  basic: boolean;
  /** Masken ohne KI-Masken */
  masks: boolean;
  /** Zuschnitt, Drehen, Spiegeln, Geraderichten */
  geometry: boolean;
}

/** Koordinaten normiert auf das aufrechte Original (vor Drehen/Zuschnitt). */
export interface PromptPoint {
  x: number;
  y: number;
  positive: boolean;
}

export interface Stroke {
  points: [number, number][];
  /** relativ zur längeren Bildkante */
  radius: number;
  feather: number;
  opacity: number;
  erase: boolean;
}

/** Pipettenklick: Stelle (normiert, Originalausrichtung) und Farbe (Oklab) */
export interface ColorSample {
  x: number;
  y: number;
  lab: [number, number, number];
}

/** Oklab → sRGB-Hex für die Anzeige gemessener Farben (Farbfelder). */
export function oklabToCss([L, a, b]: [number, number, number]): string {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const lin = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
  // Quellbilder sind linear und oft dunkel: auf die hellste Komponente normieren
  const peak = Math.max(...lin, 1e-6);
  const scale = peak > 1 ? 1 / peak : 1;
  const enc = (v: number) => {
    const x = Math.min(1, Math.max(0, v * scale));
    const g = x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055;
    return Math.round(g * 255).toString(16).padStart(2, "0");
  };
  return `#${lin.map(enc).join("")}`;
}

export type MaskShape =
  | { type: "linear"; x0: number; y0: number; x1: number; y1: number; invert?: boolean; subtract?: boolean }
  | {
      type: "radial";
      cx: number;
      cy: number;
      rx: number;
      ry: number;
      angle?: number;
      feather?: number;
      invert?: boolean;
      subtract?: boolean;
    }
  | { type: "brush"; strokes: Stroke[]; subtract?: boolean }
  | {
      /** Farbbereich (Pipette): gemessene Farben in Oklab, Toleranz 0…100 */
      type: "color";
      samples: ColorSample[];
      range: number;
      invert?: boolean;
      subtract?: boolean;
    }
  | {
      type: "raster";
      maskId: string;
      kind: "subject" | "sky" | "object";
      prompt?: PromptPoint[];
      invert?: boolean;
      subtract?: boolean;
    };

export interface LocalAdjust {
  exposure: number;
  contrast: number;
  highlights: number;
  shadows: number;
  temperature: number;
  tint: number;
  saturation: number;
  clarity: number;
}

export const NEUTRAL_LOCAL: LocalAdjust = {
  exposure: 0,
  contrast: 0,
  highlights: 0,
  shadows: 0,
  temperature: 0,
  tint: 0,
  saturation: 0,
  clarity: 0,
};

export interface MaskLayer {
  id: string;
  name: string;
  enabled: boolean;
  shapes: MaskShape[];
  adjust: LocalAdjust;
}

/** KI-Reparatur: Auswahl wie bei Masken, Ergebnis als gespeicherter Flicken. */
export interface RepairLayer {
  id: string;
  name: string;
  enabled: boolean;
  shapes: MaskShape[];
  /** ID des berechneten Flickens; null = noch nicht berechnet */
  result: string | null;
  /** Fingerabdruck der Auswahl beim Berechnen */
  resultKey: string | null;
}

export interface MaskResult {
  maskId: string;
  coverage: number;
}

export const DEFAULT_PARAMS: DevelopParams = {
  version: 1,
  temperature: 0,
  tint: 0,
  exposure: 0,
  contrast: 0,
  highlights: 0,
  shadows: 0,
  whites: 0,
  blacks: 0,
  clarity: 0,
  vibrance: 0,
  saturation: 0,
  sharpen: 0,
  hsl: { hue: [0, 0, 0, 0, 0, 0, 0, 0], saturation: [0, 0, 0, 0, 0, 0, 0, 0], luminance: [0, 0, 0, 0, 0, 0, 0, 0] },
  rotate: 0,
  flipHorizontal: false,
  straighten: 0,
  crop: null,
  masks: [],
  denoise: null,
  repairs: [],
};

/** Regler des Reiters „Basis“ (ohne KI-Rauschreduzierung). */
export const BASIC_KEYS = [
  "temperature", "tint", "exposure", "contrast", "highlights", "shadows",
  "whites", "blacks", "clarity", "vibrance", "saturation", "sharpen",
] as const;

/**
 * Spiegelt `DevelopParams::transfer_from` (params.rs): überträgt die gewählten
 * Gruppen von `src` auf `target`. KI-Rauschreduzierung, KI-Reparaturen und
 * KI-Masken hängen an Dateien des Ursprungsfotos und gehen nie mit.
 */
export function transferParams(target: DevelopParams, src: DevelopParams, g: TransferGroups): DevelopParams {
  const out: DevelopParams = JSON.parse(JSON.stringify(target));
  if (g.basic) {
    for (const k of BASIC_KEYS) out[k] = src[k];
    out.hsl = JSON.parse(JSON.stringify(src.hsl ?? DEFAULT_PARAMS.hsl));
  }
  if (g.masks) {
    // Eigene KI-Masken bleiben, die übrigen ersetzen die übertragbaren der Quelle
    const isAi = (m: MaskLayer) => m.shapes.some((s) => s.type === "raster");
    out.masks = [...out.masks.filter(isAi), ...JSON.parse(JSON.stringify(src.masks.filter((m) => !isAi(m))))];
  }
  if (g.geometry) {
    out.rotate = src.rotate;
    out.flipHorizontal = src.flipHorizontal;
    out.straighten = src.straighten;
    out.crop = src.crop ? { ...src.crop } : null;
  }
  return out;
}

export interface Edit {
  id: number;
  imageId: number;
  name: string;
  params: DevelopParams;
  isCurrent: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface DevelopInfo {
  params: DevelopParams;
  fullWidth: number;
  fullHeight: number;
  raw: boolean;
  snapshots: Edit[];
}

export interface ExportOptions {
  format: "jpeg" | "tiff16" | "png";
  quality: number;
  longEdge: number | null;
}

export interface ExportProgress {
  done: number;
  total: number;
  current: string | null;
  failed: string[];
  finished: boolean;
  dir: string;
}

export type Backend = "cpu" | "webgpu" | "directml";
export type AccelPreference = "auto" | Backend;
export interface AiSettings {
  autoTag: boolean;
  accel: AccelPreference;
  developGpu: boolean;
  faces: boolean;
}
export interface DevelopGpuStatus {
  enabled: boolean;
  /** null: Erkennung läuft noch */
  available: boolean | null;
  info: { name: string; backend: string; kind: string; driver: string } | null;
  error: string | null;
}
export interface GpuRuntime {
  backend: Backend;
  label: string;
  version: string;
  installed: boolean;
  downloadSize: number;
}
export interface GpuDevice {
  vendor: string;
  name: string;
  memory: number | null;
  discrete: boolean | null;
}
export interface AccelStatus {
  active: {
    library: Backend | null;
    gpuEnabled: boolean;
    gpuError: string | null;
    placements: { model: string; gpu: boolean }[];
  };
  devices: GpuDevice[];
  restartRequired: boolean;
  develop: DevelopGpuStatus;
}
export interface Benchmark {
  gpu: boolean;
  backend: string;
  size: number;
  seconds: number;
  seconds24mp: number;
}

export interface AiStatus {
  runtimePath: string | null;
  runtimeLoaded: boolean;
  runtimeDownloadable: boolean;
  modelReady: boolean;
  missing: string[];
  downloadSize: number;
  modelDir: string;
  masksReady: boolean;
  repairReady: boolean;
  repairDownloadSize: number;
  masksMissing: string[];
  masksDownloadSize: number;
  denoiseReady: boolean;
  denoiseDownloadSize: number;
  gpuRuntimes: GpuRuntime[];
  accel: AccelStatus;
  /** Lumenkorn ist beim letzten Start abgestürzt – was wurde abgeschaltet */
  crashNotice: string | null;
  downloading: boolean;
  loaded: boolean;
  error: string | null;
  analyzed: number;
  total: number;
  settings: AiSettings;
}

export interface AiDownloadEvent {
  progress: { item: string; index: number; count: number; done: number; total: number | null } | null;
  finished: boolean;
  error: string | null;
}

export interface AppInfo {
  version: string;
  platform: string;
  dataDir: string;
}

// --- Aufrufe ------------------------------------------------------------------

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return isTauri ? tauriInvoke<T>(cmd, args) : mock.invoke<T>(cmd, args);
}

export const api = {
  appInfo: () => call<AppInfo>("app_info"),

  listRoots: () => call<Root[]>("list_roots"),
  addRoot: (path: string) => call<Root>("add_root", { path }),
  removeRoot: (id: number) => call<void>("remove_root", { id }),
  rescanRoot: (id: number) => call<void>("rescan_root", { id }),
  folderTree: (rootId: number) => call<FolderNode>("folder_tree", { rootId }),

  queryImages: (query: ImageQuery) => call<ImageSummary[]>("query_images", { query }),
  imageDetail: (id: number) => call<ImageDetail>("image_detail", { id }),
  setRating: (ids: number[], rating: number) => call<void>("set_rating", { ids, rating }),
  setFlag: (ids: number[], flag: number) => call<void>("set_flag", { ids, flag }),
  rejectedSummary: (ids: number[]) => call<RejectedSummary>("rejected_summary", { ids }),
  deleteRejected: (ids: number[], toTrash: boolean) => call<DeleteReport>("delete_rejected", { ids, toTrash }),
  addTag: (imageId: number, name: string) => call<ImageTag[]>("add_tag", { imageId, name }),
  removeTag: (imageId: number, name: string) => call<ImageTag[]>("remove_tag", { imageId, name }),

  listProjects: () => call<Project[]>("list_projects"),
  createProject: (name: string, description = "") => call<Project>("create_project", { name, description }),
  updateProject: (id: number, name: string, description: string) =>
    call<Project>("update_project", { id, name, description }),
  deleteProject: (id: number) => call<void>("delete_project", { id }),
  addProjectFolder: (projectId: number, path: string, recursive: boolean) =>
    call<Project>("add_project_folder", { projectId, path, recursive }),
  removeProjectFolder: (projectId: number, path: string) =>
    call<Project>("remove_project_folder", { projectId, path }),

  developLoad: (id: number) => call<DevelopInfo>("develop_load", { id }),
  /** Rendert die Vorschau mit ungespeicherten Parametern und liefert sie als Bild. */
  async developRender(
    id: number,
    params: DevelopParams,
    maxEdge: number,
    overlay: number | null = null,
    overlayRepair: number | null = null,
  ): Promise<Blob> {
    if (!isTauri) return mock.developRender(id, params);
    const buf = await tauriInvoke<ArrayBuffer>("develop_render", { id, params, maxEdge, overlay, overlayRepair });
    return new Blob([buf], { type: "image/jpeg" });
  },
  /** Ausschnitt in hoher Auflösung (Zoom). `region` normiert auf das fertige Bild,
   * `scale` = Bildschirm- zu Originalpixel. */
  async developRenderDetail(
    id: number,
    params: DevelopParams,
    region: { x: number; y: number; w: number; h: number },
    scale: number,
  ): Promise<Blob> {
    if (!isTauri) return mock.developRender(id, params);
    const buf = await tauriInvoke<ArrayBuffer>("develop_render_detail", { id, params, region, scale });
    return new Blob([buf], { type: "image/jpeg" });
  },
  developDetectTilt: (id: number, rotate: number, flip: boolean) =>
    call<number | null>("develop_detect_tilt", { id, rotate, flip }),
  developSave: (ids: number[], params: DevelopParams) => call<void>("develop_save", { ids, params }),
  /** Große Bildpuffer im Backend freigeben (beim Verlassen des Entwickelns). */
  developRelease: () => call<void>("develop_release"),
  /** Gespeicherte Einstellungen eines Fotos (ohne es zu laden) */
  developParams: (id: number) => call<DevelopParams>("develop_params", { id }),
  developTransfer: (ids: number[], source: DevelopParams, groups: TransferGroups) =>
    call<void>("develop_transfer", { ids, source, groups }),
  developSnapshot: (id: number, name: string) => call<Edit[]>("develop_snapshot", { id, name }),
  developRestore: (editId: number) => call<DevelopParams>("develop_restore", { editId }),
  developDeleteSnapshot: (id: number, editId: number) =>
    call<Edit[]>("develop_delete_snapshot", { id, editId }),
  exportImages: (ids: number[], dir: string, options: ExportOptions) =>
    call<void>("export_images", { ids, dir, options }),

  aiStatus: () => call<AiStatus>("ai_status"),
  aiSetSettings: (settings: AiSettings) => call<void>("ai_set_settings", { settings }),
  aiBenchmark: () => call<Benchmark>("ai_benchmark"),
  appRestart: () => call<void>("app_restart"),
  aiDismissNotice: () => call<void>("ai_dismiss_notice"),
  peopleStatus: () => call<PeopleStatus>("people_status"),
  peopleList: () => call<Person[]>("people_list"),
  personFaces: (id: number) => call<FaceInfo[]>("person_faces", { id }),
  imageFaces: (id: number) => call<FaceInfo[]>("image_faces", { id }),
  personRename: (id: number, name: string) => call<number>("person_rename", { id, name }),
  faceAssign: (faceId: number, name: string) => call<number>("face_assign", { faceId, name }),
  faceConfirm: (faceId: number) => call<void>("face_confirm", { faceId }),
  facesConfirm: (faceIds: number[]) => call<number>("faces_confirm", { faceIds }),
  facesOfImages: (ids: number[]) => call<FaceInfo[]>("faces_of_images", { ids }),
  /** Mit Namen: diesen Gesichtern die Person zuordnen; ohne: Zuordnung lösen. */
  facesReassign: (faceIds: number[], name: string | null) => call<number | null>("faces_reassign", { faceIds, name }),
  faceReject: (faceId: number) => call<void>("face_reject", { faceId }),
  faceHide: (faceId: number) => call<void>("face_hide", { faceId }),
  personDelete: (id: number) => call<void>("person_delete", { id }),
  peopleRescan: () => call<void>("people_rescan"),
  denoiseStart: (id: number, strength = 1) => call<void>("denoise_start", { id, strength }),
  denoiseCancel: () => call<void>("denoise_cancel"),
  mergeStart: (kind: MergeKind, ids: number[], options: MergeOptions) =>
    call<void>("merge_start", { kind, ids, options }),
  mergeCancel: () => call<void>("merge_cancel"),
  repairCompute: (imageId: number, shapes: MaskShape[], denoise: DevelopParams["denoise"]) =>
    call<string>("repair_compute", { imageId, shapes, denoise }),
  repairCancel: () => call<void>("repair_cancel"),
  aiDownload: (component: "clip" | "masks" | "denoise" | "faces" | "repair" | Backend = "clip") => call<void>("ai_download", { component }),
  maskSubject: (id: number) => call<MaskResult>("mask_subject", { id }),
  maskSky: (id: number) => call<MaskResult>("mask_sky", { id }),
  developSampleColor: (id: number, x: number, y: number) =>
    call<[number, number, number]>("develop_sample_color", { id, x, y }),
  maskObject: (id: number, points: PromptPoint[], maskId: string | null) =>
    call<MaskResult>("mask_object", { id, points, maskId }),
  aiCancelDownload: () => call<void>("ai_cancel_download"),
  aiRetagAll: () => call<void>("ai_retag_all"),
  semanticSearch: (query: string, scope: Scope, minRating: number | null) =>
    call<ImageSummary[]>("semantic_search", { query, scope, minRating }),
};

// --- Zusammenfügen ------------------------------------------------------------

export type MergeKind = "hdr" | "pano" | "focus";

export interface MergeOptions {
  hdr: { align: boolean; deghost: boolean };
  pano: { projection: "cylindrical" | "spherical" | "perspective"; autoCrop: boolean; maxMegapixels: number };
  focus: { align: boolean };
}

export interface MergeProgress {
  stage: string;
  done: number;
  total: number;
  finished: boolean;
  error: string | null;
  imageId: number | null;
  fileName: string | null;
}

// --- Ereignisse ---------------------------------------------------------------

export interface Events {
  "scan-progress": ScanProgress;
  "library-changed": ScanReport;
  /** Wartung hat Katalogeinträge korrigiert – Liste neu laden */
  "catalog-updated": null;
  "thumbs-ready": { ids: number[] };
  "job-error": { message: string };
  "export-progress": ExportProgress;
  "ai-progress": { done: number; total: number; ids: number[] };
  "faces-progress": { done: number; total: number; faces: number; ids: number[] };
  "ai-download": AiDownloadEvent;
  "repair-progress": { imageId: number; done: number; total: number };
  "merge-progress": MergeProgress;
  "denoise-progress": {
    imageId: number;
    done: number;
    total: number;
    finished: boolean;
    error: string | null;
    denoiseId: string | null;
  };
}

export async function listen<K extends keyof Events>(event: K, cb: (payload: Events[K]) => void) {
  if (!isTauri) return mock.listen(event, cb as (p: unknown) => void);
  return tauriListen<Events[K]>(event, (e) => cb(e.payload));
}

// --- Bilder -------------------------------------------------------------------

let protocolBase: string | null = null;

/** URL für Thumbnail oder Vorschau. `version` umgeht den Browser-Cache nach Änderungen. */
export function faceUrl(faceId: number): string {
  if (!isTauri) return "";
  protocolBase ??= convertFileSrc("", "lw");
  return `${protocolBase}face/${faceId}`;
}

export function imageUrl(variant: "thumb" | "preview" | "full", id: number, version = 0): string {
  if (!isTauri) return mock.imageUrl(variant, id);
  // convertFileSrc kennt die plattformabhängige Form (lw://localhost/ bzw. http://lw.localhost/).
  protocolBase ??= convertFileSrc("", "lw");
  return `${protocolBase}${variant}/${id}?v=${version}`;
}

// --- Ordnerauswahl ------------------------------------------------------------

/** Öffnet den Ordnerdialog. Liefert `null`, wenn abgebrochen oder nicht verfügbar (Android). */
export async function pickFolder(title: string): Promise<string | null> {
  if (!isTauri) return null;
  try {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const res = await open({ directory: true, multiple: false, title });
    return typeof res === "string" ? res : null;
  } catch {
    return null;
  }
}
