// Zustand des Entwickeln-Modus: Parameter, Live-Vorschau, Verlauf, Speichern.

import {
  api,
  DEFAULT_PARAMS,
  listen,
  NEUTRAL_LOCAL,
  NO_GROUPS,
  type ColorGrade,
  type DevelopParams,
  type Edit,
  type GradeRange,
  type GradeWheel,
  type ExportProgress,
  type LocalAdjust,
  type MaskLayer,
  type MaskShape,
  type LensSummary,
  type Preset,
  type PromptPoint,
  type PersonPart,
  type AnimalKind,
  type RepairLayer,
  type ToneCurve,
  type TransferGroups,
  transferParams,
} from "./api";
import { lib } from "./store.svelte";
import type { ImageSummary } from "./api";
import { editAspect } from "./layout";
import { isCanceled, t } from "./i18n";
import { keymap } from "./keymap.svelte";
import type { HslMode } from "./hsl";
import { isIdentity, type CurveChannel, type CurvePoint } from "./curve";

export type MaskTool = "linear" | "radial" | "brush" | "object" | "color";
/** KI-Masken, die ohne Klick sofort berechnet werden */
export type AiMaskKind = "subject" | "sky" | `person:${PersonPart}` | `animal:${AnimalKind}`;
export type NewMaskKind = MaskTool | AiMaskKind;

export const PERSON_PARTS: [PersonPart, string][] = [
  ["person", t("Ganze Person")],
  ["face", t("Gesichtshaut")],
  ["hair", t("Haare")],
  ["eyes", t("Augen")],
  ["iris", t("Iris")],
  ["brows", t("Augenbrauen")],
  ["lips", t("Lippen")],
  ["bodySkin", t("Körperhaut")],
  ["clothes", t("Kleidung")],
];
export const ANIMAL_KINDS: [AnimalKind, string][] = [
  ["all", t("Alle Tiere")],
  ["bird", t("Vögel")],
  ["mammal", t("Säugetiere")],
  ["reptile", t("Reptilien")],
];

const MASK_NAMES: Record<MaskTool | "subject" | "sky", string> = {
  linear: t("Verlauf"),
  radial: t("Radial"),
  brush: t("Pinsel"),
  object: t("Objekt"),
  color: t("Farbe"),
  subject: t("Motiv"),
  sky: t("Himmel"),
};

/** Name einer neuen Maske bzw. Beschreibung einer KI-Form */
function maskName(kind: NewMaskKind | "object"): string {
  if (kind.startsWith("person:")) {
    const part = kind.slice(7);
    return part === "person" ? t("Person") : (PERSON_PARTS.find(([p]) => p === part)?.[1] ?? t("Person"));
  }
  if (kind.startsWith("animal:")) {
    const k = kind.slice(7);
    return k === "all" ? t("Tiere") : (ANIMAL_KINDS.find(([a]) => a === k)?.[1] ?? t("Tiere"));
  }
  return MASK_NAMES[kind as keyof typeof MASK_NAMES];
}

const isAiKind = (k: NewMaskKind): k is AiMaskKind =>
  k === "subject" || k === "sky" || k.startsWith("person:") || k.startsWith("animal:");

/** Kurzbeschreibung einer Form für die Formenliste (Masken und Reparaturen). */
export function describeShape(s: MaskShape): string {
  switch (s.type) {
    case "linear":
      return t("Verlauf");
    case "radial":
      return t("Radial");
    case "brush":
      return t(s.strokes.length === 1 ? "Pinsel ({n} Strich)" : "Pinsel ({n} Striche)", { n: s.strokes.length });
    case "color":
      return t(s.samples.length === 1 ? "Farbe ({n} Ton)" : "Farbe ({n} Töne)", { n: s.samples.length });
    case "raster":
      if (s.kind === "object") return t((s.prompt?.length ?? 0) === 1 ? "Objekt ({n} Punkt)" : "Objekt ({n} Punkte)", { n: s.prompt?.length ?? 0 });
      return maskName(s.kind);
  }
}

const same = (a: DevelopParams, b: DevelopParams) => JSON.stringify(a) === JSON.stringify(b);
/** Obergrenze für den Rückgängig-Verlauf (Zeichen der JSON-Darstellung, grob 50 MB) */
const UNDO_MAX_CHARS = 50_000_000;
const clone = (p: DevelopParams): DevelopParams => JSON.parse(JSON.stringify(p));
/** Serialisierte Standardwerte: Vergleiche „unverändert/Standard“ laufen über Zeichenketten */
const DEFAULT_JSON = JSON.stringify(DEFAULT_PARAMS);

/** Übertragen wird ausschließlich der Reiter „Basis“ (ohne Rauschreduzierung). */
const BASIC_ONLY: TransferGroups = { ...NO_GROUPS, basic: true };

class Develop {
  /** Bild, das gerade entwickelt wird. */
  imageId = $state<number | null>(null);
  params = $state<DevelopParams>(clone(DEFAULT_PARAMS));
  raw = $state(false);
  /** Farbtemperatur des Kamera-Weißabgleichs; dann zeigt der Weißabgleich Kelvin */
  asShotKelvin = $state<number | null>(null);
  /** Korrekturdaten der Kamera für das geöffnete Foto */
  lensInfo = $state<LensSummary | null>(null);
  fullSize = $state<[number, number]>([0, 0]);
  snapshots = $state<Edit[]>([]);

  /** Objekt-URL der aktuellen Vorschau. */
  url = $state<string | null>(null);
  beforeUrl = $state<string | null>(null);
  showBefore = $state(false);
  /** Vorher-Knopf gedrückt gehalten (kurzer Vergleich) */
  holdBefore = $state(false);
  loading = $state(false);

  /** Zielgröße der Vorschau (Bühne × Pixeldichte). */
  maxEdge = $state(1600);

  /** Kopierte Basis-Einstellungen (Strg+Umschalt+C) und ihre Herkunft */
  clipboard = $state<{ params: DevelopParams; name: string } | null>(null);

  // --- Rauschreduzierung --------------------------------------------------
  /** Laufende Berechnung: Bild und Fortschritt */
  denoising = $state<{ imageId: number; done: number; total: number } | null>(null);
  /** Stärke für die nächste Berechnung, 0…100; 50 = geschätztes Rauschen */
  denoiseStrength = $state(50);

  /** Reiter im Entwickeln-Bereich */
  tab = $state<"basic" | "masks" | "repair" | "crop">("basic");

  // --- Reparatur ---------------------------------------------------------
  /** Laufende Berechnung: Index der Reparatur und Fortschritt */
  repairing = $state<{ index: number; done: number; total: number } | null>(null);

  /** Masken-Werkzeuge arbeiten im Reiter „Reparatur“ auf den Reparaturen. */
  get editsRepairs(): boolean {
    return this.tab === "repair";
  }

  /** Ebenen, auf die sich Auswahl und Werkzeuge beziehen */
  get list(): (MaskLayer | RepairLayer)[] {
    return this.editsRepairs ? this.params.repairs : this.params.masks;
  }

  // --- Zuschnitt ---------------------------------------------------------
  /** Zuschnitt-Werkzeug aktiv: Anzeige ohne Zuschnitt, Rahmen bearbeiten. */
  cropMode = $state(false);
  /** Seitenverhältnis (Breite/Höhe in Pixeln) oder null = frei */
  cropAspect = $state<number | null>(null);
  #cropBefore: DevelopParams["crop"] = null;
  #straightenBefore = 0;

  // --- Masken -----------------------------------------------------------
  /** Index der ausgewählten Maskenebene. */
  activeMask = $state<number | null>(null);
  /** Werkzeug, mit dem auf dem Bild gearbeitet wird. */
  tool = $state<MaskTool | null>(null);
  brush = $state({ size: 0.03, feather: 0.6, erase: false });
  showOverlay = $state(false);
  /** Laufende KI-Berechnung, z. B. „Motiv wird erkannt …“ */
  maskBusy = $state<string | null>(null);
  exportProgress = $state<ExportProgress | null>(null);

  // Verlauf als JSON-Zeichenketten: Die Länge ist die exakte Größe, Pinsel-
  // masken mit tausenden Punkten belegen als Objekte ein Vielfaches, und
  // geparst wird nur bei Rückgängig/Wiederholen.
  #undo: string[] = [];
  #redo: string[] = [];
  /** Letzter Verlaufsschritt (JSON) */
  #committedJson = DEFAULT_JSON;
  /** Zuletzt gespeicherter Stand (JSON); "" erzwingt das nächste Speichern */
  #savedJson = DEFAULT_JSON;
  #saving: Promise<unknown> = Promise.resolve();
  #lastOpen = -Infinity;
  #inFlight = false;
  /** Nummer der neuesten Vorschau-Anfrage: Beim Fotowechsel ruft open()
   *  renderNow() direkt auf, während die vorige noch läuft – deren Antwort
   *  darf die neuere weder überschreiben noch `#inFlight` zurücksetzen. */
  #renderSeq = 0;
  #pending = false;
  #saveTimer: ReturnType<typeof setTimeout> | undefined;
  #exportTimer: ReturnType<typeof setTimeout> | undefined;
  #loadToken = 0;
  /** Zuletzt angefordertes Bild – kann noch auf das Speichern warten, während
   *  `imageId` schon wieder auf das vorige zeigt */
  #target: number | null = null;
  /** Für welches Bild `params` geladen sind. Während ein neues Bild lädt,
   *  stehen dort noch die Werte des vorigen – nie speichern oder rendern. */
  // Reaktiv: Knöpfe mit `disabled={!dev.ready …}` müssen freigeschaltet
  // werden, sobald die Parameter geladen sind
  #paramsFor = $state<number | null>(null);
  #beforeSeq = 0;

  /** Parameter des geöffneten Bildes sind geladen. */
  get ready(): boolean {
    return this.imageId !== null && this.#paramsFor === this.imageId;
  }

  /** Letzter Verlaufsschritt entspricht den Standardwerten. Bewusst nicht aus
   *  den Live-Parametern abgeleitet: Das würde bei jedem Pinselpunkt alle
   *  Parameter serialisieren. */
  isDefault = $state(true);
  /** Erhöht sich mit jedem Verlaufsschritt (auch Öffnen, Rückgängig) – für
   *  Ableitungen, die nicht bei jedem Pinselpunkt neu rechnen sollen. */
  committedVersion = $state(0);

  #setCommitted(json: string) {
    this.#committedJson = json;
    this.isDefault = json === DEFAULT_JSON;
    this.committedVersion++;
  }

  constructor() {
    // Ein KI-Agent hat die Bearbeitung des geöffneten Fotos geändert:
    // übernehmen (mit Rückgängig)
    listen("develop-external", async ({ ids }) => {
      const id = this.imageId;
      if (id === null || !this.ready || !ids.includes(id)) return;
      // Nicht vorher speichern – das überschriebe die Änderung des Agenten
      clearTimeout(this.#saveTimer);
      try {
        const p = await api.developParams(id);
        if (this.imageId === id) this.replace(p);
      } catch (e) {
        lib.notify(String(e), "error");
      }
    });
    listen("repair-progress", (e) => {
      if (this.repairing && e.imageId === this.imageId) this.repairing = { ...this.repairing, done: e.done, total: e.total };
    });
    listen("denoise-progress", async (e) => {
      if (!e.finished) {
        this.denoising = { imageId: e.imageId, done: e.done, total: e.total };
        return;
      }
      this.denoising = null;
      if (e.error) {
        if (!isCanceled(e.error)) lib.notify(t("Rauschreduzierung: {error}", { error: e.error }), "error");
        return;
      }
      if (!e.denoiseId) return;
      if (this.imageId === e.imageId && this.ready) {
        this.params.denoise = { id: e.denoiseId, amount: this.params.denoise?.amount ?? 100 };
        this.schedule();
        this.commit();
      } else {
        // Inzwischen anderes Bild offen: direkt speichern
        try {
          // Nur die Einstellungen lesen – das RAW nicht dekodieren
          const saved = { ...clone(DEFAULT_PARAMS), ...(await api.developParams(e.imageId)) };
          const denoise = { id: e.denoiseId, amount: saved.denoise?.amount ?? 100 };
          await this.#saving;
          await api.developSave([e.imageId], { ...saved, denoise });
        } catch (err) {
          lib.notify(String(err), "error");
        }
      }
      lib.notify(t("Rauschreduzierung fertig."));
    });

    listen("export-progress", (p) => {
      this.exportProgress = p;
      // Ein neuer Export darf nicht vom Ausblenden des alten getroffen werden
      clearTimeout(this.#exportTimer);
      if (p.finished) {
        const ok = p.total - p.failed.length;
        const skipped = p.skipped
          ? " " + t(p.skipped === 1 ? "{n} vorhandene Datei übersprungen." : "{n} vorhandene Dateien übersprungen.", { n: p.skipped })
          : "";
        lib.notify(
          (p.failed.length
            ? t("{ok} von {total} exportiert. Fehlgeschlagen: {failed}", { ok, total: p.total, failed: p.failed.join("; ") })
            : t(ok === 1 ? "{n} Foto exportiert nach {dir}" : "{n} Fotos exportiert nach {dir}", { n: ok, dir: p.dir })) + skipped,
          p.failed.length ? "error" : "info",
        );
        this.#exportTimer = setTimeout(() => (this.exportProgress = null), 1500);
      }
    });
  }

  /** Öffnet ein Bild zur Bearbeitung. Vorherige Änderungen werden vorher gespeichert. */
  async open(id: number) {
    // Mit dem Ziel vergleichen, nicht mit `imageId`: Bei → und sofort ←
    // wartet open(B) noch auf das Speichern; open(A) muss es überholen.
    if (this.#target === id) return;
    this.#target = id;
    if (this.cropMode) this.applyCrop();
    // Token vor dem Warten auf das Speichern holen: Wird inzwischen ein
    // weiteres Bild geöffnet, gewinnt dieses (nicht das zuletzt fertige).
    const token = ++this.#loadToken;
    await this.flush();
    if (token !== this.#loadToken) return;
    this.#dropPreview();
    this.imageId = id;
    this.#paramsFor = null;
    this.loading = true;
    this.#undo = [];
    this.#redo = [];
    this.activeMask = null;
    this.tool = null;
    this.showOverlay = false;
    this.cropMode = false;
    this.revoke();
    try {
      // Schnelles Durchblättern (Pfeiltaste gehalten): Jeder Ladevorgang
      // dekodiert ein RAW (Sekunden, bis 1,5 GB). Erst laden, wenn kurz
      // nicht weitergeblättert wurde – sonst nur das, wo man ankommt.
      const now = performance.now();
      const rapid = now - this.#lastOpen < 400;
      this.#lastOpen = now;
      if (rapid) {
        await new Promise((r) => setTimeout(r, 150));
        if (token !== this.#loadToken) return;
      }
      const info = await api.developLoad(id);
      if (token !== this.#loadToken) return;
      const p = { ...clone(DEFAULT_PARAMS), ...(info.params ?? {}) };
      p.masks = (p.masks ?? []).map((m) => ({ ...m, adjust: { ...NEUTRAL_LOCAL, ...m.adjust } }));
      p.repairs = (p.repairs ?? []).map((r) => ({ ...r, result: r.result ?? null, resultKey: r.resultKey ?? null }));
      this.params = p;
      this.#paramsFor = id;
      const json = JSON.stringify(p);
      this.#setCommitted(json);
      this.#savedJson = json;
      this.raw = info.raw;
      this.asShotKelvin = info.asShotKelvin ?? null;
      this.lensInfo = info.lens ?? null;
      this.fullSize = [info.fullWidth, info.fullHeight];
      this.snapshots = info.snapshots;
      await this.renderNow();
      this.renderBefore();
    } catch (e) {
      if (token === this.#loadToken) lib.notify(t("Bild kann nicht entwickelt werden: {error}", { error: String(e) }), "error");
    } finally {
      if (token === this.#loadToken) this.loading = false;
    }
  }

  close() {
    this.#dropPreview();
    const wasOpen = this.imageId !== null;
    // Erst speichern, dann die großen Puffer im Backend freigeben
    const saved = this.flush();
    const token = ++this.#loadToken;
    this.#target = null;
    this.imageId = null;
    this.#paramsFor = null;
    this.revoke();
    this.#undo = [];
    this.#redo = [];
    // Nicht freigeben, wenn inzwischen wieder ein Bild geöffnet wurde
    if (wasOpen)
      Promise.resolve(saved).finally(() => {
        if (token === this.#loadToken) api.developRelease().catch(() => {});
      });
  }

  revoke() {
    if (this.url) URL.revokeObjectURL(this.url);
    if (this.beforeUrl) URL.revokeObjectURL(this.beforeUrl);
    this.url = null;
    this.beforeUrl = null;
  }

  // --- Parameter ändern ------------------------------------------------------

  // --- HSL ------------------------------------------------------------------

  /** Welche Reglergruppe das HSL-Werkzeug zeigt (und die Pipette verändert). */
  hslMode = $state<HslMode>("hue");
  /** Pipette aktiv: auf dem Bild klicken und nach oben/unten ziehen. */
  hslPick = $state(false);

  /** Einen Farbbereich setzen (immer als neues Objekt). */
  setHsl(mode: HslMode, band: number, value: number) {
    const cur = this.params.hsl ?? DEFAULT_PARAMS.hsl;
    const arr = [...cur[mode]];
    arr[band] = Math.round(Math.max(-100, Math.min(100, value)));
    this.params.hsl = { ...$state.snapshot(cur), [mode]: arr } as DevelopParams["hsl"];
    this.schedule();
  }

  /** Im Color Grading ausgewähltes Farbrad (die Regler darunter gelten ihm) */
  gradeRange = $state<GradeRange>("midtones");

  /** Color Grading: ein Feld eines Farbrads oder Überblenden/Abgleich setzen. */
  setGrade(range: GradeRange | null, field: keyof GradeWheel | "blending" | "balance", value: number) {
    const cur = $state.snapshot(this.params.colorGrade ?? DEFAULT_PARAMS.colorGrade) as ColorGrade;
    if (range && field !== "blending" && field !== "balance") {
      cur[range] = { ...cur[range], [field]: value };
    } else if (field === "blending" || field === "balance") {
      cur[field] = value;
    }
    this.params.colorGrade = cur;
    this.schedule();
  }

  /** In der Gradationskurve gewählter Kanal */
  curveChannel = $state<CurveChannel>("rgb");

  /** Punkte einer Kurve setzen (live; `commit()` beim Loslassen). Die
   * Gerade aus den beiden Endpunkten wird leer gespeichert (= unverändert);
   * Ankerpunkte auf der Diagonalen bleiben, bis ein anderer Punkt sich bewegt. */
  setCurve(ch: CurveChannel, pts: CurvePoint[]) {
    const cur = $state.snapshot(this.params.toneCurve ?? DEFAULT_PARAMS.toneCurve) as ToneCurve;
    cur[ch] = pts.length <= 2 && isIdentity(pts) ? [] : pts.map(([x, y]) => [x, y] as [number, number]);
    this.params.toneCurve = cur;
    this.schedule();
  }

  setCurvePreserve(on: boolean) {
    const cur = $state.snapshot(this.params.toneCurve ?? DEFAULT_PARAMS.toneCurve) as ToneCurve;
    this.params.toneCurve = { ...cur, preserveColors: on };
    this.schedule();
    this.commit();
  }

  /** Mehrere Bereiche auf einmal (Pipette): `base` + `delta` × Anteil. */
  setHslWeighted(mode: HslMode, base: number[], weights: number[], delta: number) {
    const cur = $state.snapshot(this.params.hsl ?? DEFAULT_PARAMS.hsl) as DevelopParams["hsl"];
    const arr = base.map((b, i) => (weights[i] > 0 ? Math.round(Math.max(-100, Math.min(100, b + delta * weights[i]))) : cur[mode][i]));
    this.params.hsl = { ...cur, [mode]: arr };
    this.schedule();
  }

  /** Wert setzen (live, z. B. beim Ziehen eines Reglers); `commit()` folgt beim Loslassen. */
  set<K extends keyof DevelopParams>(key: K, value: DevelopParams[K]) {
    this.params[key] = value;
    this.schedule();
  }

  /** Ende einer Änderung (Regler losgelassen): Verlaufsschritt anlegen. */
  commit() {
    if (!this.ready) return;
    // Nur einmal serialisieren: Vergleich, Verlauf und Standard-Prüfung nutzen dieselbe Zeichenkette
    const json = JSON.stringify($state.snapshot(this.params));
    if (json === this.#committedJson) return;
    this.#undo.push(this.#committedJson);
    // Verlauf nach Anzahl und Gesamtgröße begrenzen (älteste zuerst);
    // der jüngste Schritt bleibt immer erhalten.
    let total = 0;
    for (const e of this.#undo) total += e.length;
    while (this.#undo.length > 1 && (this.#undo.length > 200 || total > UNDO_MAX_CHARS)) {
      total -= this.#undo[0].length;
      this.#undo.shift();
    }
    this.#redo = [];
    this.#setCommitted(json);
    this.scheduleSave();
  }

  replace(p: DevelopParams) {
    this.params = clone(p);
    this.schedule();
    this.commit();
  }

  reset() {
    this.replace(DEFAULT_PARAMS);
  }

  undo() {
    const prev = this.#undo.pop();
    if (!prev) return;
    this.#redo.push(JSON.stringify($state.snapshot(this.params)));
    this.params = JSON.parse(prev) as DevelopParams;
    this.#setCommitted(prev);
    this.schedule();
    this.scheduleSave();
  }

  redo() {
    const next = this.#redo.pop();
    if (!next) return;
    this.#undo.push(JSON.stringify($state.snapshot(this.params)));
    this.params = JSON.parse(next) as DevelopParams;
    this.#setCommitted(next);
    this.schedule();
    this.scheduleSave();
  }

  rotate(dir: 1 | -1) {
    this.params.rotate = (this.params.rotate + 4 + dir) % 4;
    this.schedule();
    this.commit();
    this.renderBefore();
  }

  // --- Rendern ---------------------------------------------------------------

  /** Rendert so oft wie nötig, aber nie mehr als eine Anfrage gleichzeitig. */
  schedule() {
    if (this.#inFlight) {
      this.#pending = true;
      return;
    }
    this.renderNow();
  }

  async renderNow() {
    const id = this.imageId;
    if (id === null || !this.ready) return;
    this.#inFlight = true;
    const seq = ++this.#renderSeq;
    try {
      const params = $state.snapshot(this.params) as DevelopParams;
      // Beim Zuschneiden das ganze Bild zeigen; der Rahmen wird darübergelegt.
      if (this.cropMode) params.crop = null;
      const sel = this.showOverlay && this.activeMask !== null ? this.activeMask : null;
      const overlay = this.editsRepairs ? null : sel;
      const overlayRepair = this.editsRepairs ? sel : null;
      const blob = await api.developRender(id, params, this.maxEdge, overlay, overlayRepair);
      if (id !== this.imageId || seq !== this.#renderSeq) return;
      const old = this.url;
      this.url = URL.createObjectURL(blob);
      if (old) setTimeout(() => URL.revokeObjectURL(old), 1000);
    } catch (e) {
      if (seq === this.#renderSeq) lib.notify(String(e), "error");
    } finally {
      // Eine überholte Anfrage lässt die neuere weiterlaufen
      if (seq === this.#renderSeq) {
        this.#inFlight = false;
        if (this.#pending) {
          this.#pending = false;
          this.renderNow();
        }
      }
    }
  }

  /** Ausschnitt in hoher Auflösung für die Zoomansicht (Objekt-URL, der Aufrufer
   *  muss sie mit `URL.revokeObjectURL` freigeben); `before`: unbearbeitet wie `renderBefore` */
  async detailUrl(r: { x: number; y: number; w: number; h: number }, scale: number, before = false): Promise<string | null> {
    const id = this.imageId;
    if (id === null) return null;
    const params = before
      ? { ...clone(DEFAULT_PARAMS), rotate: this.params.rotate, flipHorizontal: this.params.flipHorizontal }
      : ($state.snapshot(this.params) as DevelopParams);
    if (this.cropMode) params.crop = null;
    const blob = await api.developRenderDetail(id, params, r, scale);
    if (id !== this.imageId) return null;
    return URL.createObjectURL(blob);
  }

  /** Vorher-Ansicht: Standardparameter, aber mit derselben Geometrie. */
  async renderBefore() {
    const id = this.imageId;
    if (id === null || !this.ready) return;
    const seq = ++this.#beforeSeq;
    const p = { ...clone(DEFAULT_PARAMS), rotate: this.params.rotate, flipHorizontal: this.params.flipHorizontal };
    let blob: Blob;
    try {
      blob = await api.developRender(id, p, this.maxEdge);
    } catch (e) {
      // Wird ohne await aufgerufen – Fehler melden statt als unbehandelte
      // Ablehnung zu verlieren (veraltete Anfragen bleiben still)
      if (id === this.imageId && seq === this.#beforeSeq) lib.notify(String(e), "error");
      return;
    }
    // Nur die neueste Anfrage zählt (schnelles Drehen)
    if (id !== this.imageId || seq !== this.#beforeSeq) return;
    if (this.beforeUrl) URL.revokeObjectURL(this.beforeUrl);
    this.beforeUrl = URL.createObjectURL(blob);
  }

  // --- Rauschreduzierung -----------------------------------------------------

  async startDenoise() {
    if (this.imageId === null) return;
    try {
      this.denoising = { imageId: this.imageId, done: 0, total: 0 };
      // Reglerwert 0–100; das Backend rechnet ihn in die Rauschstärke um
      await api.denoiseStart(this.imageId, this.denoiseStrength);
    } catch (e) {
      this.denoising = null;
      lib.notify(String(e), "error");
    }
  }

  removeDenoise() {
    this.params.denoise = null;
    this.schedule();
    this.commit();
  }

  // --- Zuschnitt -------------------------------------------------------------

  enterCrop() {
    if (this.imageId === null || this.cropMode) return;
    this.#cropBefore = $state.snapshot(this.params.crop) as DevelopParams["crop"];
    this.#straightenBefore = this.params.straighten;
    this.tool = null;
    this.activeMask = null;
    this.showOverlay = false;
    this.cropMode = true;
    this.schedule();
  }

  applyCrop() {
    if (!this.cropMode) return;
    const c = this.params.crop;
    // Voller Rahmen ist kein Zuschnitt
    if (c && c.x <= 1e-4 && c.y <= 1e-4 && c.w >= 0.9999 && c.h >= 0.9999) this.params.crop = null;
    this.cropMode = false;
    this.schedule();
    this.commit();
  }

  cancelCrop() {
    if (!this.cropMode) return;
    this.params.crop = this.#cropBefore;
    this.params.straighten = this.#straightenBefore;
    this.cropMode = false;
    this.schedule();
  }

  resetCrop() {
    this.params.crop = null;
    this.params.straighten = 0;
    this.schedule();
    if (!this.cropMode) this.commit();
  }

  /** Größter zentrierter Ausschnitt mit dem Seitenverhältnis innerhalb des Rahmens. */
  setAspect(aspect: number | null, frame: [number, number]) {
    this.cropAspect = aspect;
    if (aspect === null) return;
    const [fw, fh] = frame;
    const c = this.params.crop ?? { x: 0, y: 0, w: 1, h: 1 };
    // Mittelpunkt behalten, so groß wie möglich
    const cx = c.x + c.w / 2;
    const cy = c.y + c.h / 2;
    let w = c.w;
    let h = (w * fw) / (fh * aspect);
    if (h > c.h) {
      h = c.h;
      w = (h * fh * aspect) / fw;
    }
    const x = Math.min(Math.max(0, cx - w / 2), 1 - w);
    const y = Math.min(Math.max(0, cy - h / 2), 1 - h);
    this.params.crop = { x, y, w, h };
    this.schedule();
  }

  async autoStraighten() {
    const id = this.imageId;
    if (id === null) return;
    this.maskBusy = t("Horizont wird gesucht …");
    try {
      const fix = await api.developDetectTilt(id, this.params.rotate, this.params.flipHorizontal);
      // Inzwischen anderes Foto geöffnet: Ergebnis gehört nicht hierher
      if (this.imageId !== id || !this.ready) return;
      if (fix === null) lib.notify(t("Keine eindeutigen geraden Kanten gefunden."));
      else {
        this.params.straighten = Math.round(fix * 100) / 100;
        this.straightenFeedback();
        this.schedule();
        if (!this.cropMode) this.commit();
      }
    } catch (e) {
      lib.notify(String(e), "error");
    } finally {
      // Nur zurücksetzen, wenn es noch dieses Foto betrifft
      if (this.imageId === id) this.maskBusy = null;
    }
  }

  // --- Masken ---------------------------------------------------------------

  get layer(): MaskLayer | RepairLayer | null {
    return this.activeMask !== null ? (this.list[this.activeMask] ?? null) : null;
  }

  /** Aktive Maskenebene (nicht Reparatur) – für die Regler der lokalen Anpassung */
  get maskLayer(): MaskLayer | null {
    return this.editsRepairs || this.activeMask === null ? null : (this.params.masks[this.activeMask] ?? null);
  }

  /** Neue Maskenebene. Geometrische Masken werden danach auf dem Bild gezogen. */
  async addMask(kind: NewMaskKind) {
    if (this.imageId === null) return;
    const list = this.list;
    let layer: MaskLayer | RepairLayer;
    if (this.editsRepairs) {
      layer = {
        id: `r${Date.now().toString(36)}`,
        name: t("Reparatur {n}", { n: list.length + 1 }),
        enabled: true,
        shapes: [],
        result: null,
        resultKey: null,
      };
    } else {
      const base = maskName(kind);
      const n = this.params.masks.filter((m) => m.name.startsWith(base)).length;
      layer = {
        id: `m${Date.now().toString(36)}`,
        name: n ? `${base} ${n + 1}` : base,
        enabled: true,
        shapes: [],
        adjust: { ...NEUTRAL_LOCAL },
      };
    }
    if (isAiKind(kind)) {
      const shape = await this.#runAi(kind);
      // Inzwischen anderes Foto, Rückgängig o. Ä.: Ergebnis verwerfen
      if (!shape || this.list !== list) return;
      layer.shapes.push(shape);
      this.tool = null;
    } else {
      this.tool = kind;
      this.#newObject = kind === "object";
      this.#newColor = kind === "color";
      this.pendingSubtract = false;
    }
    list.push(layer);
    this.activeMask = list.length - 1;
    this.showOverlay = true;
    this.schedule();
    this.commit();
  }

  /** Weitere Form zur aktiven Ebene hinzufügen oder davon abziehen. */
  async addShape(kind: NewMaskKind, subtract: boolean) {
    const layer = this.layer;
    if (!layer) return;
    if (isAiKind(kind)) {
      const shape = await this.#runAi(kind);
      if (!shape || !this.list.includes(layer)) return;
      layer.shapes.push({ ...shape, subtract });
      this.schedule();
      this.commit();
    } else {
      this.tool = kind;
      this.pendingSubtract = subtract;
      this.#newObject = kind === "object";
      this.#newColor = kind === "color";
      if (kind === "brush") this.brush.erase = false;
    }
    this.showOverlay = true;
    this.schedule();
  }

  /** Neue Formen der aktiven Ebene werden abgezogen statt hinzugefügt. */
  pendingSubtract = $state(false);
  /** Nächster Objekt-Klick beginnt eine neue Form statt die letzte zu verfeinern. */
  #newObject = false;
  /** Nächster Pipetten-Klick beginnt eine neue Farbform. */
  #newColor = false;

  async #runAi(kind: AiMaskKind): Promise<MaskShape | null> {
    if (this.imageId === null) return null;
    const id = this.imageId;
    const person = kind.startsWith("person:") ? (kind.slice(7) as PersonPart) : null;
    const animal = kind.startsWith("animal:") ? (kind.slice(7) as AnimalKind) : null;
    this.maskBusy =
      kind === "subject" ? t("Motiv wird erkannt …")
      : kind === "sky" ? t("Himmel wird gesucht …")
      : person ? t("Personen werden gesucht …")
      : t("Tiere werden gesucht …");
    try {
      const r =
        kind === "subject" ? await api.maskSubject(id)
        : kind === "sky" ? await api.maskSky(id)
        : person ? await api.maskPerson(id, person)
        : await api.maskAnimal(id, animal!);
      if (this.imageId !== id) return null;
      // Teile wie Augen oder Iris sind winzig – nur „gar nichts“ melden
      const empty = person || animal ? r.coverage <= 0 : r.coverage < 0.005;
      if (empty) {
        lib.notify(
          kind === "sky" ? t("Kein Himmel gefunden.")
          : kind === "subject" ? t("Kein Motiv gefunden.")
          : person ? (["person", "hair", "bodySkin", "clothes"].includes(person) ? t("Keine Person gefunden.") : t("Kein Gesicht gefunden."))
          : t("Kein passendes Tier gefunden."),
        );
      }
      return { type: "raster", maskId: r.maskId, kind };
    } catch (e) {
      lib.notify(String(e), "error");
      return null;
    } finally {
      this.maskBusy = null;
    }
  }

  selectMask(index: number | null) {
    this.activeMask = index;
    const shapes = index !== null ? this.list[index]?.shapes : undefined;
    this.tool = null;
    if (shapes?.length) {
      const last = shapes[shapes.length - 1];
      if (last.type === "brush") this.tool = "brush";
      if (last.type === "raster" && last.kind === "object") this.tool = "object";
      if (last.type === "color") this.tool = "color";
    }
    this.pendingSubtract = false;
    // Reparatur ohne Ergebnis: Auswahl rot zeigen
    if (this.editsRepairs && index !== null && !(this.list[index] as RepairLayer).result) this.showOverlay = true;
    if (this.showOverlay) this.schedule();
  }

  removeMask(index: number) {
    this.list.splice(index, 1);
    if (this.activeMask !== null) {
      if (index === this.activeMask) this.activeMask = null;
      else if (index < this.activeMask) this.activeMask--;
    }
    this.tool = null;
    this.schedule();
    this.commit();
  }

  toggleOverlay() {
    this.showOverlay = !this.showOverlay;
    this.schedule();
  }

  setLocal<K extends keyof LocalAdjust>(key: K, value: number) {
    const layer = this.maskLayer;
    if (!layer) return;
    layer.adjust[key] = value;
    // Sobald eine Wirkung eingestellt wird, die Überlagerung ausblenden – man
    // will das Ergebnis sehen.
    if (this.showOverlay) this.showOverlay = false;
    this.schedule();
  }

  /** Klickpunkt für „Objekt“ setzen und die Maske neu berechnen lassen. */
  async objectClick(point: PromptPoint) {
    const layer = this.layer;
    if (!layer || this.imageId === null) return;
    const subtract = this.pendingSubtract;
    const last = layer.shapes.length - 1;
    const idx = !this.#newObject && last >= 0 && layer.shapes[last].type === "raster" && (layer.shapes[last] as { kind: string }).kind === "object" ? last : -1;
    const prev = idx >= 0 ? (layer.shapes[idx] as Extract<MaskShape, { type: "raster" }>) : null;
    const points = [...(prev?.prompt ?? []), point];
    this.maskBusy = prev ? t("Auswahl wird verfeinert …") : t("Objekt wird erkannt …");
    const id = this.imageId;
    try {
      const r = await api.maskObject(id, points, prev?.maskId ?? null);
      // Inzwischen anderes Foto oder Ebene entfernt: Ergebnis verwerfen
      if (this.imageId !== id || !this.list.includes(layer)) return;
      const shape: MaskShape = { type: "raster", kind: "object", maskId: r.maskId, prompt: points, subtract: prev?.subtract ?? subtract };
      if (idx >= 0) layer.shapes[idx] = shape;
      else layer.shapes.push(shape);
      this.#newObject = false;
      this.showOverlay = true;
      this.schedule();
      this.commit();
    } catch (e) {
      lib.notify(String(e), "error");
    } finally {
      this.maskBusy = null;
    }
  }

  /**
   * Pipette: Farbe an der Stelle messen und zur Farbmaske hinzufügen.
   * `remove` (Alt-Klick) entfernt stattdessen die nächstgelegene Farbe.
   */
  async colorClick(x: number, y: number, remove: boolean) {
    const layer = this.layer;
    const id = this.imageId;
    if (!layer || id === null) return;
    const last = layer.shapes.length - 1;
    const cur = !this.#newColor && last >= 0 && layer.shapes[last].type === "color"
      ? (layer.shapes[last] as Extract<MaskShape, { type: "color" }>)
      : null;
    if (remove) {
      if (!cur || !cur.samples.length) return;
      let best = 0;
      cur.samples.forEach((s, i) => {
        const b = cur.samples[best];
        if ((s.x - x) ** 2 + (s.y - y) ** 2 < (b.x - x) ** 2 + (b.y - y) ** 2) best = i;
      });
      cur.samples.splice(best, 1);
      if (!cur.samples.length) layer.shapes.splice(last, 1);
      this.schedule();
      this.commit();
      return;
    }
    try {
      const lab = await api.developSampleColor(id, x, y);
      if (this.imageId !== id) return;
      const sample = { x, y, lab };
      if (cur) cur.samples.push(sample);
      else layer.shapes.push({ type: "color", samples: [sample], range: 30, subtract: this.pendingSubtract });
      this.#newColor = false;
      this.showOverlay = true;
      this.schedule();
      this.commit();
    } catch (e) {
      lib.notify(String(e), "error");
    }
  }

  // --- Reparatur -------------------------------------------------------------

  /** Fingerabdruck einer Auswahl – erkennt Änderungen nach dem Berechnen. */
  repairKey(shapes: MaskShape[]): string {
    const str = JSON.stringify(shapes);
    let h = 2166136261;
    for (let i = 0; i < str.length; i++) h = Math.imul(h ^ str.charCodeAt(i), 16777619);
    return (h >>> 0).toString(36) + str.length.toString(36);
  }

  /** Reparatur mit LaMa berechnen (volle Auflösung). */
  async computeRepair(index: number) {
    const r = this.params.repairs[index];
    const id = this.imageId;
    if (!r || id === null || !r.shapes.length || this.repairing) return;
    this.tool = null;
    this.repairing = { index, done: 0, total: 0 };
    const shapes = $state.snapshot(r.shapes) as MaskShape[];
    try {
      // Auf dem Bild, wie es angezeigt wird (mit oder ohne Objektivkorrektur)
      const lens = this.params.lensCorrection && !!this.lensInfo;
      const result = await api.repairCompute(id, shapes, $state.snapshot(this.params.denoise) as DevelopParams["denoise"], lens);
      if (this.imageId !== id) return;
      const cur = this.params.repairs.find((x) => x.id === r.id);
      if (!cur) return;
      cur.result = result;
      cur.resultKey = this.repairKey(shapes);
      cur.lens = lens;
      cur.enabled = true;
      this.showOverlay = false;
      this.schedule();
      this.commit();
      this.flush();
    } catch (e) {
      if (!isCanceled(e)) lib.notify(t("Reparatur: {error}", { error: String(e) }), "error");
    } finally {
      this.repairing = null;
    }
  }

  // --- Speichern -------------------------------------------------------------

  scheduleSave() {
    clearTimeout(this.#saveTimer);
    this.#saveTimer = setTimeout(() => this.flush(), 500);
  }

  /** Vor dem Schließen des Fensters: offenen Zuschnitt übernehmen und alles
   *  schreiben, auch einen bereits laufenden Speichervorgang abwarten. */
  async flushForClose() {
    if (this.cropMode) this.applyCrop();
    await this.flush();
    await this.#saving;
  }

  async flush() {
    clearTimeout(this.#saveTimer);
    const id = this.imageId;
    if (id === null || !this.ready) return;
    // Eine Preset-Vorschau wird nie gespeichert
    const p = this.#hoverBase ?? ($state.snapshot(this.params) as DevelopParams);
    const json = JSON.stringify(p);
    if (json === this.#savedJson) return;
    this.#savedJson = json;
    // Speichervorgänge strikt nacheinander: Die Befehle laufen im Backend
    // parallel, ein älterer Stand dürfte sonst einen neueren überschreiben.
    const run = this.#saving.then(() => api.developSave([id], p));
    this.#saving = run.catch(() => {});
    try {
      await run;
      const edited = json !== DEFAULT_JSON;
      // Nur bei Änderung neu zuweisen – sonst rechnet das Raster bei jedem Speichern neu
      const im = lib.images.find((i) => i.id === id);
      const aspect = im && edited ? editAspect(im.width, im.height, p) : undefined;
      const reshaped = !!im && Math.abs((im.aspect ?? 0) - (aspect ?? 0)) > 1e-4;
      if (im && (im.edited !== edited || reshaped)) {
        lib.patchImages([id], { edited, aspect });
        // Gedreht oder zugeschnitten: Raster und Filmstreifen neu anordnen
        if (reshaped) lib.layoutVersion++;
      }
    } catch (e) {
      // Beim nächsten Anlass erneut versuchen
      if (this.imageId === id) this.#savedJson = "";
      lib.notify(t("Speichern fehlgeschlagen: {error}", { error: String(e) }), "error");
    }
  }

  // --- Kopieren / Einfügen (nur Basis) ---------------------------------------
  //
  // Bewusst ohne Knöpfe: Strg+Umschalt+C / Strg+Umschalt+V oder Rechtsklick.
  // Übertragen wird nur der Reiter „Basis“ (Weißabgleich, Licht, Farbe, HSL,
  // Klarheit, Schärfen) – nie Masken, Reparaturen, Entrauschen oder Zuschnitt.

  /** Basis-Einstellungen eines Fotos kopieren (im Entwickeln: der aktuelle Stand). */
  async copyBasic(id: number) {
    try {
      const params =
        this.imageId === id && this.ready
          ? clone($state.snapshot(this.params) as DevelopParams)
          : { ...clone(DEFAULT_PARAMS), ...(await api.developParams(id)) };
      const name = lib.images.find((i) => i.id === id)?.fileName ?? t("Foto");
      this.clipboard = { params, name };
      lib.notify(t("Basis-Einstellungen von {name} kopiert", { name }));
    } catch (e) {
      lib.notify(String(e), "error");
    }
  }

  /** Kopierte Basis-Einstellungen auf die Fotos `ids` übertragen. */
  async pasteBasic(ids: number[]) {
    const clip = this.clipboard;
    if (!clip) {
      lib.notify(t("Zuerst Basis-Einstellungen kopieren ({key}).", { key: keymap.hint("copySettings") }));
      return;
    }
    if (!ids.length) return;
    if (!(await this.#transfer(ids, clip.params, BASIC_ONLY))) return;
    lib.notify(t(ids.length === 1 ? "Basis-Einstellungen auf {n} Foto übertragen" : "Basis-Einstellungen auf {n} Fotos übertragen", { n: ids.length }));
  }

  /** Teile von `src` auf die Fotos `ids` übertragen; beim geöffneten Foto mit
   *  Rückgängig, bei den übrigen im Backend. */
  async #transfer(ids: number[], src: DevelopParams, groups: TransferGroups): Promise<boolean> {
    this.previewPreset(null);
    // Vor jedem Warten festlegen, was wohin geht (Bildwechsel währenddessen)
    const openId = this.ready ? this.imageId : null;
    const others = ids.filter((i) => i !== openId);
    if (openId !== null && ids.includes(openId)) {
      this.replace(transferParams($state.snapshot(this.params) as DevelopParams, src, groups)); // mit Rückgängig
      await this.flush();
    }
    if (others.length) {
      try {
        // Nach laufenden Speichervorgängen – sonst überschreibt ein älterer Stand das Einfügen
        await this.#saving;
        await api.developTransfer(others, src, groups);
      } catch (e) {
        lib.notify(String(e), "error");
        return false;
      }
      const set = new Set(others);
      // Über das Protokoll der Bibliothek, damit eine gleichzeitige Listenabfrage nicht überschreibt
      if (groups.geometry) {
        const changes = new Map<number, Partial<ImageSummary>>();
        for (const i of lib.images) if (set.has(i.id)) changes.set(i.id, { edited: true, aspect: editAspect(i.width, i.height, src) });
        lib.patchImages([...changes.keys()], changes);
      } else lib.patchImages(others, { edited: true });
      if (groups.geometry) lib.layoutVersion++;
    }
    return true;
  }

  /** „Auto“ rechnet gerade */
  autoBusy = $state(false);

  /** Beim Geraderichten: feines Raster im Zuschnitt, bis kurz nach der letzten Änderung */
  straightening = $state(false);
  #straightenTimer: ReturnType<typeof setTimeout> | undefined;
  straightenFeedback() {
    this.straightening = true;
    clearTimeout(this.#straightenTimer);
    this.#straightenTimer = setTimeout(() => (this.straightening = false), 1200);
  }

  /** „Auto“: Belichtung, Kontrast, Lichter, Tiefen, Weiß, Schwarz und Dynamik
   *  vorschlagen lassen und übernehmen (mit Rückgängig). */
  async autoTone() {
    const id = this.imageId;
    if (id === null || !this.ready || this.autoBusy) return;
    this.autoBusy = true;
    try {
      const a = await api.developAuto(id, $state.snapshot(this.params) as DevelopParams);
      if (this.imageId !== id) return;
      // `style` ist nur ein Hinweis, kein Regler
      const { style, ...values } = a;
      void style;
      Object.assign(this.params, values);
      this.schedule();
      this.commit();
    } catch (e) {
      lib.notify(String(e), "error");
    } finally {
      this.autoBusy = false;
    }
  }

  /** Automatischer Weißabgleich (mit Rückgängig) */
  async autoWhiteBalance() {
    const id = this.imageId;
    if (id === null || !this.ready || this.autoBusy) return;
    this.autoBusy = true;
    try {
      const wb = await api.developAutoWb(id, $state.snapshot(this.params) as DevelopParams);
      if (this.imageId !== id) return;
      this.params.temperature = wb.temperature;
      this.params.tint = wb.tint;
      this.params.kelvin = wb.kelvin;
      this.schedule();
      this.commit();
    } catch (e) {
      lib.notify(String(e), "error");
    } finally {
      this.autoBusy = false;
    }
  }

  /** Schwarzweiß ein/aus (mit Rückgängig) */
  setMonochrome(on: boolean) {
    if (!this.ready || (this.params.monochrome ?? false) === on) return;
    this.params.monochrome = on;
    this.schedule();
    this.commit();
  }

  /** Objektivkorrektur ein/aus – sofort gespeichert, weil KI-Masken und
   *  Geraderichten im Backend den gespeicherten Stand nutzen. */
  setLens(on: boolean) {
    if (this.params.lensCorrection === on) return;
    this.params.lensCorrection = on;
    this.schedule();
    this.commit();
    this.flush();
  }

  // --- Presets ---------------------------------------------------------------

  presets = $state<Preset[]>([]);
  #presetsLoaded = false;
  /** Stand vor der Vorschau eines Presets (Maus über dem Eintrag) */
  #hoverBase: DevelopParams | null = null;
  #hoverTimer: ReturnType<typeof setTimeout> | undefined;

  async loadPresets(force = false) {
    if (this.#presetsLoaded && !force) return;
    this.#presetsLoaded = true;
    try {
      this.presets = await api.presetList();
    } catch (e) {
      this.#presetsLoaded = false;
      lib.notify(String(e), "error");
    }
  }

  /** Gruppen, in denen eine Bearbeitung vom Standard abweicht – Vorauswahl
   *  beim Anlegen eines Presets. */
  changedGroups(p: DevelopParams): TransferGroups {
    const d = DEFAULT_PARAMS;
    const differs = (keys: (keyof DevelopParams)[]) => keys.some((k) => JSON.stringify(p[k]) !== JSON.stringify(d[k]));
    return {
      ...NO_GROUPS,
      whiteBalance: differs(["temperature", "tint", "kelvin"]),
      light: differs(["exposure", "contrast", "highlights", "shadows", "whites", "blacks"]),
      color: differs(["vibrance", "saturation", "monochrome"]),
      hsl: differs(["hsl"]),
      colorGrade: differs(["colorGrade"]),
      details: differs(["clarity", "dehaze", "sharpen"]),
      masks: p.masks.some((m) => !m.shapes.some((s) => s.type === "raster")),
      geometry: differs(["crop", "straighten", "rotate", "flipHorizontal"]),
    };
  }

  /** Offener Preset-Dialog: neues Preset aus einem Foto, oder Verwalten */
  presetDialog = $state<{ mode: "create"; name: string; params: DevelopParams } | { mode: "manage" } | null>(null);

  /** „Neues Preset aus diesem Foto …“: beim geöffneten Foto der aktuelle
   *  (noch nicht gespeicherte) Stand, sonst die gespeicherte Bearbeitung. */
  async openCreatePreset(id: number, name: string) {
    this.previewPreset(null);
    let params: DevelopParams;
    try {
      params = this.ready && this.imageId === id ? clone($state.snapshot(this.params) as DevelopParams) : await api.developParams(id);
    } catch (e) {
      lib.notify(String(e), "error");
      return;
    }
    if (same(params, DEFAULT_PARAMS)) {
      lib.notify(t("Dieses Foto ist unbearbeitet – es gibt nichts für ein Preset."));
      return;
    }
    this.presetDialog = { mode: "create", name, params };
  }

  async createPreset(name: string, groups: TransferGroups, params: DevelopParams) {
    try {
      this.presets = await api.presetCreate(name, params, { ...groups, basic: false });
      lib.notify(t("Preset „{name}“ gespeichert", { name: name.trim() }));
      return true;
    } catch (e) {
      lib.notify(String(e), "error");
      return false;
    }
  }

  async renamePreset(p: Preset, name: string) {
    if (!name.trim() || name.trim() === p.name) return;
    try {
      this.presets = await api.presetRename(p.id, name);
    } catch (e) {
      lib.notify(String(e), "error");
    }
  }

  async deletePreset(p: Preset) {
    if (!confirm(t("Preset „{name}“ löschen? Fotos, auf die es angewendet wurde, bleiben unverändert.", { name: p.name }))) return;
    try {
      this.presets = await api.presetDelete(p.id);
    } catch (e) {
      lib.notify(String(e), "error");
    }
  }

  /** Preset auf Fotos anwenden (Standard: das geöffnete bzw. die Auswahl). */
  async applyPreset(p: Preset, ids: number[]) {
    if (!ids.length) return;
    if (!(await this.#transfer(ids, p.params, { ...p.groups, basic: false }))) return;
    lib.notify(t(ids.length === 1 ? "Preset „{name}“ angewendet" : "Preset „{name}“ auf {n} Fotos angewendet", { name: p.name, n: ids.length }));
  }

  /** Vorschau verwerfen, ohne den Stand zurückzuschreiben (Fotowechsel). */
  #dropPreview() {
    clearTimeout(this.#hoverTimer);
    this.#hoverBase = null;
  }

  /** Wirkung eines Presets im geöffneten Foto zeigen, ohne es anzuwenden
   *  (`null` stellt den Stand davor wieder her). */
  previewPreset(p: Preset | null) {
    clearTimeout(this.#hoverTimer);
    if (!p) {
      if (this.#hoverBase) {
        this.params = this.#hoverBase;
        this.#hoverBase = null;
        this.schedule();
      }
      return;
    }
    if (!this.ready) return;
    // Kurz warten: Beim Überfahren der Liste nicht jeden Eintrag rendern
    this.#hoverTimer = setTimeout(() => {
      this.#hoverBase ??= clone($state.snapshot(this.params) as DevelopParams);
      this.params = transferParams(this.#hoverBase, p.params, { ...p.groups, basic: false });
      this.schedule();
    }, 120);
  }

  // --- Schnappschüsse --------------------------------------------------------

  async snapshot(name: string) {
    if (this.imageId === null) return;
    await this.flush();
    if (this.isDefault) {
      lib.notify(t("Ohne Bearbeitung gibt es nichts festzuhalten."));
      return;
    }
    const id = this.imageId;
    const list = await api.developSnapshot(id, name);
    if (this.imageId === id) this.snapshots = list;
  }

  async restore(edit: Edit) {
    if (!this.ready) return;
    const id = this.imageId;
    await this.flush();
    await this.#saving;
    const p = await api.developRestore(edit.id);
    // Inzwischen anderes Foto geöffnet: Stand nicht übernehmen
    if (this.imageId !== id || !this.ready) return;
    this.replace({ ...clone(DEFAULT_PARAMS), ...p });
    this.#savedJson = JSON.stringify(this.params);
  }

  async deleteSnapshot(edit: Edit) {
    if (this.imageId === null) return;
    const id = this.imageId;
    const list = await api.developDeleteSnapshot(id, edit.id);
    if (this.imageId === id) this.snapshots = list;
  }
}

export const dev = new Develop();
