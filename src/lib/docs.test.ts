import { beforeEach, describe, expect, it, vi } from "vitest";

const files = new Map<string, string>();
const key = (kind: string, scope: unknown) => `${kind}:${JSON.stringify(scope)}`;
let delay = 0;
let failNext = false;

vi.mock("./api", () => ({
  api: {
    docGet: async (kind: string, scope: unknown) => files.get(key(kind, scope)) ?? null,
    docPut: async (kind: string, scope: unknown, json: string) => {
      const wait = delay;
      delay = 0;
      if (wait) await new Promise((r) => setTimeout(r, wait));
      if (failNext) {
        failNext = false;
        throw new Error("Platte voll");
      }
      files.set(key(kind, scope), json);
    },
    docDelete: async (kind: string, scope: unknown) => void files.delete(key(kind, scope)),
  },
}));

const { deleteDoc, docsWritten, loadDoc, migrateLocalDocs, saveDoc } = await import("./docs");

const project = { type: "project", id: 1 } as const;

/** `localStorage` für die Übernahme aus früheren Versionen */
function fakeLocalStorage(entries: Record<string, string>) {
  const store = new Map(Object.entries(entries));
  vi.stubGlobal("localStorage", {
    get length() {
      return store.size;
    },
    key: (i: number) => [...store.keys()][i] ?? null,
    getItem: (k: string) => store.get(k) ?? null,
    removeItem: (k: string) => void store.delete(k),
  });
  return store;
}

beforeEach(() => {
  files.clear();
  delay = 0;
  failNext = false;
  vi.unstubAllGlobals();
});

describe("Dokumente speichern", () => {
  it("liest zurück, was gespeichert wurde", async () => {
    await saveDoc("album", project, { pages: 3 });
    expect(await loadDoc("album", project)).toEqual({ pages: 3 });
    expect(await loadDoc("slideshow", project)).toBeNull();
    await deleteDoc("album", project);
    expect(await loadDoc("album", project)).toBeNull();
  });

  it("ein älterer Stand überholt nie einen neueren", async () => {
    delay = 20; // der erste Schreibvorgang dauert länger
    void saveDoc("album", project, { stand: "alt" });
    void saveDoc("album", project, { stand: "neu" });
    await docsWritten();
    expect(await loadDoc("album", project)).toEqual({ stand: "neu" });
  });

  it("schreibt den Stand vom Aufruf, auch wenn er sich danach ändert", async () => {
    const doc = { stand: 1 };
    const saved = saveDoc("album", project, doc);
    doc.stand = 2;
    await saved;
    expect(await loadDoc("album", project)).toEqual({ stand: 1 });
  });

  it("ein Fehler kommt beim Aufrufer an und hält das Weitere nicht auf", async () => {
    failNext = true;
    await expect(saveDoc("album", project, { a: 1 })).rejects.toThrow("Platte voll");
    await saveDoc("album", project, { a: 2 });
    expect(await loadDoc("album", project)).toEqual({ a: 2 });
  });

  it("eine kaputte Datei gilt als nicht vorhanden", async () => {
    files.set(key("album", project), "{kaputt");
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(await loadDoc("album", project)).toBeNull();
    error.mockRestore();
  });
});

describe("Übernahme aus dem localStorage", () => {
  const oldKey = `lumenkorn.album.${JSON.stringify(project)}`;

  it("zieht Alben und Diashows um und lässt die Einstellungen liegen", async () => {
    const store = fakeLocalStorage({
      [oldKey]: '{"pages":3}',
      [`lumenkorn.slideshow.${JSON.stringify(project)}`]: '{"slides":1}',
      "lumenkorn.album.settings": '{"count":60}',
      "anderes.programm": "x",
    });
    await migrateLocalDocs();
    expect(await loadDoc("album", project)).toEqual({ pages: 3 });
    expect(await loadDoc("slideshow", project)).toEqual({ slides: 1 });
    expect([...store.keys()].sort()).toEqual(["anderes.programm", "lumenkorn.album.settings"]);
  });

  it("löscht nichts, was nicht sicher angekommen ist", async () => {
    const store = fakeLocalStorage({ [oldKey]: '{"pages":3}' });
    failNext = true;
    await migrateLocalDocs();
    expect(store.has(oldKey)).toBe(true);
    // Beim nächsten Start klappt es
    await migrateLocalDocs();
    expect(store.has(oldKey)).toBe(false);
    expect(await loadDoc("album", project)).toEqual({ pages: 3 });
  });

  it("überschreibt keine neuere Fassung", async () => {
    files.set(key("album", project), '{"pages":9}');
    const store = fakeLocalStorage({ [oldKey]: '{"pages":3}' });
    await migrateLocalDocs();
    expect(await loadDoc("album", project)).toEqual({ pages: 9 });
    expect(store.has(oldKey)).toBe(false);
  });
});
