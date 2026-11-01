<script lang="ts">
  // Diashow abspielen: Vollbild, Uhr per requestAnimationFrame – mit Musik gibt
  // das gerade lauteste Lied die Zeit vor, damit die Schnitte auf dem Takt
  // bleiben. Jedes Musikstück der Zeitleiste bekommt ein eigenes Audio-Element
  // (beim Überblenden laufen zwei). Leertaste hält an, Pfeiltasten springen
  // einen Abschnitt weiter bzw. zurück, Esc beendet. Die Steuerleiste erscheint,
  // wenn sich die Maus bewegt.
  import { onDestroy, onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { isTauri } from "../lib/api";
  import { clipGain, formatDuration, frameAt, slideAt, timeline } from "../lib/slideshow";
  import { slideshow } from "../lib/slideshow.svelte";
  import { preloadFull } from "../lib/fullImages.svelte";
  import { prefetchLeg, prefetchOverview } from "../lib/slideMap";
  import { fontFamily } from "../lib/album";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";
  import SlideStage from "./SlideStage.svelte";
  import Icon from "./Icon.svelte";

  let { from = 0 }: { from?: number } = $props();

  const a = $derived(slideshow.state!);
  const tl = $derived(timeline(a, slideshow.byId));

  let el = $state<HTMLDivElement>();
  let width = $state(16);
  let height = $state(9);
  // svelte-ignore state_referenced_locally
  let time = $state((() => {
    const t0 = timeline(slideshow.state!, slideshow.byId);
    // Ab dem ersten Foto: mit Titelbild
    return from > 0 ? (t0.starts[t0.photoEntry[from]] ?? 0) : 0;
  })());
  let paused = $state(false);
  let controls = $state(true);
  const frame = $derived(frameAt(a, tl, time, width / Math.max(1, height), slideshow.byId));
  /** Abschnitt und Foto zur aktuellen Zeit */
  const current = $derived(slideAt(tl.starts, time));
  const photo = $derived(tl.entries[current]?.slide ?? 0);

  // --- Musik ---------------------------------------------------------------------
  /** Audio-Element je laufendem Musikstück (Index in `tl.clips`) */
  const players = new Map<number, HTMLAudioElement>();
  /** Die WebView hat das Abspielen verweigert: nicht in jedem Bild neu versuchen */
  let audioBlocked = false;

  const urlOf = (track: number) => {
    const tr = a.music?.tracks[track];
    return tr ? slideshow.musicUrls[tr.path] : undefined;
  };

  /** Audio-Elemente der Stücke um `time` anlegen bzw. verwerfen, Lautstärke setzen */
  function syncAudio(): HTMLAudioElement | null {
    const vol = a.music?.volume ?? 0;
    let lead: HTMLAudioElement | null = null;
    let leadGain = -1;
    tl.clips.forEach((c, i) => {
      const near = time >= c.start - 1 && time < c.end;
      let au = players.get(i);
      const url = urlOf(c.track);
      if (!near || !url) {
        if (au) {
          au.pause();
          players.delete(i);
        }
        return;
      }
      if (!au) {
        au = new Audio(url);
        au.preload = "auto";
        players.set(i, au);
      }
      const want = c.offset + (time - c.start);
      const g = clipGain(c, time, tl.total);
      au.volume = Math.max(0, Math.min(1, vol * g));
      const active = time >= c.start;
      if (paused || !active) {
        if (!au.paused) au.pause();
        if (!active && au.readyState >= 1 && Math.abs(au.currentTime - c.offset) > 0.05) au.currentTime = c.offset;
        return;
      }
      if (au.paused && au.readyState >= 2 && !audioBlocked) {
        au.currentTime = want;
        au.play().catch(() => (audioBlocked = true));
      } else if (!au.paused && g > leadGain && au.readyState >= 3) {
        lead = au;
        leadGain = g;
      }
      // Nebenstimmen nachführen, wenn sie abweichen
      if (!au.paused && Math.abs(au.currentTime - want) > 0.25) au.currentTime = want;
    });
    return lead;
  }

  /** Alle Stücke an die Zeit der Diashow setzen (nach Springen) */
  function seekAudio() {
    tl.clips.forEach((c, i) => {
      const au = players.get(i);
      if (au && time >= c.start && time < c.end && au.readyState >= 1) au.currentTime = c.offset + (time - c.start);
    });
  }

  // --- Uhr ---------------------------------------------------------------------
  let raf = 0;
  let last = 0;

  function tick(now: number) {
    const dt = last ? (now - last) / 1000 : 0;
    last = now;
    if (!paused) {
      // Lautestes laufendes Lied als Uhr, sonst die Bildwiederholung
      const leadIdx = [...players.entries()].find(([, au]) => au === lastLead)?.[0];
      const c = leadIdx !== undefined ? tl.clips[leadIdx] : undefined;
      if (lastLead && c && !lastLead.paused && lastLead.readyState >= 3) {
        const t2 = c.start + (lastLead.currentTime - c.offset);
        // Sprünge der Audio-Uhr (Puffern) nicht übernehmen
        time = Math.abs(t2 - (time + dt)) < 0.3 ? t2 : time + dt;
      } else time += dt;
      if (time >= tl.total) {
        close();
        return;
      }
    }
    lastLead = syncAudio();
    raf = requestAnimationFrame(tick);
  }
  let lastLead: HTMLAudioElement | null = null;

  function setTime(x: number) {
    time = Math.max(0, Math.min(tl.total - 0.01, x));
    seekAudio();
    showControls();
  }

  // Die nächsten Fotos in voller Auflösung vorab laden, Kacheln der nächsten Karten ebenso
  $effect(() => {
    const ids: number[] = [];
    for (let i = current; i < Math.min(tl.entries.length, current + 4); i++) {
      const e = tl.entries[i];
      if (e.kind === "photo") ids.push(a.slides[e.slide].id);
      else if (e.kind === "title") ids.push(e.id);
    }
    preloadFull(ids);
  });
  const style = $derived({ tiles: lib.mapEnabled, font: fontFamily("sans") });
  $effect(() => {
    for (let i = current; i < Math.min(tl.entries.length, current + 3); i++) {
      const e = tl.entries[i];
      if (e.kind === "map") prefetchLeg(e.leg, width, height, style);
      else if (e.kind === "overview") prefetchOverview(e.places, width, height, style);
    }
  });

  // --- Vollbild ------------------------------------------------------------------
  let wasFullscreen = true;
  onMount(async () => {
    raf = requestAnimationFrame(tick);
    el?.focus();
    showControls();
    try {
      if (isTauri) {
        const w = getCurrentWindow();
        wasFullscreen = await w.isFullscreen();
        if (!wasFullscreen) await w.setFullscreen(true);
      } else if (!document.fullscreenElement) {
        wasFullscreen = false;
        await el?.requestFullscreen();
      }
    } catch {
      // Ohne Vollbild läuft die Diashow im Fenster
    }
  });

  onDestroy(() => {
    cancelAnimationFrame(raf);
    for (const au of players.values()) au.pause();
    players.clear();
    clearTimeout(hideTimer);
    if (wasFullscreen) return;
    if (isTauri) getCurrentWindow().setFullscreen(false).catch(() => {});
    else if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  });

  function close() {
    slideshow.selection = Math.min(photo, a.slides.length - 1);
    slideshow.playing = null;
  }

  function jump(index: number) {
    const i = Math.max(0, Math.min(tl.entries.length - 1, index));
    setTime(tl.starts[i]);
  }

  // --- Steuerleiste ------------------------------------------------------------------
  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  function showControls() {
    controls = true;
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => (controls = paused), 2500);
  }

  function togglePause() {
    paused = !paused;
    // Klick des Nutzers: Abspielen darf wieder versucht werden
    audioBlocked = false;
    if (paused) for (const au of players.values()) au.pause();
    else seekAudio();
    last = 0;
    showControls();
  }

  function seek(e: PointerEvent) {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    setTime(((e.clientX - r.left) / r.width) * tl.total);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "F11") return;
    e.stopPropagation();
    if (e.key === "Escape") close();
    else if (e.key === " " || e.key === "k") togglePause();
    else if (e.key === "ArrowRight" || e.key === "PageDown") jump(current + 1);
    else if (e.key === "ArrowLeft" || e.key === "PageUp") jump(time - tl.starts[current] > 1.5 ? current : current - 1);
    else if (e.key === "Home") jump(0);
    else return;
    e.preventDefault();
  }
</script>

<svelte:window onkeydowncapture={onkeydown} />

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
  class="player"
  class:nocursor={!controls}
  bind:this={el}
  bind:clientWidth={width}
  bind:clientHeight={height}
  tabindex="0"
  role="application"
  aria-label={t("Diashow")}
  onpointermove={showControls}
>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="click" onclick={togglePause}>
    <SlideStage {frame} title={a.title} full />
  </div>
  {#if paused}
    <div class="paused" aria-hidden="true"><Icon name="play" size={56} /></div>
  {/if}
  <div class="bar" class:hidden={!controls}>
    <button class="icon-btn" onclick={() => jump(current - 1)} title={t("Voriges Foto (←)")} aria-label={t("Voriges Foto (←)")}><Icon name="back" size={18} /></button>
    <button class="icon-btn" onclick={togglePause} title={paused ? t("Weiter (Leertaste)") : t("Anhalten (Leertaste)")} aria-label={paused ? t("Weiter (Leertaste)") : t("Anhalten (Leertaste)")}>
      <Icon name={paused ? "play" : "pause"} size={18} />
    </button>
    <button class="icon-btn" onclick={() => jump(current + 1)} title={t("Nächstes Foto (→)")} aria-label={t("Nächstes Foto (→)")}><Icon name="chevron" size={18} /></button>
    <span class="time">{formatDuration(time)} / {formatDuration(tl.total)}</span>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="track" onpointerdown={seek}>
      <div class="fill" style:width="{(time / Math.max(0.001, tl.total)) * 100}%"></div>
    </div>
    <span class="time">{t("{i} von {n}", { i: photo + 1, n: a.slides.length })}</span>
    <button class="icon-btn" onclick={close} title={t("Beenden (Esc)")} aria-label={t("Beenden (Esc)")}><Icon name="close" size={18} /></button>
  </div>
</div>

<style>
  .player {
    position: fixed;
    inset: 0;
    z-index: 200;
    background: #000;
    outline: none;
  }
  .player.nocursor {
    cursor: none;
  }
  .click {
    position: absolute;
    inset: 0;
  }
  .paused {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    color: rgb(255 255 255 / 0.8);
    pointer-events: none;
    filter: drop-shadow(0 0 12px rgb(0 0 0 / 0.6));
  }
  .bar {
    position: absolute;
    left: 50%;
    bottom: 24px;
    transform: translateX(-50%);
    width: min(820px, calc(100% - 32px));
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px;
    border-radius: 999px;
    background: rgb(20 20 20 / 0.72);
    backdrop-filter: blur(8px);
    color: #eee;
    transition: opacity 0.3s;
  }
  .bar.hidden {
    opacity: 0;
    pointer-events: none;
  }
  .bar .icon-btn {
    color: #eee;
  }
  .time {
    font-variant-numeric: tabular-nums;
    font-size: var(--step--1);
    white-space: nowrap;
    color: #ccc;
  }
  .track {
    flex: 1;
    height: 6px;
    border-radius: 3px;
    background: rgb(255 255 255 / 0.2);
    cursor: pointer;
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--accent);
  }
</style>
