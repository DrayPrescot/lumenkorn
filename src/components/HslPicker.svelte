<script lang="ts">
  // Pipette des HSL-Werkzeugs auf dem Bild: Klicken misst die Farbe in der
  // angezeigten Vorschau, Ziehen nach oben/unten verstellt die getroffenen
  // Farbbereiche anteilig (wie das Zielkorrektur-Werkzeug in Lightroom).
  import { onMount } from "svelte";
  import { dev } from "../lib/develop.svelte";
  import { HSL_BANDS, hslWeights, hueOf, samplePixel } from "../lib/hsl";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";

  let { stageW, stageH, imgW, imgH }: { stageW: number; stageH: number; imgW: number; imgH: number } = $props();

  // Angezeigtes Bild (object-fit: contain), wie in MaskOverlay
  const rect = $derived.by(() => {
    if (!imgW || !imgH || !stageW || !stageH) return { x: 0, y: 0, w: 0, h: 0 };
    const s = Math.min(stageW / imgW, stageH / imgH);
    const w = imgW * s;
    const h = imgH * s;
    return { x: (stageW - w) / 2, y: (stageH - h) / 2, w, h };
  });

  let el: HTMLDivElement;
  let drag: { y: number; base: number[]; weights: number[] } | null = null;
  /** Taste noch gedrückt? (Das Abtasten ist asynchron – kurzer Klick endet vorher.) */
  let pressed = false;
  let label = $state<{ x: number; y: number; text: string } | null>(null);

  /** Bildschirm → normierte Position in der Vorschau (robust gegen Zoom-Skalierung) */
  function norm(e: PointerEvent): { nx: number; ny: number; px: number; py: number } {
    const r = el.getBoundingClientRect();
    const px = ((e.clientX - r.left) / r.width) * stageW;
    const py = ((e.clientY - r.top) / r.height) * stageH;
    return { nx: (px - rect.x) / rect.w, ny: (py - rect.y) / rect.h, px, py };
  }

  async function ondown(e: PointerEvent) {
    if (e.button !== 0 || !dev.url) return;
    const { nx, ny, px, py } = norm(e);
    if (nx < 0 || ny < 0 || nx > 1 || ny > 1) return;
    el.setPointerCapture(e.pointerId);
    pressed = true;
    const startY = e.clientY;
    const [r, g, b] = await samplePixel(dev.url, nx, ny);
    if (!pressed) return;
    const { hue, chroma } = hueOf(r, g, b);
    if (chroma < 0.02) {
      lib.notify(t("Hier ist kaum Farbe – die Pipette wirkt auf bunte Bereiche."));
      return;
    }
    const weights = hslWeights(hue);
    drag = { y: startY, base: [...(dev.params.hsl ?? { hue: [], saturation: [], luminance: [] })[dev.hslMode]], weights };
    const parts = weights
      .map((w, i) => ({ w, i }))
      .filter((x) => x.w > 0.05)
      .sort((a, b) => b.w - a.w)
      .map((x) => t("{name} {p} %", { name: HSL_BANDS[x.i].name, p: Math.round(x.w * 100) }));
    label = { x: px, y: py, text: parts.join(" · ") };
  }

  function onmove(e: PointerEvent) {
    if (!drag) return;
    const delta = (drag.y - e.clientY) * 0.5; // 2 px Weg = 1 Punkt
    dev.setHslWeighted(dev.hslMode, drag.base, drag.weights, delta);
  }

  function onup() {
    pressed = false;
    if (drag) dev.commit();
    drag = null;
    label = null;
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      dev.hslPick = false;
    }
  }

  onMount(() => () => (drag = null));
</script>

<svelte:window {onkeydown} />

<div
  bind:this={el}
  class="picker"
  role="presentation"
  onpointerdown={ondown}
  onpointermove={onmove}
  onpointerup={onup}
  onpointercancel={onup}
>
  {#if label}
    <span class="label" style:left="{label.x + 14}px" style:top="{label.y - 28}px">{label.text}</span>
  {/if}
</div>

<style>
  .picker {
    position: absolute;
    inset: 0;
    cursor: crosshair;
    touch-action: none;
  }
  .label {
    position: absolute;
    padding: 3px 7px;
    border-radius: var(--radius-s);
    background: rgb(0 0 0 / 0.75);
    color: #fff;
    font-size: var(--step--1);
    white-space: nowrap;
    pointer-events: none;
  }
</style>
