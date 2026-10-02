<script lang="ts">
  import { onMount } from "svelte";
  import { api, isTauri, pickFolder, type ExportOptions } from "../lib/api";
  import { dev } from "../lib/develop.svelte";
  import { lib } from "../lib/store.svelte";
  import PathDialog from "./PathDialog.svelte";

  let { ids, onclose }: { ids: number[]; onclose: () => void } = $props();

  let format = $state<ExportOptions["format"]>("jpeg");
  let quality = $state(90);
  let resize = $state(false);
  let longEdge = $state(2560);
  let dir = $state<string | null>(null);
  let pathDialog = $state(false);
  let form = $state<HTMLFormElement>();

  onMount(() => form?.querySelector<HTMLElement>("input, button")?.focus());

  // Solange der Dialog offen ist, erreichen Tasten die Ansichten dahinter nicht
  // (sonst dreht [ das Bild, wechselt → das Foto, macht Strg+Z rückgängig …)
  function onkeydown(e: KeyboardEvent) {
    if (pathDialog) return;
    e.stopPropagation();
    if (e.key === "Escape") onclose();
  }

  async function choose() {
    const d = isTauri ? await pickFolder("Zielordner für den Export") : null;
    if (d) dir = d;
    else if (!isTauri || navigator.userAgent.includes("Android")) pathDialog = true;
  }

  async function start() {
    if (!dir) return;
    await dev.flush();
    try {
      await api.exportImages(ids, dir, { format, quality, longEdge: resize ? longEdge : null });
      lib.notify(`Export von ${ids.length} ${ids.length === 1 ? "Foto" : "Fotos"} gestartet`);
      onclose();
    } catch (e) {
      lib.notify(String(e), "error");
    }
  }
</script>

<svelte:window onkeydowncapture={onkeydown} />

<div class="backdrop" role="presentation" onclick={onclose}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
  <form
    bind:this={form}
    class="dialog"
    onclick={(e) => e.stopPropagation()}
    onsubmit={(e) => { e.preventDefault(); start(); }}
  >
    <h3>{ids.length} {ids.length === 1 ? "Foto" : "Fotos"} exportieren</h3>

    <fieldset>
      <legend>Format</legend>
      <label><input type="radio" bind:group={format} value="jpeg" /> JPEG</label>
      <label><input type="radio" bind:group={format} value="tiff16" /> TIFF, 16 Bit</label>
      <label><input type="radio" bind:group={format} value="png" /> PNG</label>
    </fieldset>

    {#if format === "jpeg"}
      <label class="line">
        Qualität
        <input type="range" min="40" max="100" bind:value={quality} />
        <span class="num">{quality}</span>
      </label>
    {/if}

    <label class="line">
      <input type="checkbox" bind:checked={resize} />
      Längste Kante begrenzen auf
      <input type="number" min="256" max="20000" step="1" bind:value={longEdge} disabled={!resize} /> px
    </label>

    <div class="line">
      <button type="button" class="btn" onclick={choose}>Zielordner wählen …</button>
      <span class="dir" title={dir ?? ""}>{dir ?? "Noch kein Ordner gewählt"}</span>
    </div>

    <p class="hint">Exportiert in voller Auflösung mit allen Bearbeitungen. Vorhandene Dateien werden nicht überschrieben.</p>

    <div class="actions">
      <button type="button" class="btn quiet" onclick={onclose}>Abbrechen</button>
      <button type="submit" class="btn primary" disabled={!dir}>Exportieren</button>
    </div>
  </form>
</div>

{#if pathDialog}
  <PathDialog
    title="Zielordner"
    hint="Vollständiger Pfad eines vorhandenen Ordners."
    onsubmit={(p) => { pathDialog = false; dir = p; }}
    oncancel={() => (pathDialog = false)}
  />
{/if}

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
    width: min(480px, 100%);
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
  fieldset {
    border: 0;
    padding: 0;
    margin: 0;
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
  }
  legend {
    color: var(--muted);
    margin-bottom: 6px;
    padding: 0;
  }
  label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .line {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .line input[type="range"] {
    flex: 1;
    accent-color: var(--accent);
  }
  input[type="radio"],
  input[type="checkbox"] {
    accent-color: var(--accent);
  }
  input[type="number"] {
    width: 88px;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
    padding: 4px 6px;
  }
  .num {
    min-width: 3ch;
    text-align: right;
  }
  .dir {
    flex: 1;
    min-width: 0;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hint {
    margin: 0;
    color: var(--faint);
    font-size: var(--step--1);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
</style>
