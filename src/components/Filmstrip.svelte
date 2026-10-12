<script lang="ts">
  import { untrack } from "svelte";
  import { imageUrl } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";
  import { aspectOf } from "../lib/layout";
  import Icon from "./Icon.svelte";

  // `compare`: Streifen unter dem Vergleich – Klicks ändern die verglichenen
  // Fotos statt die Auswahl (siehe `lib.compareStrip`).
  let { compare = false }: { compare?: boolean } = $props();

  const GAP = 4;
  /** Zusätzlich gezeichnete Breite links und rechts vom sichtbaren Bereich */
  const OVERSCAN = 600;

  let strip: HTMLDivElement;
  let scrollX = $state(0);
  let viewW = $state(0);
  let thumbH = $state(63);

  // Der Streifen kennt alle Fotos, zeichnet aber nur die sichtbaren: Die
  // Breiten folgen aus den Seitenverhältnissen im Katalog, ihre laufende Summe
  // liefert jede Position ohne Messen (bei 80 000 Fotos ein Float64Array).
  // Nur bei neu geladener Liste (`layoutVersion`), nicht bei jeder Bewertung
  const lefts = $derived.by(() => {
    void lib.layoutVersion;
    const images = untrack(() => lib.images);
    const a = new Float64Array(images.length + 1);
    for (let i = 0; i < images.length; i++) {
      a[i + 1] = a[i] + Math.round(aspectOf(images[i].width, images[i].height) * thumbH) + GAP;
    }
    return a;
  });
  const total = $derived(Math.max(0, lefts[lefts.length - 1] - GAP));

  /** Erster Index, dessen rechte Kante hinter `x` liegt (binäre Suche). */
  function indexAt(x: number): number {
    let lo = 0;
    let hi = lefts.length - 1;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (lefts[mid + 1] <= x) lo = mid + 1;
      else hi = mid;
    }
    return lo;
  }

  const items = $derived.by(() => {
    const n = lib.images.length;
    if (!n) return [];
    const from = indexAt(scrollX - OVERSCAN);
    const to = Math.min(n - 1, indexAt(scrollX + viewW + OVERSCAN));
    const out = [];
    for (let index = from; index <= to; index++) out.push({ im: lib.images[index], index });
    return out;
  });

  // Aktuelles Foto mittig holen, wenn es außerhalb des sichtbaren Bereichs liegt
  $effect(() => {
    const f = lib.focus;
    if (!strip || f < 0 || f >= lefts.length - 1) return;
    const left = lefts[f];
    const right = lefts[f + 1] - GAP;
    const view = strip.clientWidth;
    if (left < strip.scrollLeft || right > strip.scrollLeft + view) {
      strip.scrollLeft = (left + right) / 2 - view / 2;
    }
  });

  function onclick(e: MouseEvent, index: number) {
    const add = e.shiftKey || e.ctrlKey || e.metaKey;
    if (compare) lib.compareStrip(index, add);
    else lib.select(index, e.shiftKey ? "range" : e.ctrlKey || e.metaKey ? "toggle" : "single");
  }

  function oncontextmenu(e: MouseEvent, index: number) {
    // Im Vergleich würde das Menü ein fremdes Foto allein auswählen und den
    // Vergleich beenden – dort nur für verglichene Fotos
    if (compare && !lib.selection.has(lib.images[index]?.id)) {
      e.preventDefault();
      return;
    }
    lib.openMenu(e, index);
  }

  // Mausrad scrollt waagerecht (Touchpads liefern deltaX selbst)
  function onwheel(e: WheelEvent) {
    if (Math.abs(e.deltaY) <= Math.abs(e.deltaX) || e.ctrlKey) return;
    strip.scrollLeft += e.deltaY;
    e.preventDefault();
  }
</script>

<div
  class="strip scroll"
  bind:this={strip}
  bind:clientWidth={viewW}
  onscroll={() => (scrollX = strip.scrollLeft)}
  {onwheel}
  title={compare ? t("Klick: ersetzt das hervorgehobene Foto · Strg-Klick: hinzufügen oder herausnehmen") : undefined}
>
  <div class="track" style="width: {total}px" bind:clientHeight={thumbH}>
    {#each items as { im, index } (im.id)}
      <button
        class:current={index === lib.focus}
        class:selected={lib.selection.has(im.id) && index !== lib.focus}
        class:rejected={im.flag === -1}
        style="left: {lefts[index]}px; width: {lefts[index + 1] - lefts[index] - GAP}px"
        onclick={(e) => onclick(e, index)}
        oncontextmenu={(e) => oncontextmenu(e, index)}
        aria-label={im.fileName}
        title={im.fileName}
      >
        <img src={imageUrl("thumb", im.id, lib.thumbVersion(im.id))} alt="" draggable="false" />
        {#if im.edited}<span class="badge"><Icon name="sliders" size={10} /></span>{/if}
      </button>
    {/each}
  </div>
</div>

<style>
  .rejected img {
    opacity: 0.32;
  }
  .strip {
    flex: none;
    height: 76px;
    padding: 6px 8px;
    border-top: 1px solid var(--line);
    background: var(--panel);
    overflow-y: hidden;
  }
  .track {
    position: relative;
    height: 100%;
  }
  button {
    position: absolute;
    top: 0;
    height: 100%;
    border-radius: 2px;
    overflow: hidden;
    opacity: 0.6;
  }
  button:hover {
    opacity: 0.9;
  }
  button.selected {
    opacity: 0.9;
    outline: 1px solid var(--faint);
    outline-offset: -1px;
  }
  button.current {
    opacity: 1;
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .badge {
    position: absolute;
    right: 3px;
    bottom: 3px;
    display: grid;
    padding: 2px;
    border-radius: 3px;
    background: rgb(0 0 0 / 0.55);
    color: #fff;
  }
</style>
