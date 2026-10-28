<script lang="ts">
  import { onMount } from "svelte";
  import { api, isTauri, pickFolder, type ExportFormat, type ExportOptions } from "../lib/api";
  import { prefs } from "../lib/prefs.svelte";
  import { dev } from "../lib/develop.svelte";
  import { lib } from "../lib/store.svelte";
  import PathDialog from "./PathDialog.svelte";
  import { t } from "../lib/i18n";

  let { ids, onclose }: { ids: number[]; onclose: () => void } = $props();

  // Mehrere Formate auf einmal, z. B. DNG zum Archivieren und JPEG zum Teilen
  const FORMATS: { id: ExportFormat; label: string }[] = [
    { id: "jpeg", label: "JPEG" },
    { id: "tiff16", label: t("TIFF, 16 Bit") },
    { id: "png", label: "PNG" },
    { id: "dng", label: "DNG" },
  ];
  let formats = $state<ExportFormat[]>(["jpeg"]);
  // DNG: fertig bearbeitet oder die Daten des Originals zum Weiterentwickeln
  let dngEdited = $state(true);
  function toggleFormat(f: ExportFormat, on: boolean) {
    formats = on ? FORMATS.map((x) => x.id).filter((x) => x === f || formats.includes(x)) : formats.filter((x) => x !== f);
  }
  let quality = $state(90);
  let resize = $state(false);
  let longEdge = $state(2560);
  // null: Unterordner aus den Einstellungen neben dem jeweiligen Original; Vorgabe aus den
  // Einstellungen
  let dir = $state<string | null>(prefs.exportDir);
  let pathDialog = $state(false);
  let form = $state<HTMLFormElement>();

  onMount(() => form?.querySelector<HTMLElement>("input, button")?.focus());

  // Solange der Dialog offen ist, erreichen Tasten die Ansichten dahinter nicht
  // (sonst dreht [ das Bild, wechselt → das Foto, macht Strg+Z rückgängig …)
  function onkeydown(e: KeyboardEvent) {
    if (e.key === "F11") return; // Vollbild bleibt auch bei offenem Dialog erreichbar
    if (pathDialog) return;
    e.stopPropagation();
    if (e.key === "Escape") onclose();
  }

  async function choose() {
    const d = isTauri ? await pickFolder(t("Zielordner für den Export")) : null;
    if (d) dir = d;
    else if (!isTauri || navigator.userAgent.includes("Android")) pathDialog = true;
  }

  // Gibt es Zieldateien schon, wird einmal für alle gefragt
  let conflicts = $state<string[] | null>(null);
  const options = (): ExportOptions => ({ format: formats[0], formats, dngEdited, quality, longEdge: resize ? longEdge : null });

  async function start() {
    if (!formats.length) return;
    await dev.flush();
    try {
      const existing = await api.exportConflicts(ids, dir, options());
      if (existing.length) conflicts = existing;
      else await run("rename");
    } catch (e) {
      lib.notify(String(e), "error");
    }
  }

  async function run(onConflict: NonNullable<ExportOptions["onConflict"]>) {
    try {
      await api.exportImages(ids, dir, { ...options(), onConflict });
      lib.notify(t(ids.length === 1 ? "Export von {n} Foto gestartet" : "Export von {n} Fotos gestartet", { n: ids.length }));
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
    <h3>{t(ids.length === 1 ? "{n} Foto exportieren" : "{n} Fotos exportieren", { n: ids.length })}</h3>

    <fieldset>
      <legend>{t("Formate")}</legend>
      {#each FORMATS as f (f.id)}
        <label>
          <input type="checkbox" checked={formats.includes(f.id)} onchange={(e) => toggleFormat(f.id, e.currentTarget.checked)} />
          {f.label}
        </label>
      {/each}
    </fieldset>

    {#if formats.includes("dng")}
      <fieldset class="dng">
        <legend>DNG</legend>
        <label><input type="radio" name="dng" bind:group={dngEdited} value={true} /> {t("Mit Bearbeitung")}</label>
        <label><input type="radio" name="dng" bind:group={dngEdited} value={false} /> {t("Original ohne Bearbeitung")}</label>
      </fieldset>
      <p class="hint">
        {dngEdited
          ? t("Leicht verlustbehaftet wie in Lightroom: das fertige Bild, in anderen RAW-Programmen weiter bearbeitbar.")
          : t("Leicht verlustbehaftet wie in Lightroom: die Bilddaten vor jeder Bearbeitung, mit den Reserven in den Lichtern, zum Weiterentwickeln in anderen Programmen.")}
      </p>
    {/if}

    {#if formats.includes("jpeg")}
      <label class="line">
        {t("Qualität")}
        <input type="range" min="40" max="100" bind:value={quality} />
        <span class="num">{quality}</span>
      </label>
    {/if}

    <label class="line">
      <input type="checkbox" bind:checked={resize} />
      {t("Längste Kante begrenzen auf")}
      <input type="number" min="256" max="20000" step="1" bind:value={longEdge} disabled={!resize} /> px
    </label>

    <div class="line">
      <span class="label">{t("Ziel")}</span>
      <span
        class="dir"
        class:chosen={dir}
        title={dir ?? lib.exportSubfolderLabel}
      >
        {dir ?? lib.exportSubfolderLabel}
      </span>
    </div>
    <div class="line">
      <button type="button" class="btn small" onclick={choose}>{t("Anderer Ordner …")}</button>
      {#if dir}
        <button type="button" class="btn small quiet" onclick={() => (dir = null)}>{t("Zurück zum Standard")}</button>
      {/if}
    </div>

    <p class="hint">
      {t("Exportiert in voller Auflösung mit allen Bearbeitungen. Gibt es eine Datei schon, fragt Lumenkorn nach.")}
      {#if !dir}{t("Von Lumenkorn angelegte Export-Ordner erscheinen nicht in der Bibliothek.")}{/if}
    </p>

    {#if conflicts}
      <div class="conflict" role="alert">
        <p>
          {t(conflicts.length === 1 ? "{n} Datei gibt es im Ziel schon:" : "{n} Dateien gibt es im Ziel schon:", { n: conflicts.length })}
          <span class="names">{conflicts.slice(0, 3).join(", ")}{conflicts.length > 3 ? " …" : ""}</span>
        </p>
        <p class="hint">{t("Originale aus der Bibliothek werden nie überschrieben.")}</p>
        <div class="choices">
          <button type="button" class="btn small" onclick={() => run("overwrite")}>{t("Überschreiben")}</button>
          <button type="button" class="btn small" onclick={() => run("rename")} title={t("Als Name-2 usw. neben die vorhandene Datei")}>{t("Zusätzlich exportieren")}</button>
          <button type="button" class="btn small" onclick={() => run("skip")}>{t("Überspringen")}</button>
        </div>
        <button type="button" class="btn small quiet back" onclick={() => (conflicts = null)}>{t("Zurück")}</button>
      </div>
    {:else}
      <div class="actions">
        <button type="button" class="btn quiet" onclick={onclose}>{t("Abbrechen")}</button>
        <button type="submit" class="btn primary" disabled={!formats.length}>{t("Exportieren")}</button>
      </div>
    {/if}
  </form>
</div>

{#if pathDialog}
  <PathDialog
    title={t("Zielordner")}
    hint={t("Vollständiger Pfad eines vorhandenen Ordners.")}
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
  .label {
    color: var(--muted);
  }
  .dir {
    flex: 1;
    min-width: 0;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dir.chosen {
    color: inherit;
  }
  .hint {
    margin: 0;
    color: var(--faint);
    font-size: var(--step--1);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    flex-wrap: wrap;
    gap: 8px;
  }
  .conflict {
    display: grid;
    gap: 8px;
    padding: 12px;
    background: var(--raised);
    border: 1px solid var(--accent);
    border-radius: var(--radius-m);
  }
  .conflict p {
    margin: 0;
  }
  .choices {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 8px;
  }
  .back {
    justify-self: start;
  }
  .names {
    color: var(--muted);
    overflow-wrap: anywhere;
  }
  .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
</style>
