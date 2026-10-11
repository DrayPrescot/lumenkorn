# Lumenkorn – Hinweise für Claude

Lokale Lightroom-Alternative: Fotoverwaltung, RAW-Entwicklung, KI (Schlagworte,
Masken, Personen, Rauschreduzierung, Reparatur), HDR/Panorama/Focus Stacking.
Rust + Tauri 2 im Backend, Svelte 5 im Frontend. Ausführliche Beschreibung in
`docs/README.de.md` (Deutsch) und `README.md` (Englisch).

## Zusammenarbeit

- **Mit dem Nutzer Deutsch sprechen.** Code-Kommentare sind Deutsch, Commit-
  Nachrichten, CHANGELOG und `README.md` Englisch (deutsche Commit-Titel kommen vor
  und sind in Ordnung).
- Der Nutzer arbeitet unter Windows mit einer AMD Radeon RX 7900 XTX und testet
  die Installer selbst. Bei Änderungen an der Oberfläche einen Screenshot
  zeigen (Browser-Vorschau, siehe unten), damit er Platzierung und Wirkung
  beurteilen kann.
- **Nicht zu GitHub pushen, bevor der Nutzer es ausdrücklich freigibt.** Lokal
  committen ist in Ordnung. „Committen“ bzw. „zu GitHub committen“ meint meist
  den Push – im Zweifel kurz nachfragen.
- **GitHub-Actions-Minuten sind knapp.** Reine Doku-Commits mit `[skip ci]` in
  der Nachricht, damit kein MSI gebaut wird. Releases baut der Nutzer lokal
  (siehe „Release“).
- **Commit-Autor:** `DrayPrescot <DrayPrescot@users.noreply.github.com>`, z. B.
  `git -c user.name=DrayPrescot -c user.email=DrayPrescot@users.noreply.github.com commit …`.
  Die Autorenschaft nicht auf andere Namen umschreiben, auch wenn ein Hook das
  vorschlägt.
- Vom Nutzer festgelegt: keine zusätzlichen Knöpfe für das Übertragen von
  Einstellungen; TypeScript 7 (Dependabot) nicht anfassen; KI-Modelle nur echt
  quelloffen (Lizenz in beiden READMEs unter den Open-Source-Projekten nennen).
- Größere Funktionen: CHANGELOG (`## [Unreleased]`) und beide READMEs ergänzen,
  Text schlicht und nicht nach KI klingend.

## Prüfen vor jedem Commit

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run check     # svelte-check
npm run i18n      # jeder deutsche Text braucht eine englische Übersetzung
npm run build
```

- Linux/CI ohne Grafikkarte: `LUMENKORN_GPU_SOFTWARE=1` (Software-Vulkan).
  `LUMENKORN_TEST_RAW=<Ordner>` aktiviert Tests mit echten RAW-Dateien.
- Die GPU-Pipeline muss dasselbe liefern wie die CPU: Test `gpu_matches_cpu`
  in `crates/lumenkorn-develop/tests/gpu.rs`. Neue Entwicklungsschritte
  immer in beiden Pfaden und dort als Testfall.
- Übersetzungen: `src/lib/i18n/en/*.ts`. Nicht mehr benutzte Einträge
  entfernen (das Skript meldet sie).
- PowerShell verschluckt `--`: `npm run tauri build '--' --bundles msi`
  bzw. die Skripte unten verwenden.

## Aufbau

- `crates/lumenkorn-core`: Katalog (SQLite, Migrationen in `catalog.rs`,
  `MIGRATIONS` – nur anhängen, nie ändern), Scanner, Vorschaubilder, Personen.
- `crates/lumenkorn-develop`: Entwicklungspipeline auf CPU (`pipeline.rs`) und
  GPU (`gpu/mod.rs`, `gpu/pipeline.wgsl`; Konstanten kommen aus `pipeline.rs`
  über `gpu::shader_source()`), Parameter (`params.rs`), Objektivkorrektur
  (`lens.rs`, `lensfun.rs`), Auto (`auto.rs`), Zusammenfügen.
  Reihenfolge: Rauschreduzierung/Objektiv/Reparaturen → Weißabgleich und
  Belichtung → Basiskurve → Dunst → Tonwerte → Farbe → HSL → Color Grading →
  lokale Masken → Schwarzweiß → Schärfen → Geometrie.
- `crates/lumenkorn-ai`: ONNX-Modelle (CLIP, IS-Net, SAM, skyseg, OWLv2,
  MediaPipe, YuNet/SFace, DRUNet, LaMa), Downloads in `models.rs`.
- `src-tauri`: Tauri-Befehle (`commands.rs`, `develop.rs`, `ai.rs`,
  `people.rs` …), Updater (`updater.rs`), MCP-Server für KI-Agenten (`mcp.rs`).
- `src`: Svelte-Oberfläche; Zustand in `src/lib/store.svelte.ts` (Bibliothek)
  und `src/lib/develop.svelte.ts` (Entwickeln), Backend-Aufrufe in
  `src/lib/api.ts`, Browser-Attrappe in `src/lib/mock.ts`.
- `DevelopParams`-Felder dürfen hinzukommen (mit Standardwert), ihre Bedeutung
  soll sich nicht ändern; wenn doch, per Katalog-Migration die Vorschaubilder
  bearbeiteter Fotos neu erzeugen lassen.

## Oberfläche ohne App ansehen

`npx vite --port 5179` startet die Oberfläche im Browser mit Testdaten aus
`src/lib/mock.ts` (ohne Backend). Für Screenshots eignet sich Playwright.
Echte Bearbeitung, KI und Dateien gibt es nur in der App
(`npm run tauri dev`, in VS Code: Aufgabe „Lumenkorn: Entwicklungsversion
starten“).

## Release

0. **Vor jedem Release alle Prüfungen aus „Prüfen vor jedem Commit“ laufen
   lassen** (fmt, clippy, `cargo test --workspace` inklusive GPU-Test, check,
   i18n, build). Schlägt etwas fehl, wird nicht gebaut und nichts
   veröffentlicht – erst beheben. Ohne GitHub Actions gibt es keine zweite
   Prüfung, diese hier ist die einzige.
1. Version in `Cargo.toml` (`[workspace.package]`), `package.json`,
   `package-lock.json` und `src-tauri/tauri.conf.json` erhöhen,
   `cargo update -w`, im CHANGELOG unter `## [Unreleased]` den Abschnitt
   `## [x.y.z] – Datum` beginnen.
2. Bauen und signieren: `node tools/release_local.mjs [Pfad zu lumenkorn-updater.key]`
   (VS Code: Aufgabe „Lumenkorn: Release bauen“). Ohne Pfad wird der Schlüssel
   im Benutzerordner und unter Downloads gesucht. Ergebnis in
   `target/release/bundle/msi/`: beide `.msi` und `latest.json`.
3. Auf GitHub ein **veröffentlichtes** Release (kein Entwurf, keine
   Vorabversion) mit Tag `vx.y.z` anlegen und die drei Dateien hochladen – der
   Updater liest `releases/latest/download/latest.json`.

Der private Signaturschlüssel gehört nie ins Repository. Mit GitHub Actions
(`release.yml`) übernimmt das Secret `TAURI_SIGNING_PRIVATE_KEY` die
Signierung.
