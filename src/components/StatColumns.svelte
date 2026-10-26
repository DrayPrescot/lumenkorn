<script lang="ts">
  import { num, t } from "../lib/i18n";
  import type { BarItem } from "./StatBars.svelte";

  // Senkrechte Säulen für Verläufe (Monate, Tageszeit). Beim Darüberfahren
  // steht der Wert über dem Diagramm; Achsenbeschriftung nur an `ticks`.
  let { items, ticks }: { items: BarItem[]; ticks: (i: number) => string | null } = $props();

  let hover = $state<number | null>(null);
  const max = $derived(Math.max(1, ...items.map((i) => i.count)));
  const readout = $derived(
    hover !== null && items[hover]
      ? `${items[hover].hint ?? items[hover].label}: ${t(items[hover].count === 1 ? "{n} Foto" : "{n} Fotos", { n: num(items[hover].count) })}`
      : "",
  );
</script>

<div class="readout" aria-live="polite">{readout || " "}</div>
<div
  class="cols"
  role="img"
  aria-label={items.map((i) => `${i.hint ?? i.label}: ${i.count}`).join(", ")}
  onpointermove={(e) => {
    const i = (e.target as HTMLElement).closest<HTMLElement>("[data-i]")?.dataset.i;
    hover = i === undefined ? null : Number(i);
  }}
  onpointerleave={() => (hover = null)}
>
  {#each items as it, i (i)}
    <div class="col" class:hot={hover === i} data-i={i} title={`${it.hint ?? it.label}: ${num(it.count)}`}>
      <span class="bar" style:height="{it.count ? Math.max(2, (100 * it.count) / max) : 0}%"></span>
    </div>
  {/each}
</div>
<div class="axis">
  {#each items as _, i (i)}
    <span>{ticks(i) ?? ""}</span>
  {/each}
</div>

<style>
  .readout {
    font-size: var(--step--1);
    color: var(--muted);
    min-height: 1.4em;
    margin-bottom: 4px;
  }
  .cols {
    display: flex;
    align-items: flex-end;
    gap: 2px;
    height: 120px;
    border-bottom: 1px solid var(--line);
  }
  .col {
    flex: 1;
    min-width: 0;
    height: 100%;
    display: flex;
    align-items: flex-end;
  }
  .bar {
    display: block;
    width: 100%;
    background: var(--accent);
    border-radius: 4px 4px 0 0;
  }
  .col.hot .bar {
    background: #f0bf5e;
  }
  .col.hot {
    background: rgb(255 255 255 / 0.04);
  }
  .axis {
    display: flex;
    gap: 2px;
    margin-top: 4px;
  }
  .axis span {
    flex: 1;
    min-width: 0;
    font-size: 11px;
    color: var(--faint);
    white-space: nowrap;
    overflow: visible;
  }
</style>
