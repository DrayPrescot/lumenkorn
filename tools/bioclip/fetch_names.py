# Einordnung (Vorfahren) von iNaturalist und deutsche/englische Namen von
# Wikidata (CC0) für alle Arten und ihre Klassen, Ordnungen und Familien.
# Ergebnis: <out>/ancestors.json, <out>/names.json
#
#   python tools/bioclip/fetch_names.py <datenordner von fetch_inat.py>
import hashlib, json, os, sys, time, urllib.parse, urllib.request

out = sys.argv[1]
cache = os.path.join(out, "cache")
UA = {"User-Agent": "Lumenkorn taxonomy builder (github.com/DrayPrescot/lumenkorn)"}

def fetch(url, data=None, headers=None, wait=1.1):
    key = urllib.parse.quote(url + (data or ""), safe="")
    key = key[:100] + hashlib.sha1(key.encode()).hexdigest()[:16]
    path = os.path.join(cache, key + ".json")
    if os.path.exists(path):
        return json.load(open(path, encoding="utf8"))
    for attempt in range(6):
        time.sleep(wait)
        try:
            req = urllib.request.Request(url, data=data.encode() if data else None, headers={**UA, **(headers or {})})
            with urllib.request.urlopen(req, timeout=120) as r:
                d = json.loads(r.read())
            json.dump(d, open(path, "w", encoding="utf8"))
            return d
        except Exception as e:
            print("  Fehler, erneut:", e, flush=True)
            time.sleep(15 * (attempt + 1))
    raise SystemExit("aufgegeben: " + url)

species = json.load(open(os.path.join(out, "species.json"), encoding="utf8"))

# --- Vorfahren (Klasse, Ordnung, Familie …) ---------------------------------
sp_ids = [s["id"] for s in species]
anc = {}
for i in range(0, len(sp_ids), 30):
    ids = ",".join(map(str, sp_ids[i:i + 30]))
    d = fetch(f"https://api.inaturalist.org/v2/taxa/{ids}?per_page=30&fields=(id:!t,ancestors:(id:!t,name:!t,rank:!t))")
    for t in d["results"]:
        for a in t.get("ancestors") or []:
            anc[a["id"]] = {"name": a["name"], "rank": a["rank"]}
    if i % 900 == 0:
        print(f"Vorfahren {i}/{len(sp_ids)}", flush=True)
json.dump(anc, open(os.path.join(out, "ancestors.json"), "w", encoding="utf8"), ensure_ascii=False)
print("Vorfahren:", len(anc), flush=True)

# --- Namen aus Wikidata über die iNaturalist-Kennung (P3151) ----------------
wanted = [str(s["id"]) for s in species] + [str(k) for k, v in anc.items() if v["rank"] in ("class", "order", "family", "subfamily", "superfamily", "suborder", "infraorder")]
names = {}
# Deutscher Name: Trivialname (P1843), sonst Titel des Wikipedia-Artikels –
# die Bezeichnung ist bei Taxa oft nur der wissenschaftliche Name
FIELDS = """(GROUP_CONCAT(DISTINCT ?cde; separator="|") AS ?cdes) (GROUP_CONCAT(DISTINCT ?cen; separator="|") AS ?cens)
  (SAMPLE(?lde) AS ?ld) (SAMPLE(?len) AS ?le) (SAMPLE(?wde) AS ?wd) (SAMPLE(?wen) AS ?we)
  (GROUP_CONCAT(DISTINCT ?ade; separator="|") AS ?ades)"""
PARTS = """
  OPTIONAL { ?item wdt:P1843 ?cde FILTER(LANG(?cde) = "de") }
  OPTIONAL { ?item wdt:P1843 ?cen FILTER(LANG(?cen) = "en") }
  OPTIONAL { ?item rdfs:label ?lde FILTER(LANG(?lde) = "de") }
  OPTIONAL { ?item rdfs:label ?len FILTER(LANG(?len) = "en") }
  OPTIONAL { ?item skos:altLabel ?ade FILTER(LANG(?ade) = "de") }
  OPTIONAL { ?a1 schema:about ?item; schema:isPartOf <https://de.wikipedia.org/>; schema:name ?wde }
  OPTIONAL { ?a2 schema:about ?item; schema:isPartOf <https://en.wikipedia.org/>; schema:name ?wen }"""
BY_INAT = "SELECT ?inat ?sci " + FIELDS + " WHERE { VALUES ?inat { %s } ?item wdt:P3151 ?inat . OPTIONAL { ?item wdt:P225 ?sci }" + PARTS + "} GROUP BY ?inat ?sci"
BY_SCI = "SELECT ?sci " + FIELDS + " WHERE { VALUES ?sci { %s } ?item wdt:P225 ?sci ; wdt:P105 wd:Q7432 ." + PARTS + "} GROUP BY ?sci"


def query(q, vals):
    d = fetch("https://query.wikidata.org/sparql", data=urllib.parse.urlencode({"query": q % vals, "format": "json"}),
              headers={"Content-Type": "application/x-www-form-urlencoded", "Accept": "application/sparql-results+json"}, wait=2)
    for b in d["results"]["bindings"]:
        v = lambda k: b.get(k, {}).get("value", "")
        yield v, {"sci": v("sci"), "de_common": [x for x in v("cdes").split("|") if x], "en_common": [x for x in v("cens").split("|") if x],
                  "de_label": v("ld"), "en_label": v("le"), "de_wiki": v("wd"), "en_wiki": v("we"), "de_alias": [x for x in v("ades").split("|") if x]}


for i in range(0, len(wanted), 200):
    for v, rec in query(BY_INAT, " ".join(f'"{w}"' for w in wanted[i:i + 200])):
        names[v("inat")] = rec
    print(f"Namen {i}/{len(wanted)}", flush=True)
# Arten ohne iNaturalist-Kennung in Wikidata: über den wissenschaftlichen Namen
missing = [s for s in species if str(s["id"]) not in names]
by_name = {s["name"]: str(s["id"]) for s in missing}
for i in range(0, len(missing), 200):
    vals = " ".join(json.dumps(s["name"]) for s in missing[i:i + 200])
    for v, rec in query(BY_SCI, vals):
        if v("sci") in by_name:
            names[by_name[v("sci")]] = rec
print("über den Namen gefunden:", sum(1 for s in missing if str(s["id"]) in names), "von", len(missing), flush=True)
json.dump(names, open(os.path.join(out, "names.json"), "w", encoding="utf8"), ensure_ascii=False)
print("Namen:", len(names))
