<script lang="ts">
  // Farbrad für das Color Grading: Winkel = Farbton (0° = Rot, rechts, gegen
  // den Uhrzeigersinn wie in Lightroom), Abstand zur Mitte = Sättigung.
  import { t } from "../lib/i18n";

  let {
    hue,
    saturation,
    label,
    selected = false,
    size = 104,
    onselect,
    oninput,
    oncommit,
  }: {
    hue: number;
    saturation: number;
    label: string;
    selected?: boolean;
    size?: number;
    onselect: () => void;
    oninput: (hue: number, saturation: number) => void;
    oncommit: () => void;
  } = $props();

  let el: HTMLDivElement;
  let dragging = false;
  /** Rand, den der Punkt frei lässt (sonst ragt er über das Rad) */
  const PAD = 6;
  const r = $derived(size / 2);
  const dot = $derived.by(() => {
    const a = (hue * Math.PI) / 180;
    const d = (saturation / 100) * (r - PAD);
    return { x: r + Math.cos(a) * d, y: r - Math.sin(a) * d };
  });

  function setFrom(e: PointerEvent) {
    const b = el.getBoundingClientRect();
    const x = e.clientX - b.left - r;
    const y = r - (e.clientY - b.top);
    let a = (Math.atan2(y, x) * 180) / Math.PI;
    if (a < 0) a += 360;
    const s = Math.min(100, (Math.hypot(x, y) / (r - PAD)) * 100);
    // Ganz in der Mitte bleibt der Farbton stehen (sonst springt er beim Zurückziehen)
    oninput(s < 1 ? hue : Math.round(a) % 360, Math.round(s));
  }

  function down(e: PointerEvent) {
    if (e.button !== 0) return;
    onselect();
    el.setPointerCapture(e.pointerId);
    dragging = true;
    setFrom(e);
  }
  function move(e: PointerEvent) {
    if (dragging) setFrom(e);
  }
  function up() {
    if (!dragging) return;
    dragging = false;
    oncommit();
  }
</script>

<div class="wrap">
  <div
    bind:this={el}
    class="wheel"
    class:selected
    style="width: {size}px; height: {size}px"
    role="slider"
    tabindex="-1"
    aria-label={label}
    aria-valuemin={0}
    aria-valuemax={100}
    aria-valuenow={saturation}
    aria-valuetext={t("Farbton {h}°, Sättigung {s}", { h: hue, s: saturation })}
    title={t("Ziehen: Farbton und Sättigung · Doppelklick: zurücksetzen")}
    onpointerdown={down}
    onpointermove={move}
    onpointerup={up}
    onpointercancel={up}
    ondblclick={() => {
      oninput(hue, 0);
      oncommit();
    }}
  >
    <span
      class="dot"
      style="left: {dot.x}px; top: {dot.y}px; background: hsl({hue} {saturation}% 50%)"
    ></span>
  </div>
  <button class="label" class:selected tabindex="-1" onclick={onselect}>{label}</button>
</div>

<style>
  .wrap {
    display: grid;
    justify-items: center;
    gap: 4px;
  }
  .wheel {
    position: relative;
    border-radius: 50%;
    /* Mitte grau (keine Tönung), außen volle Farbe */
    background:
      radial-gradient(circle closest-side, #8a8a8a, rgb(138 138 138 / 0.3) 100%),
      conic-gradient(from 90deg, #f00, #f0f, #00f, #0ff, #0f0, #ff0, #f00);
    cursor: crosshair;
    touch-action: none;
    outline: 1px solid var(--line);
    outline-offset: 1px;
  }
  .wheel.selected {
    outline: 2px solid var(--accent);
  }
  .dot {
    position: absolute;
    width: 12px;
    height: 12px;
    margin: -6px 0 0 -6px;
    border-radius: 50%;
    border: 2px solid #fff;
    box-shadow: 0 0 0 1px rgb(0 0 0 / 0.6);
    pointer-events: none;
  }
  .label {
    font-size: var(--step--1);
    color: var(--muted);
    padding: 0 4px;
  }
  .label.selected {
    color: inherit;
    font-weight: 600;
  }
</style>
