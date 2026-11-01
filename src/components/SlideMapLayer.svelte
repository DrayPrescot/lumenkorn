<script lang="ts" module>
  import { mapData, type MapData } from "../lib/slideMap";

  // Kartendaten einmal laden, für alle Ebenen gemeinsam
  let shared = $state<MapData | null>(null);
  let loading = false;
  function ensureData() {
    if (shared || loading) return;
    loading = true;
    mapData()
      .then((d) => (shared = d))
      .catch(() => {})
      .finally(() => (loading = false));
  }
</script>

<script lang="ts">
  // Karte beim Ortswechsel in der Bühne der Diashow (Vorschau und Vollbild)
  import { drawMapLeg, type MapLeg } from "../lib/slideMap";
  import { fontFamily } from "../lib/album";

  let { leg, p }: { leg: MapLeg; p: number } = $props();

  let canvas = $state<HTMLCanvasElement>();
  let w = $state(0);
  let h = $state(0);
  const font = fontFamily("sans");
  ensureData();

  $effect(() => {
    if (!canvas || !w || !h) return;
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    const cw = Math.round(w * dpr);
    const ch = Math.round(h * dpr);
    if (canvas.width !== cw || canvas.height !== ch) {
      canvas.width = cw;
      canvas.height = ch;
    }
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    drawMapLeg(ctx, cw, ch, leg, p, shared, font);
  });
</script>

<div class="map" bind:clientWidth={w} bind:clientHeight={h}>
  <canvas bind:this={canvas}></canvas>
</div>

<style>
  .map {
    position: absolute;
    inset: 0;
  }
  canvas {
    width: 100%;
    height: 100%;
    display: block;
  }
</style>
