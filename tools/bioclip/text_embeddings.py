# Text-Embeddings der Tier-Taxonomie für BioCLIP 2 (Tierarten-Schlagworte).
#
# Lumenkorn lädt von BioCLIP 2 nur den Bildteil (ONNX); die Texte für die rund
# 300 Tiere aus crates/lumenkorn-ai/src/animals.rs werden hier einmal mit dem
# Textteil berechnet und als Datei mitgeliefert. Nach Änderungen an der
# Taxonomie (neue Tiere) taxa.tsv ergänzen und neu erzeugen:
#
#   pip install torch open_clip_torch
#   python tools/bioclip/text_embeddings.py
#
# Die Beschreibung ist der taxonomische Pfad („a photo of Animalia Chordata Aves
# Passeriformes Paridae Cyanistes caeruleus.“) – so wurde BioCLIP 2 trainiert,
# und im Vergleich traf er am besten. Ohne Taxon (z. B. „Meerestier“) gilt die
# englische Beschreibung aus animals.rs.
#
# Datei: "LKTX", Version (u32), Anzahl (u32), Dimension (u32), dann je Eintrag
# Länge (u16) und UTF-8 des deutschen Schlagworts, danach die Werte (f32),
# alles little-endian, Vektoren auf Länge 1 normiert.
import csv
import os
import re
import struct

import open_clip
import torch

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
TSV = os.path.join(ROOT, "tools", "bioclip", "taxa.tsv")
RS = os.path.join(ROOT, "crates", "lumenkorn-ai", "src", "animals.rs")
OUT = os.path.join(ROOT, "crates", "lumenkorn-ai", "assets", "bioclip2-tiere.bin")

text = open(RS, encoding="utf8").read()
phrases = {m.group(1): m.group(3) for m in re.finditer(r'\b[st]\(\s*"([^"]+)",\s*"([^"]+)",\s*"([^"]+)"', text, re.S)}
rows = list(csv.DictReader(open(TSV, encoding="utf8"), delimiter="\t"))
missing = set(phrases) - {r["deutsch"] for r in rows}
if missing:
    raise SystemExit(f"In taxa.tsv fehlen: {sorted(missing)}")


def related(a, b):
    """Gleicher Name oder Haus-/Wildform (Canis lupus / Canis lupus familiaris)"""
    return a == b or a.startswith(b + " ") or b.startswith(a + " ")


def prompt(r):
    path = r["path"].strip()
    sci = r["scientific"].strip()
    if path and sci not in ("", "-"):
        # Rassen und Hausformen (Kuh/Hochlandrind, Hund/Wolf) teilen sich den
        # Namen – dann hilft der englische Name beim Unterscheiden
        twins = [o for o in rows if o is not r and o["scientific"].strip() not in ("", "-") and related(sci, o["scientific"].strip())]
        if twins:
            return f"a photo of {path} with common name {r['english'].lower()}."
        return f"a photo of {path}."
    return "a photo of " + re.sub(r"^(a|an) ", "", phrases[r["deutsch"]]) + "."


model, _, _ = open_clip.create_model_and_transforms("hf-hub:imageomics/bioclip-2")
tok = open_clip.get_tokenizer("hf-hub:imageomics/bioclip-2")
model.eval()
rows = [r for r in rows if r["deutsch"] in phrases]
with torch.no_grad():
    e = model.encode_text(tok([prompt(r) for r in rows]))
    e = e / e.norm(dim=-1, keepdim=True)
with open(OUT, "wb") as f:
    f.write(b"LKTX" + struct.pack("<III", 1, len(rows), e.shape[1]))
    for r, v in zip(rows, e.numpy()):
        name = r["deutsch"].encode("utf8")
        f.write(struct.pack("<H", len(name)) + name)
        f.write(v.astype("<f4").tobytes())
print(f"{len(rows)} Einträge, {e.shape[1]} Werte je Eintrag -> {OUT}")
