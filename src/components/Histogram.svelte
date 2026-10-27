<script lang="ts">
  // Histogramm der aktuellen Vorschau. Wird im Browser aus dem gerenderten Bild
  // berechnet – auf einer verkleinerten Kopie, das reicht für die Form völlig.
  import { t, locale } from "../lib/i18n";
  import { loadBins, peakOf } from "../lib/histogram";

  let {
    src,
    clipping = false,
    onclipping,
  }: {
    src: string | null;
    /** Clipping-Warnung im Bild an */
    clipping?: boolean;
    /** Klick auf die Prozentangaben schaltet die Clipping-Warnung */
    onclipping?: () => void;
  } = $props();

  let canvas: HTMLCanvasElement;
  let clipLow = $state(0);
  let clipHigh = $state(0);
  const W = 256;
  const H = 72;

  $effect(() => {
    if (!src || !canvas) return;
    let cancelled = false;
    loadBins(src)
      .then((r) => {
        if (cancelled) return;
        clipLow = r.clipLow;
        clipHigh = r.clipHigh;
        draw(r.bins.slice(0, 3));
      })
      .catch(() => {});
    return () => (cancelled = true);
  });

  function draw(bins: Uint32Array[]) {
    const ctx = canvas.getContext("2d")!;
    ctx.clearRect(0, 0, W, H);
    // Spitzen an den Rändern (Ausgebranntes) sollen die Skala nicht plattdrücken.
    const peak = peakOf(...bins);
    ctx.globalCompositeOperation = "lighter";
    const colors = ["rgb(214 72 64 / 0.75)", "rgb(76 186 92 / 0.75)", "rgb(70 120 220 / 0.8)"];
    bins.forEach((b, c) => {
      ctx.beginPath();
      ctx.moveTo(0, H);
      for (let i = 0; i < 256; i++) {
        const v = Math.sqrt(Math.min(1, b[i] / peak)); // Wurzel: flache Bereiche bleiben sichtbar
        ctx.lineTo(i, H - v * (H - 2));
      }
      ctx.lineTo(255, H);
      ctx.closePath();
      ctx.fillStyle = colors[c];
      ctx.fill();
    });
    ctx.globalCompositeOperation = "source-over";
  }

  const pct = (v: number) => (v * 100).toLocaleString(locale, { maximumFractionDigits: 1 });
</script>

<figure>
  <canvas bind:this={canvas} width={W} height={H} aria-label={t("Histogramm")}></canvas>
  <figcaption>
    {#if onclipping}
      {@const tip = t("Abgeschnittene Bereiche im Bild zeigen (J)")}
      <button class="clip" class:on={clipping} class:warn={clipLow > 0.01} aria-pressed={clipping} title={tip} onclick={onclipping}
        >{t("Schwarz {p} %", { p: pct(clipLow) })}</button
      >
      <button class="clip" class:on={clipping} class:warn={clipHigh > 0.01} aria-pressed={clipping} title={tip} onclick={onclipping}
        >{t("Lichter {p} %", { p: pct(clipHigh) })}</button
      >
    {:else}
      <span class:warn={clipLow > 0.01} title={t("Anteil reiner Schwarztöne")}>{t("Schwarz {p} %", { p: pct(clipLow) })}</span>
      <span class:warn={clipHigh > 0.01} title={t("Anteil ausgebrannter Lichter")}>{t("Lichter {p} %", { p: pct(clipHigh) })}</span>
    {/if}
  </figcaption>
</figure>

<style>
  figure {
    margin: 0;
  }
  canvas {
    display: block;
    width: 100%;
    height: 72px;
    background: var(--bg);
    border-radius: var(--radius-s);
  }
  figcaption {
    display: flex;
    justify-content: space-between;
    margin-top: 4px;
    font-size: 11px;
    color: var(--faint);
  }
  .warn {
    color: var(--accent);
  }
  .clip {
    padding: 0 4px;
    margin: 0 -4px;
    border: 1px solid transparent;
    border-radius: 4px;
    background: none;
    font: inherit;
    color: inherit;
    cursor: pointer;
  }
  .clip.warn {
    color: var(--accent);
  }
  .clip:hover {
    border-color: var(--line);
  }
  .clip.on {
    border-color: var(--accent);
  }
</style>
