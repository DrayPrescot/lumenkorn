<script lang="ts">
  // Schriftstil der Beschriftung (Datum/Ort) und Vorgabe für neue Texte:
  // Ausrichtung, Größe, Farbe, Fett. Im Dialog „Fotoalbum“ und in der
  // Album-Leiste.
  import type { TextStyle } from "../lib/album";
  import { t } from "../lib/i18n";

  /** `stacked`: Beschriftung über den Reglern (schmale Seitenleiste) */
  let { value, onchange, stacked = false }: { value: TextStyle; onchange: (v: TextStyle) => void; stacked?: boolean } = $props();

  const set = (patch: Partial<TextStyle>) => onchange({ ...value, ...patch });
  // Größe erst beim Loslassen übernehmen: Jede Änderung ordnet alle Seiten mit
  // Beschriftung neu an. Die Anzeige folgt dem Regler sofort.
  let dragging = $state<number | null>(null);
</script>

<div class="row" class:stacked>
  <span>{t("Ausrichtung")}</span>
  <span class="seg">
    {#each [["left", t("Links")], ["center", t("Mitte")], ["right", t("Rechts")]] as [v, l]}
      <button type="button" class="btn quiet" class:on={value.align === v} onclick={() => set({ align: v as TextStyle["align"] })}>{l}</button>
    {/each}
  </span>
</div>
<label class="row" class:stacked>
  <span>{t("Schriftgröße")}</span>
  <span class="size">
    <input
      type="range"
      min="7"
      max="36"
      step="1"
      value={value.size}
      oninput={(e) => (dragging = Number(e.currentTarget.value))}
      onchange={(e) => {
        dragging = null;
        set({ size: Number(e.currentTarget.value) });
      }}
    />
    <span class="muted">{dragging ?? value.size} pt</span>
  </span>
</label>
<div class="row" class:stacked>
  <span>{t("Farbe")}</span>
  <span class="seg">
    {#each [["auto", t("Automatisch")], ["dark", t("Dunkel")], ["light", t("Hell")]] as [v, l]}
      <button type="button" class="btn quiet" class:on={value.color === v} onclick={() => set({ color: v as TextStyle["color"] })}>{l}</button>
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
