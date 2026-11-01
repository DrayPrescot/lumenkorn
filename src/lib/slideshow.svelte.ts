// Zustand der Diashow: Erstellen (Auswahl im Backend wie beim Fotoalbum),
// Bearbeiten in der Zeitleiste, Abspielen. Die Diashow eines Bereichs bleibt im
// Speicher der WebView erhalten und lässt sich später weiterbearbeiten.

import { api, isTauri, listen, pickMusicFile, pickVideoTarget, type Scope } from "./api";
import { renderVideo, type VideoProgress, type VideoSize } from "./slideshowVideo";
import { downmix, type BeatInfo } from "./beats";
import type { CaptionMode } from "./album";
import {
  autoCaptions,
  buildSlideshow,
  clampSeconds,
  slideshowDefaults,
  validSlideshow,
  type Slide,
  type SlideshowSettings,
  type SlideshowMusic,
  type SlideshowState,
  type Transition,
} from "./slideshow";
import type { AlbumCandidate } from "./album";
import { lib } from "./store.svelte";
import { t } from "./i18n";

const SETTINGS_KEY = "lumenkorn.slideshow.settings";
const showKey = (scope: Scope) => `lumenkorn.slideshow.${JSON.stringify(scope)}`;

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

/** Projekte mit gespeicherter Diashow (aus den Schlüsseln im Speicher) */
function savedProjects(): Record<number, true> {
  const out: Record<number, true> = {};
  try {
    const prefix = showKey({ type: "project", id: 0 }).replace(/0\}$/, "");
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      const id = k?.startsWith(prefix) ? Number(k.slice(prefix.length, -1)) : NaN;
      if (Number.isInteger(id)) out[id] = true;
    }
  } catch {
    // ohne Speicher keine Symbole
  }
  return out;
}

class SlideshowStore {
  state = $state<SlideshowState | null>(null);
  /** Projekte, für die eine Diashow gespeichert ist (Symbol in der Seitenleiste) */
  projects = $state<Record<number, true>>(savedProjects());
  /** Einstellungsdialog offen (mit dem Bereich, für den erstellt wird) */
  dialog = $state<{ scope: Scope; title: string } | null>(null);
  /** Fortschritt beim Auswählen (Vorschauen messen) */
  progress = $state<{ done: number; total: number } | null>(null);
  /** Gewähltes Dia (Index) */
  selection = $state<number | null>(null);
  /** Abspielen läuft, ab diesem Dia */
  playing = $state<{ from: number } | null>(null);

  /** Abspielbare Musik (Blob-URL) zur Diashow im Speicher */
  musicUrl = $state<string | null>(null);
  /** Musik wird geladen bzw. analysiert */
  musicBusy = $state(false);
  /** Video wird gespeichert */
  exporting = $state<VideoProgress | null>(null);
  #exportAbort: AbortController | null = null;
  #musicFor: string | null = null;

  byId = $derived(new Map<number, AlbumCandidate>((this.state?.candidates ?? []).map((c) => [c.id, c])));

  #listening = false;

  #listen() {
    if (this.#listening) return;
    this.#listening = true;
    listen("album-progress", (p) => {
      if (this.progress) this.progress = p;
    });
  }

  #markProject(scope: Scope, has: boolean) {
    if (scope.type !== "project" || !!this.projects[scope.id] === has) return;
    const next = { ...this.projects };
    if (has) next[scope.id] = true;
    else delete next[scope.id];
    this.projects = next;
  }

  /** Zuletzt benutzte Einstellungen */
  lastSettings(): SlideshowSettings {
    return slideshowDefaults(load<Partial<SlideshowSettings>>(SETTINGS_KEY) ?? {});
  }

  /** Diashow des Bereichs öffnen; gibt es noch keine, die Einstellungen zeigen. */
  open(scope: Scope, title: string) {
    this.#listen();
    const saved = load<unknown>(showKey(scope));
    if (validSlideshow(saved)) {
      saved.settings = slideshowDefaults(saved.settings);
      this.state = saved;
      this.selection = null;
      lib.view = "slideshow";
      this.#dropMusicUrl();
      this.ensureMusic();
    } else this.dialog = { scope, title };
  }

  async create(settings: SlideshowSettings, scope: Scope, title: string): Promise<boolean> {
    this.#listen();
    store(SETTINGS_KEY, settings);
    this.progress = { done: 0, total: 0 };
    try {
      const s = await api.albumSuggest({
        scope,
        theme: settings.theme,
        customTheme: settings.theme === "custom" ? settings.customTheme : null,
        count: settings.count,
        photos: settings.photos,
      });
      if (!this.progress) return false; // abgebrochen
      // Musik einer früheren Fassung bleibt (mit „an die Musik anpassen“ wird neu verteilt)
      const music = this.state && JSON.stringify(this.state.scope) === JSON.stringify(scope) ? this.state.music : undefined;
      this.state = buildSlideshow(settings, scope, title, s);
      if (music) this.state.music = music;
      else this.#dropMusicUrl();
      this.selection = null;
      this.#save();
      lib.view = "slideshow";
      const picked = s.candidates.filter((c) => c.picked).length;
      if (picked > settings.count)
        lib.notify(t("Alle Fotos mit 4 oder 5 Sternen sind dabei – die Diashow hat deshalb {n} statt {count} Fotos.", { n: picked, count: settings.count }));
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
    this.#saveNow();
    this.selection = null;
    this.playing = null;
    lib.view = "grid";
  }

  /** Diashow verwerfen (gespeicherte Fassung löschen) */
  discard() {
    clearTimeout(this.#saveTimer);
    if (this.state) {
      store(showKey(this.state.scope), null);
      this.#markProject(this.state.scope, false);
    }
    this.state = null;
    this.#dropMusicUrl();
    this.close();
  }

  play(from = 0) {
    if (!this.state?.slides.length) return;
    this.ensureMusic();
    this.playing = { from: Math.min(from, this.state.slides.length - 1) };
  }

  /** Speichern gebündelt (beim Tippen nicht bei jedem Zeichen) */
  #save() {
    clearTimeout(this.#saveTimer);
    this.#saveTimer = setTimeout(() => this.#saveNow(), 400);
  }
  #saveTimer: ReturnType<typeof setTimeout> | undefined;

  #saveNow() {
    clearTimeout(this.#saveTimer);
    this.#saveTimer = undefined;
    if (this.state) {
      store(showKey(this.state.scope), $state.snapshot(this.state));
      this.#markProject(this.state.scope, true);
    }
  }

  #edit(f: (a: SlideshowState) => void) {
    if (!this.state) return;
    f(this.state);
    this.#save();
  }

  /** Automatische Beschriftung nach Änderungen an Reihenfolge oder Fotos neu setzen */
  #recaption(a: SlideshowState) {
    a.slides = autoCaptions(a.slides, a.settings.captions, this.byId);
  }

  // --- Dias --------------------------------------------------------------------

  replace(index: number, newId: number) {
    this.#edit((a) => {
      const s = a.slides[index];
      if (!s) return;
      a.slides[index] = { ...(s.seconds !== undefined ? { seconds: s.seconds } : {}), ...(s.caption !== undefined && !s.auto ? { caption: s.caption } : {}), id: newId };
      this.#recaption(a);
    });
  }

  remove(index: number) {
    this.#edit((a) => {
      a.slides.splice(index, 1);
      this.#recaption(a);
    });
    const n = this.state?.slides.length ?? 0;
    this.selection = n ? Math.min(index, n - 1) : null;
  }

  /** Foto hinzufügen, chronologisch einsortiert */
  add(id: number) {
    let at = 0;
    this.#edit((a) => {
      const order = new Map(a.candidates.map((c, i) => [c.id, i]));
      const o = order.get(id) ?? Infinity;
      at = a.slides.findIndex((s) => (order.get(s.id) ?? 0) > o);
      if (at < 0) at = a.slides.length;
      a.slides.splice(at, 0, { id });
      this.#recaption(a);
    });
    this.selection = at;
  }

  /** Dia an eine andere Stelle verschieben */
  move(from: number, to: number) {
    if (!this.state || from === to || to < 0 || to >= this.state.slides.length) return;
    this.#edit((a) => {
      const [s] = a.slides.splice(from, 1);
      a.slides.splice(to, 0, s);
      this.#recaption(a);
    });
    this.selection = to;
  }

  /** Eigene Dauer setzen (`null`: wie eingestellt) */
  setSlideSeconds(index: number, seconds: number | null) {
    this.#edit((a) => {
      const s = a.slides[index];
      if (!s) return;
      if (seconds === null) delete s.seconds;
      else s.seconds = clampSeconds(seconds);
    });
  }

  /** Eigener Text (leer: kein Text). `null`: wieder automatisch. */
  setCaption(index: number, text: string | null) {
    this.#edit((a) => {
      const s = a.slides[index];
      if (!s) return;
      if (text === null) {
        delete s.caption;
        delete s.auto;
        this.#recaption(a);
      } else {
        s.caption = text;
        s.auto = false;
      }
    });
  }

  /** Karte vor dem Foto ein- bzw. ausschalten (`null`: nach Entfernung) */
  setSlideMap(index: number, on: boolean | null) {
    this.#edit((a) => {
      const s = a.slides[index];
      if (!s) return;
      if (on === null) delete s.map;
      else s.map = on;
    });
  }

  setFit(index: number, fit: Slide["fit"] | null) {
    this.#edit((a) => {
      const s = a.slides[index];
      if (!s) return;
      if (fit) s.fit = fit;
      else delete s.fit;
    });
  }

  // --- Musik ---------------------------------------------------------------------

  #dropMusicUrl() {
    if (this.musicUrl) URL.revokeObjectURL(this.musicUrl);
    this.musicUrl = null;
    this.#musicFor = null;
  }

  /** Musik der Diashow laden (App: aus der Datei), falls noch nicht geschehen */
  async ensureMusic() {
    const m = this.state?.music;
    if (!m || !isTauri || (this.musicUrl && this.#musicFor === m.path)) return;
    try {
      const blob = await api.slideshowMusic(m.path);
      if (this.state?.music?.path !== m.path) return;
      this.#dropMusicUrl();
      this.musicUrl = URL.createObjectURL(blob);
      this.#musicFor = m.path;
    } catch (e) {
      lib.notify(t("Die Musik „{name}“ ließ sich nicht laden: {error}", { name: m.name, error: String(e) }), "error");
    }
  }

  /** Musik wählen: in der App per Dateidialog; im Browser übergibt die Ansicht die Datei */
  async pickMusic(file?: File) {
    if (!this.state || this.musicBusy) return;
    let blob: Blob;
    let path: string;
    let name: string;
    if (file) {
      blob = file;
      path = name = file.name;
    } else {
      const p = await pickMusicFile(t("Musik für die Diashow wählen"));
      if (!p) return;
      path = p;
      name = p.split(/[\\/]/).pop() ?? p;
      try {
        blob = await api.slideshowMusic(p);
      } catch (e) {
        lib.notify(String(e), "error");
        return;
      }
    }
    this.musicBusy = true;
    try {
      const beats = await analyzeMusic(blob);
      if (!this.state) return;
      const prev = this.state.music;
      const music: SlideshowMusic = { path, name, beats, sync: prev?.sync ?? true, fit: prev?.fit ?? true, volume: prev?.volume ?? 0.8 };
      this.#dropMusicUrl();
      this.musicUrl = URL.createObjectURL(blob);
      this.#musicFor = path;
      this.#edit((a) => {
        a.music = music;
      });
      if (!beats.bpm) lib.notify(t("In dieser Musik war kein gleichmäßiger Takt zu erkennen – die Bilder wechseln deshalb nach der eingestellten Dauer."));
    } catch (e) {
      lib.notify(t("Die Musik ließ sich nicht lesen: {error}", { error: String(e) }), "error");
    } finally {
      this.musicBusy = false;
    }
  }

  removeMusic() {
    this.#dropMusicUrl();
    this.#edit((a) => delete a.music);
  }

  setMusic(patch: Partial<Pick<SlideshowMusic, "sync" | "fit" | "volume">>) {
    this.#edit((a) => {
      if (a.music) Object.assign(a.music, patch);
    });
  }

  // --- Video ---------------------------------------------------------------------

  async exportVideo(size: VideoSize) {
    const a = this.state;
    if (!a?.slides.length || this.exporting) return;
    const name = (a.title || t("Diashow")).replace(/[\\/:*?"<>|]+/g, " ").trim() || t("Diashow");
    let path: string | null = null;
    if (isTauri) {
      path = await pickVideoTarget(t("Diashow als Video speichern"), name);
      if (!path) return;
    }
    // Musik (in der App aus der Datei, in der Browser-Vorschau die gewählte)
    let music: Blob | null = null;
    if (a.music) {
      await this.ensureMusic();
      if (this.musicUrl) music = await fetch(this.musicUrl).then((r) => r.blob());
    }
    const abort = new AbortController();
    this.#exportAbort = abort;
    this.exporting = { done: 0, total: 1 };
    // Browser-Vorschau: im Speicher sammeln und herunterladen
    const parts: { data: Uint8Array; position: number }[] = [];
    let opened = false;
    try {
      if (path) {
        await api.slideshowVideoOpen(path);
        opened = true;
      }
      const snap = $state.snapshot(a) as SlideshowState;
      await renderVideo(
        snap,
        this.byId,
        (id) => lib.thumbVersion(id),
        music,
        size,
        async (c) => {
          if (path) await api.slideshowVideoWrite(c.data, c.position);
          else parts.push({ data: c.data.slice(), position: c.position });
        },
        (p) => (this.exporting = p),
        abort.signal,
      );
      if (path) {
        await api.slideshowVideoClose(true);
        opened = false;
        lib.notify(t("Video gespeichert: {path}", { path }));
      } else downloadParts(parts, `${name}.mp4`);
    } catch (e) {
      if (opened) await api.slideshowVideoClose(false).catch(() => {});
      if (!abort.signal.aborted) lib.notify(t("Video nicht gespeichert: {error}", { error: e instanceof Error ? e.message : String(e) }), "error");
    } finally {
      this.exporting = null;
      this.#exportAbort = null;
    }
  }

  cancelExport() {
    this.#exportAbort?.abort();
  }

  // --- Einstellungen -------------------------------------------------------------

  setTitle(title: string) {
    this.#edit((a) => (a.title = title));
  }

  setSeconds(seconds: number) {
    this.#edit((a) => (a.settings.seconds = clampSeconds(seconds)));
  }

  setTransition(tr: Transition) {
    this.#edit((a) => (a.settings.transition = tr));
  }

  setMaps(on: boolean) {
    this.#edit((a) => {
      a.settings.maps = on;
      // Einzeln gesetzte Karten gelten weiter nur, wo sie von der Vorgabe abweichen
      for (const s of a.slides) if (s.map === on) delete s.map;
    });
  }

  setKenBurns(on: boolean) {
    this.#edit((a) => (a.settings.kenBurns = on));
  }

  setCaptions(mode: CaptionMode) {
    this.#edit((a) => {
      a.settings.captions = mode;
      this.#recaption(a);
    });
  }
}

/** Browser-Vorschau: Stücke mit Position zu einer Datei zusammensetzen und herunterladen */
function downloadParts(parts: { data: Uint8Array; position: number }[], name: string) {
  const size = parts.reduce((n, p) => Math.max(n, p.position + p.data.length), 0);
  const all = new Uint8Array(size);
  for (const p of parts) all.set(p.data, p.position);
  const url = URL.createObjectURL(new Blob([all], { type: "video/mp4" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

/** Musik dekodieren (Web Audio) und den Takt im Hintergrund erkennen */
async function analyzeMusic(blob: Blob): Promise<BeatInfo> {
  const ctx = new AudioContext();
  let audio: AudioBuffer;
  try {
    audio = await ctx.decodeAudioData(await blob.arrayBuffer());
  } finally {
    ctx.close().catch(() => {});
  }
  const channels = Array.from({ length: audio.numberOfChannels }, (_, i) => audio.getChannelData(i));
  // Kopie: Die Kanaldaten gehören dem AudioBuffer und lassen sich nicht übergeben
  const samples = channels.length === 1 ? channels[0].slice() : downmix(channels);
  const worker = new Worker(new URL("./beatWorker.ts", import.meta.url), { type: "module" });
  try {
    const info = await new Promise<BeatInfo>((resolve, reject) => {
      worker.onmessage = (e: MessageEvent<BeatInfo>) => resolve(e.data);
      worker.onerror = (e) => reject(e.message);
      worker.postMessage({ samples, rate: audio.sampleRate }, [samples.buffer]);
    });
    // Gerundet speichern (Millisekunden, zwei Stellen Stärke)
    return {
      ...info,
      beats: info.beats.map((x) => Math.round(x * 1000) / 1000),
      strength: info.strength.map((x) => Math.round(x * 100) / 100),
    };
  } finally {
    worker.terminate();
  }
}

export const slideshow = new SlideshowStore();
