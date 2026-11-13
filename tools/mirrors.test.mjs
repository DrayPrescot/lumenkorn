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
