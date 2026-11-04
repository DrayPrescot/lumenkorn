// Alben und Diashows im Datenordner speichern. Bewusst nicht im `localStorage`:
// Der ist klein, und ein voller Speicher ginge ohne Meldung verloren.
import { api, type DocKind, type Scope } from "./api";

/** Schreiben nacheinander: Ein älterer Stand darf einen neueren nie überholen. */
let queue: Promise<unknown> = Promise.resolve();

function inOrder(write: () => Promise<void>): Promise<void> {
  const run = queue.then(write);
  queue = run.catch(() => {});
  return run;
}

export async function loadDoc(kind: DocKind, scope: Scope): Promise<unknown> {
  // Erst nach allen laufenden Schreibvorgängen, sonst käme ein alter Stand zurück
  await queue;
  const json = await api.docGet(kind, scope);
  if (json === null) return null;
  try {
    return JSON.parse(json);
  } catch {
    // Kaputte Datei: als nicht vorhanden behandeln, sie bleibt liegen
    console.error(`Dokument nicht lesbar: ${kind}`, scope);
    return null;
  }
}

export function saveDoc(kind: DocKind, scope: Scope, value: unknown): Promise<void> {
  // Jetzt in Text wandeln: Geschrieben wird dieser Stand, auch wenn es dauert
  const json = JSON.stringify(value);
  return inOrder(() => api.docPut(kind, scope, json));
}

export function deleteDoc(kind: DocKind, scope: Scope): Promise<void> {
  return inOrder(() => api.docDelete(kind, scope));
}

/** Warten, bis alles geschrieben ist */
export const docsWritten = () => queue;

const LOCAL_PREFIXES: [string, DocKind][] = [
  ["lumenkorn.album.", "album"],
  ["lumenkorn.slideshow.", "slideshow"],
];

/** Alben und Diashows aus früheren Versionen (`localStorage`) übernehmen.
 *  Ein Eintrag wird erst gelöscht, wenn er sicher auf der Platte liegt. */
export async function migrateLocalDocs() {
  let keys: string[];
  try {
    keys = Array.from({ length: localStorage.length }, (_, i) => localStorage.key(i) ?? "");
  } catch {
    return;
  }
  for (const key of keys) {
    const hit = LOCAL_PREFIXES.find(([p]) => key.startsWith(p));
    if (!hit) continue;
    try {
      // Die Einstellungen liegen unter `….settings`: kein JSON, bleiben, wo sie sind
      const scope = JSON.parse(key.slice(hit[0].length)) as Scope;
      const value = localStorage.getItem(key);
      if (value === null) continue;
      if ((await api.docGet(hit[1], scope)) === null) await api.docPut(hit[1], scope, value);
      localStorage.removeItem(key);
    } catch {
      // Beim nächsten Start wieder versuchen
    }
  }
}
