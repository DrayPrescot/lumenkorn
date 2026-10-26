<script lang="ts" module>
  export interface BarItem {
    label: string;
    count: number;
    /** Zusatz im Tooltip (z. B. genauer Bereich) */
    hint?: string;
  }
</script>

<script lang="ts">
  import { num, t } from "../lib/i18n";

  // Waagerechte Balken, ein Farbton, Länge relativ zum größten Eintrag.
  // `total`: Anteil im Tooltip; `limit`: so viele zeigen, Rest aufklappbar.
  let {
    items,
    total,
    limit = 10,
    onpick,
    pickHint,
  }: {
    items: BarItem[];
    total: number;
    limit?: number;
    onpick?: (item: BarItem) => void;
    pickHint?: string;
  } = $props();

  let all = $state(false);
  const max = $derived(Math.max(1, ...items.map((i) => i.count)));
  const shown = $derived(all ? items : items.slice(0, limit));
  const pct = (n: number) => num(total ? (100 * n) / total : 0, { maximumFractionDigits: n / total < 0.01 ? 1 : 0 });
</script>

<ul class="bars">
  {#each shown as it (it.label)}
    {@const tip = `${it.hint ?? it.label}: ${t(it.count === 1 ? "{n} Foto" : "{n} Fotos", { n: num(it.count) })} (${pct(it.count)} %)${onpick ? `\n${pickHint ?? ""}` : ""}`}
    <li>
      {#if onpick}
        <button class="row pick" title={tip} onclick={() => onpick(it)}>
          <span class="lbl">{it.label}</span>
          <span class="track"><span class="bar" style:width="{(100 * it.count) / max}%"></span></span>
          <span class="n">{num(it.count)}</span>
        </button>
      {:else}
        <div class="row" title={tip}>
          <span class="lbl">{it.label}</span>
          <span class="track"><span class="bar" style:width="{(100 * it.count) / max}%"></span></span>
          <span class="n">{num(it.count)}</span>
        </div>
      {/if}
    </li>
  {/each}
</ul>
{#if items.length > limit}
  <button class="more" onclick={() => (all = !all)}>
    {all ? t("Weniger zeigen") : t("Alle {n} zeigen", { n: items.length })}
  </button>
{/if}

<style>
  .bars {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 2px;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(0, 11rem) 1fr 4.5rem;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 3px 4px;
    border-radius: var(--radius-s);
    text-align: left;
    font-size: var(--step-0);
  }
  .pick:hover {
    background: var(--raised);
  }
  .pick:hover .lbl {
    color: var(--accent);
  }
  .lbl {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .track {
    height: 10px;
    display: block;
  }
  .bar {
    display: block;
    height: 100%;
    min-width: 2px;
    background: var(--accent);
    border-radius: 0 4px 4px 0;
  }
  .n {
    text-align: right;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }
  .more {
    margin-top: 6px;
    color: var(--accent);
    font-size: var(--step--1);
  }
  .more:hover {
    text-decoration: underline;
  }
</style>
