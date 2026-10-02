#!/usr/bin/env python3
"""Erzeugt Testdaten für die KI-Tests von lumenkorn-ai.

    pip install onnx numpy ftfy regex
    python3 tools/make_ai_test_model.py <zielordner>

Benötigt `bpe_simple_vocab_16e6.txt.gz` und `simple_tokenizer.py` aus
https://github.com/openai/CLIP/tree/main/clip im aktuellen Ordner.

Ergebnis im Zielordner:
  vocab.json, merges.txt   CLIP-Tokenizer im Hugging-Face-Format
  token_cases.json         Referenz-Token-IDs des OpenAI-Tokenizers
  vision.onnx, text.onnx   Winzige Modelle mit derselben Schnittstelle wie CLIP:
                           Das Bildmodell bildet die mittlere Farbe ab, das
                           Textmodell kennt nur „red/green/blue“ – genug, um
                           Laufzeit, Tokenizer, Ranking und Verschlagwortung
                           ohne das echte, große Modell zu testen.
"""

import gzip
import json
import os
import sys

import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper

sys.path.insert(0, os.getcwd())
from simple_tokenizer import SimpleTokenizer, bytes_to_unicode  # noqa: E402

out = sys.argv[1]
os.makedirs(out, exist_ok=True)

# --- Tokenizer im HF-Format ------------------------------------------------
merges = gzip.open("bpe_simple_vocab_16e6.txt.gz").read().decode("utf-8").split("\n")
merges = [tuple(m.split()) for m in merges[1 : 49152 - 256 - 2 + 1]]
vocab = list(bytes_to_unicode().values())
vocab += [v + "</w>" for v in vocab]
vocab += ["".join(m) for m in merges]
vocab += ["<|startoftext|>", "<|endoftext|>"]
enc = {t: i for i, t in enumerate(vocab)}
json.dump(enc, open(f"{out}/vocab.json", "w"), ensure_ascii=False)
with open(f"{out}/merges.txt", "w") as f:
    f.write("#version: 0.2\n")
    f.writelines(" ".join(m) + "\n" for m in merges)

tok = SimpleTokenizer("bpe_simple_vocab_16e6.txt.gz")
cases = [
    "a photo of a dog",
    "A Photo of a DOG on the beach!",
    "sunset, mountains & lake",
    "Hund am Strand",
    "it's 2024 — über alles",
    "  multiple   spaces\tand\nnewlines ",
    "photography",
    "a black and white photo of a cat's face",
]
json.dump([{"text": c, "ids": tok.encode(c)} for c in cases], open(f"{out}/token_cases.json", "w"), ensure_ascii=False, indent=1)

# --- Modelle ---------------------------------------------------------------
D = 512
W = np.random.default_rng(0).standard_normal((3, D)).astype(np.float32)
mean = np.array([0.48145466, 0.4578275, 0.40821073])
std = np.array([0.26862954, 0.26130258, 0.27577711])


def embed(rgb):
    return (((np.array(rgb) - mean) / std) @ W).astype(np.float32)


def save(graph, name):
    m = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 18)])
    m.ir_version = 9
    onnx.checker.check_model(m)
    onnx.save(m, f"{out}/{name}")


i64 = lambda v, n: numpy_helper.from_array(np.array(v, dtype=np.int64), n)  # noqa: E731

save(
    helper.make_graph(
        [
            helper.make_node("ReduceMean", ["pixel_values", "axes"], ["m"], keepdims=0),
            helper.make_node("MatMul", ["m", "W"], ["image_embeds"]),
            helper.make_node("Unsqueeze", ["image_embeds", "ax1"], ["last_hidden_state"]),
        ],
        "vision",
        [helper.make_tensor_value_info("pixel_values", TensorProto.FLOAT, ["batch", 3, 224, 224])],
        [
            helper.make_tensor_value_info("last_hidden_state", TensorProto.FLOAT, ["batch", 1, D]),
            helper.make_tensor_value_info("image_embeds", TensorProto.FLOAT, ["batch", D]),
        ],
        [i64([2, 3], "axes"), numpy_helper.from_array(W, "W"), i64([1], "ax1")],
    ),
    "vision.onnx",
)

E = np.zeros((len(enc), D), dtype=np.float32)
for word, rgb in [("red", (1, 0, 0)), ("green", (0, 1, 0)), ("blue", (0, 0, 1))]:
    E[enc[word + "</w>"]] = embed(rgb)

save(
    helper.make_graph(
        [
            helper.make_node("Gather", ["E", "input_ids"], ["g"], axis=0),
            helper.make_node("Cast", ["attention_mask"], ["mf"], to=TensorProto.FLOAT),
            helper.make_node("Unsqueeze", ["mf", "ax2"], ["mu"]),
            helper.make_node("Mul", ["g", "mu"], ["gm"]),
            helper.make_node("ReduceSum", ["gm", "ax1"], ["s"], keepdims=0),
            helper.make_node("ReduceSum", ["mu", "ax1"], ["n"], keepdims=0),
            helper.make_node("Div", ["s", "n"], ["text_embeds"]),
        ],
        "text",
        [
            helper.make_tensor_value_info("input_ids", TensorProto.INT64, ["batch", "seq"]),
            helper.make_tensor_value_info("attention_mask", TensorProto.INT64, ["batch", "seq"]),
        ],
        [helper.make_tensor_value_info("text_embeds", TensorProto.FLOAT, ["batch", D])],
        [numpy_helper.from_array(E, "E"), i64([2], "ax2"), i64([1], "ax1")],
    ),
    "text.onnx",
)
print(f"Testmodell in {out}")
