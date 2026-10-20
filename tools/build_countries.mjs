// Erzeugt src/lib/countries.json aus den Natural-Earth-Ländergrenzen.
//
// Quelle:  Natural Earth, „Admin 0 – Countries“, Maßstab 1:50m
//          https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/ne_50m_admin_0_countries.geojson
// Lizenz:  Public Domain (Natural Earth, https://www.naturalearthdata.com)
// Aufruf:  node tools/build_countries.mjs <pfad zur ne_50m_admin_0_countries.geojson>
//
// Format je Land: { iso, de, en, bbox:[minLon,minLat,maxLon,maxLat],
//   p: Polygone -> Ringe (erster = Außenring, Rest = Löcher) -> flache Liste
//   [x0,y0,dx1,dy1,…] in 1/1000 Grad, delta-kodiert }.
import { readFileSync, writeFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const eingabe = process.argv[2];
if (!eingabe) {
  console.error("Aufruf: node tools/build_countries.mjs <geojson>");
  process.exit(1);
}
const ziel = join(dirname(fileURLToPath(import.meta.url)), "..", "src", "lib", "countries.json");
const TOLERANZ = 0.005; // Grad, Douglas-Peucker

// Douglas-Peucker auf einer offenen Punktfolge (iterativ).
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
      let d;
      if (len2 === 0) d = Math.hypot(px - ax, py - ay);
      else d = Math.abs(dy * px - dx * py + bx * ay - by * ax) / Math.sqrt(len2);
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

function ring(r) {
  // Quantisieren, gleiche Nachbarpunkte entfernen
  let q = [];
  for (const [lon, lat] of r) {
    const p = [Math.round(lon * 1000), Math.round(lat * 1000)];
    const l = q[q.length - 1];
    if (!l || l[0] !== p[0] || l[1] !== p[1]) q.push(p);
  }
  // Ring geschlossen halten, vereinfachen
  if (q.length > 1) {
    const a = q[0];
    const z = q[q.length - 1];
    if (a[0] !== z[0] || a[1] !== z[1]) q.push([a[0], a[1]]);
  }
  if (q.length < 4) return null;
  q = vereinfache(q, TOLERANZ * 1000);
  if (q.length < 4) return null;
  const flach = [q[0][0], q[0][1]];
  for (let i = 1; i < q.length; i++) {
    flach.push(q[i][0] - q[i - 1][0], q[i][1] - q[i - 1][1]);
  }
  return flach;
}

const daten = JSON.parse(readFileSync(eingabe, "utf8"));
const laender = [];
for (const f of daten.features) {
  const e = f.properties ?? {};
  let iso = e.ISO_A2;
  if (!iso || iso === "-99") iso = e.ISO_A2_EH;
  if (!iso || iso === "-99") iso = e.ADM0_A3;
  const g = f.geometry;
  if (!g) continue;
  const polys = g.type === "Polygon" ? [g.coordinates] : g.type === "MultiPolygon" ? g.coordinates : [];
  const p = [];
  let minLon = 1e9, minLat = 1e9, maxLon = -1e9, maxLat = -1e9;
  for (const poly of polys) {
    const ringe = [];
    poly.forEach((r, i) => {
      const k = ring(r);
      if (!k) return;
      ringe.push(k);
      if (i === 0) {
        for (const [lon, lat] of r) {
          minLon = Math.min(minLon, lon);
          maxLon = Math.max(maxLon, lon);
          minLat = Math.min(minLat, lat);
          maxLat = Math.max(maxLat, lat);
        }
      }
    });
    // Ein Polygon ohne gültigen Außenring ist wertlos (Löcher allein auch)
    if (ringe.length && poly[0] && ring(poly[0])) p.push(ringe);
  }
  if (!p.length) continue;
  const r3 = (x) => Math.round(x * 1000) / 1000;
  laender.push({
    iso,
    de: e.NAME_DE || e.NAME_EN || e.NAME,
    en: e.NAME_EN || e.NAME,
    bbox: [r3(minLon), r3(minLat), r3(maxLon), r3(maxLat)],
    p,
  });
}
writeFileSync(ziel, JSON.stringify(laender));
console.log(`${laender.length} Länder, ${(statSync(ziel).size / 1024).toFixed(0)} KiB -> ${ziel}`);
