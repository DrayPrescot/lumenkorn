// Fotos in voller Auflösung für das Vollbild der Diashow vorab laden. Bis ein
// Foto da ist (bearbeitete RAWs werden dafür neu entwickelt), zeigt die Bühne
// die Vorschau; danach wechselt sie auf das scharfe Bild.

import { SvelteSet } from "svelte/reactivity";
import { imageUrl } from "./api";
import { lib } from "./store.svelte";

/** Fotos, deren volle Auflösung geladen und dekodiert ist */
export const fullReady = new SvelteSet<string>();
/** Höchstens so viele Fotos in voller Auflösung halten */
const KEEP = 8;
const images = new Map<string, HTMLImageElement>();
/** Laden nacheinander: Das Backend entwickelt höchstens zwei zugleich */
let queue: Promise<void> = Promise.resolve();

export const fullUrl = (id: number) => imageUrl("full", id, lib.thumbVersion(id));

/** Fotos (in dieser Reihenfolge) laden; ältere geladene werden freigegeben */
export function preloadFull(ids: number[]) {
  for (const id of ids) {
    const url = fullUrl(id);
    if (images.has(url)) continue;
    const img = new Image();
    images.set(url, img);
    queue = queue.then(async () => {
      if (!images.has(url)) return;
      img.src = url;
      try {
        await img.decode();
        fullReady.add(url);
      } catch {
        images.delete(url);
      }
    });
  }
  const want = new Set(ids.map(fullUrl));
  for (const url of [...images.keys()]) {
    if (images.size <= KEEP) break;
    if (want.has(url)) continue;
    images.delete(url);
    fullReady.delete(url);
  }
}
