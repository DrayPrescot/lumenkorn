// Fotos aus der angezeigten Liste nehmen (gelöscht), mit Fokus und Auswahl.

export interface ListView<T extends { id: number }> {
  images: T[];
  /** Index des fokussierten Fotos, -1 ohne */
  focus: number;
  selection: Set<number>;
}

/** Nimmt die Fotos `gone` heraus. Der Fokus bleibt auf demselben Foto; war es selbst
 *  gelöscht, geht er auf das Foto, das an seine Stelle rückt (am Ende auf das letzte).
 *  In Einzelansichten (Lupe, Entwickeln) ist das neu fokussierte Foto dann auch die
 *  Auswahl. */
export function removeFromView<T extends { id: number }>(view: ListView<T>, gone: Set<number>, singleView: boolean): ListView<T> {
  const { images, focus } = view;
  const focusId = focus >= 0 ? images[focus]?.id : undefined;
  // Gelöschte vor dem Fokus verschieben ihn nach vorn
  const before = images.slice(0, Math.max(focus, 0)).filter((i) => gone.has(i.id)).length;
  const oldFocus = focus - before;
  const rest = images.filter((i) => !gone.has(i.id));
  let selection = new Set([...view.selection].filter((id) => !gone.has(id)));
  let next: number;
  if (focusId !== undefined && !gone.has(focusId)) {
    next = rest.findIndex((i) => i.id === focusId);
  } else if (rest.length) {
    next = Math.min(Math.max(oldFocus, 0), rest.length - 1);
    if (singleView) selection = new Set([rest[next].id]);
  } else {
    next = -1;
  }
  return { images: rest, focus: next, selection };
}
