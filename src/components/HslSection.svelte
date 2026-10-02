<script lang="ts">
  // HSL-Werkzeug im Reiter „Basis“: Farbton, Sättigung und Luminanz je
  // Farbbereich – mit Pipette, die auf dem Bild die getroffenen Bereiche verstellt.
  import { DEFAULT_PARAMS } from "../lib/api";
  import { dev } from "../lib/develop.svelte";
  import { HSL_BANDS, type HslMode } from "../lib/hsl";
  import Icon from "./Icon.svelte";
  import Slider from "./Slider.svelte";

  const modes: { id: HslMode; label: string }[] = [
    { id: "hue", label: "Farbton" },
    { id: "saturation", label: "Sättigung" },
    { id: "luminance", label: "Luminanz" },
  ];

  const hsl = $derived(dev.params.hsl ?? DEFAULT_PARAMS.hsl);
  const changed = $derived([...hsl.hue, ...hsl.saturation, ...hsl.luminance].some((v) => v !== 0));

  function track(i: number): string {
    const c = HSL_BANDS[i].color;
    if (dev.hslMode === "hue") {
      const prev = HSL_BANDS[(i + 7) % 8].color;
      const next = HSL_BANDS[(i + 1) % 8].color;
      return `linear-gradient(to right, ${prev}, ${c}, ${next})`;
    }
    if (dev.hslMode === "saturation") return `linear-gradient(to right, #8a8a8a, ${c})`;
    return `linear-gradient(to right, #1d1d1d, ${c}, #f2f2f2)`;
  }

  function reset() {
    dev.params.hsl = JSON.parse(JSON.stringify(DEFAULT_PARAMS.hsl));
    dev.schedule();
    dev.commit();
  }
</script>

<details open class="hsl">
  <summary>HSL</summary>
  <div class="head">
    <div class="seg" role="tablist" aria-label="HSL-Regler">
      {#each modes as m (m.id)}
        <button role="tab" aria-selected={dev.hslMode === m.id} class:on={dev.hslMode === m.id} onclick={() => (dev.hslMode = m.id)}>
          {m.label}
        </button>
      {/each}
    </div>
    <button
      class="icon-btn pick"
      class:on={dev.hslPick}
      aria-pressed={dev.hslPick}
      aria-label="Pipette"
      title="Pipette: auf eine Farbe im Bild klicken und nach oben/unten ziehen"
      onclick={() => (dev.hslPick = !dev.hslPick)}
    >
      <Icon name="pipette" size={15} />
    </button>
  </div>

  {#if dev.hslPick}
    <p class="hint">
      Auf eine Farbe im Bild klicken und nach oben (mehr) oder unten (weniger) ziehen – verstellt wird
      {dev.hslMode === "hue" ? "der Farbton" : dev.hslMode === "saturation" ? "die Sättigung" : "die Luminanz"}
      der getroffenen Farbbereiche. Esc beendet.
    </p>
  {/if}

  <div class="sliders">
    {#each HSL_BANDS as b, i (b.name)}
      <Slider
        label={b.name}
        value={hsl[dev.hslMode][i]}
        min={-100}
        max={100}
        track={track(i)}
        oninput={(v) => dev.setHsl(dev.hslMode, i, v)}
        oncommit={() => dev.commit()}
      />
    {/each}
  </div>
  {#if changed}
    <button class="btn small reset" onclick={reset}>HSL zurücksetzen</button>
  {/if}
</details>

<style>
  /* wie die übrigen Abschnitte in DevelopPanel */
  .hsl {
    padding: 12px 16px;
    border-bottom: 1px solid var(--line);
  }
  summary {
    cursor: pointer;
    font-weight: 600;
    font-size: var(--step--1);
    color: var(--muted);
    list-style: none;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  summary::before {
    content: "";
    width: 0;
    height: 0;
    border-left: 4px solid currentColor;
    border-top: 4px solid transparent;
    border-bottom: 4px solid transparent;
    transition: transform 0.12s;
  }
  details[open] > summary::before {
    transform: rotate(90deg);
  }
  summary::-webkit-details-marker {
    display: none;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 8px;
  }
  .seg {
    display: flex;
    flex: 1;
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
    overflow: hidden;
  }
  .seg button {
    flex: 1;
    padding: 4px 0;
    font-size: var(--step--1);
    color: var(--muted);
  }
  .seg button.on {
    background: var(--raised);
    color: inherit;
  }
  .pick.on {
    color: var(--accent-ink);
    background: var(--accent);
  }
  .hint {
    margin: 8px 0 0;
    color: var(--faint);
    font-size: var(--step--1);
  }
  .sliders {
    display: grid;
    gap: 8px;
    margin-top: 10px;
  }
  .reset {
    margin-top: 8px;
  }
</style>
