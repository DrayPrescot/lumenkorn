# Text-Embeddings der Tierarten-Liste für BioCLIP 2.
#
# Lumenkorn lädt von BioCLIP 2 nur den Bildteil (ONNX). Die Texte für die
# rund 10 000 Tiere aus tierarten.tsv (build_taxonomy.py) werden hier einmal
# mit dem Textteil berechnet und als Datei neben dem Modell veröffentlicht
# (Hugging Face, DrayPrescot/bioclip2-onnx):
#
#   pip install torch open_clip_torch
#   python tools/bioclip/text_embeddings.py <ausgabe.bin>
#
# Datei (little-endian): "LKTA", Version (u32), Anzahl (u32), Dimension (u32);
# je Knoten: Elternindex (i32, -1 = Klasse), Rang (u8: 0 Klasse, 1 Gruppe,
# 2 Art), Vorab-Wert (f32, Logarithmus der Häufigkeit, siehe
# build_taxonomy.py), deutscher und englischer Name (je u16-Länge + UTF-8);
# danach je Knoten ein Skalierungsfaktor (f32) und zuletzt die Vektoren als
# i8 (Wert = i8 × Faktor, auf Länge 1 normiert).
import csv
import os
import struct
import sys

import numpy as np
import open_clip
import torch

HERE = os.path.dirname(os.path.abspath(__file__))
out = sys.argv[1]
rows = list(csv.DictReader(open(os.path.join(HERE, "tierarten.tsv"), encoding="utf8"), delimiter="\t"))
index = {r["key"]: i for i, r in enumerate(rows)}
RANK = {"class": 0, "group": 1, "species": 2}

model, _, _ = open_clip.create_model_and_transforms("hf-hub:imageomics/bioclip-2")
tok = open_clip.get_tokenizer("hf-hub:imageomics/bioclip-2")
model.eval()
emb = []
with torch.no_grad():
    for i in range(0, len(rows), 256):
        e = model.encode_text(tok([r["prompt"] for r in rows[i:i + 256]]))
        emb.append((e / e.norm(dim=-1, keepdim=True)).numpy())
        print(f"{i + 256}/{len(rows)}", flush=True)
emb = np.concatenate(emb).astype(np.float32)
scale = np.abs(emb).max(axis=1) / 127.0
q = np.clip(np.round(emb / scale[:, None]), -127, 127).astype(np.int8)

with open(out, "wb") as f:
    f.write(b"LKTA" + struct.pack("<III", 2, len(rows), emb.shape[1]))
    for r in rows:
        parent = index[r["parent"]] if r["parent"] else -1
        f.write(struct.pack("<iBf", parent, RANK[r["rank"]], float(r["prior"])))
        for name in (r["deutsch"], r["english"]):
            b = name.encode("utf8")
            f.write(struct.pack("<H", len(b)) + b)
    f.write(scale.astype("<f4").tobytes())
    f.write(q.tobytes())
# Genauigkeit der 8-Bit-Werte
back = q.astype(np.float32) * scale[:, None]
print(f"{len(rows)} Knoten, {emb.shape[1]} Werte, Kosinus nach Rundung min {(back * emb).sum(1).min():.4f} -> {out} ({os.path.getsize(out) / 1e6:.1f} MB)")
