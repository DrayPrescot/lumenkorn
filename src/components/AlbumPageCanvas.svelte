<script lang="ts">
  // Texte und Karte einer Albumseite als Ebene über den Fotos – derselbe
  // Zeichencode wie beim Export (albumDraw.ts), nur in Bildschirmauflösung.
  import { untrack } from "svelte";
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
  // Neu zeichnen nur, wenn sich diese Seite ändert (Texte, Karte), die
  // Einstellungen oder – bei der Karte – die Orte; nicht bei Änderungen an
  // anderen Seiten
  const key = $derived(
    JSON.stringify([
      a.pages[page],
      a.settings,
      a.pages[page]?.map ? a.candidates.map((c) => [c.lat, c.lon, c.place]) : null,
    ]),
  );

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
    // Abhängigkeiten: `key`, Layout, Größe, Kartendaten, Schriften
    void key;
    void fontsReady;
    const l = layout;
    const m = map;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    if (!untrack(() => needed)) return;
    untrack(() => {
      const pxPerMm = (width * dpr) / a.settings.widthMm;
      ctx.scale(pxPerMm, pxPerMm);
      drawPage(ctx, a, page, l, m);
    });
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
