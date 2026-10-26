// Dateitypen für Filter und Statistik: Endungen, die ein Format sind,
// zusammenfassen (wie `formats::expand_file_types` im Backend).

import type { FileTypeCount } from "./api";

const SAME_TYPE: [string, string[]][] = [
  ["JPEG", ["jpg", "jpeg", "jpe", "jfif"]],
  ["TIFF", ["tif", "tiff"]],
];

/** Anzeigename einer Endung („jpg“ → „JPEG“, „arw“ → „ARW“) */
export function typeLabel(ext: string): string {
  const e = ext.toLowerCase();
  return SAME_TYPE.find(([, g]) => g.includes(e))?.[0] ?? e.toUpperCase();
}

/** Alle Endungen eines Typs */
export function typeExts(ext: string): string[] {
  const e = ext.toLowerCase();
  return SAME_TYPE.find(([, g]) => g.includes(e))?.[1] ?? [e];
}

export interface FileType {
  label: string;
  exts: string[];
  raw: boolean;
  count: number;
}

/** Endungen zu Typen zusammenfassen, häufigste zuerst */
export function groupTypes(list: FileTypeCount[]): FileType[] {
  const by = new Map<string, FileType>();
  for (const t of list) {
    const label = typeLabel(t.ext);
    const g = by.get(label) ?? { label, exts: typeExts(t.ext), raw: t.kind === "raw", count: 0 };
    g.count += t.count;
    by.set(label, g);
  }
  return [...by.values()].sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));
}
