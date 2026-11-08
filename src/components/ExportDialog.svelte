<script lang="ts">
  import { onMount } from "svelte";
  import { api, isTauri, pickFolder, type BakeSummary, type ExportConflicts, type ExportFormat, type ExportOptions } from "../lib/api";
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
  let conflicts = $state<ExportConflicts | null>(null);
  const options = (): ExportOptions => ({ format: formats[0], formats, dngEdited, quality, longEdge: resize ? longEdge : null });

  // „Bearbeitung ins Original übernehmen“ statt exportieren: ersetzt die Dateien
  // selbst (volle Auflösung, Format des Originals), das bisherige Original kommt
  // in den Ordner „Original“. Format, Qualität, Größe und Ziel gelten dann nicht.
  // Beim Öffnen immer aus.
  let bake = $state(false);
  let bakeSummary = $state<BakeSummary | null>(null);
  let baking = $state(false);

  async function toggleBake(on: boolean) {
    bake = on;
    conflicts = null;
    if (!on || bakeSummary) return;
    try {
      await dev.flush(); // offene Bearbeitung zählt mit
      bakeSummary = await api.bakeSummary(ids);
    } catch (e) {
      lib.notify(String(e), "error");
      bake = false;
    }
  }

  // Was nicht übernommen wird, und warum
  const skipped = $derived.by(() => {
    const s = bakeSummary;
    if (!s) return [];
    return [
      s.raw && t(s.raw === 1 ? "{n} RAW-Datei (dafür exportieren)" : "{n} RAW-Dateien (dafür exportieren)", { n: s.raw }),
      s.unedited && t(s.unedited === 1 ? "{n} unbearbeitetes Foto" : "{n} unbearbeitete Fotos", { n: s.unedited }),
      s.unsupported && t(s.unsupported === 1 ? "{n} Datei in einem Format, das Lumenkorn nicht schreibt" : "{n} Dateien in einem Format, das Lumenkorn nicht schreibt", { n: s.unsupported }),
      s.missing && t(s.missing === 1 ? "{n} fehlende Datei" : "{n} fehlende Dateien", { n: s.missing }),
    ].filter((x): x is string => !!x);
  });

  async function runBake() {
    if (baking || !bakeSummary?.ready) return;
    baking = true;
    // Das Entwickeln verlassen: Seine Bearbeitung und ihr Verlauf gehören zur
    // alten Datei und dürfen danach nicht wieder gespeichert werden
    if (lib.mode === "develop") {
      await dev.flush();
      lib.mode = "library";
      lib.view = "loupe";
    }
    const started = await lib.bakeOriginals(ids);
    baking = false;
    if (started) onclose();
  }

  async function start() {
    // Enter ersetzt nie Originale: das geht nur über den Knopf
    if (bake || !formats.length) return;
    await dev.flush();
    try {
      const existing = await api.exportConflicts(ids, dir, options());
      if (existing.names.length) conflicts = existing;
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
    <h3>
      {bake
        ? t(ids.length === 1 ? "Bearbeitung von {n} Foto ins Original übernehmen" : "Bearbeitung von {n} Fotos ins Original übernehmen", { n: ids.length })
        : t(ids.length === 1 ? "{n} Foto exportieren" : "{n} Fotos exportieren", { n: ids.length })}
    </h3>

    <fieldset disabled={bake} class:off={bake}>
      <legend>{t("Formate")}</legend>
      {#each FORMATS as f (f.id)}
        <label>
          <input type="checkbox" checked={formats.includes(f.id)} onchange={(e) => toggleFormat(f.id, e.currentTarget.checked)} />
          {f.label}
        </label>
      {/each}
    </fieldset>

    {#if formats.includes("dng") && !bake}
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
      <label class="line" class:off={bake}>
        {t("Qualität")}
        <input type="range" min="40" max="100" bind:value={quality} disabled={bake} />
        <span class="num">{quality}</span>
      </label>
    {/if}

    <label class="line" class:off={bake}>
      <input type="checkbox" bind:checked={resize} disabled={bake} />
      {t("Längste Kante begrenzen auf")}
      <input type="number" min="256" max="20000" step="1" bind:value={longEdge} disabled={!resize || bake} /> px
    </label>

    <div class="line">
      <span class="label">{t("Ziel")}</span>
      {#if bake}
        <span class="dir chosen">{t("die Originaldatei, die bisherige kommt in den Ordner „Original“")}</span>
      {:else}
        <span
          class="dir"
          class:chosen={dir}
          title={dir ?? lib.exportSubfolderLabel}
        >
          {dir ?? lib.exportSubfolderLabel}
        </span>
      {/if}
    </div>
    {#if !bake}
      <div class="line">
        <button type="button" class="btn small" onclick={choose}>{t("Anderer Ordner …")}</button>
        {#if dir}
          <button type="button" class="btn small quiet" onclick={() => (dir = null)}>{t("Zurück zum Standard")}</button>
        {/if}
      </div>
    {/if}

    <label class="line">
      <input type="checkbox" checked={bake} onchange={(e) => toggleBake(e.currentTarget.checked)} disabled={baking} />
      <span class="stack">
        {t("Bearbeitung ins Original übernehmen")}
        <small>{t("ersetzt die Datei, das Original bleibt gesichert")}</small>
      </span>
    </label>

    {#if bake}
      <p class="hint">
        {t("Die Bearbeitung wird in die Datei geschrieben, in voller Auflösung und im selben Format, mit allen Metadaten einschließlich Ort.")}
        {t("Das bisherige Original kommt in den Ordner „Original“ neben dem Foto, die Bearbeitung in einen Schnappschuss. „Original wiederherstellen“ im Kontextmenü macht beides rückgängig.")}
      </p>
      {#if !bakeSummary}
        <p class="hint">{t("Fotos werden geprüft …")}</p>
      {:else if !bakeSummary.ready}
        <p class="warn">{t("Keines dieser Fotos lässt sich übernehmen. Möglich sind bearbeitete JPEG-, TIFF- und PNG-Dateien.")}</p>
      {/if}
      {#if skipped.length}
        <p class="hint">{t("Nicht übernommen: {list}", { list: skipped.join(", ") })}</p>
      {/if}
    {:else}
      <p class="hint">
        {t("Exportiert in voller Auflösung mit allen Bearbeitungen. Gibt es eine Datei schon, fragt Lumenkorn nach.")}
        {#if !dir}{t("Von Lumenkorn angelegte Export-Ordner erscheinen nicht in der Bibliothek.")}{/if}
      </p>
    {/if}

    {#if bake}
      <div class="actions">
        <button type="button" class="btn quiet" onclick={onclose} disabled={baking}>{t("Abbrechen")}</button>
        <button type="button" class="btn danger" onclick={runBake} disabled={baking || !bakeSummary?.ready}>
          {bakeSummary?.ready === 1 ? t("Original ersetzen") : t("{n} Originale ersetzen", { n: bakeSummary?.ready ?? 0 })}
        </button>
      </div>
    {:else if conflicts}
      <div class="conflict" role="alert">
        <p>
          {t(conflicts.names.length === 1 ? "{n} Datei gibt es im Ziel schon:" : "{n} Dateien gibt es im Ziel schon:", { n: conflicts.names.length })}
          <span class="names">{conflicts.names.slice(0, 3).join(", ")}{conflicts.names.length > 3 ? " …" : ""}</span>
        </p>
        {#if conflicts.library}
          <p class="warn">
            {t(conflicts.library === 1 ? "Darunter ist {n} Foto aus der Bibliothek. Überschreiben ersetzt es endgültig." : "Darunter sind {n} Fotos aus der Bibliothek. Überschreiben ersetzt sie endgültig.", { n: conflicts.library })}
          </p>
        {/if}
        {#if conflicts.sources}
          <p class="hint">{t("Fotos, die selbst exportiert werden, bleiben unverändert, ihr Export kommt als Name-2 daneben. Um die Originale zu ersetzen, „Bearbeitung ins Original übernehmen“ anhaken.")}</p>
        {/if}
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
  .warn {
    margin: 0;
    color: var(--reject);
    font-size: var(--step--1);
  }
  /* Gilt beim Übernehmen ins Original nicht */
  .off {
    opacity: 0.45;
  }
  .stack {
    display: grid;
  }
  .stack small {
    color: var(--faint);
    font-size: var(--step--1);
  }
  .btn.danger {
    background: var(--reject);
    border-color: var(--reject);
    color: #fff;
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
