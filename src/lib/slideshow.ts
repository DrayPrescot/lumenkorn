// Diashow: Datenmodell, Zeitleiste und Bewegung (Ken Burns).
//
// Die Auswahl der Fotos kommt aus demselben Backend wie beim Fotoalbum
// (album/select.rs). Hier wird daraus eine Folge von Abschnitten: Titelbild,
// Fotos mit Dauer, Übergängen und Beschriftung, bei einem Ortswechsel eine
// Karte dazwischen, am Ende auf Wunsch die Karte der ganzen Reise. Was zu
// einem Zeitpunkt zu sehen und zu hören ist, berechnen `frameAt` und die
// Audio-Stücke der Zeitleiste allein aus der Zeit – so laufen Vorschau,
// Abspielen und Videoexport auf derselben Zeitleiste.

import type { Scope } from "./api";
import { focusPosition, shortPlace, type AlbumCandidate, type AlbumSuggestion, type CaptionMode, type PhotoSource, type ThemeKey } from "./album";
import { beatGrid, snapToBeats, type Beat, type BeatInfo } from "./beats";
import { distanceKm, type MapLeg, type MapPlace } from "./slideMap";
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
  /** Animierte Karte, wenn sich der Aufnahmeort ändert */
  maps: boolean;
  /** Zum Schluss die Karte mit allen Orten der Reise */
  endMap: boolean;
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
  endMap: true,
};

export const MIN_SECONDS = 1.5;
export const MAX_SECONDS = 30;
/** Dauer eines Übergangs (s), höchstens die Hälfte des kürzeren Abschnitts */
export const TRANSITION_S = 1;
/** Ein- und Ausblenden am Anfang und Ende (s) */
const EDGE_FADE_S = 1;
/** Hochformat (schmaler als dieser Anteil des Bildformats): ganz zeigen statt beschneiden */
const CONTAIN_BELOW = 0.8;
/** Stärke des Ken-Burns-Zooms */
const ZOOM = 0.12;
/** Dauer der Karte beim Ortswechsel (s) */
export const MAP_SECONDS = 4;
/** Dauer der Karte mit allen Orten am Ende (s) */
export const OVERVIEW_SECONDS = 7;
/** Ab dieser Entfernung zum vorigen Aufnahmeort kommt eine Karte (km) */
export const MAP_MIN_KM = 5;
/** „Lieder nach Ort“: neuer Ort ab dieser Entfernung vom ersten Foto des bisherigen (km) */
export const PLACE_KM = 20;
/** Überblenden zwischen zwei Liedern (s) */
export const CROSSFADE_S = 3;

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

/** Ein Lied */
export interface MusicTrack {
  /** Pfad der Datei (in der Browser-Vorschau nur der Name) */
  path: string;
  name: string;
  /** Erkannter Takt (gespeichert, damit nicht bei jedem Öffnen neu gerechnet wird) */
  beats: BeatInfo;
}

/** Hintereinander mit Überblenden oder nach Ort */
export type MusicMode = "sequence" | "place";

/** Hintergrundmusik */
export interface SlideshowMusic {
  tracks: MusicTrack[];
  mode: MusicMode;
  /** Bildwechsel auf den Takt legen */
  sync: boolean;
  /** Dauer der Fotos so wählen, dass die Diashow mit der Musik endet (nur hintereinander) */
  fit: boolean;
  /** Lautstärke 0…1 */
  volume: number;
  /** „Lieder nach Ort“: Lieder je Ort (Schlüssel aus `musicPlaces`, Werte: Pfade der Lieder) */
  assign?: Record<string, string[]>;
}

export interface SlideshowState {
  version: 1;
  settings: SlideshowSettings;
  scope: Scope;
  /** Titel über dem Titelbild (leer: ohne Titelbild) */
  title: string;
  /** Foto für das Titelbild; ohne Angabe das am besten bewertete */
  cover?: number;
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

/**
 * Ältere gespeicherte Fassungen anheben: Musik war ein einzelnes Lied
 * (`path`, `name`, `beats` direkt in `music`).
 */
export function upgradeSlideshow(x: unknown): unknown {
  const a = x as { music?: Record<string, unknown> } | null;
  const m = a?.music;
  if (m && !Array.isArray(m.tracks) && typeof m.path === "string") {
    a.music = {
      tracks: [{ path: m.path, name: m.name, beats: m.beats }],
      mode: "sequence",
      sync: m.sync ?? true,
      fit: m.fit ?? true,
      volume: m.volume ?? 0.8,
    };
  }
  return x;
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

/** Foto des Titelbilds: das gewählte, sonst das am besten bewertete der Diashow */
export function coverId(a: SlideshowState, byId: Map<number, AlbumCandidate>): number | undefined {
  if (a.cover !== undefined && byId.has(a.cover)) return a.cover;
  let best: AlbumCandidate | undefined;
  for (const s of a.slides) {
    const c = byId.get(s.id);
    if (c && (!best || c.score > best.score)) best = c;
  }
  return best?.id;
}

// --- Zeitleiste ----------------------------------------------------------------

/** Eingestellte Dauer eines Dias (ohne Anpassung an die Musik) */
export function slideSeconds(a: SlideshowState, s: Slide): number {
  return clampSeconds(s.seconds ?? a.settings.seconds);
}

const placeOf = (c: AlbumCandidate): MapPlace => ({ lat: c.lat!, lon: c.lon!, label: shortPlace(c.place) });

/** Flug vom vorigen Aufnahmeort zum Ort des Dias (Index → Flug), soweit eine Karte kommt */
export function mapLegs(a: SlideshowState, byId: Map<number, AlbumCandidate>): Map<number, MapLeg> {
  const legs = new Map<number, MapLeg>();
  let prev: AlbumCandidate | undefined;
  a.slides.forEach((s, j) => {
    const c = byId.get(s.id);
    if (c?.lat == null || c.lon == null) return;
    if (prev) {
      const from = placeOf(prev);
      const to = placeOf(c);
      const km = distanceKm(from, to);
      const want = s.map ?? (a.settings.maps && km >= MAP_MIN_KM);
      // Am selben Ort ergibt ein Flug keinen Sinn, auch wenn eingeschaltet
      if (want && km >= 0.5) legs.set(j, { from, to });
    }
    prev = c;
  });
  return legs;
}

/** Alle Aufnahmeorte in Reihenfolge (direkt aufeinanderfolgende nahe Orte einmal) */
export function routePlaces(a: SlideshowState, byId: Map<number, AlbumCandidate>): MapPlace[] {
  const out: MapPlace[] = [];
  for (const s of a.slides) {
    const c = byId.get(s.id);
    if (c?.lat == null || c.lon == null) continue;
    const p = placeOf(c);
    const last = out[out.length - 1];
    if (last && distanceKm(last, p) < 1) {
      if (!last.label) last.label = p.label;
      continue;
    }
    out.push(p);
  }
  return out;
}

/** Abschnitt der Zeitleiste. `slide`: zugehöriges Foto (für Zähler und Auswahl). */
export type Entry =
  | { kind: "title"; slide: number; id: number }
  | { kind: "photo"; slide: number }
  | { kind: "map"; slide: number; leg: MapLeg }
  | { kind: "overview"; slide: number; places: MapPlace[] };

/** Abschnitte in Reihenfolge */
export function entriesOf(a: SlideshowState, byId: Map<number, AlbumCandidate>): Entry[] {
  const out: Entry[] = [];
  if (!a.slides.length) return out;
  const cover = coverId(a, byId);
  if (a.title.trim() && cover !== undefined) out.push({ kind: "title", slide: 0, id: cover });
  const legs = mapLegs(a, byId);
  a.slides.forEach((_, j) => {
    const leg = legs.get(j);
    if (leg) out.push({ kind: "map", slide: j, leg });
    out.push({ kind: "photo", slide: j });
  });
  if (a.settings.endMap) {
    const places = routePlaces(a, byId);
    if (places.length >= 2) out.push({ kind: "overview", slide: a.slides.length - 1, places });
  }
  return out;
}

/** Gesamtlänge der Lieder hintereinander (mit Überblenden) */
function playlistSeconds(m: SlideshowMusic): number {
  const ds = m.tracks.map((t) => t.beats.duration).filter((d) => d > 0);
  return ds.reduce((s, d) => s + d, 0) - Math.max(0, ds.length - 1) * CROSSFADE_S;
}

/** Gewünschte Dauer je Abschnitt; `def`: Dauer der Fotos ohne eigene */
function durationsFor(a: SlideshowState, entries: Entry[], def: number): number[] {
  return entries.map((e) =>
    e.kind === "map" ? MAP_SECONDS : e.kind === "overview" ? OVERVIEW_SECONDS : e.kind === "title" ? clampSeconds(def) : clampSeconds(a.slides[e.slide].seconds ?? def),
  );
}

/** Ein Stück Musik in der Zeitleiste: Lied `track` ab `offset`, von `start` bis `end` (s) */
export interface AudioClip {
  track: number;
  start: number;
  end: number;
  offset: number;
  fadeIn: number;
  fadeOut: number;
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
  /** Musik: welche Lieder wann laufen */
  clips: AudioClip[];
}

/**
 * Lieder `tracks` (Indizes) hintereinander mit Überblenden ab `start`,
 * wiederholt, bis `until` erreicht ist.
 */
function chainClips(m: SlideshowMusic, tracks: number[], start: number, until: number, firstFadeIn: number): AudioClip[] {
  const list = tracks.filter((i) => m.tracks[i]?.beats.duration > 0);
  const clips: AudioClip[] = [];
  let t = start;
  for (let k = 0; list.length && t < until && k < 10000; k++) {
    const i = list[k % list.length];
    const d = m.tracks[i].beats.duration;
    clips.push({ track: i, start: t, end: t + d, offset: 0, fadeIn: k === 0 ? firstFadeIn : CROSSFADE_S, fadeOut: CROSSFADE_S });
    t += d - CROSSFADE_S;
  }
  return clips;
}

/** Alle Lieder hintereinander mit Überblenden, wiederholt bis `total` */
function sequenceClips(m: SlideshowMusic, total: number): AudioClip[] {
  return chainClips(
    m,
    m.tracks.map((_, i) => i),
    0,
    total,
    0,
  );
}

/** Schläge der Lieder hintereinander; in Überblendungen gilt das neue Lied ab der Mitte */
function sequenceBeats(m: SlideshowMusic, clips: AudioClip[]): Beat[] {
  return clips.flatMap((c, k) => {
    const from = k > 0 ? c.start + CROSSFADE_S / 2 : c.start;
    const to = k + 1 < clips.length ? c.end - CROSSFADE_S / 2 : c.end;
    return beatGrid(m.tracks[c.track].beats, c.start, c.end, from, to);
  });
}

function plainStarts(d: number[], t0 = 0): { starts: number[]; total: number } {
  const starts: number[] = [];
  let t = t0;
  for (const x of d) {
    starts.push(t);
    t += x;
  }
  return { starts, total: t };
}

/** Ein Ort für die Musik: ab Abschnitt `start`, benannt nach seinem ersten Foto mit Aufnahmeort */
export interface MusicPlace {
  start: number;
  /** Schlüssel für die Zuordnung der Lieder (gerundete Koordinaten; leer: ohne Ort) */
  key: string;
  label: string;
  /** Zahl der Fotos */
  photos: number;
}

/**
 * „Lieder nach Ort“: Orte in Reihenfolge. Ein neuer Ort beginnt, wenn ein
 * Foto mindestens `PLACE_KM` vom ersten Foto des bisherigen entfernt ist;
 * die Karte davor gehört schon zum neuen Ort.
 */
function placeSegments(a: SlideshowState, entries: Entry[], byId: Map<number, AlbumCandidate>): MusicPlace[] {
  const out: MusicPlace[] = [{ start: 0, key: "", label: "", photos: 0 }];
  let anchor: AlbumCandidate | undefined;
  entries.forEach((e, i) => {
    if (e.kind !== "photo") return;
    const c = byId.get(a.slides[e.slide].id);
    if (c?.lat != null && c.lon != null) {
      if (anchor && distanceKm(placeOf(anchor), placeOf(c)) >= PLACE_KM) {
        out.push({ start: entries[i - 1]?.kind === "map" ? i - 1 : i, key: "", label: "", photos: 0 });
        anchor = undefined;
      }
      const seg = out[out.length - 1];
      if (!anchor) {
        anchor = c;
        seg.key = `${c.lat.toFixed(2)},${c.lon.toFixed(2)}`;
      }
      if (!seg.label) seg.label = shortPlace(c.place);
    }
    out[out.length - 1].photos++;
  });
  return out;
}

/** Orte für die Zuordnung der Lieder (wie sie die Zeitleiste im Modus „pro Ort“ bildet) */
export function musicPlaces(a: SlideshowState, byId: Map<number, AlbumCandidate>): MusicPlace[] {
  return placeSegments(a, entriesOf(a, byId), byId);
}

/**
 * Lieder eines Orts: die zugeordneten; Orte ohne Zuordnung bekommen reihum
 * eines der Lieder, die keinem Ort zugeordnet sind (sind alle vergeben, alle).
 */
function placeTracks(m: SlideshowMusic, places: MusicPlace[]): number[][] {
  const lists = places.map((pl) => (m.assign?.[pl.key] ?? []).map((path) => m.tracks.findIndex((t) => t.path === path)).filter((i) => i >= 0));
  const used = new Set(lists.flat());
  const unused = m.tracks.map((_, i) => i).filter((i) => !used.has(i));
  const pool = unused.length ? unused : m.tracks.map((_, i) => i);
  let free = 0;
  return lists.map((own) => (own.length ? own : [pool[free++ % pool.length]]));
}

/**
 * Beginn jedes Abschnitts, Gesamtdauer und Musik. Der Übergang liegt mittig
 * auf der Grenze zwischen zwei Abschnitten – mit Musik im Takt genau auf dem
 * Schlag, und kein Foto steht kürzer als eingestellt.
 */
export function timeline(a: SlideshowState, byId = new Map(a.candidates.map((c) => [c.id, c]))): Timeline {
  const entries = entriesOf(a, byId);
  const photoEntry: number[] = [];
  entries.forEach((e, i) => {
    if (e.kind === "photo") photoEntry[e.slide] = i;
  });
  const m = a.music?.tracks.some((t) => t.beats.duration > 0) ? a.music : undefined;
  const sync = !!m?.sync;
  const beatLen = m ? Math.min(...m.tracks.map((t) => (t.beats.bpm > 0 ? 60 / t.beats.bpm : TRANSITION_S))) : TRANSITION_S;
  const transition = sync ? Math.min(TRANSITION_S, beatLen) : TRANSITION_S;
  const base = { entries, photoEntry, transition };

  if (m?.mode === "place") {
    // Je Ort seine Lieder (sonst reihum eins), Wechsel mit Überblenden an der Ortsgrenze
    const d = durationsFor(a, entries, a.settings.seconds);
    const places = placeSegments(a, entries, byId);
    const lists = placeTracks(m, places);
    const starts: number[] = [];
    const clips: AudioClip[] = [];
    let t = 0;
    places.forEach((pl, seg) => {
      const i = pl.start;
      const j = places[seg + 1]?.start ?? entries.length;
      if (j <= i) return;
      const part = d.slice(i, j);
      const firstFade = seg === 0 ? 0 : CROSSFADE_S / 2;
      const est = t + part.reduce((x, y) => x + y, 0) * 1.6 + 10;
      const chain = chainClips(m, lists[seg], t, est, firstFade);
      const r = sync ? snapToBeats(part, sequenceBeats(m, chain), t) : plainStarts(part, t);
      starts.push(...r.starts);
      const last = j >= entries.length;
      const end = r.total + (last ? 0 : CROSSFADE_S / 2);
      for (const c of chain) {
        if (c.start >= end) break;
        clips.push(c.end > end ? { ...c, end, fadeOut: last ? c.fadeOut : CROSSFADE_S / 2 } : c);
      }
      t = r.total;
    });
    return { ...base, starts, total: t, clips };
  }

  // Hintereinander (oder ohne Musik)
  let def = a.settings.seconds;
  let r = plainStarts(durationsFor(a, entries, def));
  if (m?.fit) {
    // Die Fotos ohne eigene Dauer teilen sich die Länge der Musik; mit Takt
    // werden sie durch das Warten auf den Schlag länger – nachregeln
    const want = playlistSeconds(m);
    const free = entries.filter((e) => e.kind === "title" || (e.kind === "photo" && a.slides[e.slide].seconds === undefined)).length;
    if (free && want > 0) {
      const fixed = durationsFor(a, entries, 0).reduce((s, x, k) => s + (entries[k].kind === "title" || (entries[k].kind === "photo" && a.slides[entries[k].slide].seconds === undefined) ? 0 : x), 0);
      def = Math.max(MIN_SECONDS, (want - fixed) / free);
      for (let iter = 0; iter < 5; iter++) {
        const d = durationsFor(a, entries, def);
        r = sync ? snapToBeats(d, sequenceBeats(m, sequenceClips(m, want + 60))) : plainStarts(d);
        if (!sync || Math.abs(r.total - want) < beatLen) break;
        def = Math.max(MIN_SECONDS, def * Math.max(0.5, (want - fixed) / Math.max(1, r.total - fixed)));
      }
    }
  } else if (m && sync) {
    const d = durationsFor(a, entries, def);
    const est = d.reduce((s, x) => s + x, 0) * 1.6 + 10;
    r = snapToBeats(d, sequenceBeats(m, sequenceClips(m, est)));
  }
  return { ...base, ...r, clips: m ? sequenceClips(m, r.total).map((c) => ({ ...c, end: Math.min(c.end, r.total) })).filter((c) => c.end > c.start) : [] };
}

/** Tatsächliche Dauer des Abschnitts `i` */
export function durationOf(tl: Timeline, i: number): number {
  return (i + 1 < tl.starts.length ? tl.starts[i + 1] : tl.total) - tl.starts[i];
}

/** Tatsächliche Dauer des Fotos `j` (ohne Karte davor) */
export function photoDuration(tl: Timeline, j: number): number {
  return durationOf(tl, tl.photoEntry[j]);
}

/** Lautstärke eines Musikstücks zur Zeit `t` (0…1, ohne Gesamtlautstärke), samt Ein-/Ausblenden der ganzen Diashow */
export function clipGain(c: AudioClip, t: number, total: number): number {
  if (t < c.start || t >= c.end) return 0;
  const fin = c.fadeIn > 0 ? Math.min(1, (t - c.start) / c.fadeIn) : 1;
  const fout = c.fadeOut > 0 ? Math.min(1, (c.end - t) / c.fadeOut) : 1;
  const edge = Math.min(1, t / 0.5, (total - t) / 2);
  return Math.max(0, Math.min(fin, fout, edge));
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

/** Ein sichtbarer Abschnitt zu einem Zeitpunkt */
export interface Layer {
  /** Schlüssel (Foto: Index des Dias, Titelbild, Karten: negativ) */
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
  /** Karte mit allen Orten, Fortschritt 0…1 */
  overview?: { places: MapPlace[]; p: number };
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
export function isContain(s: Slide | undefined, c: AlbumCandidate | undefined, frameAspect: number): boolean {
  if (s?.fit) return s.fit === "contain";
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

const TITLE_KEY = -1_000_000;
const OVERVIEW_KEY = -2_000_000;

/** Was zur Zeit `t` zu sehen ist. `frameAspect`: Breite/Höhe der Bildfläche. */
export function frameAt(a: SlideshowState, tl: Timeline, t: number, frameAspect: number, byId: Map<number, AlbumCandidate>): Frame {
  const { starts, total, entries } = tl;
  const n = entries.length;
  if (!n) return { layers: [], title: 0, black: 1 };
  const tr = a.settings.transition;
  const i = slideAt(starts, t);
  const layers: Layer[] = [];
  // Halbe Dauer des Übergangs vor Abschnitt `j` (höchstens die Hälfte beider)
  const halfOf = (j: number) => (tr === "none" || j <= 0 || j >= n ? 0 : Math.min(tl.transition, durationOf(tl, j) / 2, durationOf(tl, j - 1) / 2) / 2);
  const still = { scale: 1, tx: 0, ty: 0, contain: false, pos: [0.5, 0.5] as [number, number], captionOpacity: 0 };
  const make = (j: number, opacity: number, shift: number): Layer => {
    const e = entries[j];
    const start = starts[j];
    const dur = durationOf(tl, j);
    const own = Math.min(1, Math.max(0, (t - start) / Math.max(0.001, dur)));
    if (e.kind === "map") return { index: -1 - e.slide, id: a.slides[e.slide].id, opacity, shift, ...still, map: { leg: e.leg, p: own } };
    if (e.kind === "overview") return { index: OVERVIEW_KEY, id: -1, opacity, shift, ...still, overview: { places: e.places, p: own } };
    const s = e.kind === "photo" ? a.slides[e.slide] : undefined;
    const id = e.kind === "title" ? e.id : s!.id;
    const c = byId.get(id);
    // Sichtbar von halbem Übergang vor bis halbem Übergang nach dem Abschnitt
    const from = start - halfOf(j);
    const to = start + dur + halfOf(j + 1);
    const p = (t - from) / Math.max(0.001, to - from);
    const contain = isContain(s ?? a.slides.find((x) => x.id === id), c, frameAspect);
    const key = e.kind === "title" ? TITLE_KEY : e.slide;
    const kb = a.settings.kenBurns ? kenBurns(e.kind === "title" ? 0 : e.slide + 1, c, contain, Math.min(1, Math.max(0, p))) : { scale: 1, tx: 0, ty: 0 };
    // Text kurz nach Beginn einblenden, vor dem Ende ausblenden
    const local = t - start;
    const capIn = smooth((local - 0.3) / 0.6);
    const capOut = 1 - smooth((local - (dur - 1.2)) / 0.6);
    const caption = s?.caption;
    return {
      index: key,
      id,
      opacity,
      shift,
      ...kb,
      contain,
      pos: contain ? [0.5, 0.5] : focusPosition(c?.aspect ?? 1.5, frameAspect, c?.focus),
      caption,
      captionOpacity: caption ? Math.min(capIn, capOut) : 0,
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
    if (tr === "slide") layers.push(make(j0, 1, -q), make(j1, 1, 1 - q));
    // Überblenden: der neue Abschnitt liegt oben und wird deckend
    else layers.push(make(j0, 1, 0), make(j1, q, 0));
  } else layers.push(make(i, 1, 0));

  const fadeIn = 1 - smooth(t / EDGE_FADE_S);
  const fadeOut = smooth((t - (total - EDGE_FADE_S)) / EDGE_FADE_S);
  let title = 0;
  if (entries[0].kind === "title") {
    const d0 = durationOf(tl, 0);
    title = Math.min(smooth((t - 0.4) / 0.8), 1 - smooth((t - (d0 - 1.2)) / 0.7));
  }
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
    (a.cover === undefined || id(a.cover)) &&
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

function validTrack(t: MusicTrack): boolean {
  const num = (v: unknown, min = -Infinity, max = Infinity) => typeof v === "number" && Number.isFinite(v) && v >= min && v <= max;
  const b = t?.beats;
  return (
    !!t &&
    typeof t.path === "string" &&
    typeof t.name === "string" &&
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

function validMusic(m: SlideshowMusic): boolean {
  return (
    !!m &&
    Array.isArray(m.tracks) &&
    m.tracks.length >= 1 &&
    m.tracks.length <= 100 &&
    m.tracks.every(validTrack) &&
    (m.mode === "sequence" || m.mode === "place") &&
    typeof m.sync === "boolean" &&
    typeof m.fit === "boolean" &&
    typeof m.volume === "number" &&
    m.volume >= 0 &&
    m.volume <= 1 &&
    (m.assign === undefined ||
      (typeof m.assign === "object" &&
        m.assign !== null &&
        Object.values(m.assign).every((v) => Array.isArray(v) && v.length <= 100 && v.every((p) => typeof p === "string"))))
  );
}
