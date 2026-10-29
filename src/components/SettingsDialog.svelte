<script lang="ts" module>
  export type SettingsPage = "general" | "folders" | "develop" | "cull" | "ai" | "gpu" | "keys" | "nextcloud" | "agents";
</script>

<script lang="ts">
  // Einstellungen: links die Bereiche, rechts der Inhalt. Bündelt, was vorher
  // im KI-Dialog und in der Fußzeile lag, dazu Ordner und Tastenkürzel.
  import { onMount } from "svelte";
  import { api, isTauri, pickFolder, type AppInfo } from "../lib/api";
  import { lang, num, setLang, t } from "../lib/i18n";
  import { keymap } from "../lib/keymap.svelte";
  import { prefs } from "../lib/prefs.svelte";
  import { lib } from "../lib/store.svelte";
  import AiPanel from "./AiPanel.svelte";
  import Icon, { type IconName } from "./Icon.svelte";
  import KeysPanel from "./KeysPanel.svelte";
  import McpSection from "./McpSection.svelte";
  import NextcloudSection from "./NextcloudSection.svelte";
  import StyleSection from "./StyleSection.svelte";
  import CullSection from "./CullSection.svelte";

  let { page = "general", onclose }: { page?: SettingsPage; onclose: () => void } = $props();

  // svelte-ignore state_referenced_locally
  let current = $state<SettingsPage>(page);
  let dialog = $state<HTMLDivElement>();
  let info = $state<AppInfo | null>(null);
  let importTarget = $state<string | null>(null);
  /** Belegter Platz der Großvorschauen (Bytes) */
  let cacheUsage = $state<number | null>(null);
  const gb = (b: number) => `${num(b / 2 ** 30, { maximumFractionDigits: 1 })} GB`;

  const PAGES: { id: SettingsPage; icon: IconName; label: string }[] = [
    { id: "general", icon: "settings", label: t("Allgemein") },
    { id: "folders", icon: "folder", label: t("Ordner") },
    { id: "develop", icon: "develop", label: t("Entwickeln") },
    { id: "cull", icon: "reject", label: t("Aussortieren") },
    { id: "ai", icon: "sparkle", label: t("KI") },
    { id: "gpu", icon: "sliders", label: t("Grafikkarte") },
    { id: "keys", icon: "keyboard", label: t("Tastenkürzel") },
    { id: "nextcloud", icon: "cloud", label: "Nextcloud" },
    { id: "agents", icon: "export", label: t("KI-Agenten (MCP)") },
  ];

  onMount(() => {
    dialog?.focus();
    void lib.refreshAi();
    void lib.refreshPeople();
    api.appInfo().then((i) => (info = i)).catch(() => {});
    api.previewCacheUsage().then((b) => (cacheUsage = b)).catch(() => {});
    api
      .importDefaults()
      .then((d) => (importTarget = d.settings.target ?? null))
      .catch(() => {});
  });

  // Tasten erreichen die Ansichten dahinter nicht (sonst bewerten X, P, 0–5 …
  // die Fotos hinter dem Dialog). Beim Belegen eines Kürzels nimmt das
  // Tastenkürzel-Feld die Taste.
  function onkeydown(e: KeyboardEvent) {
    if (keymap.recording) return;
    if (keymap.is(e, "fullscreen")) return; // Vollbild bleibt erreichbar
    e.stopPropagation();
    if (e.key === "Escape") onclose();
  }

  async function chooseImport() {
    const d = isTauri ? await pickFolder(t("Standardziel für den Import")) : null;
    if (!d) return;
    await api.importSetTarget(d);
    importTarget = d;
  }

  async function resetImport() {
    await api.importSetTarget(null);
    const d = await api.importDefaults().catch(() => null);
    importTarget = d?.settings.target ?? null;
  }

  async function chooseDir(which: "exportDir" | "albumDir") {
    const d = isTauri ? await pickFolder(which === "exportDir" ? t("Standardordner für den Export") : t("Standardordner für Fotoalben")) : null;
    if (!d) return;
    prefs[which] = d;
    prefs.save();
  }

  function clearDir(which: "exportDir" | "albumDir") {
    prefs[which] = null;
    prefs.save();
  }

  // Unterordner oder derselbe Ordner wie das Original („.“)
  let sameFolder = $state(lib.exportSubfolder === ".");
  let subfolder = $state(lib.exportSubfolder === "." ? "Export" : lib.exportSubfolder);
  let subError = $state<string | null>(null);
  async function saveSubfolder() {
    const want = sameFolder ? "." : subfolder;
    if (want.trim() === lib.exportSubfolder) return;
    subError = await lib.setExportSubfolder(want);
    if (subError) return;
    sameFolder = lib.exportSubfolder === ".";
    if (!sameFolder) subfolder = lib.exportSubfolder;
  }
</script>

<svelte:window onkeydowncapture={onkeydown} />

<div class="backdrop" role="presentation" onclick={onclose}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
  <div bind:this={dialog} class="dialog" role="dialog" aria-label={t("Einstellungen")} tabindex="-1" onclick={(e) => e.stopPropagation()}>
    <nav aria-label={t("Bereiche")}>
      <h3>{t("Einstellungen")}</h3>
      {#each PAGES as p (p.id)}
        <button class:on={current === p.id} aria-current={current === p.id ? "page" : undefined} onclick={() => (current = p.id)}>
          <Icon name={p.icon} size={14} />
          {p.label}
        </button>
      {/each}
    </nav>
    <section>
      <header>
        <h4>{PAGES.find((p) => p.id === current)?.label}</h4>
        <button class="icon-btn" aria-label={t("Schließen")} onclick={onclose}><Icon name="close" size={14} /></button>
      </header>
      <div class="content">
        {#if current === "general"}
          <div class="field">
            <span class="label">{t("Sprache")}</span>
            <div class="seg" role="group" aria-label="Sprache / Language">
              <button class="btn" class:on={lang === "de"} aria-pressed={lang === "de"} onclick={() => lang !== "de" && setLang("de")}>Deutsch</button>
              <button class="btn" class:on={lang === "en"} aria-pressed={lang === "en"} onclick={() => lang !== "en" && setLang("en")}>English</button>
            </div>
            <p class="muted small">{t("Ein Wechsel lädt die Oberfläche neu.")}</p>
          </div>
          <div class="field">
            <span class="label">{t("Updates")}</span>
            <div class="row">
              <span>{info ? t("Installiert: Version {v}", { v: info.version }) : ""}</span>
              <button class="btn" onclick={() => lib.updateRequest++}>{t("Jetzt nach Updates suchen")}</button>
            </div>
            <p class="muted small">{t("Lumenkorn sucht außerdem bei jedem Start auf GitHub nach einer neuen Version und fragt, bevor es etwas installiert.")}</p>
          </div>
          <div class="field">
            <span class="label">{t("Karte und Ortsnamen")}</span>
            <label class="check">
              <input type="checkbox" checked={lib.mapEnabled} onchange={(e) => lib.setMapEnabled(e.currentTarget.checked)} />
              {t("Karte und Ortsnamen über OpenStreetMap verwenden")}
            </label>
            <p class="muted small">
              {t("Eingeschaltet lädt Lumenkorn Kartenbilder von OpenStreetMap, sucht dort Orte und ermittelt Ortsnamen zu den Aufnahmeorten (Koordinaten auf etwa 100 m gerundet). Ausgeschaltet geht nichts an OpenStreetMap: Die Karte ist ausgeblendet, GPS-Daten aus den Fotos und schon bekannte Ortsnamen bleiben, die Karte im Fotoalbum funktioniert weiter (sie ist im Programm enthalten).")}
            </p>
          </div>
          <div class="field">
            <span class="label">{t("Vorschau-Cache")}</span>
            <div class="row">
              <label class="row">
                {t("höchstens")}
                <input
                  class="gb"
                  type="number"
                  min="1"
                  max="1000"
                  step="1"
                  value={lib.previewCacheGb}
                  onchange={(e) => lib.setPreviewCacheGb(e.currentTarget.valueAsNumber)}
                />
                GB
              </label>
              {#if cacheUsage !== null}
                <span class="muted small">{t("belegt: {size}", { size: gb(cacheUsage) })}</span>
              {/if}
            </div>
            <p class="muted small">{t("Große Vorschauen für Einzelansicht und Entwickeln. Darüber hinaus löscht Lumenkorn die am längsten nicht angesehenen; sie entstehen bei Bedarf neu.")}</p>
          </div>
        {:else if current === "folders"}
          <div class="field">
            <span class="label">{t("Import: Zielordner")}</span>
            <div class="row">
              <code class="path" class:default={!importTarget}>{importTarget ?? t("zuletzt benutzter Ordner")}</code>
              <button class="btn" onclick={chooseImport}>{t("Ändern …")}</button>
              <button class="btn quiet" onclick={resetImport}>{t("Zurücksetzen")}</button>
            </div>
            <p class="muted small">{t("Vorschlag beim Import von der Speicherkarte; im Import-Dialog lässt er sich jedes Mal ändern.")}</p>
          </div>
          <div class="field">
            <label class="row choice">
              <input type="checkbox" checked={lib.includeSubfolders} onchange={(e) => lib.setIncludeSubfolders(e.currentTarget.checked)} />
              <span class="label">{t("Unterordner in die Bibliothek aufnehmen")}</span>
            </label>
            <p class="muted small">
              {t("Standard für alle Ordner. Für einzelne Ordner per Rechtsklick im Ordnerbaum → „Unterordner einbeziehen“ änderbar. Ausgeschlossene Fotos verschwinden aus der Bibliothek; Bewertungen und Bearbeitungen bleiben erhalten und sind wieder da, sobald du die Unterordner wieder einbeziehst.")}
            </p>
          </div>
          <div class="field">
            <span class="label">{t("Export: Zielordner")}</span>
            <div class="row">
              <code class="path" class:default={!prefs.exportDir}>{prefs.exportDir ?? lib.exportSubfolderLabel}</code>
              <button class="btn" onclick={() => chooseDir("exportDir")}>{t("Ändern …")}</button>
              {#if prefs.exportDir}<button class="btn quiet" onclick={() => clearDir("exportDir")}>{t("Zurücksetzen")}</button>{/if}
            </div>
          </div>
          <div class="field">
            <span class="label">{t("Export ohne festen Zielordner")}</span>
            <label class="row choice">
              <input type="radio" name="export-where" checked={sameFolder} onchange={() => { sameFolder = true; saveSubfolder(); }} />
              {t("Im selben Ordner wie das Original")}
            </label>
            <div class="row choice">
              <input
                type="radio"
                name="export-where"
                checked={!sameFolder}
                aria-label={t("In einen Unterordner")}
                onchange={() => { sameFolder = false; saveSubfolder(); }}
              />
              <label for="export-sub">{t("In einen Unterordner")}</label>
              <input
                id="export-sub"
                class="text"
                class:err={!!subError}
                disabled={sameFolder}
                bind:value={subfolder}
                placeholder="Export"
                spellcheck="false"
                onchange={saveSubfolder}
                onkeydown={(e) => {
                  if (e.key === "Enter") (e.currentTarget as HTMLInputElement).blur();
                }}
              />
            </div>
            {#if subError}<p class="small error">{subError}</p>{/if}
            <p class="muted small">
              {t("Der Unterordner ist relativ zum Ordner des Fotos, z. B. „Export“, „Export/JPEG“ oder „../Exporte“. Neu angelegte Ordner erscheinen nicht in der Bibliothek; Exporte in den Ordner des Originals schon.")}
            </p>
          </div>
          <div class="field">
            <span class="label">{t("Fotoalbum: Speicherort")}</span>
            <div class="row">
              <code class="path" class:default={!prefs.albumDir}>{prefs.albumDir ?? t("wie zuletzt im Speichern-Dialog")}</code>
              <button class="btn" onclick={() => chooseDir("albumDir")}>{t("Ändern …")}</button>
              {#if prefs.albumDir}<button class="btn quiet" onclick={() => clearDir("albumDir")}>{t("Zurücksetzen")}</button>{/if}
            </div>
          </div>
          {#if info}
            <div class="field">
              <span class="label">{t("Datenordner")}</span>
              <code class="path">{info.dataDir}</code>
              <p class="muted small">{t("Katalog, Vorschaubilder, KI-Modelle und Einstellungen. Die Fotos selbst bleiben in ihren Ordnern.")}</p>
            </div>
          {/if}
        {:else if current === "develop"}
          <StyleSection />
        {:else if current === "ai"}
          <AiPanel part="ai" />
        {:else if current === "cull"}
          <CullSection />
        {:else if current === "gpu"}
          <AiPanel part="gpu" />
        {:else if current === "keys"}
          <KeysPanel />
        {:else if current === "nextcloud"}
          <NextcloudSection />
        {:else}
          <McpSection />
        {/if}
      </div>
    </section>
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
    width: min(900px, 100%);
    height: min(680px, calc(100vh - 32px));
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    display: grid;
    grid-template-columns: 200px minmax(0, 1fr);
    overflow: hidden;
  }
  nav {
    border-right: 1px solid var(--line);
    background: var(--bg);
    padding: 14px 8px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow-y: auto;
  }
  nav h3 {
    margin: 0 8px 10px;
    font-size: var(--step-1);
    font-weight: 600;
  }
  nav button {
    display: flex;
    align-items: center;
    gap: 8px;
    text-align: left;
    border: 0;
    border-radius: var(--radius-s);
    background: none;
    color: var(--muted);
    padding: 7px 10px;
    cursor: pointer;
    font: inherit;
  }
  nav button:hover {
    color: var(--text);
    background: var(--raised);
  }
  nav button.on {
    color: var(--text);
    background: var(--raised);
  }
  nav button.on :global(svg) {
    color: var(--accent);
  }
  section {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 14px 18px 10px;
    border-bottom: 1px solid var(--line);
  }
  header h4 {
    margin: 0;
    font-size: var(--step-1);
    font-weight: 600;
  }
  .content {
    flex: 1;
    overflow-y: auto;
    padding: 16px 18px 24px;
    display: grid;
    align-content: start;
    gap: 18px;
  }
  /* Abschnitte der Unterseiten beginnen ohne Trennlinie */
  .content > :global(.sep:first-child) {
    display: none;
  }
  .field {
    display: grid;
    gap: 6px;
  }
  .label {
    font-weight: 600;
  }
  .text {
    flex: 1;
    min-width: 0;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
    padding: 5px 9px;
    font: inherit;
    color: inherit;
  }
  .choice {
    align-items: center;
    gap: 8px;
  }
  .text.err {
    border-color: var(--reject);
  }
  .error {
    margin: 0;
    color: var(--reject);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .seg {
    display: flex;
    gap: 6px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .check input {
    accent-color: var(--accent);
  }
  .seg .on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .gb {
    width: 72px;
  }
  .path {
    font-size: var(--step--1);
    background: var(--raised);
    padding: 3px 8px;
    border-radius: var(--radius-s);
    word-break: break-all;
    flex: 1;
    min-width: 0;
  }
  /* Vorgabe statt Pfad: normale Schrift, gedämpft */
  .path.default {
    font-family: inherit;
    color: var(--muted);
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
  @media (max-width: 640px) {
    .dialog {
      grid-template-columns: 1fr;
      grid-template-rows: auto 1fr;
    }
    nav {
      flex-direction: row;
      flex-wrap: wrap;
      border-right: 0;
      border-bottom: 1px solid var(--line);
    }
    nav h3 {
      width: 100%;
    }
  }
</style>
