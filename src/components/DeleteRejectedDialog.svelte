<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import { api, imageUrl, type ImageSummary, type RejectedSummary } from "../lib/api";
  import { dev } from "../lib/develop.svelte";
  import { lib } from "../lib/store.svelte";
  import { num, t } from "../lib/i18n";

  let { images, onclose }: { images: ImageSummary[]; onclose: () => void } = $props();

  // Liste beim Öffnen festhalten – spätere Markierungen ändern den Dialog nicht
  const items = untrack(() => images.slice());
  const ids = items.map((i) => i.id);
  let summary = $state<RejectedSummary | null>(null);
  let toTrash = $state(true);
  let busy = $state(false);
  let confirmBtn = $state<HTMLButtonElement>();

  onMount(async () => {
    try {
      summary = await api.rejectedSummary(ids);
      if (!summary.trashSupported) toTrash = false;
    } catch (e) {
      lib.notify(String(e), "error");
      onclose();
      return;
    }
    await tick(); // Knopf ist erst nach dem Laden aktiv und fokussierbar
    confirmBtn?.focus();
  });

  const fmt = (v: number, d: number) => num(v, { maximumFractionDigits: d });
  const mb = (b: number) =>
    b >= 1e9 ? `${fmt(b / 1e9, 1)} GB` : b >= 1e6 ? `${fmt(b / 1e6, 0)} MB` : `${fmt(Math.max(1, b / 1e3), 0)} KB`;

  // Solange der Dialog offen ist, erreichen Tasten die Ansichten dahinter nicht
  function onkeydown(e: KeyboardEvent) {
    if (e.key === "F11") return; // Vollbild bleibt auch bei offenem Dialog erreichbar
    e.stopPropagation();
    if (e.key === "Escape" && !busy) onclose();
    else if (e.key === "Enter" && !(e.target instanceof HTMLElement && e.target.closest("form"))) {
      e.preventDefault();
      run();
    }
  }

  async function run() {
    if (busy || !summary?.count) return;
    busy = true;
    // Offene Bearbeitung zuerst speichern und schließen
    const wasDevelop = lib.mode === "develop";
    if (wasDevelop) {
      await dev.flush();
      lib.mode = "library";
      lib.view = "loupe";
    }
    await lib.deleteRejected(ids, toTrash);
    if (wasDevelop && lib.focused) lib.mode = "develop"; // mit dem nächsten Foto weiter entwickeln
    busy = false;
    onclose();
  }
</script>

<svelte:window onkeydowncapture={onkeydown} />

<div class="backdrop" role="presentation" onclick={() => !busy && onclose()}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
  <form
    class="dialog"
    onclick={(e) => e.stopPropagation()}
    onsubmit={(e) => { e.preventDefault(); run(); }}
  >
    <h3>
      {#if summary}
        {t(summary.count === 1 ? "{n} abgelehntes Foto löschen?" : "{n} abgelehnte Fotos löschen?", { n: summary.count })}
      {:else}
        {t("Abgelehnte Fotos werden gezählt …")}
      {/if}
    </h3>

    <div class="thumbs">
      {#each items.slice(0, 24) as im (im.id)}
        <img src={imageUrl("thumb", im.id, lib.thumbVersion[im.id] ?? 0)} alt={im.fileName} title={im.fileName} loading="lazy" />
      {/each}
      {#if items.length > 24}<span class="more">+{items.length - 24}</span>{/if}
    </div>

    {#if summary}
      <p class="meta">
        {t(summary.count === 1 ? "{n} Datei, zusammen {size}, aus „{scope}“." : "{n} Dateien, zusammen {size}, aus „{scope}“.", {
          n: summary.count,
          size: mb(summary.bytes),
          scope: lib.scopeTitle,
        })}
        {t("Zugehörige XMP-Dateien und Bearbeitungen werden mit entfernt.")}
      </p>
    {/if}

    {#if summary?.trashSupported}
      <fieldset>
        <label><input type="radio" bind:group={toTrash} value={true} /> {t("In den Papierkorb verschieben")}</label>
        <label><input type="radio" bind:group={toTrash} value={false} /> {t("Endgültig löschen")}</label>
      </fieldset>
    {/if}
    {#if !toTrash}
      <p class="warn">{t("Endgültig gelöschte Dateien lassen sich nicht wiederherstellen.")}</p>
    {/if}

    <div class="actions">
      <button type="button" class="btn quiet" onclick={onclose} disabled={busy}>{t("Abbrechen")}</button>
      <button bind:this={confirmBtn} type="submit" class="btn danger" disabled={busy || !summary?.count}>
        {busy ? t("Lösche …") : toTrash ? t("In den Papierkorb") : t("Endgültig löschen")}
      </button>
    </div>
  </form>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: grid;
    place-items: center;
    background: rgb(0 0 0 / 0.5);
    padding: 16px;
  }
  .dialog {
    width: min(560px, 100%);
    max-height: calc(100vh - 32px);
    overflow: auto;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    padding: 18px;
    display: grid;
    gap: 14px;
  }
  h3 {
    margin: 0;
    font-size: var(--step-1);
    font-weight: 600;
  }
  .thumbs {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    align-items: center;
  }
  .thumbs img {
    height: 56px;
    width: auto;
    max-width: 96px;
    object-fit: cover;
    border-radius: var(--radius-s);
    background: var(--raised);
    opacity: 0.85;
  }
  .more {
    color: var(--muted);
    padding: 0 6px;
  }
  .meta {
    margin: 0;
    color: var(--muted);
  }
  fieldset {
    border: 0;
    padding: 0;
    margin: 0;
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
  }
  label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  input[type="radio"] {
    accent-color: var(--accent);
  }
  .warn {
    margin: 0;
    color: var(--reject);
    font-size: var(--step--1);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .btn.danger {
    background: var(--reject);
    border-color: var(--reject);
    color: #fff;
  }
  .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
</style>
