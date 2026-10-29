# Veröffentlicht ein lokal gebautes Lumenkorn-Release auf GitHub.
# Aufruf: python tools/publish_release.py . <Version>  (nach tools/release_local.mjs)
# Der Zugang kommt aus `git credential fill` und wird nie ausgegeben.
import json, subprocess, sys, time, urllib.request, urllib.error, pathlib

repo_dir = pathlib.Path(sys.argv[1])
version = sys.argv[2]
tag = f"v{version}"
REPO = "DrayPrescot/lumenkorn"
API = f"https://api.github.com/repos/{REPO}"

cred = subprocess.run(["git", "credential", "fill"], input="protocol=https\nhost=github.com\n\n",
                      capture_output=True, text=True, cwd=repo_dir, check=True).stdout
token = next(l.split("=", 1)[1] for l in cred.splitlines() if l.startswith("password="))

def call(method, url, data=None, ctype="application/json"):
    body = json.dumps(data).encode() if isinstance(data, (dict, list)) else data
    req = urllib.request.Request(url, data=body, method=method, headers={
        "Authorization": f"Bearer {token}", "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28", "Content-Type": ctype, "User-Agent": "lumenkorn-release"})
    try:
        with urllib.request.urlopen(req) as r:
            raw = r.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        print(f"HTTP {e.code} bei {method} {url.split('?')[0]}: {e.read().decode()[:400]}")
        raise

# Release-Text wie in release.yml, dazu 1.0.1 (nie einzeln veröffentlicht)
changelog = (repo_dir / "CHANGELOG.md").read_text(encoding="utf8").replace("\r\n", "\n")
def section(v):
    start = changelog.index(f"## [{v}]")
    body = changelog[changelog.index("\n", start) + 1:]
    end = body.find("\n## [")
    return (body[:end] if end >= 0 else body).strip()
notes = section(version)
extra = ""
if version == "1.0.2":
    extra = ("\n\n## Also included: 1.0.1\n\n1.0.1 was not published separately; its changes are part of this release.\n\n"
             + section("1.0.1"))
body = (notes + extra + "\n\nSee [CHANGELOG.md](https://github.com/DrayPrescot/lumenkorn/blob/main/CHANGELOG.md) for details.\n\n"
        "Lumenkorn statically links rawler (LGPL-2.1). To relink with a modified rawler,\n"
        "build from source as described in the README (section \"License\").\n")

sha = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, cwd=repo_dir, check=True).stdout.strip()
msi_dir = repo_dir / "target" / "release" / "bundle" / "msi"
files = sorted(msi_dir.glob(f"*_{version}_*.msi")) + [msi_dir / "latest.json"]
assert len(files) == 3 and all(f.exists() for f in files), files
latest = json.loads((msi_dir / "latest.json").read_text(encoding="utf8"))
assert latest["version"] == version, latest["version"]

# 1. Entwurf (noch kein Tag → noch kein Workflow)
rel = call("POST", f"{API}/releases", {"tag_name": tag, "target_commitish": sha, "name": f"Lumenkorn {tag}",
                                       "body": body, "draft": True, "prerelease": False})
print(f"Entwurf angelegt (id {rel['id']})")
# 2. Dateien
upload = rel["upload_url"].split("{")[0]
for f in files:
    ctype = "application/json" if f.suffix == ".json" else "application/octet-stream"
    a = call("POST", f"{upload}?name={f.name}", f.read_bytes(), ctype)
    print(f"  hochgeladen: {a['name']} ({a['size']} Bytes)")
# 3. Veröffentlichen (legt den Tag an)
started = time.time()
rel = call("PATCH", f"{API}/releases/{rel['id']}", {"draft": False, "make_latest": "true"})
print(f"Veröffentlicht: {rel['html_url']}")
# 4. Von release.yml gestarteten Lauf abbrechen. Ein wartender Lauf (queued)
#    lässt sich nicht abbrechen (409) – dann warten, bis er startet.
def try_call(method, url):
    try:
        call(method, url)
        return True
    except urllib.error.HTTPError:
        return False

cancelled = False
for _ in range(240):
    time.sleep(5)
    runs = call("GET", f"{API}/actions/runs?event=push&per_page=10")["workflow_runs"]
    mine = [r for r in runs if r["head_branch"] == tag and r["name"] == "Release"]
    if not mine:
        continue
    r = mine[0]
    st = call("GET", f"{API}/actions/runs/{r['id']}")
    if st["status"] == "completed":
        print(f"Actions-Lauf {r['id']}: {st['status']} / {st['conclusion']} (nach {time.time() - started:.0f} s)")
        cancelled = True
        break
    if st["status"] == "queued":
        continue
    if not try_call("POST", f"{API}/actions/runs/{r['id']}/cancel"):
        try_call("POST", f"{API}/actions/runs/{r['id']}/force-cancel")
if not cancelled:
    print("Release-Workflow nicht abgebrochen – bitte in Actions nachsehen.")
