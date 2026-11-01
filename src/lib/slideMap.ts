// Animierte Karte der Diashow beim Ortswechsel: Die Kamera fliegt vom
// vorigen zum neuen Aufnahmeort (gleichmäßiges Zoomen und Schwenken nach
// van Wijk & Nuij, wie `flyTo` in Leaflet bzw. `interpolateZoom` in d3), die
// Route wird als Bogen nachgezeichnet, am Ziel fällt eine Stecknadel.
// Gezeichnet wird mit den eingebauten Kartendaten (Natural Earth) wie im
// Fotoalbum – ohne Internet, für Vorschau, Vollbild und Video gleich.

import { MAP, mapData, type MapData } from "./albumDraw";
import { countryAt } from "./countries";
import { lang } from "./i18n";

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
}

/** Anteil der Zeit, in der die Karte am Start bzw. Ziel steht */
const HOLD_START = 0.15;
const HOLD_END = 0.25;
/** Steilheit der Flugkurve (d3: √2) */
const RHO = Math.SQRT2;

const rad = (deg: number) => (deg * Math.PI) / 180;
const mercY = (lat: number) => Math.log(Math.tan(Math.PI / 4 + (Math.max(-80, Math.min(80, lat)) * Math.PI) / 360));
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

/** Ort in Mercator; das Ziel über die Datumsgrenze hinweg auf dem kürzeren Weg */
function legPoints(leg: MapLeg): { a: [number, number]; b: [number, number] } {
  let lonB = leg.to.lon;
  if (lonB - leg.from.lon > 180) lonB -= 360;
  else if (leg.from.lon - lonB > 180) lonB += 360;
  return { a: [rad(leg.from.lon), mercY(leg.from.lat)], b: [rad(lonB), mercY(leg.to.lat)] };
}

/** Punkt auf der Route (Bogen, nach Norden gewölbt), `s`: 0…1 */
function routeAt(a: [number, number], b: [number, number], s: number): [number, number] {
  const mx = (a[0] + b[0]) / 2;
  const my = (a[1] + b[1]) / 2;
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  // Senkrechte, nach oben (Norden) zeigend
  let nx = -dy;
  let ny = dx;
  if (ny < 0) ((nx = -nx), (ny = -ny));
  const cx = mx + nx * 0.18;
  const cy = my + ny * 0.18;
  const u = 1 - s;
  return [u * u * a[0] + 2 * u * s * cx + s * s * b[0], u * u * a[1] + 2 * u * s * cy + s * s * b[1]];
}

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

/**
 * Zeichnet den Flug `leg` zum Zeitpunkt `p` (0…1 über die Dauer der Karte)
 * in eine Fläche der Größe `w`×`h` (Pixel). `font`: CSS-Schriftfamilie.
 */
export function drawMapLeg(ctx: CanvasRenderingContext2D, w: number, h: number, leg: MapLeg, p: number, data: MapData | null, font: string) {
  const { a, b } = legPoints(leg);
  const dist = Math.hypot(b[0] - a[0], b[1] - a[1]);
  // Am Start und Ziel: etwas Umgebung (mindestens ~250 km, bei weiten Reisen mehr)
  const near = Math.min(rad(14), Math.max(rad(2.5), dist * 0.6));
  const fly = flyTo([a[0], a[1], near], [b[0], b[1], near]);
  const q = smooth((p - HOLD_START) / (1 - HOLD_START - HOLD_END));
  const [cx, cy, vw] = fly(q);
  const vh = (vw * h) / w;
  const toX = (x: number) => ((x - cx) / vw) * w + w / 2;
  const toY = (y: number) => ((cy - y) / vh) * h + h / 2;
  const lon0 = ((cx - vw / 2) * 180) / Math.PI;
  const lon1 = ((cx + vw / 2) * 180) / Math.PI;
  const lat0 = latOf(cy - vh / 2);
  const lat1 = latOf(cy + vh / 2);
  // Länge der Datumsgrenze: Daten liegen in −180…180, die Kamera evtl. daneben
  const shifts = [0, ...(lon0 < -180 ? [-360] : []), ...(lon1 > 180 ? [360] : [])];
  const inView = ([a0, b0, a1, b1]: [number, number, number, number], s: number, pad = 0) =>
    !(a1 + s < lon0 - pad || a0 + s > lon1 + pad || b1 < lat0 - pad || b0 > lat1 + pad);
  // Punkte, die weniger als ~0,8 Pixel auseinanderliegen, überspringen
  const minStep = (0.8 / w) * vw;
  const path = (pts: [number, number][], s: number, close: boolean) => {
    let lx = Infinity;
    let ly = Infinity;
    pts.forEach(([lon, lat], i) => {
      const x = rad(lon + s);
      const y = mercY(lat);
      if (i > 0 && i < pts.length - 1 && Math.abs(x - lx) < minStep && Math.abs(y - ly) < minStep) return;
      lx = x;
      ly = y;
      if (i === 0) ctx.moveTo(toX(x), toY(y));
      else ctx.lineTo(toX(x), toY(y));
    });
    if (close) ctx.closePath();
  };

  ctx.save();
  ctx.beginPath();
  ctx.rect(0, 0, w, h);
  ctx.clip();
  ctx.fillStyle = MAP.sea;
  ctx.fillRect(0, 0, w, h);
  const side = Math.min(w, h);
  if (data) {
    const visited = new Set([countryAt(data.countries, leg.from.lon, leg.from.lat)?.iso, countryAt(data.countries, leg.to.lon, leg.to.lat)?.iso].filter(Boolean));
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
    const d = data.details;
    const degPerPx = (lon1 - lon0) / w;
    if (d) {
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
    }
    // Städte nur, wo Platz ist (die Stecknadeln haben Vorrang)
    if (d) {
      const size = side * 0.024;
      const placed: Box[] = [];
      for (const pt of [a, b]) placed.push({ x: toX(pt[0]) - size * 3, y: toY(pt[1]) - size * 3, w: size * 12, h: size * 4 });
      ctx.font = `500 ${size}px ${font}`;
      ctx.textBaseline = "middle";
      ctx.textAlign = "left";
      const dot = size * 0.3;
      const skip = new Set([leg.from.label, leg.to.label].map((s) => s.toLowerCase()));
      let shown = 0;
      const cities = d.cities.filter((c) => c.lon >= lon0 && c.lon <= lon1 && c.lat >= lat0 && c.lat <= lat1).sort((x, y) => x.rank - y.rank);
      for (const c of cities) {
        if (shown >= 12) break;
        const name = (lang === "de" && c.de) || c.en;
        if (!name || skip.has(name.toLowerCase())) continue;
        const x = toX(rad(c.lon));
        const y = toY(mercY(c.lat));
        const tw = ctx.measureText(name).width;
        const box = { x: x - dot * 2, y: y - size * 0.7, w: tw + dot * 4, h: size * 1.4 };
        if (overlaps(box, placed) || box.x < 0 || box.x + box.w > w || box.y < 0 || box.y + box.h > h) continue;
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
  }

  // Route bis zur aktuellen Stelle, gestrichelt, mit Punkt an der Spitze
  const steps = 64;
  const head = Math.max(0, q);
  ctx.beginPath();
  for (let i = 0; i <= steps; i++) {
    const s = (i / steps) * head;
    const [x, y] = routeAt(a, b, s);
    if (i === 0) ctx.moveTo(toX(x), toY(y));
    else ctx.lineTo(toX(x), toY(y));
  }
  ctx.lineWidth = side * 0.012;
  ctx.strokeStyle = "rgba(255,255,255,0.85)";
  ctx.setLineDash([]);
  ctx.stroke();
  ctx.lineWidth = side * 0.006;
  ctx.strokeStyle = MAP.pin;
  ctx.setLineDash([side * 0.018, side * 0.012]);
  ctx.stroke();
  ctx.setLineDash([]);
  const pinH = side * 0.07;
  const [ax, ay] = [toX(a[0]), toY(a[1])];
  drawPin(ctx, ax, ay, pinH * 0.75);
  if (q > 0 && q < 1) {
    const [hx, hy] = routeAt(a, b, head);
    ctx.beginPath();
    ctx.arc(toX(hx), toY(hy), side * 0.011, 0, Math.PI * 2);
    ctx.fillStyle = MAP.pin;
    ctx.fill();
    ctx.lineWidth = side * 0.004;
    ctx.strokeStyle = "#fff";
    ctx.stroke();
  }
  // Ziel: Nadel fällt nach der Ankunft herein
  const arrive = smooth((p - (1 - HOLD_END)) / 0.1);
  const [bx, by] = [toX(b[0]), toY(b[1])];
  if (arrive > 0) {
    ctx.save();
    ctx.globalAlpha = arrive;
    drawPin(ctx, bx, by - (1 - arrive) * pinH * 0.6, pinH);
    ctx.restore();
  }
  // Namen: Start am Anfang, Ziel nach der Ankunft
  const size = side * 0.042;
  ctx.font = `600 ${size}px ${font}`;
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
