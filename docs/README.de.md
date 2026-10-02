# Lumenkorn

*[English version](../README.md)*

Lokale Fotoverwaltung und -entwicklung als Alternative zu Lightroom.
Rust-Kern, Web-Oberfläche, läuft unter Windows, Linux und (mit Einschränkungen) Android.
Bildbearbeitung und KI nutzen die Grafikkarte (AMD, NVIDIA, Intel) über Vulkan bzw. DirectX 12.

**Stand:** Bibliothek, non-destruktive RAW-Entwicklung mit Export, lokale Masken
(Verlauf, Radial, Pinsel, KI: Motiv, Himmel, Objekt per Klick), KI-Rauschreduzierung
und lokale KI für Verschlagwortung und Inhaltssuche.

## Bibliothek

- **Ordner statt Import.** Du fügst einen Ordner hinzu, Lumenkorn liest ihn ein. Nichts wird kopiert, verschoben oder verändert.
- **Live-Abgleich.** Neue, geänderte, gelöschte Dateien werden per Dateiüberwachung erkannt (1,5 s nach der letzten Änderung, gebündelt). Abgeglichen werden nur die gemeldeten Dateien und Ordner, nicht der ganze Quellordner – eine neue Datei neben 50 000 anderen kostet einen Dateizugriff statt 50 000 (wichtig bei NAS und Netzlaufwerken). Umbenannte oder verschobene Ordner werden am Inhalt wiedererkannt, Bearbeitungen bleiben erhalten. Nur wenn das System Ereignisse verliert (Pufferüberlauf), wird vollständig abgeglichen; beim Start ebenfalls.
- **Umbenennen und Verschieben** außerhalb der App wird über einen Inhalts-Hash erkannt. Bewertung, Schlagworte und Bearbeitungen bleiben am Bild.
- **Projekte** bündeln beliebig viele Ordner, auch von verschiedenen Laufwerken. Pro Ordner wählbar, ob Unterordner dazugehören. Neue Fotos in diesen Ordnern landen automatisch im Projekt; überlappende Ordner führen nicht zu Dubletten.
- **Thumbnails** für JPEG, PNG, TIFF, WebP und RAW (NEF, CR2, CR3, ARW, RAF, DNG, ORF, RW2 …). Unbearbeitete RAWs nutzen das eingebettete Kamera-JPEG (schnell); bearbeitete Bilder zeigen die Bearbeitung.
- **EXIF:** Aufnahmezeit, Kamera, Objektiv, ISO, Belichtung, Blende, Brennweite, Orientierung.
- **Seitenleiste:** Die Bereiche „Projekte“, „Personen“ und „Ordner“ lassen sich per Klick auf die
  Überschrift einklappen (bleibt nach einem Neustart so). Eingeklappt bleibt der gerade gewählte
  Eintrag sichtbar.
- **Raster** im Blocksatz mit echten Seitenverhältnissen, virtualisiert, Einzelansicht mit Filmstreifen.
- **Suche** über Dateiname, Ordner, Kamera, Objektiv, Schlagworte und Personen – oder nach Bildinhalt (KI). Mehrere Wörter müssen alle passen, Teilwörter genügen („sonnen“ findet „Sonnenuntergang“); Groß-/Kleinschreibung, Umlaute und Akzente spielen keine Rolle („cafe“ findet „Café“). Ein Volltextindex (SQLite FTS5) macht das auch bei 50 000 Fotos in Millisekunden. Filter nach Mindestbewertung, Sortierung.

## Entwickeln (non-destruktiv)

Taste **D** oder „Entwickeln“ oben. Die Originaldatei wird nie verändert; gespeichert wird nur
der Parametersatz (JSON im Katalog). Änderungen werden automatisch gespeichert.

- **RAW-Dekodierung** mit [rawler](https://github.com/dnglab/dnglab) (reines Rust, auch für Android), getestet mit Canon CR2 und Nikon NEF.
- **Pipeline in linearem Float:** Weißabgleich und Belichtung szenenbezogen, eine Basiskurve mit weichem Lichter-Rolloff für RAWs, danach Tonwerte und Farbe. Unbearbeitete JPEGs bleiben exakt wie sie sind.
- **Regler:** Temperatur, Tönung · Belichtung, Kontrast, Lichter, Tiefen, Weiß, Schwarz · Klarheit, Dynamik, Sättigung · Schärfen · Drehen und Spiegeln.
- **HSL** (im Reiter Basis): Farbton, Sättigung und Luminanz für acht Farbbereiche (Rot, Orange,
  Gelb, Grün, Aquamarin, Blau, Lila, Magenta). Gerechnet in Oklch mit weichen Übergängen zwischen
  benachbarten Bereichen; graue Flächen bleiben bei Farbton und Luminanz unberührt (Blau abdunkeln
  dunkelt keine grauen Wolken ab). Farbton ±30°, Sättigung bis grau bzw. doppelt, Luminanz ±50 %.
  Mit der **Pipette** auf eine Farbe im Bild klicken und nach oben (mehr) oder unten (weniger)
  ziehen – verstellt werden die getroffenen Bereiche anteilig (eine Anzeige nennt sie, z. B.
  „Blau 70 % · Lila 30 %“); gemessen wird im angezeigten, bearbeiteten Bild. Esc beendet die Pipette.
- **Zuschneiden** (Taste R): Rahmen über dem ganzen Bild ziehen, mit Drittel-Raster und festen Seitenverhältnissen (Original, 1:1, 4:5, 3:2, 16:9, per ⇄ hoch/quer). Enter übernimmt, Esc bricht ab.
- **Geraderichten** um beliebige Winkel (±45°) ohne leere Ecken, mit **Auto**-Funktion: Lumenkorn sucht Horizont und gerade Kanten und schlägt die Korrektur vor (im Test auf ±0,6° genau).
- Lichter, Tiefen und Klarheit arbeiten **lokal** über weichgezeichnete Helligkeitskarten, deren Radius relativ zur Bildgröße ist – die Vorschau sieht aus wie der Export.
- **Live-Vorschau** aus einem 2048-px-Proxy im Speicher (RAW einmal dekodieren, danach ~100–250 ms pro Änderung), Histogramm mit Clipping-Anteil, **Vorher/Nachher**.
- **Verlauf** (Strg+Z / Strg+Umschalt+Z), **Schnappschüsse**, **Kopieren/Einfügen** von Einstellungen auf beliebig viele ausgewählte Fotos.
- **Export** in voller Auflösung als JPEG, 16-Bit-TIFF oder PNG, optional verkleinert, im Hintergrund mit Fortschritt. Vorhandene Dateien werden nie überschrieben.

## Masken (lokale Anpassungen)

Im Entwickeln-Modus unter „Masken“. Jede Maskenebene hat eigene Regler (Belichtung, Kontrast,
Lichter, Tiefen, Klarheit, Temperatur, Tönung, Sättigung) und kann aus mehreren Formen bestehen,
die hinzugefügt oder abgezogen werden – z. B. „Himmel minus Kirchturm“.

- **Verlauf** und **Radial** werden direkt auf dem Bild gezogen und über Griffe nachjustiert.
- **Pinsel** mit Größe und weicher Kante; Alt-Taste oder „Radieren“ nimmt Bereiche wieder heraus.
- **Motiv** (KI, IS-Net): erkennt das Hauptmotiv automatisch.
- **Himmel**: findet den Himmel über eine Heuristik (helle, glatte Flächen vom oberen Rand, Sonne/Mond eingeschlossen). Sind die KI-Masken installiert, verfeinert Segment Anything die Kante.
- **Objekt** (KI, Segment Anything): auf ein Objekt klicken, weitere Klicks verfeinern, Alt-Klick nimmt heraus.
- **Farbe** (Pipette): auf eine Farbe im Bild klicken – die Maske erfasst alle Stellen ähnlicher
  Farbe, auch in Licht und Schatten. Weitere Klicks nehmen Farbtöne hinzu, Alt-Klick entfernt den
  nächstgelegenen. Der Regler **Farbbereich** bestimmt die Toleranz (beim Ziehen wird die Auswahl
  rot gezeigt). Gemessen wird im unbearbeiteten Bild, verglichen in Oklab (wahrnehmungsgerecht,
  vor allem nach Farbart, Helligkeit zählt schwach). Gespeichert werden die gemessenen Farben,
  nicht die Klickstellen. Kombinierbar wie alle Formen, z. B. „Farbe Blau minus Radial“.
- **Maske zeigen** (Taste O) legt die Maske rot über das Bild.

Masken beziehen sich auf das aufrechte Original. Drehen, Spiegeln und späteres Zuschneiden
verschieben sie nicht vom Motiv. KI-Masken liegen als PNG unter `<daten>/masken/`; die Bearbeitung
speichert ihre ID und – bei „Objekt“ – die Klickpunkte. Auch Thumbnails und Export zeigen die Masken.

Die KI-Masken (IS-Net ~179 MB, Segment Anything ViT-B quantisiert ~109 MB) lädt die App über
„KI“ unten links von den GitHub-Releases von [rembg](https://github.com/danielgatis/rembg).
**Diese Modelle habe ich mit echten Gewichten getestet**: Motiv-Erkennung, Objekt-Klick
(Fläche auf wenige Prozent genau) und Himmel-Verfeinerung. Auf zwei CPU-Kernen dauert die erste
Objekt-Auswahl pro Bild etwa 8 s (das Bild-Embedding wird danach für weitere Klicks wiederverwendet,
jeder weitere Klick ~0,2 s), die Motiv-Erkennung etwa 5 s. Auf einem aktuellen Rechner mit 8 Kernen
ist es entsprechend schneller.

## KI-Rauschreduzierung

Im Entwickeln-Modus unter „Details“. Verwendet **DRUNet** (Zhang et al., KAIR, MIT-Lizenz).

- **Kein inoffiziell gehostetes Modell:** Lumenkorn lädt die Original-Gewichte (130 MB) aus dem
  GitHub-Release von KAIR und baut daraus lokal ein ONNX-Modell. Dafür enthält `lumenkorn-ai` einen
  kleinen Leser für das alte PyTorch-Format und einen ONNX-Schreiber – PyTorch wird nicht gebraucht.
- **Rauschstärke wird geschätzt**, getrennt nach Helligkeit (Schatten rauschen stärker). Der Regler
  **Stärke** skaliert diese Schätzung vor der Berechnung (50 = Schätzung, 65 = Standard, höher = glatter).
- Gerechnet wird **einmal in voller Auflösung** im Hintergrund, mit Fortschrittsanzeige und Abbrechen.
  Das Ergebnis liegt als Zwischenstufe in `<daten>/entrauscht/` (lineares RGB, f16) und wird von
  Vorschau, Thumbnails und Export genutzt. **Mischung** blendet danach ohne Neuberechnung zum Original.
- Getestet mit den echten Gewichten: Gaußrauschen σ = 20 → PSNR von 23,0 auf 30,4 dB, Strukturen
  bleiben erhalten; Download, Konvertierung und Berechnung auch über die Oberfläche durchgespielt.
- **Dauer:** auf der CPU etwa 20 s pro Megapixel auf zwei Kernen, also bei 24 MP rund 8 Minuten auf
  einem schwachen und etwa 2 Minuten auf einem 8-Kern-Rechner. Mit Grafikkarte deutlich schneller,
  siehe nächster Abschnitt.

## Grafikkarte (GPU)

Im KI-Dialog unter „Grafikkarte“. Zwei unabhängige Teile:

### Bildbearbeitung

Vorschau, Thumbnails und Export laufen auf der Grafikkarte, sobald eine gefunden ist (Schalter
„Bildbearbeitung auf der Grafikkarte“, standardmäßig an). Nichts zu installieren: wgpu spricht die
Karte direkt an – **Vulkan** unter Linux (AMD: Mesa/RADV) und Android, **DirectX 12** oder Vulkan
unter Windows, Metal unter macOS.

- Alle Stufen der Pipeline sind als Compute-Shader umgesetzt (`crates/lumenkorn-develop/src/gpu/`):
  Weißabgleich, Belichtung, RAW-Basiskurve, Lichter/Tiefen/Weiß/Schwarz mit den weichgezeichneten
  Helligkeitskarten, Kontrast, Klarheit, Dynamik/Sättigung, alle Maskenebenen, Schärfen,
  Masken-Überlagerung, Drehen, Spiegeln, Geraderichten, Zuschneiden, Einmischen der Rauschreduzierung.
- Nur die *Form* einer Maske (Verlauf, Radial, Pinsel, KI-Maske) wird auf der CPU berechnet – und
  zwischengespeichert, weil sie sich beim Ziehen der Regler nicht ändert. Davon profitiert auch die CPU.
- **Gleiches Ergebnis:** Ein Test vergleicht GPU und CPU Pixel für Pixel über alle Regler, Masken
  und Geometrie-Fälle, für JPEG- und RAW-Quellen: größte Abweichung 0,0001 (weit unter einer 8-Bit-Stufe).
- **Rückfall:** Keine Karte, nur Software-Rendering, zu wenig Grafikspeicher oder ein Treiberfehler –
  dann rechnet die CPU-Pipeline, der Grund steht im Dialog. Die Karte wird beim Start im
  Hintergrund gesucht; bis dahin rechnet die CPU.
- Speicherbedarf auf der Karte ≈ 90 Byte pro Pixel: ~250 MB für die Vorschau, ~2 GB für den
  Export eines 24-MP-Bildes (wird danach wieder freigegeben). Passt ein Bild nicht in einen Puffer
  der Karte, wird es auf der CPU gerechnet.
- Messen: `cargo run --release -p lumenkorn-develop --example bench` (Vorschaugröße) bzw.
  `… --example bench -- 6000 4000` (Exportgröße) vergleicht CPU und GPU auf deinem Rechner.

### KI-Modelle

Ein Klick lädt eine GPU-fähige ONNX Runtime (~25 MB), danach
rechnen die KI-Modelle auf der Grafikkarte. **AMD, NVIDIA und Intel** werden gleichermaßen
unterstützt, über den normalen Grafiktreiber – keine CUDA- oder ROCm-Installation.

| System  | Weg (empfohlen zuerst)                         | Paket                                 |
|---------|------------------------------------------------|---------------------------------------|
| Linux   | WebGPU über **Vulkan** (bei AMD: Mesa/RADV)     | `onnxruntime-webgpu` 1.27.0           |
| Windows | WebGPU über **DirectX 12**                     | `onnxruntime-webgpu` 1.27.0           |
| macOS   | WebGPU über Metal                              | `onnxruntime-webgpu` 1.27.0           |

- Die Pakete sind die offiziellen ONNX-Runtime-Builds von PyPI. Lumenkorn entnimmt ihnen nur die
  C-Bibliothek (plus den DirectX-Shader-Compiler unter Windows) und prüft vorher die
  **SHA-256-Prüfsumme**. Abgelegt unter `<daten>/ki/runtime-webgpu/`.
- **DirectML zurückgezogen:** Die letzte DirectML-Laufzeit (1.24) stürzte auf einer Radeon RX 7900 XTX
  unter Windows beim Laden der Modelle mit einer Zugriffsverletzung in `onnxruntime.dll` ab. Eine
  vorhandene `runtime-directml` wird beim Start entfernt, die Einstellung auf „Automatisch“ gesetzt.
- **Ganz oder gar nicht je Modell:** Ein Modell läuft nur auf der GPU, wenn *alle* seine
  Operationen dort laufen können; sonst komplett auf der CPU (ständiges Kopieren zwischen GPU und
  CPU wäre oft langsamer als die CPU allein). Der Dialog zeigt pro Modell, wo es rechnet.
  Unter WebGPU laufen Rauschreduzierung (DRUNet) und „Motiv“ (IS-Net) vollständig auf der GPU;
  die quantisierten Modelle (Segment Anything, CLIP-Text) enthalten int8-Operationen ohne
  WebGPU-Umsetzung und bleiben auf der CPU.
- **Rechenwerk** „Automatisch“ / „Nur Prozessor“. „Nur Prozessor“ lädt die reine CPU-Laufzeit
  (falls installiert), damit kein Grafiktreiber-Code im Prozess ist; ein Wechsel der Laufzeit
  braucht einen Neustart, den der Dialog anbietet.
- **Fällt die GPU aus** (Treiberproblem, zu wenig Grafikspeicher), rechnet Lumenkorn auf der CPU
  weiter und zeigt den Fehler im Dialog an.
- **Absturzschutz:** Native Bibliotheken (ONNX Runtime, Grafiktreiber) können Lumenkorn ohne
  Fehlermeldung beenden. Vor jedem riskanten Schritt (Laufzeit laden, Modell öffnen, Analyse,
  Grafikkarte öffnen) legt Lumenkorn eine Markierung im KI-Ordner an und entfernt sie danach.
  Findet es beim Start noch eine, wird der Auslöser abgeschaltet – GPU-Laufzeit beiseitegelegt
  (`…deaktiviert`) und KI auf CPU, bzw. Verschlagwortung pausiert, bzw. GPU-Bildbearbeitung aus –
  und der KI-Dialog erklärt, was passiert ist. Lumenkorn startet so in jedem Fall wieder.
- **Protokoll:** `<daten>/lumenkorn.log` (Windows: `%APPDATA%\io.github.drayprescot.lumenkorn\lumenkorn.log`),
  inklusive Rust-Paniken mit Rückverfolgung.
- **„Geschwindigkeit testen“** misst die Rauschreduzierung an einem 512×512-Testbild und rechnet
  auf ein 24-MP-Foto hoch.
- Die Rauschreduzierung rechnet auf der GPU in Kacheln zu 512 Pixeln (plus Rand): Zwischenergebnisse
  bleiben so unter der 128-MB-Puffergrenze, die WebGPU auf jeder Karte garantiert.
- Erkannt wird die Grafikkarte unter Linux über `/sys/class/drm` (mit Grafikspeicher bei AMD);
  unter Windows wählt der Treiber selbst die stärkste Karte, der Dialog nennt dort keinen Namen.
- Windows braucht wie bei der CPU-Laufzeit das *Microsoft Visual C++ Redistributable (x64)*,
  das auf den meisten Rechnern vorhanden ist; fehlt es, sagt die Fehlermeldung das.

**Ehrlicher Teststand (beide Teile):** In meiner Bauumgebung gibt es keine Grafikkarte. Getestet habe ich mit
dem Software-Vulkan-Treiber von Mesa (lavapipe), also über genau denselben WebGPU/Vulkan-Weg, den
eine AMD-Karte unter Linux nimmt – nur ohne Tempo: Download und Prüfsumme, Laden, Rauschreduzierung
und „Motiv“ auf der „GPU“ liefern dieselben Ergebnisse wie die CPU (größte Abweichung 2·10⁻⁶ bzw.
0/255), Umschalten ohne Neustart, Neustart-Weg, Geschwindigkeitstest. **Nicht getestet:** echte
Geschwindigkeit auf einer echten Karte, DirectML und WebGPU unter Windows (nur Exporte und
Abhängigkeiten der DLLs geprüft; DirectML stürzte im echten Einsatz ab und ist zurückgezogen),
macOS. Den Absturzschutz habe ich mit einem simulierten Absturz an derselben Stelle getestet. Für die Bildbearbeitung: Vergleichstest GPU/CPU und
Bedienung in der App (Regler, Masken, Schalter) über lavapipe; schon dieser Software-Vulkan auf zwei
CPU-Kernen ist bei Masken und Klarheit etwa doppelt so schnell wie die Rayon-Pipeline (401 statt
771 ms für 2,8 MP mit drei Maskenebenen) – eine echte Karte sollte im Bereich weniger Millisekunden
liegen, gemessen habe ich das nicht.

## Lokale KI

Über „KI einrichten“ unten links. Einmaliger Download von ca. 160 MB:

- **ONNX Runtime 1.28** von den offiziellen GitHub-Releases (Windows x64/ARM64, Linux x64/ARM64, macOS ARM64).
- **CLIP ViT-B/32** (quantisiert) von Hugging Face ([Xenova/clip-vit-base-patch32](https://huggingface.co/Xenova/clip-vit-base-patch32)).

Danach analysiert Lumenkorn alle Fotos im Hintergrund (aus den Thumbnails, also schnell):

- **Automatische Schlagworte** auf Deutsch in fünf Gruppen – Ort, Motiv, Licht, Wetter, Stil. Innerhalb einer Gruppe konkurrieren die Begriffe mit einer neutralen Option, damit nicht jedes Foto irgendein „Wetter“ bekommt. KI-Schlagworte sind gestrichelt umrandet, tragen ihre Sicherheit und lassen sich entfernen; eigene Schlagworte werden nie überschrieben. Das Vokabular steht in `crates/lumenkorn-ai/src/vocab.rs` und ist leicht erweiterbar.
- **Inhaltssuche:** „KI“-Schalter im Suchfeld, dann z. B. „Hund am Strand“ oder „Nebel im Wald“. Deutsche Wörter aus dem Vokabular werden für CLIP ins Englische übertragen; englische Beschreibungen funktionieren am besten.
- Downloads nutzen den Zertifikatsspeicher des Betriebssystems und funktionieren damit auch hinter Firmen-Proxys mit eigenem Stammzertifikat.
- Alle Daten bleiben auf dem Gerät. `ki/quellen.json` im Datenordner kann die Download-Adressen überschreiben (Spiegelserver, anderes kompatibles CLIP-Modell).

**Ehrlicher Teststand:** Laufzeit-Laden, Tokenizer (identisch mit OpenAIs Referenz, auch bei Umlauten),
Inferenz, Ranking, Verschlagwortung, Speicherung, Suche und Download-Logik sind mit echter
ONNX Runtime 1.28 und einem Testmodell gleicher Schnittstelle getestet (`tools/make_ai_test_model.py`).
Das echte CLIP-Modell konnte ich in meiner Bauumgebung nicht laden (Hugging Face war gesperrt).
Die Qualität der Schlagworte und die Schwellwerte (`min_p` je Gruppe) solltest du daher mit deinen
eigenen Fotos prüfen und ggf. nachjustieren.

## KI-Reparatur

Im Entwickeln-Modus im Reiter **Reparatur**. Entfernt Störendes – Staubflecken, Leitungen, Passanten,
Schilder – und füllt die Stelle passend aus dem restlichen Bild.

- **Auswählen wie bei Masken:** „Neu“ → Pinsel oder Objekt (Klick, Segment Anything). Mit „Hinzufügen“
  und „Abziehen“ werden mehrere Auswahlen zu **einer** Reparatur zusammengefasst; die Auswahl ist rot
  zu sehen. Gerechnet wird erst nach Klick auf **„Reparieren“**.
- **Modell:** LaMa (Apache-2.0), ONNX-Fassung aus dem OpenCV Model Zoo, ~93 MB, Download beim ersten Mal.
- **Volle Auflösung:** LaMa rechnet mit 512×512. Jeder zusammenhängende Bereich wird deshalb einzeln
  mit Umgebung ausgeschnitten (mindestens 512 px, sonst das 2,2-Fache des Bereichs), gefüllt und nur
  innerhalb der leicht erweiterten, weich auslaufenden Auswahl eingesetzt. Kleine Reparaturen bleiben
  so pixelscharf; sehr große Bereiche werden für das Modell verkleinert und wirken entsprechend weicher.
- **Non-destruktiv:** Das Ergebnis liegt als Flicken in `<daten>/reparaturen/` (lineares RGB, f16) und
  wird in der Pipeline direkt nach der Rauschreduzierung eingesetzt – in Vorschau, Zoom, Thumbnails und
  Export, auf CPU wie GPU. Reparaturen lassen sich ein-/ausblenden, löschen, rückgängig machen.
  Ändert man die Auswahl danach, zeigt der Reiter „geändert“ und bietet „Neu berechnen“ an.
- Getestet mit dem echten Modell: aufgemalter Fleck und Strich in einem Foto → mittlere Abweichung zum
  unversehrten Original im Schadensbereich von 0,59 auf 0,024; in der App Mond aus einem Bergfoto
  entfernt (zwei Bereiche, 1800×1800, 13 s auf zwei CPU-Kernen).

## Personen

Einrichten über „KI“ → „Personenerkennung herunterladen“ (~39 MB). Danach sucht Lumenkorn im
Hintergrund in allen Fotos nach Gesichtern (nach der Verschlagwortung).

- **Modelle:** YuNet zum Finden (MIT), SFace zum Wiedererkennen (Apache-2.0), beide aus dem OpenCV
  Model Zoo. Die verbreiteten InsightFace-Modelle sind bewusst nicht dabei – sie dürfen nur
  nichtkommerziell genutzt werden.
- **Finden:** Das Foto (bis 1600 px) wird einmal ganz und einmal in doppelter Auflösung in Kacheln
  abgesucht, so werden auch kleine Gesichter in Gruppenfotos gefunden. Gesichter unter 20 px fallen weg.
- **Gruppieren:** Jedes Gesicht wird ausgerichtet (fünf Merkmalspunkte) und in einen 128-dimensionalen
  Vektor übersetzt. Ähnliche Gesichter (Kosinus ≥ 0,5) landen in derselben Gruppe; eine Person
  erscheint höchstens einmal pro Foto.
- **Benennen:** In „Personen verwalten“ einer Gruppe einen Namen geben. Ähnliche Gruppen und neue
  Fotos werden der Person danach **vorgeschlagen** (gestrichelt bzw. „Lena?“) – ✓ bestätigt, ✕ nimmt
  heraus und schlägt dieses Gesicht der Person nie wieder vor. Ein gleicher Name führt zwei Personen
  zusammen. Einzelne Gesichter lassen sich im Infobereich direkt benennen oder ignorieren.
- **Umbenennen:** Doppel- oder Rechtsklick auf eine Person in der Seitenleiste bzw. Klick auf den
  Namen in „Personen verwalten“, z. B. „Max“ → „Max Mustermann“. Bestätigte Gesichter bleiben
  bestätigt, offene Vorschläge bleiben zu prüfen. Gibt es den neuen Namen schon, werden beide
  Personen nach Rückfrage zusammengeführt.
- **Mehrere Fotos auf einmal korrigieren:** Fotos auswählen (Strg-/Umschalt-Klick, Strg+A) – der
  Infobereich zeigt unter „Personen in der Auswahl“ alle darin erkannten Personen. Je Person lassen
  sich die Gesichter aller ausgewählten Fotos einer anderen Person zuordnen (Name eingeben) oder
  mit ✕ herausnehmen. In der Ansicht einer Person verschwinden falsch zugeordnete Fotos sofort.
- **Filtern und Suchen:** Personen erscheinen in der Seitenleiste; ein Klick zeigt alle ihre Fotos.
  Die normale Suche findet auch Namen.
- Gesichter beziehen sich auf das unbearbeitete Foto; Zuschnitt oder Bearbeitung ändern nichts an
  der Zuordnung. Alles bleibt lokal im Katalog.

**Ehrlicher Teststand:** Mit den echten Modellen und den OpenCV-Beispielfotos geprüft: Gesichtskasten
von „Lena“ gleich OpenCVs `FaceDetectorYN` (±1 px); dieselbe Person nach Verkleinern und Aufhellen
0,78 ähnlich, verschiedene Personen 0,11; im Gruppenfoto 49 Gesichter (OpenCV: 58 bei voller
Auflösung – Lumenkorn sucht auf 1600 px), unter 435 Paaren fremder Gesichter nur 2 über der
Gruppierschwelle. In der App: Download, Analyse, Gruppen, Benennen, Vorschlag für ein neues Foto,
Bestätigen, Filtern, Namenssuche. Wie gut es bei deinen Fotos über Jahre (Altern, Brillen, Profile)
trennt, habe ich nicht prüfen können – die Schwelle steht in `crates/lumenkorn-core/src/people.rs`.

## Vergleichen und Zoomen

- **Vergleich:** Mehrere Fotos mit Strg- bzw. Umschalt-Klick auswählen, dann „Vergleichen“,
  Taste C, Enter oder Rechtsklick → „Groß nebeneinander vergleichen“ (auch im Entwickeln, mit
  Auswahl im Filmstreifen). Bis zu 12 Fotos erscheinen groß nebeneinander, mit **gekoppeltem Zoom** (abschaltbar):
  Zoomen und Verschieben wirkt auf alle gleichzeitig, an derselben Bildstelle – ideal für
  Schärfevergleiche in Serien. Ein Klick hebt ein Foto hervor; Bewertung und Markierung wirken im
  Vergleich nur auf dieses. × oder Entf nimmt ein Foto heraus.
- **Zoom** in Einzelansicht, Vergleich und Entwickeln: Scrollrad (zum Mauszeiger hin), Strg + / −,
  Strg 0, Z oder Doppelklick (Einpassen ↔ 100 %), Ziehen zum Verschieben. 100 % heißt ein Bildpixel
  pro Bildschirmpixel, auch auf HiDPI-Bildschirmen.
- **Echte volle Auflösung:** Die Vorschauen haben 2048 px. Reicht das beim Hineinzoomen nicht, lädt
  die Bibliothek das ganze Foto in voller Auflösung nach (bearbeitet bzw. aus dem RAW entwickelt; die
  letzten sechs bleiben im Speicher). Beim Entwickeln wird nur der **sichtbare Ausschnitt** aus der
  vollen Auflösung gerechnet – mit den aktuellen, noch nicht gespeicherten Reglern – und ersetzt
  ~0,2 s nach der letzten Bewegung die hochskalierte Vorschau. Auf der GPU kostet der Ausschnitt fast
  nichts zusätzlich; auf der CPU wird das ganze Bild in Zoomgröße gerechnet.
- Masken und Zuschnitt bearbeitet man in der Einpassen-Ansicht; beim Zuschneiden springt der Zoom
  automatisch zurück.

## Aussortieren (Culling)

- **X** lehnt das sichtbare Foto ab (erneut drücken hebt auf), **Umschalt+X** lehnt ab und springt
  sofort zum nächsten Foto – so geht man eine Serie in der Einzelansicht schnell durch. In der
  Einzelansicht, im Vergleich und im Entwickeln wirken Bewertung und Markierung immer nur auf das
  groß angezeigte Foto, auch wenn im Raster mehrere ausgewählt sind.
- Sobald in der aktuellen Ansicht (Ordner, Projekt, Suche …) Fotos abgelehnt sind, zeigt die
  Werkzeugleiste **„N abgelehnt“**. Ein Klick (oder Strg+Entf) öffnet die Rückfrage mit Vorschau,
  Anzahl und Gesamtgröße.
- Gelöscht wird standardmäßig **in den Papierkorb** des Systems (Windows, macOS, Linux); wahlweise
  endgültig. Auf Android gibt es keinen Papierkorb, dort wird endgültig gelöscht.
- Das Backend löscht nur Fotos, die beim Löschen noch abgelehnt sind. Mit der Datei verschwinden
  XMP-Begleitdateien (`foto.cr3.xmp`; `foto.xmp` nur, wenn kein JPEG/RAW gleichen Namens daneben
  liegt), Katalogeintrag, Bearbeitungen, Masken, Entrauschungen, Reparaturen, Vorschauen und Gesichter.
- Dateien, die sich nicht löschen lassen (z. B. schreibgeschützt oder in einem anderen Programm
  geöffnet), bleiben im Katalog und werden gemeldet.

## Basis-Einstellungen übertragen

Ohne Knöpfe – per Tastenkürzel oder Rechtsklick auf ein Foto (Raster, Filmstreifen,
Einzelansicht, Entwickeln):

- **Strg+Umschalt+C** kopiert die Basis-Einstellungen des gewählten Fotos.
- **Strg+Umschalt+V** fügt sie ein – im Raster und im Filmstreifen auf alle ausgewählten Fotos,
  in Einzelansicht und Vergleich auf das sichtbare.
- Übertragen wird **nur der Reiter „Basis“**: Weißabgleich, Licht, Farbe, HSL, Klarheit, Schärfen.
  Masken, Reparaturen, Rauschreduzierung, Zuschnitt und Drehen des Zielfotos bleiben unberührt.
- Im Entwickeln lässt sich das Einfügen beim geöffneten Foto mit Strg+Z rückgängig machen.

## Bilder zusammenfügen

Mehrere Fotos auswählen (Strg- oder Umschalt-Klick), dann Rechtsklick → **Zusammenfügen** oder
per Tastenkürzel. Ein kleiner Dialog zeigt die Optionen, die Berechnung läuft im Hintergrund
(„Im Hintergrund“ schließt den Dialog, Abbrechen ist jederzeit möglich).

| | Kürzel | Was passiert |
|---|---|---|
| **HDR aus Belichtungsreihe** | Strg+H | Ausrichten (Freihand), Belichtungsverhältnisse aus den Pixeln gemessen (keine EXIF-Angaben nötig), gewichtetes Zusammenführen zur linearen Strahldichte, optional Geisterbilder (bewegte Motive) unterdrücken |
| **Panorama** | Strg+M | Überlappungen aller Paare suchen (Reihenfolge egal, auch mehrzeilig), Brennweite und Kameradrehungen schätzen, Horizont gerade richten, Projektion zylindrisch / Kugel / perspektivisch, Helligkeit angleichen, weich überblenden, optional auf das größte gefüllte Rechteck zuschneiden |
| **Focus Stacking** | Strg+Umschalt+F | Ausrichten inkl. Objektiv-Atmen (Merkmale, dann pixelgenaue Nachführung), Laplace-Pyramiden: je Pixel und Größenordnung gewinnt das schärfste Bild |

Das Ergebnis ist eine **EXR-Datei** (lineares Rec.709, 16-Bit-Gleitkomma, ZIP-komprimiert) neben dem
ersten Foto, z. B. `IMG_0412-HDR.exr`, `IMG_0412-Pano.exr`, `IMG_0412-Fokus.exr`. Sie wird sofort in
den Katalog aufgenommen und ausgewählt, Aufnahmedatum, Kamera und Objektiv stammen vom Referenzfoto.
Werte über 1,0 bleiben erhalten: Aus RAWs entstandene Ergebnisse werden wie ein RAW entwickelt
(Basiskurve, Lichter zurückholbar), aus JPEGs entstandene wie ein JPEG. Die Bearbeitungen der
Quellfotos fließen nicht ein – zusammengefügt werden die unbearbeiteten Originale.

## Entwickeln: Reiter

- **Basis:** Weißabgleich, Licht, Farbe, Details (Klarheit, Schärfen, KI-Rauschreduzierung), Schnappschüsse.
- **Masken:** Maskenebenen und ihre Regler. Wählt man auf dem Bild eine Maske oder ein Werkzeug, öffnet sich der Reiter von selbst.
- **Reparatur:** Störendes mit KI entfernen (siehe oben).
- **Zuschnitt:** Zuschneiden, Seitenverhältnis, Geraderichten (mit Automatik), Drehen, Spiegeln. Der Reiter startet das Zuschneiden; die Taste R öffnet ihn und kehrt nach Enter/Esc zum vorherigen Reiter zurück.

Histogramm, Kopieren/Einfügen/Zurücksetzen und Export bleiben in allen Reitern sichtbar.

## Tastenkürzel

| Taste | Aktion |
|---|---|
| Pfeiltasten | Navigieren |
| Enter / Leertaste / E | Groß anzeigen (mehrere ausgewählt: nebeneinander im Vergleich) |
| D | Entwickeln |
| Esc / G | Zurück zum Raster |
| 0–5 | Bewertung |
| P / X / U | Markieren / Ablehnen / Aufheben |
| Umschalt+X | Ablehnen und zum nächsten Foto (Einzelansicht, Vergleich, Entwickeln, Raster) |
| Strg+Entf / Strg+Rücktaste | Abgelehnte Fotos der Ansicht von der Festplatte löschen (mit Rückfrage) |
| Strg+A | Alles auswählen |
| Klick mit Strg / Umschalt | Mehrfachauswahl (Raster und Filmstreifen) |
| C | Ausgewählte Fotos vergleichen (2–12, groß nebeneinander) – auch im Entwickeln und per Rechtsklick |
| Scrollrad | Zoomen zum Mauszeiger hin (Einzelansicht, Vergleich, Entwickeln) |
| Strg + / Strg − / Strg 0 | Vergrößern / Verkleinern / Einpassen |
| Z oder Doppelklick | Einpassen ↔ 100 % |
| Ziehen | Ausschnitt verschieben (gezoomt) |
| Entf | Foto aus dem Vergleich nehmen |
| I | Infobereich ein/aus |
| \ | Vorher/Nachher (Entwickeln) |
| [ / ] | Links/rechts drehen (Entwickeln) |
| Strg+Z / Strg+Umschalt+Z | Rückgängig / Wiederholen (Entwickeln) |
| Strg+Umschalt+C / Strg+Umschalt+V | Basis-Einstellungen kopieren / einfügen (auch per Rechtsklick) |
| O | Maske rot anzeigen (Entwickeln, Maske ausgewählt) |
| R | Zuschneiden ein/aus (Entwickeln); Enter übernimmt, Esc bricht ab |
| Esc | Werkzeug bzw. Maske abwählen (Entwickeln) |
| Strg+Umschalt+E | Auswahl exportieren |
| Strg+H / Strg+M / Strg+Umschalt+F | Auswahl zu HDR / Panorama / Focus Stacking zusammenfügen (auch per Rechtsklick) |
| Doppelklick auf Regler | Regler zurücksetzen |

## Arbeitsspeicher und Datenordner

Rust hat keinen Garbage Collector – Speicher wird frei, sobald niemand ihn mehr hält. Lange
gehalten werden nur bewusste Zwischenspeicher; Lumenkorn lässt sie zu festen Zeitpunkten los:

| Was | Wann freigegeben |
|---|---|
| Bild in voller Auflösung (Zoom), entrauschte Fassungen, Masken, Reparatur-Quelle, GPU-Puffer | beim Verlassen des Entwickelns; nach einem Export nur die großen |
| KI-Modelle für Masken, Rauschreduzierung, Reparatur | nach 3 Minuten ohne Nutzung |
| KI-Modelle für Verschlagwortung und Personen | nach 5 Minuten ohne Nutzung |
| Bearbeitungs-Proxys (4 Fotos × ~35 MB) | bleiben – Zurückkehren ins Entwickeln ist sofort da |

Ein Modell, das gerade rechnet, wird nie entladen; beim nächsten Gebrauch lädt es in wenigen
Sekunden neu. Thumbnails bearbeiteter Fotos werden höchstens zu zweit gleichzeitig entwickelt
(ein RAW braucht dabei bis ~1,5 GB), die Anzeige in voller Auflösung höchstens zweimal parallel.

**Datenordner:** 90 Sekunden nach dem Start und danach täglich räumt Lumenkorn verwaiste
Dateien auf: alte Entrauschungen nach erneutem Entrauschen (bis ~360 MB je 60-MP-Foto),
gelöschte KI-Masken und Reparaturen, Gesichter- und Vorschaubilder entfernter Fotos.
Was eine Bearbeitung oder ein Schnappschuss noch braucht, bleibt; Dateien jünger als 24 Stunden
ebenfalls. Das Ergebnis steht im Protokoll (`lumenkorn.log`).

## Aufbau

```
lumenkorn/
├── crates/lumenkorn-core/     Katalog, Scanner, Projekte, Thumbnails, Dateiüberwachung
├── crates/lumenkorn-develop/  RAW-Dekodierung und Entwicklungspipeline (ohne GUI, ohne DB)
│   ├── params.rs              Parametersatz einer Bearbeitung (versioniert)
│   ├── source.rs              RAW/Raster → lineares Rec.709-Float-Bild
│   ├── pipeline.rs            alle Bearbeitungsschritte
│   ├── local.rs               Helligkeitskarten für lokale Anpassungen
│   ├── masks.rs               Masken auswerten (Verlauf, Radial, Pinsel, Raster), zwischengespeichert
│   ├── repair.rs              Flicken der KI-Reparatur: Dateiformat, Einsetzen
│   ├── gpu/                   dieselbe Pipeline als wgpu-Compute-Shader (pipeline.wgsl)
│   ├── merge/                 Zusammenfügen: align (Ecken, BRIEF, RANSAC-Homografie, Nachführung),
│   │                          hdr, pano (Drehungen, Projektionen, Helligkeitsausgleich), focus (Laplace-Pyramiden)
│   ├── exr_io.rs              EXR lesen/schreiben (Ergebnisse des Zusammenfügens)
│   └── output.rs              JPEG / 16-Bit-TIFF / PNG
├── crates/lumenkorn-ai/       CLIP über ONNX Runtime (dynamisch geladen)
│   ├── clip.rs                Bild-/Text-Embeddings, Ein-/Ausgänge per Name
│   ├── tokenizer.rs           CLIP-BPE, ohne C-Abhängigkeiten
│   ├── vocab.rs, tagger.rs    Zero-Shot-Schlagworte
│   ├── index.rs               semantische Suche
│   ├── segment.rs             Motiv (IS-Net), Objekt (SAM), Himmel
│   ├── inpaint.rs             KI-Reparatur (LaMa): Bereiche finden, ausschneiden, füllen
│   ├── faces.rs               Gesichter finden (YuNet), ausrichten, Merkmalsvektor (SFace)
│   ├── denoise.rs             DRUNet: Modell bauen, Rauschschätzung, Kachel-Inferenz
│   ├── torch_legacy.rs        PyTorch-Gewichte (Altformat) ohne PyTorch lesen
│   ├── onnx_writer.rs         ONNX-Modell ohne Protobuf-Bibliothek schreiben
│   ├── accel.rs               GPU: Laufzeitwahl, Sitzungen mit Rückfall, Grafikkarten-Erkennung
│   └── models.rs              Downloads von Laufzeit und Modell
├── src-tauri/                 Tauri-2-Shell
│   ├── commands.rs            Bibliotheksbefehle
│   ├── develop.rs             Proxy-Cache, Live-Vorschau (binär), Export
│   ├── ai.rs                  Analyse im Hintergrund, Suche, Downloads
│   ├── worker.rs              Hintergrundaufträge (Scan → Thumbnails → KI)
│   ├── merge.rs               HDR/Panorama/Focus Stacking im Hintergrund, Ergebnis in den Katalog
│   └── protocol.rs            lw://-Schema für Bilder
├── src/                       Svelte-5-Oberfläche
└── tools/                     Testdaten-Erzeugung
```

Die drei Crates kennen weder Tauri noch einander mehr als nötig: `core` nutzt `develop` für
bearbeitete Thumbnails, `ai` nutzt `develop` nur für Bild- und Flickentypen der Reparatur. Alles Wesentliche lässt sich ohne GUI testen.

**Daten.** Katalog und KI im App-Datenordner (Linux `~/.local/share/io.github.drayprescot.lumenkorn/`,
Windows `%APPDATA%\io.github.drayprescot.lumenkorn\`), Thumbnails im Cache-Ordner.
Daten der früheren Fassung „Lichtwerk“ (`de.lichtwerk.app`) werden beim ersten Start übernommen.

## Entwickeln am Code

Voraussetzungen: Rust (stabil, ≥ 1.90), Node.js ≥ 20, plus die Tauri-Systemabhängigkeiten:

- **Windows:** Microsoft C++ Build Tools und WebView2 (ab Windows 10 meist vorhanden).
- **Linux (Debian/Ubuntu):** `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev librsvg2-dev libayatana-appindicator3-dev build-essential`
- Details: <https://v2.tauri.app/start/prerequisites/>

```bash
npm install
npm run tauri dev        # App mit Hot-Reload starten
npm run tauri build      # Installer bauen (.msi/.exe bzw. .deb/.AppImage/.rpm)
npm run dev              # Nur Oberfläche im Browser, mit Beispieldaten

cargo test --workspace                                               # alle Tests
cargo run -p lumenkorn-core --example scan -- ~/Bilder               # Scan ohne GUI
cargo run -p lumenkorn-develop --example develop -- in.NEF out.jpg '{"exposure":0.5}'
# HDR, Panorama und Focus Stacking an aus einem echten Foto erzeugten Reihen prüfen
cargo run --release -p lumenkorn-develop --example merge -- in.CR2 /tmp/merge-test

# Tests mit echten Dateien (optional)
LUMENKORN_TEST_RAW=/ordner/mit/raws cargo test -p lumenkorn-develop
LUMENKORN_TEST_ORT=/pfad/libonnxruntime.so LUMENKORN_TEST_MODEL=/pfad/modell cargo test -p lumenkorn-ai
LUMENKORN_TEST_ORT=/pfad/libonnxruntime.so LUMENKORN_TEST_SEG=/pfad/masken cargo test -p lumenkorn-ai --test segment
LUMENKORN_TEST_ORT=/pfad/libonnxruntime.so LUMENKORN_TEST_DENOISE=/pfad cargo test -p lumenkorn-ai --test denoise
LUMENKORN_TEST_ORT=/pfad/libonnxruntime.so LUMENKORN_TEST_FACES=/pfad cargo test -p lumenkorn-ai --test faces
LUMENKORN_TEST_ORT=/pfad/libonnxruntime.so LUMENKORN_TEST_LAMA=/pfad/lama.onnx LUMENKORN_TEST_IMAGE=/pfad/foto.jpg cargo test --release -p lumenkorn-ai --test inpaint
# Bildbearbeitung GPU gegen CPU (ohne Grafikkarte: Mesa lavapipe + LUMENKORN_GPU_SOFTWARE=1):
LUMENKORN_GPU_SOFTWARE=1 cargo test -p lumenkorn-develop --test gpu
# KI GPU gegen CPU (WebGPU-Laufzeit; ohne Grafikkarte genügt Mesa lavapipe):
LUMENKORN_TEST_ORT_GPU=/pfad/runtime-webgpu/libonnxruntime.so LUMENKORN_TEST_DENOISE=/pfad LUMENKORN_TEST_SEG=/pfad/masken cargo test --release -p lumenkorn-ai --test gpu
```

## Android

Der Rust-Code ist für Android vorbereitet (`mobile_entry_point`, keine C-Abhängigkeiten außer
SQLite), die Oberfläche passt sich schmalen Bildschirmen an.

```bash
# Android Studio mit SDK + NDK installieren, ANDROID_HOME und NDK_HOME setzen
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
npm run tauri android init
npm run tauri android dev
```

- **Dateizugriff:** Android erlaubt seit Version 11 keinen freien Zugriff auf Ordnerpfade. Dafür braucht die App *Zugriff auf alle Dateien* (`MANAGE_EXTERNAL_STORAGE`). Nach `tauri android init` in `src-tauri/gen/android/app/src/main/AndroidManifest.xml` ergänzen: `<uses-permission android:name="android.permission.MANAGE_EXTERNAL_STORAGE" />`. Für den Play Store nur mit Begründung zulässig. Der Ordnerdialog liefert dort keine echten Pfade; die App fragt deshalb nach dem Pfad (z. B. `/storage/emulated/0/DCIM`). Die saubere Langfristlösung ist das Storage Access Framework über ein Kotlin-Plugin.
- **KI:** Für Android gibt es keinen automatischen Laufzeit-Download. `libonnxruntime.so` aus dem offiziellen AAR (`com.microsoft.onnxruntime:onnxruntime-android`) in `<daten>/ki/runtime/` legen oder mitpaketieren; das Modell lädt die App wie auf dem Desktop.
- Den Android-Build konnte ich nicht testen (kein NDK in der Bauumgebung). Linux ist gebaut und live bedient.

## Fahrplan

1. GPU-Feinschliff nach Messungen auf echter Hardware: Vorschau direkt als Textur anzeigen statt als JPEG zu übertragen, Maskenformen ebenfalls auf der GPU, größere Kacheln bei der Rauschreduzierung.
2. Masken: Helligkeitsbereich als Form, Masken auf andere Bilder übertragen (KI-Masken dabei neu berechnen).
3. Mehrsprachiges Textmodell für die Inhaltssuche, hierarchische Schlagworte, XMP-Sidecars (auch für Personennamen).

## Lizenzen

Lumenkorn: MIT oder Apache-2.0 (siehe `LICENSE-MIT`, `LICENSE-APACHE`). Abhängigkeiten u. a.: wgpu (MIT/Apache-2.0), rawler (LGPL-2.1 – bei Weitergabe
statisch gelinkter Binärdateien die LGPL-Pflichten beachten), ONNX Runtime (MIT),
CLIP-Gewichte (MIT), IS-Net/DIS (Apache-2.0), Segment Anything (Apache-2.0), DRUNet/KAIR (MIT), YuNet (MIT), SFace (Apache-2.0), LaMa (Apache-2.0), Tauri (MIT/Apache-2.0).

## Bekannte Grenzen

- Windows: Pfadvergleiche sind case-sensitiv (`D:\Fotos` und `d:\fotos` wären zwei Ordner).
- Export eines 24-MP-RAWs braucht kurzzeitig rund 1 GB Arbeitsspeicher (volle Auflösung in Float).
- Die Vorschau wird nach dem Rechnen auf der GPU als JPEG an die Oberfläche übergeben – bei sehr schnellen Karten ist das der größere Teil der Zeit.
- Die Temperatur ist relativ zum Kamera-Weißabgleich, nicht in Kelvin.
- Jede aktive Maskenebene kostet einen weiteren Durchlauf der Tonwert-Pipeline; ohne Grafikkarte wird die Vorschau bei vielen Ebenen spürbar langsamer.
- Ändert sich eine Originaldatei nach dem Entrauschen, wird die Zwischenstufe nicht automatisch neu berechnet.
- Panoramen über volle 360° werden noch nicht unterstützt; Aufnahmen brauchen ~20 % Überlappung. Parallaxe (Kamera nicht um den Objektiv-Drehpunkt geschwenkt) zeigt sich als Doppelkonturen im Nahbereich.
- Zusammenfügen arbeitet in voller Auflösung auf der CPU: Eine HDR-Reihe aus drei 24-MP-RAWs braucht ~1–2 GB Arbeitsspeicher, große Panoramen entsprechend mehr (begrenzt auf 150 MP).
- X-Trans-RAWs (Fujifilm) werden bilinear demosaikt – brauchbar, aber weicher als bei Spezialprogrammen.
