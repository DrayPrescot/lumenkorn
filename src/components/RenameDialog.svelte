<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { api, type RenameItem, type RenameOptions } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";

  let { ids, onclose }: { ids: number[]; onclose: () => void } = $props();

  // Liste beim Öffnen festhalten – spätere Markierungen ändern den Dialog nicht
  const photoIds = untrack(() => ids.slice());

  const TOKENS: [string, string][] = [
    ["name", t("Dateiname")],
    ["yyyy", t("Jahr")],
    ["mm", t("Monat")],
    ["dd", t("Tag")],
    ["hh", t("Stunde")],
    ["min", t("Minute")],
    ["ss", t("Sekunde")],
    ["n", t("Nummer")],
    ["camera", t("Kamera")],
    ["lens", t("Objektiv")],
    ["folder", t("Ordnername")],
  ];
  const DEFAULT_PATTERN = "{yyyy}{mm}{dd}_{hh}{min}{ss}_{n}";
  const STORE = "lumenkorn.rename";

  function saved(): Partial<RenameOptions> {
    try {
      return JSON.parse(localStorage.getItem(STORE) ?? "{}");
    } catch {
      return {};
    }
  }
  const prev = saved();
  let pattern = $state(typeof prev.pattern === "string" && prev.pattern ? prev.pattern : DEFAULT_PATTERN);
  let start = $state(Number.isFinite(prev.start) ? Math.max(0, Math.floor(prev.start!)) : 1);
  let digits = $state(Number.isFinite(prev.digits) ? Math.min(9, Math.max(1, Math.floor(prev.digits!))) : 3);
  let byCapture = $state(prev.byCapture ?? true);

  let items = $state<RenameItem[]>([]);
  let loaded = $state(false);
  let busy = $state(false);
  let patternEl = $state<HTMLInputElement>();

  const options = $derived<RenameOptions>({
    pattern,
    start: Number.isFinite(start) ? Math.max(0, Math.floor(start)) : 0,
    digits: Number.isFinite(digits) ? Math.min(9, Math.max(1, Math.floor(digits))) : 1,
    byCapture,
  });

  // Vorschau nach kurzer Pause neu rechnen; nur die neueste Antwort zählt
  let seq = 0;
  let refresh = $state(0);
  $effect(() => {
    const o = options;
    void refresh;
    const my = ++seq;
    const timer = setTimeout(async () => {
      try {
        const r = await api.renamePreview(photoIds, o);
        if (my === seq) {
          items = r;
          loaded = true;
        }
      } catch (e) {
        if (my === seq) {
          lib.notify(String(e), "error");
          onclose();
        }
      }
    }, 150);
    return () => clearTimeout(timer);
  });

  onMount(() => patternEl?.focus());

  const conflicts = $derived(items.filter((i) => i.conflict).length);
  const changed = $derived(items.filter((i) => i.new !== i.old).length);
  const canRun = $derived(loaded && !busy && !conflicts && changed > 0);

  function insert(tok: string) {
    const text = `{${tok}}`;
    const el = patternEl;
    const a = el?.selectionStart ?? pattern.length;
    const b = el?.selectionEnd ?? pattern.length;
    pattern = pattern.slice(0, a) + text + pattern.slice(b);
    const pos = a + text.length;
    requestAnimationFrame(() => {
      el?.focus();
      el?.setSelectionRange(pos, pos);
    });
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "F11") return; // Vollbild bleibt auch bei offenem Dialog erreichbar
    e.stopPropagation();
    if (e.key === "Escape" && !busy) onclose();
  }

  async function run() {
    if (!canRun) return;
    busy = true;
    try {
      localStorage.setItem(STORE, JSON.stringify(options));
    } catch {
      /* ohne Speicher: Einstellungen nicht merken */
    }
    const ok = await lib.renameImages(photoIds, options);
    busy = false;
    if (ok) onclose();
    else refresh++; // Vorschau neu holen: Dateien können sich geändert haben
  }
</script>

<svelte:window onkeydowncapture={onkeydown} />

<div class="backdrop" role="presentation" onclick={() => !busy && onclose()}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
  <form
    class="dialog"
    onclick={(e) => e.stopPropagation()}
    onsubmit={(e) => {
      e.preventDefault();
      run();
    }}
  >
    <h3>{t(photoIds.length === 1 ? "{n} Foto umbenennen" : "{n} Fotos umbenennen", { n: photoIds.length })}</h3>

    <div class="field">
      <label for="rename-pattern">{t("Neuer Name")}</label>
      <input id="rename-pattern" class="pattern" bind:this={patternEl} bind:value={pattern} spellcheck="false" autocomplete="off" />
      <div class="tokens">
        {#each TOKENS as [tok, label] (tok)}
          <button type="button" class="chip" onclick={() => insert(tok)} title={`{${tok}}`}>{label}</button>
        {/each}
      </div>
    </div>

    <div class="line">
      <label class="inline">
        {t("Startnummer")}
        <input type="number" min="0" max="999999999" bind:value={start} />
      </label>
      <label class="inline">
        {t("Stellen")}
        <input type="number" min="1" max="9" bind:value={digits} />
      </label>
    </div>

    <fieldset>
      <legend>{t("Nummerieren nach")}</legend>
      <label><input type="radio" bind:group={byCapture} value={true} /> {t("Aufnahmezeit")}</label>
      <label><input type="radio" bind:group={byCapture} value={false} /> {t("Reihenfolge in der Ansicht")}</label>
    </fieldset>

    <div class="preview" aria-live="polite">
      {#if !loaded}
        <div class="row muted">{t("Vorschau wird berechnet …")}</div>
      {:else}
        {#each items as it (it.id)}
          <div class="row" class:bad={!!it.conflict}>
            <span class="old" title={it.old}>{it.old}</span>
            <span class="arrow" aria-hidden="true">→</span>
            <span class="new" title={it.new}>{it.new}</span>
            {#if it.conflict}<span class="why">{it.conflict}</span>{/if}
          </div>
        {/each}
      {/if}
    </div>

    <p class="hint">
      {t("Die Dateiendung bleibt. XMP-Dateien werden mitgenommen, RAW+JPEG-Paare bekommen denselben Namen.")}
    </p>
    {#if conflicts}
      <p class="warn">{t(conflicts === 1 ? "{n} Name ist nicht möglich." : "{n} Namen sind nicht möglich.", { n: conflicts })}</p>
    {/if}

    <div class="actions">
      <button type="button" class="btn quiet" onclick={onclose} disabled={busy}>{t("Abbrechen")}</button>
      <button type="submit" class="btn primary" disabled={!canRun}>{busy ? t("Benenne um …") : t("Umbenennen")}</button>
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
    width: min(640px, 100%);
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
  .field {
    display: grid;
    gap: 6px;
  }
  .field > label,
  legend {
    color: var(--muted);
  }
  legend {
    margin-bottom: 6px;
    padding: 0;
  }
  input {
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
    padding: 5px 8px;
    color: inherit;
  }
  .pattern {
    font-family: var(--mono, ui-monospace, monospace);
  }
  .tokens {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .chip {
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 2px 10px;
    color: inherit;
    font-size: var(--step--1);
    cursor: pointer;
  }
  .chip:hover {
    border-color: var(--accent);
  }
  .line {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
  }
  label.inline {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  input[type="number"] {
    width: 84px;
  }
  fieldset {
    border: 0;
    padding: 0;
    margin: 0;
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
  }
  fieldset label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  input[type="radio"] {
    accent-color: var(--accent);
  }
  .preview {
    max-height: 240px;
    overflow: auto;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
    padding: 4px 0;
    font-size: var(--step--1);
  }
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr);
    gap: 8px;
    padding: 2px 10px;
    align-items: baseline;
  }
  .row.muted {
    color: var(--muted);
    display: block;
  }
  .old {
    color: var(--muted);
  }
  .old,
  .new {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row.bad {
    background: color-mix(in srgb, var(--reject) 14%, transparent);
  }
  .row.bad .new {
    color: var(--reject);
  }
  .why {
    grid-column: 1 / -1;
    color: var(--reject);
  }
  .hint {
    margin: 0;
    color: var(--faint);
    font-size: var(--step--1);
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
  .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
</style>
