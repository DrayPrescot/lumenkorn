<script lang="ts">
  // HSL-Werkzeug im Reiter „Basis“: Farbton, Sättigung und Luminanz je
  // Farbbereich – mit Pipette, die auf dem Bild die getroffenen Bereiche verstellt.
  import { DEFAULT_PARAMS } from "../lib/api";
  import { dev } from "../lib/develop.svelte";
  import { HSL_BANDS, type HslMode } from "../lib/hsl";
  import Icon from "./Icon.svelte";
  import Slider from "./Slider.svelte";
  import { t } from "../lib/i18n";

  const modes: { id: HslMode; label: string }[] = [
    { id: "hue", label: t("Farbton") },
    { id: "saturation", label: t("Sättigung") },
    { id: "luminance", label: t("Luminanz") },
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
  <summary tabindex="-1">HSL</summary>
  <div class="head">
    <div class="seg" role="tablist" aria-label={t("HSL-Regler")}>
      {#each modes as m (m.id)}
        <!-- Tab springt von Regler zu Regler; Knöpfe per Maus -->
        <button role="tab" tabindex="-1" aria-selected={dev.hslMode === m.id} class:on={dev.hslMode === m.id} onclick={() => (dev.hslMode = m.id)}>
          {m.label}
        </button>
      {/each}
    </div>
    <button
      tabindex="-1"
      class="icon-btn pick"
      class:on={dev.hslPick}
      aria-pressed={dev.hslPick}
      aria-label={t("Pipette")}
      title={t("Pipette: auf eine Farbe im Bild klicken und nach oben/unten ziehen")}
      onclick={() => (dev.hslPick = !dev.hslPick)}
    >
      <Icon name="pipette" size={15} />
    </button>
  </div>

  {#if dev.hslPick}
    <p class="hint">
      {t("Auf eine Farbe im Bild klicken und nach oben (mehr) oder unten (weniger) ziehen – verstellt wird")}
      {dev.hslMode === "hue" ? t("der Farbton") : dev.hslMode === "saturation" ? t("die Sättigung") : t("die Luminanz")}
      {t("der getroffenen Farbbereiche. Esc beendet.")}
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
    <button class="btn small reset" tabindex="-1" onclick={reset}>{t("HSL zurücksetzen")}</button>
  {/if}
</details>

<style>
  /* wie die übrigen Abschnitte in DevelopPanel */
  .hsl {
    padding: 12px 16px;
    border-bottom: 1px solid var(--line);
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
