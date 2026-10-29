# Baut die große Tierarten-Liste für BioCLIP 2 (tools/bioclip/tierarten.tsv).
#
# Auswahl: die meistbeobachteten Arten bei iNaturalist – Tiere in Europa, Vögel
# weltweit, alle Primaten, Säugetiere/Vögel/Reptilien aus Kenia, Tansania und
# Südafrika (Abruf: fetch_inat.py). Einordnung (Klasse, Ordnung, Familie) von
# iNaturalist, deutsche und englische Namen aus Wikidata (CC0, fetch_names.py).
# Arten ohne deutschen bzw. englischen Namen heißen wissenschaftlich.
# Die handgepflegten Einträge aus taxa.tsv haben Vorrang (Namen) und ergänzen
# fehlende Arten (z. B. Haustiere, die iNaturalist als „nicht wild“ ausblendet).
#
#   python tools/bioclip/build_taxonomy.py <datenordner von fetch_*.py>
#
# Spalten: key, parent, rank (class/group/species), deutsch, english,
# scientific, prior, prompt (Text für BioCLIP 2)
#
# prior: Logarithmus der Häufigkeit in der Region (Beobachtungen relativ zur
# zehnthäufigsten Art der jeweiligen Auswahl, höchstens 1). Ähnliche Arten
# aus anderen Erdteilen (Kanadareiher statt Graureiher) sind auf Fotos aus
# Europa unwahrscheinlich; Vögel weltweit zählen daher nur schwach.
import csv
import json
import math
import os
import re
import sys

data = sys.argv[1]
HERE = os.path.dirname(os.path.abspath(__file__))
species = json.load(open(os.path.join(data, "species.json"), encoding="utf8"))
anc = {int(k): v for k, v in json.load(open(os.path.join(data, "ancestors.json"), encoding="utf8")).items()}
names = json.load(open(os.path.join(data, "names.json"), encoding="utf8"))
curated = list(csv.DictReader(open(os.path.join(HERE, "taxa.tsv"), encoding="utf8"), delimiter="\t"))
# Je wissenschaftlichem Namen der erste Eintrag (die Art; Rassen wie Pony oder
# Hochlandrind folgen mit demselben Namen und kommen unten dazu)
by_sci = {}
for r in curated:
    if r["scientific"].strip() not in ("", "-"):
        by_sci.setdefault(r["scientific"].strip(), r)
# Andere Schreibweise bei iNaturalist als in taxa.tsv
SYNONYMS = {"Canis familiaris": "Canis lupus familiaris"}
used = set()  # schon vergebene handgepflegte Einträge

# Lumenkorn-Klassen: Name deutsch/englisch je wissenschaftlicher Klasse
CLASSES = {
    "Mammalia": ("Säugetier", "Mammal"), "Aves": ("Vogel", "Bird"), "Reptilia": ("Reptil", "Reptile"),
    "Amphibia": ("Amphibie", "Amphibian"), "Actinopterygii": ("Fisch", "Fish"), "Elasmobranchii": ("Fisch", "Fish"),
    "Insecta": ("Insekt", "Insect"), "Arachnida": ("Spinnentier", "Arachnid"), "Gastropoda": ("Schnecke", "Snail"),
    "Bivalvia": ("Muschel", "Bivalve"), "Malacostraca": ("Krebstier", "Crustacean"), "Cephalopoda": ("Kopffüßer", "Cephalopod"),
    "Chilopoda": ("Hundertfüßer", "Centipede"), "Diplopoda": ("Tausendfüßer", "Millipede"), "Asteroidea": ("Seestern", "Starfish"),
    "Echinoidea": ("Seeigel", "Sea urchin"), "Scyphozoa": ("Qualle", "Jellyfish"), "Hydrozoa": ("Nesseltier", "Cnidarian"),
    "Anthozoa": ("Nesseltier", "Cnidarian"), "Clitellata": ("Ringelwurm", "Annelid"), "Polychaeta": ("Ringelwurm", "Annelid"),
    "Entognatha": ("Springschwanz", "Springtail"), "Demospongiae": ("Schwamm", "Sponge"),
    "Chondrichthyes": ("Fisch", "Fish"), "Hexanauplia": ("Krebstier", "Crustacean"), "Ascidiacea": ("Seescheide", "Sea squirt"),
    "Ophiuroidea": ("Schlangenstern", "Brittle star"), "Holothuroidea": ("Seegurke", "Sea cucumber"),
}
# Insekten und Spinnentiere: Gruppe = Ordnung (Familien sind zu fein und
# haben selten deutsche Namen); sonst Familie
ORDER_GROUPS = {"Insecta", "Arachnida", "Entognatha", "Chilopoda", "Diplopoda"}


def clean(s):
    return re.sub(r"\s+", " ", s or "").strip()


LATIN = re.compile(r"^[A-Z][a-z]+( [a-z]+){1,2}$")


def usable(name, sci):
    """Ein Name, der nicht bloß der wissenschaftliche (oder ein Synonym) ist"""
    name = clean(re.sub(r"\s*\(.*?\)", "", name or ""))
    return name if name and name.lower() != sci.lower() and not LATIN.match(name) else ""


def pick(n, lang, sci):
    """Bester Name: Trivialname, sonst Titel des Wikipedia-Artikels, Bezeichnung
    oder (deutsch) Alias – jeweils nur, wenn es nicht der wissenschaftliche ist.
    Deutsch zuerst der Artikeltitel: Er ist meist der gebräuchliche Name
    („Feldsperling“, nicht „Feldspatz“)."""
    if not n:
        return ""
    if lang == "de" and usable(n.get("de_wiki"), sci):
        return usable(n["de_wiki"], sci)
    common = sorted(filter(None, (usable(c, sci) for c in n[f"{lang}_common"])), key=len)
    if common:
        return common[0]
    for name in (n.get(f"{lang}_wiki"), n[f"{lang}_label"]):
        if usable(name, sci):
            return usable(name, sci)
    aliases = sorted(filter(None, (usable(a, sci) for a in n.get(f"{lang}_alias", []))), key=len)
    return aliases[0] if aliases else ""


SOURCE_WEIGHT = {"Vögel weltweit": 0.05}
PRIOR_MIN = 1e-3
tenth = {}
for s_ in species:
    for k, v in s_["counts"].items():
        tenth.setdefault(k, []).append(v)
tenth = {k: sorted(v, reverse=True)[min(9, len(v) - 1)] for k, v in tenth.items()}


def prior(s):
    rel = max(SOURCE_WEIGHT.get(k, 1.0) * min(1.0, v / tenth[k]) for k, v in s["counts"].items())
    return math.log(max(PRIOR_MIN, rel))


# Handgepflegte Arten ohne Zählung (Zoo- und Haustiere, Rassen)
CURATED_PRIOR = math.log(0.1)


def cap(s):
    return s[:1].upper() + s[1:] if s else s


nodes = {}  # key -> dict
order = []


def add(key, parent, rank, de, en, sci, path, pr=None):
    if key in nodes:
        return
    nodes[key] = dict(key=key, parent=parent, rank=rank, deutsch=de, english=en, scientific=sci, path=path, prior=pr)
    order.append(key)


skipped = {"Hybride": 0, "Klasse fehlt": 0}
species.sort(key=lambda s: -s["count"])
for s in species:
    chain = [anc[a] for a in s["ancestors"] if a in anc]
    rank_name = {c["rank"]: c["name"] for c in chain}
    rank_id = {anc[a]["rank"]: a for a in s["ancestors"] if a in anc}
    cls = rank_name.get("class")
    if cls not in CLASSES:
        skipped["Klasse fehlt"] += 1
        continue
    sci = s["name"]
    # Taxonomischer Pfad wie im Training von BioCLIP 2
    path = " ".join(rank_name.get(r, "") for r in ("kingdom", "phylum", "class", "order", "family")).split() + [sci]
    path = " ".join(p for p in path if p)
    cur = by_sci.get(sci) or by_sci.get(SYNONYMS.get(sci, ""))
    if cur:
        used.add(cur["deutsch"])
        # Geprüfter Pfad (so in den Trainingsdaten von BioCLIP 2)
        path = cur["path"].strip()
        sci = cur["scientific"].strip()
    n = names.get(str(s["id"]))
    de = cur["deutsch"] if cur else cap(pick(n, "de", sci))
    en = cur["english"] if cur else cap(pick(n, "en", sci))
    if "×" in sci:
        skipped["Hybride"] += 1
        continue
    # Ohne Trivialnamen (oft kleine Insekten) der wissenschaftliche Name: Fehlte
    # die Art, nähme BioCLIP die ähnlichste benannte – also eine falsche
    de = de or sci
    en = en or sci
    grank = "order" if cls in ORDER_GROUPS else "family"
    gid, gsci = rank_id.get(grank), rank_name.get(grank)
    cde, cen = CLASSES[cls]
    ckey = f"class:{cde}"
    add(ckey, "", "class", cde, cen, cls, " ".join(p for p in (rank_name.get("kingdom"), rank_name.get("phylum"), cls) if p))
    parent = ckey
    if gid and gsci:
        gn = names.get(str(gid))
        gde = cap(pick(gn, "de", gsci)) or gsci
        gen = cap(pick(gn, "en", gsci)) or gsci
        gkey = f"group:{gsci}"
        ranks = ("kingdom", "phylum", "class", "order") + (("family",) if grank == "family" else ())
        add(gkey, ckey, "group", gde, gen, gsci, " ".join(rank_name.get(r, "") for r in ranks).strip())
        parent = gkey
    add(f"species:{s['id']}", parent, "species", de, en, sci, path, prior(s))

# Handgepflegte Arten und Rassen, die fehlen (Haustiere u. a.)
for r in curated:
    sci = r["scientific"].strip()
    p = r["path"].strip().split()
    if len(sci.split()) < 2 or r["deutsch"] in used or len(p) < 6:
        continue
    cls = p[2]
    if cls not in CLASSES:
        continue
    cde, cen = CLASSES[cls]
    ckey = f"class:{cde}"
    if ckey not in nodes:
        add(ckey, "", "class", cde, cen, cls, " ".join(p[:3]))
    grank_i = 3 if cls in ORDER_GROUPS else 4
    gsci = p[grank_i]
    gkey = f"group:{gsci}"
    if gkey not in nodes:
        add(gkey, ckey, "group", gsci, gsci, gsci, " ".join(p[: grank_i + 1]))
    add(f"curated:{r['deutsch']}", gkey, "species", r["deutsch"], r["english"], sci, " ".join(p), CURATED_PRIOR)

# Gleiche Namen in einer Sprache unterscheidbar machen (zwei Arten „Rotmilan“?)
for lang in ("deutsch", "english"):
    seen = {}
    for k in order:
        nm = nodes[k][lang]
        seen.setdefault((nodes[k]["rank"] != "species", nm), []).append(k)
    for (_, nm), keys in seen.items():
        if len(keys) > 1:
            for k in keys[1:]:
                nodes[k][lang] = f"{nm} ({nodes[k]['scientific']})"

# Text für BioCLIP: taxonomischer Pfad; Hausformen/Rassen mit gleichem
# Namen bekommen den englischen Namen dazu
sci_count = {}
for k in order:
    sci_count[nodes[k]["scientific"]] = sci_count.get(nodes[k]["scientific"], 0) + 1


def related(a, b):
    return a == b or a.startswith(b + " ") or b.startswith(a + " ")


species_sci = [nodes[k]["scientific"] for k in order if nodes[k]["rank"] == "species"]
for k in order:
    nd = nodes[k]
    twin = nd["rank"] == "species" and sum(related(nd["scientific"], o) for o in species_sci) > 1
    nd["prompt"] = f"a photo of {nd['path']}" + (f" with common name {nd['english'].lower()}" if twin else "") + "."

# Gruppen und Klassen: Wert ihrer häufigsten Art (Kinder stehen hinter den Eltern)
for k in reversed(order):
    nd = nodes[k]
    if nd["parent"]:
        par = nodes[nd["parent"]]
        par["prior"] = max(par["prior"] if par["prior"] is not None else -1e9, nd["prior"])
for nd in nodes.values():
    nd["prior"] = f"{nd['prior']:.3f}"

out = os.path.join(HERE, "tierarten.tsv")
with open(out, "w", encoding="utf8", newline="") as f:
    w = csv.DictWriter(f, fieldnames=["key", "parent", "rank", "deutsch", "english", "scientific", "prior", "prompt"], delimiter="\t", lineterminator="\n", extrasaction="ignore")
    w.writeheader()
    for k in order:
        w.writerow(nodes[k])
counts = {}
for k in order:
    counts[nodes[k]["rank"]] = counts.get(nodes[k]["rank"], 0) + 1
print("geschrieben:", out, counts, "ausgelassen:", skipped)
