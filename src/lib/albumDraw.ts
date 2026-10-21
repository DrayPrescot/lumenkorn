// Fotoalbum: Texte und Reisekarte zeichnen – in der Vorschau (Canvas über der
// Seite) und beim Export (transparente PNG-Ebene in Druckauflösung, die das
// Backend über die Seite legt). Beides läuft durch denselben Code, damit
// Umbruch, Schrift und Karte im Druck genau so aussehen wie in der Vorschau.

import {
  BACKGROUNDS,
  BLEED_MM,
  DPI,
  FONT_FAMILY,
  LINE,
  PT_MM,
  canvasFont,
  shortPlace,
  type AlbumCandidate,
  type AlbumFont,
  type AlbumState,
  type PageLayout,
  type PlacedText,
  type Rect,
} from "./album";
import { countryAt, loadCountries, type Country } from "./countries";
import { lang, locale } from "./i18n";
import { loadMapDetails, type MapDetails } from "./mapdetails";

/** Schriften laden, bevor auf Canvas gezeichnet wird (sonst Ersatzschrift) */
export async function ensureFonts(font: AlbumFont): Promise<void> {
  try {
    await Promise.all([document.fonts.load(`400 16px ${FONT_FAMILY[font]}`), document.fonts.load(`600 16px ${FONT_FAMILY[font]}`)]);
  } catch {
    // Ohne Schrift zeichnet Canvas mit der Ersatzschrift – besser als nichts
  }
}

/** Ist der Seitenhintergrund dunkel? */
function darkBackground(a: AlbumState): boolean {
  const [r, g, b] = BACKGROUNDS[a.settings.background];
  return r * 0.3 + g * 0.59 + b * 0.11 < 128;
}

/** Liegt das Textfeld (überwiegend) auf einem Foto? */
function onPhoto(t: PlacedText, layout: PageLayout): boolean {
  const cx = t.x + t.w / 2;
  const cy = t.y + t.h / 2;
  return layout.cells.some((c) => cx >= c.x && cx <= c.x + c.w && cy >= c.y && cy <= c.y + c.h);
}

function drawText(ctx: CanvasRenderingContext2D, a: AlbumState, t: PlacedText, layout: PageLayout) {
  const photo = onPhoto(t, layout);
  const light = t.text.color === "light" || (t.text.color === "auto" && (photo || darkBackground(a)));
  const size = t.text.size * PT_MM;
  ctx.save();
  ctx.font = canvasFont(a.settings.font, size, t.text.bold);
  ctx.textBaseline = "alphabetic";
  ctx.textAlign = t.text.align;
  ctx.fillStyle = light ? "#ffffff" : "#222222";
  if (light && photo) {
    // Lesbar auf hellen Stellen des Fotos
    ctx.shadowColor = "rgba(0,0,0,0.55)";
    ctx.shadowBlur = size * 0.35;
    ctx.shadowOffsetY = size * 0.05;
  }
  const x = t.text.align === "left" ? t.x : t.text.align === "right" ? t.x + t.w : t.x + t.w / 2;
  t.lines.forEach((line, i) => {
    // Grundlinie: etwa 80 % der Zeilenhöhe von oben
    const y = t.y + size * LINE * i + size * (LINE - 0.3);
    ctx.fillText(line, x, y);
  });
  ctx.restore();
}

// --- Karte -----------------------------------------------------------------------

/** Ort auf der Karte (Stecknadel) */
export interface MapPin {
  lon: number;
  lat: number;
  /** Anzahl Fotos */
  count: number;
  label: string;
}

/** Web-Mercator: y wächst nach Norden */
const mercY = (lat: number) => Math.log(Math.tan(Math.PI / 4 + (Math.max(-80, Math.min(80, lat)) * Math.PI) / 360));
const rad = (deg: number) => (deg * Math.PI) / 180;

interface View {
  /** Kartenausschnitt in Mercator-Einheiten */
  x0: number;
  x1: number;
  y0: number;
  y1: number;
  rect: Rect;
}

function project(v: View, lon: number, lat: number): [number, number] {
  const x = v.rect.x + ((rad(lon) - v.x0) / (v.x1 - v.x0)) * v.rect.w;
  const y = v.rect.y + ((v.y1 - mercY(lat)) / (v.y1 - v.y0)) * v.rect.h;
  return [x, y];
}

/**
 * Ausschnitt um alle Orte mit etwas Rand, mindestens etwa 300 km breit (damit
 * eine Insel oder Region als solche erkennbar ist), passend zum Seitenformat.
 */
function viewFor(points: { lon: number; lat: number }[], rect: Rect): View {
  const lons = points.map((p) => rad(p.lon));
  const ys = points.map((p) => mercY(p.lat));
  let x0 = Math.min(...lons);
  let x1 = Math.max(...lons);
  let y0 = Math.min(...ys);
  let y1 = Math.max(...ys);
  const minSpan = rad(3);
  const cx = (x0 + x1) / 2;
  const cy = (y0 + y1) / 2;
  let w = Math.max(x1 - x0, minSpan) * 1.35;
  let h = Math.max(y1 - y0, minSpan) * 1.35;
  const aspect = rect.w / rect.h;
  if (w / h > aspect) h = w / aspect;
  else w = h * aspect;
  // Nicht über die Pole hinaus (dort gibt es keine Daten) und nicht mehr als
  // einmal um die Welt: dann wird die Karte flacher bzw. schmaler und mittig
  // gesetzt, statt leere Ränder zu zeigen
  const yMin = mercY(-58);
  const yMax = mercY(78);
  let r = rect;
  if (h > yMax - yMin) {
    // Zu hoch: Ausschnitt begrenzen, Karte flacher
    h = yMax - yMin;
    const rh = rect.w * (h / w);
    r = { x: rect.x, y: rect.y + (rect.h - rh) / 2, w: rect.w, h: rh };
  }
  if (w > 2 * Math.PI) {
    // Breiter als die Welt: Karte schmaler
    w = 2 * Math.PI;
    const rw = r.h * (w / h);
    r = { x: r.x + (r.w - rw) / 2, y: r.y, w: rw, h: r.h };
  }
  x0 = cx - w / 2;
  x1 = cx + w / 2;
  y0 = Math.min(Math.max(cy - h / 2, yMin), yMax - h);
  y1 = y0 + h;
  return { x0, x1, y0, y1, rect: r };
}

/** Häufigster Wert und sein Anteil */
function mostCommon(values: string[]): [string, number] {
  const counts = new Map<string, number>();
  for (const v of values) if (v) counts.set(v, (counts.get(v) ?? 0) + 1);
  const best = [...counts.entries()].sort((a, b) => b[1] - a[1])[0];
  return best ? [best[0], best[1] / values.length] : ["", 0];
}

/**
 * Name einer Stecknadel. Liegen darunter verschiedene Orte (z. B. eine ganze
 * Insel), passt die nächsthöhere Ebene aus dem Ortsnamen besser:
 * „Tegallalang, Bali, Indonesien“ und „Ubud, Bali, …“ → „Bali“.
 */
function pinLabel(members: AlbumCandidate[]): string {
  const parts = members.map((m) => (m.place ?? "").split(",").map((x) => x.trim()));
  const [town, share] = mostCommon(parts.map((p) => p[0] ?? ""));
  if (share >= 0.75) return town;
  const [region, rshare] = mostCommon(parts.map((p) => (p.length > 2 ? p[1] : "")));
  return rshare >= 0.6 ? region : town || shortPlace(members[0]?.place);
}

/**
 * Fasst Orte zusammen, die auf der Karte näher als `minMm` beieinander lägen:
 * Die Karte bleibt übersichtlich, egal ob die Reise eine Stadt oder einen
 * Kontinent umfasst. Name: der häufigste Ort der Gruppe.
 */
export function clusterPins(cands: AlbumCandidate[], view: View, minMm: number): MapPin[] {
  const pts = cands.filter((c) => c.lat != null && c.lon != null).map((c) => ({ c, xy: project(view, c.lon!, c.lat!) }));
  const groups: { x: number; y: number; members: AlbumCandidate[] }[] = [];
  for (const p of pts) {
    const g = groups.find((g) => Math.hypot(g.x - p.xy[0], g.y - p.xy[1]) < minMm);
    if (g) {
      g.members.push(p.c);
      const n = g.members.length;
      g.x += (p.xy[0] - g.x) / n;
      g.y += (p.xy[1] - g.y) / n;
    } else groups.push({ x: p.xy[0], y: p.xy[1], members: [p.c] });
  }
  return groups.map((g) => {
    const label = pinLabel(g.members);
    // Die Nadel steht auf einem echten Aufnahmeort (dem der Mitte nächsten),
    // nicht auf dem Mittelpunkt – der läge an der Küste womöglich im Meer
    const at = g.members.reduce((best, m) => {
      const [x, y] = project(view, m.lon!, m.lat!);
      const [bx, by] = project(view, best.lon!, best.lat!);
      return Math.hypot(x - g.x, y - g.y) < Math.hypot(bx - g.x, by - g.y) ? m : best;
    });
    return { lon: at.lon!, lat: at.lat!, count: g.members.length, label };
  });
}

const MAP = {
  sea: "#dfe9ef",
  land: "#f4f1ea",
  visited: "#e6d9bd",
  border: "#b7b0a2",
  river: "#9fc0d4",
  pin: "#c2412d",
  text: "#2b2b2b",
  city: "#55524c",
  peak: "#6b5a45",
};

function ringPath(ctx: CanvasRenderingContext2D, v: View, ring: [number, number][]) {
  ring.forEach(([lon, lat], i) => {
    const [x, y] = project(v, lon, lat);
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  });
  ctx.closePath();
}

function drawPin(ctx: CanvasRenderingContext2D, x: number, y: number, h: number) {
  // Stecknadel: Kreis oben, Spitze auf dem Ort
  const r = h * 0.32;
  const cy = y - h + r;
  ctx.beginPath();
  ctx.moveTo(x, y);
  ctx.bezierCurveTo(x - r * 0.35, y - h * 0.35, x - r, cy + r * 0.6, x - r, cy);
  ctx.arc(x, cy, r, Math.PI, 0);
  ctx.bezierCurveTo(x + r, cy + r * 0.6, x + r * 0.35, y - h * 0.35, x, y);
  ctx.closePath();
  ctx.fillStyle = MAP.pin;
  ctx.shadowColor = "rgba(0,0,0,0.3)";
  ctx.shadowBlur = h * 0.15;
  ctx.shadowOffsetY = h * 0.04;
  ctx.fill();
  ctx.shadowColor = "transparent";
  ctx.beginPath();
  ctx.arc(x, cy, r * 0.42, 0, Math.PI * 2);
  ctx.fillStyle = "#ffffff";
  ctx.fill();
}

/** Stecknadeln der Karte in `rect` (mm), mit Ausschnitt */
function mapView(a: AlbumState, rect: Rect): { view: View; pins: MapPin[]; located: AlbumCandidate[] } | null {
  const located = a.candidates.filter((c) => c.lat != null && c.lon != null);
  if (!located.length) return null;
  const view = viewFor(
    located.map((c) => ({ lon: c.lon!, lat: c.lat! })),
    rect,
  );
  const side = Math.min(view.rect.w, view.rect.h);
  return { view, pins: clusterPins(located, view, side * 0.06), located };
}

/**
 * Stecknadeln einer Kartenseite für die Seitenleiste: vorgeschlagener Name
 * (Schlüssel zum Umbenennen), angezeigter Name und Zahl der Fotos.
 */
export function mapPins(a: AlbumState, pageIndex: number, layout: PageLayout): { key: string; name: string; count: number }[] {
  const m = mapView(a, layout.content);
  const names = a.pages[pageIndex]?.map?.names ?? {};
  return (m?.pins ?? [])
    .filter((p) => p.label)
    .map((p) => ({ key: p.label, name: names[p.label] ?? p.label, count: p.count }))
    .sort((x, y) => y.count - x.count);
}

/** Kartendaten: Länder und (falls geladen) Städte, Berge, Gewässer */
export interface MapData {
  countries: Country[];
  details: MapDetails | null;
}

/** Was die Kartenseite zeigen soll (fehlende Angaben: an) */
export interface MapOptions {
  labels: boolean;
  names: Record<string, string>;
  cities: boolean;
  peaks: boolean;
  water: boolean;
}

type Box = Rect;
const overlaps = (b: Box, list: Box[]) => list.some((q) => b.x < q.x + q.w && q.x < b.x + b.w && b.y < q.y + q.h && q.y < b.y + b.h);
const inside = (b: Box, r: Rect) => b.x >= r.x && b.y >= r.y && b.x + b.w <= r.x + r.w && b.y + b.h <= r.y + r.h;

function haloText(ctx: CanvasRenderingContext2D, text: string, x: number, y: number, size: number, color: string) {
  ctx.lineWidth = size * 0.28;
  ctx.strokeStyle = "rgba(255,255,255,0.9)";
  ctx.strokeText(text, x, y);
  ctx.fillStyle = color;
  ctx.fillText(text, x, y);
}

/** Linie in Kartenkoordinaten */
function linePath(ctx: CanvasRenderingContext2D, v: View, line: [number, number][]) {
  line.forEach(([lon, lat], i) => {
    const [x, y] = project(v, lon, lat);
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  });
}

/** Karte mit allen Aufnahmeorten des Projekts in `rect` (mm). */
function drawMap(ctx: CanvasRenderingContext2D, a: AlbumState, rect: Rect, data: MapData, opt: MapOptions) {
  const m = mapView(a, rect);
  if (!m) return;
  const { view: v, located } = m;
  const { countries } = data;
  rect = v.rect;
  ctx.save();
  ctx.beginPath();
  ctx.rect(rect.x, rect.y, rect.w, rect.h);
  ctx.clip();
  ctx.fillStyle = MAP.sea;
  ctx.fillRect(rect.x, rect.y, rect.w, rect.h);
  const side = Math.min(rect.w, rect.h);
  // Umbenannte Orte (leer: ohne Namen)
  const pins = m.pins.map((p) => ({ ...p, label: opt.names[p.label] ?? p.label }));
  // Besuchte Länder aus den einzelnen Aufnahmeorten (auf ~1 km zusammengefasst)
  const spots = new Map(located.map((c) => [`${c.lat!.toFixed(2)},${c.lon!.toFixed(2)}`, c]));
  const visited = new Set([...spots.values()].map((c) => countryAt(countries, c.lon!, c.lat!)?.iso).filter(Boolean));
  // Ausschnitt in Grad (für den groben Vorfilter)
  const latOf = (y: number) => (Math.atan(Math.sinh(y)) * 180) / Math.PI;
  const lon0 = (v.x0 * 180) / Math.PI;
  const lon1 = (v.x1 * 180) / Math.PI;
  const lat0 = latOf(v.y0);
  const lat1 = latOf(v.y1);
  const near = ([a0, b0, a1, b1]: [number, number, number, number], pad = 0) =>
    !(a1 < lon0 - pad || a0 > lon1 + pad || b1 < lat0 - pad || b0 > lat1 + pad);
  // Wie viele Grad entspricht 1 mm? (Breite des Ausschnitts / Kartenbreite)
  const degPerMm = (lon1 - lon0) / rect.w;
  ctx.lineJoin = "round";
  ctx.lineCap = "round";
  for (const c of countries) {
    if (!near(c.bbox, 5)) continue;
    ctx.beginPath();
    for (const poly of c.polygons) for (const ring of poly) ringPath(ctx, v, ring);
    ctx.fillStyle = visited.has(c.iso) ? MAP.visited : MAP.land;
    ctx.fill("evenodd");
    ctx.strokeStyle = MAP.border;
    ctx.lineWidth = Math.max(0.15, side * 0.0012);
    ctx.stroke();
  }
  const d = data.details;
  // Seen und Flüsse: je größer der Ausschnitt, desto nur die wichtigeren
  if (d && opt.water) {
    const maxRank = detailRank(degPerMm);
    ctx.fillStyle = MAP.sea;
    ctx.strokeStyle = MAP.river;
    ctx.lineWidth = Math.max(0.12, side * 0.0008);
    for (const l of d.lakes) {
      if (l.rank > maxRank || !near(l.bbox)) continue;
      ctx.beginPath();
      for (const poly of l.polygons) for (const ring of poly) ringPath(ctx, v, ring);
      ctx.fill("evenodd");
      ctx.stroke();
    }
    ctx.strokeStyle = MAP.river;
    for (const r of d.rivers) {
      if (r.rank > maxRank || !near(r.bbox)) continue;
      // Wichtige Flüsse etwas breiter
      ctx.lineWidth = Math.max(0.15, side * (0.0028 - Math.min(r.rank, 10) * 0.00018));
      ctx.beginPath();
      for (const line of r.lines) linePath(ctx, v, line);
      ctx.stroke();
    }
  }
  // Platz für die Stecknadeln und ihre Namen zuerst reservieren: Städte und
  // Berge kommen nur dorthin, wo danach noch Platz ist
  const pinH = side * 0.035;
  const placed: Box[] = [];
  const xy = pins.map((p) => project(v, p.lon, p.lat));
  for (const [x, y] of xy) placed.push({ x: x - pinH * 0.4, y: y - pinH, w: pinH * 0.8, h: pinH });
  const size = Math.max(2.2, side * 0.012);
  const pinLabels: { text: string; box: Box }[] = [];
  if (opt.labels) {
    ctx.font = canvasFont(a.settings.font, size, true);
    // Große Orte zuerst beschriften
    const order = pins.map((_, i) => i).sort((i, j) => pins[j].count - pins[i].count);
    for (const i of order) {
      const p = pins[i];
      if (!p.label) continue;
      const [x, y] = xy[i];
      const w = ctx.measureText(p.label).width;
      // rechts der Nadel, sonst links
      const cy = y - pinH * 0.68;
      const box = [x + pinH * 0.45, x - pinH * 0.45 - w]
        .map((bx) => ({ x: bx, y: cy - size * 0.6, w, h: size * 1.2 }))
        .find((b) => inside(b, rect) && !overlaps(b, placed));
      if (!box) continue;
      placed.push(box);
      pinLabels.push({ text: p.label, box });
    }
  }
  const small = size * 0.8;
  const pinNames = new Set(pins.map((p) => p.label.toLowerCase()));
  // Berge: die höchsten im Ausschnitt, Dreieck mit Name und Höhe
  if (d && opt.peaks) {
    ctx.font = canvasFont(a.settings.font, small);
    ctx.textBaseline = "middle";
    ctx.textAlign = "left";
    const tri = small * 0.75;
    let shown = 0;
    const peaks = d.peaks.filter((p) => p.lon >= lon0 && p.lon <= lon1 && p.lat >= lat0 && p.lat <= lat1).sort((p, q) => q.elevation - p.elevation);
    for (const p of peaks) {
      if (shown >= 8) break;
      const [x, y] = project(v, p.lon, p.lat);
      const text = `${p.name} ${p.elevation.toLocaleString(locale)} m`;
      const w = ctx.measureText(text).width;
      const mark: Box = { x: x - tri / 2, y: y - tri / 2, w: tri, h: tri };
      const label: Box = { x: x + tri * 0.8, y: y - small * 0.6, w, h: small * 1.2 };
      if (!inside(label, rect) || overlaps(mark, placed) || overlaps(label, placed)) continue;
      placed.push(mark, label);
      shown++;
      ctx.beginPath();
      ctx.moveTo(x, y - tri * 0.55);
      ctx.lineTo(x + tri * 0.55, y + tri * 0.4);
      ctx.lineTo(x - tri * 0.55, y + tri * 0.4);
      ctx.closePath();
      ctx.fillStyle = MAP.peak;
      ctx.fill();
      haloText(ctx, text, label.x, y, small, MAP.peak);
    }
  }
  // Städte: nach Wichtigkeit, so viele wie Platz haben (höchstens ~25)
  if (d && opt.cities) {
    ctx.font = canvasFont(a.settings.font, small);
    ctx.textBaseline = "middle";
    ctx.textAlign = "left";
    const dot = small * 0.32;
    let shown = 0;
    const cities = d.cities
      .filter((c) => c.lon >= lon0 && c.lon <= lon1 && c.lat >= lat0 && c.lat <= lat1)
      .sort((p, q) => p.rank - q.rank);
    for (const c of cities) {
      if (shown >= 25) break;
      const name = (lang === "de" && c.de) || c.en;
      // Nicht doppelt (Daten enthalten manche Städte zweimal) und nicht, wenn
      // eine Stecknadel schon so heißt
      if (!name || pinNames.has(name.toLowerCase())) continue;
      pinNames.add(name.toLowerCase());
      const [x, y] = project(v, c.lon, c.lat);
      const w = ctx.measureText(name).width;
      const mark: Box = { x: x - dot * 2, y: y - dot * 2, w: dot * 4, h: dot * 4 };
      const label = [x + dot * 2, x - dot * 2 - w]
        .map((bx) => ({ x: bx, y: y - small * 0.6, w, h: small * 1.2 }))
        .find((b) => inside(b, rect) && !overlaps(b, placed));
      if (!label || overlaps(mark, placed)) continue;
      placed.push(mark, label);
      shown++;
      ctx.beginPath();
      ctx.arc(x, y, dot, 0, Math.PI * 2);
      ctx.fillStyle = MAP.city;
      ctx.fill();
      haloText(ctx, name, label.x, y, small, MAP.city);
    }
  }
  // Stecknadeln und ihre Namen obenauf
  for (const [x, y] of xy) drawPin(ctx, x, y, pinH);
  ctx.font = canvasFont(a.settings.font, size, true);
  ctx.textBaseline = "middle";
  ctx.textAlign = "left";
  for (const l of pinLabels) haloText(ctx, l.text, l.box.x, l.box.y + l.box.h / 2, size, MAP.text);
  ctx.restore();
  // Feiner Rahmen
  ctx.strokeStyle = MAP.border;
  ctx.lineWidth = Math.max(0.15, side * 0.001);
  ctx.strokeRect(rect.x, rect.y, rect.w, rect.h);
}

/**
 * Bis zu welchem Rang Flüsse und Seen gezeigt werden: Auf einer Weltkarte
 * nur die großen, auf einer Inselkarte auch kleinere (Rang wie Natural
 * Earth: klein = wichtig).
 */
function detailRank(degPerMm: number): number {
  if (degPerMm > 0.5) return 3;
  if (degPerMm > 0.15) return 5;
  if (degPerMm > 0.05) return 7;
  return 10;
}

let mapCache: MapData | null = null;

/** Kartendaten (beim ersten Mal nachladen) */
export async function mapData(): Promise<MapData> {
  if (!mapCache) {
    const [countries, details] = await Promise.all([loadCountries(), loadMapDetails().catch(() => null)]);
    mapCache = { countries, details };
  }
  return mapCache;
}

/**
 * Zeichnet Texte und Karte einer Seite. Koordinaten in mm; `ctx` muss vorher
 * so skaliert sein, dass 1 Einheit = 1 mm ist und (0,0) die beschnittene
 * Seitenecke (beim Export also um die Beschnittzugabe verschoben).
 */
export function drawPage(ctx: CanvasRenderingContext2D, a: AlbumState, pageIndex: number, layout: PageLayout, map: MapData | null) {
  const page = a.pages[pageIndex];
  if (page.map && map)
    drawMap(ctx, a, layout.content, map, {
      labels: page.map.labels,
      names: page.map.names ?? {},
      cities: page.map.cities ?? true,
      peaks: page.map.peaks ?? true,
      water: page.map.water ?? true,
    });
  for (const t of layout.texts) drawText(ctx, a, t, layout);
}

/** Braucht die Seite eine Ebene (Texte oder Karte)? */
export function hasOverlay(a: AlbumState, pageIndex: number): boolean {
  const p = a.pages[pageIndex];
  return !!p.map || !!p.texts?.length;
}

/** Ebene einer Seite für den Export: PNG in Druckauflösung, Base64. */
export async function renderOverlay(a: AlbumState, pageIndex: number, layout: PageLayout): Promise<string | undefined> {
  if (!hasOverlay(a, pageIndex)) return undefined;
  await ensureFonts(a.settings.font);
  const map = a.pages[pageIndex].map ? await mapData() : null;
  const pxPerMm = DPI / 25.4;
  // Gleiche Rundung wie das Backend (export.rs, page_pixels)
  const w = Math.round((a.settings.widthMm + 2 * BLEED_MM) * pxPerMm);
  const h = Math.round((a.settings.heightMm + 2 * BLEED_MM) * pxPerMm);
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d")!;
  ctx.scale(pxPerMm, pxPerMm);
  ctx.translate(BLEED_MM, BLEED_MM);
  drawPage(ctx, a, pageIndex, layout, map);
  const blob = await new Promise<Blob | null>((r) => canvas.toBlob(r, "image/png"));
  canvas.width = canvas.height = 0;
  if (!blob) return undefined;
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

/** Für Hinweise: Sprache der Ländernamen */
export const countryName = (c: Country) => (lang === "de" ? c.de : c.en);
