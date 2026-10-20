<script lang="ts">
  // Texte und Karte einer Albumseite als Ebene über den Fotos – derselbe
  // Zeichencode wie beim Export (albumDraw.ts), nur in Bildschirmauflösung.
  import type { PageLayout } from "../lib/album";
  import { album } from "../lib/album.svelte";
  import { countries, drawPage, ensureFonts, hasOverlay } from "../lib/albumDraw";
  import type { Country } from "../lib/countries";

  let { page, layout }: { page: number; layout: PageLayout } = $props();

  let canvas = $state<HTMLCanvasElement>();
  let width = $state(0);
  let height = $state(0);
  let map = $state<Country[] | null>(null);
  let fontsReady = $state(0);

  const a = $derived(album.state!);
  const needed = $derived(hasOverlay(a, page));

  // Kartendaten und Schriften einmal nachladen, dann neu zeichnen
  $effect(() => {
    if (a.pages[page]?.map && !map) countries().then((c) => (map = c));
  });
  $effect(() => {
    const font = a.settings.font;
    ensureFonts(font).then(() => fontsReady++);
  });

  $effect(() => {
    if (!canvas || !width || !height) return;
    // Abhängigkeiten: Seite (Texte, Karte), Einstellungen, Layout
    const snap = $state.snapshot(a);
    void fontsReady;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    if (!needed) return;
    const pxPerMm = (width * dpr) / snap.settings.widthMm;
    ctx.scale(pxPerMm, pxPerMm);
    drawPage(ctx, snap as typeof a, page, layout, map);
  });
</script>

<canvas bind:this={canvas} bind:clientWidth={width} bind:clientHeight={height} aria-hidden="true"></canvas>

<style>
  canvas {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
    z-index: 2;
  }
</style>
