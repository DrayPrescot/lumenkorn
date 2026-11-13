/** Speichern der Bearbeitungen: strikt nacheinander (die Befehle laufen im Backend
 *  parallel, ein älterer Stand dürfte sonst einen neueren überschreiben), nur bei
 *  Änderung, und was scheitert, wird beim nächsten Anlass erneut versucht. Sonst wäre
 *  es verloren, sobald ein anderes Foto geöffnet wird. */
export class SaveQueue<P> {
  #chain: Promise<unknown> = Promise.resolve();
  /** Gescheiterte Stände je Foto */
  #unsaved = new Map<number, P>();
  /** Zuletzt gespeicherter Stand des offenen Fotos (JSON); "" erzwingt das nächste Speichern */
  saved: string;

  constructor(
    private readonly write: (id: number, p: P) => Promise<unknown>,
    saved = "",
  ) {
    this.saved = saved;
  }

  /** Erfüllt, wenn alles Eingereihte durch ist (auch Gescheitertes). */
  idle(): Promise<unknown> {
    return this.#chain;
  }

  /** Fotos, deren Stand nicht gespeichert werden konnte */
  get unsavedCount(): number {
    return this.#unsaved.size;
  }

  /** Konnte der Stand von Foto `id` zuletzt nicht gespeichert werden? */
  isUnsaved(id: number): boolean {
    return this.#unsaved.has(id);
  }

  /** Gescheiterte Stände erneut versuchen. Das offene Foto `open` speichert gleich
   *  seinen aktuellen Stand, sein alter wird verworfen. */
  retry(open: number | null) {
    for (const [id, p] of [...this.#unsaved]) {
      if (id === open) {
        this.#unsaved.delete(id);
        continue;
      }
      this.#enqueue(id, p).then(
        () => {
          if (this.#unsaved.get(id) === p) this.#unsaved.delete(id);
        },
        (e) => console.error(`Foto ${id} weiter nicht gespeichert`, e),
      );
    }
  }

  /** Stand `p` (als `json`) des offenen Fotos `id` speichern, wenn er sich vom zuletzt
   *  gespeicherten unterscheidet. `false`: nichts zu tun. Scheitert es, merkt sich die
   *  Warteschlange den Stand für [`retry`] und der Fehler geht an den Aufrufer. */
  async save(id: number, p: P, json: string): Promise<boolean> {
    if (json === this.saved) return false;
    this.saved = json;
    try {
      await this.#enqueue(id, p);
    } catch (e) {
      this.#unsaved.set(id, p);
      throw e;
    }
    // Ein früher gescheiterter Stand dieses Fotos ist jetzt überholt (gespeichert
    // wird der Reihe nach: Was davor scheiterte, ist älter)
    this.#unsaved.delete(id);
    return true;
  }

  #enqueue(id: number, p: P): Promise<unknown> {
    const run = this.#chain.then(() => this.write(id, p));
    this.#chain = run.catch(() => {});
    return run;
  }
}
