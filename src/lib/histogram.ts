// Histogramm eines gerenderten Bildes, im Browser auf einer verkleinerten
// Kopie berechnet – für die Form reicht das völlig.

export interface Bins {
  /** Rot, Grün, Blau, Helligkeit (Rec. 709) je 256 Stufen */
  bins: [Uint32Array, Uint32Array, Uint32Array, Uint32Array];
  /** Anteil der Pixel nahe Schwarz bzw. Weiß (abgeschnitten) */
  clipLow: number;
  clipHigh: number;
}

/** Letzte Anfrage: Histogramm und Kurvenfeld fragen dieselbe URL, einmal rechnen. */
let last: { src: string; promise: Promise<Bins> } | null = null;
/** Ein Zeichenfläche für alle Berechnungen */
let scratch: HTMLCanvasElement | null = null;

/** Lädt das Bild `src` und zählt die Stufen (je URL nur einmal). */
export function loadBins(src: string): Promise<Bins> {
  if (last?.src === src) return last.promise;
  const promise = compute(src);
  const entry = { src, promise };
  last = entry;
  // Fehlschläge (z. B. bereits widerrufene Objekt-URL) nicht zwischenspeichern
  promise.catch(() => {
    if (last === entry) last = null;
  });
  return promise;
}

/** Bild laden – über `<img>`, nicht `fetch`: Die CSP der App erlaubt `blob:`
 *  nur als Bildquelle (`img-src`), nicht für `connect-src`. */
function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const el = new Image();
    el.onload = () => resolve(el);
    el.onerror = () => reject(new Error("Bild nicht lesbar"));
    el.src = src;
  });
}

/** Pixel des Bildes in voller Größe (RGBA) */
async function readPixels(src: string): Promise<{ w: number; h: number; px: Uint8ClampedArray }> {
  const el = await loadImage(src);
  const w = el.naturalWidth;
  const h = el.naturalHeight;
  scratch ??= document.createElement("canvas");
  scratch.width = w;
  scratch.height = h;
  const ctx = scratch.getContext("2d", { willReadFrequently: true })!;
  ctx.drawImage(el, 0, 0);
  return { w, h, px: ctx.getImageData(0, 0, w, h).data };
}

/** Schwellen für „abgeschnitten“ (Prozentangaben unter dem Histogramm) */
const CLIP_HIGH = 254;
const CLIP_LOW = 1;

/** Ganzes Vorschaubild: Auch kleine Spitzlichter zählen, die eine verkleinerte
 *  Kopie wegmitteln würde. */
async function compute(src: string): Promise<Bins> {
  const { px } = await readPixels(src);
  const bins: Bins["bins"] = [new Uint32Array(256), new Uint32Array(256), new Uint32Array(256), new Uint32Array(256)];
  let lo = 0;
  let hi = 0;
  for (let i = 0; i < px.length; i += 4) {
    const r = px[i];
    const g = px[i + 1];
    const b = px[i + 2];
    bins[0][r]++;
    bins[1][g]++;
    bins[2][b]++;
    bins[3][Math.min(255, (0.2126 * r + 0.7152 * g + 0.0722 * b + 0.5) | 0)]++;
    const m = r > g ? (r > b ? r : b) : g > b ? g : b;
    if (m >= CLIP_HIGH) hi++;
    else if (m <= CLIP_LOW) lo++;
  }
  const n = px.length / 4;
  return { bins, clipLow: lo / n, clipHigh: hi / n };
}

/** Höchster Wert ohne die Ränder (Ausgebranntes drückt die Skala sonst platt). */
export function peakOf(...bins: Uint32Array[]): number {
  let peak = 1;
  for (const b of bins) for (let i = 2; i < 254; i++) peak = Math.max(peak, b[i]);
  return peak;
}
