// Englische Texte, je Bereich eine Datei. Schlüssel: der deutsche Quelltext.
const parts = import.meta.glob<Record<string, string>>("./*.ts", { eager: true, import: "default" });

export const en: Record<string, string> = Object.assign(
  {},
  ...Object.entries(parts)
    .filter(([file]) => !file.endsWith("/index.ts"))
    .map(([, dict]) => dict),
);
