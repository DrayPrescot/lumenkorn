// Baut die Windows-Installer lokal, signiert für den Updater und legt die
// passende latest.json daneben – für Releases ohne GitHub Actions.
//
//   node tools/release_local.mjs          alles nacheinander
//   node tools/release_local.mjs build    nur bauen, ohne Schlüssel und Passwort
//   node tools/release_local.mjs sign     signieren, latest.json schreiben, prüfen
//   node tools/release_local.mjs verify   Signaturen und latest.json prüfen
//
// Gebaut wird OHNE den Signaturschlüssel, signiert erst danach: Beim Bauen laufen
// die Build-Skripte aller Abhängigkeiten mit den Umgebungsvariablen dieses
// Prozesses. Eine kompromittierte Bibliothek könnte den Schlüssel sonst lesen
// und signierte Updates an alle Nutzer ausliefern.
//
// Das Passwort des Schlüssels fragt ein eigenes Fenster ab. So kann auch ein Agent
// den ganzen Ablauf starten: Das Passwort geht vom Fenster nur an den Signierschritt
// und erscheint in keiner Ausgabe. Der Nutzer gibt es für jedes Release selbst ein.
//
// In VS Code: Terminal → Aufgabe ausführen → „Lumenkorn: Release bauen“.
// Danach im Release auf GitHub hochladen: beide .msi und latest.json
// (das Skript zeigt die Dateien am Ende an).

import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { parsePublicKey, verifySignature } from "./minisign.mjs";

const root = resolve(import.meta.dirname, "..");
const fail = (msg) => {
  console.error(`\n✖ ${msg}`);
  process.exit(1);
};

const command = process.argv[2] ?? "all";
if (!["all", "build", "sign", "verify"].includes(command)) {
  fail(`Unbekannter Schritt „${command}“. Möglich sind build, sign und verify, ohne Angabe alle.`);
}

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

// Die Tauri-CLI direkt mit Node starten (kein Umweg über die Shell, so
// bleibt das JSON von --config unverändert)
const cli = join(root, "node_modules", "@tauri-apps", "cli", "tauri.js");
const dir = join(root, "target", "release", "bundle", "msi");
const latestPath = join(dir, "latest.json");

const PASSWORD_VAR = "TAURI_SIGNING_PRIVATE_KEY_PASSWORD";
const cleanEnv = { ...process.env };
for (const name of ["TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PATH", PASSWORD_VAR, "LUMENKORN_UPDATER_KEY"]) {
  delete cleanEnv[name];
}

function requireCli() {
  if (!existsSync(cli)) fail("Tauri-CLI fehlt – zuerst „npm ci“ ausführen.");
}

function installers() {
  const msis = existsSync(dir) ? readdirSync(dir).filter((f) => f.endsWith(".msi") && f.includes(`_${version}_`)) : [];
  if (!msis.length) fail(`Keine MSI für ${version} in ${dir} – zuerst bauen.`);
  return msis;
}

// Der Updater lädt das deutsche Installationsprogramm (die App selbst folgt
// der Systemsprache); ohne es das erste vorhandene
const updaterInstaller = (msis) => msis.find((f) => f.includes("de-DE")) ?? msis[0];
const downloadUrl = (msi) => `https://github.com/DrayPrescot/lumenkorn/releases/download/v${version}/${msi}`;

// --- Bauen ----------------------------------------------------------------
function build() {
  requireCli();
  console.log(`Lumenkorn ${version} wird gebaut …\n`);
  const res = spawnSync(process.execPath, [cli, "build", "--bundles", "msi"], { cwd: root, stdio: "inherit", env: cleanEnv });
  if (res.status !== 0) fail("Der Build ist fehlgeschlagen (Meldungen oben).");
}

// --- Signieren -------------------------------------------------------------------
const MAX_TRIES = 3;
const CANCELLED = 3; // Exit-Code des Fensters

/** Fragt in einem eigenen Fenster nach Schlüsseldatei und Passwort. Die Antwort nie ausgeben. */
function askPassword(keyPath, message) {
  const script = join(root, "tools", "ask_signing_password.ps1");
  const args = ["-NoProfile", "-STA", "-ExecutionPolicy", "Bypass", "-File", script, "-Version", version, "-KeyPath", keyPath, "-Message", message];
  // Kein windowsHide: Es gilt unter Windows auch für das erste Fenster des Prozesses,
  // das Passwortfenster bliebe unsichtbar und der Lauf hinge.
  const res = spawnSync("powershell.exe", args, { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] });
  if (res.status === CANCELLED) fail("Signieren abgebrochen, nichts signiert.");
  if (res.status !== 0) fail(`Das Passwortfenster ließ sich nicht öffnen (Status ${res.status}).`);
  return JSON.parse(Buffer.from(res.stdout.trim(), "base64").toString("utf8"));
}

function signOne(msi, keyPath, password) {
  return spawnSync(process.execPath, [cli, "signer", "sign", "--private-key-path", keyPath, "--app-version", version, join(dir, msi)], {
    cwd: root,
    encoding: "utf8",
    env: { ...cleanEnv, [PASSWORD_VAR]: password },
  });
}

function sign(msis) {
  requireCli();
  const KEY = "lumenkorn-updater.key";
  let keyPath = process.env.LUMENKORN_UPDATER_KEY || [join(homedir(), ".tauri", KEY), join(homedir(), KEY)].find(existsSync) || "";
  // Ohne Fenster nur, wenn jemand das Passwort selbst in seinem Terminal gesetzt hat
  const given = process.env[PASSWORD_VAR];
  if (given && !existsSync(keyPath)) fail(`Signaturschlüssel nicht gefunden – Pfad in LUMENKORN_UPDATER_KEY angeben oder ${KEY} nach ~/.tauri legen.`);

  let message = "";
  for (let attempt = 0; attempt < MAX_TRIES; attempt++) {
    const answer = given ? { key: keyPath, password: given } : askPassword(keyPath, message);
    keyPath = answer.key;
    const first = signOne(msis[0], keyPath, answer.password);
    if (first.status === 0) {
      for (const msi of msis.slice(1)) {
        const res = signOne(msi, keyPath, answer.password);
        if (res.status !== 0) fail(`Signieren von ${msi} fehlgeschlagen:\n${res.stdout}${res.stderr}`);
      }
      console.log(`✔ ${msis.length} Installer signiert`);
      return;
    }
    const output = `${first.stdout}${first.stderr}`;
    if (given || !/password/i.test(output)) fail(`Signieren von ${msis[0]} fehlgeschlagen:\n${output}`);
    message = "Das Passwort passt nicht zu dieser Schlüsseldatei.";
    console.error("Das Passwort war falsch, das Fenster fragt noch einmal.");
  }
  fail(`Nach ${MAX_TRIES} falschen Passwörtern abgebrochen, nichts signiert.`);
}

// --- latest.json --------------------------------------------------------------
function writeLatest(msis) {
  const main = updaterInstaller(msis);
  const latest = {
    version,
    notes,
    pub_date: new Date().toISOString().replace(/\.\d+Z$/, "Z"),
    platforms: {
      "windows-x86_64": {
        signature: readFileSync(join(dir, `${main}.sig`), "utf8").trim(),
        url: downloadUrl(main),
      },
    },
  };
  writeFileSync(latestPath, JSON.stringify(latest, null, 2) + "\n");
}

// --- Prüfen -----------------------------------------------------------------------
/** Prüft, was der Updater später prüft: Passen Signaturen, Version und latest.json zusammen? */
function verifyRelease(msis) {
  const publicKey = parsePublicKey(conf.plugins.updater.pubkey);
  for (const msi of msis) {
    const sigPath = join(dir, `${msi}.sig`);
    if (!existsSync(sigPath)) fail(`Signatur fehlt: ${sigPath}`);
    let signed;
    try {
      signed = verifySignature(publicKey, readFileSync(join(dir, msi)), readFileSync(sigPath, "utf8"));
    } catch (e) {
      fail(`${msi}: ${e.message}`);
    }
    if (signed.version !== version) fail(`${msi}: signiert für Version ${signed.version}, erwartet ${version}.`);
    if (signed.file !== msi) fail(`${msi}: signiert unter dem Namen ${signed.file}.`);
  }

  if (!existsSync(latestPath)) fail(`latest.json fehlt: ${latestPath}`);
  const latest = JSON.parse(readFileSync(latestPath, "utf8"));
  const main = updaterInstaller(msis);
  const platform = latest.platforms?.["windows-x86_64"];
  if (latest.version !== version) fail(`latest.json nennt Version ${latest.version}, erwartet ${version}.`);
  if (platform?.url !== downloadUrl(main)) fail(`latest.json verweist auf ${platform?.url}.`);
  if (platform.signature !== readFileSync(join(dir, `${main}.sig`), "utf8").trim()) fail("latest.json enthält eine andere Signatur als die Datei daneben.");
  console.log(`✔ Signaturen und latest.json für ${version} sind in Ordnung`);
}

// --- Ablauf ---------------------------------------------------------------------
if (command === "all" || command === "build") build();
if (command === "build") {
  console.log("\n✔ Gebaut. Weiter mit: node tools/release_local.mjs sign");
  process.exit(0);
}

const msis = installers();
if (command !== "verify") {
  sign(msis);
  writeLatest(msis);
}
verifyRelease(msis);

if (command !== "verify") {
  console.log(`\n✔ Fertig. Auf GitHub ein Release mit dem Tag v${version} anlegen und hochladen:`);
  for (const f of [...msis, "latest.json"]) console.log(`   ${join(dir, f)}`);
  console.log(`\n  Das Release muss veröffentlicht sein (kein Entwurf, keine Vorabversion).`);
}
