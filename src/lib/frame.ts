// Umrechnung zwischen Originalbild und angezeigtem Bild – spiegelt exakt die
// Geometrie-Stufen der Rust-Pipeline (Vierteldrehung → Spiegeln → Perspektive
// und Geraderichten → Zuschnitt). Alle Koordinaten normiert 0…1.

import type { DevelopParams } from "./api";

type Geo = Pick<
  DevelopParams,
  "rotate" | "flipHorizontal" | "straighten" | "perspectiveVertical" | "perspectiveHorizontal" | "crop"
>;

/** Pixelmaße des gedrehten Rahmens (vor dem Zuschnitt). */
export function frameSize(p: Geo, full: [number, number]): [number, number] {
  const q = ((p.rotate % 4) + 4) % 4;
  return q % 2 ? [full[1], full[0]] : [full[0], full[1]];
}

/** Wie in geometry::straighten_scale */
export function straightenScale(w: number, h: number, deg: number): number {
  const t = Math.abs((deg * Math.PI) / 180);
  const s = Math.sin(t);
  const c = Math.cos(t);
  return Math.max(c + (h / w) * s, c + (w / h) * s);
}

type M3 = number[][];

const mul = (a: M3, b: M3): M3 => a.map((row) => [0, 1, 2].map((j) => row[0] * b[0][j] + row[1] * b[1][j] + row[2] * b[2][j]));
const translate = (x: number, y: number): M3 => [
  [1, 0, x],
  [0, 1, y],
  [0, 0, 1],
];

function apply(m: M3, x: number, y: number): [number, number] | null {
  const z = m[2][0] * x + m[2][1] * y + m[2][2];
  if (z <= 1e-9) return null;
  return [(m[0][0] * x + m[0][1] * y + m[0][2]) / z, (m[1][0] * x + m[1][1] * y + m[1][2]) / z];
}

function invert(m: M3): M3 {
  const [[a, b, c], [d, e, f], [g, h, i]] = m;
  const A = e * i - f * h;
  const B = -(d * i - f * g);
  const C = d * h - e * g;
  const det = a * A + b * B + c * C;
  return [
    [A / det, -(b * i - c * h) / det, (b * f - c * e) / det],
    [B / det, (a * i - c * g) / det, -(a * f - c * d) / det],
    [C / det, -(a * h - b * g) / det, (a * e - b * d) / det],
  ];
}

/** Wie geometry::PERSPECTIVE_MAX_DEG */
const PERSPECTIVE_MAX_DEG = 30;

/** Wie `geometry::Warp` in Rust: Ausgabe → Quelle in Pixeln des Rahmens
 *  (fw × fh) und die Vergrößerung. `null` ohne Geraderichten und Perspektive. */
export function warpOf(p: Geo, fw: number, fh: number): { m: M3; mInv: M3; zoom: number } | null {
  const v = p.perspectiveVertical ?? 0;
  const hz = p.perspectiveHorizontal ?? 0;
  const st = p.straighten;
  if ((!st && !v && !hz) || !fw || !fh) return null;
  // Masken rechnen jeden Punkt einzeln um: das letzte Ergebnis merken
  const key = `${st} ${v} ${hz} ${fw} ${fh}`;
  if (lastWarp?.key === key) return lastWarp.w;
  const w = computeWarp(st, v, hz, fw, fh);
  lastWarp = { key, w };
  return w;
}

let lastWarp: { key: string; w: ReturnType<typeof computeWarp> } | null = null;

function computeWarp(st: number, v: number, hz: number, fw: number, fh: number) {
  const [cx, cy] = [fw / 2, fh / 2];
  const a = (((v / 100) * PERSPECTIVE_MAX_DEG) * Math.PI) / 180;
  const b = (((hz / 100) * PERSPECTIVE_MAX_DEG) * Math.PI) / 180;
  const f = Math.max(fw, fh);
  const rx = [
    [1, 0, 0],
    [0, Math.cos(a), -Math.sin(a)],
    [0, Math.sin(a), Math.cos(a)],
  ];
  const ry = [
    [Math.cos(b), 0, Math.sin(b)],
    [0, 1, 0],
    [-Math.sin(b), 0, Math.cos(b)],
  ];
  const r = mul(ry, rx);
  const rt = [0, 1, 2].map((i) => [0, 1, 2].map((j) => r[j][i]));
  const k = [
    [f, 0, 0],
    [0, f, 0],
    [0, 0, 1],
  ];
  const kInv = [
    [1 / f, 0, 0],
    [0, 1 / f, 0],
    [0, 0, 1],
  ];
  const c0 = apply(mul(mul(k, r), kInv), 0, 0) ?? [0, 0];
  const persp = mul(mul(mul(k, rt), kInv), translate(c0[0], c0[1]));
  const t = (st * Math.PI) / 180;
  const unrot = [
    [Math.cos(t), Math.sin(t), 0],
    [-Math.sin(t), Math.cos(t), 0],
    [0, 0, 1],
  ];
  const base = mul(persp, unrot);
  let zoom: number;
  if (!v && !hz) zoom = straightenScale(fw, fh, st);
  else {
    const inside = (z: number) =>
      [
        [-cx, -cy],
        [cx, -cy],
        [cx, cy],
        [-cx, cy],
      ].every(([x, y]) => {
        const s = apply(base, x / z, y / z);
        return !!s && Math.abs(s[0]) <= cx * (1 + 1e-6) && Math.abs(s[1]) <= cy * (1 + 1e-6);
      });
    let [lo, hi] = [0.25, 50];
    if (inside(lo)) hi = lo;
    else
      for (let i = 0; i < 60; i++) {
        const mid = (lo + hi) / 2;
        if (inside(mid)) hi = mid;
        else lo = mid;
      }
    zoom = hi;
  }
  const scale = [
    [1 / zoom, 0, 0],
    [0, 1 / zoom, 0],
    [0, 0, 1],
  ];
  const m = mul(mul(mul(translate(cx, cy), base), scale), translate(-cx, -cy));
  // Mittlere Vergrößerung: Wurzel des Flächenverhältnisses (wie in Rust)
  const quad = [
    [0, 0],
    [fw, 0],
    [fw, fh],
    [0, fh],
  ].map(([x, y]) => apply(m, x, y) ?? [x, y]);
  const area = Math.abs(quad.reduce((s, a, i) => s + a[0] * quad[(i + 1) % 4][1] - quad[(i + 1) % 4][0] * a[1], 0)) / 2;
  return { m, mInv: invert(m), zoom: Math.sqrt((fw * fh) / Math.max(area, 1e-9)) };
}

/** Vergrößerung durch Geraderichten und Perspektive (1 ohne beides) */
export function warpZoom(p: Geo, full: [number, number]): number {
  const [fw, fh] = frameSize(p, full);
  return warpOf(p, fw, fh)?.zoom ?? 1;
}

/** Original → gedrehter, gerade gerichteter Rahmen (ohne Zuschnitt) */
export function sourceToFrame(p: Geo, full: [number, number], sx: number, sy: number): [number, number] {
  const q = ((p.rotate % 4) + 4) % 4;
  let [x, y] = q === 1 ? [1 - sy, sx] : q === 2 ? [1 - sx, 1 - sy] : q === 3 ? [sy, 1 - sx] : [sx, sy];
  if (p.flipHorizontal) x = 1 - x;
  const [fw, fh] = frameSize(p, full);
  const w = warpOf(p, fw, fh);
  if (w) {
    const o = apply(w.mInv, x * fw, y * fh);
    if (o) [x, y] = [o[0] / fw, o[1] / fh];
  }
  return [x, y];
}

export function frameToSource(p: Geo, full: [number, number], fx: number, fy: number): [number, number] {
  let [x, y] = [fx, fy];
  const [fw, fh] = frameSize(p, full);
  const w = warpOf(p, fw, fh);
  if (w) {
    const s = apply(w.m, x * fw, y * fh);
    if (s) [x, y] = [s[0] / fw, s[1] / fh];
  }
  if (p.flipHorizontal) x = 1 - x;
  const q = ((p.rotate % 4) + 4) % 4;
  return q === 1 ? [y, 1 - x] : q === 2 ? [1 - x, 1 - y] : q === 3 ? [1 - y, x] : [x, y];
}

/** Rahmen → Ausschnitt */
export function frameToCrop(p: Geo, fx: number, fy: number): [number, number] {
  const c = p.crop ?? { x: 0, y: 0, w: 1, h: 1 };
  return [(fx - c.x) / c.w, (fy - c.y) / c.h];
}

export function cropToFrame(p: Geo, cx: number, cy: number): [number, number] {
  const c = p.crop ?? { x: 0, y: 0, w: 1, h: 1 };
  return [c.x + cx * c.w, c.y + cy * c.h];
}

/** Rechteck des angezeigten Bildes in der Bühne (wie `object-fit: contain`),
 *  in Pixeln. Leer, solange eine der Größen noch unbekannt ist. */
export function containRect(stageW: number, stageH: number, imgW: number, imgH: number) {
  if (!imgW || !imgH || !stageW || !stageH) return { x: 0, y: 0, w: 0, h: 0 };
  const s = Math.min(stageW / imgW, stageH / imgH);
  const w = imgW * s;
  const h = imgH * s;
  return { x: (stageW - w) / 2, y: (stageH - h) / 2, w, h };
}
