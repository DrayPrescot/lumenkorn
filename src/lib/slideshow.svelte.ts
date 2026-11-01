// Zustand der Diashow: Erstellen (Auswahl im Backend wie beim Fotoalbum),
// Bearbeiten in der Zeitleiste, Abspielen. Die Diashow eines Bereichs bleibt im
// Speicher der WebView erhalten und lässt sich später weiterbearbeiten.

import { api, isTauri, listen, pickMusicFiles, pickVideoTarget, type Scope } from "./api";
import { renderVideo, type VideoCodecKey, type VideoProgress, type VideoSize } from "./slideshowVideo";
import { downmix, type BeatInfo } from "./beats";
import type { AlbumFont, CaptionMode } from "./album";
import {
  autoCaptions,
  buildSlideshow,
  clampSeconds,
  fontCss,
  slideshowDefaults,
  textStyle,
  upgradeSlideshow,
  DEFAULT_CAPTION_STYLE,
  DEFAULT_TITLE_STYLE,
  validSlideshow,
  type MusicMode,
  type MusicTrack,
  type Slide,
  type SlideshowSettings,
  type SlideshowMusic,
  type SlideTextStyle,
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

  /** Abspielbare Lieder (Pfad → Blob-URL) der Diashow im Speicher */
  musicUrls = $state<Record<string, string>>({});
  /** Musik wird geladen bzw. analysiert */
  musicBusy = $state(false);
  /** Video wird gespeichert */
  exporting = $state<VideoProgress | null>(null);
  #exportAbort: AbortController | null = null;

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
    const saved = upgradeSlideshow(load<unknown>(showKey(scope)));
    if (validSlideshow(saved)) {
      saved.settings = slideshowDefaults(saved.settings);
      this.state = saved;
      this.selection = null;
      lib.view = "slideshow";
      this.#dropMusicUrls();
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
      else this.#dropMusicUrls();
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
    this.#dropMusicUrls();
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

  #dropMusicUrls(keep: string[] = []) {
    const next: Record<string, string> = {};
    for (const [path, url] of Object.entries(this.musicUrls)) {
      if (keep.includes(path)) next[path] = url;
      else URL.revokeObjectURL(url);
    }
    this.musicUrls = next;
  }

  /** Lieder der Diashow laden (App: aus den Dateien), soweit noch nicht geschehen */
  async ensureMusic() {
    const m = this.state?.music;
    if (!m || !isTauri) return;
    for (const tr of m.tracks) {
      if (this.musicUrls[tr.path]) continue;
      try {
        const blob = await api.slideshowMusic(tr.path);
        if (!this.state?.music?.tracks.some((x) => x.path === tr.path)) continue;
        this.musicUrls = { ...this.musicUrls, [tr.path]: URL.createObjectURL(blob) };
      } catch (e) {
        lib.notify(t("Die Musik „{name}“ ließ sich nicht laden: {error}", { name: tr.name, error: String(e) }), "error");
      }
    }
  }

  /** Lieder hinzufügen: in der App per Dateidialog (mehrere), im Browser übergibt die Ansicht die Dateien */
  async addMusic(files?: File[]) {
    if (!this.state || this.musicBusy) return;
    let items: { path: string; name: string; blob: () => Promise<Blob> }[];
    if (files) items = files.map((f) => ({ path: f.name, name: f.name, blob: async () => f }));
    else {
      const paths = await pickMusicFiles(t("Musik für die Diashow wählen"));
      if (!paths.length) return;
      items = paths.map((p) => ({ path: p, name: p.split(/[\\/]/).pop() ?? p, blob: () => api.slideshowMusic(p) }));
    }
    this.musicBusy = true;
    try {
      for (const it of items) {
        if (this.state?.music?.tracks.some((x) => x.path === it.path)) continue;
        try {
          const blob = await it.blob();
          const beats = await analyzeMusic(blob);
          if (!this.state) return;
          const track: MusicTrack = { path: it.path, name: it.name, beats };
          this.musicUrls = { ...this.musicUrls, [it.path]: URL.createObjectURL(blob) };
          this.#edit((a) => {
            if (a.music) a.music.tracks.push(track);
            else a.music = { tracks: [track], mode: "sequence", sync: true, fit: true, volume: 0.8 };
          });
          if (!beats.bpm) lib.notify(t("In „{name}“ war kein gleichmäßiger Takt zu erkennen – dort wechseln die Bilder nach der eingestellten Dauer.", { name: it.name }));
        } catch (e) {
          lib.notify(t("Die Musik „{name}“ ließ sich nicht lesen: {error}", { name: it.name, error: String(e) }), "error");
        }
      }
    } finally {
      this.musicBusy = false;
    }
  }

  removeTrack(index: number) {
    this.#edit((a) => {
      if (!a.music) return;
      const [gone] = a.music.tracks.splice(index, 1);
      // Auch aus der Zuordnung zu den Orten nehmen
      if (a.music.assign && gone) {
        for (const k of Object.keys(a.music.assign)) {
          a.music.assign[k] = a.music.assign[k].filter((p) => p !== gone.path);
          if (!a.music.assign[k].length) delete a.music.assign[k];
        }
      }
      if (!a.music.tracks.length) delete a.music;
    });
    this.#dropMusicUrls(this.state?.music?.tracks.map((x) => x.path) ?? []);
  }

  moveTrack(index: number, dir: -1 | 1) {
    this.#edit((a) => {
      const tr = a.music?.tracks;
      const to = index + dir;
      if (!tr || to < 0 || to >= tr.length) return;
      [tr[index], tr[to]] = [tr[to], tr[index]];
    });
  }

  /** „Lieder nach Ort“: Lied einem Ort zuordnen bzw. die Zuordnung lösen */
  assignTrack(place: string, path: string, on: boolean) {
    this.#edit((a) => {
      const m = a.music;
      if (!m) return;
      const assign = { ...(m.assign ?? {}) };
      const list = (assign[place] ?? []).filter((p) => p !== path);
      // In der Reihenfolge der Liederliste
      if (on) list.push(path);
      const order = m.tracks.map((t) => t.path);
      list.sort((x, y) => order.indexOf(x) - order.indexOf(y));
      if (list.length) assign[place] = list;
      else delete assign[place];
      m.assign = Object.keys(assign).length ? assign : undefined;
    });
  }

  removeMusic() {
    this.#dropMusicUrls();
    this.#edit((a) => delete a.music);
  }

  setMusic(patch: Partial<{ sync: boolean; fit: boolean; volume: number; mode: MusicMode }>) {
    this.#edit((a) => {
      if (a.music) Object.assign(a.music, patch);
    });
  }

  // --- Video ---------------------------------------------------------------------

  async exportVideo(size: VideoSize, codec: VideoCodecKey) {
    const a = this.state;
    if (!a?.slides.length || this.exporting) return;
    const name = (a.title || t("Diashow")).replace(/[\\/:*?"<>|]+/g, " ").trim() || t("Diashow");
    let path: string | null = null;
    if (isTauri) {
      path = await pickVideoTarget(t("Diashow als Video speichern"), name);
      if (!path) return;
    }
    // Lieder (in der App aus den Dateien, in der Browser-Vorschau die gewählten)
    const music: (Blob | null)[] = [];
    if (a.music) {
      await this.ensureMusic();
      for (const tr of a.music.tracks) {
        const url = this.musicUrls[tr.path];
        music.push(url ? await fetch(url).then((r) => r.blob()) : null);
      }
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
        codec,
        { font: fontCss(a.settings.font) },
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

  /** Foto für das Titelbild (`null`: das am besten bewertete) */
  setCover(id: number | null) {
    this.#edit((a) => {
      if (id === null) delete a.cover;
      else a.cover = id;
    });
  }

  setFont(font: AlbumFont) {
    this.#edit((a) => (a.settings.font = font));
  }

  /** Stil von Datum/Ort und eigenen Texten bzw. des Titels */
  setTextStyle(which: "captionStyle" | "titleStyle", st: SlideTextStyle) {
    this.#edit((a) => (a.settings[which] = textStyle(st, which === "captionStyle" ? DEFAULT_CAPTION_STYLE : DEFAULT_TITLE_STYLE)));
  }

  /** Eigener Stil des Textes eines Fotos (`null`: wie alle) – gespeichert wird nur, was abweicht */
  setSlideStyle(index: number, st: SlideTextStyle | null) {
    this.#edit((a) => {
      const s = a.slides[index];
      if (!s) return;
      if (!st) {
        delete s.style;
        return;
      }
      const base = a.settings.captionStyle;
      const diff: Partial<SlideTextStyle> = {};
      for (const k of Object.keys(base) as (keyof SlideTextStyle)[]) if (st[k] !== base[k]) Object.assign(diff, { [k]: st[k] });
      if (Object.keys(diff).length) s.style = diff;
      else delete s.style;
    });
  }

  setEndMap(on: boolean) {
    this.#edit((a) => (a.settings.endMap = on));
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
