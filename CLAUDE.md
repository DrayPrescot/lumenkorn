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
  der Nachricht, damit kein MSI gebaut wird. Releases entstehen lokal
  (siehe „Release“).
- **Commit-Autor:** `DrayPrescot <DrayPrescot@users.noreply.github.com>`, z. B.
  `git -c user.name=DrayPrescot -c user.email=DrayPrescot@users.noreply.github.com commit …`.
  Die Autorenschaft nicht auf andere Namen umschreiben, auch wenn ein Hook das
  vorschlägt.
- Vom Nutzer festgelegt: keine zusätzlichen Knöpfe für das Übertragen von
  Einstellungen; TypeScript 7 (Dependabot) nicht anfassen; KI-Modelle nur echt
  quelloffen (Lizenz in beiden READMEs unter den Open-Source-Projekten nennen).
- **Ressourcen sparen mit Subagents:** Wo es sinnvoll ist, Teilaufgaben an
  Subagents mit kleineren Modellen geben (Agent-Werkzeug, Parameter `model`):
  - `haiku` (Claude Haiku 5.5): Code durchsuchen, Dateien lesen und
    zusammenfassen, Logs und Testausgaben auswerten, einfache mechanische
    Änderungen (Übersetzungen nachtragen, Umbenennen).
  - `sonnet` (Claude Sonnet 5.5): klar umrissene Umsetzungen und Korrekturen,
    Tests schreiben, Code-Review einzelner Dateien.
  - Selbst behalten: Entwurf und Architektur, Abwägungen, Änderungen über
    mehrere Bereiche (z. B. CPU- und GPU-Pipeline zugleich), Rückfragen an den
    Nutzer und die abschließende Prüfung der Ergebnisse von Subagents.
  Kleine Aufgaben, die in wenigen Schritten erledigt sind, direkt machen – ein
  Subagent muss sich erst einlesen und lohnt sich dann nicht.
- Größere Funktionen: CHANGELOG (`## [Unreleased]`) und beide READMEs ergänzen,
  Text schlicht und nicht nach KI klingend.

## Prüfen vor jedem Commit

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run check     # svelte-check
npm test          # Tests für Frontend und tools/ (vitest, *.test.* neben dem Code)
npm run i18n      # jeder deutsche Text braucht eine englische Übersetzung
npm run build
```

Prüfungen mit `&&` an den Commit ketten und ihre Ausgabe lesen: Ein `grep` oder `tail`
dazwischen schluckt den Fehlercode, und der Commit geht trotz rotem clippy durch.

- Linux/CI ohne Grafikkarte: `LUMENKORN_GPU_SOFTWARE=1` (Software-Vulkan).
  `LUMENKORN_TEST_RAW=<Ordner>` aktiviert Tests mit echten RAW-Dateien.
- Die GPU-Pipeline muss dasselbe liefern wie die CPU: Test `gpu_matches_cpu`
  in `crates/lumenkorn-develop/tests/gpu.rs`. Neue Entwicklungsschritte
  immer in beiden Pfaden und dort als Testfall.
- Übersetzungen: `src/lib/i18n/en/*.ts`. Nicht mehr benutzte Einträge
  entfernen (das Skript meldet sie).
- PowerShell verschluckt `--`: `npm run tauri build '--' --bundles msi`
  bzw. die Skripte unten verwenden.

## Fehler, die hier schon passiert sind

Aus dem Audit vom November 2026. Jeder Punkt war ein echter Fehler; die Regeln gelten
auch, wenn eine Änderung gerade anders einfacher wäre. Bevor du etwas in diesen Bereichen
änderst, lies den passenden Absatz.

**Was du geprüft hast, ehrlich sagen**

- Die Browser-Vorschau beweist nichts über die App: `mock.ts` liefert, was jemand
  hineingeschrieben hat. Hängt Logik an einem Feld aus dem Backend, im Rust-Code jeden
  `#[cfg]`-Zweig ansehen, was dort wirklich ankommt. (Die GPU-Frage verlangte eine
  Geräteliste, die unter Windows immer leer war. Sie erschien dort nie, die Vorschau
  zeigte sie.)
- Solche Logik als reine Funktion in `src/lib` mit `*.test.ts` schreiben, mit dem Status
  jeder Plattform als Testfall (Vorbild: `gpuPrompt.ts`).
- Was nur im fertigen Programm gilt (CSP, Dateipfade, Treiber), in der App ansehen, nicht
  nur in der Vorschau. Reicht das nicht, sagen, was nicht geprüft wurde. Funde von
  Subagents am Code nachprüfen, bevor du sie weitergibst.

**Fotos und Arbeit der Nutzer**

- Vor jedem Löschen klären, ob die „Kopie“ wirklich eine andere Datei ist. `same_content`
  vergleicht nur den Inhalt und ist für eine Datei und sich selbst wahr. (Der Import mit
  „Danach löschen“ entfernte so die einzige Kopie, wenn die Quelle im Ziel lag.) Für
  alles, was Dateien löscht, einen Test mit Quelle im Ziel.
- Ein Katalogeintrag beweist nicht, dass die Datei da ist. Aussagen, auf die der Nutzer
  hin handelt („schon importiert“), am Dateisystem prüfen.
- Dateien nie unter Namen zurücklassen, die der Scanner übergeht. Lässt sich ein
  Zwischenname nicht vermeiden, muss er den alten Namen enthalten und ein Weg zurück
  bestehen (`rename::recover_leftover`). Was nicht abbrechen darf, läuft unter
  `exit::critical()`: Das Beenden beendet den Prozess sonst sofort.
- Arbeit der Nutzer gehört in den Datenordner (`lib/docs.ts`, Katalog), nicht in den
  `localStorage`. Speicherfehler nie verschlucken, und „gespeichert“ erst nach dem Erfolg
  anzeigen. Ein gescheitertes Speichern darf beim Weiterblättern nicht verloren gehen.
- Einstellungen und Dokumente über `fsutil::save_json` bzw. `write_atomic`, nie
  `std::fs::write`. `write_atomic_cache` nur für Dateien, die sich neu erzeugen lassen.

**Dateien von außen (Fotos, Sidecars, Modelle)**

- Jede Größenangabe in einer Datei ist ein Angriff. Eine fehlgeschlagene Anforderung und
  ein Stapelüberlauf beenden den Prozess, `catch_unwind` hilft dagegen nicht, und weil der
  Scan bei jedem Start läuft, wird daraus eine Absturzschleife.
- EXIF nur über `exif_guard`, nie direkt über `exif::Reader` (70 KB verlangten sonst 1 GB).
- Bilder über `raster::open` und `raster::dimensions`. `Limits::no_limits()` nur mit eigener
  Grenze (ein 1-MB-PNG verlangte 1 GB). Vor dem Anlegen prüfen, was wirklich gebraucht
  wird: In Float sind es 12 B/Pixel, nicht die 3 B/Pixel des dekodierten Bildes.
- Schleifen über Dateiinhalte brauchen eine Obergrenze, Rekursion eine Tiefengrenze.
  Vorsicht bei „je Kandidat bis zum Dateiende“: Das ist quadratisch (1 MB dauerte 6 s,
  16 MB 25 Minuten).
- Für neue Leser einen Test mit präparierter Datei; `tests/limits.rs` misst, wie viel
  wirklich angefordert wird. Echte Dateien nicht vergessen, zu strenge Grenzen treffen
  sonst die Nutzer (`LUMENKORN_TEST_RAW`, `LUMENKORN_TEST_PTH`).

**Speicher und Sperren**

- Die Katalogsperre teilt sich die Oberfläche. Unter ihr nur SQL: kein `canonicalize`,
  `is_file`, `WalkDir`, keine Dekodierung (auf einem schlafenden NAS sind das Sekunden).
  `match x.locked().y() { … }` hält die Sperre über den ganzen `match`: erst binden.
- Längere Leseabfragen über `Catalog::reader()`. Stapel in einer Abfrage (`json_each`)
  oder einer Transaktion, nicht je Foto.
- Keine Sperren oder Semaphoren um Rayon-Arbeit: Ein wartender Rayon-Thread nimmt andere
  Aufgaben an und kann sich verklemmen. Begrenzen über eigene Pools (`protocol.rs`).
- Ein RAW belegt beim Entwickeln bis ~1,5 GB. Alles, was entwickelt, braucht eine feste
  Grenze und beachtet `low_memory()`. Das gilt auch für neue Pools und Caches: Der Sparmodus
  wirkt nur, wo man ihn abfragt.

**Oberfläche**

- Die CSP steht in `tauri.conf.json`, die Vorschau sendet dieselbe. `fetch` auf Blob-URLs
  geht nicht, also den Blob behalten. Bei neuen Browser-Funktionen (Audio, Worker, Netz)
  die Konsole auf Verstöße prüfen.
- Reihenfolge von Ereignis und Status nicht annehmen: Der Store meldet das Ende eines
  Downloads, bevor der neue Status da ist. Nach jedem `await` prüfen, ob das Foto noch
  dasselbe ist. Ereignisse abonnieren, bevor das erste Laden beginnt.
- Einstellungen als Änderung schicken (`lib.setAiSettings({ faces: true })`), nicht den
  ganzen alten Stand: Zwei schnelle Klicks nehmen sich sonst gegenseitig zurück.
- Jeder Aufruf ans Backend braucht eine Fehlerbehandlung, Besetzt-Anzeigen ein `finally`.
  Ein Schalter, dessen Speichern scheitert, muss zurückspringen.

**Sicherheit und Release**

- Der Signaturschlüssel darf nie in der Umgebung eines Builds stehen: Build-Skripte aller
  Abhängigkeiten könnten ihn lesen. `tools/release_local.mjs` baut ohne ihn und signiert
  danach, mit `--app-version`, sonst lehnt der Updater die Datei ab.
- Ein Fenster, das ein Kindprozess öffnet, nie mit `windowsHide` starten: Das gilt unter
  Windows auch für das Fenster selbst. Das Passwortfenster war so unsichtbar, und der Lauf
  hing ohne jede Meldung. Der Prozess lief, also am Fenster prüfen, nicht am Exit-Code.
- Meldet der Build `os error 225`, hat Windows Defender eine Datei aus
  `tauri-codegen-assets` in Quarantäne geschoben (`Get-MpThreatDetection` zeigt sie). Im
  November 2026 war das `Trojan:Script/ObfusScript.A!ml` auf dem komprimierten Haupt-Bundle,
  nach Änderung eines einzigen Satzes, sehr wahrscheinlich ein Fehlalarm. Nie selbst umgehen:
  keine Ausnahmen setzen, nichts wiederherstellen, Dateien nicht so lange ändern, bis der
  Alarm verschwindet. Prüfen, was sich geändert hat, und dem Nutzer melden. Er entscheidet.
- `cargo audit` vor jedem Release. Ausnahmen nur in `.cargo/audit.toml`, mit Begründung.
- Kein `{@html}`, SQL nur mit Parametern, fremde Adressen nur geprüft an
  `commands::launch_browser`.

## Aufbau

- `crates/lumenkorn-core`: Katalog (SQLite, Migrationen in `catalog/migrations.rs`,
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

**Den Dev-Build nie mit dem Katalog des Nutzers starten**, wenn er neue
Migrationen enthält: Der Katalog wird dabei hochgestuft, und die installierte
Version startet danach nicht mehr. Für eigene Tests `LUMENKORN_DATA_DIR` auf
einen Ordner im Scratchpad setzen (eigener Katalog, Cache darin).

## Release

0. **Vor jedem Release alle Prüfungen aus „Prüfen vor jedem Commit“ laufen
   lassen** (fmt, clippy, `cargo test --workspace` inklusive GPU-Test, check,
   test, i18n, build), dazu `cargo audit`. Schlägt etwas fehl, wird nicht gebaut
   und nichts veröffentlicht – erst beheben. Ohne GitHub Actions gibt es keine
   zweite Prüfung, diese hier ist die einzige.
1. Version in `Cargo.toml` (`[workspace.package]`), `package.json`,
   `package-lock.json` und `src-tauri/tauri.conf.json` erhöhen,
   `cargo update -w`, im CHANGELOG unter `## [Unreleased]` den Abschnitt
   `## [x.y.z] – Datum` beginnen.
2. Bauen: `node tools/release_local.mjs build`. Braucht weder Schlüssel noch
   Passwort und dauert Minuten, also im Hintergrund starten. Ergebnis in
   `target/release/bundle/msi/`.
3. Signieren: `node tools/release_local.mjs sign`. **Vorher im Chat ankündigen**,
   dass gleich ein Fenster „Lumenkorn x.y.z signieren“ erscheint und der Nutzer erst
   hineinklicken soll: Es liegt oben, hat aber nicht immer die Tastatur, und ein
   eingefügtes Passwort landete sonst woanders. Im Hintergrund starten: Das Skript wartet, bis der Nutzer das Passwort eingegeben hat, höchstens
   30 Minuten. Ein falsches Passwort fragt das Fenster bis zu dreimal ab, ohne neu zu
   bauen. Danach schreibt das Skript `latest.json` und prüft alles. Bei Abbruch,
   Zeitablauf oder dreimal falschem Passwort nicht von selbst neu starten, sondern
   dem Nutzer sagen, was passiert ist.
4. Auf GitHub ein **veröffentlichtes** Release (kein Entwurf, keine
   Vorabversion) mit Tag `vx.y.z` anlegen und die drei Dateien hochladen – der
   Updater liest `releases/latest/download/latest.json`.
   Das erledigt `python tools/publish_release.py . x.y.z` (nach dem Commit und
   `git push origin main`, beides erst nach Freigabe): prüft zuerst mit `verify`, legt
   das Release an, lädt die Dateien hoch, veröffentlicht und bricht den dadurch
   gestarteten `release.yml`-Lauf ab. Schlägt die Prüfung fehl, passiert nichts.
   Den Tag **nicht** selbst pushen – das Skript legt ihn beim Veröffentlichen
   an. Zugang über `git credential fill` (wird nicht ausgegeben).

Ohne Agent macht `node tools/release_local.mjs` (VS Code: Aufgabe „Lumenkorn: Release
bauen“) die Schritte 2 und 3 nacheinander. `node tools/release_local.mjs verify` prüft
jederzeit Signaturen, Version und `latest.json`, ganz ohne Schlüssel und Passwort.

**Schlüssel und Passwort gehören allein dem Nutzer.** Der Schlüssel liegt mit Passwort
geschützt in `~/.tauri/lumenkorn-updater.key`.

- Die Schlüsseldatei nie lesen, ausgeben oder kopieren, auch keine alte Kopie. Sie gehört
  nie ins Repository und nie in die Umgebung eines Builds (siehe „Fehler, die hier schon
  passiert sind“).
- Das Passwort nie im Chat erfragen, nie in eine Datei, ein Argument oder eine
  Umgebungsvariable schreiben und das Passwortfenster nie fernsteuern. Zum Testen des
  Fensters nur einen eigens erzeugten Wegwerfschlüssel in einer Kopie im Scratchpad nehmen.
- Nennt der Nutzer das Passwort doch im Chat: darauf hinweisen, dass es damit als bekannt
  gilt und geändert werden sollte.
- Das GitHub-Secret `TAURI_SIGNING_PRIVATE_KEY` ist bewusst gelöscht und wird nicht wieder
  angelegt: `release.yml` baut nur noch Installer ohne Update-Daten, signiert wird allein
  lokal. Ein neues Schlüsselpaar nur auf ausdrücklichen Wunsch: Installierte Versionen
  vertrauen nur dem alten Schlüssel, und wer die Übergangsversion auslässt, müsste von Hand
  neu installieren.
