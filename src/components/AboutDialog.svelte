<script lang="ts">
  // „Über Lumenkorn“: Version, Lizenz, Hinweis zur Entstehung mit KI und die
  // verwendeten Open-Source-Projekte und KI-Modelle (Liste in lib/credits.ts).
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { LIBRARIES, LICENSE_URL, MODELS, REPO_URL, type Credit } from "../lib/credits";
  import { t } from "../lib/i18n";
  import { lib } from "../lib/store.svelte";
  import Icon from "./Icon.svelte";

  let { onclose }: { onclose: () => void } = $props();

  let version = $state("");
  let dialog = $state<HTMLDivElement>();
  onMount(() => {
    api.appInfo().then((i) => (version = i.version)).catch(() => {});
    dialog?.focus();
  });

  // Tasten erreichen die Ansichten dahinter nicht (Bewerten, Markieren …)
  function onkeydown(e: KeyboardEvent) {
    if (e.key === "F11") return;
    e.stopPropagation();
    if (e.key === "Escape") onclose();
  }

  function open(url: string) {
    api.openUrl(url).catch((e) => lib.notify(String(e), "error"));
  }
</script>

<svelte:window onkeydowncapture={onkeydown} />

{#snippet list(items: Credit[])}
  <ul class="credits">
    {#each items as c (c.name)}
      <li>
        <button class="link" onclick={() => open(c.url)} title={c.url}>{c.name}</button>
        <span class="use">{c.use}</span>
        <span class="license">{c.license}</span>
      </li>
    {/each}
  </ul>
{/snippet}

<div class="backdrop" role="presentation" onclick={onclose}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
  <div
    bind:this={dialog}
    class="dialog"
    role="dialog"
    aria-labelledby="about-title"
    tabindex="-1"
    onclick={(e) => e.stopPropagation()}
  >
    <header>
      <div class="title">
        <span class="mark" aria-hidden="true"></span>
        <div>
          <h3 id="about-title">Lumenkorn</h3>
          {#if version}<p class="version">{t("Version {v}", { v: version })}</p>{/if}
        </div>
      </div>
      <button class="icon-btn" aria-label={t("Schließen")} onclick={onclose}><Icon name="close" size={14} /></button>
    </header>

    <p>{t("Fotoverwaltung und RAW-Entwicklung, die vollständig auf deinem Rechner läuft.")}</p>

    <div class="note">
      <p>
        {t("Lumenkorn entsteht mit Hilfe von KI: Ein großer Teil des Codes wurde mit einem KI-Assistenten geschrieben und danach an echten Fotos geprüft. Fehler sind trotzdem möglich – bitte sichere deine Fotos und deinen Katalog.")}
      </p>
    </div>

    <p class="small">
      {t("Lumenkorn ist freie Software unter der GNU General Public License, Version 3 oder neuer (GPL-3.0-or-later). Du darfst es nutzen, verändern und weitergeben; es wird ohne jede Gewährleistung bereitgestellt.")}
    </p>
    <div class="actions">
      <button class="btn small" onclick={() => open(REPO_URL)}>{t("Quelltext auf GitHub")}</button>
      <button class="btn small" onclick={() => open(LICENSE_URL)}>{t("Lizenztext")}</button>
    </div>

    <section>
      <h4>{t("Open-Source-Projekte in Lumenkorn")}</h4>
      {@render list(LIBRARIES)}
    </section>

    <section>
      <h4>{t("KI-Modelle")}</h4>
      <p class="small muted">{t("Nicht in der App enthalten – sie werden bei Bedarf einmal heruntergeladen und laufen danach ohne Internet.")}</p>
      {@render list(MODELS)}
    </section>

    <p class="small muted">{t("Danke an alle, die an diesen Projekten arbeiten.")}</p>
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
    width: min(640px, 100%);
    max-height: calc(100vh - 32px);
    overflow-y: auto;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    padding: 18px;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 14px;
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
  }
  .title {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .mark {
    width: 32px;
    height: 32px;
    border-radius: 50%;
    flex: none;
    background: radial-gradient(circle at 50% 50%, #ffd68c 0 28%, transparent 30%), conic-gradient(var(--accent), #b9792a, var(--accent));
  }
  h3 {
    margin: 0;
    font-size: var(--step-1);
    font-weight: 600;
  }
  h4 {
    margin: 0 0 8px;
    font-size: var(--step-0);
    font-weight: 600;
  }
  p {
    margin: 0;
  }
  .version,
  .muted {
    color: var(--muted);
  }
  .small {
    font-size: var(--step--1);
  }
  .note {
    padding: 10px 12px;
    background: var(--raised);
    border-left: 3px solid var(--accent);
    border-radius: var(--radius-s);
  }
  .actions {
    display: flex;
    gap: 8px;
    margin-top: -4px;
  }
  .credits {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    font-size: var(--step--1);
  }
  .credits li {
    display: grid;
    grid-template-columns: minmax(0, 11rem) minmax(0, 1fr) auto;
    gap: 10px;
    padding: 5px 0;
    border-top: 1px solid var(--line);
    align-items: baseline;
  }
  .link {
    text-align: left;
    color: var(--text);
    text-decoration: underline;
    text-decoration-color: var(--line);
    text-underline-offset: 2px;
    overflow-wrap: anywhere;
  }
  .link:hover {
    text-decoration-color: var(--accent);
  }
  .use {
    color: var(--muted);
  }
  .license {
    color: var(--faint);
    white-space: nowrap;
  }
  @media (max-width: 560px) {
    .credits li {
      grid-template-columns: minmax(0, 1fr);
      gap: 2px;
    }
  }
</style>
