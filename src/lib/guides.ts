// Hilfslinien im Zuschnitt (wie „Upright mit Hilfslinien“): Linien entlang
// von Kanten, die senkrecht bzw. waagerecht stehen sollen. Aus ihnen werden
// Geraderichten und Perspektive so bestimmt, dass alle Linien danach gerade
// stehen – mit derselben Abbildung wie die Pipeline (`warpOf` in frame.ts).

import type { DevelopParams } from "./api";
import { frameSize, warpForward } from "./frame";

/** Endpunkte im gedrehten Rahmen vor Geraderichten und Perspektive (normiert) */
export interface Guide {
  a: [number, number];
  b: [number, number];
  /** soll senkrecht (v) bzw. waagerecht (h) stehen */
  kind: "v" | "h";
}

type Geo = Pick<DevelopParams, "rotate" | "flipHorizontal" | "straighten" | "perspectiveVertical" | "perspectiveHorizontal" | "crop">;
type Warp = Pick<DevelopParams, "straighten" | "perspectiveVertical" | "perspectiveHorizontal">;

const LIMIT: Record<keyof Warp, number> = { straighten: 45, perspectiveVertical: 100, perspectiveHorizontal: 100 };

/** Höchstens zwei Linien je Richtung */
export const MAX_PER_KIND = 2;

/** Winkel auf (−90°, 90°] bringen */
function fold(a: number): number {
  while (a > Math.PI / 2) a -= Math.PI;
  while (a <= -Math.PI / 2) a += Math.PI;
  return a;
}

/** Abweichung einer Strecke (Pixel) von senkrecht bzw. waagerecht, im Bogenmaß */
export function deviation(dx: number, dy: number, kind: "v" | "h"): number {
  return kind === "h" ? fold(Math.atan2(dy, dx)) : fold(Math.atan2(-dx, dy));
}

/** Nelder-Mead-Suche nach dem Minimum von `f` ab `x0` */
function nelderMead(f: (x: number[]) => number, x0: number[], step: number[], iters = 400): number[] {
  const n = x0.length;
  let pts = [x0, ...x0.map((_, i) => x0.map((v, j) => (j === i ? v + step[i] : v)))];
  let vals = pts.map(f);
  for (let it = 0; it < iters; it++) {
    const order = vals.map((_, i) => i).sort((a, b) => vals[a] - vals[b]);
    pts = order.map((i) => pts[i]);
    vals = order.map((i) => vals[i]);
    if (vals[n] - vals[0] < 1e-14) break;
    const c = x0.map((_, j) => pts.slice(0, n).reduce((s, p) => s + p[j], 0) / n);
    const along = (t: number) => c.map((v, j) => v + t * (pts[n][j] - v));
    const xr = along(-1);
    const fr = f(xr);
    if (fr < vals[0]) {
      const xe = along(-2);
      const fe = f(xe);
      [pts[n], vals[n]] = fe < fr ? [xe, fe] : [xr, fr];
    } else if (fr < vals[n - 1]) {
      [pts[n], vals[n]] = [xr, fr];
    } else {
      const xc = along(fr < vals[n] ? -0.5 : 0.5);
      const fc = f(xc);
      if (fc < Math.min(fr, vals[n])) [pts[n], vals[n]] = [xc, fc];
      else
        for (let i = 1; i <= n; i++) {
          pts[i] = pts[i].map((v, j) => pts[0][j] + 0.5 * (v - pts[0][j]));
          vals[i] = f(pts[i]);
        }
    }
  }
  return pts[vals.indexOf(Math.min(...vals))];
}

/**
 * Geraderichten und Perspektive, bei denen die Hilfslinien gerade stehen.
 * Frei sind: Geraderichten immer, Perspektive senkrecht ab zwei senkrechten,
 * waagerecht ab zwei waagerechten Linien; der Rest bleibt, wie er ist.
 */
export function solveGuides(guides: Guide[], p: Geo, full: [number, number]): Warp {
  const cur: Warp = {
    straighten: p.straighten,
    perspectiveVertical: p.perspectiveVertical ?? 0,
    perspectiveHorizontal: p.perspectiveHorizontal ?? 0,
  };
  if (!guides.length) return cur;
  const free: (keyof Warp)[] = ["straighten"];
  if (guides.filter((g) => g.kind === "v").length >= 2) free.push("perspectiveVertical");
  if (guides.filter((g) => g.kind === "h").length >= 2) free.push("perspectiveHorizontal");
  const [fw, fh] = frameSize(p, full);
  const geo = (x: number[]): Warp => {
    const g = { ...cur };
    free.forEach((k, i) => (g[k] = x[i]));
    return g;
  };
  const cost = (x: number[]) => {
    let pen = 0;
    free.forEach((k, i) => {
      const over = Math.abs(x[i]) - LIMIT[k];
      if (over > 0) pen += over * over;
    });
    const q = { ...p, ...geo(x) };
    let s = 0;
    for (const g of guides) {
      const [ax, ay] = warpForward(q, full, g.a[0], g.a[1]);
      const [bx, by] = warpForward(q, full, g.b[0], g.b[1]);
      s += deviation((bx - ax) * fw, (by - ay) * fh, g.kind) ** 2;
    }
    return s + pen;
  };
  const x = nelderMead(
    cost,
    free.map((k) => cur[k]),
    free.map((k) => (k === "straighten" ? 2 : 10)),
  );
  const out = geo(x);
  // Auf die Auflösung der Regler runden und begrenzen
  const round = (v: number, lim: number, f: number) => Math.round(Math.min(lim, Math.max(-lim, v)) * f) / f;
  return {
    straighten: round(out.straighten, 45, 100),
    perspectiveVertical: round(out.perspectiveVertical ?? 0, 100, 10),
    perspectiveHorizontal: round(out.perspectiveHorizontal ?? 0, 100, 10),
  };
}
