"""Weiß- und Schwarzpunkte der Sensoren aus der Kameradatenbank von rawspeed
(darktable) als kompakte Tabelle für Lumenkorn.

rawspeed: https://github.com/darktable-org/rawspeed, data/cameras.xml,
LGPL-2.1-or-later. Aufruf:

    python tools/rawspeed_white.py cameras.xml > crates/lumenkorn-develop/data/rawspeed_sensors.tsv

Spalten: Hersteller, Modell, Modus, ISO von, ISO bis, ISO-Liste, Schwarz, Weiß.
Hersteller und Modell kommen doppelt vor: wie in der Datei (Camera) und
bereinigt (ID, Alias) – rawler liefert je nach Kamera das eine oder andere.
"""

import sys
import xml.etree.ElementTree as ET

root = ET.parse(sys.argv[1]).getroot()
rows = set()
for cam in root.iter("Camera"):
    if cam.get("supported", "yes") == "no":
        continue
    mode = cam.get("mode", "")
    names = {(cam.get("make", ""), cam.get("model", ""))}
    ident = cam.find("ID")
    if ident is not None:
        names.add((ident.get("make", ""), ident.get("model", "")))
    aliases = cam.find("Aliases")
    if aliases is not None:
        for a in aliases.iter("Alias"):
            names.add((cam.get("make", ""), (a.text or "").strip()))
            if ident is not None and a.get("id"):
                names.add((ident.get("make", ""), a.get("id")))
    for s in cam.iter("Sensor"):
        white, black = s.get("white"), s.get("black")
        if white is None:
            continue
        for make, model in names:
            if make and model:
                rows.add((make, model, mode, s.get("iso_min", ""), s.get("iso_max", ""),
                          s.get("iso_list", ""), black or "", white))
print("# Aus rawspeed data/cameras.xml (LGPL-2.1-or-later), erzeugt mit tools/rawspeed_white.py")
for r in sorted(rows):
    print("\t".join(r))
