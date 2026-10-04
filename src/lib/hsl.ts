// HSL-Werkzeug: Farbbereiche und Farbtonzuordnung – spiegelt color.rs
// (HSL_CENTERS, hsl_weights, oklab), damit die Pipette genau die Bereiche
// trifft, die das Backend für diese Farbe verändert.

import { t } from "./i18n";

export type HslMode = "hue" | "saturation" | "luminance";

export const HSL_BANDS: { name: string; color: string }[] = [
  { name: t("Rot"), color: "#e0453a" },
  { name: t("Orange"), color: "#ec8a2f" },
  { name: t("Gelb"), color: "#e9cf3a" },
  { name: t("Grün"), color: "#4fb04a" },
  { name: t("Aquamarin"), color: "#37b9b0" },
  { name: t("Blau"), color: "#3d6fe0" },
  { name: t("Lila"), color: "#8a4fd8" },
  { name: t("Magenta"), color: "#d24bb4" },
];

const CENTERS = [29, 60, 110, 142, 195, 264, 294, 328];

/** Anteile der 8 Bereiche an einem Farbton (Grad), Summe 1. */
export function hslWeights(hueDeg: number): number[] {
  const h = ((hueDeg % 360) + 360) % 360;
  const w = new Array(8).fill(0);
  for (let i = 0; i < 8; i++) {
    const a = CENTERS[i];
    let b = CENTERS[(i + 1) % 8];
    if (b <= a) b += 360;
    let x = h;
    if (x < a) x += 360;
    if (x >= a && x < b) {
      const t = (x - a) / (b - a);
      w[i] = 1 - t;
      w[(i + 1) % 8] = t;
      break;
    }
  }
  return w;
}

const toLinear = (v: number) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);

/** Farbton (Grad, Oklch) und Buntheit eines 8-Bit-sRGB-Pixels. */
export function hueOf(r: number, g: number, b: number): { hue: number; chroma: number } {
  const [lr, lg, lb] = [r, g, b].map((v) => toLinear(v / 255));
  const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb);
  const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb);
  const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb);
  const A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const B = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  return { hue: (Math.atan2(B, A) * 180) / Math.PI, chroma: Math.hypot(A, B) };
}

/** Pixel eines (Blob-)Bildes lesen; das dekodierte Bild wird je URL gemerkt. */
let cached: { url: string; data: ImageData } | null = null;
export async function samplePixel(url: string, nx: number, ny: number): Promise<[number, number, number]> {
  if (cached?.url !== url) {
    const img = new Image();
    img.src = url;
    await img.decode();
    const c = document.createElement("canvas");
    c.width = img.naturalWidth;
    c.height = img.naturalHeight;
    const ctx = c.getContext("2d", { willReadFrequently: true })!;
    ctx.drawImage(img, 0, 0);
    cached = { url, data: ctx.getImageData(0, 0, c.width, c.height) };
  }
  const { data, width, height } = cached.data;
  const cx = Math.round(Math.min(1, Math.max(0, nx)) * (width - 1));
  const cy = Math.round(Math.min(1, Math.max(0, ny)) * (height - 1));
  // 5×5 mitteln – einzelne Pixel sind verrauscht
  const sum = [0, 0, 0];
  let n = 0;
  for (let y = Math.max(0, cy - 2); y <= Math.min(height - 1, cy + 2); y++) {
    for (let x = Math.max(0, cx - 2); x <= Math.min(width - 1, cx + 2); x++) {
      const i = (y * width + x) * 4;
      sum[0] += data[i];
      sum[1] += data[i + 1];
      sum[2] += data[i + 2];
      n++;
    }
  }
  return [sum[0] / n, sum[1] / n, sum[2] / n];
}
