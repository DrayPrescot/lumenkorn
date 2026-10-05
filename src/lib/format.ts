// Kleine Anzeige-Helfer, die mehrere Komponenten brauchen.

/** Langer Pfad gekürzt auf die letzten drei Ordner, z. B. „… / 2025 / Lofoten / RAW“. */
export function shortPath(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts.length > 3 ? `… / ${parts.slice(-3).join(" / ")}` : parts.join(" / ");
}
