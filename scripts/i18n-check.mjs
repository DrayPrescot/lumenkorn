// Prüft, ob jeder Text in t("…") eine englische Übersetzung hat und ob
// Platzhalter ({n}, {name} …) in beiden Sprachen übereinstimmen.
// Aufruf: npm run i18n
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

const root = new URL("..", import.meta.url).pathname;
const files = [];
(function walk(d) {
  for (const f of readdirSync(d)) {
    const p = join(d, f);
    if (statSync(p).isDirectory()) walk(p);
    else if (/\.(svelte|ts)$/.test(f) && !p.includes("/i18n/")) files.push(p);
  }
})(join(root, "src"));

// Schlüssel aus dem Quelltext: t("…") mit doppelten Anführungszeichen
// außerdem t(bedingung ? "…" : "…") und tc("kontext", "…") (Schlüssel „kontext|…“)
const keys = new Map();
const str = String.raw`"((?:[^"\\]|\\.)*)"`;
const patterns = [
  new RegExp(String.raw`\bt\(\s*` + str, "g"),
  new RegExp(String.raw`\bt\(\s*[^";]*?\?\s*` + str + String.raw`\s*:\s*` + str, "g"),
  new RegExp(String.raw`\btc\(\s*` + str + String.raw`\s*,\s*` + str, "g"),
];
const add = (k, f) => keys.has(k) || keys.set(k, f.replace(root, ""));
const lit = (s) => JSON.parse(`"${s}"`);
for (const f of files) {
  const src = readFileSync(f, "utf8");
  for (const m of src.matchAll(patterns[0])) add(lit(m[1]), f);
  for (const m of src.matchAll(patterns[1])) (add(lit(m[1]), f), add(lit(m[2]), f));
  for (const m of src.matchAll(patterns[2])) add(`${lit(m[1])}|${lit(m[2])}`, f);
}

// Wörterbücher: Objekt-Literale `"de": "en",` in src/lib/i18n/en/*.ts
const dict = new Map();
const dir = join(root, "src/lib/i18n/en");
const pair = /^\s*("(?:[^"\\]|\\.)*")\s*:\s*("(?:[^"\\]|\\.)*")\s*,?\s*$/;
const dupes = [];
for (const f of readdirSync(dir).filter((f) => f !== "index.ts")) {
  for (const line of readFileSync(join(dir, f), "utf8").split("\n")) {
    const m = line.match(pair);
    if (!m) continue;
    const k = JSON.parse(m[1]);
    if (dict.has(k) && dict.get(k) !== JSON.parse(m[2])) dupes.push(`${k} (${f})`);
    dict.set(k, JSON.parse(m[2]));
  }
}

const vars = (s) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(",");
const problems = [];
for (const [k, f] of keys) {
  if (!dict.has(k)) problems.push(`fehlt: ${JSON.stringify(k)}  (${f})`);
  else if (vars(k) !== vars(dict.get(k))) problems.push(`Platzhalter: ${JSON.stringify(k)} → ${JSON.stringify(dict.get(k))}`);
}
for (const d of dupes) problems.push(`widersprüchlich doppelt: ${d}`);
const unused = [...dict.keys()].filter((k) => !keys.has(k));

console.log(`${keys.size} Texte, ${dict.size} Übersetzungen, ${unused.length} unbenutzt`);
if (process.argv.includes("--unused")) for (const u of unused) console.log(`unbenutzt: ${JSON.stringify(u)}`);
for (const p of problems) console.log(p);
process.exit(problems.length ? 1 : 0);
