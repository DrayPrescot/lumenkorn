# Arten für die Tierarten-Liste bei iNaturalist sammeln (nur Auswahl nach
# Beobachtungszahlen und Einordnung; Namen kommen später aus Wikidata).
# Zwischenspeicher je Anfrage unter <out>/cache, Ergebnis <out>/species.json.
#
#   python tools/bioclip/fetch_inat.py <datenordner>
#
# Danach fetch_names.py, build_taxonomy.py und text_embeddings.py.
import json, os, sys, time, urllib.parse, urllib.request

out = sys.argv[1]
cache = os.path.join(out, "cache")
os.makedirs(cache, exist_ok=True)
UA = {"User-Agent": "Lumenkorn taxonomy builder (github.com/DrayPrescot/lumenkorn)"}

def get(url):
    key = urllib.parse.quote(url, safe="")[:200]
    path = os.path.join(cache, key + ".json")
    if os.path.exists(path):
        return json.load(open(path, encoding="utf8"))
    for attempt in range(5):
        time.sleep(1.1)
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=60) as r:
                data = json.loads(r.read())
            json.dump(data, open(path, "w", encoding="utf8"))
            return data
        except Exception as e:
            print("  Fehler, erneut:", e, flush=True)
            time.sleep(10 * (attempt + 1))
    raise SystemExit("aufgegeben: " + url)

def species_counts(label, limit, **params):
    params.update(verifiable="true", hrank="species", lrank="species", per_page=500)
    got = []
    page = 1
    while len(got) < limit:
        q = urllib.parse.urlencode({**params, "page": page})
        d = get(f"https://api.inaturalist.org/v1/observations/species_counts?{q}")
        res = d.get("results", [])
        if not res:
            break
        for r in res:
            t = r["taxon"]
            got.append({"id": t["id"], "name": t["name"], "count": r["count"], "ancestors": t.get("ancestor_ids", []), "iconic": t.get("iconic_taxon_name"), "source": label})
        page += 1
    print(f"{label}: {len(got[:limit])}", flush=True)
    return got[:limit]

EUROPE, KENYA, TANZANIA, SOUTH_AFRICA = 97391, 7042, 7144, 6986
ANIMALIA, AVES, MAMMALIA, REPTILIA, PRIMATES = 1, 3, 40151, 26036, 43367

lists = [
    species_counts("Europa Tiere", 7000, place_id=EUROPE, taxon_id=ANIMALIA),
    species_counts("Europa Vögel", 900, place_id=EUROPE, taxon_id=AVES),
    species_counts("Vögel weltweit", 1800, taxon_id=AVES),
    species_counts("Primaten", 600, taxon_id=PRIMATES),
]
for place, name in [(KENYA, "Kenia"), (TANZANIA, "Tansania"), (SOUTH_AFRICA, "Südafrika")]:
    lists.append(species_counts(f"{name} Säugetiere", 250, place_id=place, taxon_id=MAMMALIA))
    lists.append(species_counts(f"{name} Vögel", 300, place_id=place, taxon_id=AVES))
    lists.append(species_counts(f"{name} Reptilien", 60, place_id=place, taxon_id=REPTILIA))

species = {}
for lst in lists:
    for s in lst:
        e = species.setdefault(s["id"], {**s, "sources": [], "counts": {}})
        e["sources"].append(s["source"])
        e["counts"][s["source"]] = s["count"]
        e["count"] = max(e["count"], s["count"])
json.dump(list(species.values()), open(os.path.join(out, "species.json"), "w", encoding="utf8"), ensure_ascii=False)
print("Arten gesamt (ohne Doppelte):", len(species))
