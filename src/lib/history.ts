/** Rückgängig/Wiederholen für einen Bearbeitungsstand als JSON-Zeichenketten: Die
 *  Länge ist die exakte Größe, Pinselmasken mit tausenden Punkten belegen als Objekte
 *  ein Vielfaches, und geparst wird nur bei Rückgängig/Wiederholen. */
export class History {
  #undo: string[] = [];
  #redo: string[] = [];

  /** Höchstens so viele Schritte und so viele Zeichen insgesamt; der jüngste Schritt
   *  bleibt immer erhalten. */
  constructor(
    readonly maxSteps = 200,
    readonly maxChars = 50_000_000,
  ) {}

  get canUndo(): boolean {
    return this.#undo.length > 0;
  }

  get canRedo(): boolean {
    return this.#redo.length > 0;
  }

  /** Neuer Schritt: `before` ist der Stand vor der Änderung. Verwirft „Wiederholen“. */
  push(before: string) {
    this.#undo.push(before);
    let total = 0;
    for (const e of this.#undo) total += e.length;
    while (this.#undo.length > 1 && (this.#undo.length > this.maxSteps || total > this.maxChars)) {
      total -= this.#undo[0].length;
      this.#undo.shift();
    }
    this.#redo = [];
  }

  /** Einen Schritt zurück: liefert den vorigen Stand, `current` kommt nach „Wiederholen“. */
  undo(current: string): string | undefined {
    const prev = this.#undo.pop();
    if (prev !== undefined) this.#redo.push(current);
    return prev;
  }

  /** Einen Schritt vor: liefert den nächsten Stand, `current` kommt nach „Rückgängig“. */
  redo(current: string): string | undefined {
    const next = this.#redo.pop();
    if (next !== undefined) this.#undo.push(current);
    return next;
  }

  clear() {
    this.#undo = [];
    this.#redo = [];
  }
}
