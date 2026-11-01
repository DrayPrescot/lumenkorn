<script lang="ts">
  // Stil eines Textes der Diashow: Ausrichtung, Lage, Größe, Farbe, Fett.
  // Wie `TextStyleFields` beim Fotoalbum, die Größe aber in Prozent der
  // Bildhöhe (gleich in Vorschau, Vollbild und Video).
  import { MAX_TEXT_SIZE, MIN_TEXT_SIZE, type SlideTextStyle } from "../lib/slideshow";
  import { t } from "../lib/i18n";

  /** `stacked`: Beschriftung über den Reglern (schmale Seitenleiste) */
  let { value, onchange, stacked = false }: { value: SlideTextStyle; onchange: (v: SlideTextStyle) => void; stacked?: boolean } = $props();

  const set = (patch: Partial<SlideTextStyle>) => onchange({ ...value, ...patch });
  // Größe erst beim Loslassen übernehmen, die Anzeige folgt dem Regler sofort
  let dragging = $state<number | null>(null);
</script>

<div class="row" class:stacked>
  <span>{t("Ausrichtung")}</span>
  <span class="seg">
    {#each [["left", t("Links")], ["center", t("Mitte")], ["right", t("Rechts")]] as [v, l]}
      <button type="button" class="btn quiet" class:on={value.align === v} onclick={() => set({ align: v as SlideTextStyle["align"] })}>{l}</button>
    {/each}
  </span>
</div>
<div class="row" class:stacked>
  <span>{t("Lage")}</span>
  <span class="seg">
    {#each [["top", t("Oben")], ["middle", t("Mitte")], ["bottom", t("Unten")]] as [v, l]}
      <button type="button" class="btn quiet" class:on={value.pos === v} onclick={() => set({ pos: v as SlideTextStyle["pos"] })}>{l}</button>
    {/each}
  </span>
</div>
<label class="row" class:stacked>
  <span>{t("Schriftgröße")}</span>
  <span class="size">
    <input
      type="range"
      min={MIN_TEXT_SIZE}
      max={MAX_TEXT_SIZE}
      step="0.1"
      value={value.size}
      oninput={(e) => (dragging = Number(e.currentTarget.value))}
      onchange={(e) => {
        dragging = null;
        set({ size: Number(e.currentTarget.value) });
      }}
    />
    <span class="muted" title={t("Prozent der Bildhöhe")}>{(dragging ?? value.size).toLocaleString(undefined, { maximumFractionDigits: 1 })} %</span>
  </span>
</label>
<div class="row" class:stacked>
  <span>{t("Farbe")}</span>
  <span class="seg">
    {#each [["light", t("Hell")], ["dark", t("Dunkel")]] as [v, l]}
      <button type="button" class="btn quiet" class:on={value.color === v} onclick={() => set({ color: v as SlideTextStyle["color"] })}>{l}</button>
    {/each}
    <label class="bold"><input type="checkbox" checked={value.bold} onchange={(e) => set({ bold: e.currentTarget.checked })} /> {t("Fett")}</label>
  </span>
</div>

<style>
  .row {
    display: grid;
    grid-template-columns: var(--label-width, 130px) 1fr;
    align-items: center;
    gap: 10px;
  }
  .row > span:first-child {
    color: var(--muted);
  }
  .row.stacked {
    grid-template-columns: 1fr;
    gap: 4px;
  }
  .row.stacked > span:first-child {
    font-size: var(--step--1);
  }
  .seg {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
  }
  .seg .on {
    color: var(--accent);
    border-color: var(--accent);
  }
  .size {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .size input {
    flex: 1;
    min-width: 0;
  }
  .bold {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-left: 8px;
  }
  .muted {
    color: var(--muted);
    white-space: nowrap;
  }
</style>
