// Zentraler Oberflächenzustand (Svelte-5-Runes).

import { api, listen, type AiStatus, type Person, type PeopleStatus, type AiDownloadEvent, type DeleteReport, type FolderNode, type ImageSummary, type Project, type Root, type Scope, type SortOrder, type ScanProgress, type MergeKind, type MergeProgress } from "./api";

class Library {
  roots = $state<Root[]>([]);
  trees = $state<Record<number, FolderNode>>({});
  projects = $state<Project[]>([]);

  scope = $state<Scope>({ type: "all" });
  search = $state("");
  /** Suche nach Bildinhalt (KI) statt nach Dateiname/Schlagwort. */
  semantic = $state(false);
  sort = $state<SortOrder>("capturedAsc");
  minRating = $state(0);

  /** Flach (`raw`): Bei 50 000 Fotos kein Proxy je Bild; geändert wird nur durch Ersetzen. */
  images = $state.raw<ImageSummary[]>([]);
  loading = $state(false);
  /** Index des fokussierten Bildes in `images`. */
  focus = $state(-1);
  /** Mehrfachauswahl (IDs). */
  selection = $state<Set<number>>(new Set());
  view = $state<"grid" | "loupe" | "compare" | "people">("grid");
  /** Bibliothek (Raster/Einzelansicht) oder Entwickeln. */
  mode = $state<"library" | "develop">("library");
  rowHeight = $state(200);

  /** Erhöht sich, wenn ein Thumbnail neu erzeugt wurde – erzwingt Neuladen. */
  thumbVersion = $state<Record<number, number>>({});
  scan = $state<ScanProgress | null>(null);
  message = $state<{ text: string; kind: "info" | "error" } | null>(null);
  ai = $state<AiStatus | null>(null);
  people = $state<Person[]>([]);
  peopleStatus = $state<PeopleStatus | null>(null);
  /** Erhöht sich, wenn Gesichter eines Bildes neu erkannt wurden. */
  facesVersion = $state<Record<number, number>>({});
  aiDownload = $state<AiDownloadEvent | null>(null);
  /** Erhöht sich, wenn KI-Schlagworte eines Bildes neu sind. */
  tagsVersion = $state<Record<number, number>>({});

  focused = $derived(this.focus >= 0 ? this.images[this.focus] : undefined);

  scopeTitle = $derived.by(() => {
    const s = this.scope;
    if (s.type === "all") return "Alle Fotos";
    if (s.type === "project") return this.projects.find((p) => p.id === s.id)?.name ?? "Projekt";
    if (s.type === "person") return this.people.find((p) => p.id === s.id)?.name ?? "Unbenannte Person";
    return s.path.split(/[\\/]/).pop() || s.path;
  });

  #queryTimer: ReturnType<typeof setTimeout> | undefined;
  #loadToken = 0;
  #messageTimer: ReturnType<typeof setTimeout> | undefined;

  async init() {
    await Promise.all([this.loadRoots(), this.loadProjects()]);
    await this.loadImages();

    listen("scan-progress", (p) => (this.scan = p));
    listen("library-changed", async (r) => {
      this.scan = null;
      const changes = r.added + r.updated + r.moved + r.missing;
      if (changes > 0) {
        const parts = [
          r.added && `${r.added} neu`,
          r.moved && `${r.moved} verschoben`,
          r.updated && `${r.updated} geändert`,
          r.missing && `${r.missing} fehlen`,
        ].filter(Boolean);
        this.notify(`Ordner abgeglichen: ${parts.join(", ")}`);
      }
      // Nichts geändert (z. B. nur eine XMP-Datei gespeichert): die Liste mit
      // bis zu 50 000 Fotos nicht neu laden
      if (changes === 0) return;
      await Promise.all([this.loadRoots(), this.loadProjects()]);
      await this.loadImages(true);
    });
    listen("catalog-updated", () => this.loadImages(true));
    // Einzelne Schlüssel ändern statt das ganze Objekt zu kopieren (O(1) statt O(n))
    listen("thumbs-ready", ({ ids }) => {
      for (const id of ids) this.thumbVersion[id] = (this.thumbVersion[id] ?? 0) + 1;
    });
    listen("job-error", ({ message }) => this.notify(message, "error"));
    listen("merge-progress", (e) => {
      this.mergeProgress = e;
      if (e.finished) this.#mergeDone(e);
    });

    this.refreshAi();
    this.refreshPeople();
    let peopleTimer: ReturnType<typeof setTimeout> | undefined;
    listen("faces-progress", ({ done, total, faces, ids }) => {
      if (this.peopleStatus) this.peopleStatus = { ...this.peopleStatus, analyzed: done, total, faces };
      for (const id of ids) this.facesVersion[id] = (this.facesVersion[id] ?? 0) + 1;
      // Personenliste gebündelt aktualisieren
      clearTimeout(peopleTimer);
      peopleTimer = setTimeout(() => this.refreshPeople(), 1500);
    });
    listen("ai-progress", ({ done, total, ids }) => {
      if (this.ai) this.ai = { ...this.ai, analyzed: done, total };
      for (const id of ids) this.tagsVersion[id] = (this.tagsVersion[id] ?? 0) + 1;
    });
    listen("ai-download", async (e) => {
      this.aiDownload = e.finished ? null : e;
      if (e.finished) {
        if (e.error) this.notify(`KI-Download: ${e.error}`, "error");
        else this.notify("KI ist bereit. Fotos werden jetzt im Hintergrund verschlagwortet.");
        await Promise.all([this.refreshAi(), this.refreshPeople()]);
      }
    });
  }

  async refreshPeople() {
    try {
      [this.people, this.peopleStatus] = await Promise.all([api.peopleList(), api.peopleStatus()]);
    } catch {
      this.people = [];
    }
  }

  async refreshAi() {
    try {
      this.ai = await api.aiStatus();
      if (!this.ai.modelReady) this.semantic = false;
    } catch {
      this.ai = null;
    }
  }

  async loadRoots() {
    this.roots = await api.listRoots();
    const trees: Record<number, FolderNode> = {};
    await Promise.all(this.roots.map(async (r) => (trees[r.id] = await api.folderTree(r.id))));
    this.trees = trees;
  }

  async loadProjects() {
    this.projects = await api.listProjects();
  }

  /** Lädt die Bilder des aktuellen Bereichs. `keepFocus` behält die Auswahl bei. */
  async loadImages(keepFocus = false) {
    // Anfragen laufen parallel: Nur das Ergebnis der neuesten zählt
    // (sonst kann die Suche nach „so“ die nach „sonne“ überholen).
    const token = ++this.#loadToken;
    this.loading = true;
    try {
      const scope = $state.snapshot(this.scope) as Scope;
      const text = this.search.trim();
      const images =
        this.semantic && text
          ? await api.semanticSearch(text, scope, this.minRating || null)
          : await api.queryImages({ scope, sort: this.sort, search: text || null, minRating: this.minRating || null });
      if (token !== this.#loadToken) return;
      // Fokus erst jetzt lesen: Während der Anfrage kann weitergeblättert worden sein
      const focusId = keepFocus ? this.focused?.id : undefined;
      this.images = images;
      this.focus = focusId !== undefined ? this.images.findIndex((i) => i.id === focusId) : -1;
      if (!keepFocus) this.selection = new Set();
      else if (this.selection.size) {
        // Nicht mehr gelistete Fotos aus der Auswahl nehmen (Export, Einfügen …)
        const ids = new Set(images.map((i) => i.id));
        const kept = [...this.selection].filter((id) => ids.has(id));
        if (kept.length !== this.selection.size) this.selection = new Set(kept);
      }
      if (this.focus < 0 && this.view === "loupe") this.view = "grid";
      // Entwickeln ohne Foto (z. B. nach Suche/Filter): zurück zur Bibliothek
      if (this.focus < 0 && this.mode === "develop") {
        this.mode = "library";
        this.view = "grid";
      }
    } catch (e) {
      if (token === this.#loadToken) this.notify(String(e), "error");
    } finally {
      if (token === this.#loadToken) this.loading = false;
    }
  }

  /** Verzögertes Neuladen, z. B. beim Tippen in der Suche. */
  reloadSoon() {
    clearTimeout(this.#queryTimer);
    this.#queryTimer = setTimeout(() => this.loadImages(), 180);
  }

  setScope(scope: Scope) {
    this.scope = scope;
    this.view = "grid";
    this.mode = "library";
    this.loadImages();
  }

  /** Wechselt in den Entwickeln-Modus mit dem fokussierten (oder ersten) Bild. */
  enterDevelop() {
    if (!this.images.length) return;
    if (this.focus < 0) this.select(0);
    this.mode = "develop";
  }

  isScope(scope: Scope) {
    const s = this.scope;
    if (s.type !== scope.type) return false;
    if (s.type === "folder" && scope.type === "folder") return s.path === scope.path;
    if (s.type === "project" && scope.type === "project") return s.id === scope.id;
    if (s.type === "person" && scope.type === "person") return s.id === scope.id;
    return true;
  }

  // --- Auswahl --------------------------------------------------------------

  select(index: number, mode: "single" | "toggle" | "range" = "single") {
    const img = this.images[index];
    if (!img) return;
    if (mode === "single") {
      this.selection = new Set([img.id]);
    } else if (mode === "toggle") {
      const s = new Set(this.selection);
      s.has(img.id) ? s.delete(img.id) : s.add(img.id);
      this.selection = s;
    } else {
      const from = Math.max(0, this.focus);
      const [a, b] = from < index ? [from, index] : [index, from];
      this.selection = new Set(this.images.slice(a, b + 1).map((i) => i.id));
    }
    this.focus = index;
  }

  move(delta: number) {
    if (!this.images.length) return;
    const next = Math.min(this.images.length - 1, Math.max(0, (this.focus < 0 ? 0 : this.focus + delta)));
    this.select(next);
  }

  /** Bilder in der Vergleichsansicht (Auswahl in Rasterreihenfolge, höchstens 12). */
  compareItems = $derived(
    this.images
      .map((im, index) => ({ im, index }))
      .filter((x) => this.selection.has(x.im.id))
      .slice(0, 12),
  );

  /** Vergleichsansicht der ausgewählten Bilder öffnen. */
  compare() {
    if (this.selection.size < 2) {
      this.notify("Für den Vergleich mindestens zwei Fotos auswählen (Strg- oder Umschalt-Klick).");
      return;
    }
    if (this.selection.size > 12) this.notify("Verglichen werden die ersten 12 ausgewählten Fotos.");
    const items = this.compareItems;
    if (!items.some((x) => x.index === this.focus)) this.focus = items[0].index;
    this.mode = "library";
    this.view = "compare";
  }

  /** Groß anzeigen: mehrere ausgewählte Fotos nebeneinander (Vergleich),
   *  sonst das fokussierte in der Einzelansicht. */
  openLarge() {
    if (this.selection.size > 1) this.compare();
    else if (this.focused) {
      this.mode = "library";
      this.view = "loupe";
    }
  }

  /** Bild aus dem Vergleich nehmen; bei weniger als zwei zurück zur Einzelansicht. */
  removeFromCompare(id: number) {
    const s = new Set(this.selection);
    s.delete(id);
    this.selection = s;
    if (this.focused?.id === id) {
      const next = this.compareItems[0];
      if (next) this.focus = next.index;
    }
    if (s.size < 2) this.view = s.size ? "loupe" : "grid";
  }

  /** Ein Bild groß geöffnet (Einzelansicht, Vergleich oder Entwickeln)? */
  #singleView(): boolean {
    return this.mode === "develop" || this.view === "loupe" || this.view === "compare";
  }

  /** Bewerten und Markieren: in den großen Ansichten nur das sichtbare Bild. */
  #rateTargets(): number[] {
    if (this.#singleView()) return this.focused ? [this.focused.id] : [];
    return this.targets();
  }

  /** Ziele für „Basis-Einstellungen einfügen“: in Einzelansicht und Vergleich
   *  das sichtbare Foto, sonst die Auswahl (Raster, Filmstreifen im Entwickeln). */
  pasteTargets(): number[] {
    if (this.mode === "library" && (this.view === "loupe" || this.view === "compare")) {
      return this.focused ? [this.focused.id] : [];
    }
    return this.targets();
  }

  // --- Zusammenfügen (HDR, Panorama, Focus Stacking) ---------------------------

  /** Offener Dialog: Art und Fotos in Anzeigereihenfolge */
  merge = $state<{ kind: MergeKind; ids: number[] } | null>(null);
  mergeProgress = $state<MergeProgress | null>(null);

  /** Öffnet den Dialog zum Zusammenfügen der ausgewählten Fotos. */
  openMerge(kind: MergeKind) {
    if (this.mergeProgress && !this.mergeProgress.finished) {
      this.notify("Es werden bereits Fotos zusammengefügt.");
      return;
    }
    const sel = this.selection;
    // Reihenfolge wie angezeigt (für Focus Stacking: Schärfeebene nah → fern)
    const ids = this.images.filter((i) => sel.has(i.id)).map((i) => i.id);
    if (ids.length < 2) {
      this.notify("Zum Zusammenfügen mindestens zwei Fotos auswählen (Strg- oder Umschalt-Klick).");
      return;
    }
    this.merge = { kind, ids };
  }

  /** Ergebnis fertig: Liste neu laden und das neue Foto auswählen. */
  async #mergeDone(e: MergeProgress) {
    if (e.error) {
      if (e.error !== "Abgebrochen") this.notify(`Zusammenfügen fehlgeschlagen: ${e.error}`, "error");
      return;
    }
    await this.loadImages(true);
    const index = e.imageId === null ? -1 : this.images.findIndex((i) => i.id === e.imageId);
    if (index >= 0) {
      this.select(index);
      this.notify(`${e.fileName} erstellt`);
    } else {
      this.notify(`${e.fileName} erstellt (in dieser Ansicht nicht sichtbar)`);
    }
  }

  /** Offenes Rechtsklick-Menü (Bildschirmposition) */
  contextMenu = $state<{ x: number; y: number } | null>(null);

  /** Rechtsklick auf ein Foto: es wird ausgewählt, falls es nicht schon zur Auswahl gehört. */
  openMenu(e: MouseEvent, index?: number) {
    e.preventDefault();
    if (index !== undefined) {
      const img = this.images[index];
      if (img && !this.selection.has(img.id)) this.select(index);
      else if (img) this.focus = index;
    }
    if (!this.focused) return;
    this.contextMenu = { x: e.clientX, y: e.clientY };
  }

  /** Ziel einer Aktion: Mehrfachauswahl, sonst das fokussierte Bild. */
  targets(): number[] {
    if (this.selection.size > 1) return [...this.selection];
    return this.focused ? [this.focused.id] : [];
  }

  // --- Bewertung ------------------------------------------------------------

  #writes: Promise<unknown> = Promise.resolve();
  /** Bewertungen/Markierungen strikt nacheinander schreiben (letzter Tastendruck gewinnt). */
  #write<T>(f: () => Promise<T>): Promise<T> {
    const run = this.#writes.then(f);
    this.#writes = run.catch(() => {});
    return run;
  }

  /** Ändert die Bilder `ids` (Set statt `includes`: bei 50 000 Fotos O(n) statt O(n·m)). */
  #patch(ids: number[], change: Partial<ImageSummary>) {
    const set = new Set(ids);
    this.images = this.images.map((i) => (set.has(i.id) ? { ...i, ...change } : i));
  }

  async rate(rating: number) {
    const ids = this.#rateTargets();
    if (!ids.length) return;
    // Sofort anzeigen (schnelles Tippen liest so den neuen Stand), dann schreiben
    this.#patch(ids, { rating });
    await this.#write(() => api.setRating(ids, rating)).catch((e) => this.#writeFailed(e));
  }

  #writeFailed(e: unknown) {
    this.notify(String(e), "error");
    this.loadImages(true);
  }

  async flag(flag: number) {
    const ids = this.#rateTargets();
    if (!ids.length) return;
    const current = this.focused?.flag;
    const value = ids.length === 1 && current === flag ? 0 : flag; // erneut drücken hebt auf
    this.#patch(ids, { flag: value });
    await this.#write(() => api.setFlag(ids, value)).catch((e) => this.#writeFailed(e));
  }

  // --- Aussortieren ---------------------------------------------------------

  /** Abgelehnte (und vorhandene) Bilder des aktuellen Bereichs. */
  rejected = $derived(this.images.filter((i) => i.flag === -1 && !i.missing));

  /**
   * Umschalt+X: ablehnen (ohne Umschalten) und zum nächsten Bild springen.
   * Im Vergleich weiter zum nächsten verglichenen Bild, im Raster mit
   * Mehrfachauswahl hinter das letzte ausgewählte Bild.
   */
  async rejectAndAdvance() {
    const ids = this.#rateTargets();
    if (!ids.length) return;
    await this.#write(() => api.setFlag(ids, -1));
    this.#patch(ids, { flag: -1 });

    if (this.mode === "library" && this.view === "compare") {
      const items = this.compareItems;
      const pos = items.findIndex((x) => x.index === this.focus);
      if (pos >= 0 && pos < items.length - 1) this.focus = items[pos + 1].index;
      else this.notify("Letztes Foto im Vergleich");
      return;
    }
    const set = new Set(ids);
    let last = -1;
    this.images.forEach((im, idx) => {
      if (set.has(im.id)) last = idx;
    });
    if (last >= this.images.length - 1) {
      this.notify("Letztes Foto erreicht");
      if (ids.length > 1) this.select(last);
      return;
    }
    this.select(last + 1);
  }

  /** Abgelehnte Fotos von der Festplatte löschen (Papierkorb oder endgültig). */
  async deleteRejected(ids: number[], toTrash: boolean): Promise<DeleteReport | null> {
    let report: DeleteReport;
    await this.#writes; // eben gedrückte Markierungen zuerst speichern
    try {
      report = await api.deleteRejected(ids, toTrash);
    } catch (e) {
      this.notify(String(e), "error");
      return null;
    }
    const gone = new Set(report.deleted);
    const focusId = this.focused?.id;
    // Gelöschte vor dem Fokus verschieben ihn nach vorn
    const before = this.images.slice(0, Math.max(this.focus, 0)).filter((i) => gone.has(i.id)).length;
    const oldFocus = this.focus - before;
    this.images = this.images.filter((i) => !gone.has(i.id));
    this.selection = new Set([...this.selection].filter((id) => !gone.has(id)));
    if (focusId !== undefined && !gone.has(focusId)) {
      this.focus = this.images.findIndex((i) => i.id === focusId);
    } else if (this.images.length) {
      // Fokus auf das Bild, das an die Stelle des gelöschten gerückt ist
      this.focus = Math.min(Math.max(oldFocus, 0), this.images.length - 1);
      if (this.#singleView()) this.selection = new Set([this.images[this.focus].id]);
    } else {
      this.focus = -1;
    }
    if (this.focus < 0 && this.mode === "develop") this.mode = "library";
    if (this.view === "compare" && this.selection.size < 2) this.view = this.focus >= 0 ? "loupe" : "grid";
    if (this.focus < 0 && this.view === "loupe") this.view = "grid";

    const mb = (report.bytes / 1e6).toLocaleString("de-DE", { maximumFractionDigits: report.bytes < 1e7 ? 2 : 0 });
    const n = (k: number) => `${k} ${k === 1 ? "Foto" : "Fotos"}`;
    const where = report.toTrash ? "in den Papierkorb verschoben" : "endgültig gelöscht";
    if (report.failed.length) {
      this.notify(
        `${n(report.deleted.length)} ${where}, ${report.failed.length} nicht: ${report.failed[0].error}`,
        "error",
      );
    } else {
      this.notify(`${n(report.deleted.length)} ${where} (${mb} MB)`);
    }
    this.loadRoots();
    this.refreshPeople();
    return report;
  }

  // --- Ordner & Projekte ----------------------------------------------------

  async addRoot(path: string) {
    try {
      const root = await api.addRoot(path);
      this.notify(`„${root.name}“ wird eingelesen …`);
      await this.loadRoots();
    } catch (e) {
      this.notify(String(e), "error");
    }
  }

  async removeRoot(id: number) {
    await api.removeRoot(id);
    if (this.scope.type === "folder") this.scope = { type: "all" };
    await Promise.all([this.loadRoots(), this.loadProjects()]);
    await this.loadImages();
  }

  async createProject(name: string): Promise<Project | null> {
    try {
      const p = await api.createProject(name);
      await this.loadProjects();
      return p;
    } catch (e) {
      this.notify(String(e), "error");
      return null;
    }
  }

  async addFolderToProject(projectId: number, path: string, recursive = true) {
    try {
      const p = await api.addProjectFolder(projectId, path, recursive);
      this.projects = this.projects.map((x) => (x.id === p.id ? p : x));
      this.notify(`Zu „${p.name}“ hinzugefügt`);
      if (this.scope.type === "project" && this.scope.id === projectId) await this.loadImages(true);
    } catch (e) {
      this.notify(String(e), "error");
    }
  }

  async removeFolderFromProject(projectId: number, path: string) {
    const p = await api.removeProjectFolder(projectId, path);
    this.projects = this.projects.map((x) => (x.id === p.id ? p : x));
    if (this.scope.type === "project" && this.scope.id === projectId) await this.loadImages(true);
  }

  async renameProject(id: number, name: string) {
    try {
      const old = this.projects.find((p) => p.id === id);
      const p = await api.updateProject(id, name, old?.description ?? "");
      this.projects = this.projects.map((x) => (x.id === p.id ? p : x));
    } catch (e) {
      this.notify(String(e), "error");
    }
  }

  async deleteProject(id: number) {
    await api.deleteProject(id);
    if (this.scope.type === "project" && this.scope.id === id) this.setScope({ type: "all" });
    await this.loadProjects();
  }

  notify(text: string, kind: "info" | "error" = "info") {
    this.message = { text, kind };
    clearTimeout(this.#messageTimer);
    this.#messageTimer = setTimeout(() => (this.message = null), kind === "error" ? 7000 : 3500);
  }
}

export const lib = new Library();
