// Umrechnung zwischen Originalbild und angezeigtem Bild – spiegelt exakt die
// Geometrie-Stufen der Rust-Pipeline (Vierteldrehung → Spiegeln → Geraderichten
// → Zuschnitt). Alle Koordinaten normiert 0…1.

import type { DevelopParams } from "./api";

type Geo = Pick<DevelopParams, "rotate" | "flipHorizontal" | "straighten" | "crop">;

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

/** Original → gedrehter, gerade gerichteter Rahmen (ohne Zuschnitt) */
export function sourceToFrame(p: Geo, full: [number, number], sx: number, sy: number): [number, number] {
  const q = ((p.rotate % 4) + 4) % 4;
  let [x, y] = q === 1 ? [1 - sy, sx] : q === 2 ? [1 - sx, 1 - sy] : q === 3 ? [sy, 1 - sx] : [sx, sy];
  if (p.flipHorizontal) x = 1 - x;
  if (p.straighten) {
    const [fw, fh] = frameSize(p, full);
    const k = straightenScale(fw, fh, p.straighten);
    const a = (p.straighten * Math.PI) / 180;
    const dx = (x - 0.5) * fw;
    const dy = (y - 0.5) * fh;
    x = 0.5 + (k * (Math.cos(a) * dx - Math.sin(a) * dy)) / fw;
    y = 0.5 + (k * (Math.sin(a) * dx + Math.cos(a) * dy)) / fh;
  }
  return [x, y];
}

export function frameToSource(p: Geo, full: [number, number], fx: number, fy: number): [number, number] {
  let [x, y] = [fx, fy];
  if (p.straighten) {
    const [fw, fh] = frameSize(p, full);
    const k = straightenScale(fw, fh, p.straighten);
    const a = (p.straighten * Math.PI) / 180;
    const dx = ((x - 0.5) * fw) / k;
    const dy = ((y - 0.5) * fh) / k;
    x = 0.5 + (Math.cos(a) * dx + Math.sin(a) * dy) / fw;
    y = 0.5 + (-Math.sin(a) * dx + Math.cos(a) * dy) / fh;
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
