// Zustand des Fotoalbums: Erstellen (Auswahl im Backend), Bearbeiten in der
// Vorschau, Speichern als PDF. Das Album eines Projekts bleibt im Speicher der
// WebView erhalten und lässt sich später weiterbearbeiten.

import { api, listen, pickPdfTarget, type AlbumExportProgress, type Scope } from "./api";
import {
  DEFAULT_SETTINGS,
  autoCaptions,
  buildAlbum,
  mapPage,
  textId,
  pageLayout,
  redistribute,
  toDoc,
  validAlbum,
  type AlbumCandidate,
  type AlbumFont,
  type AlbumSettings,
  type AlbumState,
  type AlbumText,
  type CaptionMode,
} from "./album";
import { renderOverlay } from "./albumDraw";
import { lib } from "./store.svelte";
import { t } from "./i18n";

const SETTINGS_KEY = "lumenkorn.album.settings";
const albumKey = (scope: Scope) => `lumenkorn.album.${JSON.stringify(scope)}`;

function load<T>(key: string): T | null {
  try {
    const s = localStorage.getItem(key);
    return s ? (JSON.parse(s) as T) : null;
  } catch {
    return null;
  }
}

function store(key: string, value: unknown) {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Speicher voll oder gesperrt: dann eben nur für diese Sitzung
  }
}

/** Gewählt: ein Foto (`id`), ein Text (`text`) oder nur die Seite */
export type AlbumSelection = { page: number; id: number | null; text?: string };

class AlbumStore {
  state = $state<AlbumState | null>(null);
  /** Einstellungsdialog offen (mit dem Bereich, für den erstellt wird) */
  dialog = $state<{ scope: Scope; title: string } | null>(null);
  /** Fortschritt beim Auswählen (Vorschauen messen) */
  progress = $state<{ done: number; total: number } | null>(null);
  exporting = $state<AlbumExportProgress | null>(null);
  selection = $state<AlbumSelection | null>(null);

  byId = $derived(new Map<number, AlbumCandidate>((this.state?.candidates ?? []).map((c) => [c.id, c])));
  /** Layout je Seite (Felder in mm, Zahl der Anordnungen) */
  layouts = $derived(this.state ? this.state.pages.map((p) => pageLayout(this.state!, p, this.byId)) : []);
  photoCount = $derived(this.state ? this.state.pages.reduce((n, p) => n + p.ids.length, 0) : 0);

  #listening = false;

  #listen() {
    if (this.#listening) return;
    this.#listening = true;
    listen("album-progress", (p) => {
      if (this.progress) this.progress = p;
    });
    listen("album-export-progress", (p) => {
      this.exporting = p.finished ? null : p;
      if (!p.finished) return;
      if (p.error) lib.notify(t("Album nicht gespeichert: {error}", { error: p.error }), "error");
      else {
        lib.notify(t("Album gespeichert: {pdf} und die Seiten als JPG in {dir}", { pdf: p.pdf, dir: p.dir }));
        if (p.failed.length)
          lib.notify(t("Nicht alle Fotos ließen sich entwickeln: {list}", { list: p.failed.join("; ") }), "error");
      }
    });
  }

  /** Gespeicherte Einstellungen (zuletzt benutzt) */
  lastSettings(): AlbumSettings {
    return { ...DEFAULT_SETTINGS, ...(load<Partial<AlbumSettings>>(SETTINGS_KEY) ?? {}) };
  }

  /** Album des Bereichs öffnen; gibt es noch keins, die Einstellungen zeigen. */
  open(scope: Scope, title: string) {
    this.#listen();
    const saved = load<unknown>(albumKey(scope));
    if (validAlbum(saved)) {
      // Ältere Fassungen kennen Schrift, Beschriftung und Karte noch nicht
      saved.settings = { ...DEFAULT_SETTINGS, ...saved.settings };
      this.state = saved;
      this.selection = null;
      lib.view = "album";
      this.#refreshPlaces();
    } else this.dialog = { scope, title };
  }

  async create(settings: AlbumSettings, scope: Scope, title: string): Promise<boolean> {
    this.#listen();
    store(SETTINGS_KEY, settings);
    this.progress = { done: 0, total: 0 };
    try {
      const s = await api.albumSuggest({
        scope,
        theme: settings.theme,
        customTheme: settings.theme === "custom" ? settings.customTheme : null,
        count: settings.count,
        skipUneditedRaw: settings.skipUneditedRaw,
      });
      if (!this.progress) return false; // abgebrochen
      this.state = buildAlbum(settings, scope, title, s);
      this.selection = null;
      this.#save();
      lib.view = "album";
      const picked = s.candidates.filter((c) => c.picked).length;
      if (picked > settings.count)
        lib.notify(t("Alle Fotos mit 4 oder 5 Sternen sind dabei – das Album hat deshalb {n} statt {count} Fotos.", { n: picked, count: settings.count }));
      if (!s.themeUsed && (settings.theme !== "custom" || settings.customTheme.trim()))
        lib.notify(t("Ohne KI-Analyse wurden die Fotos nur nach Qualität und Bewertung ausgewählt, nicht nach dem Thema."));
      return true;
    } catch (e) {
      if (this.progress) lib.notify(String(e), "error");
      return false;
    } finally {
      this.progress = null;
    }
  }

  cancelCreate() {
    this.progress = null;
    api.albumSuggestCancel();
  }

  close() {
    this.selection = null;
    lib.view = "grid";
  }

  /** Album verwerfen (gespeicherte Fassung löschen) */
  discard() {
    if (this.state) store(albumKey(this.state.scope), null);
    this.state = null;
    this.close();
  }

  #save() {
    if (this.state) store(albumKey(this.state.scope), $state.snapshot(this.state));
  }

  #edit(f: (a: AlbumState) => void) {
    if (!this.state) return;
    f(this.state);
    this.#save();
  }

  replace(page: number, oldId: number, newId: number) {
    // Die jetzige Aufteilung festhalten: Neu angeordnet landete das neue Foto
    // oft in einem anderen Feld (anderes Seitenverhältnis, andere Wertung)
    const slots = this.layouts[page]?.cells.map(({ x, y, w, h }) => ({ x, y, w, h }));
    this.#edit((a) => {
      const p = a.pages[page];
      const i = p.ids.indexOf(oldId);
      if (i < 0) return;
      p.ids[i] = newId;
      if (slots?.length === p.ids.length) p.slots = slots;
      delete a.offsets[oldId];
    });
    this.selection = { page, id: newId };
  }

  remove(page: number, id: number) {
    this.#edit((a) => {
      a.pages[page].ids = a.pages[page].ids.filter((x) => x !== id);
      delete a.pages[page].slots;
      delete a.offsets[id];
    });
    this.selection = { page, id: null };
  }

  add(page: number, id: number) {
    this.#edit((a) => {
      const ids = a.pages[page].ids;
      if (ids.length >= 6) return;
      // chronologisch einsortieren
      const order = new Map(a.candidates.map((c, i) => [c.id, i]));
      ids.push(id);
      ids.sort((x, y) => (order.get(x) ?? 0) - (order.get(y) ?? 0));
      delete a.pages[page].slots;
    });
  }

  /** Foto auf die vorige (−1) oder nächste (+1) Seite */
  move(page: number, id: number, dir: -1 | 1) {
    const to = page + dir;
    if (!this.state || to < 0 || to >= this.state.pages.length || this.state.pages[to].ids.length >= 6) return;
    this.#edit((a) => {
      a.pages[page].ids = a.pages[page].ids.filter((x) => x !== id);
      delete a.pages[page].slots;
      delete a.pages[to].slots;
      if (dir < 0) a.pages[to].ids.push(id);
      else a.pages[to].ids.unshift(id);
      delete a.offsets[id];
    });
    this.selection = { page: to, id };
  }

  relayout(page: number) {
    this.#edit((a) => {
      a.pages[page].variant++;
      delete a.pages[page].slots;
      for (const id of a.pages[page].ids) delete a.offsets[id];
    });
  }

  // --- Texte -------------------------------------------------------------------

  /** Neuer Text auf der Seite (unter den Fotos) */
  addText(page: number) {
    const text: AlbumText = { id: textId(), text: t("Text"), pos: "bottom", size: 12, align: "center", color: "auto" };
    this.#edit((a) => {
      const p = a.pages[page];
      p.texts = [...(p.texts ?? []), text];
      delete p.slots;
    });
    this.selection = { page, id: null, text: text.id };
  }

  updateText(page: number, id: string, patch: Partial<AlbumText>, save = true) {
    if (!this.state) return;
    const p = this.state.pages[page];
    const t = p?.texts?.find((x) => x.id === id);
    if (!t) return;
    Object.assign(t, patch, { auto: false });
    // Bänder oben/unten verändern den Platz für die Fotos
    if (t.pos !== "free" || patch.pos) delete p.slots;
    if (save) this.#save();
  }

  /** Nach dem Ziehen eines Textes einmal speichern */
  commitText() {
    this.#save();
  }

  removeText(page: number, id: string) {
    this.#edit((a) => {
      const p = a.pages[page];
      p.texts = (p.texts ?? []).filter((x) => x.id !== id);
      if (!p.texts.length) delete p.texts;
      delete p.slots;
    });
    this.selection = { page, id: null };
  }

  /** Datum/Ort neu eintragen (ersetzt nur die automatischen Beschriftungen) */
  setCaptions(mode: CaptionMode) {
    this.#edit((a) => {
      a.settings.captions = mode;
      a.pages = autoCaptions(a, this.byId);
      for (const p of a.pages) delete p.slots;
    });
  }

  setFont(font: AlbumFont) {
    this.#edit((a) => (a.settings.font = font));
  }

  /** Kartenseite vorne oder hinten einfügen */
  addMap(where: "first" | "last") {
    this.#edit((a) => {
      if (where === "first") a.pages.unshift(mapPage());
      else a.pages.push(mapPage());
    });
    this.selection = { page: where === "first" ? 0 : this.state!.pages.length - 1, id: null };
  }

  /** Stecknadel umbenennen (`name` leer: ohne Namen; gleich dem Vorschlag: zurücksetzen) */
  renamePin(page: number, key: string, name: string) {
    this.#edit((a) => {
      const m = a.pages[page].map;
      if (!m) return;
      const names = { ...(m.names ?? {}) };
      if (name === key) delete names[key];
      else names[key] = name;
      m.names = Object.keys(names).length ? names : undefined;
    });
  }

  /**
   * Ortsnamen eines gespeicherten Albums auffrischen: Sie können inzwischen
   * neu ermittelt worden sein (z. B. lateinisch statt in Landesschrift). Die
   * automatische Beschriftung folgt, eigene Texte bleiben.
   */
  async #refreshPlaces() {
    const a = this.state;
    if (!a) return;
    try {
      const ids = a.candidates.filter((c) => c.lat != null).map((c) => c.id);
      if (!ids.length) return;
      const places = await api.albumPlaces(ids);
      if (this.state !== a) return;
      let changed = false;
      for (const c of a.candidates) {
        const p = places[c.id];
        if (p !== undefined && p !== c.place) {
          c.place = p;
          changed = true;
        }
      }
      if (changed) {
        a.pages = autoCaptions(a, this.byId);
        this.#save();
      }
    } catch {
      // Ohne Antwort bleiben die gespeicherten Namen
    }
  }

  setMapLabels(page: number, labels: boolean) {
    this.#edit((a) => {
      const p = a.pages[page];
      if (p.map) p.map.labels = labels;
    });
  }

  setOffset(id: number, pos: [number, number]) {
    if (!this.state) return;
    this.state.offsets[id] = pos;
  }

  /** Nach dem Verschieben mit der Maus einmal speichern */
  commitOffset() {
    this.#save();
  }

  resetOffset(id: number) {
    this.#edit((a) => delete a.offsets[id]);
  }

  addPage(after: number) {
    this.#edit((a) => a.pages.splice(after + 1, 0, { ids: [], variant: 0 }));
    this.selection = { page: after + 1, id: null };
  }

  deletePage(page: number) {
    this.#edit((a) => a.pages.splice(page, 1));
    this.selection = null;
  }

  redistribute() {
    this.#edit((a) => {
      a.pages = redistribute(a);
      a.offsets = {};
    });
    this.selection = null;
  }

  async exportPdf() {
    const a = this.state;
    if (!a || this.exporting) return;
    this.#listen();
    const path = await pickPdfTarget(t("Album speichern"), a.title || t("Fotoalbum"));
    if (!path) return;
    const doc = toDoc(a, this.byId);
    this.exporting = { done: 0, total: doc.pages.length, finished: false, failed: [], error: null, pdf: path, dir: "" };
    try {
      // Texte und Karte in Druckauflösung zeichnen (dieselbe Schrift wie in der Vorschau)
      const snap = $state.snapshot(a) as AlbumState;
      for (let i = 0; i < doc.pages.length; i++) doc.pages[i].overlay = await renderOverlay(snap, i, this.layouts[i]);
      // Leere Seiten (ohne Fotos, Text und Karte) nicht drucken
      doc.pages = doc.pages.filter((p) => p.cells.length || p.overlay);
      if (!doc.pages.length) {
        this.exporting = null;
        return;
      }
      this.exporting = { ...this.exporting, total: doc.pages.length };
      await api.albumExport(doc, path);
    } catch (e) {
      this.exporting = null;
      lib.notify(String(e), "error");
    }
  }
}

export const album = new AlbumStore();
