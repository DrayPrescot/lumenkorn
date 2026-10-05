// Zoom-Zustand einer Ansicht. Mehrere Ansichten (Vergleich) können sich einen
// Zustand teilen und zoomen dann gemeinsam.
//
// `zoom` ist das Verhältnis Bildschirmpixel (Gerätepixel) zu Originalpixeln:
// 1 = 100 %, also jedes Bildpixel ein Bildschirmpixel.

export const ZOOM_STEPS = [0.0625, 0.125, 0.25, 1 / 3, 0.5, 2 / 3, 1, 1.5, 2, 3, 4, 8];
export const MAX_ZOOM = 8;

export class ZoomState {
  /** Bild ganz einpassen */
  fit = $state(true);
  zoom = $state(1);
  /** Sichtbarer Mittelpunkt, normiert 0…1 */
  cx = $state(0.5);
  cy = $state(0.5);
  /** Zoom der Einpassen-Ansicht (von der Ansicht gemeldet) */
  fitZoom = $state(0.25);

  /** Aktueller Zoom, auch in der Einpassen-Ansicht */
  get effective(): number {
    return this.fit ? this.fitZoom : this.zoom;
  }

  reset() {
    this.fit = true;
    this.cx = 0.5;
    this.cy = 0.5;
  }

  set(z: number, cx = this.cx, cy = this.cy) {
    if (z <= this.fitZoom * 1.001) {
      this.reset();
      return;
    }
    this.fit = false;
    this.zoom = Math.min(MAX_ZOOM, z);
    this.cx = cx;
    this.cy = cy;
  }

  stepIn() {
    const cur = this.effective;
    const next = ZOOM_STEPS.find((s) => s > cur * 1.01) ?? MAX_ZOOM;
    this.set(Math.max(next, this.fitZoom * 1.01));
  }

  stepOut() {
    const cur = this.effective;
    const prev = [...ZOOM_STEPS].reverse().find((s) => s < cur * 0.99);
    if (prev === undefined || prev <= this.fitZoom) this.reset();
    else this.set(prev);
  }

  /** Einpassen ↔ 100 % */
  toggle(cx = 0.5, cy = 0.5) {
    if (!this.fit) this.reset();
    else this.set(Math.max(1, this.fitZoom * 1.01), cx, cy);
  }
}

/** Gemeinsame Tastenkürzel: Strg + / Strg − / Strg 0 / Z. `true`, wenn verarbeitet. */
export function zoomKey(e: KeyboardEvent, z: ZoomState): boolean {
  const mod = e.ctrlKey || e.metaKey;
  if (mod && (e.key === "+" || e.key === "=" || e.code === "NumpadAdd")) z.stepIn();
  else if (mod && (e.key === "-" || e.code === "NumpadSubtract")) z.stepOut();
  else if (mod && (e.key === "0" || e.code === "Numpad0")) z.reset();
  else if (!mod && !e.altKey && e.key.toLowerCase() === "z") z.toggle(z.cx, z.cy);
  else return false;
  e.preventDefault();
  return true;
}

// --- Volle Auflösung laden (Bibliothek) -------------------------------------
// Im Vergleich mit gekoppeltem Zoom wollen bis zu 12 Zellen zugleich das
// Original (je ~80 MB dekodiert): höchstens zwei gleichzeitig laden, das
// hervorgehobene Foto zuerst, abgebrochene Wünsche aus der Warteschlange nehmen.
const HI_MAX = 2;
let hiRunning = 0;
const hiQueue: { start: () => void }[] = [];

function hiPump() {
  while (hiRunning < HI_MAX && hiQueue.length) hiQueue.shift()!.start();
}

/** Bild `src` in die Ladewarteschlange stellen; `done` erhält das geladene Bild
 *  (null bei Fehler). Rückgabe: Abbrechen (wartend oder laufend), danach kein `done`. */
export function loadHi(src: string, priority: boolean, done: (img: HTMLImageElement | null) => void): () => void {
  let img: HTMLImageElement | null = null;
  let finished = false;
  const finish = (ok: boolean) => {
    if (finished) return;
    finished = true;
    hiRunning--;
    hiPump();
    done(ok ? img : null);
  };
  const entry = {
    start: () => {
      hiRunning++;
      img = new Image();
      img.onload = () => finish(true);
      img.onerror = () => finish(false);
      img.src = src;
    },
  };
  if (priority) hiQueue.unshift(entry);
  else hiQueue.push(entry);
  hiPump();
  return () => {
    const i = hiQueue.indexOf(entry);
    if (i >= 0) hiQueue.splice(i, 1);
    else if (img && !finished) {
      finished = true;
      img.onload = img.onerror = null;
      img.src = ""; // Download abbrechen
      hiRunning--;
      hiPump();
    }
  };
}
