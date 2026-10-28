// Schriftarten des Systems für das Fotoalbum. Das Backend liefert die Namen
// der installierten Familien; hier bleiben nur die, die die Oberfläche auch
// wirklich darstellt (der Name in der Schriftdatei und der, unter dem
// WebView2/WebKit die Schrift kennt, können sich unterscheiden).
import { api } from "./api";

let cached: Promise<string[]> | null = null;

/** Prüftext: breite und schmale Zeichen, damit sich Schriften unterscheiden */
const PROBE = "mmmmmmmmmmlliiWQ@#&1";

/** Zeichnet der Browser `name` mit einer eigenen Schrift (nicht der Ersatzschrift)? */
function renders(ctx: CanvasRenderingContext2D, name: string): boolean {
  const quoted = `"${name.replace(/["\\]/g, "\\$&")}"`;
  for (const base of ["monospace", "serif"]) {
    ctx.font = `72px ${base}`;
    const fallback = ctx.measureText(PROBE).width;
    ctx.font = `72px ${quoted}, ${base}`;
    if (ctx.measureText(PROBE).width !== fallback) return true;
  }
  return false;
}

/** Nutzbare Schriftfamilien des Systems (einmal ermittelt) */
export function systemFonts(): Promise<string[]> {
  cached ??= api
    .systemFonts()
    .then((names) => {
      const ctx = document.createElement("canvas").getContext("2d");
      return ctx ? names.filter((n) => renders(ctx, n)) : names;
    })
    .catch(() => {
      cached = null;
      return [];
    });
  return cached;
}
