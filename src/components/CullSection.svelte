<script lang="ts">
  // Aussortier-Assistent in den Einstellungen: was er aus den Entscheidungen
  // gelernt hat (je Grund), zurücksetzen.
  import { onMount } from "svelte";
  import { api, type CullStatus } from "../lib/api";
  import { reasonLabel } from "../lib/cull";
  import { lib } from "../lib/store.svelte";
  import { num, t } from "../lib/i18n";

  let status = $state<CullStatus | null>(null);
  let confirmReset = $state(false);

  async function refresh() {
    try {
      status = await api.cullStatus();
    } catch {
      status = null;
    }
  }
  onMount(refresh);

  async function reset() {
    await api.cullReset();
    confirmReset = false;
    await refresh();
    lib.notify(t("Der Aussortier-Assistent hat alles Gelernte vergessen und schlägt wieder vor wie ab Werk."));
  }

  /** Wie sich der Grund durch das Lernen verschoben hat */
  function tendency(threshold: number, base: number): string {
    if (threshold > base + 0.02) return t("schlägt seltener vor");
    if (threshold < base - 0.02) return t("schlägt öfter vor");
    return t("wie ab Werk");
  }
</script>

<div class="sep"></div>
<h4>{t("Aussortier-Assistent")}</h4>
<p class="muted">
  {t("Der Assistent („Aussortieren …“ im Rechtsklick-Menü von Ordnern und Projekten) lernt aus deinen Entscheidungen: welche Vorschläge du abwählst, welche Grenzfälle du doch ablehnst und welche Ablehnungen du später wieder aufhebst. Je Grund verschiebt sich die Schwelle erst, wenn genug Entscheidungen vorliegen. Alles bleibt auf diesem Rechner.")}
</p>
{#if status}
  {#if status.samples}
    <ul class="reasons">
      {#each status.reasons as r (r.reason)}
        <li>
          <strong>{reasonLabel(r.reason)}</strong>:
          {#if r.samples}
            {t("{n} Entscheidungen, davon {rejected} abgelehnt", { n: num(r.samples), rejected: num(r.rejected) })} ·
            {tendency(r.threshold, status.base)}
          {:else}
            <span class="muted">{t("noch keine Entscheidungen")}</span>
          {/if}
        </li>
      {/each}
    </ul>
  {:else}
    <p class="muted small">{t("Noch nichts gelernt.")}</p>
  {/if}
  <div class="actions">
    {#if confirmReset}
      <span>{t("Gelerntes wirklich löschen?")}</span>
      <button class="btn quiet" onclick={() => (confirmReset = false)}>{t("Abbrechen")}</button>
      <button class="btn danger" onclick={reset}>{t("Löschen")}</button>
    {:else}
      <button class="btn" disabled={!status.samples} onclick={() => (confirmReset = true)}>{t("Gelerntes zurücksetzen")}</button>
    {/if}
  </div>
{/if}

<style>
  .sep {
    border-top: 1px solid var(--line);
  }
  h4 {
    margin: 0;
    font-size: var(--step-0);
    font-weight: 600;
  }
  p {
    margin: 0;
  }
  .muted {
    color: var(--muted);
  }
  .small {
    font-size: var(--step--1);
  }
  .reasons {
    margin: 0;
    padding-left: 18px;
    display: grid;
    gap: 4px;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .danger {
    color: var(--reject);
  }
</style>
