<script lang="ts">
  import { untrack } from "svelte";
  import { isTyping } from "../lib/keys";
  import { imageUrl } from "../lib/api";
  import { aspectOf, justify, type Row } from "../lib/layout";
  import { lib } from "../lib/store.svelte";
  import Icon from "./Icon.svelte";
  import { t } from "../lib/i18n";

  const GAP = 4;
  const OVERSCAN = 600;

  let viewport: HTMLDivElement;
  let width = $state(0);
  let height = $state(0);
  let scrollTop = $state(0);

  // Nur bei neu geladener Liste neu berechnen (`layoutVersion`), nicht bei
  // jeder Bewertung – die ersetzt `lib.images`, ändert aber keine Geometrie.
  // Gleiche Seitenverhältnisse → dasselbe Array, damit auch ein Neuladen ohne
  // Änderung nicht das ganze Layout (50 000 Bilder) neu berechnet.
  let lastAspects: number[] = [];
  const aspects = $derived.by(() => {
    void lib.layoutVersion;
    const a = untrack(() => lib.images.map((i) => aspectOf(i.width, i.height)));
    if (a.length === lastAspects.length && a.every((v, i) => v === lastAspects[i])) return lastAspects;
    lastAspects = a;
    return a;
  });
  const layout = $derived(justify(aspects, Math.max(0, width - 2 * GAP), lib.rowHeight, GAP));
  /** Erste Zeile, für die `pred` gilt (Zeilen sind nach Lage und Index sortiert). */
  function firstRow(pred: (r: Row) => boolean): number {
    let lo = 0;
    let hi = layout.rows.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (pred(layout.rows[mid])) hi = mid;
      else lo = mid + 1;
    }
    return lo;
  }

  // Binäre Suche statt alle Zeilen (bei 50 000 Fotos ~8 000) je Scrollschritt zu prüfen
  const visible = $derived.by(() => {
    const from = firstRow((r) => r.top + r.height > scrollTop - OVERSCAN);
    const to = firstRow((r) => r.top >= scrollTop + height + OVERSCAN);
    return layout.rows.slice(from, Math.max(from, to));
  });

  function rowOf(index: number): number {
    const i = firstRow((r) => r.cells[r.cells.length - 1].index >= index);
    const r = layout.rows[i];
    return r && r.cells[0].index <= index ? i : -1;
  }

  // Fokussiertes Bild in den sichtbaren Bereich holen.
  $effect(() => {
    const r = layout.rows[rowOf(lib.focus)];
    if (!r || !viewport) return;
    const top = r.top + GAP;
    if (top < viewport.scrollTop) viewport.scrollTop = top - GAP;
    else if (top + r.height > viewport.scrollTop + height) viewport.scrollTop = top + r.height - height + GAP * 2;
  });

  // Beim Wechsel des Bereichs nach oben springen.
  $effect(() => {
    lib.scope;
    if (viewport) viewport.scrollTop = 0;
  });

  /** Pfeil hoch/runter: in die Nachbarzeile, zum Bild mit der nächsten Mitte. */
  function vertical(dir: 1 | -1) {
    const ri = rowOf(lib.focus);
    if (ri < 0) return lib.move(0);
    const target: Row | undefined = layout.rows[ri + dir];
    if (!target) return;
    const cur = layout.rows[ri].cells.find((c) => c.index === lib.focus)!;
    const mid = cur.left + cur.width / 2;
    const best = target.cells.reduce((a, c) => (Math.abs(c.left + c.width / 2 - mid) < Math.abs(a.left + a.width / 2 - mid) ? c : a));
    lib.select(best.index);
  }

  function onkeydown(e: KeyboardEvent) {
    if (lib.view !== "grid" || isTyping(e)) return;
    const k = e.key;
    if (k === "ArrowRight") lib.move(1);
    else if (k === "ArrowLeft") lib.move(-1);
    else if (k === "ArrowDown") vertical(1);
    else if (k === "ArrowUp") vertical(-1);
    // Mehrere ausgewählt: alle groß nebeneinander (Vergleich)
    else if ((k === "Enter" || k === " ") && lib.focused) lib.openLarge();
    else if (k === "a" && (e.ctrlKey || e.metaKey)) lib.selection = new Set(lib.images.map((i) => i.id));
    else return;
    e.preventDefault();
  }

  function onclick(e: MouseEvent, index: number) {
    lib.select(index, e.shiftKey ? "range" : e.ctrlKey || e.metaKey ? "toggle" : "single");
  }
</script>

<svelte:window {onkeydown} />

<div
  class="viewport scroll"
  bind:this={viewport}
  bind:clientWidth={width}
  bind:clientHeight={height}
  onscroll={() => (scrollTop = viewport.scrollTop)}
>
  {#if !lib.images.length && !lib.loading}
    <div class="empty">
      {#if lib.search || lib.minRating}
        <p>{t("Keine Fotos passen zu diesem Filter.")}</p>
        <button class="btn" onclick={() => { lib.search = ""; lib.minRating = 0; lib.loadImages(); }}>{t("Filter zurücksetzen")}</button>
      {:else if lib.scope.type === "project"}
        <p>{t("Dieses Projekt hat noch keine Fotos. Füge oben über „Ordner“ einen Ordner hinzu.")}</p>
      {:else if lib.scope.type === "person"}
        <p>{t("Dieser Person sind keine Fotos (mehr) zugeordnet.")}</p>
      {:else if !lib.roots.length}
        <p>{t("Füge links unter „Ordner“ einen Ordner mit Fotos hinzu.")}</p>
      {:else}
        <p>{t("In diesem Ordner liegen keine Fotos.")}</p>
      {/if}
    </div>
  {/if}

  <div class="canvas" style:height="{layout.height + GAP * 2}px">
    {#each visible as row (row.top)}
      {#each row.cells as cell (lib.images[cell.index].id)}
        {@const img = lib.images[cell.index]}
        {@const selected = lib.selection.has(img.id)}
        <button
          class="cell"
          class:selected
          class:focused={lib.focus === cell.index}
          class:rejected={img.flag === -1}
          style:transform="translate({cell.left + GAP}px, {row.top + GAP}px)"
          style:width="{cell.width}px"
          style:height="{row.height}px"
          onclick={(e) => onclick(e, cell.index)}
          ondblclick={() => (lib.view = "loupe")}
          oncontextmenu={(e) => lib.openMenu(e, cell.index)}
          title={img.fileName}
        >
          <img src={imageUrl("thumb", img.id, lib.thumbVersion(img.id))} alt="" decoding="async" draggable="false" />
          <span class="meta">
            {#if img.flag === 1}<span class="flag" title={t("Markiert")}><Icon name="flag" size={12} /></span>{/if}
            {#if img.rating}<span class="rating">{"★".repeat(img.rating)}</span>{/if}
            {#if img.edited}<span class="edited" title={t("Bearbeitet")}><Icon name="sliders" size={11} /></span>{/if}
            {#if img.kind === "raw"}<span class="raw">RAW</span>{/if}
          </span>
        </button>
      {/each}
    {/each}
  </div>
</div>

<style>
  .viewport {
    position: relative;
    flex: 1;
    min-height: 0;
    background: var(--bg);
  }
  .canvas {
    position: relative;
  }
  .cell {
    position: absolute;
    left: 0;
    top: 0;
    overflow: hidden;
    background: var(--panel);
    border-radius: 2px;
    contain: strict;
  }
  .cell img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  /* Fehlende Thumbnails nicht als kaputte Bilder anzeigen */
  .cell img:not([src]),
  .cell img[src=""] {
    visibility: hidden;
  }
  .cell.selected {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .cell.focused {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .cell.rejected img {
    opacity: 0.32;
  }
  .meta {
    position: absolute;
    left: 6px;
    bottom: 5px;
    right: 6px;
    display: flex;
    gap: 6px;
    align-items: center;
    pointer-events: none;
    font-size: 11px;
    text-shadow: 0 1px 3px rgb(0 0 0 / 0.8);
  }
  .rating {
    color: var(--accent);
    letter-spacing: 1px;
  }
  .flag {
    color: #fff;
    display: inline-grid;
  }
  .edited {
    display: inline-grid;
    color: #fff;
  }
  .raw {
    margin-left: auto;
    font-size: 9.5px;
    font-weight: 600;
    padding: 1px 4px;
    border-radius: 3px;
    background: rgb(0 0 0 / 0.5);
    color: #d8d6d0;
    opacity: 0;
    transition: opacity 0.12s;
    text-shadow: none;
  }
  .cell:hover .raw {
    opacity: 1;
  }
  .empty {
    position: absolute;
    inset: 0;
    display: grid;
    place-content: center;
    justify-items: center;
    gap: 12px;
    color: var(--muted);
    text-align: center;
    padding: 24px;
  }
  .empty p {
    margin: 0;
    max-width: 44ch;
  }
</style>
