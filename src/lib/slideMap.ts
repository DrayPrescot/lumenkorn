// Karten der Diashow: beim Ortswechsel fliegt die Kamera vom vorigen zum
// neuen Aufnahmeort (gleichmäßiges Zoomen und Schwenken nach van Wijk & Nuij,
// wie `flyTo` in Leaflet bzw. `interpolateZoom` in d3), die Route wird als
// Bogen nachgezeichnet, am Ziel fällt eine Stecknadel. Am Ende zeigt eine
// Karte alle Orte der Reise.
//
// Gezeichnet wird wie die Kartenseite im Fotoalbum: Ländergrenzen, besuchte
// Länder hervorgehoben, Flüsse, Seen, größere Städte und Berge aus den
// eingebauten Kartendaten (Natural Earth) – ohne Internet.

import { MAP, mapData, type MapData } from "./albumDraw";
import { countryAt } from "./countries";
import { lang, locale } from "./i18n";

export { mapData, type MapData };

export interface MapPlace {
  lat: number;
  lon: number;
  /** Kurzer Ortsname (leer: ohne) */
  label: string;
}

/** Ein Flug von einem Aufnahmeort zum nächsten */
export interface MapLeg {
  from: MapPlace;
  to: MapPlace;
  /** Alle Orte der Reise (ihre Länder werden hervorgehoben, wie im Fotoalbum) */
  visited?: MapPlace[];
}

/** Wie gezeichnet wird: Schrift */
export interface MapStyle {
  font: string;
}

/** Anteil der Zeit, in der die Karte am Start bzw. Ziel steht */
const HOLD_START = 0.15;
const HOLD_END = 0.25;
/** Steilheit der Flugkurve (d3: √2) */
const RHO = Math.SQRT2;

const rad = (deg: number) => (deg * Math.PI) / 180;
const mercY = (lat: number) => Math.log(Math.tan(Math.PI / 4 + (Math.max(-85, Math.min(85, lat)) * Math.PI) / 360));
const latOf = (y: number) => (Math.atan(Math.sinh(y)) * 180) / Math.PI;
const smooth = (x: number) => {
  const v = Math.min(1, Math.max(0, x));
  return v * v * (3 - 2 * v);
};

/** Entfernung in km (Großkreis) */
export function distanceKm(a: { lat: number; lon: number }, b: { lat: number; lon: number }): number {
  const dLat = rad(b.lat - a.lat);
  const dLon = rad(b.lon - a.lon);
  const h = Math.sin(dLat / 2) ** 2 + Math.cos(rad(a.lat)) * Math.cos(rad(b.lat)) * Math.sin(dLon / 2) ** 2;
  return 2 * 6371 * Math.asin(Math.min(1, Math.sqrt(h)));
}

/** Kamera: Mitte (Mercator, x = Länge im Bogenmaß) und sichtbare Breite */
type Camera = [number, number, number];

/** Gleichmäßiger Flug zwischen zwei Kameras (van Wijk & Nuij 2003). `t`: 0…1 */
export function flyTo(p0: Camera, p1: Camera): (t: number) => Camera {
  const [ux0, uy0, w0] = p0;
  const [ux1, uy1, w1] = p1;
  const dx = ux1 - ux0;
  const dy = uy1 - uy0;
  const d2 = dx * dx + dy * dy;
  const rho2 = RHO * RHO;
  if (d2 < 1e-12) {
    const s = Math.log(w1 / w0) / RHO;
    return (t) => [ux0 + t * dx, uy0 + t * dy, w0 * Math.exp(RHO * t * s)];
  }
  const d1 = Math.sqrt(d2);
  const b0 = (w1 * w1 - w0 * w0 + rho2 * rho2 * d2) / (2 * w0 * rho2 * d1);
  const b1 = (w1 * w1 - w0 * w0 - rho2 * rho2 * d2) / (2 * w1 * rho2 * d1);
  const r0 = Math.log(Math.sqrt(b0 * b0 + 1) - b0);
  const r1 = Math.log(Math.sqrt(b1 * b1 + 1) - b1);
  const s = (r1 - r0) / RHO;
  const cosh = Math.cosh(r0);
  const sinh = Math.sinh(r0);
  return (t) => {
    const u = (w0 / (rho2 * d1)) * (cosh * Math.tanh(RHO * t * s + r0) - sinh);
    return [ux0 + u * dx, uy0 + u * dy, (w0 * cosh) / Math.cosh(RHO * t * s + r0)];
  };
}

/** Ort in Mercator; `nearLon`: über die Datumsgrenze hinweg auf dem kürzeren Weg */
function toMerc(p: MapPlace, nearLon = p.lon): [number, number] {
  let lon = p.lon;
  while (lon - nearLon > 180) lon -= 360;
  while (nearLon - lon > 180) lon += 360;
  return [rad(lon), mercY(p.lat)];
}

/** Punkt auf dem Bogen von `a` nach `b` (nach Norden gewölbt), `s`: 0…1 */
function arcAt(a: [number, number], b: [number, number], s: number, bend = 0.18): [number, number] {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  let nx = -dy;
  let ny = dx;
  if (ny < 0) ((nx = -nx), (ny = -ny));
  const cx = (a[0] + b[0]) / 2 + nx * bend;
  const cy = (a[1] + b[1]) / 2 + ny * bend;
  const u = 1 - s;
  return [u * u * a[0] + 2 * u * s * cx + s * s * b[0], u * u * a[1] + 2 * u * s * cy + s * s * b[1]];
}

/**
 * Ausschnitt rund um einen Ort, so weit, dass die Entfernung `dist`
 * (Bogenmaß) gut zu sehen ist – aber nicht enger als ~2° (rund 150 km):
 * Näher heran zeigen die eingebauten Kartendaten kaum noch etwas (das
 * Fotoalbum zeigt aus demselben Grund mindestens ~3°).
 */
function nearWidth(dist: number): number {
  return Math.min(rad(14), Math.max(rad(2), dist * 2.2));
}

// --- Kamera der Abschnitte --------------------------------------------------------

/** Kamera und Routenstand des Flugs zum Zeitpunkt `p` */
function legState(leg: MapLeg, p: number): { cam: Camera; a: [number, number]; b: [number, number]; q: number } {
  const a = toMerc(leg.from);
  const b = toMerc(leg.to, leg.from.lon);
  const near = nearWidth(Math.hypot(b[0] - a[0], b[1] - a[1]));
  const q = smooth((p - HOLD_START) / (1 - HOLD_START - HOLD_END));
  return { cam: flyTo([a[0], a[1], near], [b[0], b[1], near])(q), a, b, q };
}

/** Ausschnitt mit allen Orten (mit Rand), passend zum Seitenverhältnis */
function fitAll(pts: [number, number][], aspect: number): Camera {
  const xs = pts.map((p) => p[0]);
  const ys = pts.map((p) => p[1]);
  const x0 = Math.min(...xs);
  const x1 = Math.max(...xs);
  const y0 = Math.min(...ys);
  const y1 = Math.max(...ys);
  const w = Math.max((x1 - x0) * 1.35, (y1 - y0) * 1.35 * aspect, rad(2));
  return [(x0 + x1) / 2, (y0 + y1) / 2, Math.min(w, 2 * Math.PI)];
}

function overviewState(places: MapPlace[], p: number, aspect: number) {
  const pts: [number, number][] = [];
  for (const pl of places) pts.push(toMerc(pl, pts.length ? (pts[pts.length - 1][0] * 180) / Math.PI : pl.lon));
  const last = pts[pts.length - 1];
  const prev = pts[pts.length - 2] ?? last;
  const end = fitAll(pts, aspect);
  const start: Camera = [last[0], last[1], nearWidth(Math.hypot(last[0] - prev[0], last[1] - prev[1]))];
  // Erst herauszoomen, dann die Route zeichnen, zuletzt die Namen
  const cam = flyTo(start, end)(smooth(p / 0.3));
  const route = smooth((p - 0.25) / 0.5);
  return { cam, pts, route };
}

// --- Zeichnen -----------------------------------------------------------------------

interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}
const overlaps = (b: Box, list: Box[]) => list.some((q) => b.x < q.x + q.w && q.x < b.x + b.w && b.y < q.y + q.h && q.y < b.y + b.h);

function haloText(ctx: CanvasRenderingContext2D, text: string, x: number, y: number, size: number, color: string) {
  ctx.lineWidth = size * 0.28;
  ctx.strokeStyle = "rgba(255,255,255,0.9)";
  ctx.strokeText(text, x, y);
  ctx.fillStyle = color;
  ctx.fillText(text, x, y);
}

function drawPin(ctx: CanvasRenderingContext2D, x: number, y: number, h: number) {
  const r = h * 0.32;
  const cy = y - h + r;
  ctx.beginPath();
  ctx.moveTo(x, y);
  ctx.bezierCurveTo(x - r * 0.35, y - h * 0.35, x - r, cy + r * 0.6, x - r, cy);
  ctx.arc(x, cy, r, Math.PI, 0);
  ctx.bezierCurveTo(x + r, cy + r * 0.6, x + r * 0.35, y - h * 0.35, x, y);
  ctx.closePath();
  ctx.save();
  ctx.fillStyle = MAP.pin;
  ctx.shadowColor = "rgba(0,0,0,0.3)";
  ctx.shadowBlur = h * 0.15;
  ctx.shadowOffsetY = h * 0.04;
  ctx.fill();
  ctx.restore();
  ctx.beginPath();
  ctx.arc(x, cy, r * 0.42, 0, Math.PI * 2);
  ctx.fillStyle = "#ffffff";
  ctx.fill();
}

/** Projektion der Kamera auf die Fläche */
function projector(cam: Camera, w: number, h: number) {
  const [cx, cy, vw] = cam;
  const vh = (vw * h) / w;
  return {
    x: (x: number) => ((x - cx) / vw) * w + w / 2,
    y: (y: number) => ((cy - y) / vh) * h + h / 2,
    lon0: ((cx - vw / 2) * 180) / Math.PI,
    lon1: ((cx + vw / 2) * 180) / Math.PI,
    lat0: latOf(cy - vh / 2),
    lat1: latOf(cy + vh / 2),
  };
}

/**
 * Grundkarte wie im Fotoalbum: Meer, Länder (besuchte hervorgehoben),
 * Gewässer, Berge und Städte. `keep`: Bereiche, die frei bleiben sollen
 * (Stecknadeln und ihre Namen), `skip`: Ortsnamen, die schon dort stehen.
 */
function drawBase(ctx: CanvasRenderingContext2D, cam: Camera, w: number, h: number, data: MapData | null, style: MapStyle, visitedAt: MapPlace[], keep: Box[], skip: Set<string>) {
  const pr = projector(cam, w, h);
  const side = Math.min(w, h);
  ctx.fillStyle = MAP.sea;
  ctx.fillRect(0, 0, w, h);
  // Datumsgrenze: Daten liegen in −180…180, die Kamera evtl. daneben
  const shifts = [0, ...(pr.lon0 < -180 ? [-360] : []), ...(pr.lon1 > 180 ? [360] : [])];
  const inView = ([a0, b0, a1, b1]: [number, number, number, number], s: number, pad = 0) =>
    !(a1 + s < pr.lon0 - pad || a0 + s > pr.lon1 + pad || b1 < pr.lat0 - pad || b0 > pr.lat1 + pad);
  // Punkte, die weniger als ~0,8 Pixel auseinanderliegen, überspringen
  const minStep = (0.8 / w) * cam[2];
  const path = (pts: [number, number][], s: number, close: boolean) => {
    let lx = Infinity;
    let ly = Infinity;
    pts.forEach(([lon, lat], i) => {
      const x = rad(lon + s);
      const y = mercY(lat);
      if (i > 0 && i < pts.length - 1 && Math.abs(x - lx) < minStep && Math.abs(y - ly) < minStep) return;
      lx = x;
      ly = y;
      if (i === 0) ctx.moveTo(pr.x(x), pr.y(y));
      else ctx.lineTo(pr.x(x), pr.y(y));
    });
    if (close) ctx.closePath();
  };
  if (data) {
    const visited = new Set(visitedAt.map((p) => countryAt(data.countries, p.lon, p.lat)?.iso).filter(Boolean));
    ctx.lineJoin = "round";
    ctx.lineCap = "round";
    for (const s of shifts)
      for (const c of data.countries) {
        if (!inView(c.bbox, s, 2)) continue;
        ctx.beginPath();
        for (const poly of c.polygons) for (const ring of poly) path(ring, s, true);
        ctx.fillStyle = visited.has(c.iso) ? MAP.visited : MAP.land;
        ctx.fill("evenodd");
        ctx.strokeStyle = MAP.border;
        ctx.lineWidth = Math.max(0.5, side * 0.0012);
        ctx.stroke();
      }
  }
  const d = data?.details;
  if (!d) return;
  const degPerPx = (pr.lon1 - pr.lon0) / w;
  // Je weiter herausgezoomt, desto nur die wichtigeren Gewässer und Städte
  const maxRank = degPerPx > 0.05 ? 3 : degPerPx > 0.015 ? 5 : degPerPx > 0.005 ? 7 : 10;
  ctx.fillStyle = MAP.sea;
  ctx.strokeStyle = MAP.river;
  ctx.lineWidth = Math.max(0.5, side * 0.0009);
  for (const l of d.lakes) {
    if (l.rank > maxRank || !inView(l.bbox, 0)) continue;
    ctx.beginPath();
    for (const poly of l.polygons) for (const ring of poly) path(ring, 0, true);
    ctx.fill("evenodd");
    ctx.stroke();
  }
  for (const r of d.rivers) {
    if (r.rank > maxRank || !inView(r.bbox, 0)) continue;
    ctx.lineWidth = Math.max(0.5, side * (0.003 - Math.min(r.rank, 10) * 0.0002));
    ctx.beginPath();
    for (const line of r.lines) path(line, 0, false);
    ctx.stroke();
  }
  // Berge und Städte nur, wo Platz ist (die Stecknadeln haben Vorrang)
  const size = side * 0.024;
  const placed = [...keep];
  const fits = (b: Box) => !overlaps(b, placed) && b.x >= 0 && b.x + b.w <= w && b.y >= 0 && b.y + b.h <= h;
  ctx.textBaseline = "middle";
  ctx.textAlign = "left";
  // Berge: die höchsten im Ausschnitt, Dreieck mit Name und Höhe
  ctx.font = `${size * 0.9}px ${style.font}`;
  const tri = size * 0.7;
  let peaks = 0;
  for (const pk of d.peaks.filter((x) => x.lon >= pr.lon0 && x.lon <= pr.lon1 && x.lat >= pr.lat0 && x.lat <= pr.lat1).sort((x, y) => y.elevation - x.elevation)) {
    if (peaks >= 6) break;
    const x = pr.x(rad(pk.lon));
    const y = pr.y(mercY(pk.lat));
    const text = `${pk.name} ${pk.elevation.toLocaleString(locale)} m`;
    const mark = { x: x - tri / 2, y: y - tri / 2, w: tri, h: tri };
    const label = { x: x + tri * 0.8, y: y - size * 0.6, w: ctx.measureText(text).width, h: size * 1.2 };
    if (!fits(mark) || !fits(label)) continue;
    placed.push(mark, label);
    peaks++;
    ctx.beginPath();
    ctx.moveTo(x, y - tri * 0.55);
    ctx.lineTo(x + tri * 0.55, y + tri * 0.4);
    ctx.lineTo(x - tri * 0.55, y + tri * 0.4);
    ctx.closePath();
    ctx.fillStyle = MAP.peak;
    ctx.fill();
    haloText(ctx, text, label.x, y, size * 0.9, MAP.peak);
  }
  ctx.font = `500 ${size}px ${style.font}`;
  const dot = size * 0.3;
  let shown = 0;
  const cities = d.cities.filter((c) => c.lon >= pr.lon0 && c.lon <= pr.lon1 && c.lat >= pr.lat0 && c.lat <= pr.lat1).sort((x, y) => x.rank - y.rank);
  for (const c of cities) {
    if (shown >= 12) break;
    const name = (lang === "de" && c.de) || c.en;
    if (!name || skip.has(name.toLowerCase())) continue;
    const x = pr.x(rad(c.lon));
    const y = pr.y(mercY(c.lat));
    const tw = ctx.measureText(name).width;
    const box = { x: x - dot * 2, y: y - size * 0.7, w: tw + dot * 4, h: size * 1.4 };
    if (!fits(box)) continue;
    placed.push(box);
    skip.add(name.toLowerCase());
    shown++;
    ctx.beginPath();
    ctx.arc(x, y, dot, 0, Math.PI * 2);
    ctx.fillStyle = MAP.city;
    ctx.fill();
    haloText(ctx, name, x + dot * 2, y, size, MAP.city);
  }
}

/** Route als Linie mit hellem Rand, gestrichelt */
function strokeRoute(ctx: CanvasRenderingContext2D, side: number) {
  ctx.lineWidth = side * 0.012;
  ctx.strokeStyle = "rgba(255,255,255,0.85)";
  ctx.setLineDash([]);
  ctx.stroke();
  ctx.lineWidth = side * 0.006;
  ctx.strokeStyle = MAP.pin;
  ctx.setLineDash([side * 0.018, side * 0.012]);
  ctx.stroke();
  ctx.setLineDash([]);
}

/**
 * Zeichnet den Flug `leg` zum Zeitpunkt `p` (0…1 über die Dauer der Karte)
 * in eine Fläche der Größe `w`×`h` (Pixel).
 */
export function drawMapLeg(ctx: CanvasRenderingContext2D, w: number, h: number, leg: MapLeg, p: number, data: MapData | null, style: MapStyle) {
  const { cam, a, b, q } = legState(leg, p);
  const pr = projector(cam, w, h);
  const side = Math.min(w, h);
  const pinH = side * 0.07;
  const size = side * 0.042;
  ctx.save();
  ctx.beginPath();
  ctx.rect(0, 0, w, h);
  ctx.clip();
  const keep = [a, b].map((pt) => ({ x: pr.x(pt[0]) - size * 3, y: pr.y(pt[1]) - size * 3, w: size * 12, h: size * 4 }));
  drawBase(ctx, cam, w, h, data, style, leg.visited ?? [leg.from, leg.to], keep, new Set([leg.from.label, leg.to.label].map((s) => s.toLowerCase())));

  // Route bis zur aktuellen Stelle, mit Punkt an der Spitze
  const head = Math.max(0, q);
  ctx.beginPath();
  for (let i = 0; i <= 64; i++) {
    const [x, y] = arcAt(a, b, (i / 64) * head);
    if (i === 0) ctx.moveTo(pr.x(x), pr.y(y));
    else ctx.lineTo(pr.x(x), pr.y(y));
  }
  strokeRoute(ctx, side);
  const [ax, ay] = [pr.x(a[0]), pr.y(a[1])];
  drawPin(ctx, ax, ay, pinH * 0.75);
  if (q > 0 && q < 1) {
    const [hx, hy] = arcAt(a, b, head);
    ctx.beginPath();
    ctx.arc(pr.x(hx), pr.y(hy), side * 0.011, 0, Math.PI * 2);
    ctx.fillStyle = MAP.pin;
    ctx.fill();
    ctx.lineWidth = side * 0.004;
    ctx.strokeStyle = "#fff";
    ctx.stroke();
  }
  // Ziel: Nadel fällt nach der Ankunft herein
  const arrive = smooth((p - (1 - HOLD_END)) / 0.1);
  const [bx, by] = [pr.x(b[0]), pr.y(b[1])];
  if (arrive > 0) {
    ctx.save();
    ctx.globalAlpha = arrive;
    drawPin(ctx, bx, by - (1 - arrive) * pinH * 0.6, pinH);
    ctx.restore();
  }
  // Namen: Start am Anfang, Ziel nach der Ankunft
  ctx.font = `600 ${size}px ${style.font}`;
  ctx.textBaseline = "middle";
  ctx.textAlign = "left";
  const fromAlpha = 1 - smooth((p - HOLD_START) / 0.15);
  if (leg.from.label && fromAlpha > 0) {
    ctx.save();
    ctx.globalAlpha = fromAlpha;
    haloText(ctx, leg.from.label, ax + pinH * 0.4, ay - pinH * 0.5, size * 0.8, MAP.text);
    ctx.restore();
  }
  if (leg.to.label && arrive > 0) {
    ctx.save();
    ctx.globalAlpha = arrive;
    haloText(ctx, leg.to.label, bx + pinH * 0.5, by - pinH * 0.6, size, MAP.text);
    ctx.restore();
  }
  ctx.restore();
}

/**
 * Karte mit allen Orten der Reise zum Zeitpunkt `p`: vom letzten Ort
 * herauszoomen, die ganze Route nachzeichnen, die Stecknadeln setzen und
 * zuletzt die Namen zeigen.
 */
export function drawOverview(ctx: CanvasRenderingContext2D, w: number, h: number, places: MapPlace[], p: number, data: MapData | null, style: MapStyle) {
  const { cam, pts, route } = overviewState(places, p, w / h);
  const pr = projector(cam, w, h);
  const side = Math.min(w, h);
  const pinH = side * 0.05;
  ctx.save();
  ctx.beginPath();
  ctx.rect(0, 0, w, h);
  ctx.clip();
  const xy = pts.map((pt) => [pr.x(pt[0]), pr.y(pt[1])] as [number, number]);
  const keep = xy.map(([x, y]) => ({ x: x - pinH * 0.5, y: y - pinH, w: pinH, h: pinH }));
  drawBase(ctx, cam, w, h, data, style, places, keep, new Set(places.map((pl) => pl.label.toLowerCase())));
  // Route: Bögen von Ort zu Ort, nacheinander
  const legs = pts.length - 1;
  const upto = route * legs;
  ctx.beginPath();
  for (let k = 0; k < legs && k < upto; k++) {
    const part = Math.min(1, upto - k);
    for (let i = 0; i <= 24; i++) {
      const [x, y] = arcAt(pts[k], pts[k + 1], (i / 24) * part, 0.12);
      if (i === 0 && k === 0) ctx.moveTo(pr.x(x), pr.y(y));
      else ctx.lineTo(pr.x(x), pr.y(y));
    }
  }
  strokeRoute(ctx, side * 0.8);
  // Stecknadeln, sobald die Route sie erreicht; nahe beieinander nur eine
  const pins: { x: number; y: number; label: string }[] = [];
  xy.forEach(([x, y], k) => {
    if (k > upto + 0.001 && route < 1) return;
    const near = pins.find((q) => Math.hypot(q.x - x, q.y - y) < pinH * 0.7);
    if (near) {
      if (!near.label) near.label = places[k].label;
      return;
    }
    pins.push({ x, y, label: places[k].label });
  });
  for (const pin of pins) drawPin(ctx, pin.x, pin.y, pinH);
  // Namen zum Schluss, so viele wie ohne Überlappung Platz haben
  const labels = smooth((p - 0.75) / 0.12);
  if (labels > 0) {
    const size = side * 0.03;
    ctx.font = `600 ${size}px ${style.font}`;
    ctx.textBaseline = "middle";
    ctx.textAlign = "left";
    ctx.globalAlpha = labels;
    const placed: Box[] = pins.map((q) => ({ x: q.x - pinH * 0.4, y: q.y - pinH, w: pinH * 0.8, h: pinH }));
    const seen = new Set<string>();
    for (const pin of pins) {
      if (!pin.label || seen.has(pin.label)) continue;
      const tw = ctx.measureText(pin.label).width;
      const cy = pin.y - pinH * 0.68;
      const box = [pin.x + pinH * 0.45, pin.x - pinH * 0.45 - tw]
        .map((bx) => ({ x: bx, y: cy - size * 0.6, w: tw, h: size * 1.2 }))
        .find((bb) => bb.x >= 0 && bb.x + bb.w <= w && bb.y >= 0 && bb.y + bb.h <= h && !overlaps(bb, placed));
      if (!box) continue;
      placed.push(box);
      seen.add(pin.label);
      haloText(ctx, pin.label, box.x, cy, size, MAP.text);
    }
  }
  ctx.restore();
}
