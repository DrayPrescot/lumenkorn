import { describe, expect, it } from "vitest";
import { DEFAULT_SETTINGS, photoAllowed, withDefaults } from "./album";

describe("Fotos für das Album", () => {
  const raw = { flag: 0, kind: "raw", edited: false };
  const editedRaw = { flag: 0, kind: "raw", edited: true };
  const jpeg = { flag: 0, kind: "raster", edited: false };
  const rejected = { flag: -1, kind: "raster", edited: true };

  it("„Nur bearbeitete“", () => {
    expect([raw, editedRaw, jpeg].map((i) => photoAllowed(i, "edited"))).toEqual([false, true, false]);
  });
  it("„Bearbeitete und JPEGs“", () => {
    expect([raw, editedRaw, jpeg].map((i) => photoAllowed(i, "editedJpeg"))).toEqual([false, true, true]);
  });
  it("„Alle“", () => {
    expect([raw, editedRaw, jpeg].map((i) => photoAllowed(i, "all"))).toEqual([true, true, true]);
  });
  it("abgelehnte nie", () => {
    for (const src of ["edited", "editedJpeg", "all"] as const) expect(photoAllowed(rejected, src)).toBe(false);
  });
});

describe("Gespeicherte Album-Einstellungen", () => {
  it("ergänzen fehlende Werte", () => {
    expect(withDefaults({}).photos).toBe(DEFAULT_SETTINGS.photos);
  });
  it("übernehmen den alten Haken: aus heißt alle Fotos", () => {
    expect(withDefaults({ skipUneditedRaw: false }).photos).toBe("all");
    expect(withDefaults({ skipUneditedRaw: true }).photos).toBe("editedJpeg");
  });
  it("behalten die neue Auswahl", () => {
    expect(withDefaults({ photos: "edited", skipUneditedRaw: false }).photos).toBe("edited");
  });
});
