<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { api, imageUrl, type MergeKind, type MergeOptions } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";

  let { kind, ids, onclose }: { kind: MergeKind; ids: number[]; onclose: () => void } = $props();

  const TITLES: Record<MergeKind, string> = {
    hdr: t("HDR aus Belichtungsreihe"),
    pano: t("Panorama"),
    focus: t("Focus Stacking"),
  };
  const HINTS: Record<MergeKind, string> = {
    hdr: t("Die Fotos werden aufeinander ausgerichtet und zu einem Bild mit dem vollen Helligkeitsumfang aller Belichtungen verrechnet. Belichtungsunterschiede werden aus den Bildern selbst gemessen."),
    pano: t("Überlappende Aufnahmen (ca. 20–50 %) aus einem Standpunkt. Die Reihenfolge spielt keine Rolle, auch mehrere Reihen übereinander sind möglich."),
    focus: t("Aufnahmen mit verschobener Schärfeebene, in Reihenfolge von nah nach fern (oder umgekehrt). Die jeweils scharfen Bereiche werden zusammengesetzt."),
  };
  // Phasen kommen als deutscher Text vom Backend (merge.rs, merge/*.rs)
  const STAGES: Record<string, string> = {
    Vorbereiten: t("Vorbereiten"),
    "Bilder laden": t("Bilder laden"),
    Ausrichten: t("Ausrichten"),
    "Belichtung messen": t("Belichtung messen"),
    Zusammenführen: t("Zusammenführen"),
    "Überlappungen suchen": t("Überlappungen suchen"),
    "Helligkeit angleichen": t("Helligkeit angleichen"),
    Zusammenfügen: t("Zusammenfügen"),
    "Schärfe zusammenführen": t("Schärfe zusammenführen"),
    Speichern: t("Speichern"),
    Fertig: t("Fertig"),
  };

  // Gewählte Optionen für diese Sitzung merken
  const opts = $state<MergeOptions>(untrack(() => structuredClone(remembered)));
  $effect(() => {
    remembered = $state.snapshot(opts) as MergeOptions;
  });

  const items = untrack(() => {
    const byId = new Map(lib.images.map((i) => [i.id, i]));
    return ids.map((id) => byId.get(id)).filter((i) => i !== undefined);
  });

  let started = $state(false);
  let startBtn = $state<HTMLButtonElement>();
  const progress = $derived(started ? lib.mergeProgress : null);
  const running = $derived(started && !(progress?.finished ?? false));

  onMount(() => startBtn?.focus());

  // Fertig (oder Fehler): Dialog schließen, die Meldung kommt vom Store
  $effect(() => {
    if (started && progress?.finished) onclose();
  });

  async function start() {
    if (started) return;
    started = true;
    lib.mergeProgress = { stage: "Vorbereiten", done: 0, total: 1, finished: false, error: null, imageId: null, fileName: null };
    try {
      await api.mergeStart(kind, ids, $state.snapshot(opts) as MergeOptions);
    } catch (e) {
      lib.mergeProgress = null;
      started = false;
      lib.notify(String(e), "error");
    }
  }

  function onkeydown(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "Escape" && (!started || running)) onclose();
    // Enter startet – außer auf einem Knopf (z. B. „Abbrechen“), der seine eigene Aktion auslöst
    else if (e.key === "Enter" && !started && !(e.target instanceof HTMLButtonElement)) {
      e.preventDefault();
      start();
    }
  }

  const pct = $derived(progress && progress.total ? Math.round((100 * progress.done) / progress.total) : 0);
</script>

<script module lang="ts">
  let remembered: MergeOptions = {
    hdr: { align: true, deghost: true },
    pano: { projection: "cylindrical", autoCrop: true, maxMegapixels: 150 },
    focus: { align: true },
  };
</script>

<svelte:window onkeydowncapture={onkeydown} />

<div class="backdrop" role="presentation" onclick={() => !started && onclose()}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
  <form class="dialog" onclick={(e) => e.stopPropagation()} onsubmit={(e) => { e.preventDefault(); start(); }}>
    <h3>{t("{title} aus {n} Fotos", { title: TITLES[kind], n: ids.length })}</h3>

    <div class="thumbs">
      {#each items.slice(0, 24) as im (im.id)}
        <img src={imageUrl("thumb", im.id, lib.thumbVersion[im.id] ?? 0)} alt={im.fileName} title={im.fileName} loading="lazy" />
      {/each}
      {#if items.length > 24}<span class="more">+{items.length - 24}</span>{/if}
    </div>

    <p class="meta">{HINTS[kind]}</p>

    <fieldset disabled={started}>
      {#if kind === "hdr"}
        <label><input type="checkbox" bind:checked={opts.hdr.align} /> {t("Ausrichten (Freihand-Reihe)")}</label>
        <label><input type="checkbox" bind:checked={opts.hdr.deghost} /> {t("Geisterbilder entfernen (bewegte Motive)")}</label>
      {:else if kind === "pano"}
        <div class="row">
          <span>{t("Projektion")}</span>
          <label><input type="radio" bind:group={opts.pano.projection} value="cylindrical" /> {t("Zylindrisch")}</label>
          <label><input type="radio" bind:group={opts.pano.projection} value="spherical" /> {t("Kugel")}</label>
          <label><input type="radio" bind:group={opts.pano.projection} value="perspective" /> {t("Perspektivisch")}</label>
        </div>
        <label><input type="checkbox" bind:checked={opts.pano.autoCrop} /> {t("Automatisch zuschneiden (keine leeren Ränder)")}</label>
      {:else}
        <label><input type="checkbox" bind:checked={opts.focus.align} /> {t("Ausrichten (Verwackeln, Objektiv-Atmen)")}</label>
      {/if}
    </fieldset>

    {#if progress && !progress.finished}
      <div class="progress" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
        <div class="bar" style:width="{pct}%"></div>
      </div>
      <p class="meta">{STAGES[progress.stage] ?? progress.stage}{progress.total > 1 ? t(" – {done} von {total}", { done: progress.done, total: progress.total }) : " …"}</p>
    {:else}
      <p class="meta small">{t("Das Ergebnis wird als EXR-Datei (linear, 16 Bit, mit Lichterreserve) neben dem ersten Foto gespeichert und danach ausgewählt.")}</p>
    {/if}

    <div class="actions">
      {#if running}
        <button type="button" class="btn quiet" onclick={() => api.mergeCancel()}>{t("Abbrechen")}</button>
        <button type="button" class="btn" onclick={onclose}>{t("Im Hintergrund")}</button>
      {:else}
        <button type="button" class="btn quiet" onclick={onclose}>{t("Abbrechen")}</button>
        <button bind:this={startBtn} type="submit" class="btn primary">{t("Zusammenfügen")}</button>
      {/if}
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
  }
  .more {
    color: var(--muted);
    padding: 0 6px;
  }
  .meta {
    margin: 0;
    color: var(--muted);
  }
  .small {
    font-size: var(--step--1);
  }
  fieldset {
    border: 0;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 8px;
  }
  fieldset:disabled {
    opacity: 0.6;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 16px;
    align-items: center;
  }
  .row > span {
    color: var(--muted);
  }
  label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  input {
    accent-color: var(--accent);
  }
  .progress {
    height: 6px;
    border-radius: 3px;
    background: var(--raised);
    overflow: hidden;
  }
  .bar {
    height: 100%;
    background: var(--accent);
    transition: width 0.2s;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
