import { describe, expect, it } from "vitest";
import { History } from "./history";

describe("History", () => {
  it("geht Schritte zurück und wieder vor", () => {
    const h = new History();
    // Stände a → b → c; push bekommt jeweils den Stand vor der Änderung
    h.push("a");
    h.push("b");
    expect(h.undo("c")).toBe("b");
    expect(h.undo("b")).toBe("a");
    expect(h.undo("a")).toBeUndefined();
    expect(h.redo("a")).toBe("b");
    expect(h.redo("b")).toBe("c");
    expect(h.redo("c")).toBeUndefined();
  });

  it("eine neue Änderung verwirft Wiederholen", () => {
    const h = new History();
    h.push("a");
    expect(h.undo("b")).toBe("a");
    expect(h.canRedo).toBe(true);
    h.push("a");
    expect(h.canRedo).toBe(false);
    expect(h.undo("x")).toBe("a");
  });

  it("begrenzt die Zahl der Schritte, die ältesten fallen weg", () => {
    const h = new History(3);
    for (const s of ["1", "2", "3", "4", "5"]) h.push(s);
    expect([h.undo("6"), h.undo("5"), h.undo("4"), h.undo("3")]).toEqual(["5", "4", "3", undefined]);
  });

  it("begrenzt die Gesamtgröße, behält aber den jüngsten Schritt", () => {
    const h = new History(200, 10);
    h.push("aaaa");
    h.push("bbbb");
    h.push("cccccccccccccccc"); // allein schon zu groß
    expect(h.undo("d")).toBe("cccccccccccccccc");
    expect(h.canUndo).toBe(false);
  });

  it("clear leert beide Richtungen (anderes Foto geöffnet)", () => {
    const h = new History();
    h.push("a");
    h.push("b");
    h.undo("c");
    h.clear();
    expect(h.canUndo || h.canRedo).toBe(false);
  });
});
