# Lumenkorn

*[English version](../README.md)*

Lumenkorn ist eine Fotoverwaltung mit RAW-Entwicklung, die komplett auf dem eigenen Rechner läuft.
Gedacht ist es als Ersatz für Lightroom Classic: Fotos und Bearbeitungen bleiben auf der eigenen
Platte, es gibt kein Konto, keine Cloud und kein Abo. Die KI-Funktionen nutzen offene Modelle,
die einmal heruntergeladen werden und danach offline auf Prozessor oder Grafikkarte rechnen. Nur
die Kartenansicht braucht das Internet (siehe [Karte](#karte)).

Lumenkorn ist in Rust geschrieben, die Oberfläche in Svelte, zusammengehalten von Tauri 2. Es läuft
unter Windows und Linux; für Android ist der Code vorbereitet. Bildbearbeitung und KI nutzen die
Grafikkarte (AMD, NVIDIA, Intel) über DirectX 12 bzw. Vulkan.

**Mit KI entwickelt:** Ein großer Teil des Codes von Lumenkorn ist mit einem KI-Assistenten
entstanden. Was das Programm können soll und ob eine Änderung gut genug ist, entscheidet ein
Mensch, und jede Version wird vor dem Erscheinen an echten Fotos ausprobiert. Fehler können
trotzdem drinstecken. Sichere deshalb deine Fotos und den Katalog, bevor du Lumenkorn auf deine
ganze Sammlung loslässt.

**Installer:** Den aktuellen Stand von `main` gibt es immer als Windows-MSI (deutsch und englisch)
unter [Releases → nightly](https://github.com/DrayPrescot/lumenkorn/releases/tag/nightly). Er wird
nach jedem Push neu gebaut, sobald alle Prüfungen grün sind, und lässt sich über den alten
installieren. Die MSI ist nicht signiert, deshalb fragt Windows SmartScreen beim ersten Start nach
(„Weitere Informationen“ → „Trotzdem ausführen“).

**Updates:** Kurz nach dem Start schaut Lumenkorn einmal auf GitHub nach einer neueren Version. Gibt
es eine, zeigt es, was neu ist, und fragt: **Jetzt installieren** lädt sie, prüft die Signatur,
installiert und startet neu; **Später** fragt beim nächsten Start wieder. Katalog und Bearbeitungen
bleiben erhalten. Updates kommen aus den Versions-Releases, nicht aus *nightly*.

**Inhalt:** [Bibliothek](#bibliothek) · [Entwickeln](#entwickeln-non-destruktiv) ·
[Karte](#karte) · [Masken](#masken-lokale-anpassungen) · [KI-Funktionen](#ki-funktionen) · [KI-Agenten](#ki-agenten-mcp) ·
[Zusammenfügen](#bilder-zusammenfügen) · [Tastenkürzel](#tastenkürzel) ·
[So funktioniert Lumenkorn](#so-funktioniert-lumenkorn) · [Technik](#technik) ·
[Open-Source-Projekte](#genutzte-open-source-projekte) · [Entwickeln am Code](#entwickeln-am-code)

## Sprache

Die Oberfläche gibt es auf Deutsch und Englisch. Lumenkorn folgt der Systemsprache; **DE / EN**
unten links in der Seitenleiste schaltet um. Auch Meldungen aus dem Hintergrund und neu vergebene
KI-Schlagworte folgen der Sprache. Schon vergebene Schlagworte bleiben, „KI → Alle neu analysieren“
vergibt sie in der neuen Sprache neu.

## Bibliothek

- **Ordner statt Import.** Du fügst einen Ordner hinzu, Lumenkorn liest ihn ein. Nichts wird
  kopiert, verschoben oder verändert.
- **Import von der Speicherkarte** (Strg+Umschalt+I oder der Pfeil neben „Ordner“): Fotos im
  Vorschauraster nach Tagen oder einzeln auswählen. Lumenkorn kopiert sie nach
  `Ziel\JJJJ\JJJJ.MM.TT[_Anlass]` und benennt sie `JJJJMMTT_HHMMSS_Nummer` – Aufnahmezeit aus
  dem EXIF, Nummer aus dem Dateinamen der Kamera (`DSC07869.ARW` wird `…_7869.ARW`). RAW und
  JPEG mit gleichem Namen sowie `.xmp` behalten denselben Namen. Schon importierte Fotos werden
  erkannt. Jede Kopie wird Byte für Byte geprüft; erst danach lassen sich die Originale auf
  Wunsch von der Karte löschen.
- **Kamerauhr korrigieren:** Ging die Uhr einer Kamera falsch, etwa auf Reisen noch auf der Zeit
  daheim, verschiebt „Korrigieren …“ neben „Uhrzeit“ im Import-Dialog die Aufnahmezeit je Kamera
  vor dem Kopieren. Ordner, Dateinamen und die Zeitangaben in den Kopien (EXIF, XMP) bekommen
  die richtige Zeit; die Dateien auf der Karte bleiben unverändert. „Schätzen“ ermittelt die
  Abweichung aus Fotos mit Ort, die schon in der Bibliothek sind, etwa denen vom Handy.
- **Live-Abgleich.** Eine Dateiüberwachung meldet neue, geänderte und gelöschte Dateien. Lumenkorn
  sammelt die Meldungen 1,5 Sekunden lang und prüft dann nur die gemeldeten Pfade, nicht den ganzen
  Ordner. Bei 50 000 Fotos auf einem NAS macht das einen großen Unterschied. Vollständig
  abgeglichen wird beim Start und wenn das System Ereignisse verliert.
- **Umbenennen und Verschieben** außerhalb der App erkennt Lumenkorn am Inhalts-Hash der Datei.
  Bewertung, Schlagworte und Bearbeitungen bleiben am Bild.
- **Neue Ordner** erscheinen sofort im Ordnerbaum, auch leere. Übersprungene Dateien (z. B. HEIC vom
  Handy) und nicht lesbare Einträge meldet Lumenkorn nach dem Abgleich.
- **Projekte** bündeln beliebig viele Ordner, auch von verschiedenen Laufwerken. Mehrere Ordner in
  der Seitenleiste mit Strg- oder Umschalt-Klick wählen, dann Rechtsklick → „Zu … hinzufügen“ oder
  „Neues Projekt aus diesen Ordnern“. Pro Ordner lässt sich festlegen, ob Unterordner dazugehören.
- **Formate:** JPEG, PNG, TIFF (8, 16 und 32 Bit), WebP, OpenEXR und RAW (NEF, CR2, CR3, ARW, RAF,
  DNG, ORF, RW2, PEF und weitere, die rawler kennt). Sehr große Bilder wie Panoramen mit mehreren
  hundert Megapixeln werden nacheinander geladen und für die Vorschau schon beim Einlesen verkleinert.
- **Thumbnails:** Unbearbeitete RAWs zeigen das eingebettete Kamera-JPEG, bearbeitete Fotos ihre
  Bearbeitung.
- **Raster** im Blocksatz mit echten Seitenverhältnissen, Einzelansicht mit Filmstreifen. Der
  Filmstreifen reicht über alle Fotos der Ansicht; das Mausrad scrollt ihn seitwärts.
- **Suche** über Dateiname, Ordner, Kamera, Objektiv, Schlagworte, Personen und Ortsnamen, oder nach Bildinhalt
  (KI). Teilwörter genügen („sonnen“ findet „Sonnenuntergang“), Groß- und Kleinschreibung und
  Akzente spielen keine Rolle. Dazu Filter nach Mindestbewertung und Sortierung.
- **Seitenleiste:** „Projekte“, „Personen“ und „Ordner“ lassen sich per Klick auf die Überschrift
  einklappen.

### Vergleichen und Zoomen

- **Vergleich:** Mehrere Fotos auswählen, dann Taste C, Enter oder Rechtsklick → „Groß nebeneinander
  vergleichen“ (auch im Entwickeln). Bis zu 12 Fotos stehen nebeneinander, mit gekoppeltem Zoom: Zoomen
  und Verschieben wirkt auf alle an derselben Bildstelle. Unten liegt der Filmstreifen; ein Klick
  ersetzt das hervorgehobene Foto, Strg-Klick nimmt eines hinzu oder heraus.
- **Zoom:** Scrollrad (zum Mauszeiger hin), Strg + / −, Strg 0, Z oder Doppelklick (Einpassen ↔ 100 %).
  100 % heißt ein Bildpixel pro Bildschirmpixel, auch auf HiDPI-Bildschirmen. Beim Hineinzoomen lädt
  Lumenkorn das Foto in voller Auflösung nach; im Entwickeln wird nur der sichtbare Ausschnitt aus der
  vollen Auflösung neu gerechnet.

### Aussortieren

- **X** lehnt das Foto ab, **Umschalt+X** lehnt ab und springt zum nächsten. In Einzelansicht,
  Vergleich und Entwickeln wirken Bewertung und Markierung nur auf das groß gezeigte Foto.
- Sind in der Ansicht Fotos abgelehnt, zeigt die Werkzeugleiste „N abgelehnt“. Ein Klick (oder
  Strg+Entf) öffnet die Rückfrage mit Vorschau, Anzahl und Gesamtgröße.
- Gelöscht wird in den Papierkorb des Systems, wahlweise endgültig. Mit der Datei verschwinden
  XMP-Begleitdateien, Katalogeintrag, Bearbeitungen, Masken, Entrauschungen, Reparaturen,
  Vorschauen und Gesichter. Dateien, die sich nicht löschen lassen, bleiben im Katalog und werden
  gemeldet.

## Karte

Der Reiter **Karte** (Taste M, neben Bibliothek und Entwickeln) zeigt die Fotos der aktuellen
Ansicht (Ordner, Projekt, Person, Suche, Filter) als gruppierte Markierungen auf einer
OpenStreetMap-Weltkarte. Ein Klick auf eine Markierung oder Gruppe wählt diese Fotos im
Filmstreifen darunter aus (Strg fügt zur Auswahl hinzu); Gruppen zoomen hinein.

- **GPS-Daten** von Kamera oder Handy (EXIF) liest Lumenkorn beim Einlesen. Für Fotos, die schon im
  Katalog sind, werden sie einmalig im Hintergrund nachgetragen.
- **Ort setzen:** Fotos im Filmstreifen auswählen, dann über der Karte nach einem Ort suchen
  (z. B. „München“), Enter drücken, ein Ergebnis wählen und „Für N Fotos übernehmen“ anklicken; oder
  die Fotos auf die Karte ziehen; oder Rechtsklick auf die Karte → „N ausgewählte Fotos
  hierher“. „Ort entfernen“ löscht ihn wieder. Fotos mit Ort zeigen im Filmstreifen eine kleine
  Stecknadel.
- **Ortsnamen:** Jeder Ort bekommt einen Namen wie „München, Bayern, Deutschland“ (Ort, Region,
  Land). Die Bibliothekssuche findet ihn, „Ubud“ liefert also die Fotos von dort. Der Infobereich
  zeigt den Namen (bis er bekannt ist: die Koordinaten) mit einem Link zur Karte.
- **Gespeichert** werden selbst gesetzte Orte nur im Katalog. Die Dateien bleiben unverändert, und
  ein erneutes Einlesen überschreibt keinen von Hand gesetzten Ort.
- **Orte vorschlagen …** (Knopf über der Karte) gibt Fotos ohne Ort den Ort eines Fotos mit Ort
  (etwa vom Handy), das wenige Minuten davor oder danach entstand. Das gilt für die Fotos der
  Ansicht oder die ausgewählten, läuft nur auf Klick, und gespeichert wird erst, was du anhakst
  und bestätigst. Geht die Kamerauhr falsch, schätzt Lumenkorn die Abweichung je Kamera aus den
  Zeiten beider, oder du gibst sie selbst ein.
- **Internet:** Karte und Ortsnamen brauchen eine Verbindung, siehe
  [Daten und Arbeitsspeicher](#daten-und-arbeitsspeicher).

## Entwickeln (non-destruktiv)

Taste **D** oder „Entwickeln“ oben. Die Originaldatei wird nie verändert. Gespeichert wird nur der
Parametersatz der Bearbeitung, automatisch nach jeder Änderung.

Der Reiter **Basis** enthält von oben nach unten:

- **Auto** (Strg+U): stellt Belichtung, Kontrast, Lichter, Tiefen, Weiß, Schwarz und Dynamik so
  ein, dass das Foto ausgewogen aussieht. Lumenkorn entwickelt dafür eine kleine Kopie mit der
  echten Pipeline und bringt Regler für Regler auf ein Ziel: mittlere Helligkeit, keine neu
  ausgefressenen Lichter, sichtbare Tiefen, voller Tonwertumfang. Nachtaufnahmen und Motive mit
  großen hellen Flächen (Schnee, Himmel) werden nur wenig aufgehellt. Strg+Z macht es rückgängig.
- **Schwarzweiß** (V), gleich neben Auto: wandelt das Foto in Graustufen um. Die Umwandlung kommt
  nach HSL und den lokalen Anpassungen. Die HSL-Luminanz wird so zum Grau-Mix (Blau dunkler gibt
  einen dramatischen Himmel), und Masken färben das Bild nicht wieder ein. Color Grading wirkt
  danach und tont das Schwarzweißbild, etwa kühle Tiefen und warme Lichter oder Sepia.

- **Weißabgleich:** Temperatur und Tönung, relativ zum Kamera-Weißabgleich.
- **Licht:** Belichtung, Kontrast, Lichter, Tiefen, Weiß, Schwarz. Lichter und Tiefen wirken auf die
  helle bzw. dunkle Bildhälfte bis in die Mitteltöne, Weiß und Schwarz nur auf die Enden der Skala
  (Weiß- und Schwarzpunkt) – die Mitteltöne bleiben dabei nahezu, wo sie sind.
- **Farbe:** Dynamik, Sättigung.
- **Details:** Klarheit, Schärfen, **Dunst entfernen** und die KI-Rauschreduzierung. Dunst entfernen
  schätzt den Dunst nach dem Dark-Channel-Verfahren, aber mit neutraler Dunstfarbe. Farben behalten
  so ihren Ton, statt ins Blaue oder Gelbe zu kippen; die verlorene Helligkeit wird in den
  Mitteltönen zurückgegeben. Negative Werte fügen Dunst hinzu.
- **HSL:** Farbton, Sättigung und Luminanz für acht Farbbereiche, gerechnet in Oklch mit weichen
  Übergängen. Graue Flächen bleiben bei Farbton und Luminanz unberührt (Blau abdunkeln dunkelt keine
  grauen Wolken ab). Mit der **Pipette** auf eine Farbe im Bild klicken und nach oben oder unten
  ziehen; verstellt werden die getroffenen Bereiche anteilig.
- **Color Grading:** Farbräder für Tiefen, Mitteltöne, Lichter und Global wie in Lightroom. Ziehen
  setzt Farbton und Sättigung, Doppelklick setzt ein Rad zurück. Dazu Luminanz je Bereich,
  Überblenden und Abgleich. Die Tönung macht nichts heller oder dunkler, Schwarz und Weiß bleiben
  neutral.
- **Gradationskurve** (unter Color Grading), wie die RGB-Kurve in darktable: eine Kurve für RGB
  und je eine für Rot, Grün und Blau, dahinter das Histogramm des Kanals. Klicken setzt einen
  Punkt (bis zu 16), Ziehen verschiebt ihn, Doppelklick entfernt ihn. Die Punkte verbindet ein
  monotoner Spline, die Kurve schwingt zwischen ihnen also nicht über. „Farben erhalten“
  (Standard) wendet die RGB-Kurve auf die Helligkeit an, Farbton und Sättigung bleiben; aus wirkt
  sie wie in Lightroom auf jeden Kanal, mehr Kontrast macht dann auch die Farben kräftiger.
- **Objektiv:** Die **Objektivkorrektur** ist für RAWs standardmäßig an und entfernt
  Verzeichnung, Farbsäume und Vignettierung. Die Daten kommen aus dem RAW selbst (Sony
  schreibt sie in jedes ARW, DNG-Dateien enthalten sie oft) oder aus der
  [Lensfun](https://lensfun.github.io/)-Datenbank mit rund 1 500 Objektiven. Verzeichnung und
  Farbsäume nimmt Lumenkorn bevorzugt aus der Kamera, weil sie genau für Brennweite und Fokus
  der Aufnahme gelten; die Vignettierung bevorzugt aus Lensfun, weil Sony sie mit seinen Daten
  nur teilweise ausgleicht. Unter dem Schalter steht, woher die Daten kommen. Damit keine
  leeren Ränder entstehen, wird minimal vergrößert. Bearbeitungen von vor dieser Funktion, die
  Pinsel- oder KI-Masken enthalten, bleiben ohne Korrektur, damit die Masken nicht verrutschen.
- **Schnappschüsse** halten einen Stand fest, zu dem man später zurückkehren kann.
- **Auf Original zurücksetzen …** ganz unten verwirft nach einer Rückfrage alle Bearbeitungen des
  Fotos: Regler, Masken, Reparaturen, Rauschreduzierung und Zuschnitt. Schnappschüsse bleiben,
  Strg+Z macht es rückgängig, solange das Foto offen ist.

Weitere Reiter:

- **Masken:** lokale Anpassungen, siehe unten.
- **Reparatur:** Störendes mit KI entfernen.
- **Zuschnitt:** Zuschneiden mit Drittel-Raster und festen Seitenverhältnissen (Original, 1:1, 4:5,
  3:2, 16:9, per ⇄ hoch oder quer), Geraderichten bis ±45° ohne leere Ecken, mit **Auto**: Lumenkorn
  sucht Horizont und gerade Kanten und schlägt die Korrektur vor. Drehen und Spiegeln. Taste R
  öffnet den Reiter, Enter übernimmt, Esc bricht ab.

Außerdem:

- **RAW-Details:** Die RAW-Entwicklung folgt dem Standard-Ablauf von darktable 5.0 (szenenbezogen,
  mit Sigmoid): Bayer-Sensoren werden mit RCD demosaikt, Fujifilm X-Trans mit Markesteijn
  (ein Durchgang, wie bei darktable). Danach folgt eine Eingangsschärfung in Sensorauflösung, wie
  Lightroom sie standardmäßig anwendet: nur auf der Helligkeit, ohne Säume, mit einer Schwelle, die
  sich nach dem gemessenen Rauschen richtet.
- **Vorher/Nachher:** „Original zeigen“ oben in der Leiste (Klick schaltet um, gedrückt halten zeigt
  kurz) oder Taste `\`.
- **Werte per Tastatur:** Tab springt von Regler zu Regler, der Wert ist markiert und kann
  überschrieben werden. Enter übernimmt, Esc verwirft, ↑/↓ ändern schrittweise (Umschalt ×10).
  Doppelklick auf einen Regler setzt ihn zurück.
- **Verlauf** mit Strg+Z / Strg+Umschalt+Z.
- **Einstellungen übertragen:** Strg+Umschalt+C kopiert die Basis-Einstellungen, Strg+Umschalt+V
  fügt sie auf alle ausgewählten Fotos ein (auch per Rechtsklick). Masken, Reparaturen,
  Rauschreduzierung und Zuschnitt des Zielfotos bleiben unberührt.
- **Export** als JPEG, 16-Bit-TIFF oder PNG, optional verkleinert, im Hintergrund. Standardziel ist
  der Unterordner `Export` neben dem Original. Von Lumenkorn angelegte Export-Ordner tragen eine
  versteckte Markierung (`.lumenkorn-export`) und erscheinen nicht in der Bibliothek. Vorhandene
  Dateien werden nie überschrieben.

## Masken (lokale Anpassungen)

Jede Maskenebene hat eigene Regler (Belichtung, Kontrast, Lichter, Tiefen, Klarheit, Temperatur,
Tönung, Sättigung) und besteht aus einer oder mehreren Formen, die hinzugefügt oder abgezogen werden,
z. B. „Himmel minus Kirchturm“.

- **Verlauf** und **Radial** werden auf dem Bild gezogen und über Griffe nachjustiert.
- **Pinsel** mit Größe und weicher Kante; Alt oder „Radieren“ nimmt Bereiche heraus.
- **Farbe:** Mit der Pipette auf eine Farbe klicken; die Maske erfasst alle Stellen ähnlicher Farbe,
  auch in Licht und Schatten. Weitere Klicks nehmen Farbtöne hinzu, Alt-Klick entfernt einen. Der
  Regler **Farbbereich** bestimmt die Toleranz. Verglichen wird in Oklab, vor allem nach Farbart.
- **Motiv** (KI): erkennt das Hauptmotiv.
- **Himmel** (KI): erkennt den Himmel mit Wolken, Dunst und Dämmerung. Unsichere Kanten wie
  Baumkronen und Äste entscheidet Lumenkorn danach anhand der Farben des Fotos. Ohne die KI-Masken
  greift eine Erkennung nach Farbe und Fläche.
- **Objekt** (KI): auf ein Objekt klicken, weitere Klicks verfeinern, Alt-Klick nimmt heraus.
- **Person** (KI): die ganze Person oder nur einen Teil – Gesichtshaut, Haare, Augen, Iris,
  Augenbrauen, Lippen, Körperhaut oder Kleidung. Gilt für alle Personen im Foto; einzelne nimmt man
  mit **Abziehen** und dem Pinsel heraus.
- **Tier** (KI): alle Tiere, nur Vögel, nur Säugetiere oder nur Reptilien.
- **Maske zeigen** (Taste O) legt die Maske rot über das Bild.

Masken beziehen sich auf das aufrechte Original; Drehen, Spiegeln und späteres Zuschneiden
verschieben sie nicht vom Motiv. KI-Masken liegen als PNG im Datenordner. Auch Thumbnails und Export
zeigen die Masken.

## KI-Funktionen

Alle KI-Funktionen sind optional und werden unter **KI** unten links eingerichtet. Jede lädt ihre
Modelle einmal herunter; danach läuft alles offline.

| Funktion | Modell | Download |
|---|---|---|
| Schlagworte und Inhaltssuche | CLIP ViT-B/32 | ca. 160 MB inkl. ONNX Runtime |
| Masken: Motiv, Objekt, Himmel, Person, Tier | IS-Net, Segment Anything, skyseg, OWLv2, MediaPipe | ca. 640 MB |
| Rauschreduzierung | DRUNet | ca. 130 MB |
| Reparatur | LaMa | ca. 93 MB |
| Personen | YuNet, SFace | ca. 39 MB |
| Grafikkarte für die KI | ONNX Runtime WebGPU | ca. 25 MB |

### Schlagworte und Inhaltssuche

Lumenkorn analysiert alle Fotos im Hintergrund aus den Thumbnails. Schlagworte kommen aus fünf
Gruppen: Ort, Motiv, Licht, Wetter, Stil. Innerhalb einer Gruppe konkurrieren die Begriffe mit einer
neutralen Option, damit nicht jedes Foto irgendein „Wetter“ bekommt. KI-Schlagworte sind gestrichelt
umrandet und lassen sich entfernen; eigene Schlagworte werden nie überschrieben. Das Vokabular steht in
`crates/lumenkorn-ai/src/vocab.rs`.

Für die Inhaltssuche den Schalter „KI“ im Suchfeld anklicken und z. B. „Hund am Strand“ eingeben.
Deutsche Wörter aus dem Vokabular werden für CLIP ins Englische übertragen; englische Beschreibungen
funktionieren am besten. Gesucht wird innerhalb des gewählten Ordners, Projekts oder der Person.

### Rauschreduzierung

Im Reiter Basis unter „Details“. Lumenkorn lädt die Original-Gewichte von DRUNet aus dem
GitHub-Release von KAIR und baut daraus auf dem eigenen Rechner ein ONNX-Modell; PyTorch wird dafür
nicht gebraucht.

- Die Rauschstärke wird geschätzt, getrennt nach Helligkeit und auf zwei Skalen. Kamerarauschen ist
  durch das Demosaicing räumlich korreliert und würde in voller Auflösung allein unterschätzt.
- Farbrauschen wird zusätzlich in halber Auflösung entfernt.
- Der Regler **Stärke** skaliert die Schätzung: 0 = ¾, 50 (Standard) = 1,5-fach, 100 = 3-fach.
- Gerechnet wird einmal in voller Auflösung im Hintergrund. Das Ergebnis liegt im Datenordner und
  wird von Vorschau, Thumbnails und Export genutzt. **Mischung** blendet ohne Neuberechnung zum
  Original.

### Reparatur

Im Reiter **Reparatur**. Entfernt Staubflecken, Leitungen, Passanten oder Schilder und füllt die
Stelle passend aus dem Bild.

- Auswählen mit Pinsel oder Objekt-Klick. „Hinzufügen“ und „Abziehen“ fassen mehrere Auswahlen zu
  einer Reparatur zusammen. Gerechnet wird erst nach Klick auf **Reparieren**.
- LaMa rechnet mit 512 × 512 Pixeln. Jeder Bereich wird deshalb mit Umgebung ausgeschnitten,
  gefüllt und nur innerhalb der weich auslaufenden Auswahl eingesetzt. Kleine Reparaturen bleiben so
  pixelscharf.
- Das Ergebnis ist ein Flicken im Datenordner, den die Pipeline direkt nach der Rauschreduzierung
  einsetzt. Reparaturen lassen sich ausblenden, löschen und neu berechnen.

### Personen

Nach dem Download sucht Lumenkorn im Hintergrund in allen Fotos nach Gesichtern.

- **Finden und gruppieren:** YuNet findet Gesichter, auch kleine in Gruppenfotos (Suche in Kacheln).
  Jedes Gesicht wird ausgerichtet und von SFace in einen Merkmalsvektor übersetzt. Ähnliche Gesichter
  landen in derselben Gruppe; eine Person erscheint höchstens einmal pro Foto.
- **Benennen:** Einer Gruppe einen Namen geben. Ähnliche Gruppen und neue Fotos werden der Person
  danach vorgeschlagen; ✓ bestätigt, ✕ nimmt heraus und schlägt das Gesicht nie wieder vor. Ein Name,
  den es schon gibt, führt beide zusammen.
- **Name ändern:** Klick auf den Namen einer geöffneten Person oder Gruppe, oder Doppel- bzw.
  Rechtsklick in der Seitenleiste.
- **Viele auf einmal:** In „Gesichter prüfen“ Gesichter mit Klick, Umschalt-Klick oder Strg+A wählen
  und gemeinsam bestätigen oder herausnehmen. Für ausgewählte Fotos zeigt der Infobereich unter
  „Personen in der Auswahl“ alle erkannten Personen; sie lassen sich dort für alle Fotos neu zuordnen
  oder entfernen.
- **Gruppen:** „+ Gruppe“ in der Personenübersicht legt eine Gruppe an, z. B. „Familie“ oder
  „Freunde“. Jede Gruppe ist ein eigener Abschnitt, der sich wie „Unbenannt“ einklappen lässt.
  Zugeordnet wird per Ziehen der Personenkarte auf die Gruppe oder über das Ordnersymbol an der
  Karte; eine Person gehört zu höchstens einer Gruppe. Doppelklick auf den Gruppennamen benennt
  um, ✕ löscht die Gruppe (die Personen bleiben). Auch die Seitenleiste ordnet die Personen nach
  Gruppen; jede lässt sich dort für sich einklappen, und Personen lassen sich auch dort auf eine
  Gruppe ziehen oder aus ihr heraus.
- **Übersicht:** Benannte Personen ohne Gruppe stehen unter „Sonstige“, alle noch unbenannten
  Gesichtergruppen eingeklappt unter „Unbenannt“, die größten zuerst.
- Die verbreiteten InsightFace-Modelle sind bewusst nicht dabei; sie dürfen nur nichtkommerziell
  genutzt werden.

### Grafikkarte

Im KI-Dialog unter „Grafikkarte“. Zwei Teile, die unabhängig voneinander laufen:

- **Bildbearbeitung:** Vorschau, Thumbnails und Export laufen auf der Grafikkarte, sobald eine
  gefunden ist. Es muss nichts installiert werden. Klappt es nicht (keine Karte, zu wenig
  Grafikspeicher, Treiberfehler), rechnet die CPU, und der Dialog nennt den Grund.
- **KI:** Ein Klick lädt die WebGPU-Fassung von ONNX Runtime. Sie läuft auf AMD, NVIDIA und Intel über
  den normalen Treiber, ohne CUDA oder ROCm. Ein Modell läuft nur dann auf der GPU, wenn alle seine
  Operationen dort laufen können, sonst ganz auf der CPU; der Dialog zeigt pro Modell, wo es rechnet.
  Die quantisierten Modelle (Segment Anything, CLIP-Text) bleiben auf der CPU.
- **Absturzschutz:** Vor riskanten Schritten wie dem Laden einer GPU-Laufzeit legt Lumenkorn eine
  Markierung an. Stürzt die App dabei ab, schaltet der nächste Start den Auslöser ab und erklärt im
  KI-Dialog, was passiert ist.
- **„Geschwindigkeit testen“** misst die Rauschreduzierung an einem Testbild und rechnet auf ein
  24-MP-Foto hoch.

## KI-Agenten (MCP)

Lumenkorn bringt einen Server für das [Model Context Protocol](https://modelcontextprotocol.io)
mit. Damit können KI-Agenten wie Claude oder ChatGPT/Codex das Programm bedienen, etwa „Suche alle
Fotos vom Strand mit mindestens drei Sternen, mach sie wärmer und exportiere sie als JPEG“. Die
Schnittstelle ist anfangs aus und wird unter **KI → KI-Agenten (MCP)** eingeschaltet; dort stehen
auch die fertigen Angaben zum Kopieren.

- **stdio** (Claude Desktop, OpenAI Codex und die meisten anderen Agenten): Programm
  `lumenkorn.exe` (mit vollem Pfad), Argument `--mcp`. Diese kleine Brücke startet Lumenkorn bei
  Bedarf und reicht alles an das laufende Programm weiter.
- **HTTP** (Claude Code und andere Agenten, die es können): `http://127.0.0.1:47823/mcp` mit dem
  Header `Authorization: Bearer <Schlüssel>`. Der Schlüssel liegt in `mcp.json` im Datenordner;
  verbinden kann sich nur dieser Rechner, Anfragen von Webseiten werden abgewiesen.

Werkzeuge: `list_library`, `search_photos` (Text, Inhaltssuche, Ordner, Projekt, Person,
Bewertung, Markierung), `get_photo`, `view_photo` (das Foto als Bild mit seinen Bearbeitungen),
`rate_photos`, `flag_photos`, `tag_photos`, `get_develop_settings`, `set_develop_settings`
(beliebige Entwicklungswerte ändern), `auto_tone`, `reset_photos`, `apply_preset`,
`copy_settings`, `add_mask` (KI-Maske wie Himmel, `person:eyes` oder `animal:bird` mit lokalen
Anpassungen), `export_photos` und `show_photo`. Keines löscht Fotos oder entfernt Ordner.
Änderungen erscheinen sofort im Programm; beim gerade entwickelten Foto macht Strg+Z sie rückgängig.

## Bilder zusammenfügen

Mehrere Fotos auswählen, dann Rechtsklick → **Zusammenfügen** oder per Tastenkürzel. Die Berechnung
läuft im Hintergrund und lässt sich abbrechen.

| | Kürzel | Was passiert |
|---|---|---|
| **HDR** | Strg+H | Ausrichten (auch Freihand), Belichtungsverhältnisse aus den Pixeln gemessen, gewichtetes Zusammenführen, optional Geisterbilder unterdrücken. Bis ±6 EV. |
| **Panorama** | Strg+M | Reihenfolge egal, auch mehrzeilig. Brennweite und Kameradrehungen werden geschätzt, der Horizont ausgerichtet. Projektion zylindrisch, Kugel oder perspektivisch, Helligkeit angeglichen, optional automatisch zugeschnitten. |
| **Focus Stacking** | Strg+Umschalt+F | Ausrichten einschließlich Objektiv-Atmen, dann gewinnt je Pixel und Größenordnung das schärfste Bild. |

Das Ergebnis ist eine EXR-Datei (lineares 16-Bit-Gleitkomma) neben dem ersten Foto, z. B.
`IMG_0412-HDR.exr`. Sie kommt sofort in den Katalog. Aus RAWs entstandene Ergebnisse werden wie ein
RAW entwickelt, Lichter über 1,0 lassen sich also zurückholen. Zusammengefügt werden die
unbearbeiteten Originale.

## Fotoalbum

Über **Fotoalbum** oberhalb eines Projekts oder Ordners entsteht daraus ein Fotobuch. Man wählt
das Thema (Urlaub, Reise, Hochzeit, Geburtstag, Familie, Baby und Kind, Natur, Feier, Sport,
Tiere oder eine eigene Beschreibung), das Format (30 × 30, 20 × 20, A4 hoch oder quer, 28 × 21,
A3 oder eigene Maße in mm), die Anzahl der Fotos und Seiten und ob mit Rand oder randlos.

- Gemessen werden Schärfe (an der schärfsten Stelle, Porträts mit unscharfem Hintergrund
  verlieren also nicht) und Belichtung an den Vorschauen, dazu zählen Sterne und Markierungen.
  Ist die KI eingerichtet, bewertet CLIP, wie gut ein Foto zum Thema passt, und schiebt
  Bildschirmfotos und abfotografierte Dokumente nach hinten; bei Personenthemen zählen
  Gesichter. Ohne KI bleibt das Thema unberücksichtigt.
- Standardmäßig kommen nur bearbeitete Fotos und JPEGs ins Album; unbearbeitete RAW-Dateien,
  die nur das flaue Kamerabild zeigen, bleiben draußen. Ein Haken im Dialog nimmt sie dazu.
- Serien und fast gleiche Fotos zählen als ein Motiv, auch wenn andere Fotos dazwischen liegen –
  außer es sind andere Personen darauf. Fotos mit 4 oder 5 Sternen kommen immer ins Album, auch
  wenn es dadurch mehr werden als gewünscht. Die Fotos werden über die Tage und Ausflüge
  verteilt, innerhalb eines Ausflugs gewinnen verschiedene Motive gegenüber ähnlichen.
- Die Seiten werden chronologisch gefüllt, möglichst ohne Ausflüge zu mischen. Jede Seite bekommt
  die Anordnung, die die Fotos am wenigsten beschneidet; die besten Fotos stehen auch mal allein.
- Danach lässt sich alles ändern: Ein Klick auf ein Foto zeigt Ersatzfotos (aus derselben Serie
  zuerst), Entfernen und Verschieben auf die vorige oder nächste Seite; Ziehen verschiebt den
  Ausschnitt. Ein Klick auf eine Seite bietet Fotos zum Hinzufügen, eine andere Anordnung und
  Seiten einfügen oder löschen. Ein rotes Schild zeigt Fotos, deren Auflösung für die Größe im
  Druck zu gering ist.
- **Texte:** Datum und Ort stehen einmal auf der Seite, auf der ein neuer Tag bzw. Ort beginnt
  (Datum und Ort, nur Datum, nur Ort oder keine). Eigene Texte kommen über oder unter die Fotos
  (die Fotos rücken zusammen) oder frei auf die Seite (ziehen), mit Größe, Ausrichtung, Farbe und
  Fett, in Instrument Sans oder Lora. Vorschau und Druck zeichnen Texte auf dieselbe Weise.
- **Karte:** Eine Kartenseite als erste oder letzte Seite zeigt die Ländergrenzen, hebt die
  besuchten Länder hervor und setzt eine Stecknadel für jeden Ort; nahe Orte teilen sich eine
  Nadel, benannt nach dem Ort oder – für eine ganze Insel – nach der Region. Ohne Internet.
- **Als PDF speichern** schreibt ein druckfertiges PDF (300 dpi, 3 mm Beschnittzugabe an jedem
  Rand, mit Endformat-Rahmen) und einen Ordner mit jeder Seite als JPG, wie ihn die meisten
  Fotobuch-Druckereien annehmen. Das Album bleibt erhalten und lässt sich später weiter
  bearbeiten.

## Tastenkürzel

| Taste | Aktion |
|---|---|
| Pfeiltasten | Navigieren |
| Enter / Leertaste / E | Groß anzeigen (mehrere ausgewählt: Vergleich) |
| D | Entwickeln |
| M | Karte |
| Esc / G | Zurück zum Raster |
| 0–5 | Bewertung |
| P / X / U | Markieren / Ablehnen / Aufheben |
| Umschalt+X | Ablehnen und zum nächsten Foto |
| Strg+Entf | Abgelehnte Fotos löschen (mit Rückfrage) |
| Strg+A | Alles auswählen |
| Strg- / Umschalt-Klick | Mehrfachauswahl |
| C | Ausgewählte Fotos vergleichen |
| Scrollrad | Zoomen zum Mauszeiger hin |
| Strg + / Strg − / Strg 0 | Vergrößern / Verkleinern / Einpassen |
| Z oder Doppelklick | Einpassen ↔ 100 % |
| Entf | Foto aus dem Vergleich nehmen |
| I | Infobereich ein/aus |
| \ | Vorher/Nachher |
| F11 | Vollbild |
| [ / ] | Links / rechts drehen |
| Strg+U | Auto-Tonwerte (Entwickeln) |
| V | Schwarzweiß ein/aus (Entwickeln) |
| Strg+Z / Strg+Umschalt+Z | Rückgängig / Wiederholen |
| Strg+Umschalt+C / Strg+Umschalt+V | Einstellungen kopieren / einfügen |
| O | Maske rot anzeigen |
| R | Zuschneiden |
| Strg+Umschalt+E | Auswahl exportieren |
| Strg+Umschalt+I | Fotos von der Speicherkarte importieren |
| Strg+H / Strg+M / Strg+Umschalt+F | HDR / Panorama / Focus Stacking |

## So funktioniert Lumenkorn

### Aufbau

Lumenkorn besteht aus zwei Hälften. Das Fenster zeigt eine Weboberfläche in Svelte. Alles, was
Dateien, Pixel oder Modelle anfasst, passiert in Rust. Verbunden sind beide über Tauri: Die Oberfläche
ruft Rust-Befehle auf und bekommt JSON zurück. Bilder holt sie über ein eigenes URL-Schema (`lw://`),
damit keine Pixeldaten durch JSON müssen und der Browser-Cache greift.

Der Rust-Teil ist in drei Bibliotheken (Crates) aufgeteilt, die Tauri nicht kennen und sich ohne
Fenster testen lassen:

- **lumenkorn-core:** Katalog, Ordner-Scan, Dateiüberwachung, Thumbnails, Personen.
- **lumenkorn-develop:** RAW-Dekodierung, Demosaicing, Entwicklungspipeline für CPU und GPU,
  Masken, Reparatur-Flicken, Zusammenfügen, Export.
- **lumenkorn-ai:** ONNX Runtime laden, alle KI-Modelle, Downloads.

Dazu kommt `src-tauri`, die eigentliche App: Befehle für die Oberfläche, Hintergrundaufträge
(Scan → Thumbnails → KI-Analyse → Gesichter), das `lw://`-Schema, Absturzschutz und Protokoll.

### Katalog

Der Katalog ist eine SQLite-Datenbank im Datenordner. Sie enthält Quellordner, Fotos, Projekte,
Schlagworte, Gesichter und Bearbeitungen. Beim Scan liest Lumenkorn die EXIF-Daten und berechnet
für jede Datei einen BLAKE3-Hash. Verschwindet eine Datei und taucht eine mit gleichem Hash woanders
auf, gilt sie als verschoben und behält ihre Daten. Die Suche nutzt einen FTS5-Volltextindex mit
Trigrammen, deshalb finden auch Teilwörter etwas.

### Entwicklungspipeline

Eine Bearbeitung ist ein kleines JSON-Dokument mit allen Reglerwerten, Masken und dem Zuschnitt. Zum
Anzeigen wird das Foto einmal in ein lineares Gleitkommabild umgewandelt: RAWs liest rawler, danach
folgt Lumenkorn dem Standard-Ablauf von [darktable](https://github.com/darktable-org/darktable) 5.0,
aus dessen C-Code nach Rust übersetzt: Weiß- und Schwarzpegel des Sensors aus der Kameradatenbank von
[rawspeed](https://github.com/darktable-org/rawspeed), Weißabgleich wie von der Kamera gesetzt,
Lichter-Rekonstruktion („inpaint opposed“), Demosaicing (RCD bzw. Markesteijn mit einem Durchgang für
X-Trans, über rawler), Belichtung +0,7 EV abzüglich der Belichtungskorrektur der Kamera, Kameramatrix
nach linear Rec.2020 und Farbkalibrierung mit CAT16 aus dem Illuminanten der Kamera. Danach folgt die
Eingangsschärfung. JPEG, PNG, TIFF und WebP werden in lineares Licht umgerechnet.

Für die Live-Vorschau liegt dieses Bild mit 2048 Pixeln Kantenlänge im Speicher. Jede
Regleränderung rechnet die Pipeline auf dieser Vorschau neu, auf der Grafikkarte in wenigen
Millisekunden. Beim Zoomen wird nur der sichtbare Ausschnitt aus der vollen Auflösung gerechnet, beim
Export das ganze Bild.

Die Reihenfolge ist fest:

| Stufe | Arbeitsraum |
|---|---|
| Weißabgleich, Belichtung | linear, szenenbezogen |
| Sigmoid-Tonwertzuordnung (nur RAW) | Szene → Anzeige |
| Objektivkorrektur (nur RAW) | linear, vor allem anderen |
| Dunst entfernen | anzeigebezogen |
| Weiß/Schwarz, Lichter/Tiefen, Kontrast, Klarheit | Helligkeit |
| Dynamik, Sättigung, HSL, Color Grading | Farbe (HSL in Oklch) |
| Gradationskurve | anzeigebezogen (RGB bzw. Helligkeit, je Kanal) |
| Masken | je Ebene überblendet |
| Schärfen | Helligkeit |
| Drehen, Spiegeln, Geraderichten, Zuschneiden | zuletzt |

Lichter, Tiefen und Klarheit arbeiten mit weichgezeichneten Helligkeitskarten, deren Radius sich nach
der Bildgröße richtet. Die Vorschau sieht deshalb aus wie der Export. Mit Standardwerten lässt die
Pipeline ein JPEG unverändert.

Die Pipeline gibt es zweimal: in Rust für die CPU (parallel mit Rayon) und als WGSL-Compute-Shader
für die Grafikkarte über wgpu. Ein Test rechnet alle Regler, Maskenarten und Geometriefälle auf
beiden und vergleicht die Pixel; die größte erlaubte Abweichung liegt weit unter einer 8-Bit-Stufe.
Nur die Form einer Maske wird auf der CPU berechnet und zwischengespeichert, weil sie sich beim
Ziehen eines Reglers nicht ändert.

### KI

ONNX Runtime ist nicht fest eingebaut, sondern wird zur Laufzeit geladen. Lumenkorn startet und
funktioniert also auch ohne, und für jede Plattform kann die passende Fassung heruntergeladen
werden: die CPU-Fassung von den GitHub-Releases von ONNX Runtime, die WebGPU-Fassung als offizielles
Paket von PyPI (geprüft per SHA-256). Weil die WebGPU-Laufzeit nicht zwei Modelle gleichzeitig
rechnen lässt, kommen Rechnungen auf der GPU nacheinander dran.

Aufwendige Ergebnisse (entrauschtes Bild, Reparatur-Flicken, KI-Masken) liegen im Datenordner und
fließen wie jede andere Eingabe in die Pipeline ein. Modelle werden nach einigen Minuten ohne Nutzung
aus dem Speicher entladen; ein Modell, das gerade rechnet, nie.

Die Himmelserkennung arbeitet in zwei Schritten: skyseg sagt für jeden Bildpunkt, wie sicher er Himmel
ist. Wo es unsicher ist, etwa an Ästen, vergleicht Lumenkorn den Bildpunkt mit den Farben von
sicherem Himmel und sicherem Vordergrund in der Nähe.

### Zusammenfügen

Alle drei Verfahren richten die Bilder zuerst aus: Harris-Ecken auf einem helligkeitsnormierten
Graubild, BRIEF-Beschreiber, Zuordnung über den Hamming-Abstand und eine Homografie per RANSAC.
HDR misst danach die Belichtungsverhältnisse und führt zur linearen Strahldichte zusammen. Das
Panorama schätzt aus allen Bildpaaren Brennweite und Drehungen und projiziert. Focus Stacking
zerlegt jedes Bild in eine Laplace-Pyramide und behält je Stufe das schärfste Detail.

### Daten und Arbeitsspeicher

- **Datenordner:** Katalog, Bearbeitungen, KI-Modelle und KI-Ergebnisse. Windows
  `%APPDATA%\io.github.drayprescot.lumenkorn\`, Linux `~/.local/share/io.github.drayprescot.lumenkorn/`.
  Thumbnails liegen im Cache-Ordner des Systems. Daten der früheren Fassung „Lichtwerk“ werden beim
  ersten Start übernommen.
- **Fotoordner** werden nur beschrieben, wenn du es auslöst: abgelehnte Fotos löschen,
  Zusammenfügen, Export.
- **Netzwerk** braucht Lumenkorn für die KI-Downloads und für die Karte. Downloads nutzen den
  Zertifikatsspeicher des Betriebssystems und funktionieren damit auch hinter Firmen-Proxys.
  `ki/quellen.json` im Datenordner kann die Download-Adressen überschreiben, z. B. für einen
  Spiegelserver. Kartenkacheln kommen von `tile.openstreetmap.org`, Ortssuche und Ortsnamen von
  `nominatim.openstreetmap.org`. Die Anfragen laufen über die App mit einem eindeutigen User-Agent,
  an Nominatim höchstens eine pro Sekunde, und Kacheln werden 7 Tage im Cache-Ordner
  zwischengespeichert. Die Koordinaten deiner Fotos gehen an Nominatim, damit es Ortsnamen liefert –
  auf etwa 100 m gerundet und gebündelt: Eine Anfrage gilt für alle Fotos im selben Ort.
  Alles andere bleibt lokal.
- **Aufräumen:** 90 Sekunden nach dem Start und danach täglich löscht Lumenkorn verwaiste Dateien im
  Datenordner (alte Entrauschungen, gelöschte Masken und Reparaturen, Vorschauen entfernter Fotos).
  Was eine Bearbeitung oder ein Schnappschuss noch braucht, bleibt.
- **Speicher:** Zwischenspeicher wie das Bild in voller Auflösung werden beim Verlassen des Entwickelns
  freigegeben, KI-Modelle nach 3 bzw. 5 Minuten ohne Nutzung. Die Vorschauen der letzten vier
  bearbeiteten Fotos bleiben im Speicher, damit sie beim Zurückkehren sofort da sind.
- **Protokoll:** `lumenkorn.log` im Datenordner, mit Fehlern, Rust-Paniken und den Angaben der KI
  (z. B. „Himmel (KI): Fläche …“).

## Technik

| Bereich | Eingesetzt |
|---|---|
| Sprache | Rust 1.95 (Edition 2024), TypeScript |
| App-Gerüst | Tauri 2 (WebView2 unter Windows, WebKitGTK unter Linux) |
| Oberfläche | Svelte 5 (Runes), Vite |
| Datenbank | SQLite über rusqlite, FTS5-Volltextsuche |
| RAW | rawler (aus dnglab); RCD, Inpaint opposed, Farbkalibrierung und Sigmoid aus darktable übersetzt; Sensorpegel aus rawspeed |
| Bilddateien | image-rs, jpeg-encoder, exrs, kamadak-exif |
| Grafikkarte | wgpu mit WGSL-Compute-Shadern (DirectX 12, Vulkan, Metal) |
| Parallelisierung | Rayon |
| KI | ONNX Runtime über ort, CPU oder WebGPU |
| Downloads | ureq mit dem Zertifikatsspeicher des Systems |
| CI | GitHub Actions, Mesa lavapipe für GPU-Tests ohne Grafikkarte, Dependabot |
| Installer | Tauri-Bundler: MSI und NSIS (Windows), deb, rpm, AppImage (Linux) |

## Genutzte Open-Source-Projekte

Lumenkorn baut auf der Arbeit vieler anderer auf. Danke an alle, die diese Projekte pflegen.
Dieselbe Liste steht auch in der App: unten links in der Seitenleiste auf ⓘ (**Über Lumenkorn**)
klicken.

**In der App enthalten:**
[Tauri](https://github.com/tauri-apps/tauri) mit den Plugins [dialog, updater und single-instance](https://github.com/tauri-apps/plugins-workspace) (MIT/Apache-2.0),
[Svelte](https://github.com/sveltejs/svelte) (MIT),
[rawler](https://github.com/dnglab/dnglab) (LGPL-2.1),
[darktable](https://github.com/darktable-org/darktable) (GPL-3.0-or-later; RAW-Entwicklung: Lichter-Rekonstruktion, Demosaicing, Farbkalibrierung, Sigmoid, nach Rust übersetzt),
die Kameradaten von [rawspeed](https://github.com/darktable-org/rawspeed) (LGPL-2.1-or-later; `cameras.xml`, mit `tools/rawspeed_white.py` zu einer Tabelle für Weiß- und Schwarzpegel verkleinert),
[wgpu](https://github.com/gfx-rs/wgpu) (MIT/Apache-2.0),
[ort](https://github.com/pykeio/ort) (MIT/Apache-2.0),
[SQLite](https://sqlite.org) (gemeinfrei) über [rusqlite](https://github.com/rusqlite/rusqlite) (MIT),
[image](https://github.com/image-rs/image) (MIT/Apache-2.0),
[jpeg-encoder](https://github.com/vstroebel/jpeg-encoder) (MIT/Apache-2.0, IJG),
[exrs](https://github.com/johannesvollmer/exrs) (BSD-3-Clause),
[kamadak-exif](https://github.com/kamadak/exif-rs) (BSD-2-Clause),
[Rayon](https://github.com/rayon-rs/rayon) (MIT/Apache-2.0),
[notify](https://github.com/notify-rs/notify) (CC0-1.0),
[walkdir](https://github.com/BurntSushi/walkdir) (MIT/Unlicense),
[BLAKE3](https://github.com/BLAKE3-team/BLAKE3) (CC0-1.0/Apache-2.0),
[trash](https://github.com/Byron/trash-rs) (MIT),
[ureq](https://github.com/algesten/ureq) (MIT/Apache-2.0),
[tiny_http](https://github.com/tiny-http/tiny-http) für die Schnittstelle für KI-Agenten (MIT/Apache-2.0),
[serde](https://github.com/serde-rs/serde) und [serde_json](https://github.com/serde-rs/json) (MIT/Apache-2.0),
[sha2](https://github.com/RustCrypto/hashes) (MIT/Apache-2.0),
[flate2](https://github.com/rust-lang/flate2-rs), [tar](https://github.com/composefs/tar-rs) und [zip](https://github.com/zip-rs/zip2) (MIT/Apache-2.0 bzw. MIT),
[half](https://github.com/VoidStarKat/half-rs), [bytemuck](https://github.com/Lokathor/bytemuck) und [pollster](https://github.com/zesterer/pollster) (MIT/Apache-2.0),
[regex](https://github.com/rust-lang/regex), [log](https://github.com/rust-lang/log), [env_logger](https://github.com/rust-cli/env_logger), [thiserror](https://github.com/dtolnay/thiserror), [dirs](https://github.com/dirs-dev/dirs-rs) und [base64](https://github.com/marshallpierce/rust-base64) (MIT/Apache-2.0),
[libloading](https://github.com/nagisa/rust_libloading) (ISC),
die [Lensfun](https://github.com/lensfun/lensfun)-Datenbank für die Objektivkorrektur (CC BY-SA 3.0, als JSON mitgeliefert, erzeugt mit `tools/lensfun_db.py`),
die Schriften [Instrument Sans](https://github.com/Instrument/instrument-sans) und [Lora](https://github.com/cyrealtype/Lora-Cyrillic) über [Fontsource](https://fontsource.org) (OFL-1.1),
die Ländergrenzen von [Natural Earth](https://www.naturalearthdata.com) (Admin 0, 1:50m) für die Karte im Fotoalbum (gemeinfrei, mitgeliefert, erzeugt mit `tools/build_countries.mjs`),
[Leaflet](https://github.com/Leaflet/Leaflet) für die Kartenanzeige (BSD-2-Clause),
[Leaflet.markercluster](https://github.com/Leaflet/Leaflet.markercluster) für die gruppierten Markierungen (MIT).

**Online-Dienst:** Die Karte nutzt [OpenStreetMap](https://www.openstreetmap.org/copyright).
Kartendaten © OpenStreetMap-Mitwirkende (ODbL), Standardkacheln CC BY-SA 2.0, Ortssuche über
Nominatim. Der Hinweis „© OpenStreetMap contributors“ steht auf der Karte.

**Zur Laufzeit geladen (nicht mitgeliefert):**

| Was | Von | Bezugsquelle | Lizenz |
|---|---|---|---|
| ONNX Runtime | [Microsoft](https://github.com/microsoft/onnxruntime) | GitHub-Releases, PyPI (WebGPU) | MIT |
| Dawn, DirectX Shader Compiler (in der WebGPU-Fassung von ONNX Runtime enthalten) | [Google](https://dawn.googlesource.com/dawn), [Microsoft](https://github.com/microsoft/DirectXShaderCompiler) | mit ONNX Runtime | BSD-3-Clause, NCSA (`dxil.dll`: Microsoft-Lizenz) |
| CLIP ViT-B/32 | OpenAI, ONNX-Fassung von [Xenova](https://huggingface.co/Xenova/clip-vit-base-patch32) | Hugging Face | MIT |
| IS-Net / DIS | [Xuebin Qin u. a.](https://github.com/xuebinqin/DIS) | Releases von [rembg](https://github.com/danielgatis/rembg) | Apache-2.0 |
| Segment Anything ViT-B | [Meta AI](https://github.com/facebookresearch/segment-anything) | Releases von rembg | Apache-2.0 |
| skyseg (U²-Net) | [xiongzhu666](https://github.com/xiongzhu666/Sky-Segmentation-and-Post-processing), auf Basis von [U²-Net](https://github.com/xuebinqin/U-2-Net) | [Hugging Face](https://huggingface.co/JianyuanWang/skyseg) | MIT |
| OWLv2 (base, patch16) | [Google Research](https://github.com/google-research/scenic/tree/main/scenic/projects/owl_vit), ONNX-Fassung von [Xenova](https://huggingface.co/Xenova/owlv2-base-patch16) | Hugging Face | Apache-2.0 |
| MediaPipe Selfie Multiclass | [Google](https://ai.google.dev/edge/mediapipe/solutions/vision/image_segmenter), ONNX-Umwandlung von [senty-au](https://huggingface.co/senty-au/selfie_multiclass_256x256-ONNX) | Hugging Face | Apache-2.0 |
| MediaPipe Face Mesh V2 | [Google](https://ai.google.dev/edge/mediapipe/solutions/vision/face_landmarker), ONNX-Umwandlung für [QtMeshEditor](https://huggingface.co/fernandotonon/QtMeshEditor-facemesh-onnx) | Hugging Face | Apache-2.0 |
| DRUNet | Kai Zhang u. a., [KAIR](https://github.com/cszn/KAIR) | Releases von KAIR | MIT |
| LaMa | [Samsung AI](https://github.com/advimman/lama) | [OpenCV Model Zoo](https://github.com/opencv/opencv_zoo) | Apache-2.0 |
| YuNet | Shiqi Yu u. a. | OpenCV Model Zoo | MIT |
| SFace | Yaoyao Zhong u. a. | OpenCV Model Zoo | Apache-2.0 |

**Nicht quelloffen, aber nötig:** Unter Windows zeigt Lumenkorn seine Oberfläche mit Microsoft
WebView2, das zu Windows gehört. Die GPU-Fassung von ONNX Runtime braucht das Microsoft Visual C++
Redistributable; fehlt es, weist Lumenkorn darauf hin. Beides liefert Lumenkorn nicht mit.

**Veröffentlichte Verfahren, die Lumenkorn umsetzt:** die Objektivmodelle von Lensfun/Hugin, die DNG-Opcodes WarpRectilinear und FixVignetteRadial, die Bedeutung der Sony-Korrekturdaten nach den Vorarbeiten von [darktable](https://github.com/darktable-org/darktable) und [RawTherapee](https://github.com/Beep6581/RawTherapee) (dafür ohne deren Code), aus darktable nach Rust übersetzt: RCD-Demosaicing (Luis Sanz Rodríguez),
die Lichter-Rekonstruktion „inpaint opposed“ (garagecoder und Iain vom G'MIC-Team sowie Hanno Schwalm), Farbkalibrierung mit CAT16 und der Sigmoid-Tonwertzuordner,
Markesteijn-Demosaicing für X-Trans (Frank Markesteijn, über rawler), Oklab/Oklch (Björn Ottosson), Dark Channel
Prior (Kaiming He u. a.) als Grundlage für „Dunst entfernen“, Harris-Ecken, BRIEF, RANSAC und
Hartley-Normierung zum Ausrichten, Laplace-Pyramiden (Burt und Adelson) für Focus Stacking.

**Zum Entwickeln und Testen:** [TypeScript](https://github.com/microsoft/TypeScript),
[Vite](https://github.com/vitejs/vite), [svelte-check](https://github.com/sveltejs/language-tools),
die [Tauri-CLI](https://github.com/tauri-apps/tauri), [Mesa](https://www.mesa3d.org) (lavapipe für
GPU-Tests in der CI), [tempfile](https://github.com/Stebalien/tempfile) und die RAW-Beispiele aus
[OpenImageIO-images](https://github.com/AcademySoftwareFoundation/OpenImageIO-images), mit denen die
CI jedes rawler-Update prüft.

## Entwickeln am Code

Voraussetzungen: Rust 1.95 (in `rust-toolchain.toml` festgelegt, rustup installiert es selbst),
Node.js ab 20 und die Tauri-Systemabhängigkeiten:

- **Windows:** Microsoft C++ Build Tools und WebView2 (ab Windows 10 meist vorhanden).
- **Linux (Debian/Ubuntu):** `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev librsvg2-dev libayatana-appindicator3-dev build-essential`
- Details: <https://v2.tauri.app/start/prerequisites/>

```bash
npm install
npm run tauri dev        # App mit Hot-Reload starten
npm run tauri build      # Installer bauen
npm run dev              # nur die Oberfläche im Browser, mit Beispieldaten
LUMENKORN_DATA_DIR=/tmp/lk npm run tauri dev   # mit eigenem Katalog und Cache (echten Katalog nicht anfassen)

cargo test --workspace                                               # alle Tests
cargo run -p lumenkorn-core --example scan -- ~/Bilder               # Scan ohne GUI
cargo run -p lumenkorn-develop --example develop -- in.NEF out.jpg '{"exposure":0.5}'
cargo run --release -p lumenkorn-develop --example bench             # CPU gegen GPU
cargo run --release -p lumenkorn-develop --example merge -- in.CR2 /tmp/merge-test

# Tests mit echten Dateien (optional)
LUMENKORN_TEST_RAW=/ordner/mit/raws cargo test -p lumenkorn-develop
LUMENKORN_TEST_ORT=/pfad/libonnxruntime.so LUMENKORN_TEST_MODEL=/pfad/modell cargo test -p lumenkorn-ai
LUMENKORN_TEST_ORT=/pfad/libonnxruntime.so LUMENKORN_TEST_SEG=/pfad/masken cargo test -p lumenkorn-ai --test segment
LUMENKORN_TEST_ORT=/pfad/libonnxruntime.so LUMENKORN_TEST_DENOISE=/pfad cargo test -p lumenkorn-ai --test denoise
LUMENKORN_TEST_ORT=/pfad/libonnxruntime.so LUMENKORN_TEST_FACES=/pfad cargo test -p lumenkorn-ai --test faces
LUMENKORN_TEST_ORT=/pfad/libonnxruntime.so LUMENKORN_TEST_LAMA=/pfad/lama.onnx LUMENKORN_TEST_IMAGE=/pfad/foto.jpg cargo test --release -p lumenkorn-ai --test inpaint
# GPU gegen CPU (ohne Grafikkarte: Mesa lavapipe)
LUMENKORN_GPU_SOFTWARE=1 cargo test -p lumenkorn-develop --test gpu
LUMENKORN_TEST_ORT_GPU=/pfad/runtime-webgpu/libonnxruntime.so LUMENKORN_TEST_DENOISE=/pfad LUMENKORN_TEST_SEG=/pfad/masken cargo test --release -p lumenkorn-ai --test gpu
```

Vor einem Pull Request: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, `npm run check` und `npm run i18n`.

**Übersetzungen:** Die Texte der Oberfläche stehen im Quelltext auf Deutsch und sind zugleich der
Schlüssel: `t("Ordner hinzufügen")` ergibt auf Englisch „Add folder“. Die englischen Texte liegen in
`src/lib/i18n/en/*.ts`. Für Wörter mit mehreren Bedeutungen sucht `tc("kontext", "…")` unter
`"kontext|…"`. Meldungen aus Rust: `tr!("deutsch", "english")`. `npm run i18n` meldet fehlende
Übersetzungen und abweichende Platzhalter.

**Abhängigkeiten:** Dependabot prüft jeden Montag Rust-Crates, npm-Pakete und GitHub-Actions. Kleine
Updates werden nach grüner CI automatisch übernommen, dabei entsteht eine neue Nightly-MSI.
Inkompatible Sprünge bekommen das Label `manual-review` und bleiben offen. Jede neue rawler-Version
prüft die CI mit echten RAWs mehrerer Kameras. Rust-Version, KI-Modelle und ONNX Runtime sind bewusst
festgelegt und werden von Hand aktualisiert.

**Releases:** Ein Versions-Tag wie `v0.2.0` baut die Installer für Windows und Linux und legt einen
Release-Entwurf an; sein Text ist der Abschnitt der Version aus dem CHANGELOG. Von Hand gestartet
(Actions → Release → Run workflow) baut der Workflow nur die Installer und hebt sie 14 Tage als
Download auf. Mit dem Secret `TAURI_SIGNING_PRIVATE_KEY` (minisign-Schlüssel aus
`npx tauri signer generate`, der öffentliche Schlüssel steht in `tauri.conf.json`) werden die
Installer für den Updater signiert und `latest.json` angehängt. Sobald der Entwurf veröffentlicht
ist, bieten installierte Programme das Update an.

**Release ohne GitHub Actions:** In VS Code **Terminal → Aufgabe ausführen → „Lumenkorn: Release
bauen“** (oder `node tools/release_local.mjs <Pfad zu lumenkorn-updater.key>`) baut die
Windows-Installer, signiert sie und erzeugt `latest.json`. Die beiden MSI und `latest.json` dann
in ein veröffentlichtes Release mit dem Tag `v<Version>` hochladen.

## Android

Der Rust-Code ist für Android vorbereitet, die Oberfläche passt sich schmalen Bildschirmen an.
Gebaut und getestet ist die Android-Fassung bisher nicht.

```bash
# Android Studio mit SDK + NDK installieren, ANDROID_HOME und NDK_HOME setzen
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
npm run tauri android init
npm run tauri android dev
```

- **Dateizugriff:** Android erlaubt seit Version 11 keinen freien Zugriff auf Ordnerpfade. Die App
  braucht dafür *Zugriff auf alle Dateien*: nach `tauri android init` in
  `src-tauri/gen/android/app/src/main/AndroidManifest.xml`
  `<uses-permission android:name="android.permission.MANAGE_EXTERNAL_STORAGE" />` ergänzen. Den Pfad
  fragt die App ab (z. B. `/storage/emulated/0/DCIM`).
- **KI:** Für Android gibt es keinen automatischen Download der Laufzeit. `libonnxruntime.so` aus dem
  offiziellen AAR in `<daten>/ki/runtime/` legen; die Modelle lädt die App wie auf dem Desktop.

## Fahrplan

1. Presets: Einstellungen speichern und auf Fotos anwenden.
2. Masken: Helligkeitsbereich als Form, Masken auf andere Bilder übertragen.
3. Mehrsprachiges Textmodell für die Inhaltssuche, hierarchische Schlagworte, XMP-Sidecars.
4. GPU-Feinschliff nach Messungen auf echter Hardware, z. B. die Vorschau direkt als Textur zeigen.

## Bekannte Grenzen

- Windows: Pfadvergleiche unterscheiden Groß- und Kleinschreibung (`D:\Fotos` und `d:\fotos` wären
  zwei Ordner).
- Die Temperatur ist relativ zum Kamera-Weißabgleich, nicht in Kelvin.
- Jede aktive Maskenebene kostet einen weiteren Durchlauf der Tonwert-Pipeline; ohne Grafikkarte wird
  die Vorschau bei vielen Ebenen spürbar langsamer.
- Die Vorschau wird nach dem Rechnen auf der GPU als JPEG an die Oberfläche übergeben. Bei sehr
  schnellen Karten ist das der größere Teil der Zeit.
- Ändert sich eine Originaldatei nach dem Entrauschen, wird das Ergebnis nicht automatisch neu
  berechnet.
- Panoramen über volle 360° gehen noch nicht; Aufnahmen brauchen etwa 20 % Überlappung. Wurde die
  Kamera nicht um den Objektiv-Drehpunkt geschwenkt, zeigen sich im Nahbereich Doppelkonturen.
- Zusammenfügen rechnet in voller Auflösung auf der CPU. Eine HDR-Reihe aus drei 24-MP-RAWs braucht
  etwa 1–2 GB Arbeitsspeicher, Panoramen sind auf 150 MP begrenzt.

## Lizenz

Ab Version 1.0.5 steht Lumenkorn unter der [GNU General Public License, Version 3 oder
neuer](../LICENSE) (GPL-3.0-or-later), weil es aus darktable übersetzten Code enthält
(GPL-3.0-or-later). Die Versionen bis einschließlich 1.0.4 wurden unter MIT OR Apache-2.0
veröffentlicht und bleiben unter diesen Bedingungen verfügbar. Die Sensortabelle
(`crates/lumenkorn-develop/data/rawspeed_sensors.tsv`) stammt aus der Kameradatenbank von rawspeed
(LGPL-2.1-or-later). Die mitgelieferte Lensfun-Datenbank steht unter CC BY-SA 3.0
(`crates/lumenkorn-develop/data/LICENSE-lensfun-db.txt`). rawler steht unter der LGPL-2.1 und wird in den Installern statisch gelinkt; wer
eine geänderte Fassung nutzen will, ändert die Abhängigkeit `rawler` in
`crates/lumenkorn-develop/Cargo.toml` und baut selbst. Die KI-Modelle gehören nicht zu Lumenkorn und
behalten ihre oben genannten Lizenzen.
