// Histogramm eines gerenderten Bildes, im Browser auf einer verkleinerten
// Kopie berechnet – für die Form reicht das völlig.

export interface Bins {
  /** Rot, Grün, Blau, Helligkeit (Rec. 709) je 256 Stufen */
  bins: [Uint32Array, Uint32Array, Uint32Array, Uint32Array];
  /** Anteil der Pixel nahe Schwarz bzw. Weiß (abgeschnitten) */
  clipLow: number;
  clipHigh: number;
}

/** Lädt das Bild `src` und zählt die Stufen. */
export function loadBins(src: string): Promise<Bins> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      const scale = Math.min(1, 320 / Math.max(img.width, img.height));
      const w = Math.max(1, Math.round(img.width * scale));
      const h = Math.max(1, Math.round(img.height * scale));
      const off = document.createElement("canvas");
      off.width = w;
      off.height = h;
      const ctx = off.getContext("2d", { willReadFrequently: true })!;
      ctx.drawImage(img, 0, 0, w, h);
      const px = ctx.getImageData(0, 0, w, h).data;
      const bins: Bins["bins"] = [new Uint32Array(256), new Uint32Array(256), new Uint32Array(256), new Uint32Array(256)];
      let lo = 0;
      let hi = 0;
      for (let i = 0; i < px.length; i += 4) {
        const [r, g, b] = [px[i], px[i + 1], px[i + 2]];
        bins[0][r]++;
        bins[1][g]++;
        bins[2][b]++;
        bins[3][Math.min(255, Math.round(0.2126 * r + 0.7152 * g + 0.0722 * b))]++;
        const m = Math.max(r, g, b);
        if (m >= 254) hi++;
        if (m <= 1) lo++;
      }
      const n = px.length / 4;
      resolve({ bins, clipLow: lo / n, clipHigh: hi / n });
    };
    img.onerror = () => reject(new Error("Bild nicht lesbar"));
    img.src = src;
  });
}

/** Höchster Wert ohne die Ränder (Ausgebranntes drückt die Skala sonst platt). */
export function peakOf(...bins: Uint32Array[]): number {
  let peak = 1;
  for (const b of bins) for (let i = 2; i < 254; i++) peak = Math.max(peak, b[i]);
  return peak;
}
