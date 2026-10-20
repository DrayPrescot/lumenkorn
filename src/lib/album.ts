// Fotoalbum: Formate, Verteilung der Fotos auf Seiten und Seitenlayout.
//
// Die Auswahl der Fotos kommt aus dem Backend (album/select.rs). Hier wird
// gestaltet: Die gewählten Fotos werden chronologisch auf die Seiten verteilt
// (möglichst nicht über Ereignisse hinweg, die besten Fotos auch mal allein auf
// einer Seite), und jede Seite bekommt ein Layout aus verschachtelten
// Teilungen, das die Seitenverhältnisse der Fotos möglichst wenig beschneidet.

import type { Scope } from "./api";
import { t } from "./i18n";

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
  pages: { cells: AlbumCell[] }[];
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
};

export interface AlbumPage {
  ids: number[];
  /** Welche der guten Anordnungen (wechselt mit „Anders anordnen“) */
  variant: number;
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
  return {
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
}

/** Verteilt die Fotos des Albums neu auf die vorhandene Seitenzahl. */
export function redistribute(a: AlbumState): AlbumPage[] {
  const used = new Set(a.pages.flatMap((p) => p.ids));
  const picked = a.candidates.filter((c) => used.has(c.id));
  const pages = Math.min(a.pages.length, picked.length) || 1;
  return paginate(picked, pages, pickHighlights(picked, pages)).map((ids) => ({ ids, variant: 0 }));
}

// --- Seitenlayout -------------------------------------------------------------

type Node = { leaf: number } | { split: "h" | "v"; a: Node; b: Node };

interface Rect {
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
export function layouts(ids: number[], aspectOf: (id: number) => number, scoreOf: (id: number) => number, s: AlbumSettings): Map<number, Rect>[] {
  if (!ids.length) return [];
  const { margin, gap } = spacing(s);
  const content: Rect = { x: margin, y: margin, w: s.widthMm - 2 * margin, h: s.heightMm - 2 * margin };
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

/** Bildfelder einer Seite (mm) mit Ausschnitt und Zahl der möglichen Anordnungen. */
export function pageLayout(a: AlbumState, page: AlbumPage, byId: Map<number, AlbumCandidate>): { cells: AlbumCell[]; variants: number } {
  const all = layouts(
    page.ids,
    (id) => byId.get(id)?.aspect ?? 1.5,
    (id) => byId.get(id)?.score ?? 0,
    a.settings,
  );
  if (!all.length) return { cells: [], variants: 0 };
  const rects = all[page.variant % all.length];
  const cells = page.ids.map((id) => {
    const r = rects.get(id)!;
    const c = byId.get(id);
    const [ox, oy] = a.offsets[id] ?? focusPosition(c?.aspect ?? 1.5, r.w / r.h, c?.focus);
    return { id, x: r.x, y: r.y, w: r.w, h: r.h, ox, oy };
  });
  return { cells, variants: all.length };
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
