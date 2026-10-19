// Gradationskurve: derselbe monotone kubische Spline wie in
// lumenkorn-develop/src/curve.rs (Fritsch–Carlson), damit die gezeichnete
// Kurve genau dem entspricht, was die Pipeline rechnet.

export type CurvePoint = [number, number];
export type CurveChannel = "rgb" | "red" | "green" | "blue";

/** Höchstzahl der Punkte je Kurve (params.rs: CURVE_POINTS) */
export const CURVE_POINTS = 16;
/** Mindestabstand zweier Punkte in x (params.rs: ToneCurve::points) */
export const CURVE_GAP = 0.01;
export const IDENTITY: CurvePoint[] = [
  [0, 0],
  [1, 1],
];

/** Liegt die Kurve auf der Diagonalen (ändert nichts)? */
export function isIdentity(pts: CurvePoint[]): boolean {
  return (
    pts.length < 2 ||
    (pts.every(([x, y]) => Math.abs(x - y) < 1e-4) && pts[0][0] <= 1e-4 && pts[pts.length - 1][0] >= 1 - 1e-4)
  );
}

/** Kurve als Funktion 0…1 → 0…1 */
export function spline(points: CurvePoint[]): (v: number) => number {
  if (points.length < 2) return (v) => v;
  const x = points.map((p) => p[0]);
  const y = points.map((p) => p[1]);
  const n = x.length;
  const d = Array.from({ length: n - 1 }, (_, k) => (y[k + 1] - y[k]) / (x[k + 1] - x[k]));
  const m = new Array<number>(n).fill(0);
  m[0] = d[0];
  m[n - 1] = d[n - 2];
  for (let k = 1; k < n - 1; k++) m[k] = d[k - 1] * d[k] <= 0 ? 0 : (d[k - 1] + d[k]) / 2;
  for (let k = 0; k < n - 1; k++) {
    if (d[k] === 0) {
      m[k] = 0;
      m[k + 1] = 0;
      continue;
    }
    const a = m[k] / d[k];
    const b = m[k + 1] / d[k];
    const s = a * a + b * b;
    if (s > 9) {
      const t = 3 / Math.sqrt(s);
      m[k] = t * a * d[k];
      m[k + 1] = t * b * d[k];
    }
  }
  return (v) => {
    if (v <= x[0]) return y[0];
    if (v >= x[n - 1]) return y[n - 1];
    let k = 0;
    while (k < n - 2 && x[k + 1] <= v) k++;
    const h = x[k + 1] - x[k];
    const t = (v - x[k]) / h;
    const t2 = t * t;
    const t3 = t2 * t;
    const r =
      (2 * t3 - 3 * t2 + 1) * y[k] + (t3 - 2 * t2 + t) * h * m[k] + (-2 * t3 + 3 * t2) * y[k + 1] + (t3 - t2) * h * m[k + 1];
    return Math.min(1, Math.max(0, r));
  };
}
