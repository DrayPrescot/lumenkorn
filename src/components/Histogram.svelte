<script lang="ts">
  // Histogramm der aktuellen Vorschau. Wird im Browser aus dem gerenderten Bild
  // berechnet – auf einer verkleinerten Kopie, das reicht für die Form völlig.
  import { t, locale } from "../lib/i18n";

  let { src }: { src: string | null } = $props();

  let canvas: HTMLCanvasElement;
  let clipLow = $state(0);
  let clipHigh = $state(0);
  const W = 256;
  const H = 72;

  $effect(() => {
    if (!src || !canvas) return;
    const img = new Image();
    let cancelled = false;
    img.onload = () => {
      if (cancelled) return;
      const scale = Math.min(1, 320 / Math.max(img.width, img.height));
      const w = Math.max(1, Math.round(img.width * scale));
      const h = Math.max(1, Math.round(img.height * scale));
      const off = document.createElement("canvas");
      off.width = w;
      off.height = h;
      const octx = off.getContext("2d", { willReadFrequently: true })!;
      octx.drawImage(img, 0, 0, w, h);
      const px = octx.getImageData(0, 0, w, h).data;
      const bins = [new Uint32Array(256), new Uint32Array(256), new Uint32Array(256)];
      let lo = 0;
      let hi = 0;
      for (let i = 0; i < px.length; i += 4) {
        bins[0][px[i]]++;
        bins[1][px[i + 1]]++;
        bins[2][px[i + 2]]++;
        const m = Math.max(px[i], px[i + 1], px[i + 2]);
        if (m >= 254) hi++;
        if (Math.max(px[i], px[i + 1], px[i + 2]) <= 1) lo++;
      }
      const n = px.length / 4;
      clipLow = lo / n;
      clipHigh = hi / n;
      draw(bins);
    };
    img.src = src;
    return () => (cancelled = true);
  });

  function draw(bins: Uint32Array[]) {
    const ctx = canvas.getContext("2d")!;
    ctx.clearRect(0, 0, W, H);
    // Spitzen an den Rändern (Ausgebranntes) sollen die Skala nicht plattdrücken.
    let peak = 1;
    for (const b of bins) for (let i = 2; i < 254; i++) peak = Math.max(peak, b[i]);
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
    <span class:warn={clipLow > 0.01} title={t("Anteil reiner Schwarztöne")}>{t("Schwarz {p} %", { p: pct(clipLow) })}</span>
    <span class:warn={clipHigh > 0.01} title={t("Anteil ausgebrannter Lichter")}>{t("Lichter {p} %", { p: pct(clipHigh) })}</span>
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
</style>
