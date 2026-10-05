<script lang="ts">
  import { isTauri, pickFolder, type Project } from "../lib/api";
  import { shortPath } from "../lib/format";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";
  import Icon from "./Icon.svelte";
  import PathDialog from "./PathDialog.svelte";

  let { project, onclose }: { project: Project; onclose: () => void } = $props();
  let pathDialog = $state(false);
  let confirmDelete = $state(false);

  async function add() {
    const path = isTauri ? await pickFolder(t("Ordner zu „{name}“ hinzufügen", { name: project.name })) : null;
    if (path) await lib.addFolderToProject(project.id, path, true);
    else if (!isTauri || navigator.userAgent.includes("Android")) pathDialog = true;
  }
</script>

<svelte:window onclick={onclose} />

<div class="pop" role="dialog" aria-label={t("Ordner im Projekt")} tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.key === "Escape" && onclose()}>
  <p class="lead">{t("Alle Fotos dieser Ordner gehören zu „{name}“, auch wenn sie später dazukommen.", { name: project.name })}</p>
  <ul>
    {#each project.folders as f (f.path)}
      <li>
        <Icon name="folder" size={14} />
        <span class="path" title={f.path}>{shortPath(f.path)}</span>
        <label title={t("Unterordner einbeziehen")}>
          <input type="checkbox" checked={f.recursive} onchange={(e) => lib.addFolderToProject(project.id, f.path, e.currentTarget.checked)} />
          {t("Unterordner")}
        </label>
        <button class="icon-btn" aria-label={t("Ordner aus Projekt entfernen")} title={t("Aus Projekt entfernen")} onclick={() => lib.removeFolderFromProject(project.id, f.path)}>
          <Icon name="close" size={13} />
        </button>
      </li>
    {:else}
      <li class="empty">{t("Noch keine Ordner. Füge welche hinzu oder klicke im Ordnerbaum mit rechts auf einen Ordner.")}</li>
    {/each}
  </ul>
  <div class="actions">
    {#if confirmDelete}
      <span class="muted">{t("Projekt löschen? Fotos bleiben erhalten.")}</span>
      <button class="btn quiet" onclick={() => (confirmDelete = false)}>{t("Abbrechen")}</button>
      <button class="btn danger" onclick={() => { onclose(); lib.deleteProject(project.id); }}>{t("Löschen")}</button>
    {:else}
      <button class="btn quiet" onclick={() => (confirmDelete = true)}><Icon name="trash" size={14} /> {t("Projekt löschen")}</button>
      <span class="grow"></span>
      <button class="btn primary" onclick={add}><Icon name="plus" size={14} /> {t("Ordner hinzufügen")}</button>
    {/if}
  </div>
</div>

{#if pathDialog}
  <PathDialog
    title={t("Ordner zum Projekt hinzufügen")}
    hint={t("Vollständiger Pfad. Der Ordner muss in einem Ordner der Bibliothek liegen, damit Fotos erscheinen.")}
    onsubmit={(p) => { pathDialog = false; lib.addFolderToProject(project.id, p, true); }}
    oncancel={() => (pathDialog = false)}
  />
{/if}

<style>
  .pop {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    z-index: 40;
    width: min(460px, 92vw);
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    box-shadow: 0 12px 32px rgb(0 0 0 / 0.45);
    padding: 12px;
  }
  .lead {
    margin: 0 0 10px;
    color: var(--muted);
    font-size: var(--step--1);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 2px;
  }
  li {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 4px 4px 6px;
    border-radius: var(--radius-s);
    background: var(--panel);
  }
  li :global(svg) {
    flex: none;
    color: var(--muted);
  }
  .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  label {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--muted);
    font-size: var(--step--1);
  }
  input[type="checkbox"] {
    accent-color: var(--accent);
  }
  .empty {
    color: var(--faint);
    background: none;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 12px;
    flex-wrap: wrap;
  }
  .grow {
    flex: 1;
  }
  .btn.danger {
    background: var(--reject);
    border-color: var(--reject);
    color: #fff;
  }
</style>
