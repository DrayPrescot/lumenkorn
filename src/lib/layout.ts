// Blocksatz-Layout für das Bildraster: Jede Zeile wird exakt auf die
// verfügbare Breite skaliert, Bilder behalten ihr echtes Seitenverhältnis.

export interface Cell {
  index: number;
  left: number;
  width: number;
}

export interface Row {
  top: number;
  height: number;
  cells: Cell[];
}

export function justify(aspects: number[], width: number, target: number, gap: number): { rows: Row[]; height: number } {
  const rows: Row[] = [];
  if (width <= 0) return { rows, height: 0 };

  let top = 0;
  let start = 0;
  let sum = 0;

  const emit = (end: number, height: number) => {
    let left = 0;
    const cells: Cell[] = [];
    for (let i = start; i < end; i++) {
      const w = aspects[i] * height;
      cells.push({ index: i, left, width: w });
      left += w + gap;
    }
    // Rundungsfehler an der letzten Zelle ausgleichen, damit die Kante bündig ist.
    if (height !== target && cells.length) {
      const last = cells[cells.length - 1];
      last.width = Math.max(1, width - last.left);
    }
    rows.push({ top, height, cells });
    top += height + gap;
  };

  for (let i = 0; i < aspects.length; i++) {
    sum += aspects[i];
    const n = i - start + 1;
    const rowWidth = sum * target + gap * (n - 1);
    if (rowWidth >= width) {
      const h = (width - gap * (n - 1)) / sum;
      emit(i + 1, h);
      start = i + 1;
      sum = 0;
    }
  }
  // Letzte, unvollständige Zeile nicht strecken.
  if (start < aspects.length) emit(aspects.length, target);

  return { rows, height: Math.max(0, top - gap) };
}

/** Seitenverhältnis mit vernünftigem Standard, falls Maße noch unbekannt sind. */
export function aspectOf(w: number | null, h: number | null): number {
  if (!w || !h) return 1.5;
  return Math.min(3, Math.max(0.4, w / h));
}

/** Seitenverhältnis im Raster und Filmstreifen: nach Drehen und Zuschnitt der
 *  Bearbeitung, sonst das des Originals. */
export function imageAspect(im: { width: number | null; height: number | null; aspect?: number }): number {
  return im.aspect ? Math.min(3, Math.max(0.4, im.aspect)) : aspectOf(im.width, im.height);
}

/** Wie `EDIT_ASPECT` im Katalog: Vierteldrehung und Zuschnitt (normiert auf
 *  das gedrehte Bild). `undefined` ohne bekannte Maße. */
export function editAspect(
  w: number | null,
  h: number | null,
  p: { rotate: number; crop: { w: number; h: number } | null },
): number | undefined {
  if (!w || !h) return undefined;
  const odd = p.rotate % 2 === 1;
  const c = p.crop ?? { w: 1, h: 1 };
  const a = ((odd ? h : w) * c.w) / ((odd ? w : h) * c.h);
  return Number.isFinite(a) && a > 0 ? a : undefined;
}

/** Maße für den Zoom in der Bibliothek: längste Kante in voller Auflösung und
 * die der Quelle, aus der die Vorschau entstand. Bearbeitete Bilder werden für
 * die Vorschau aus der auf 2048 px verkleinerten Quelle entwickelt (ein
 * Zuschnitt macht das Bild kleiner, nicht gröber); unbearbeitete sind das
 * verkleinerte bzw. eingebettete Bild selbst – dann zählt die natürliche Größe. */
export function libraryDims(im: { width: number | null; height: number | null; edited: boolean }): {
  fullLong: number | null;
  srcLong: number | null;
} {
  const fullLong = im.width && im.height ? Math.max(im.width, im.height) : null;
  const srcLong = fullLong && im.edited ? Math.min(2048, fullLong) : null;
  return { fullLong, srcLong };
}
