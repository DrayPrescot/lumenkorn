// Länderumrisse (Natural Earth 1:50m, Public Domain) für die Reisekarte.
// Die Daten liegen in countries.json und werden nur bei Bedarf geladen.

export interface Country {
  iso: string;
  de: string;
  en: string;
  /** [minLon, minLat, maxLon, maxLat] in Grad. */
  bbox: [number, number, number, number];
  /** Polygone -> Ringe (erster = Außenring, Rest = Löcher) -> [lon, lat]. */
  polygons: [number, number][][][];
}

interface Roh {
  iso: string;
  de: string;
  en: string;
  bbox: [number, number, number, number];
  /** Delta-kodierte Ringe: [x0, y0, dx1, dy1, …] in 1/1000 Grad. */
  p: number[][][];
}

let geladen: Promise<Country[]> | undefined;

function dekodiere(roh: Roh[]): Country[] {
  return roh.map((c) => ({
    iso: c.iso,
    de: c.de,
    en: c.en,
    bbox: c.bbox,
    polygons: c.p.map((ringe) =>
      ringe.map((flach) => {
        const punkte: [number, number][] = [];
        let x = 0;
        let y = 0;
        for (let i = 0; i + 1 < flach.length; i += 2) {
          x += flach[i];
          y += flach[i + 1];
          punkte.push([x / 1000, y / 1000]);
        }
        return punkte;
      }),
    ),
  }));
}

/** Lädt und dekodiert die Länder einmalig (Ergebnis wird gemerkt). */
export function loadCountries(): Promise<Country[]> {
  geladen ??= import("./countries.json").then((m) => dekodiere(m.default as Roh[]));
  return geladen;
}

function imRing(ring: [number, number][], lon: number, lat: number): boolean {
  let innen = false;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const [xi, yi] = ring[i];
    const [xj, yj] = ring[j];
    if (yi > lat !== yj > lat && lon < ((xj - xi) * (lat - yi)) / (yj - yi) + xi) {
      innen = !innen;
    }
  }
  return innen;
}

/** Land am Punkt (BBox-Vorfilter, dann Even-Odd mit Löchern). */
export function countryAt(countries: Country[], lon: number, lat: number): Country | undefined {
  for (const c of countries) {
    const [x0, y0, x1, y1] = c.bbox;
    if (lon < x0 || lon > x1 || lat < y0 || lat > y1) continue;
    for (const poly of c.polygons) {
      let n = 0;
      for (const ring of poly) if (imRing(ring, lon, lat)) n++;
      if (n % 2 === 1) return c;
    }
  }
  return undefined;
}
