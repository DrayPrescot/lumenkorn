<script lang="ts">
  // Zeichnet einen Zeitpunkt der Diashow (`frameAt`) in die Fläche: Fotos mit
  // Bewegung und Übergang, Beschriftung, Titel, Ein- und Ausblenden. Schrift
  // wächst mit der Fläche (Containerhöhe), damit Vorschau und Vollbild gleich
  // aussehen.
  import { imageUrl } from "../lib/api";
  import type { Frame } from "../lib/slideshow";
  import { lib } from "../lib/store.svelte";
  import SlideMapLayer from "./SlideMapLayer.svelte";

  let { frame, title }: { frame: Frame; title: string } = $props();
</script>

<div class="stage">
  {#each frame.layers as l (l.index)}
    {@const src = imageUrl("preview", l.id, lib.thumbVersion(l.id))}
    <div class="layer" style:opacity={l.opacity} style:transform="translateX({l.shift * 100}%)">
      {#if l.map}
        <SlideMapLayer leg={l.map.leg} p={l.map.p} />
      {:else}
        {#if l.contain}
          <img class="backdrop" {src} alt="" draggable="false" />
        {/if}
        <img
          class="photo"
          class:contain={l.contain}
          {src}
          alt=""
          draggable="false"
          style:object-position="{l.pos[0] * 100}% {l.pos[1] * 100}%"
          style:transform="translate({l.tx * 100}%, {l.ty * 100}%) scale({l.scale})"
        />
      {/if}
      {#if l.caption && l.captionOpacity > 0}
        <div class="caption" style:opacity={l.captionOpacity}>{l.caption}</div>
      {/if}
    </div>
  {/each}
  {#if title.trim() && frame.title > 0}
    <div class="title" style:opacity={frame.title}>{title}</div>
  {/if}
  {#if frame.black > 0}
    <div class="black" style:opacity={frame.black}></div>
  {/if}
</div>

<style>
  .stage {
    position: absolute;
    inset: 0;
    overflow: hidden;
    background: #000;
    container-type: size;
  }
  .layer {
    position: absolute;
    inset: 0;
    overflow: hidden;
    will-change: opacity, transform;
  }
  img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    display: block;
    user-select: none;
    pointer-events: none;
  }
  .photo {
    object-fit: cover;
    will-change: transform;
  }
  .photo.contain {
    object-fit: contain;
  }
  /* Hochformat: dasselbe Foto unscharf und dunkler dahinter */
  .backdrop {
    object-fit: cover;
    filter: blur(28px) brightness(0.55);
    transform: scale(1.15);
  }
  .caption {
    position: absolute;
    left: 5cqw;
    bottom: 9cqh;
    max-width: 70cqw;
    color: #fff;
    font-size: 3.6cqh;
    font-weight: 500;
    line-height: 1.25;
    text-shadow:
      0 0 0.4em rgb(0 0 0 / 0.7),
      0 0.05em 0.15em rgb(0 0 0 / 0.8);
    white-space: pre-line;
  }
  .title {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding: 0 8cqw;
    text-align: center;
    color: #fff;
    font-size: 8cqh;
    font-weight: 600;
    line-height: 1.15;
    text-shadow:
      0 0 0.5em rgb(0 0 0 / 0.6),
      0 0.04em 0.12em rgb(0 0 0 / 0.8);
    background: radial-gradient(ellipse at center, rgb(0 0 0 / 0.35), transparent 70%);
    white-space: pre-line;
  }
  .black {
    position: absolute;
    inset: 0;
    background: #000;
  }
</style>
