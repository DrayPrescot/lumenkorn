<script lang="ts">
  import { untrack } from "svelte";
  import { isTyping } from "../lib/keys";
  import { imageUrl } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { ZoomState, loadHi, zoomKey } from "../lib/zoom.svelte";
  import { libraryDims } from "../lib/layout";
  import Filmstrip from "./Filmstrip.svelte";
  import ZoomView from "./ZoomView.svelte";

  const img = $derived(lib.focused);
  // Zoom bleibt beim Blättern erhalten – so lassen sich Serien vergleichen.
  const zoom = new ZoomState();
  let loaded = $state<number | null>(null);
  let stageW = $state(0);
  let stageH = $state(0);

  /** Braucht ein Foto eingepasst mehr Pixel, als seine Vorschau (2048) hat?
   *  Dann lädt ZoomView die volle Auflösung (siehe dort). */
  function needsFull(im: { width: number | null; height: number | null }): boolean {
    if (!im.width || !im.height || !stageW || !stageH) return false;
    const dpr = window.devicePixelRatio || 1;
    const fit = Math.min((stageW * dpr) / im.width, (stageH * dpr) / im.height);
    const preview = Math.min(1, 2048 / Math.max(im.width, im.height));
    return fit > preview * 1.05;
  }

  // Beim Aussortieren blättert man meist in eine Richtung: das nächste Foto in
  // Blätterrichtung kurz nach dem Anzeigen in voller Auflösung vorbereiten. Das
  // Backend behält die letzten Vollbilder (protocol.rs), beim Weiterblättern
  // ist es dann gleich scharf. Immer nur eins zugleich, und nicht abbrechen –
  // ein abgebrochener Abruf liefe im Backend trotzdem zu Ende, ein zweiter
  // Abruf desselben Bildes rechnete es doppelt.
  let lastIndex = -1;
  let direction = 1;
  let prefetching = false;
  $effect(() => {
    const i = lib.focus;
    if (lib.view !== "loupe" || i < 0) return;
    if (lastIndex >= 0 && i !== lastIndex) direction = i > lastIndex ? 1 : -1;
    lastIndex = i;
    const next = untrack(() => lib.images[i + direction]);
    if (!next || !untrack(() => needsFull(next))) return;
    const src = imageUrl("full", next.id, untrack(() => lib.thumbVersion(next.id)));
    const timer = setTimeout(() => {
      if (prefetching) return;
      prefetching = true;
      loadHi(src, false, () => (prefetching = false));
    }, 600);
    return () => clearTimeout(timer);
  });

  function onkeydown(e: KeyboardEvent) {
    if (lib.view !== "loupe" || isTyping(e)) return;
    if (zoomKey(e, zoom)) return;
    if (e.key === "ArrowRight") lib.move(1);
    else if (e.key === "ArrowLeft") lib.move(-1);
    else if (e.key === "Escape" || e.key === "Enter" || e.key === " ") lib.view = "grid";
    else return;
    e.preventDefault();
  }
</script>

<svelte:window {onkeydown} />

<div class="loupe">
  {#if img}
    {@const v = lib.thumbVersion(img.id)}
    {@const dims = libraryDims(img)}
    <div class="stage" role="presentation" bind:clientWidth={stageW} bind:clientHeight={stageH} oncontextmenu={(e) => lib.openMenu(e)}>
      {#key img.id}
        <!-- Thumbnail sofort, große Vorschau darüber, sobald sie geladen ist -->
        {#if loaded !== img.id && zoom.fit}
          <img class="base" src={imageUrl("thumb", img.id, v)} alt="" />
        {/if}
        <img class="probe" src={imageUrl("preview", img.id, v)} alt="" onload={() => (loaded = img.id)} />
      {/key}
      {#if loaded === img.id}
        <ZoomView
          src={imageUrl("preview", img.id, v)}
          hiSrc={imageUrl("full", img.id, v)}
          fullLong={dims.fullLong}
          srcLong={dims.srcLong}
          {zoom}
          alt={img.fileName}
        />
      {/if}
    </div>
  {/if}
  <Filmstrip />
</div>

<style>
  .loupe {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg);
  }
  .stage {
    position: relative;
    flex: 1;
    min-height: 0;
    margin: 16px;
  }
  .base {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
    filter: blur(0.5px);
  }
  .probe {
    position: absolute;
    width: 1px;
    height: 1px;
    opacity: 0;
    pointer-events: none;
  }
</style>
