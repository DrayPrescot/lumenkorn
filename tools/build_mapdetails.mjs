// Erzeugt src/lib/mapdetails.json (Städte, Berge, Flüsse, Seen) für die Reisekarte.
//
// Quelle:  Natural Earth 1:10m, https://github.com/nvkelso/natural-earth-vector/tree/master/geojson
//          ne_10m_populated_places (bzw. ne_10m_populated_places_simple, dann ohne deutsche Namen),
//          ne_10m_geography_regions_elevation_points, ne_10m_rivers_lake_centerlines, ne_10m_lakes
// Lizenz:  Public Domain (Natural Earth, https://www.naturalearthdata.com)
// Aufruf:  node tools/build_mapdetails.mjs <ordner mit den vier .geojson-Dateien>
//
// Format (Koordinaten in Grad bzw. 1/1000 Grad, Ränge: kleiner = wichtiger):
//   cities: [lon, lat, deName ("" = wie en), enName, rang]   rang 0..10 (scalerank)
//   peaks:  [lon, lat, name, höhe in m, rang]                rang 1..9 (scalerank)
//   rivers: [{ r: rang 0..8, l: [Linie, …] }]  Linie = [x0,y0,dx1,dy1,…] in 1/1000°, delta-kodiert
//   lakes:  [{ r: rang 0..6, p: [Polygon, …] }] Polygon = [Ring, …] (erster = Außenring), Ring wie Linie
import { readFileSync, writeFileSync, statSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ordner = process.argv[2];
if (!ordner) {
  console.error("Aufruf: node tools/build_mapdetails.mjs <ordner>");
  process.exit(1);
}
const ziel = join(dirname(fileURLToPath(import.meta.url)), "..", "src", "lib", "mapdetails.json");

const MAX_FLUSS_RANG = 8;
const MAX_SEE_RANG = 6;
const TOL_LINIE = 0.008; // Grad, Douglas-Peucker
const TOL_SEE = 0.01;

const lade = (name) => JSON.parse(readFileSync(join(ordner, name), "utf8")).features;
const gross = (p, k) => p[k] ?? p[k.toUpperCase()];

// Nur lateinische Schrift (mit Akzenten), Ziffern und übliche Satzzeichen.
const LATEIN = /^[\p{Script=Latin}\p{M}\d\s.,'’()\/&-]+$/u;
function latein(...kandidaten) {
  for (const k of kandidaten) {
    if (typeof k === "string" && k.trim() && LATEIN.test(k)) return k.trim();
  }
  return null;
}

function vereinfache(pkt, tol) {
  const n = pkt.length;
  const behalten = new Uint8Array(n);
  behalten[0] = behalten[n - 1] = 1;
  const stapel = [[0, n - 1]];
  while (stapel.length) {
    const [a, b] = stapel.pop();
    let maxD = 0;
    let idx = -1;
    const [ax, ay] = pkt[a];
    const [bx, by] = pkt[b];
    const dx = bx - ax;
    const dy = by - ay;
    const len2 = dx * dx + dy * dy;
    for (let i = a + 1; i < b; i++) {
      const [px, py] = pkt[i];
      const d =
        len2 === 0
          ? Math.hypot(px - ax, py - ay)
          : Math.abs(dy * px - dx * py + bx * ay - by * ax) / Math.sqrt(len2);
      if (d > maxD) {
        maxD = d;
        idx = i;
      }
    }
    if (maxD > tol && idx > 0) {
      behalten[idx] = 1;
      stapel.push([a, idx], [idx, b]);
    }
  }
  return pkt.filter((_, i) => behalten[i]);
}

// Quantisieren, vereinfachen, delta-kodieren. geschlossen: Ring (mind. 4 Punkte).
function kodiere(punkte, tol, geschlossen) {
  let q = [];
  for (const [lon, lat] of punkte) {
    const p = [Math.round(lon * 1000), Math.round(lat * 1000)];
    const l = q[q.length - 1];
    if (!l || l[0] !== p[0] || l[1] !== p[1]) q.push(p);
  }
  if (geschlossen && q.length > 1) {
    const a = q[0];
    const z = q[q.length - 1];
    if (a[0] !== z[0] || a[1] !== z[1]) q.push([a[0], a[1]]);
  }
  if (q.length < (geschlossen ? 4 : 2)) return null;
  q = vereinfache(q, tol * 1000);
  if (q.length < (geschlossen ? 4 : 2)) return null;
  const flach = [q[0][0], q[0][1]];
  for (let i = 1; i < q.length; i++) flach.push(q[i][0] - q[i - 1][0], q[i][1] - q[i - 1][1]);
  return flach;
}

const r3 = (x) => Math.round(x * 1000) / 1000;
const groesse = (o) => (JSON.stringify(o).length / 1024).toFixed(0) + " KiB";

// Städte
const ortDatei = existsSync(join(ordner, "ne_10m_populated_places.geojson"))
  ? "ne_10m_populated_places.geojson"
  : "ne_10m_populated_places_simple.geojson";
const cities = [];
for (const f of lade(ortDatei)) {
  const e = f.properties;
  const g = f.geometry;
  if (!g || g.type !== "Point") continue;
  const en = latein(gross(e, "name_en"), gross(e, "name"), gross(e, "nameascii"));
  if (!en) continue;
  const de = latein(gross(e, "name_de")) ?? en;
  const rang = gross(e, "scalerank");
  cities.push([r3(g.coordinates[0]), r3(g.coordinates[1]), de === en ? "" : de, en, rang ?? 10]);
}

// Berge
const peaks = [];
for (const f of lade("ne_10m_geography_regions_elevation_points.geojson")) {
  const e = f.properties;
  const g = f.geometry;
  if (!g || g.type !== "Point" || e.featurecla !== "mountain") continue;
  const name = latein(e.name_en, e.name);
  if (!name || !Number.isFinite(e.elevation) || e.elevation <= 0) continue;
  peaks.push([r3(g.coordinates[0]), r3(g.coordinates[1]), name, Math.round(e.elevation), e.scalerank ?? 9]);
}

// Flüsse
const rivers = [];
for (const f of lade("ne_10m_rivers_lake_centerlines.geojson")) {
  const g = f.geometry;
  const r = f.properties.scalerank ?? 9;
  if (!g || r > MAX_FLUSS_RANG) continue;
  const linien = g.type === "LineString" ? [g.coordinates] : g.type === "MultiLineString" ? g.coordinates : [];
  const l = linien.map((x) => kodiere(x, TOL_LINIE, false)).filter(Boolean);
  if (l.length) rivers.push({ r, l });
}

// Seen
const lakes = [];
for (const f of lade("ne_10m_lakes.geojson")) {
  const g = f.geometry;
  const r = f.properties.scalerank ?? 9;
  if (!g || r > MAX_SEE_RANG) continue;
  const polys = g.type === "Polygon" ? [g.coordinates] : g.type === "MultiPolygon" ? g.coordinates : [];
  const p = [];
  for (const poly of polys) {
    const aussen = kodiere(poly[0], TOL_SEE, true);
    if (!aussen) continue;
    p.push([aussen, ...poly.slice(1).map((h) => kodiere(h, TOL_SEE, true)).filter(Boolean)]);
  }
  if (p.length) lakes.push({ r, p });
}

console.log(`cities ${cities.length} (${groesse(cities)}), peaks ${peaks.length} (${groesse(peaks)})`);
console.log(`rivers ${rivers.length} (${groesse(rivers)}), lakes ${lakes.length} (${groesse(lakes)})`);
writeFileSync(ziel, JSON.stringify({ cities, peaks, rivers, lakes }));
console.log(`${(statSync(ziel).size / 1024).toFixed(0)} KiB -> ${ziel}`);
