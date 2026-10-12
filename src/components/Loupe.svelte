<script lang="ts">
  import { isTyping } from "../lib/keys";
  import { imageUrl } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { ZoomState, zoomKey } from "../lib/zoom.svelte";
  import { libraryDims } from "../lib/layout";
  import Filmstrip from "./Filmstrip.svelte";
  import ZoomView from "./ZoomView.svelte";

  const img = $derived(lib.focused);
  // Zoom bleibt beim Blättern erhalten – so lassen sich Serien vergleichen.
  const zoom = new ZoomState();
  let loaded = $state<number | null>(null);

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
    <div class="stage" role="presentation" oncontextmenu={(e) => lib.openMenu(e)}>
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
