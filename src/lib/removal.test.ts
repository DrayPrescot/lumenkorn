import { describe, expect, it } from "vitest";
import { removeFromView } from "./removal";

const list = (...ids: number[]) => ids.map((id) => ({ id }));
const ids = (v: { images: { id: number }[] }) => v.images.map((i) => i.id);

describe("removeFromView", () => {
  it("der Fokus bleibt auf demselben Foto, auch wenn davor gelöscht wird", () => {
    const v = removeFromView({ images: list(1, 2, 3, 4), focus: 2, selection: new Set([3]) }, new Set([1]), false);
    expect(ids(v)).toEqual([2, 3, 4]);
    expect(v.focus).toBe(1);
    expect(v.images[v.focus].id).toBe(3);
    expect([...v.selection]).toEqual([3]);
  });

  it("war das fokussierte Foto gelöscht, rückt das nächste nach (Lupe: auch als Auswahl)", () => {
    const v = removeFromView({ images: list(1, 2, 3, 4), focus: 1, selection: new Set([2]) }, new Set([2]), true);
    expect(v.images[v.focus].id).toBe(3);
    expect([...v.selection]).toEqual([3]);
  });

  it("im Raster bleibt die Auswahl ohne die gelöschten, ohne neues Foto", () => {
    const v = removeFromView({ images: list(1, 2, 3, 4), focus: 1, selection: new Set([2, 4]) }, new Set([2]), false);
    expect(v.images[v.focus].id).toBe(3);
    expect([...v.selection]).toEqual([4]);
  });

  it("mehrere gelöscht, davor und am Fokus: der Fokus rückt um die davor gelöschten nach vorn", () => {
    const v = removeFromView({ images: list(1, 2, 3, 4, 5), focus: 2, selection: new Set() }, new Set([1, 3]), false);
    expect(ids(v)).toEqual([2, 4, 5]);
    expect(v.images[v.focus].id).toBe(4);
  });

  it("war das letzte Foto fokussiert, geht der Fokus auf das neue letzte", () => {
    const v = removeFromView({ images: list(1, 2, 3), focus: 2, selection: new Set([3]) }, new Set([3]), true);
    expect(v.images[v.focus].id).toBe(2);
  });

  it("alles gelöscht: kein Fokus, keine Auswahl", () => {
    const v = removeFromView({ images: list(1, 2), focus: 0, selection: new Set([1, 2]) }, new Set([1, 2]), true);
    expect(v).toEqual({ images: [], focus: -1, selection: new Set() });
  });
});
