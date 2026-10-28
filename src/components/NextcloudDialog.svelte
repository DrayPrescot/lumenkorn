<script lang="ts">
  // Projekt → „Zu Nextcloud hochladen …“: die Exporte oder die Originale des
  // Projekts in einen Nextcloud-Ordner bringen (abgleichen) und den Ordner als
  // Link teilen.
  import { onDestroy, onMount } from "svelte";
  import { api, listen, type NcProgress, type NcProjectInfo, type NcSource, type Project } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { num, t } from "../lib/i18n";

  let { project, onclose }: { project: Project; onclose: () => void } = $props();

  let info = $state<NcProjectInfo | null>(null);
  let folder = $state("");
  let source = $state<NcSource>("exports");
  let removeMissing = $state(false);
  let share = $state(true);
  let usePassword = $state(false);
  let password = $state("");
  let useExpire = $state(false);
  let expire = $state("");
  let progress = $state<NcProgress | null>(null);
  let running = $state(false);
  let error = $state<string | null>(null);

  // Ablauf frühestens morgen; Vorschlag in 30 Tagen
  const day = (d: number) => new Date(Date.now() + d * 86_400_000).toISOString().slice(0, 10);
  const minDate = day(1);

  let unlisten: (() => void) | null = null;
  onMount(async () => {
    unlisten = await listen("nextcloud-progress", (p) => {
      progress = p;
      if (p.finished) {
        running = false;
        void lib.refreshNcShared();
        if (p.error) error = p.error;
        else if (info) info = { ...info, shareUrl: p.shareUrl };
      }
    });
    try {
      info = await api.ncProjectInfo(project.id);
      folder = info.folder;
      // Zuletzt gewählt; ohne Exporte gleich die Originale
      source = info.source === "exports" && !info.exports.files && info.originals.files ? "originals" : info.source;
      share = true;
    } catch (e) {
      error = String(e);
    }
  });
  onDestroy(() => unlisten?.());

  const fmt = (v: number, d: number) => num(v, { maximumFractionDigits: d });
  const mb = (b: number) => (b >= 1e9 ? `${fmt(b / 1e9, 1)} GB` : `${fmt(Math.max(0.1, b / 1e6), 1)} MB`);

  async function start() {
    error = null;
    progress = null;
    running = true;
    try {
      await api.ncUploadProject(project.id, {
        folder,
        source,
        removeMissing,
        share: share
          ? { password: usePassword && password ? password : null, expire: useExpire && expire ? expire : null }
          : null,
      });
    } catch (e) {
      running = false;
      error = String(e);
    }
  }

  async function copy(url: string) {
    try {
      await navigator.clipboard.writeText(url);
      lib.notify(t("Link kopiert"));
    } catch {
      lib.notify(url);
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "F11") return;
    e.stopPropagation();
    if (e.key === "Escape" && !running) onclose();
  }

  const done = $derived(!!progress?.finished && !progress.error);
  const chosen = $derived(info ? info[source] : null);
  const canStart = $derived(!!chosen && chosen.files > 0 && !!folder.trim() && !running && !(share && usePassword && !password) && !(share && useExpire && !expire));
</script>

<svelte:window onkeydowncapture={onkeydown} />

<div class="backdrop" role="presentation" onclick={() => !running && onclose()}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
  <div class="dialog" role="dialog" aria-label={t("Zu Nextcloud hochladen")} tabindex="-1" onclick={(e) => e.stopPropagation()}>
    <h3>{t("„{name}“ zu Nextcloud hochladen", { name: project.name })}</h3>

    {#if !info && !error}
      <p class="muted">{t("Dateien werden gesucht …")}</p>
    {:else if info && !info.exports.files && !info.originals.files}
      <p>{t("Dieses Projekt enthält keine Fotos zum Hochladen.")}</p>
    {:else if info}
      <fieldset class="source" disabled={running}>
        <legend class="label">{t("Hochladen")}</legend>
        <label class="check">
          <input type="radio" name="nc-source" value="exports" bind:group={source} disabled={!info.exports.files} />
          <span>
            {t("Exporte")}
            <span class="muted small">
              {info.exports.files
                ? t("{n} Dateien, {size}", { n: num(info.exports.files), size: mb(info.exports.bytes) })
                : t("noch keine – zuerst exportieren (Strg+Umschalt+E)")}
            </span>
          </span>
        </label>
        <label class="check">
          <input type="radio" name="nc-source" value="originals" bind:group={source} disabled={!info.originals.files} />
          <span>
            {t("Alle Fotos des Projekts (Originale)")}
            <span class="muted small">{t("{n} Dateien, {size}", { n: num(info.originals.files), size: mb(info.originals.bytes) })}</span>
          </span>
        </label>
        {#if source === "originals"}
          <p class="small muted">{t("Die Originaldateien so, wie sie auf der Platte liegen (auch RAWs, ohne Bearbeitungen). Abgelehnte Fotos bleiben weg.")}</p>
        {/if}
      </fieldset>

      <label class="field">
        <span class="label">{t("Ordner in Nextcloud")}</span>
        <input class="text" bind:value={folder} spellcheck="false" disabled={running} />
      </label>
      <label class="check">
        <input type="checkbox" bind:checked={removeMissing} disabled={running} />
        {t("Dateien im Ordner entfernen, die es im Projekt nicht mehr gibt")}
      </label>
      {#if removeMissing && source !== info.source && info.shareUrl}
        <p class="small warn">
          {source === "originals"
            ? t("Die zuletzt hochgeladenen Exporte werden dabei aus dem Ordner entfernt.")
            : t("Die zuletzt hochgeladenen Originale werden dabei aus dem Ordner entfernt.")}
        </p>
      {/if}

      <fieldset disabled={running}>
        <label class="check">
          <input type="checkbox" bind:checked={share} />
          {t("Als Link teilen")}
        </label>
        {#if share}
          <div class="sub">
            <label class="check">
              <input type="checkbox" bind:checked={usePassword} />
              {t("Mit Passwort")}
            </label>
            {#if usePassword}
              <input class="text" type="password" autocomplete="new-password" bind:value={password} placeholder={t("Passwort für den Link")} />
            {/if}
            <label class="check">
              <input
                type="checkbox"
                bind:checked={useExpire}
                onchange={() => {
                  if (useExpire && !expire) expire = day(30);
                }}
              />
              {t("Läuft ab am")}
              {#if useExpire}<input class="date" type="date" min={minDate} bind:value={expire} />{/if}
            </label>
          </div>
        {:else if info.shareUrl}
          <p class="small muted">{t("Der bisherige Link wird beim Hochladen entfernt.")}</p>
        {/if}
      </fieldset>
      <p class="small muted">{t("Neue und geänderte Dateien werden hochgeladen, unveränderte übersprungen. Ein vorhandener Link bleibt derselbe.")}</p>
    {/if}

    {#if progress && !progress.finished}
      <div class="progress">
        <progress max={progress.total || 1} value={progress.done}></progress>
        <span class="small">{t("{done} von {total}", { done: String(progress.done), total: String(progress.total) })}{progress.current ? ` · ${progress.current}` : ""}</span>
      </div>
    {/if}

    {#if done && progress}
      <p class="ok">
        {t("Fertig: {up} hochgeladen, {same} unverändert", { up: String(progress.uploaded), same: String(progress.unchanged) })}{progress.removed ? t(", {n} entfernt", { n: String(progress.removed) }) : ""}.
      </p>
      {#if progress.failed.length}
        <details class="failed">
          <summary>{t("{n} Dateien fehlgeschlagen", { n: String(progress.failed.length) })}</summary>
          <ul>{#each progress.failed as f (f)}<li>{f}</li>{/each}</ul>
        </details>
      {/if}
    {/if}

    {#if info?.shareUrl}
      <div class="link">
        <code>{info.shareUrl}</code>
        <button class="btn small" onclick={() => copy(info!.shareUrl!)}>{t("Kopieren")}</button>
        <button class="btn small quiet" onclick={() => api.openUrl(info!.shareUrl!)}>{t("Öffnen")}</button>
      </div>
    {/if}

    {#if error}<p class="error">{error}</p>{/if}

    <div class="buttons">
      <button class="btn quiet" disabled={running} onclick={onclose}>{done ? t("Schließen") : t("Abbrechen")}</button>
      {#if !done}
        <button class="btn primary" disabled={!canStart} onclick={start}>{running ? t("Wird hochgeladen …") : t("Hochladen")}</button>
      {/if}
    </div>
  </div>
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
    width: min(600px, 100%);
    max-height: calc(100vh - 32px);
    overflow-y: auto;
    /* Eine Spalte in Dialogbreite: lange Link-Adressen werden gekürzt statt
       den Dialog zu verbreitern */
    grid-template-columns: minmax(0, 1fr);
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    padding: 18px;
    display: grid;
    gap: 12px;
  }
  h3 {
    margin: 0;
    font-size: var(--step-1);
    font-weight: 600;
  }
  p {
    margin: 0;
  }
  .field {
    display: grid;
    gap: 4px;
  }
  .label {
    font-weight: 600;
  }
  .text,
  .date {
    min-width: 0;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
    padding: 5px 9px;
    font: inherit;
    color: inherit;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .check input[type="checkbox"] {
    accent-color: var(--accent);
  }
  fieldset {
    border: 0;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 8px;
  }
  legend {
    padding: 0;
    margin-bottom: 4px;
  }
  .source .check {
    align-items: baseline;
  }
  .source .check input {
    accent-color: var(--accent);
  }
  .muted {
    color: var(--muted);
  }
  .warn {
    color: var(--reject);
  }
  .sub {
    display: grid;
    gap: 8px;
    padding-left: 24px;
  }
  .small {
    font-size: var(--step--1);
  }
  .progress {
    display: grid;
    gap: 4px;
  }
  progress {
    width: 100%;
    height: 6px;
    accent-color: var(--accent);
  }
  .ok {
    color: var(--text);
  }
  .link {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px;
    background: var(--raised);
    border-radius: var(--radius-s);
  }
  .link button {
    flex-shrink: 0;
  }
  .link code {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .failed ul {
    margin: 4px 0 0;
    padding-left: 18px;
    font-size: var(--step--1);
  }
  .error {
    color: var(--reject);
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
