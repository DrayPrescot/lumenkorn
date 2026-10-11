#!/usr/bin/env python3
"""Lensfun-Datenbank → kompaktes JSON für Lumenkorn.

    python3 tools/lensfun_db.py <lensfun/data/db> crates/lumenkorn-develop/data/lensfun.json.gz [version]

Übernommen werden Kameras (Crop-Faktor), Bajonette und rektilineare Objektive
mit ihren Kalibrierungen (Verzeichnung, Farbsäume, Vignettierung). Die
Datenbank steht unter CC BY-SA 3.0 (https://lensfun.github.io/).
"""
import glob, gzip, json, os, sys
import xml.etree.ElementTree as ET

def num(v, d=None):
    if v is None:
        return d
    v = v.strip()
    if ":" in v:
        a, b = v.split(":")
        return float(a) / float(b)
    return float(v)

def names(el, tag):
    return [e.text.strip() for e in el.findall(tag) if e.text and e.text.strip()]

src, out = sys.argv[1], sys.argv[2]
version = sys.argv[3] if len(sys.argv) > 3 else ""
mounts, cameras, lenses = {}, [], []
for f in sorted(glob.glob(os.path.join(src, "*.xml"))):
    root = ET.parse(f).getroot()
    for m in root.findall("mount"):
        n = (m.findtext("name") or "").strip()
        if n:
            mounts.setdefault(n, []).extend(names(m, "compat"))
    for c in root.findall("camera"):
        crop = num(c.findtext("cropfactor"))
        if not crop:
            continue
        cameras.append({"maker": names(c, "maker"), "model": names(c, "model"),
                        "mount": (c.findtext("mount") or "").strip(), "crop": crop})
    for l in root.findall("lens"):
        if (l.findtext("type") or "rectilinear").strip() != "rectilinear":
            continue
        crop = num(l.findtext("cropfactor"))
        cal = l.find("calibration")
        if not crop or cal is None:
            continue
        dist, tca, vig = [], [], []
        for d in cal.findall("distortion"):
            m, fo = d.get("model"), num(d.get("focal"))
            if fo is None:
                continue
            if m == "poly3":
                dist.append([fo, "poly3", num(d.get("k1"), 0)])
            elif m == "poly5":
                dist.append([fo, "poly5", num(d.get("k1"), 0), num(d.get("k2"), 0)])
            elif m == "ptlens":
                dist.append([fo, "ptlens", num(d.get("a"), 0), num(d.get("b"), 0), num(d.get("c"), 0)])
        for t in cal.findall("tca"):
            m, fo = t.get("model"), num(t.get("focal"))
            if fo is None:
                continue
            if m == "linear":
                tca.append([fo, "linear", num(t.get("kr"), 1), num(t.get("kb"), 1)])
            elif m == "poly3":
                tca.append([fo, "poly3", num(t.get("vr"), 1), num(t.get("cr"), 0), num(t.get("br"), 0),
                            num(t.get("vb"), 1), num(t.get("cb"), 0), num(t.get("bb"), 0)])
        for v in cal.findall("vignetting"):
            if v.get("model") != "pa":
                continue
            fo, ap = num(v.get("focal")), num(v.get("aperture"))
            if fo is None or ap is None:
                continue
            vig.append([fo, ap, num(v.get("distance"), 1000), num(v.get("k1"), 0), num(v.get("k2"), 0), num(v.get("k3"), 0)])
        if not (dist or tca or vig):
            continue
        center = l.find("center")
        lenses.append({
            "maker": names(l, "maker"), "model": names(l, "model"), "mount": names(l, "mount"),
            "crop": crop, "aspect": num(l.findtext("aspect-ratio"), 1.5),
            "center": [num(center.get("x"), 0), num(center.get("y"), 0)] if center is not None else [0, 0],
            "dist": dist, "tca": tca, "vig": vig,
        })
db = {"version": version, "mounts": mounts, "cameras": cameras, "lenses": lenses}
data = json.dumps(db, separators=(",", ":"), ensure_ascii=False).encode()
with gzip.GzipFile(out, "wb", 9, mtime=0) as g:
    g.write(data)
print(f"{len(cameras)} Kameras, {len(lenses)} Objektive, {len(data)//1024} KB → {os.path.getsize(out)//1024} KB")
