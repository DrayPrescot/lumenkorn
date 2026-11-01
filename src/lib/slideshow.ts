// Diashow: Datenmodell, Zeitleiste und Bewegung (Ken Burns).
//
// Die Auswahl der Fotos kommt aus demselben Backend wie beim Fotoalbum
// (album/select.rs). Hier wird daraus eine Folge von Dias mit Dauer,
// Übergängen und Beschriftung, bei einem Ortswechsel mit einer Karte
// dazwischen. Was zu einem Zeitpunkt zu sehen ist, berechnet `frameAt` allein
// aus der Zeit – so laufen Vorschau, Abspielen, Musik und Videoexport auf
// derselben Zeitleiste.

import type { Scope } from "./api";
import { focusPosition, shortPlace, type AlbumCandidate, type AlbumSuggestion, type CaptionMode, type PhotoSource, type ThemeKey } from "./album";
import { snapToBeats, type BeatInfo } from "./beats";
import { distanceKm, type MapLeg } from "./slideMap";
import { locale } from "./i18n";

export type Transition = "fade" | "slide" | "none";

export interface SlideshowSettings {
  theme: ThemeKey;
  customTheme: string;
  count: number;
  photos: PhotoSource;
  /** Anzeigedauer je Foto in Sekunden (samt halbem Übergang davor und danach) */
  seconds: number;
  transition: Transition;
  /** Langsames Zoomen und Schwenken */
  kenBurns: boolean;
  /** Datum und Ort einblenden, wo ein neuer Tag bzw. Ort beginnt */
  captions: CaptionMode;
  /** Animierte Karte, wenn sich der Aufnahmeort deutlich ändert */
  maps: boolean;
}

export const DEFAULT_SLIDESHOW: SlideshowSettings = {
  theme: "vacation",
  customTheme: "",
  count: 60,
  photos: "editedJpeg",
  seconds: 5,
  transition: "fade",
  kenBurns: true,
  captions: "both",
  maps: true,
};

export const MIN_SECONDS = 1.5;
export const MAX_SECONDS = 30;
/** Dauer eines Übergangs (s), höchstens die Hälfte des kürzeren Dias */
export const TRANSITION_S = 1;
/** Ein- und Ausblenden am Anfang und Ende (s) */
const EDGE_FADE_S = 1;
/** Hochformat (schmaler als dieser Anteil des Bildformats): ganz zeigen statt beschneiden */
const CONTAIN_BELOW = 0.8;
/** Stärke des Ken-Burns-Zooms */
const ZOOM = 0.12;
/** Dauer der Karte beim Ortswechsel (s) */
export const MAP_SECONDS = 4;
/** Ab dieser Entfernung zum vorigen Aufnahmeort kommt eine Karte (km) */
export const MAP_MIN_KM = 50;

export interface Slide {
  id: number;
  /** Eigene Dauer (s); sonst die der Einstellungen */
  seconds?: number;
  /** Eingeblendeter Text (Datum/Ort oder eigener Text) */
  caption?: string;
  /** Automatische Beschriftung (wird bei Änderung der Einstellung ersetzt) */
  auto?: boolean;
  /** Ganzes Foto zeigen (mit unscharfem Hintergrund) bzw. bildfüllend; ohne Angabe nach Seitenverhältnis */
  fit?: "cover" | "contain";
  /** Karte vor diesem Foto ein- bzw. ausschalten; ohne Angabe nach Entfernung */
  map?: boolean;
}

/** Hintergrundmusik */
export interface SlideshowMusic {
  /** Pfad der Datei (in der Browser-Vorschau nur der Name) */
  path: string;
  name: string;
  /** Erkannter Takt (gespeichert, damit nicht bei jedem Öffnen neu gerechnet wird) */
  beats: BeatInfo;
  /** Bildwechsel auf den Takt legen */
  sync: boolean;
  /** Dauer der Dias so wählen, dass die Diashow mit der Musik endet */
  fit: boolean;
  /** Lautstärke 0…1 */
  volume: number;
}

export interface SlideshowState {
  version: 1;
  settings: SlideshowSettings;
  scope: Scope;
  /** Titel, groß über dem ersten Dia (leer: ohne) */
  title: string;
  candidates: AlbumCandidate[];
  slides: Slide[];
  music?: SlideshowMusic;
  themeUsed: boolean;
  unanalyzed: number;
}

/** Gespeicherte Einstellungen mit Vorgaben ergänzen und in gültige Bereiche bringen */
export function slideshowDefaults(s: Partial<SlideshowSettings>): SlideshowSettings {
  const out = { ...DEFAULT_SLIDESHOW, ...s };
  out.seconds = clampSeconds(out.seconds);
  if (!["fade", "slide", "none"].includes(out.transition)) out.transition = "fade";
  return out;
}

export function clampSeconds(v: number): number {
  return Number.isFinite(v) ? Math.min(MAX_SECONDS, Math.max(MIN_SECONDS, v)) : DEFAULT_SLIDESHOW.seconds;
}

// --- Erstellen ---------------------------------------------------------------

function dayOf(c: AlbumCandidate | undefined): string {
  return c?.capturedAt?.slice(0, 10) ?? "";
}

function formatDay(day: string): string {
  const [y, m, d] = day.split("-").map(Number);
  if (!y || !m || !d) return "";
  return new Date(y, m - 1, d).toLocaleDateString(locale, { day: "numeric", month: "long", year: "numeric" });
}

/**
 * Beschriftet die Dias: Wo ein neuer Tag bzw. Ort beginnt, wird Datum
 * und/oder Ort eingeblendet. Frühere automatische Texte werden ersetzt,
 * eigene bleiben.
 */
export function autoCaptions(slides: Slide[], mode: CaptionMode, byId: Map<number, AlbumCandidate>): Slide[] {
  let prev = "";
  return slides.map((s) => {
    const own = s.caption !== undefined && !s.auto;
    const c = byId.get(s.id);
    const day = dayOf(c);
    const place = shortPlace(c?.place);
    const key = mode === "date" ? day : mode === "place" ? place : `${day}|${place}`;
    const fresh = key.replace("|", "") !== "" && key !== prev;
    // Fehlt die Angabe bei einem Foto, gilt die vorige weiter
    if (key.replace("|", "")) prev = key;
    if (own) return s;
    const { caption: _c, auto: _a, ...rest } = s;
    if (mode === "none" || !fresh) return rest;
    const parts = [mode !== "place" ? formatDay(day) : "", mode !== "date" ? place : ""].filter(Boolean);
    return parts.length ? { ...rest, caption: parts.join(" · "), auto: true } : rest;
  });
}

/** Neue Diashow aus dem Vorschlag des Backends */
export function buildSlideshow(settings: SlideshowSettings, scope: Scope, title: string, s: AlbumSuggestion): SlideshowState {
  const byId = new Map(s.candidates.map((c) => [c.id, c]));
  const slides = s.candidates.filter((c) => c.picked).map((c) => ({ id: c.id }));
  return {
    version: 1,
    settings,
    scope,
    title,
    candidates: s.candidates,
    slides: autoCaptions(slides, settings.captions, byId),
    themeUsed: s.themeUsed,
    unanalyzed: s.unanalyzed,
  };
}

/**
 * Noch nicht verwendete Fotos als Ersatz bzw. zum Hinzufügen: zuerst aus
 * derselben Serie, dann aus demselben Ereignis, jeweils nach Wertung.
 */
export function slideAlternatives(a: SlideshowState, ref: { group?: number; events: number[] }, limit = 60): AlbumCandidate[] {
  const used = new Set(a.slides.map((s) => s.id));
  const rank = (c: AlbumCandidate) => (c.group === ref.group ? 0 : ref.events.includes(c.event) ? 1 : 2);
  return a.candidates
    .filter((c) => !used.has(c.id))
    .sort((x, y) => rank(x) - rank(y) || y.score - x.score)
    .slice(0, limit);
}

// --- Zeitleiste ----------------------------------------------------------------

/** Eingestellte Dauer eines Dias (ohne Anpassung an die Musik) */
export function slideSeconds(a: SlideshowState, s: Slide): number {
  return clampSeconds(s.seconds ?? a.settings.seconds);
}

/** Flug vom vorigen Aufnahmeort zum Ort des Dias (Index → Flug), soweit eine Karte kommt */
export function mapLegs(a: SlideshowState, byId: Map<number, AlbumCandidate>): Map<number, MapLeg> {
  const legs = new Map<number, MapLeg>();
  let prev: AlbumCandidate | undefined;
  a.slides.forEach((s, j) => {
    const c = byId.get(s.id);
    if (c?.lat == null || c.lon == null) return;
    if (prev) {
      const from = { lat: prev.lat!, lon: prev.lon!, label: shortPlace(prev.place) };
      const to = { lat: c.lat, lon: c.lon, label: shortPlace(c.place) };
      const km = distanceKm(from, to);
      const want = s.map ?? (a.settings.maps && km >= MAP_MIN_KM);
      // Am selben Ort ergibt ein Flug keinen Sinn, auch wenn eingeschaltet
      if (want && km >= 1) legs.set(j, { from, to });
    }
    prev = c;
  });
  return legs;
}

/** Abschnitt der Zeitleiste: ein Foto oder die Karte vor einem Foto */
export type Entry = { kind: "photo"; slide: number } | { kind: "map"; slide: number; leg: MapLeg };

/** Abschnitte in Reihenfolge */
export function entriesOf(a: SlideshowState, byId: Map<number, AlbumCandidate>): Entry[] {
  const legs = mapLegs(a, byId);
  const out: Entry[] = [];
  a.slides.forEach((_, j) => {
    const leg = legs.get(j);
    if (leg) out.push({ kind: "map", slide: j, leg });
    out.push({ kind: "photo", slide: j });
  });
  return out;
}

/**
 * Gewünschte Dauer je Abschnitt: Karten fest, Fotos mit eigener oder
 * eingestellter Dauer; mit „an die Musik anpassen“ teilen sich die Fotos ohne
 * eigene Dauer die übrige Länge des Stücks.
 */
export function baseDurations(a: SlideshowState, entries: Entry[]): number[] {
  let def = a.settings.seconds;
  const m = a.music;
  if (m?.fit && m.beats.duration > 0) {
    let fixed = 0;
    let rest = 0;
    for (const e of entries) {
      const s = a.slides[e.slide];
      if (e.kind === "map") fixed += MAP_SECONDS;
      else if (s.seconds !== undefined) fixed += clampSeconds(s.seconds);
      else rest++;
    }
    if (rest) def = (m.beats.duration - fixed) / rest;
  }
  return entries.map((e) => (e.kind === "map" ? MAP_SECONDS : clampSeconds(a.slides[e.slide].seconds ?? def)));
}

export interface Timeline {
  entries: Entry[];
  /** Beginn jedes Abschnitts (s) */
  starts: number[];
  total: number;
  /** Längster Übergang (s) */
  transition: number;
  /** Abschnitt jedes Fotos */
  photoEntry: number[];
}

/**
 * Beginn jedes Abschnitts und Gesamtdauer. Der Übergang liegt mittig auf der
 * Grenze zwischen zwei Abschnitten – mit Musik genau auf dem Schlag.
 */
export function timeline(a: SlideshowState, byId = new Map(a.candidates.map((c) => [c.id, c]))): Timeline {
  const entries = entriesOf(a, byId);
  const photoEntry: number[] = [];
  entries.forEach((e, i) => {
    if (e.kind === "photo") photoEntry[e.slide] = i;
  });
  const d = baseDurations(a, entries);
  const info = a.music?.beats;
  if (a.music?.sync && info && info.bpm > 0 && info.beats.length >= 4) {
    // Übergang höchstens einen Schlag lang, sonst verschwimmt der Schnitt
    return { entries, photoEntry, ...snapToBeats(d, info, MIN_SECONDS * 0.8), transition: Math.min(TRANSITION_S, 60 / info.bpm) };
  }
  const starts: number[] = [];
  let t = 0;
  for (const x of d) {
    starts.push(t);
    t += x;
  }
  return { entries, photoEntry, starts, total: t, transition: TRANSITION_S };
}

/** Tatsächliche Dauer des Abschnitts `i` */
export function durationOf(tl: Timeline, i: number): number {
  return (i + 1 < tl.starts.length ? tl.starts[i + 1] : tl.total) - tl.starts[i];
}

/** Tatsächliche Dauer des Fotos `j` (ohne Karte davor) */
export function photoDuration(tl: Timeline, j: number): number {
  return durationOf(tl, tl.photoEntry[j]);
}

/** Index des Abschnitts zur Zeit `t` */
export function slideAt(starts: number[], t: number): number {
  let lo = 0;
  let hi = starts.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (starts[mid] <= t) lo = mid;
    else hi = mid - 1;
  }
  return Math.max(0, lo);
}

/** Ein sichtbares Dia zu einem Zeitpunkt */
export interface Layer {
  index: number;
  id: number;
  opacity: number;
  /** Verschiebung beim Übergang „Schieben“ (Anteil der Breite, −1…1) */
  shift: number;
  /** Ken Burns: Vergrößerung und Verschiebung (Anteil der Größe) */
  scale: number;
  tx: number;
  ty: number;
  /** Bildfüllend mit Ausschnitt (object-position) oder ganz */
  contain: boolean;
  pos: [number, number];
  /** Beschriftung mit Deckkraft */
  caption?: string;
  captionOpacity: number;
  /** Karte statt Foto: Flug und Fortschritt 0…1 */
  map?: { leg: MapLeg; p: number };
}

export interface Frame {
  layers: Layer[];
  /** Deckkraft des Titels */
  title: number;
  /** Abdunkeln am Anfang und Ende (0: Bild, 1: schwarz) */
  black: number;
}

const smooth = (x: number) => {
  const v = Math.min(1, Math.max(0, x));
  return v * v * (3 - 2 * v);
};

/** Pseudozufall 0…1 aus einer Zahl (gleich bei jedem Abspielen) */
function hash01(n: number): number {
  const x = Math.sin(n * 12.9898 + 78.233) * 43758.5453;
  return x - Math.floor(x);
}

/** Ganzes Foto zeigen? (ausdrücklich oder Hochformat in breitem Bild) */
export function isContain(s: Slide, c: AlbumCandidate | undefined, frameAspect: number): boolean {
  if (s.fit) return s.fit === "contain";
  return (c?.aspect ?? 1.5) < frameAspect * CONTAIN_BELOW;
}

/**
 * Ken Burns für ein Dia: abwechselnd hinein- und herauszoomen, mit Gesichtern
 * zu ihnen hin, sonst in eine (gleichbleibend zufällige) Richtung.
 * `p`: Fortschritt 0…1 über die sichtbare Zeit.
 */
export function kenBurns(index: number, c: AlbumCandidate | undefined, contain: boolean, p: number): { scale: number; tx: number; ty: number } {
  const zoomIn = index % 2 === 0;
  const k = zoomIn ? p : 1 - p;
  if (contain) return { scale: 1 + (ZOOM / 2) * k, tx: 0, ty: 0 };
  const scale = 1 + ZOOM * k;
  const max = (scale - 1) / 2;
  let dx: number;
  let dy: number;
  if (c?.focus) {
    // Den Gesichtern entgegen (sie rücken zur Mitte)
    dx = (0.5 - c.focus[0]) * 2;
    dy = (0.5 - c.focus[1]) * 2;
  } else {
    const a = hash01(c?.id ?? index) * Math.PI * 2;
    dx = Math.cos(a);
    dy = Math.sin(a) * 0.6;
  }
  const clamp = (v: number) => Math.max(-1, Math.min(1, v));
  return { scale, tx: clamp(dx) * max, ty: clamp(dy) * max };
}

/** Was zur Zeit `t` zu sehen ist. `frameAspect`: Breite/Höhe der Bildfläche. */
export function frameAt(a: SlideshowState, tl: Timeline, t: number, frameAspect: number, byId: Map<number, AlbumCandidate>): Frame {
  const { starts, total, entries } = tl;
  const n = entries.length;
  if (!n) return { layers: [], title: 0, black: 1 };
  const tr = a.settings.transition;
  const i = slideAt(starts, t);
  const layers: Layer[] = [];
  // Halbe Dauer des Übergangs vor Dia `j` (höchstens die Hälfte beider Dias)
  const halfOf = (j: number) =>
    tr === "none" || j <= 0 || j >= n ? 0 : Math.min(tl.transition, durationOf(tl, j) / 2, durationOf(tl, j - 1) / 2) / 2;
  const make = (j: number, opacity: number, shift: number): Layer => {
    const e = entries[j];
    const s = a.slides[e.slide];
    const c = byId.get(s.id);
    const start = starts[j];
    const dur = durationOf(tl, j);
    // Sichtbar von halbem Übergang vor bis halbem Übergang nach dem Dia
    const from = start - halfOf(j);
    const to = start + dur + halfOf(j + 1);
    const p = (t - from) / Math.max(0.001, to - from);
    if (e.kind === "map") {
      return {
        index: -1 - e.slide,
        id: s.id,
        opacity,
        shift,
        scale: 1,
        tx: 0,
        ty: 0,
        contain: false,
        pos: [0.5, 0.5],
        captionOpacity: 0,
        map: { leg: e.leg, p: Math.min(1, Math.max(0, (t - start) / Math.max(0.001, dur))) },
      };
    }
    const contain = isContain(s, c, frameAspect);
    const kb = a.settings.kenBurns ? kenBurns(e.slide, c, contain, Math.min(1, Math.max(0, p))) : { scale: 1, tx: 0, ty: 0 };
    // Text kurz nach Beginn einblenden, vor dem Ende ausblenden
    const local = t - start;
    const capIn = smooth((local - 0.3) / 0.6);
    const capOut = 1 - smooth((local - (dur - 1.2)) / 0.6);
    return {
      index: e.slide,
      id: s.id,
      opacity,
      shift,
      ...kb,
      contain,
      pos: contain ? [0.5, 0.5] : focusPosition(c?.aspect ?? 1.5, frameAspect, c?.focus),
      caption: s.caption,
      captionOpacity: s.caption ? Math.min(capIn, capOut) : 0,
    };
  };

  // Übergang zum Nachbarn, falls `t` im Übergangsfenster liegt
  let pair: [number, number, number] | null = null; // [vorher, nachher, Fortschritt]
  if (i + 1 < n) {
    const h = halfOf(i + 1);
    if (h > 0 && t >= starts[i + 1] - h) pair = [i, i + 1, (t - (starts[i + 1] - h)) / (2 * h)];
  }
  if (!pair && i > 0) {
    const h = halfOf(i);
    if (h > 0 && t < starts[i] + h) pair = [i - 1, i, (t - (starts[i] - h)) / (2 * h)];
  }
  if (pair) {
    const [j0, j1, raw] = pair;
    const q = smooth(raw);
    if (tr === "slide") {
      layers.push(make(j0, 1, -q), make(j1, 1, 1 - q));
    } else {
      // Überblenden: das neue Dia liegt oben und wird deckend
      layers.push(make(j0, 1, 0), make(j1, q, 0));
    }
  } else layers.push(make(i, 1, 0));

  const fadeIn = 1 - smooth(t / EDGE_FADE_S);
  const fadeOut = smooth((t - (total - EDGE_FADE_S)) / EDGE_FADE_S);
  const firstDur = durationOf(tl, 0);
  const title = a.title.trim() ? Math.min(smooth((t - 0.4) / 0.8), 1 - smooth((t - (firstDur - 1.4)) / 0.8)) : 0;
  return { layers, title, black: Math.max(fadeIn, fadeOut) };
}

/** Dauer als „m:ss“ */
export function formatDuration(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/**
 * Prüft eine gespeicherte Diashow (localStorage) auf Form und Wertebereiche,
 * bevor sie benutzt wird.
 */
export function validSlideshow(x: unknown): x is SlideshowState {
  const a = x as SlideshowState;
  const num = (v: unknown, min = -Infinity, max = Infinity) => typeof v === "number" && Number.isFinite(v) && v >= min && v <= max;
  const id = (v: unknown) => Number.isInteger(v);
  return (
    !!a &&
    a.version === 1 &&
    !!a.settings &&
    typeof a.settings === "object" &&
    typeof a.title === "string" &&
    a.title.length <= 500 &&
    Array.isArray(a.candidates) &&
    a.candidates.every((c) => id(c?.id) && num(c.aspect, 0.01, 100) && num(c.score) && num(c.event, 0) && num(c.group, 0)) &&
    Array.isArray(a.slides) &&
    a.slides.length <= 5000 &&
    a.slides.every(
      (s) =>
        id(s?.id) &&
        (s.seconds === undefined || num(s.seconds, MIN_SECONDS, MAX_SECONDS)) &&
        (s.caption === undefined || (typeof s.caption === "string" && s.caption.length <= 1000)) &&
        (s.fit === undefined || s.fit === "cover" || s.fit === "contain") &&
        (s.map === undefined || typeof s.map === "boolean"),
    ) &&
    (a.music === undefined || validMusic(a.music)) &&
    typeof a.scope === "object" &&
    a.scope !== null
  );
}

function validMusic(m: SlideshowMusic): boolean {
  const num = (v: unknown, min = -Infinity, max = Infinity) => typeof v === "number" && Number.isFinite(v) && v >= min && v <= max;
  const b = m?.beats;
  return (
    !!m &&
    typeof m.path === "string" &&
    typeof m.name === "string" &&
    typeof m.sync === "boolean" &&
    typeof m.fit === "boolean" &&
    num(m.volume, 0, 1) &&
    !!b &&
    num(b.duration, 0, 24 * 3600) &&
    num(b.bpm, 0, 400) &&
    num(b.bar, 0, 3) &&
    Array.isArray(b.beats) &&
    Array.isArray(b.strength) &&
    b.beats.length === b.strength.length &&
    b.beats.length <= 100000 &&
    b.beats.every((x) => num(x)) &&
    b.strength.every((x) => num(x, 0, 1))
  );
}
