import { describe, expect, it } from "vitest";
import { SaveQueue } from "./saveQueue";

/** Schreiben, das erst auf Zuruf fertig wird oder scheitert */
function controlledWrites() {
  const calls: { id: number; p: string; done: () => void; fail: (e: unknown) => void }[] = [];
  const written: [number, string][] = [];
  const write = (id: number, p: string) =>
    new Promise<void>((resolve, reject) =>
      calls.push({
        id,
        p,
        done: () => {
          written.push([id, p]);
          resolve();
        },
        fail: reject,
      }),
    );
  return { calls, written, write };
}

const tick = () => new Promise((r) => setTimeout(r, 0));

describe("SaveQueue", () => {
  it("speichert nur, wenn sich der Stand geändert hat", async () => {
    const written: string[] = [];
    const q = new SaveQueue<string>(async (_, p) => void written.push(p), "a");
    expect(await q.save(1, "a", "a")).toBe(false);
    expect(await q.save(1, "b", "b")).toBe(true);
    expect(await q.save(1, "b", "b")).toBe(false);
    expect(written).toEqual(["b"]);
  });

  it("schreibt strikt nacheinander: ein älterer Stand überholt keinen neueren", async () => {
    const w = controlledWrites();
    const q = new SaveQueue<string>(w.write);
    const first = q.save(1, "alt", "alt");
    const second = q.save(1, "neu", "neu");
    await tick();
    // Der zweite wartet, bis der erste fertig ist
    expect(w.calls.map((c) => c.p)).toEqual(["alt"]);
    w.calls[0].done();
    await first;
    await tick();
    expect(w.calls.map((c) => c.p)).toEqual(["alt", "neu"]);
    w.calls[1].done();
    await second;
    expect(w.written).toEqual([
      [1, "alt"],
      [1, "neu"],
    ]);
  });

  it("merkt sich Gescheitertes und versucht es beim nächsten Anlass erneut", async () => {
    const w = controlledWrites();
    const q = new SaveQueue<string>(w.write);
    const failed = q.save(1, "a", "a");
    await tick();
    w.calls[0].fail(new Error("Platte voll"));
    await expect(failed).rejects.toThrow("Platte voll");
    expect(q.isUnsaved(1)).toBe(true);
    // Inzwischen ist Foto 2 offen: Foto 1 wird im Hintergrund neu versucht
    q.retry(2);
    await tick();
    expect(w.calls[1]).toMatchObject({ id: 1, p: "a" });
    w.calls[1].done();
    await q.idle();
    expect(q.unsavedCount).toBe(0);
  });

  it("ein späteres erfolgreiches Speichern desselben Fotos ersetzt den gescheiterten Stand", async () => {
    let fail = true;
    const q = new SaveQueue<string>(async () => {
      if (fail) throw new Error("weg");
    });
    await expect(q.save(1, "a", "a")).rejects.toThrow();
    fail = false;
    // Der Aufrufer setzt nach einem Fehler `saved` zurück, damit derselbe Stand erneut geht
    q.saved = "";
    expect(await q.save(1, "b", "b")).toBe(true);
    expect(q.isUnsaved(1)).toBe(false);
  });

  it("das offene Foto verwirft seinen alten gescheiterten Stand (es speichert gleich den aktuellen)", async () => {
    const q = new SaveQueue<string>(async () => {
      throw new Error("weg");
    });
    await expect(q.save(1, "a", "a")).rejects.toThrow();
    q.retry(1);
    expect(q.isUnsaved(1)).toBe(false);
  });

  it("ein Fehler blockiert die Warteschlange nicht", async () => {
    const order: string[] = [];
    const q = new SaveQueue<string>(async (_, p) => {
      if (p === "x") throw new Error("weg");
      order.push(p);
    });
    const a = q.save(1, "x", "x");
    const b = q.save(2, "y", "y");
    await expect(a).rejects.toThrow();
    await b;
    expect(order).toEqual(["y"]);
  });
});
