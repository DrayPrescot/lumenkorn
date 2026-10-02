// Tastenkürzel dürfen nicht feuern, während jemand tippt – wohl aber, wenn
// gerade ein Regler oder eine Checkbox den Fokus hat.
const NON_TEXT = new Set(["range", "checkbox", "radio", "button", "submit", "reset", "color"]);

export function isTyping(e: KeyboardEvent): boolean {
  const t = e.target as HTMLElement | null;
  if (!t) return false;
  if (t.isContentEditable || t.tagName === "TEXTAREA" || t.tagName === "SELECT") return true;
  if (t.tagName === "INPUT") return !NON_TEXT.has((t as HTMLInputElement).type);
  return false;
}

/** Tasten, die ein fokussierter Regler selbst verarbeitet. */
export const SLIDER_KEYS = new Set(["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"]);
