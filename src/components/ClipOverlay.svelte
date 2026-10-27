<script lang="ts">
  // Clipping-Warnung: ausgebrannte Lichter rot, reines Schwarz blau – dieselben
  // Schwellen wie die Prozentangaben unter dem Histogramm.
  import { containRect } from "../lib/frame";
  import { clipMask } from "../lib/histogram";

  let {
    src,
    stageW,
    stageH,
    imgW,
    imgH,
    view = null,
  }: {
    src: string;
    stageW: number;
    stageH: number;
    imgW: number;
    imgH: number;
    /** Lage des Bildes beim Zoomen (ZoomView), sonst eingepasst */
    view?: { x: number; y: number; w: number; h: number } | null;
  } = $props();

  const rect = $derived(view ?? containRect(stageW, stageH, imgW, imgH));
  let canvas: HTMLCanvasElement;

  $effect(() => {
    if (!canvas) return;
    let cancelled = false;
    clipMask(src)
      .then((mask) => {
        if (cancelled) return;
        canvas.width = mask.width;
        canvas.height = mask.height;
        canvas.getContext("2d")!.putImageData(mask, 0, 0);
      })
      .catch(() => {});
    return () => (cancelled = true);
  });
</script>

<canvas
  bind:this={canvas}
  class="clip"
  style:left="{rect.x}px"
  style:top="{rect.y}px"
  style:width="{rect.w}px"
  style:height="{rect.h}px"
></canvas>

<style>
  .clip {
    position: absolute;
    pointer-events: none;
    image-rendering: pixelated;
  }
</style>
