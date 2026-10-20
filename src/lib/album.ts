// Fotoalbum: Formate, Verteilung der Fotos auf Seiten und Seitenlayout.
//
// Die Auswahl der Fotos kommt aus dem Backend (album/select.rs). Hier wird
// gestaltet: Die gewählten Fotos werden chronologisch auf die Seiten verteilt
// (möglichst nicht über Ereignisse hinweg, die besten Fotos auch mal allein auf
// einer Seite), und jede Seite bekommt ein Layout aus verschachtelten
// Teilungen, das die Seitenverhältnisse der Fotos möglichst wenig beschneidet.

import type { Scope } from "./api";
import { locale, t } from "./i18n";

export interface AlbumCandidate {
  id: number;
  /** Breite/Höhe wie entwickelt */
  aspect: number;
  capturedAt: string | null;
  score: number;
  event: number;
  group: number;
  picked: boolean;
  sharpness: number;
  /** Mitte der Gesichter, normiert */
  focus?: [number, number];
  /** Lange Kante des Originals in Pixeln */
  pixels?: number;
  /** Aufnahmeort (fehlt ohne GPS) */
  lat?: number;
  lon?: number;
  /** Ortsname, z. B. „Ubud, Bali, Indonesien“ */
  place?: string;
}

export interface AlbumSuggestion {
  candidates: AlbumCandidate[];
  themeUsed: boolean;
  unanalyzed: number;
}

/** Bildfeld in mm auf der beschnittenen Seite (wie `AlbumCell` in Rust) */
export interface AlbumCell {
  id: number;
  x: number;
  y: number;
  w: number;
  h: number;
  ox: number;
  oy: number;
}

export interface AlbumDoc {
  widthMm: number;
  heightMm: number;
  bleedMm: number;
  dpi: number;
  background: [number, number, number];
  /** `overlay`: Texte und Karte als PNG (Base64) über der ganzen Seite samt Beschnitt */
  pages: { cells: AlbumCell[]; overlay?: string }[];
}

export interface AlbumFormat {
  key: string;
  w: number;
  h: number;
}

/** Gängige Fotobuch-Formate (eine Seite, beschnitten, in mm) */
export const FORMATS: AlbumFormat[] = [
  { key: "30x30", w: 300, h: 300 },
  { key: "20x20", w: 200, h: 200 },
  { key: "a4l", w: 297, h: 210 },
  { key: "a4p", w: 210, h: 297 },
  { key: "28x21", w: 280, h: 210 },
  { key: "a3l", w: 420, h: 297 },
];

/** Name eines Formats; eigene Maße als „B × H mm“ */
export function formatName(key: string, w: number, h: number): string {
  const f = FORMATS.find((x) => x.key === key);
  if (!f || f.w !== w || f.h !== h) return `${Math.round(w)} × ${Math.round(h)} mm`;
  const names: Record<string, string> = {
    "30x30": t("30 × 30 cm"),
    "20x20": t("20 × 20 cm"),
    a4l: t("A4 quer (29,7 × 21 cm)"),
    a4p: t("A4 hoch (21 × 29,7 cm)"),
    "28x21": t("28 × 21 cm quer"),
    a3l: t("A3 quer (42 × 29,7 cm)"),
  };
  return names[key];
}

export const THEME_KEYS = ["vacation", "travel", "wedding", "birthday", "family", "baby", "nature", "party", "sport", "animals", "custom"] as const;
export type ThemeKey = (typeof THEME_KEYS)[number];

export type AlbumStyle = "margins" | "borderless";
/** Automatische Beschriftung, wo ein neuer Tag bzw. Ort beginnt */
export type CaptionMode = "both" | "date" | "place" | "none";
/** Kartenseite mit den besuchten Orten */
export type MapPlacement = "none" | "first" | "last";
export type AlbumFont = "sans" | "serif";

/** CSS-Schriftfamilie (über @fontsource eingebunden, siehe main.ts) */
export const FONT_FAMILY: Record<AlbumFont, string> = {
  sans: '"Instrument Sans Variable"',
  serif: '"Lora Variable"',
};

/** Text auf einer Seite. */
export interface AlbumText {
  id: string;
  text: string;
  /** über bzw. unter den Fotos (die Fotos rücken zusammen) oder frei auf der Seite */
  pos: "top" | "bottom" | "free";
  /** Nur bei `free`: linke obere Ecke und Breite des Textfelds in mm */
  x?: number;
  y?: number;
  w?: number;
  /** Schriftgröße in Punkt */
  size: number;
  align: "left" | "center" | "right";
  /** auto: passend zum Hintergrund (frei auf Fotos: hell mit Schatten) */
  color: "auto" | "dark" | "light";
  bold?: boolean;
  /** Automatisch erzeugte Beschriftung (wird bei „Neu beschriften“ ersetzt) */
  auto?: boolean;
}
export type AlbumBackground = "white" | "black" | "gray";

export const BACKGROUNDS: Record<AlbumBackground, [number, number, number]> = {
  white: [255, 255, 255],
  black: [18, 18, 18],
  gray: [232, 230, 226],
};

export interface AlbumSettings {
  theme: ThemeKey;
  customTheme: string;
  /** Schlüssel aus FORMATS oder "custom" */
  format: string;
  widthMm: number;
  heightMm: number;
  /** 0: automatisch aus der Anzahl der Fotos */
  pages: number;
  count: number;
  style: AlbumStyle;
  background: AlbumBackground;
  /** Unbearbeitete RAW-Dateien auslassen (nur Bearbeitetes und JPEGs) */
  skipUneditedRaw: boolean;
  captions: CaptionMode;
  map: MapPlacement;
  font: AlbumFont;
}

export const DEFAULT_SETTINGS: AlbumSettings = {
  theme: "vacation",
  customTheme: "",
  format: "30x30",
  widthMm: 300,
  heightMm: 300,
  pages: 0,
  count: 60,
  style: "margins",
  background: "white",
  skipUneditedRaw: true,
  captions: "both",
  map: "none",
  font: "sans",
};

export interface AlbumPage {
  ids: number[];
  /** Welche der guten Anordnungen (wechselt mit „Anders anordnen“) */
  variant: number;
  /**
   * Festgehaltene Felder (mm, gleiche Reihenfolge wie `ids`): Nach dem
   * Ersetzen eines Fotos bleibt die Aufteilung der Seite, das neue Foto kommt
   * genau in das Feld des alten. Jede andere Änderung der Seite ordnet neu.
   */
  slots?: { x: number; y: number; w: number; h: number }[];
  texts?: AlbumText[];
  /** Kartenseite (statt Fotos): Karte mit den besuchten Orten */
  map?: { labels: boolean };
}

export interface AlbumState {
  version: 1;
  settings: AlbumSettings;
  scope: Scope;
  title: string;
  candidates: AlbumCandidate[];
  pages: AlbumPage[];
  /** Vom Nutzer verschobene Ausschnitte (object-position) je Foto */
  offsets: Record<number, [number, number]>;
  themeUsed: boolean;
  unanalyzed: number;
}

export const BLEED_MM = 3;
export const DPI = 300;
/** Höchstens so viele Fotos auf einer Seite */
export const MAX_PER_PAGE = 6;

/** Rand und Abstand in mm für Stil und Format */
export function spacing(s: Pick<AlbumSettings, "style" | "widthMm" | "heightMm">): { margin: number; gap: number } {
  const side = Math.min(s.widthMm, s.heightMm);
  return s.style === "borderless" ? { margin: 0, gap: Math.max(1.5, side * 0.008) } : { margin: side * 0.05, gap: side * 0.015 };
}

/** Seitenzahl bei „automatisch“: im Schnitt 2,5 Fotos je Seite, gerade Zahl */
export function autoPages(count: number): number {
  return Math.max(2, 2 * Math.round(count / 5));
}

// --- Texte ---------------------------------------------------------------------

export const PT_MM = 25.4 / 72;
/** Zeilenhöhe relativ zur Schriftgröße */
export const LINE = 1.3;

let measureCtx: CanvasRenderingContext2D | null = null;

/** CSS-Schriftangabe für Canvas, Größe in Pixeln */
export function canvasFont(font: AlbumFont, px: number, bold = false): string {
  return `${bold ? 600 : 400} ${px}px ${FONT_FAMILY[font]}, sans-serif`;
}

/**
 * Zeilen eines Textes bei gegebener Breite (mm): Absätze bleiben, lange Zeilen
 * werden an Wortgrenzen umbrochen. Gemessen mit derselben Schrift wie beim
 * Zeichnen, damit Vorschau und Druck gleich umbrechen.
 */
export function wrapText(text: string, font: AlbumFont, sizePt: number, widthMm: number, bold = false): string[] {
  measureCtx ??= document.createElement("canvas").getContext("2d");
  const ctx = measureCtx!;
  // In Millimetern messen: 1 px = 1 mm bei dieser Schriftgröße
  ctx.font = canvasFont(font, sizePt * PT_MM, bold);
  const out: string[] = [];
  for (const para of text.split("\n")) {
    const words = para.split(/\s+/).filter(Boolean);
    if (!words.length) {
      out.push("");
      continue;
    }
    let line = words[0];
    for (const w of words.slice(1)) {
      const next = `${line} ${w}`;
      if (ctx.measureText(next).width <= widthMm) line = next;
      else {
        out.push(line);
        line = w;
      }
    }
    out.push(line);
  }
  return out;
}

/** Höhe eines Textes in mm */
export function textHeight(lines: number, sizePt: number): number {
  return lines * sizePt * PT_MM * LINE;
}

/** Bereich der Seite, auf dem Text sicher steht (nicht zu nah an der Schnittkante) */
export function safeArea(s: AlbumSettings): Rect {
  const { margin } = spacing(s);
  const inset = Math.max(margin, Math.min(s.widthMm, s.heightMm) * 0.04);
  return { x: inset, y: inset, w: s.widthMm - 2 * inset, h: s.heightMm - 2 * inset };
}

/** Lage eines Textes auf der Seite (mm) mit seinen Zeilen */
export interface PlacedText {
  text: AlbumText;
  x: number;
  y: number;
  w: number;
  h: number;
  lines: string[];
}

/**
 * Bereich für Fotos bzw. Karte und die Lage aller Texte. Texte oben und unten
 * bilden Bänder, die Fotos rücken dafür zusammen; freie Texte liegen darüber.
 */
export function pageFrame(s: AlbumSettings, page: AlbumPage): { content: Rect; texts: PlacedText[] } {
  const { margin, gap } = spacing(s);
  let content: Rect = { x: margin, y: margin, w: s.widthMm - 2 * margin, h: s.heightMm - 2 * margin };
  const texts: PlacedText[] = [];
  const texts0 = page.texts ?? [];
  if (!texts0.length) return { content, texts };
  const safe = safeArea(s);
  const band = (pos: "top" | "bottom") => {
    const list = texts0.filter((t) => t.pos === pos);
    if (!list.length) return;
    const placed = list.map((t) => {
      const lines = wrapText(t.text, s.font, t.size, safe.w, t.bold);
      return { t, lines, h: textHeight(lines.length, t.size) };
    });
    const total = placed.reduce((n, p) => n + p.h, 0) + gap * (placed.length - 1);
    // Randlos beginnt das Band am sicheren Rand, sonst am Seitenrand des Inhalts
    let y = pos === "top" ? Math.max(content.y, safe.y) : Math.min(content.y + content.h, safe.y + safe.h) - total;
    for (const p of placed) {
      texts.push({ text: p.t, x: safe.x, y, w: safe.w, h: p.h, lines: p.lines });
      y += p.h + gap;
    }
    const used = pos === "top" ? Math.max(content.y, safe.y) + total + gap - content.y : content.y + content.h - (Math.min(content.y + content.h, safe.y + safe.h) - total - gap);
    // Fotos nie ganz verdrängen
    const take = Math.min(used, content.h * 0.5);
    content = pos === "top" ? { ...content, y: content.y + take, h: content.h - take } : { ...content, h: content.h - take };
  };
  band("top");
  band("bottom");
  for (const t of texts0.filter((t) => t.pos === "free")) {
    const w = Math.min(t.w ?? safe.w * 0.6, s.widthMm);
    const lines = wrapText(t.text, s.font, t.size, w, t.bold);
    const h = textHeight(lines.length, t.size);
    const x = Math.min(Math.max(t.x ?? safe.x, 0), s.widthMm - w);
    const y = Math.min(Math.max(t.y ?? safe.y + safe.h - h, 0), s.heightMm - h);
    texts.push({ text: t, x, y, w, h, lines });
  }
  return { content, texts };
}

/** Neue Kennung für einen Text */
export function textId(): string {
  return Math.random().toString(36).slice(2, 10);
}

/** Erster Teil eines Ortsnamens („Ubud, Bali, Indonesien“ → „Ubud“) */
export function shortPlace(place: string | undefined): string {
  return place?.split(",")[0]?.trim() ?? "";
}

/** Tag (JJJJ-MM-TT) aus einer Aufnahmezeit */
function dayOf(c: AlbumCandidate | undefined): string {
  return c?.capturedAt?.slice(0, 10) ?? "";
}

function formatDay(day: string): string {
  const [y, m, d] = day.split("-").map(Number);
  if (!y || !m || !d) return "";
  return new Date(y, m - 1, d).toLocaleDateString(locale, { day: "numeric", month: "long", year: "numeric" });
}

/**
 * Beschriftet das Album: Wo ein neuer Tag bzw. Ort beginnt, bekommt die Seite
 * einmal Datum und/oder Ort. Frühere automatische Beschriftungen werden
 * ersetzt, eigene Texte bleiben.
 */
export function autoCaptions(a: AlbumState, byId: Map<number, AlbumCandidate>): AlbumPage[] {
  const mode = a.settings.captions;
  let prev = "";
  return a.pages.map((p) => {
    const own = (p.texts ?? []).filter((t) => !t.auto);
    const page = { ...p, texts: own.length ? own : undefined };
    if (mode === "none" || p.map || !p.ids.length) return page;
    // Erstes Foto der Seite mit Angabe (Datum fehlt selten, Ort öfter)
    const cands = p.ids.map((id) => byId.get(id));
    const day = dayOf(cands.find((c) => dayOf(c)));
    const place = shortPlace(cands.find((c) => c?.place)?.place);
    const key = mode === "date" ? day : mode === "place" ? place : `${day}|${place}`;
    if (!key.replace("|", "") || key === prev) {
      if (key.replace("|", "")) prev = key;
      return page;
    }
    prev = key;
    const parts = [mode !== "place" ? formatDay(day) : "", mode !== "date" ? place : ""].filter(Boolean);
    if (!parts.length) return page;
    const borderless = a.settings.style === "borderless";
    const caption: AlbumText = {
      id: textId(),
      text: parts.join(" · "),
      pos: borderless ? "free" : "top",
      size: 11,
      align: "left",
      color: "auto",
      auto: true,
    };
    if (borderless) {
      // Randlos: unten links auf dem Foto, im sicheren Bereich
      const safe = safeArea(a.settings);
      caption.x = safe.x;
      caption.w = safe.w * 0.7;
      caption.y = safe.y + safe.h - textHeight(1, caption.size);
    }
    return { ...page, texts: [caption, ...own] };
  });
}

/** Neue Kartenseite (vorne oder hinten) */
export function mapPage(): AlbumPage {
  return { ids: [], variant: 0, map: { labels: true }, texts: [{ id: textId(), text: t("Unsere Reise"), pos: "top", size: 20, align: "center", color: "auto", bold: true }] };
}

// --- Verteilung auf Seiten -----------------------------------------------------

/**
 * Teilt die chronologisch sortierten Fotos in `pages` Gruppen. Kosten: Abstand
 * von der mittleren Anzahl je Seite, Seiten über Ereignisgrenzen hinweg; die
 * besten Fotos (`highlights`) dürfen ohne Abzug allein stehen.
 */
export function paginate(items: { id: number; event: number }[], pages: number, highlights: Set<number>): number[][] {
  const n = items.length;
  if (!n) return [];
  pages = Math.max(1, Math.min(pages, n));
  const target = n / pages;
  const cost = (i: number, j: number) => {
    // Gruppe items[i..j)
    const size = j - i;
    let c = size === 1 && highlights.has(items[i].id) ? 0 : 0.5 * (size - target) ** 2;
    for (let k = i + 1; k < j; k++) if (items[k].event !== items[k - 1].event) c += 3;
    if (size > 1) for (let k = i; k < j; k++) if (highlights.has(items[k].id)) c += 0.6;
    return c;
  };
  // best[p][i]: geringste Kosten für die ersten i Fotos auf p Seiten
  const INF = Number.POSITIVE_INFINITY;
  const best = Array.from({ length: pages + 1 }, () => new Array<number>(n + 1).fill(INF));
  const from = Array.from({ length: pages + 1 }, () => new Array<number>(n + 1).fill(-1));
  best[0][0] = 0;
  for (let p = 1; p <= pages; p++) {
    for (let i = p; i <= n; i++) {
      for (let s = 1; s <= MAX_PER_PAGE && s <= i; s++) {
        const prev = best[p - 1][i - s];
        if (prev === INF) continue;
        const c = prev + cost(i - s, i);
        if (c < best[p][i]) {
          best[p][i] = c;
          from[p][i] = i - s;
        }
      }
    }
  }
  // Mehr Fotos, als auf die Seiten passen: hinten abschneiden
  let end = n;
  while (end > 0 && best[pages][end] === INF) end--;
  const groups: number[][] = [];
  for (let p = pages, i = end; p > 0; p--) {
    const j = from[p][i];
    groups.unshift(items.slice(j, i).map((x) => x.id));
    i = j;
  }
  return groups;
}

/** Die besten Fotos, die eine Seite für sich bekommen dürfen. */
export function pickHighlights(picked: AlbumCandidate[], pages: number): Set<number> {
  const k = Math.max(1, Math.round(pages / 6));
  if (picked.length < pages * 1.5) return new Set(picked.map((c) => c.id)); // genug Platz
  return new Set([...picked].sort((a, b) => b.score - a.score).slice(0, k).map((c) => c.id));
}

/** Neues Album aus dem Vorschlag des Backends. */
export function buildAlbum(settings: AlbumSettings, scope: Scope, title: string, s: AlbumSuggestion): AlbumState {
  const picked = s.candidates.filter((c) => c.picked);
  // Feste Seitenzahl: Reicht sie nicht (mehr Fotos mit 4–5 Sternen als
  // gewünscht), kommen Seiten dazu – gerade Zahl
  const needed = 2 * Math.ceil(picked.length / MAX_PER_PAGE / 2);
  const pages = settings.pages > 0 ? Math.max(settings.pages, needed) : autoPages(picked.length);
  const groups = paginate(picked, pages, pickHighlights(picked, pages));
  const a: AlbumState = {
    version: 1,
    settings,
    scope,
    title,
    candidates: s.candidates,
    pages: groups.map((ids) => ({ ids, variant: 0 })),
    offsets: {},
    themeUsed: s.themeUsed,
    unanalyzed: s.unanalyzed,
  };
  // Karte nur, wenn es Aufnahmeorte gibt
  if (settings.map !== "none" && s.candidates.some((c) => c.lat != null && c.lon != null)) {
    if (settings.map === "first") a.pages.unshift(mapPage());
    else a.pages.push(mapPage());
  }
  a.pages = autoCaptions(a, new Map(a.candidates.map((c) => [c.id, c])));
  return a;
}

/**
 * Verteilt die Fotos des Albums neu auf die vorhandenen Fotoseiten.
 * Kartenseiten bleiben an ihrer Stelle, eigene Texte bei ihrer Seitennummer;
 * die automatische Beschriftung entsteht neu.
 */
export function redistribute(a: AlbumState): AlbumPage[] {
  const used = new Set(a.pages.flatMap((p) => p.ids));
  const picked = a.candidates.filter((c) => used.has(c.id));
  const photoPages = a.pages.filter((p) => !p.map);
  const n = Math.min(photoPages.length, picked.length) || 1;
  const groups = paginate(picked, n, pickHighlights(picked, n));
  let k = 0;
  const pages = a.pages.map((p) => {
    if (p.map) return p;
    const ids = groups[k++] ?? [];
    const own = (p.texts ?? []).filter((t) => !t.auto);
    return { ids, variant: 0, ...(own.length ? { texts: own } : {}) };
  });
  return autoCaptions({ ...a, pages }, new Map(a.candidates.map((c) => [c.id, c])));
}

// --- Seitenlayout -------------------------------------------------------------

type Node = { leaf: number } | { split: "h" | "v"; a: Node; b: Node };

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Alle Teilungsbäume über die Fotos in dieser Reihenfolge. */
function trees(items: number[]): Node[] {
  if (items.length === 1) return [{ leaf: items[0] }];
  const out: Node[] = [];
  for (let k = 1; k < items.length; k++) {
    const left = trees(items.slice(0, k));
    const right = trees(items.slice(k));
    for (const a of left) for (const b of right) for (const split of ["h", "v"] as const) out.push({ split, a, b });
  }
  return out;
}

function permutations<T>(xs: T[]): T[][] {
  if (xs.length <= 1) return [xs];
  return xs.flatMap((x, i) => permutations([...xs.slice(0, i), ...xs.slice(i + 1)]).map((p) => [x, ...p]));
}

/** Natürliches Seitenverhältnis eines Baums (Fotos unbeschnitten, ohne Abstände). */
function naturalAspect(n: Node, aspect: (i: number) => number): number {
  if ("leaf" in n) return aspect(n.leaf);
  const a = naturalAspect(n.a, aspect);
  const b = naturalAspect(n.b, aspect);
  // h: nebeneinander (gleiche Höhe), v: übereinander (gleiche Breite)
  return n.split === "h" ? a + b : 1 / (1 / a + 1 / b);
}

function place(n: Node, r: Rect, gap: number, aspect: (i: number) => number, out: Map<number, Rect>) {
  if ("leaf" in n) {
    out.set(n.leaf, r);
    return;
  }
  const a = naturalAspect(n.a, aspect);
  const b = naturalAspect(n.b, aspect);
  if (n.split === "h") {
    const w1 = ((r.w - gap) * a) / (a + b);
    place(n.a, { x: r.x, y: r.y, w: w1, h: r.h }, gap, aspect, out);
    place(n.b, { x: r.x + w1 + gap, y: r.y, w: r.w - w1 - gap, h: r.h }, gap, aspect, out);
  } else {
    const h1 = ((r.h - gap) * (1 / a)) / (1 / a + 1 / b);
    place(n.a, { x: r.x, y: r.y, w: r.w, h: h1 }, gap, aspect, out);
    place(n.b, { x: r.x, y: r.y + h1 + gap, w: r.w, h: r.h - h1 - gap }, gap, aspect, out);
  }
}

/** Anteil eines Fotos, der in einem Feld abgeschnitten wird (0…1). */
export function cropLoss(photoAspect: number, cellAspect: number): number {
  return 1 - Math.min(photoAspect / cellAspect, cellAspect / photoAspect);
}

interface Candidate {
  rects: Map<number, Rect>;
  cost: number;
  key: string;
}

/**
 * Gute Anordnungen für die Fotos einer Seite, die beste zuerst. `ids` in
 * chronologischer Reihenfolge; bis 4 Fotos werden auch andere Reihenfolgen
 * probiert. Jeder Baum wird einmal formatfüllend (Fotos leicht beschnitten) und
 * einmal eingepasst (unbeschnitten, mit freier Fläche) bewertet.
 */
export function layouts(
  ids: number[],
  aspectOf: (id: number) => number,
  scoreOf: (id: number) => number,
  s: AlbumSettings,
  area?: Rect,
): Map<number, Rect>[] {
  if (!ids.length) return [];
  const { margin, gap } = spacing(s);
  const content: Rect = area ?? { x: margin, y: margin, w: s.widthMm - 2 * margin, h: s.heightMm - 2 * margin };
  const pageArea = content.w * content.h;
  const aspect = (id: number) => Math.min(4, Math.max(0.25, aspectOf(id) || 1.5));
  const orders = ids.length <= 4 ? permutations(ids) : [ids];
  const best = ids.reduce((a, b) => (scoreOf(b) > scoreOf(a) ? b : a));
  const found: Candidate[] = [];
  for (const order of orders) {
    for (const tree of trees(order)) {
      const nat = naturalAspect(tree, aspect);
      // formatfüllend
      const fill = new Map<number, Rect>();
      place(tree, content, gap, aspect, fill);
      // eingepasst und mittig
      const ca = content.w / content.h;
      const fw = nat > ca ? content.w : content.h * nat;
      const fh = nat > ca ? content.w / nat : content.h;
      const fit = new Map<number, Rect>();
      place(tree, { x: content.x + (content.w - fw) / 2, y: content.y + (content.h - fh) / 2, w: fw, h: fh }, gap, aspect, fit);
      for (const [rects, empty] of [
        [fill, 0],
        [fit, 1 - (fw * fh) / pageArea],
      ] as const) {
        // Randlos: freie Fläche wirkt wie ein Fehler, lieber etwas beschneiden
        let cost = empty * (s.style === "borderless" ? 1.8 : 0.9);
        let largest = 0;
        let largestId = -1;
        for (const [id, r] of rects) {
          if (r.w <= 0 || r.h <= 0) {
            cost = Number.POSITIVE_INFINITY;
            break;
          }
          const share = (r.w * r.h) / pageArea;
          const loss = cropLoss(aspect(id), r.w / r.h);
          cost += loss * (0.5 + share);
          // Stark beschnitten (Köpfe ab) oder winzig: deutlich schlechter
          if (loss > 0.3) cost += (loss - 0.3) * 3;
          if (share < 0.07) cost += 0.4;
          if (share > largest) {
            largest = share;
            largestId = id;
          }
        }
        if (ids.length > 1 && largestId !== best) cost += 0.05;
        if (!Number.isFinite(cost)) continue;
        const key = [...rects.entries()]
          .sort((a, b) => a[0] - b[0])
          .map(([id, r]) => `${id}:${Math.round(r.x)},${Math.round(r.y)},${Math.round(r.w)},${Math.round(r.h)}`)
          .join(";");
        found.push({ rects, cost, key });
      }
    }
  }
  found.sort((a, b) => a.cost - b.cost);
  // Verschiedene Anordnungen, nicht viel schlechter als die beste
  const seen = new Set<string>();
  const out: Map<number, Rect>[] = [];
  for (const c of found) {
    if (seen.has(c.key)) continue;
    if (out.length && c.cost > found[0].cost + 0.25) break;
    seen.add(c.key);
    out.push(c.rects);
    if (out.length >= 8) break;
  }
  return out;
}

/** object-position, das den Punkt `focus` (normiert) möglichst mittig zeigt. */
export function focusPosition(photoAspect: number, cellAspect: number, focus: [number, number] | undefined): [number, number] {
  if (!focus) return [0.5, 0.5];
  const pos = (f: number, visible: number) => (visible >= 0.999 ? 0.5 : Math.min(1, Math.max(0, (f - visible / 2) / (1 - visible))));
  // Sichtbarer Anteil in x bzw. y bei object-fit: cover
  const vx = Math.min(1, cellAspect / photoAspect);
  const vy = Math.min(1, photoAspect / cellAspect);
  return [pos(focus[0], vx), pos(focus[1], vy)];
}

export interface PageLayout {
  cells: AlbumCell[];
  variants: number;
  /** Bereich für Fotos bzw. Karte (mm) */
  content: Rect;
  texts: PlacedText[];
}

/** Bildfelder einer Seite (mm) mit Ausschnitt, Texten und Zahl der möglichen Anordnungen. */
export function pageLayout(a: AlbumState, page: AlbumPage, byId: Map<number, AlbumCandidate>): PageLayout {
  const { content, texts } = pageFrame(a.settings, page);
  const all = layouts(
    page.ids,
    (id) => byId.get(id)?.aspect ?? 1.5,
    (id) => byId.get(id)?.score ?? 0,
    a.settings,
    content,
  );
  if (!all.length) return { cells: [], variants: 0, content, texts };
  const rects = all[page.variant % all.length];
  const fixed = page.slots?.length === page.ids.length ? page.slots : null;
  const cells = page.ids.map((id, i) => {
    const r = fixed ? fixed[i] : rects.get(id)!;
    const c = byId.get(id);
    const [ox, oy] = a.offsets[id] ?? focusPosition(c?.aspect ?? 1.5, r.w / r.h, c?.focus);
    return { id, x: r.x, y: r.y, w: r.w, h: r.h, ox, oy };
  });
  return { cells, variants: all.length, content, texts };
}

/** Effektive Auflösung eines Fotos im Feld (dpi), `null` wenn unbekannt. */
export function effectiveDpi(c: AlbumCandidate | undefined, cell: AlbumCell): number | null {
  if (!c?.pixels) return null;
  const a = c.aspect;
  const pw = a >= 1 ? c.pixels : c.pixels * a;
  const visible = Math.min(1, cell.w / cell.h / a);
  return (pw * visible) / (cell.w / 25.4);
}

/** Album für den Export (Rust `AlbumDoc`). */
export function toDoc(a: AlbumState, byId: Map<number, AlbumCandidate>): AlbumDoc {
  return {
    widthMm: a.settings.widthMm,
    heightMm: a.settings.heightMm,
    bleedMm: BLEED_MM,
    dpi: DPI,
    background: BACKGROUNDS[a.settings.background],
    pages: a.pages.map((p) => ({ cells: pageLayout(a, p, byId).cells })),
  };
}

/**
 * Ersatzfotos für ein Foto bzw. eine Seite: noch nicht verwendete, zuerst aus
 * derselben Serie, dann aus demselben Ereignis, jeweils nach Wertung.
 */
export function alternatives(a: AlbumState, ref: { group?: number; events: number[] }, limit = 60): AlbumCandidate[] {
  const used = new Set(a.pages.flatMap((p) => p.ids));
  const rank = (c: AlbumCandidate) => (c.group === ref.group ? 0 : ref.events.includes(c.event) ? 1 : 2);
  return a.candidates
    .filter((c) => !used.has(c.id))
    .sort((x, y) => rank(x) - rank(y) || y.score - x.score)
    .slice(0, limit);
}

/**
 * Prüft ein gespeichertes Album (localStorage) auf Form und Wertebereiche,
 * bevor es benutzt wird – eine kaputte oder fremde Fassung soll die Ansicht
 * nicht zum Absturz bringen oder unsinnige Seiten an den Export geben.
 */
function validText(t: AlbumText): boolean {
  const num = (v: unknown, min = -Infinity, max = Infinity) => typeof v === "number" && Number.isFinite(v) && v >= min && v <= max;
  const opt = (v: unknown, min: number, max: number) => v === undefined || num(v, min, max);
  return (
    !!t &&
    typeof t.id === "string" &&
    typeof t.text === "string" &&
    t.text.length <= 5000 &&
    ["top", "bottom", "free"].includes(t.pos) &&
    ["left", "center", "right"].includes(t.align) &&
    ["auto", "dark", "light"].includes(t.color) &&
    num(t.size, 4, 200) &&
    opt(t.x, -100, 1000) &&
    opt(t.y, -100, 1000) &&
    opt(t.w, 1, 1000)
  );
}

export function validAlbum(x: unknown): x is AlbumState {
  const a = x as AlbumState;
  const num = (v: unknown, min = -Infinity, max = Infinity) => typeof v === "number" && Number.isFinite(v) && v >= min && v <= max;
  const id = (v: unknown) => Number.isInteger(v);
  const s = a?.settings;
  return (
    !!a &&
    a.version === 1 &&
    !!s &&
    num(s.widthMm, 50, 600) &&
    num(s.heightMm, 50, 600) &&
    (s.style === "margins" || s.style === "borderless") &&
    s.background in BACKGROUNDS &&
    Array.isArray(a.candidates) &&
    a.candidates.every((c) => id(c?.id) && num(c.aspect, 0.01, 100) && num(c.score) && num(c.event, 0) && num(c.group, 0)) &&
    Array.isArray(a.pages) &&
    a.pages.length <= 1000 &&
    a.pages.every(
      (p) =>
        Array.isArray(p?.ids) &&
        p.ids.length <= MAX_PER_PAGE &&
        p.ids.every(id) &&
        num(p.variant, 0) &&
        (p.slots === undefined ||
          (Array.isArray(p.slots) && p.slots.every((r) => num(r?.x) && num(r.y) && num(r.w, 0.1) && num(r.h, 0.1)))) &&
        (p.texts === undefined || (Array.isArray(p.texts) && p.texts.length <= 50 && p.texts.every(validText))) &&
        (p.map === undefined || (typeof p.map === "object" && p.map !== null && typeof p.map.labels === "boolean")),
    ) &&
    typeof a.offsets === "object" &&
    a.offsets !== null &&
    Object.values(a.offsets).every((o) => Array.isArray(o) && o.length === 2 && num(o[0], 0, 1) && num(o[1], 0, 1)) &&
    typeof a.scope === "object" &&
    a.scope !== null
  );
}
