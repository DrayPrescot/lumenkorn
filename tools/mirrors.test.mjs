// Was Rust und Oberfläche getrennt pflegen, muss zusammenpassen. Fehlt etwas auf einer
// Seite, schlägt hier ein Test fehl statt erst in der App oder in der Browser-Vorschau.
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { DEFAULT_PARAMS } from "../src/lib/api.ts";

const root = join(import.meta.dirname, "..");
const read = (p) => readFileSync(join(root, p), "utf8");
const rustFiles = (dir) =>
  readdirSync(join(root, dir), { recursive: true })
    .filter((f) => f.endsWith(".rs"))
    .map((f) => read(join(dir, f)));
const matches = (text, re) => new Set([...text.matchAll(re)].map((m) => m[1]));
const sorted = (set) => [...set].sort();

const api = read("src/lib/api.ts");
const mock = read("src/lib/mock.ts");

/** Befehle, die `generate_handler![…]` in `lib.rs` registriert */
function registeredCommands() {
  const lib = read("src-tauri/src/lib.rs");
  const list = lib.match(/generate_handler!\[([\s\S]*?)\]\)/)[1].replace(/\/\/.*$/gm, "");
  return new Set(list.match(/[\w:]+/g).map((p) => p.split("::").pop()));
}

describe("Befehle", () => {
  const registered = registeredCommands();
  // `call("…")` geht im Browser an mock.invoke; `tauriInvoke("…")` (binäre Antworten)
  // hat in api.ts eine eigene Browser-Weiche.
  const viaCall = matches(api, /\bcall(?:<[^(]*?>)?\(\s*"(\w+)"/g);
  const direct = matches(api, /\btauriInvoke(?:<[^(]*?>)?\(\s*"(\w+)"/g);

  it("api.ts ruft nur registrierte Befehle", () => {
    expect(sorted([...viaCall, ...direct].filter((c) => !registered.has(c)))).toEqual([]);
  });

  it("jeder registrierte Befehl hat eine Methode in api.ts", () => {
    expect(sorted([...registered].filter((c) => !viaCall.has(c) && !direct.has(c)))).toEqual([]);
  });

  it("mock.ts kennt jeden Befehl, den api.ts über call() schickt", () => {
    const cases = matches(mock, /case "(\w+)"/g);
    expect(sorted([...viaCall].filter((c) => !cases.has(c)))).toEqual([]);
  });
});

describe("Ereignisse", () => {
  const backend = rustFiles("src-tauri/src").join("\n");
  const emitted = matches(backend, /\.emit(?:_to)?\(\s*(?:[^,"()]+,\s*)?"([a-z-]+)"/g);
  const typed = matches(api.match(/export interface Events \{([\s\S]*?)\n\}/)[1], /^\s+"([a-z-]+)":/gm);

  it("jedes gesendete Ereignis hat einen Typ in api.ts", () => {
    expect(sorted([...emitted].filter((e) => !typed.has(e)))).toEqual([]);
  });

  it("jeder Ereignistyp in api.ts wird auch gesendet", () => {
    expect(sorted([...typed].filter((e) => !emitted.has(e)))).toEqual([]);
  });
});

it("DEFAULT_PARAMS entspricht den Standardwerten in Rust", () => {
  // Rust prüft dieselbe Datei in crates/lumenkorn-develop/tests/params.rs
  const shared = JSON.parse(read("crates/lumenkorn-develop/tests/default-params.json"));
  expect(DEFAULT_PARAMS).toEqual(shared);
});

describe("Rechnungen, die src/lib aus Rust nachbaut", () => {
  // Erwartete Werte rechnet crates/lumenkorn-develop/src/mirrors.rs
  const rust = JSON.parse(read("crates/lumenkorn-develop/tests/mirrors.json"));
  const near = (a, b, tol = 1e-4) => expect(Math.abs(a - b)).toBeLessThanOrEqual(tol);

  it("Gradationskurve (curve.ts ↔ curve.rs)", async () => {
    const { spline } = await import("../src/lib/curve.ts");
    for (const c of rust.curves) {
      const f = spline(c.points);
      c.x.forEach((x, i) => near(f(x), c.y[i]));
    }
  });

  it("HSL-Bereiche (hsl.ts ↔ color.rs)", async () => {
    const { hslWeights } = await import("../src/lib/hsl.ts");
    for (const { hue, weights } of rust.hsl) hslWeights(hue).forEach((w, i) => near(w, weights[i]));
  });

  it("Geraderichten und Perspektive (frame.ts ↔ geometry.rs)", async () => {
    const { straightenScale, warpOf } = await import("../src/lib/frame.ts");
    for (const s of rust.straighten) near(straightenScale(s.w, s.h, s.deg), s.scale);
    for (const w of rust.warps) {
      const geo = { straighten: w.straighten, perspectiveVertical: w.vertical, perspectiveHorizontal: w.horizontal };
      const out = warpOf(geo, w.w, w.h);
      const m = out.m.flat();
      m.forEach((v, i) => near(v / m[8], w.m[i], Math.max(1e-4, Math.abs(w.m[i]) * 1e-4)));
      near(out.zoom, w.zoom);
    }
  });

  it("Dateitypen, die als einer gelten (filetypes.ts ↔ formats.rs)", async () => {
    const { typeExts } = await import("../src/lib/filetypes.ts");
    const src = read("crates/lumenkorn-core/src/formats.rs").match(/const SAME_TYPE: [^=]*= &\[([\s\S]*?)\];/)[1];
    const groups = [...src.matchAll(/&\[([^\]]*)\]/g)].map((g) => [...g[1].matchAll(/"(\w+)"/g)].map((x) => x[1]));
    expect(groups.length).toBeGreaterThan(0);
    for (const g of groups) for (const e of g) expect(typeExts(e)).toEqual(g);
  });
});
