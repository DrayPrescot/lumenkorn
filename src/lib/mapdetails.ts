// Städte, Berge, Flüsse und Seen (Natural Earth 1:10m, Public Domain) für die Reisekarte.
// Die Daten liegen in mapdetails.json und werden nur bei Bedarf geladen.
// Ränge: kleiner = wichtiger (Städte 0–10, Berge 1–9, Flüsse 0–8, Seen 0–6).

export type BBox = [number, number, number, number];

export interface City {
  lon: number;
  lat: number;
  de: string;
  en: string;
  rank: number;
}
export interface Peak {
  lon: number;
  lat: number;
  name: string;
  /** Höhe in Metern. */
  elevation: number;
  rank: number;
}
export interface River {
  rank: number;
  lines: [number, number][][];
}
export interface Lake {
  rank: number;
  /** Polygone -> Ringe (erster = Außenring, Rest = Inseln) -> [lon, lat]. */
  polygons: [number, number][][][];
  bbox: BBox;
}
export interface MapDetails {
  cities: City[];
  peaks: Peak[];
  rivers: (River & { bbox: BBox })[];
  lakes: Lake[];
}

interface Roh {
  cities: [number, number, string, string, number][];
  peaks: [number, number, string, number, number][];
  rivers: { r: number; l: number[][] }[];
  lakes: { r: number; p: number[][][] }[];
}

let geladen: Promise<MapDetails> | undefined;

/** Delta-kodierte Liste [x0, y0, dx1, dy1, …] in 1/1000 Grad -> Punkte. */
function punkte(flach: number[], box: BBox): [number, number][] {
  const aus: [number, number][] = [];
  let x = 0;
  let y = 0;
  for (let i = 0; i + 1 < flach.length; i += 2) {
    x += flach[i];
    y += flach[i + 1];
    const lon = x / 1000;
    const lat = y / 1000;
    if (lon < box[0]) box[0] = lon;
    if (lat < box[1]) box[1] = lat;
    if (lon > box[2]) box[2] = lon;
    if (lat > box[3]) box[3] = lat;
    aus.push([lon, lat]);
  }
  return aus;
}

function dekodiere(roh: Roh): MapDetails {
  return {
    cities: roh.cities.map(([lon, lat, de, en, rank]) => ({ lon, lat, de: de || en, en, rank })),
    peaks: roh.peaks.map(([lon, lat, name, elevation, rank]) => ({ lon, lat, name, elevation, rank })),
    rivers: roh.rivers.map((f) => {
      const bbox: BBox = [Infinity, Infinity, -Infinity, -Infinity];
      return { rank: f.r, lines: f.l.map((l) => punkte(l, bbox)), bbox };
    }),
    lakes: roh.lakes.map((s) => {
      const bbox: BBox = [Infinity, Infinity, -Infinity, -Infinity];
      return { rank: s.r, polygons: s.p.map((ringe) => ringe.map((r) => punkte(r, bbox))), bbox };
    }),
  };
}

/** Lädt und dekodiert die Kartendetails einmalig (Ergebnis wird gemerkt). */
export function loadMapDetails(): Promise<MapDetails> {
  geladen ??= import("./mapdetails.json").then((m) => dekodiere(m.default as unknown as Roh));
  return geladen;
}
