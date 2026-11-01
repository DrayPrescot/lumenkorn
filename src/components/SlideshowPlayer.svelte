<script lang="ts">
  // Diashow abspielen: Vollbild, Uhr per requestAnimationFrame – mit Musik gibt
  // die Wiedergabe der Musik die Zeit vor, damit die Schnitte auf dem Takt
  // bleiben. Leertaste hält an, Pfeiltasten springen ein Dia weiter bzw.
  // zurück, Esc beendet. Die Steuerleiste erscheint, wenn sich die Maus bewegt.
  import { onDestroy, onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { imageUrl, isTauri } from "../lib/api";
  import { formatDuration, frameAt, slideAt, timeline } from "../lib/slideshow";
  import { slideshow } from "../lib/slideshow.svelte";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";
  import SlideStage from "./SlideStage.svelte";
  import Icon from "./Icon.svelte";

  let { from = 0 }: { from?: number } = $props();

  const a = $derived(slideshow.state!);
  const tl = $derived(timeline(a));

  let el = $state<HTMLDivElement>();
  let width = $state(16);
  let height = $state(9);
  // svelte-ignore state_referenced_locally
  let time = $state((() => {
    const t0 = timeline(slideshow.state!);
    return t0.starts[t0.photoEntry[from]] ?? 0;
  })());
  let paused = $state(false);
  let controls = $state(true);
  const frame = $derived(frameAt(a, tl, time, width / Math.max(1, height), slideshow.byId));
  /** Abschnitt (Foto oder Karte) und Foto zur aktuellen Zeit */
  const current = $derived(slideAt(tl.starts, time));
  const photo = $derived(tl.entries[current]?.slide ?? 0);

  // --- Uhr ---------------------------------------------------------------------
  let raf = 0;
  let last = 0;
  let audio = $state<HTMLAudioElement>();
  /** Beginn des laufenden Durchgangs der Musik (sie wiederholt sich, wenn sie kürzer ist) */
  let loopBase = 0;
  let lastCt = 0;
  /** Die WebView hat das Abspielen verweigert: nicht in jedem Bild neu versuchen */
  let audioBlocked = false;
  const musicDur = () => (audio && Number.isFinite(audio.duration) && audio.duration > 0 ? audio.duration : a.music?.beats.duration || 0);

  function tick(now: number) {
    const dt = last ? (now - last) / 1000 : 0;
    last = now;
    const dur = musicDur();
    if (!paused) {
      if (audio && slideshow.musicUrl && dur > 0) {
        if (audio.paused && audio.readyState >= 2 && !audioBlocked) {
          // Musik ist (wieder) bereit: an der aktuellen Stelle einsetzen
          seekAudio();
          audio.play().catch(() => (audioBlocked = true));
        }
        if (!audio.paused && audio.readyState >= 3) {
          const ct = audio.currentTime;
          if (ct + 0.5 < lastCt) loopBase += dur; // von vorn begonnen
          lastCt = ct;
          time = loopBase + ct;
        } else time += dt;
      } else time += dt;
      if (time >= tl.total) {
        close();
        return;
      }
    }
    if (audio && a.music) {
      // Am Anfang kurz ein-, am Ende ausblenden
      const edge = Math.min(1, time / 0.5, (tl.total - time) / 2);
      audio.volume = a.music.volume * Math.max(0, Math.min(1, edge));
    }
    raf = requestAnimationFrame(tick);
  }

  /** Musik an die Zeit der Diashow setzen */
  function seekAudio() {
    const dur = musicDur();
    if (!audio || !dur) return;
    loopBase = Math.floor(time / dur) * dur;
    audio.currentTime = time - loopBase;
    lastCt = audio.currentTime;
  }

  function setTime(x: number) {
    time = Math.max(0, Math.min(tl.total - 0.01, x));
    seekAudio();
    showControls();
  }

  // Die nächsten Fotos vorab laden, damit der Übergang nicht auf das Bild wartet
  const preload = new Map<number, HTMLImageElement>();
  $effect(() => {
    const want = a.slides.slice(photo, photo + 3).map((s) => s.id);
    for (const id of want) {
      if (preload.has(id)) continue;
      const img = new Image();
      img.src = imageUrl("preview", id, lib.thumbVersion(id));
      preload.set(id, img);
    }
    if (preload.size > 12) for (const id of [...preload.keys()].slice(0, preload.size - 12)) preload.delete(id);
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
    audio?.pause();
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
    if (paused) audio?.pause();
    else if (audio && slideshow.musicUrl) {
      audioBlocked = false;
      seekAudio();
      audio.play().catch(() => (audioBlocked = true));
    }
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
  {#if slideshow.musicUrl && a.music}
    <audio bind:this={audio} src={slideshow.musicUrl} preload="auto"></audio>
  {/if}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="click" onclick={togglePause}>
    <SlideStage {frame} title={a.title} />
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
