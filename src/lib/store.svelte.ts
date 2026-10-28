// Zentraler Oberflächenzustand (Svelte-5-Runes).

import { isCanceled, lang, num, t, tc } from "./i18n";
import { api, listen, type AiStatus, type Person, type PersonGroup, type PeopleStatus, type AiDownloadEvent, type AiComponent, type DeleteReport, type RenameOptions, type FolderNode, type ImageSummary, type Project, type Root, type Scope, type SortOrder, type ScanProgress, type MergeKind, type MergeProgress, type ImportProgress, type ImportDone, type ImageQuery, type LocationItem, type MetaPatch, type MetaReport } from "./api";

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
  /** Dateityp-Filter: Endungen (klein), leer = alle */
  fileTypes = $state<string[]>([]);
  /** Nur bearbeitete (true) bzw. unbearbeitete (false) Fotos, null = alle */
  editedFilter = $state<boolean | null>(null);

  /** Flach (`raw`): Bei 50 000 Fotos kein Proxy je Bild; geändert wird nur durch Ersetzen. */
  images = $state.raw<ImageSummary[]>([]);
  loading = $state(false);
  /** Index des fokussierten Bildes in `images`. */
  focus = $state(-1);
  /** Mehrfachauswahl (IDs). */
  selection = $state<Set<number>>(new Set());
  view = $state<"grid" | "loupe" | "compare" | "people" | "map" | "album" | "stats">("grid");
  /** Zählt Änderungen an Aufnahmeorten (Infobereich lädt neu) */
  geoVersion = $state(0);
  #geoTimer: ReturnType<typeof setTimeout> | undefined;
  /** Bibliothek (Raster/Einzelansicht) oder Entwickeln. */
  mode = $state<"library" | "develop">("library");
  rowHeight = $state(200);

  // Zähler je Foto (neues Thumbnail, neue Gesichter, neue Schlagworte) als
  // einfache Map plus je ein gemeinsamer reaktiver Zähler: Ein `$state`-Objekt
  // legte beim ersten Import für jedes von 100 000 Fotos eigene Signale an.
  // Lesende Stellen laufen bei jeder Meldung neu, ändern aber nur dort etwas,
  // wo sich der Wert ihres Fotos geändert hat.
  #thumbV = new Map<number, number>();
  #thumbEpoch = $state(0);
  /** Erhöht sich, wenn ein Thumbnail neu erzeugt wurde – erzwingt Neuladen. */
  thumbVersion(id: number): number {
    void this.#thumbEpoch;
    return this.#thumbV.get(id) ?? 0;
  }
  scan = $state<ScanProgress | null>(null);
  /** Import-Dialog offen */
  importOpen = $state(false);
  /** Ein Import läuft (auch bei geschlossenem Dialog) */
  importRunning = $state(false);
  importProgress = $state<ImportProgress | null>(null);
  message = $state<{ text: string; kind: "info" | "error" } | null>(null);
  ai = $state<AiStatus | null>(null);
  people = $state<Person[]>([]);
  /** Personengruppen (Familie, Freunde …), alphabetisch */
  personGroups = $state<PersonGroup[]>([]);
  peopleStatus = $state<PeopleStatus | null>(null);
  #facesV = new Map<number, number>();
  #facesEpoch = $state(0);
  /** Erhöht sich, wenn Gesichter eines Bildes neu erkannt wurden. */
  facesVersion(id: number): number {
    void this.#facesEpoch;
    return this.#facesV.get(id) ?? 0;
  }
  aiDownload = $state<AiDownloadEvent | null>(null);
  /** Wird gerade geladen oder wartet darauf */
  aiPending(c: AiComponent): boolean {
    const d = this.aiDownload;
    return !!d && (d.component === c || d.queued.includes(c));
  }
  #tagsV = new Map<number, number>();
  #tagsEpoch = $state(0);
  /** Erhöht sich, wenn KI-Schlagworte eines Bildes neu sind. */
  tagsVersion(id: number): number {
    void this.#tagsEpoch;
    return this.#tagsV.get(id) ?? 0;
  }

  /** Zähler der Fotos `ids` erhöhen und die Lesenden einmal benachrichtigen. */
  static #bump(map: Map<number, number>, ids: number[]) {
    for (const id of ids) map.set(id, (map.get(id) ?? 0) + 1);
  }
  /** Erhöht sich, wenn die Bildliste neu vom Katalog geladen wurde (z. B. nach
   *  einem Rescan) – Details des fokussierten Fotos können sich geändert haben. */
  listVersion = $state(0);
  /** Erhöht sich, wenn sich Fotos oder ihre Reihenfolge ändern können – nicht
   *  beim Bewerten oder Markieren. Raster und Filmstreifen berechnen ihre
   *  Geometrie (O(n) über alle Fotos) nur dann neu. */
  layoutVersion = $state(0);

  focused = $derived(this.focus >= 0 ? this.images[this.focus] : undefined);

  scopeTitle = $derived.by(() => {
    const s = this.scope;
    if (s.type === "all") return t("Alle Fotos");
    if (s.type === "project") return this.projects.find((p) => p.id === s.id)?.name ?? t("Projekt");
    if (s.type === "person") return this.people.find((p) => p.id === s.id)?.name ?? t("Unbenannte Person");
    return s.path.split(/[\\/]/).pop() || s.path;
  });

  #queryTimer: ReturnType<typeof setTimeout> | undefined;
  #loadToken = 0;
  #messageTimer: ReturnType<typeof setTimeout> | undefined;

  async init() {
    // Meldungen aus dem Hintergrund in der Sprache der Oberfläche
    api.setLanguage(lang).catch(() => {});
    // Ein Fehler beim ersten Laden darf die Ereignisse unten nicht verhindern –
    // sonst bliebe die Oberfläche ohne Rückmeldung von Scans und Vorschauen
    try {
      await Promise.all([this.loadRoots(), this.loadProjects()]);
      await this.loadImages();
    } catch (e) {
      this.notify(String(e), "error");
    }

    listen("scan-progress", (p) => (this.scan = p));
    // GPS nachgetragen bzw. Ortsnamen ermittelt (Hintergrund): Liste gesammelt
    // neu laden – während des Nachtragens kommen die Meldungen in Stapeln
    listen("geo-changed", () => {
      clearTimeout(this.#geoTimer);
      this.#geoTimer = setTimeout(() => {
        this.geoVersion++;
        void this.loadImages(true);
      }, 3000);
    });
    listen("library-changed", async (r) => {
      this.scan = null;
      const changes = r.added + r.updated + r.moved + r.missing;
      const root = this.roots.find((x) => x.id === r.rootId)?.name ?? tc("one", "Ordner");
      const skipped = (r.unsupported ?? []).filter(([, n]) => n > 0);
      const skippedText = [
        skipped.length &&
          t("{n} Dateien übersprungen ({formats} – Format noch nicht unterstützt)", {
            n: skipped.reduce((s, [, n]) => s + n, 0),
            formats: skipped.map(([e]) => e.toUpperCase()).join(", "),
          }),
        r.unreadable && t("{n} Einträge nicht lesbar (Berechtigung? Details im Protokoll)", { n: r.unreadable }),
      ]
        .filter(Boolean)
        .join(". ");
      if (changes > 0) {
        const parts = [
          r.added && t("{n} neu", { n: r.added }),
          r.moved && t("{n} verschoben", { n: r.moved }),
          r.updated && t("{n} geändert", { n: r.updated }),
          r.missing && t("{n} fehlen", { n: r.missing }),
        ].filter(Boolean);
        this.notify(t("„{root}“ abgeglichen: {changes}", { root, changes: parts.join(", ") }) + (skippedText ? `. ${skippedText}` : ""));
      } else if ((r.requested || r.foldersChanged) && skippedText) {
        this.notify(t("„{root}“: keine neuen Fotos. {skipped}", { root, skipped: skippedText }));
      } else if (r.requested && !r.foldersChanged) {
        // Rückmeldung, dass „Neu einlesen“ gelaufen ist
        this.notify(t("„{root}“ neu eingelesen: keine Änderungen", { root }));
      }
      if (changes > 0 || r.foldersChanged) await Promise.all([this.loadRoots(), this.loadProjects()]);
      // Nichts an den Fotos geändert (z. B. nur eine XMP-Datei oder ein leerer
      // Ordner): die Liste mit bis zu 50 000 Fotos nicht neu laden
      if (changes > 0) this.refreshSoon();
    });
    listen("catalog-updated", () => this.refreshSoon());
    // Einzelne Schlüssel ändern statt das ganze Objekt zu kopieren (O(1) statt O(n))
    listen("thumbs-ready", ({ ids }) => {
      Library.#bump(this.#thumbV, ids);
      this.#thumbEpoch++;
    });
    listen("job-error", ({ message }) => this.notify(message, "error"));
    // KI-Agent (MCP): Schlagworte geändert, Foto zeigen
    listen("tags-changed", ({ ids }) => {
      Library.#bump(this.#tagsV, ids);
      this.#tagsEpoch++;
    });
    listen("mcp-show", ({ id, develop }) => this.showPhoto(id, develop));
    listen("import-progress", (p) => {
      if (this.importRunning) this.importProgress = p;
    });
    listen("import-done", (d) => this.#importDone(d));
    listen("merge-progress", (e) => {
      this.mergeProgress = e;
      if (e.finished) this.#mergeDone(e);
    });

    this.refreshAi();
    this.refreshPeople();
    api
      .appSettings()
      .then((s) => {
        this.mapEnabled = s.mapEnabled;
        this.exportSubfolder = s.exportSubfolder;
        this.includeSubfolders = s.includeSubfolders;
        this.previewCacheGb = s.previewCacheGb;
      })
      .catch(() => {});
    void this.refreshNcShared();
    void this.refreshFolderOptions();
    let peopleTimer: ReturnType<typeof setTimeout> | undefined;
    listen("faces-progress", ({ done, total, faces, ids }) => {
      if (this.peopleStatus) this.peopleStatus = { ...this.peopleStatus, analyzed: done, total, faces };
      Library.#bump(this.#facesV, ids);
      this.#facesEpoch++;
      // Personenliste gebündelt aktualisieren
      clearTimeout(peopleTimer);
      peopleTimer = setTimeout(() => this.refreshPeople(), 1500);
    });
    listen("ai-progress", ({ done, total, ids }) => {
      if (this.ai) this.ai = { ...this.ai, analyzed: done, total };
      Library.#bump(this.#tagsV, ids);
      this.#tagsEpoch++;
    });
    listen("ai-download", async (e) => {
      // Downloads laufen nacheinander: Erst wenn nichts mehr wartet, ist Schluss.
      // Ein Ereignis ohne Fortschritt (etwas wurde eingereiht) behält die Anzeige.
      const prev = this.aiDownload;
      this.aiDownload =
        e.finished && !e.queued.length ? null
        : e.finished ? { ...e, progress: null, finished: false, component: e.queued[0] }
        : { ...e, progress: e.progress ?? (prev?.component === e.component ? prev.progress : null) };
      if (e.finished) {
        if (e.error) this.notify(t("KI-Download: {error}", { error: e.error }), "error");
        else if (e.component === "clip") this.notify(t("KI ist bereit. Fotos werden jetzt im Hintergrund verschlagwortet."));
        else if (e.component === "animals") this.notify(t("Tierarten werden jetzt genauer bestimmt – schon analysierte Fotos im Hintergrund."));
        else this.notify(t("Download abgeschlossen."));
        await Promise.all([this.refreshAi(), this.refreshPeople()]);
      }
    });
  }

  async #importDone(d: ImportDone) {
    this.importRunning = false;
    this.importProgress = null;
    const r = d.report;
    if (d.error || !r) {
      this.notify(d.error ?? t("Import fehlgeschlagen"), "error");
      return;
    }
    const parts = [
      t(r.imported === 1 ? "{n} Foto importiert" : "{n} Fotos importiert", { n: r.imported }),
      r.existing && t("{n} waren schon da", { n: r.existing }),
      r.unshifted && t("bei {n} Fotos fand sich keine Zeitangabe zum Korrigieren (nur Name und Ordner angepasst)", { n: r.unshifted }),
      r.deleted && t("{n} Dateien von der Karte gelöscht", { n: r.deleted }),
      r.failed.length && t("{n} Fehler (Details im Protokoll)", { n: r.failed.length }),
      r.canceled && t("abgebrochen"),
      d.outsideLibrary && t("Das Ziel liegt außerhalb deiner Quellordner – füge es unter „Ordner“ hinzu, um die Fotos zu sehen."),
    ].filter(Boolean);
    this.notify(parts.join(" · "), r.failed.length || d.outsideLibrary ? "error" : "info");
    if (d.rootAdded) await Promise.all([this.loadRoots(), this.loadProjects()]);
  }

  async refreshPeople() {
    const token = ++this.#peopleToken;
    try {
      const [people, status, groups] = await Promise.all([api.peopleList(), api.peopleStatus(), api.personGroups()]);
      // Veraltete Antwort (neuere Anfrage läuft) verwerfen
      if (token !== this.#peopleToken) return;
      [this.people, this.peopleStatus, this.personGroups] = [people, status, groups];
    } catch {
      // Alten Stand behalten
    }
  }
  #peopleToken = 0;

  /** Änderung an Personengruppen ausführen, Fehler melden, neu laden. */
  async groupAction<T>(f: () => Promise<T>): Promise<T | null> {
    let r: T | null = null;
    try {
      r = await f();
    } catch (e) {
      this.notify(String(e), "error");
    }
    await this.refreshPeople();
    return r;
  }

  async refreshAi() {
    try {
      this.ai = await api.aiStatus();
      if (!this.ai.modelReady) this.semantic = false;
    } catch {
      this.ai = null;
    }
  }

  /** Ordner und ihre Bäume laden; `only`: nur die Bäume dieser Ordner neu laden
   *  (je Ordner eine Anfrage), die übrigen bleiben. */
  async loadRoots(only?: Set<number>) {
    const token = ++this.#rootsToken;
    const roots = await api.listRoots();
    const trees: Record<number, FolderNode> = {};
    await Promise.all(
      roots.map(async (r) => {
        const old = only && !only.has(r.id) ? this.trees[r.id] : undefined;
        trees[r.id] = old ?? (await api.folderTree(r.id));
      }),
    );
    // Nur die neueste Antwort übernehmen; ein Fehler lässt den alten Stand stehen
    if (token !== this.#rootsToken) return;
    this.roots = roots;
    this.trees = trees;
  }
  #rootsToken = 0;

  async loadProjects() {
    const token = ++this.#projectsToken;
    const projects = await api.listProjects();
    if (token === this.#projectsToken) this.projects = projects;
  }
  #projectsToken = 0;

  /** Lädt die Fotos der Ansicht. `keepFocus`: fokussiertes Foto (und Auswahl)
   *  behalten, soweit noch gelistet. `developFirst`: im Entwickeln-Modus ohne
   *  behaltenes Foto das erste öffnen, statt zur Bibliothek zu wechseln. */
  async loadImages(keepFocus = false, developFirst = false) {
    // Anfragen laufen parallel: Nur das Ergebnis der neuesten zählt
    // (sonst kann die Suche nach „so“ die nach „sonne“ überholen).
    const token = ++this.#loadToken;
    this.loading = true;
    let counted = false;
    try {
      // Erst laufende Bewertungen schreiben lassen – sonst liefert der Katalog
      // noch den alten Stand und überschreibt die gerade gesetzte Bewertung
      await this.#writes;
      const startSeq = this.#patchSeq;
      this.#loadsInFlight++;
      counted = true;
      const query = this.currentQuery();
      let images =
        this.semantic && query.search
          ? await api.semanticSearch(query.search, query.scope, query.minRating ?? null, query.exts ?? null, query.edited ?? null)
          : await api.queryImages(query);
      if (token !== this.#loadToken) return;
      // Während der Abfrage Bewertetes/Markiertes erneut anwenden
      const later = this.#patchLog.filter((p) => p.seq > startSeq);
      this.#patchLog = later;
      if (later.length) {
        images = images.map((i) => later.reduce((acc, p) => (p.ids.has(i.id) ? { ...acc, ...(p.change instanceof Map ? p.change.get(i.id) : p.change) } : acc), i));
      }
      // Fokus erst jetzt lesen: Während der Anfrage kann weitergeblättert worden sein
      const focusId = keepFocus ? this.focused?.id : undefined;
      this.images = images;
      this.rejectedCount = countRejected(images);
      this.listVersion++;
      this.layoutVersion++;
      this.focus = focusId !== undefined ? this.images.findIndex((i) => i.id === focusId) : -1;
      if (!keepFocus) this.selection = new Set();
      else if (this.selection.size) {
        // Nicht mehr gelistete Fotos aus der Auswahl nehmen (Export, Einfügen …)
        const ids = new Set(images.map((i) => i.id));
        const kept = [...this.selection].filter((id) => ids.has(id));
        if (kept.length !== this.selection.size) this.selection = new Set(kept);
      }
      if (this.focus < 0 && developFirst && this.mode === "develop" && images.length) {
        this.focus = 0;
        this.selection = new Set([images[0].id]);
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
      // Ohne laufende Abfrage braucht niemand das Änderungsprotokoll
      if (counted && --this.#loadsInFlight === 0) this.#patchLog = [];
    }
  }

  /** Anzahl laufender Listenabfragen (nur dann wird `#patchLog` geführt) */
  #loadsInFlight = 0;

  #refreshTimer: ReturnType<typeof setTimeout> | undefined;
  /** Gebündeltes Nachladen mit erhaltenem Fokus (Meldungen aus dem
   *  Hintergrund kommen oft schubweise – nicht jede lädt die ganze Liste). */
  refreshSoon() {
    clearTimeout(this.#refreshTimer);
    this.#refreshTimer = setTimeout(() => this.loadImages(true), 250);
  }

  /** Verzögertes Neuladen, z. B. beim Tippen in der Suche. */
  reloadSoon() {
    clearTimeout(this.#queryTimer);
    this.#queryTimer = setTimeout(() => this.loadImages(), 180);
  }

  setScope(scope: Scope) {
    // Rücksprung zur Gesichter-Prüfung nur für die Person, deren Fotos man öffnet
    if (scope.type !== "person" || scope.id !== this.reviewPerson) this.reviewPerson = null;
    this.scope = scope;
    this.view = "grid";
    // Im Entwickeln-Modus bleiben: das geöffnete Foto, falls es auch hier
    // liegt (z. B. übergeordneter Ordner), sonst das erste der neuen Ansicht
    if (this.mode === "develop") this.loadImages(true, true);
    else this.loadImages();
  }

  /** Foto zeigen (für KI-Agenten): in der aktuellen Liste, sonst unter
   *  „Alle Fotos“ ohne Filter; mit `develop` gleich im Entwickeln-Modus. */
  async showPhoto(id: number, develop = false) {
    let index = this.images.findIndex((i) => i.id === id);
    if (index < 0) {
      this.search = "";
      this.minRating = 0;
      this.fileTypes = [];
      this.editedFilter = null;
      this.scope = { type: "all" };
      await this.loadImages();
      index = this.images.findIndex((i) => i.id === id);
    }
    if (index < 0) return;
    this.select(index);
    if (develop) {
      this.mode = "develop";
    } else {
      this.mode = "library";
      this.view = "loupe";
    }
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

  // --- Ordner-Mehrfachauswahl (Seitenleiste) --------------------------------

  /** Per Strg-/Umschalt-Klick gewählte Ordner, z. B. um sie gemeinsam einem
   *  Projekt zuzuordnen. Leer: keine Mehrfachauswahl. */
  folderSel = $state<Set<string>>(new Set());
  #folderAnchor: string | null = null;

  /** Klick auf einen Ordner: `toggle` (Strg) nimmt ihn dazu/heraus, `range`
   *  (Umschalt) wählt alle sichtbaren Ordner zwischen Anker und Klick (`order`). */
  selectFolder(path: string, mode: "toggle" | "range", order: string[]) {
    const s = new Set(this.folderSel);
    // Der gerade angezeigte Ordner zählt als Startpunkt der Auswahl
    if (!s.size && this.scope.type === "folder" && this.scope.path !== path) {
      s.add(this.scope.path);
      this.#folderAnchor ??= this.scope.path;
    }
    if (mode === "range" && this.#folderAnchor && order.includes(this.#folderAnchor)) {
      const [a, b] = [order.indexOf(this.#folderAnchor), order.indexOf(path)].sort((x, y) => x - y);
      for (const p of order.slice(a, b + 1)) s.add(p);
    } else {
      s.has(path) ? s.delete(path) : s.add(path);
      this.#folderAnchor = path;
    }
    this.folderSel = s;
  }

  clearFolderSel() {
    if (this.folderSel.size) this.folderSel = new Set();
    this.#folderAnchor = null;
  }

  /** Mehrere Ordner (mit Unterordnern) einem Projekt zuordnen. Unterordner
   *  eines ebenfalls gewählten Ordners sind schon enthalten. */
  async addFoldersToProject(projectId: number, paths: string[]) {
    const top = topFolders(paths);
    const project = this.projects.find((p) => p.id === projectId);
    const todo = top.filter((p) => !project?.folders.some((f) => f.path === p));
    if (!todo.length) {
      this.notify(t("Alle gewählten Ordner sind schon in „{name}“", { name: project?.name ?? t("dem Projekt") }));
      return;
    }
    try {
      let p: Project | null = null;
      for (const path of todo) p = await api.addProjectFolder(projectId, path, true);
      this.projects = this.projects.map((x) => (x.id === p!.id ? p! : x));
      this.notify(
        todo.length === 1
          ? t("Zu „{name}“ hinzugefügt", { name: p!.name })
          : t("{n} Ordner zu „{name}“ hinzugefügt", { n: todo.length, name: p!.name }),
      );
      if (this.scope.type === "project" && this.scope.id === projectId) await this.loadImages(true);
    } catch (e) {
      // Bis zum Fehler hinzugefügte Ordner anzeigen
      await this.loadProjects();
      this.notify(String(e), "error");
    }
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
  compareItems = $derived.by(() => {
    // Schleife mit Abbruch statt map/filter über alle (bis 50 000) Fotos –
    // läuft nach jedem Bewerten im Vergleich erneut
    const sel = this.selection;
    const max = Math.min(12, sel.size);
    const out: { im: ImageSummary; index: number }[] = [];
    const images = this.images;
    for (let index = 0; index < images.length && out.length < max; index++) {
      if (sel.has(images[index].id)) out.push({ im: images[index], index });
    }
    return out;
  });

  /** Vergleichsansicht der ausgewählten Bilder öffnen. */
  compare() {
    if (this.selection.size < 2) {
      this.notify(t("Für den Vergleich mindestens zwei Fotos auswählen (Strg- oder Umschalt-Klick)."));
      return;
    }
    if (this.selection.size > 12) this.notify(t("Verglichen werden die ersten 12 ausgewählten Fotos."));
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

  /** Klick im Fotostreifen des Vergleichs. Ein verglichenes Foto wird
   *  hervorgehoben, ein anderes ersetzt das hervorgehobene. Mit `add`
   *  (Strg-/Umschalt-Klick) kommt es stattdessen dazu bzw. ein verglichenes
   *  wird herausgenommen. */
  compareStrip(index: number, add: boolean) {
    const img = this.images[index];
    if (!img) return;
    if (add) {
      if (this.selection.has(img.id)) {
        this.removeFromCompare(img.id);
      } else if (this.compareItems.length >= 12) {
        this.notify(t("Im Vergleich sind höchstens 12 Fotos möglich."));
      } else {
        this.selection = new Set(this.selection).add(img.id);
        this.focus = index;
      }
      return;
    }
    if (!this.compareItems.some((x) => x.index === index)) {
      const s = new Set(this.selection);
      if (this.focused) s.delete(this.focused.id);
      s.add(img.id);
      this.selection = s;
    }
    this.focus = index;
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
  /** Hochzählen: Update-Prüfung jetzt (Einstellungen → Allgemein) */
  updateRequest = $state(0);

  /** Öffnet den Dialog zum Zusammenfügen der ausgewählten Fotos. */
  openMerge(kind: MergeKind) {
    if (this.mergeProgress && !this.mergeProgress.finished) {
      this.notify(t("Es werden bereits Fotos zusammengefügt."));
      return;
    }
    const sel = this.selection;
    // Reihenfolge wie angezeigt (für Focus Stacking: Schärfeebene nah → fern)
    const ids = this.images.filter((i) => sel.has(i.id)).map((i) => i.id);
    if (ids.length < 2) {
      this.notify(t("Zum Zusammenfügen mindestens zwei Fotos auswählen (Strg- oder Umschalt-Klick)."));
      return;
    }
    this.merge = { kind, ids };
  }

  /** Ergebnis fertig: Liste neu laden und das neue Foto auswählen. */
  async #mergeDone(e: MergeProgress) {
    if (e.error) {
      if (!isCanceled(e.error)) this.notify(t("Zusammenfügen fehlgeschlagen: {error}", { error: e.error }), "error");
      return;
    }
    await this.loadImages(true);
    const index = e.imageId === null ? -1 : this.images.findIndex((i) => i.id === e.imageId);
    if (index >= 0) {
      this.select(index);
      this.notify(t("{file} erstellt", { file: e.fileName ?? "" }));
    } else {
      this.notify(t("{file} erstellt (in dieser Ansicht nicht sichtbar)", { file: e.fileName ?? "" }));
    }
    // Nicht ausgerichtete Fotos: Das Ergebnis kann dort Doppelkanten zeigen
    const off = e.unaligned ?? [];
    if (off.length) {
      this.notify(
        t(off.length === 1 ? "Foto {list} ließ sich nicht ausrichten – das Ergebnis kann dort versetzt sein." : "Fotos {list} ließen sich nicht ausrichten – das Ergebnis kann dort versetzt sein.", {
          list: off.join(", "),
        }),
        "error",
      );
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
  #patchSeq = 0;
  /** Änderungen seit dem Start der letzten Listenabfrage (siehe `loadImages`) */
  #patchLog: { seq: number; ids: Set<number>; change: Partial<ImageSummary> | Map<number, Partial<ImageSummary>> }[] = [];

  /** `change` gilt für alle `ids`, oder je Foto eigene Werte (Map, `ids` = deren Schlüssel). */
  #patch(ids: number[], change: Partial<ImageSummary> | Map<number, Partial<ImageSummary>>) {
    const set = new Set(ids);
    ++this.#patchSeq;
    if (this.#loadsInFlight > 0) {
      this.#patchLog.push({ seq: this.#patchSeq, ids: set, change });
      if (this.#patchLog.length > 500) this.#patchLog.splice(0, this.#patchLog.length - 500);
    }
    let delta = 0; // Abgelehnt-Zähler im selben Durchlauf nachführen
    this.images = this.images.map((i) => {
      if (!set.has(i.id)) return i;
      const n = { ...i, ...(change instanceof Map ? change.get(i.id) : change) };
      delta += Number(isRejected(n)) - Number(isRejected(i));
      return n;
    });
    if (delta) this.rejectedCount += delta;
    // Ortsänderungen sofort für die Karte melden (sie hört auf geoVersion)
    if (change instanceof Map || "lat" in change || "lon" in change) this.geoVersion++;
  }

  /** Von außen (Entwickeln): Felder der Fotos `ids` ändern und im Protokoll
   *  vermerken, damit eine gleichzeitig laufende Listenabfrage sie nicht mit
   *  altem Stand überschreibt. `layoutVersion` ruft der Aufrufer selbst hoch. */
  patchImages(ids: number[], change: Partial<ImageSummary> | Map<number, Partial<ImageSummary>>) {
    this.#patch(ids, change);
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

  /** Anzahl abgelehnter (und vorhandener) Bilder des aktuellen Bereichs. Wird
   *  beim Bewerten/Markieren nachgeführt statt bei jedem Tastendruck alle
   *  (bis 50 000) Fotos zu filtern; nur beim Laden/Löschen ganz gezählt.
   *  Wer `images` mit geänderter Markierung setzt, muss ihn mitführen
   *  (develop.svelte.ts ändert nur `edited` – zählt nicht). */
  rejectedCount = $state(0);

  /** Abgelehnte (und vorhandene) Bilder des aktuellen Bereichs, bei Bedarf gebaut. */
  rejectedList(): ImageSummary[] {
    return this.images.filter(isRejected);
  }

  /**
   * Umschalt+X: ablehnen (ohne Umschalten) und zum nächsten Bild springen.
   * Im Vergleich weiter zum nächsten verglichenen Bild, im Raster mit
   * Mehrfachauswahl hinter das letzte ausgewählte Bild.
   */
  /** Fotos `ids` als abgelehnt markieren (Aussortier-Assistent) */
  async rejectIds(ids: number[]) {
    if (!ids.length) return;
    this.#patch(ids, { flag: -1 });
    await this.#write(() => api.setFlag(ids, -1)).catch((e) => this.#writeFailed(e));
  }

  /** Offener Aussortier-Assistent */
  cull = $state<{ scope: Scope; title: string } | null>(null);

  async rejectAndAdvance() {
    const ids = this.#rateTargets();
    if (!ids.length) return;
    // Wird ohne await aufgerufen: Fehler hier melden statt als unbehandelte
    // Ablehnung zu verpuffen; bei Fehler nicht weiterspringen.
    try {
      await this.#write(() => api.setFlag(ids, -1));
    } catch (e) {
      this.#writeFailed(e);
      return;
    }
    this.#patch(ids, { flag: -1 });

    if (this.mode === "library" && this.view === "compare") {
      const items = this.compareItems;
      const pos = items.findIndex((x) => x.index === this.focus);
      if (pos >= 0 && pos < items.length - 1) this.focus = items[pos + 1].index;
      else this.notify(t("Letztes Foto im Vergleich"));
      return;
    }
    const set = new Set(ids);
    let last = -1;
    this.images.forEach((im, idx) => {
      if (set.has(im.id)) last = idx;
    });
    if (last >= this.images.length - 1) {
      this.notify(t("Letztes Foto erreicht"));
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
    // Betroffene Ordner, um nur deren Baum (Anzahlen) neu zu laden
    const affected = new Set<number>();
    let unknownRoot = false;
    for (const im of this.images) {
      if (!gone.has(im.id)) continue;
      const r = this.roots.filter((r) => inside(im.path, r.path));
      if (!r.length) unknownRoot = true;
      for (const x of r) affected.add(x.id);
    }
    const focusId = this.focused?.id;
    // Gelöschte vor dem Fokus verschieben ihn nach vorn
    const before = this.images.slice(0, Math.max(this.focus, 0)).filter((i) => gone.has(i.id)).length;
    const oldFocus = this.focus - before;
    this.images = this.images.filter((i) => !gone.has(i.id));
    this.layoutVersion++;
    this.rejectedCount = countRejected(this.images);
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

    const mb = num(report.bytes / 1e6, { maximumFractionDigits: report.bytes < 1e7 ? 2 : 0 });
    const k = report.deleted.length;
    const photos = t(k === 1 ? "{n} Foto" : "{n} Fotos", { n: k });
    if (report.failed.length) {
      const vars = { photos, failed: report.failed.length, error: report.failed[0].error };
      this.notify(
        report.toTrash
          ? t("{photos} in den Papierkorb verschoben, {failed} nicht: {error}", vars)
          : t("{photos} endgültig gelöscht, {failed} nicht: {error}", vars),
        "error",
      );
    } else {
      this.notify(
        report.toTrash
          ? t("{photos} in den Papierkorb verschoben ({mb} MB)", { photos, mb })
          : t("{photos} endgültig gelöscht ({mb} MB)", { photos, mb }),
      );
    }
    if (gone.size) this.loadRoots(unknownRoot ? undefined : affected).catch(() => {});
    this.refreshPeople();
    return report;
  }

  /** Fotos umbenennen (der Dialog hat Vorschau und Konflikte schon gezeigt;
   *  das Backend prüft erneut). `true`, wenn umbenannt wurde. */
  async renameImages(ids: number[], options: RenameOptions): Promise<boolean> {
    await this.#writes; // eben gedrückte Markierungen zuerst speichern
    let report;
    try {
      report = await api.renameApply(ids, options);
    } catch (e) {
      this.notify(String(e), "error");
      return false;
    }
    const changes = new Map(report.renamed.map((r) => [r.id, { path: r.path, fileName: r.fileName }]));
    if (changes.size) {
      this.#patch([...changes.keys()], changes);
      this.layoutVersion++;
      this.listVersion++; // Informationen (Dateiname, Pfad) neu laden
      // Reihenfolge nach Name und Suche nach dem Dateinamen ändern sich
      this.loadImages(true).catch(() => {});
    }
    const k = changes.size;
    this.notify(k ? t(k === 1 ? "{n} Foto umbenannt" : "{n} Fotos umbenannt", { n: k }) : t("Die Namen waren schon so."));
    return true;
  }

  // --- Ordner & Projekte ----------------------------------------------------

  async addRoot(path: string) {
    try {
      const root = await api.addRoot(path);
      this.notify(t("„{name}“ wird eingelesen …", { name: root.name }));
      await this.loadRoots();
    } catch (e) {
      this.notify(String(e), "error");
    }
  }

  async removeRoot(id: number) {
    try {
      await api.removeRoot(id);
      if (this.scope.type === "folder") this.scope = { type: "all" };
      await Promise.all([this.loadRoots(), this.loadProjects()]);
      await this.loadImages();
    } catch (e) {
      this.notify(String(e), "error");
    }
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

  /** Ordner zum Projekt hinzufügen. Ist er schon enthalten, wird nur
   *  `recursive` gesetzt – so schaltet ProjectFolders „Unterordner“ um. */
  async addFolderToProject(projectId: number, path: string, recursive = true) {
    try {
      const p = await api.addProjectFolder(projectId, path, recursive);
      this.projects = this.projects.map((x) => (x.id === p.id ? p : x));
      this.notify(t("Zu „{name}“ hinzugefügt", { name: p.name }));
      if (this.scope.type === "project" && this.scope.id === projectId) await this.loadImages(true);
    } catch (e) {
      this.notify(String(e), "error");
    }
  }

  async removeFolderFromProject(projectId: number, path: string) {
    try {
      const p = await api.removeProjectFolder(projectId, path);
      this.projects = this.projects.map((x) => (x.id === p.id ? p : x));
      if (this.scope.type === "project" && this.scope.id === projectId) await this.loadImages(true);
    } catch (e) {
      this.notify(String(e), "error");
    }
  }

  /** Person umbenennen (z. B. „Max“ → „Max Mustermann“). Gibt es den Namen
   *  schon bei einer anderen Person, werden beide nach Rückfrage zusammengeführt. */
  /** Benennt eine Person bzw. Gruppe um. Gibt es den Namen schon, wird sie
   *  dieser Person zugeordnet (bei benannten Personen erst nach Rückfrage).
   *  Liefert die ID der Person, die danach die Gesichter hat, sonst `null`. */
  async renamePerson(id: number, name: string): Promise<number | null> {
    name = name.trim();
    const old = this.people.find((p) => p.id === id);
    if (!name || name === old?.name) return null;
    const other = this.people.find((p) => p.id !== id && p.name?.toLocaleLowerCase("de") === name.toLocaleLowerCase("de"));
    // Unbenannte Gruppe + vorhandener Name = Zuordnung, wie in der Übersicht ohne Rückfrage
    if (other && old?.name && !confirm(t("„{name}“ gibt es schon. Beide Personen zu einer zusammenführen?", { name: other.name ?? "" }))) return null;
    try {
      const target = await api.personRename(id, name);
      await this.refreshPeople();
      // Ansicht der umbenannten Person: bei Zusammenführung zur Zielperson wechseln
      if (this.scope.type === "person" && this.scope.id === id && target !== id) {
        this.setScope({ type: "person", id: target });
      }
      this.notify(
        other
          ? old?.name
            ? t("Mit „{name}“ zusammengeführt", { name: other.name ?? "" })
            : t("„{name}“ zugeordnet", { name: other.name ?? "" })
          : old?.name
            ? t("„{old}“ heißt jetzt „{name}“", { old: old.name, name })
            : t("„{name}“ gespeichert", { name }),
      );
      return target;
    } catch (e) {
      this.notify(String(e), "error");
      return null;
    }
  }

  /** Gesichter-Prüfung dieser Person war offen, als ihre Fotos geöffnet wurden
   *  („Zurück zu Personen“ kehrt dorthin zurück). */
  reviewPerson = $state<number | null>(null);

  /** Zur Personenübersicht (bzw. zur zuletzt geprüften Person). */
  openPeople() {
    this.mode = "library";
    this.view = "people";
  }

  openMap() {
    if (!this.mapEnabled) return;
    this.mode = "library";
    this.view = "map";
  }

  /** Karte und Ortsnamen über OpenStreetMap (Einstellungen → Allgemein) */
  mapEnabled = $state(true);

  /** Unterordner in die Bibliothek aufnehmen: Standard und je Ordner */
  includeSubfolders = $state(true);
  folderOptions = $state<Record<string, boolean>>({});

  async refreshFolderOptions() {
    const list = await api.folderOptions().catch(() => [] as [string, boolean][]);
    this.folderOptions = Object.fromEntries(list.map(([p, v]) => [folderKey(p), v]));
  }

  /** Nimmt der Ordner seine Unterordner auf (eigene Einstellung oder Standard)? */
  subfoldersIncluded(path: string): boolean {
    return this.folderOptions[folderKey(path)] ?? this.includeSubfolders;
  }

  async setFolderSubfolders(path: string, include: boolean) {
    try {
      await api.folderSetSubfolders(path, include);
      this.folderOptions = { ...this.folderOptions, [folderKey(path)]: include };
      this.notify(include ? t("Unterordner werden eingelesen …") : t("Unterordner werden aus der Bibliothek genommen …"));
    } catch (e) {
      this.notify(String(e), "error");
    }
  }

  async setIncludeSubfolders(on: boolean) {
    try {
      const s = await api.appSetSettings({ includeSubfolders: on });
      this.includeSubfolders = s.includeSubfolders;
    } catch (e) {
      this.notify(String(e), "error");
    }
  }

  /** In Nextcloud geteilte Projekte: Projekt-ID → Link */
  ncShared = $state<Record<number, string>>({});

  async refreshNcShared() {
    this.ncShared = await api.ncSharedProjects().catch(() => ({}));
  }

  /** Exportziel ohne gewählten Ordner: relativ zum Ordner des Originals
   *  („.“ = derselbe Ordner) */
  exportSubfolder = $state("Export");

  /** Beschreibung dieses Ziels für Dialoge */
  get exportSubfolderLabel(): string {
    return this.exportSubfolder === "."
      ? t("Ordner des jeweiligen Originals")
      : t("Unterordner „{name}“ neben dem Original", { name: this.exportSubfolder });
  }

  /** Liefert eine Fehlermeldung, wenn der Pfad nicht gültig ist */
  async setExportSubfolder(sub: string): Promise<string | null> {
    try {
      const s = await api.appSetSettings({ exportSubfolder: sub });
      this.exportSubfolder = s.exportSubfolder;
      return null;
    } catch (e) {
      return e instanceof Error ? e.message : String(e);
    }
  }

  /** Grenze für die Großvorschauen auf der Platte (GB) */
  previewCacheGb = $state(5);

  async setPreviewCacheGb(gb: number) {
    if (!Number.isFinite(gb)) return;
    try {
      const s = await api.appSetSettings({ previewCacheGb: Math.round(gb) });
      this.previewCacheGb = s.previewCacheGb;
    } catch (e) {
      this.notify(String(e), "error");
    }
  }

  async setMapEnabled(on: boolean) {
    await api.appSetSettings({ mapEnabled: on });
    this.mapEnabled = on;
    if (!on && this.view === "map") this.view = "grid";
  }

  /** Aufnahmeort für Fotos setzen; ohne `place` ermittelt ihn das Backend. */
  async setLocation(ids: number[], lat: number, lon: number, place: string | null = null) {
    if (!ids.length) return;
    this.#patch(ids, { lat, lon });
    await this.#write(() => api.geoSetLocation(ids, lat, lon, place)).catch((e) => this.#writeFailed(e));
    this.geoVersion++;
  }

  /** Metadaten (Titel, Aufnahmezeit, Kamera …) ändern: im Katalog und in der
   *  XMP-Begleitdatei. `shiftSeconds`: Aufnahmezeit zusätzlich verschieben.
   *  Fotos, bei denen es nicht ging, meldet das Ergebnis; die übrigen sind
   *  gespeichert und werden in Liste, Info und Statistik aktualisiert. */
  async editMeta(ids: number[], patch: MetaPatch, shiftSeconds = 0): Promise<MetaReport> {
    const report: MetaReport = { changed: [], failed: [] };
    try {
      await this.#write(async () => {
        const parts: MetaReport[] = [];
        if (Object.keys(patch).length) parts.push(await api.metaSet(ids, patch));
        if (shiftSeconds) parts.push(await api.metaShiftTime(ids, shiftSeconds));
        for (const p of parts) {
          report.changed.push(...p.changed.filter((id) => !report.changed.includes(id)));
          report.failed.push(...p.failed);
        }
      });
    } catch (e) {
      this.notify(String(e), "error");
    }
    if (report.changed.length) await this.loadImages(true);
    return report;
  }

  /** Abfrage der aktuellen Ansicht (Ordner, Projekt, Suche, Bewertung, Dateityp) */
  currentQuery(): ImageQuery {
    const text = this.search.trim();
    return {
      scope: $state.snapshot(this.scope) as Scope,
      sort: this.sort,
      search: text || null,
      minRating: this.minRating || null,
      exts: this.fileTypes.length ? [...this.fileTypes] : null,
      edited: this.editedFilter,
    };
  }

  /** Ein Filter (Suche, Sterne, Bearbeitung, Dateityp) ist gesetzt */
  get filtered(): boolean {
    return !!(this.search.trim() || this.minRating || this.fileTypes.length || this.editedFilter !== null);
  }

  /** Suche, Mindestbewertung, Bearbeitung und Dateityp zurücksetzen */
  resetFilters() {
    this.search = "";
    this.minRating = 0;
    this.fileTypes = [];
    this.editedFilter = null;
    void this.loadImages();
  }

  /** Vorschläge übernehmen: je Foto eigener Ort */
  async applyLocations(items: LocationItem[]) {
    if (!items.length) return;
    this.#patch(
      items.map((i) => i.id),
      new Map(items.map((i) => [i.id, { lat: i.lat, lon: i.lon }])),
    );
    await this.#write(() => api.geoApply(items)).catch((e) => this.#writeFailed(e));
    this.geoVersion++;
  }

  async clearLocation(ids: number[]) {
    if (!ids.length) return;
    this.#patch(ids, { lat: undefined, lon: undefined });
    await this.#write(() => api.geoClearLocation(ids)).catch((e) => this.#writeFailed(e));
    this.geoVersion++;
  }

  /** In der Personenübersicht ist die Gesichter-Prüfung einer Person offen
   *  (setzt PeopleView) – die Werkzeugleiste zeigt dann „‹ Personen“. */
  peopleReviewOpen = $state(false);
  /** Zähler: Werkzeugleiste bittet PeopleView, die Prüfung zu schließen */
  peopleBack = $state(0);

  // --- Seitenleiste ----------------------------------------------------------

  /** Eingeklappte Bereiche der Seitenleiste (bleibt über Neustarts erhalten) */
  collapsed = $state<Record<string, boolean>>(loadCollapsed());

  /** Eingeklappt: Bereiche der Seitenleiste und Personengruppen (`pgroup:<id>`) */
  toggleSection(key: "projects" | "people" | "folders" | `pgroup:${number | "other"}` | `sgroup:${number | "other"}`) {
    this.collapsed = { ...this.collapsed, [key]: !this.collapsed[key] };
    try {
      localStorage.setItem("lumenkorn.sidebar.collapsed", JSON.stringify(this.collapsed));
    } catch {
      // ohne Speicher gilt die Einstellung nur bis zum Neustart
    }
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
    try {
      await api.deleteProject(id);
      if (this.scope.type === "project" && this.scope.id === id) this.setScope({ type: "all" });
      await this.loadProjects();
    } catch (e) {
      this.notify(String(e), "error");
    }
  }

  notify(text: string, kind: "info" | "error" = "info") {
    this.message = { text, kind };
    clearTimeout(this.#messageTimer);
    this.#messageTimer = setTimeout(() => (this.message = null), kind === "error" ? 7000 : 3500);
  }
}

export const lib = new Library();

function loadCollapsed(): Record<string, boolean> {
  try {
    const v = JSON.parse(localStorage.getItem("lumenkorn.sidebar.collapsed") ?? "{}");
    return v && typeof v === "object" ? v : {};
  } catch {
    return {};
  }
}

/** Vergleichbarer Ordnerpfad (ohne Trenner am Ende, ohne Groß-/Kleinschreibung) */
const folderKey = (p: string) => p.replace(/[\\/]+$/, "").toLowerCase();

const isRejected = (i: ImageSummary) => i.flag === -1 && !i.missing;
function countRejected(images: ImageSummary[]): number {
  let n = 0;
  for (const i of images) if (isRejected(i)) n++;
  return n;
}

/** Liegt `child` (Datei oder Ordner) unterhalb von `parent`? */
function inside(child: string, parent: string): boolean {
  const p = parent.replace(/[\\/]+$/, "");
  return child.length > p.length && child.startsWith(p) && /[\\/]/.test(child[p.length]);
}

/** Ordner ohne die, die schon in einem anderen gewählten Ordner liegen. */
export function topFolders(paths: string[]): string[] {
  const unique = [...new Set(paths)];
  return unique.filter((c) => !unique.some((p) => p !== c && inside(c, p)));
}
