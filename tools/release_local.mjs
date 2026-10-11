// Baut die Windows-Installer lokal, signiert für den Updater und legt die
// passende latest.json daneben – für Releases ohne GitHub Actions.
//
//   node tools/release_local.mjs <Pfad zur lumenkorn-updater.key>
//
// In VS Code: Terminal → Aufgabe ausführen → „Lumenkorn: Release bauen“.
// Danach im Release auf GitHub hochladen: beide .msi und latest.json
// (das Skript zeigt die Dateien am Ende an).

import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const fail = (msg) => {
  console.error(`\n✖ ${msg}`);
  process.exit(1);
};

// --- Schlüssel ------------------------------------------------------------
// Ohne Angabe: übliche Ablageorte (Benutzerordner, Downloads)
const KEY = "lumenkorn-updater.key";
const keyPath =
  process.argv[2]?.trim() ||
  process.env.LUMENKORN_UPDATER_KEY ||
  [join(homedir(), KEY), join(homedir(), ".tauri", KEY), join(homedir(), "Downloads", KEY)].find(existsSync);
if (!keyPath) fail(`Signaturschlüssel nicht gefunden – Pfad angeben oder ${KEY} in den Benutzerordner oder nach Downloads legen.`);
if (!existsSync(keyPath)) fail(`Schlüssel nicht gefunden: ${keyPath}`);
const key = readFileSync(keyPath, "utf8").trim();
if (!key) fail(`Schlüsseldatei ist leer: ${keyPath}`);

// --- Version und Notizen ----------------------------------------------------
const conf = JSON.parse(readFileSync(join(root, "src-tauri", "tauri.conf.json"), "utf8"));
const version = conf.version;
const changelog = readFileSync(join(root, "CHANGELOG.md"), "utf8");
const start = changelog.indexOf(`## [${version}]`);
let notes = "";
if (start >= 0) {
  const body = changelog.slice(changelog.indexOf("\n", start) + 1);
  const end = body.search(/^## \[/m);
  notes = (end >= 0 ? body.slice(0, end) : body).trim();
} else {
  console.warn(`⚠ Kein Abschnitt „## [${version}]“ im CHANGELOG – das Update hat keinen Text.`);
}
console.log(`Lumenkorn ${version} wird gebaut …\n`);

// --- Bauen ----------------------------------------------------------------
// Die Tauri-CLI direkt mit Node starten (kein Umweg über die Shell, so
// bleibt das JSON von --config unverändert)
const cli = join(root, "node_modules", "@tauri-apps", "cli", "tauri.js");
if (!existsSync(cli)) fail("Tauri-CLI fehlt – zuerst „npm ci“ ausführen.");
const res = spawnSync(
  process.execPath,
  [cli, "build", "--bundles", "msi", "--config", JSON.stringify({ bundle: { createUpdaterArtifacts: true } })],
  {
    cwd: root,
    stdio: "inherit",
    env: { ...process.env, TAURI_SIGNING_PRIVATE_KEY: key, TAURI_SIGNING_PRIVATE_KEY_PASSWORD: process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ?? "" },
  },
);
if (res.status !== 0) fail("Der Build ist fehlgeschlagen (Meldungen oben).");

// --- latest.json --------------------------------------------------------------
const dir = join(root, "target", "release", "bundle", "msi");
const msis = readdirSync(dir).filter((f) => f.endsWith(".msi") && f.includes(`_${version}_`));
if (!msis.length) fail(`Keine MSI für ${version} in ${dir}.`);
// Der Updater lädt das deutsche Installationsprogramm (die App selbst folgt
// der Systemsprache); ohne es das erste vorhandene
const main = msis.find((f) => f.includes("de-DE")) ?? msis[0];
const sigFile = join(dir, `${main}.sig`);
if (!existsSync(sigFile)) fail(`Signatur fehlt: ${sigFile} – war der Schlüssel richtig?`);
const latest = {
  version,
  notes,
  pub_date: new Date().toISOString().replace(/\.\d+Z$/, "Z"),
  platforms: {
    "windows-x86_64": {
      signature: readFileSync(sigFile, "utf8").trim(),
      url: `https://github.com/DrayPrescot/lumenkorn/releases/download/v${version}/${main}`,
    },
  },
};
const latestPath = join(dir, "latest.json");
writeFileSync(latestPath, JSON.stringify(latest, null, 2) + "\n");

console.log(`\n✔ Fertig. Auf GitHub ein Release mit dem Tag v${version} anlegen und hochladen:`);
for (const f of [...msis, "latest.json"]) console.log(`   ${join(dir, f)}`);
console.log(`\n  Das Release muss veröffentlicht sein (kein Entwurf, keine Vorabversion).`);
