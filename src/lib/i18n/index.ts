// Übersetzung der Oberfläche.
//
// Quelltexte sind deutsch und dienen zugleich als Schlüssel:
//   t("Ordner hinzufügen")                      → "Add folder"
//   t("{n} Fotos exportiert", { n: 3 })          → "3 photos exported"
// Der erste Parameter muss ein String-Literal in doppelten Anführungszeichen
// sein – `npm run i18n` prüft dann, ob jede Übersetzung vorhanden ist.
// Plural: zwei Schlüssel wählen, z. B. t(n === 1 ? "{n} Foto" : "{n} Fotos", { n }).
//
// Die Sprache gilt für die ganze Sitzung; ein Wechsel lädt die Oberfläche neu.
// So bleiben auch einmalig berechnete Texte (Listen, Beschriftungen) stimmig.

import { en } from "./en";

export type Lang = "de" | "en";
const KEY = "lumenkorn.lang";

function stored(): Lang | null {
  try {
    const v = localStorage.getItem(KEY);
    return v === "de" || v === "en" ? v : null;
  } catch {
    return null;
  }
}

/** Gewählte Sprache, sonst die des Systems (Deutsch nur bei deutschem System). */
export const lang: Lang =
  stored() ?? (typeof navigator !== "undefined" && navigator.language?.toLowerCase().startsWith("de") ? "de" : "en");
/** Für Zahlen- und Datumsformate */
export const locale = lang === "de" ? "de-DE" : "en-US";
/** Explizit gewählt (statt vom System übernommen) */
export const langChosen = stored() !== null;

const missing = new Set<string>();

export function t(de: string, vars?: Record<string, string | number>): string {
  let s = de;
  if (lang === "en") {
    const e = en[de];
    if (e !== undefined) s = e;
    else if (import.meta.env.DEV && !missing.has(de)) {
      missing.add(de);
      console.warn("[i18n] fehlt:", JSON.stringify(de));
    }
  }
  return fill(s, vars);
}

function fill(s: string, vars?: Record<string, string | number>): string {
  if (!vars) return s;
  return s.replace(/\{(\w+)\}/g, (m, k: string) => {
    const v = vars[k];
    return v === undefined ? m : typeof v === "number" ? v.toLocaleString(locale) : v;
  });
}

/** Wie `t`, aber für deutsche Wörter mit mehreren Bedeutungen: Die englische
 *  Übersetzung steht unter `"Kontext|Text"` (z. B. `tc("flag", "Auswahl")` →
 *  "Pick", während `t("Auswahl")` "Selection" bleibt). Deutsch: der Text selbst. */
export function tc(context: string, de: string, vars?: Record<string, string | number>): string {
  if (lang === "en") {
    const e = en[`${context}|${de}`];
    if (e !== undefined) return fill(e, vars);
  }
  return t(de, vars);
}

/** Abbruch durch den Nutzer (vom Hintergrund gemeldet) – kein Fehler. */
export function isCanceled(err: unknown): boolean {
  return /\b(Abgebrochen|Canceled)\b/.test(String(err));
}

/** Sprache wechseln (bleibt gespeichert) und Oberfläche neu laden. */
export function setLang(l: Lang) {
  try {
    localStorage.setItem(KEY, l);
  } catch {
    /* ohne Speicher: nur diese Sitzung */
  }
  location.reload();
}

/** Zahl im Format der Sprache (Dezimalkomma bzw. -punkt) */
export function num(v: number, opts?: Intl.NumberFormatOptions): string {
  return v.toLocaleString(locale, opts);
}

/** Dezimaltrennzeichen der Sprache */
export const decimalSep = lang === "de" ? "," : ".";
