// Tastenkürzel: alle anpassbaren Befehle an einer Stelle. Die Ansichten
// fragen `keymap.is(e, "befehl")` statt feste Tasten zu prüfen; eigene
// Belegungen speichert die Oberfläche im Browser-Speicher der App.
//
// Kombination als Text: Modifikatoren und Taste mit „+“, z. B. "Ctrl+Shift+E",
// "Shift+X", "5", "\\", "F11", "Ctrl+Plus". Ctrl steht auch für ⌘ (macOS).
// Fest (nicht anpassbar) bleibt die Bedienung: Pfeile, Enter, Leertaste,
// Escape in Ansichten und Dialogen, Eingaben in Feldern.

import { t } from "./i18n";

export type KeyGroup = "library" | "files" | "view" | "develop";

export interface KeyAction {
  id: string;
  group: KeyGroup;
  /** Anzeige (übersetzt) */
  label: string;
  keys: string[];
}

export const ACTIONS: KeyAction[] = [
  { id: "rate0", group: "library", label: t("Bewertung entfernen"), keys: ["0"] },
  { id: "rate1", group: "library", label: t("1 Stern"), keys: ["1"] },
  { id: "rate2", group: "library", label: t("2 Sterne"), keys: ["2"] },
  { id: "rate3", group: "library", label: t("3 Sterne"), keys: ["3"] },
  { id: "rate4", group: "library", label: t("4 Sterne"), keys: ["4"] },
  { id: "rate5", group: "library", label: t("5 Sterne"), keys: ["5"] },
  { id: "pick", group: "library", label: t("Als ausgewählt markieren"), keys: ["P"] },
  { id: "reject", group: "library", label: t("Ablehnen"), keys: ["X"] },
  { id: "rejectNext", group: "library", label: t("Ablehnen und weiter"), keys: ["Shift+X"] },
  { id: "unflag", group: "library", label: t("Markierung aufheben"), keys: ["U"] },
  { id: "grid", group: "library", label: t("Raster"), keys: ["G"] },
  { id: "large", group: "library", label: t("Groß anzeigen"), keys: ["E"] },
  { id: "compare", group: "library", label: t("Vergleichen"), keys: ["C"] },
  { id: "develop", group: "library", label: t("Entwickeln"), keys: ["D"] },
  { id: "map", group: "library", label: t("Karte"), keys: ["M"] },
  { id: "info", group: "library", label: t("Informationen ein/aus"), keys: ["I"] },
  { id: "import", group: "files", label: t("Fotos importieren"), keys: ["Ctrl+Shift+I"] },
  { id: "export", group: "files", label: t("Exportieren"), keys: ["Ctrl+Shift+E"] },
  { id: "deleteRejected", group: "files", label: t("Abgelehnte Fotos löschen"), keys: ["Ctrl+Delete", "Ctrl+Backspace"] },
  { id: "copySettings", group: "files", label: t("Basis-Einstellungen kopieren"), keys: ["Ctrl+Shift+C"] },
  { id: "pasteSettings", group: "files", label: t("Basis-Einstellungen einfügen"), keys: ["Ctrl+Shift+V"] },
  { id: "hdr", group: "files", label: t("HDR zusammenfügen"), keys: ["Ctrl+H"] },
  { id: "pano", group: "files", label: t("Panorama zusammenfügen"), keys: ["Ctrl+M"] },
  { id: "focus", group: "files", label: t("Focus Stacking"), keys: ["Ctrl+Shift+F"] },
  { id: "fullscreen", group: "view", label: t("Vollbild"), keys: ["F11"] },
  { id: "zoomIn", group: "view", label: t("Hineinzoomen"), keys: ["Ctrl+Plus", "Ctrl+="] },
  { id: "zoomOut", group: "view", label: t("Herauszoomen"), keys: ["Ctrl+Minus"] },
  { id: "zoomFit", group: "view", label: t("Einpassen"), keys: ["Ctrl+0"] },
  { id: "zoomToggle", group: "view", label: t("Einpassen ↔ 100 %"), keys: ["Z"] },
  { id: "auto", group: "develop", label: t("Auto"), keys: ["Ctrl+U"] },
  { id: "bw", group: "develop", label: t("Schwarzweiß"), keys: ["V"] },
  { id: "crop", group: "develop", label: t("Zuschneiden"), keys: ["R"] },
  { id: "before", group: "develop", label: t("Vorher/Nachher"), keys: ["\\"] },
  { id: "clipping", group: "develop", label: t("Clipping anzeigen"), keys: ["J"] },
  { id: "maskOverlay", group: "develop", label: t("Maske anzeigen"), keys: ["O"] },
  { id: "rotateLeft", group: "develop", label: t("Nach links drehen"), keys: ["["] },
  { id: "rotateRight", group: "develop", label: t("Nach rechts drehen"), keys: ["]"] },
  { id: "undo", group: "develop", label: t("Rückgängig"), keys: ["Ctrl+Z"] },
  { id: "redo", group: "develop", label: t("Wiederholen"), keys: ["Ctrl+Shift+Z", "Ctrl+Y"] },
];

export const GROUP_LABELS: Record<KeyGroup, string> = {
  library: t("Bibliothek und Ansichten"),
  files: t("Fotos und Dateien"),
  view: t("Anzeige"),
  develop: t("Entwickeln"),
};

const STORE = "lumenkorn.keys";
const MODIFIERS = new Set(["Control", "Shift", "Alt", "Meta", "AltGraph", "OS", "CapsLock"]);

interface Combo {
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
  key: string;
}

/** Ist die Taste ein Zeichen, dessen Umschalt-Stellung vom Tastaturlayout abhängt? */
function isSymbol(key: string): boolean {
  return key.length === 1 && !/[A-Z0-9]/i.test(key);
}

function parse(s: string): Combo {
  const parts = s.split("+");
  const key = parts.pop() || "+";
  return { ctrl: parts.includes("Ctrl"), shift: parts.includes("Shift"), alt: parts.includes("Alt"), key };
}

/** Taste eines Ereignisses als Name wie in den Kombinationen */
function keyName(e: KeyboardEvent): string {
  if (e.code === "NumpadAdd" || e.key === "+") return "Plus";
  if (e.code === "NumpadSubtract" || e.key === "-") return "Minus";
  if (e.key === " ") return "Space";
  if (e.key.length === 1) return e.key.toUpperCase();
  return e.key;
}

/** Modifikatoren eines Ereignisses; AltGr (Strg+Alt) bei Zeichen zählt nicht */
function eventCombo(e: KeyboardEvent): Combo {
  const key = keyName(e);
  const altGr = e.ctrlKey && e.altKey && (isSymbol(e.key) || e.getModifierState?.("AltGraph"));
  return {
    ctrl: !altGr && (e.ctrlKey || e.metaKey),
    alt: !altGr && e.altKey,
    shift: e.shiftKey,
    key,
  };
}

function same(a: Combo, b: Combo): boolean {
  if (a.key.toUpperCase() !== b.key.toUpperCase() || a.ctrl !== b.ctrl || a.alt !== b.alt) return false;
  // Bei Zeichen wie [ ] \ + hängt Umschalt vom Layout ab
  return isSymbol(a.key) || isSymbol(b.key) || a.key === "Plus" || a.key === "Minus" || a.shift === b.shift;
}

function load(): Record<string, string[]> {
  try {
    const v = JSON.parse(localStorage.getItem(STORE) ?? "{}");
    return v && typeof v === "object" ? v : {};
  } catch {
    return {};
  }
}

class Keymap {
  /** Eigene Belegungen (nur abweichende) */
  custom = $state<Record<string, string[]>>(load());
  /** Eine Taste wird gerade in den Einstellungen aufgenommen: Ansichten und
   *  Dialoge reagieren dann nicht */
  recording = $state(false);

  keys(id: string): string[] {
    return this.custom[id] ?? ACTIONS.find((a) => a.id === id)?.keys ?? [];
  }

  /** Passt das Tastenereignis zum Befehl? */
  is(e: KeyboardEvent, id: string): boolean {
    if (MODIFIERS.has(e.key)) return false;
    const ev = eventCombo(e);
    return this.keys(id).some((k) => same(parse(k), ev));
  }

  /** Kombination aus einem Tastenereignis (zum Belegen); null bei reiner Umschalt-/Strg-Taste */
  fromEvent(e: KeyboardEvent): string | null {
    if (MODIFIERS.has(e.key)) return null;
    const c = eventCombo(e);
    const mods = [c.ctrl && "Ctrl", c.shift && !isSymbol(c.key) && "Shift", c.alt && "Alt"].filter(Boolean);
    return [...mods, c.key].join("+");
  }

  /** Befehle, die diese Kombination schon nutzen */
  conflicts(combo: string, except: string): KeyAction[] {
    const c = parse(combo);
    return ACTIONS.filter((a) => a.id !== except && this.keys(a.id).some((k) => same(parse(k), c)));
  }

  set(id: string, keys: string[]) {
    const def = ACTIONS.find((a) => a.id === id)?.keys ?? [];
    const next = { ...this.custom };
    if (JSON.stringify(keys) === JSON.stringify(def)) delete next[id];
    else next[id] = keys;
    this.custom = next;
    this.#save();
  }

  reset(id: string) {
    const next = { ...this.custom };
    delete next[id];
    this.custom = next;
    this.#save();
  }

  resetAll() {
    this.custom = {};
    this.#save();
  }

  isCustom(id: string): boolean {
    return id in this.custom;
  }

  /** Anzeige, z. B. „Strg+Umschalt+E“ */
  label(combo: string): string {
    const c = parse(combo);
    const names: Record<string, string> = {
      Plus: "+",
      Minus: "−",
      Space: t("Leertaste"),
      Delete: t("Entf"),
      Backspace: t("Rücktaste"),
      ArrowLeft: "←",
      ArrowRight: "→",
      ArrowUp: "↑",
      ArrowDown: "↓",
      Escape: "Esc",
    };
    const parts = [c.ctrl && t("Strg"), c.shift && t("Umschalt"), c.alt && "Alt"].filter(Boolean) as string[];
    return [...parts, names[c.key] ?? c.key].join("+");
  }

  /** Erste Belegung eines Befehls zur Anzeige in Hinweisen („(Strg+U)“) */
  hint(id: string): string {
    const k = this.keys(id)[0];
    return k ? this.label(k) : "";
  }

  #save() {
    try {
      localStorage.setItem(STORE, JSON.stringify(this.custom));
    } catch {
      // Speicher gesperrt: gilt dann nur für diese Sitzung
    }
  }
}

export const keymap = new Keymap();

/**
 * Text mit der aktuellen Belegung eines Befehls für Tooltips, z. B.
 * withKey(t("Bibliothek"), "grid") → „Bibliothek (G)“. Ohne Belegung nur der
 * Text. Reaktiv: Nach einer Neubelegung zeigen Tooltips die neue Taste.
 */
export function withKey(text: string, id: string): string {
  const k = keymap.hint(id);
  return k ? `${text} (${k})` : text;
}
