<script lang="ts">
  // Presets anlegen (aus einem Foto) und verwalten (umbenennen, löschen).
  // Geöffnet über das Rechtsklick-Menü → „Preset anwenden“.
  import { NO_GROUPS, type Preset, type TransferGroups } from "../lib/api";
  import { dev } from "../lib/develop.svelte";
  import { t } from "../lib/i18n";
  import Icon from "./Icon.svelte";

  type GroupKey = Exclude<keyof TransferGroups, "basic">;
  const GROUPS: { key: GroupKey; label: string; hint: string }[] = [
    { key: "whiteBalance", label: t("Weißabgleich"), hint: t("Temperatur, Tönung") },
    { key: "light", label: t("Licht"), hint: t("Belichtung, Kontrast, Lichter, Tiefen, Weiß, Schwarz, Gradationskurve") },
    { key: "color", label: t("Farbe"), hint: t("Dynamik, Sättigung") },
    { key: "hsl", label: "HSL", hint: t("Farbton, Sättigung, Luminanz je Farbbereich") },
    { key: "colorGrade", label: "Color Grading", hint: t("Farbräder, Überblenden, Abgleich") },
    { key: "details", label: t("Details"), hint: t("Klarheit, Schärfen, Dunst entfernen") },
    { key: "masks", label: t("Masken"), hint: t("Verlauf, Radial, Pinsel, Farbe – keine KI-Masken") },
    { key: "geometry", label: t("Zuschnitt und Drehen"), hint: t("Zuschnitt, Geraderichten, Drehen, Spiegeln") },
  ];

  const d = $derived(dev.presetDialog);
  let name = $state("");
  let groups = $state<TransferGroups>({ ...NO_GROUPS });
  let renaming = $state<number | null>(null);
  let nameInput = $state<HTMLInputElement>();

  // Vorauswahl: was am Foto verändert ist
  $effect(() => {
    if (d?.mode !== "create") return;
    groups = dev.changedGroups(d.params);
    name = "";
    queueMicrotask(() => nameInput?.focus());
  });

  const anyGroup = $derived(GROUPS.some((g) => groups[g.key]));

  function close() {
    dev.presetDialog = null;
  }

  async function save(e: SubmitEvent) {
    e.preventDefault();
    if (d?.mode !== "create" || !name.trim() || !anyGroup) return;
    if (await dev.createPreset(name, groups, d.params)) close();
  }

  function summary(p: Preset): string {
    return GROUPS.filter((g) => p.groups[g.key]).map((g) => g.label).join(", ");
  }

  function finishRename(p: Preset, value: string) {
    renaming = null;
    dev.renamePreset(p, value);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "F11") return;
    e.stopPropagation();
    if (e.key === "Escape" && renaming === null) close();
  }
</script>

<svelte:window onkeydowncapture={onkeydown} />

{#if d}
  <div class="backdrop" role="presentation" onclick={close}>
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
    <div class="dialog" role="dialog" aria-modal="true" tabindex="-1" onclick={(e) => e.stopPropagation()}>
      {#if d.mode === "create"}
        <form onsubmit={save}>
          <h3>{t("Neues Preset")}</h3>
          <p class="meta">{t("Aus der Bearbeitung von {name}", { name: d.name })}</p>
          <input bind:this={nameInput} type="text" placeholder={t("Name, z. B. Warmer Abend")} bind:value={name} />
          <p class="label">{t("Enthält")}</p>
          <div class="groups">
            {#each GROUPS as g (g.key)}
              <label>
                <input type="checkbox" bind:checked={groups[g.key]} />
                <span>{g.label}<small>{g.hint}</small></span>
              </label>
            {/each}
          </div>
          <p class="meta small">{t("KI-Rauschreduzierung, KI-Reparaturen und KI-Masken gehören zum einzelnen Foto und kommen nie mit.")}</p>
          <div class="actions">
            <button class="btn quiet" type="button" onclick={close}>{t("Abbrechen")}</button>
            <button class="btn primary" type="submit" disabled={!name.trim() || !anyGroup}>{t("Speichern")}</button>
          </div>
        </form>
      {:else}
        <h3>{t("Presets verwalten")}</h3>
        <ul>
          {#each dev.presets as p (p.id)}
            <li>
              {#if renaming === p.id}
                <!-- svelte-ignore a11y_autofocus -->
                <input
                  class="rename"
                  value={p.name}
                  autofocus
                  onkeydown={(e) => {
                    if (e.key === "Enter") finishRename(p, e.currentTarget.value);
                    if (e.key === "Escape") renaming = null;
                  }}
                  onblur={(e) => finishRename(p, e.currentTarget.value)}
                />
              {:else}
                <button class="name" onclick={() => (renaming = p.id)} title={t("Klick: umbenennen")}>
                  <span>{p.name}</span>
                  <small>{summary(p)}</small>
                </button>
                <button class="icon-btn" aria-label={t("Preset löschen")} title={t("Preset löschen")} onclick={() => dev.deletePreset(p)}>
                  <Icon name="close" size={12} />
                </button>
              {/if}
            </li>
          {:else}
            <li class="empty">{t("Noch keine Presets. Rechtsklick auf ein bearbeitetes Foto → Preset anwenden → Neues Preset aus diesem Foto.")}</li>
          {/each}
        </ul>
        <div class="actions">
          <button class="btn primary" type="button" onclick={close}>{t("Fertig")}</button>
        </div>
      {/if}
    </div>
  </div>
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
    width: min(460px, 100%);
    max-height: calc(100vh - 32px);
    overflow: auto;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    padding: 18px;
    display: grid;
    gap: 12px;
  }
  form {
    display: grid;
    gap: 12px;
  }
  h3 {
    margin: 0;
    font-size: var(--step-1);
    font-weight: 600;
  }
  .meta {
    margin: -6px 0 0;
    color: var(--muted);
  }
  .meta.small {
    margin: 0;
    font-size: var(--step--1);
    color: var(--faint);
  }
  .label {
    margin: 0;
    color: var(--muted);
    font-size: var(--step--1);
  }
  .groups {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px 14px;
  }
  .groups label {
    display: flex;
    align-items: flex-start;
    gap: 8px;
  }
  .groups input {
    margin-top: 3px;
  }
  .groups span {
    display: grid;
  }
  small {
    color: var(--faint);
    font-size: 10.5px;
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
    gap: 4px;
    border-radius: var(--radius-s);
  }
  li:hover {
    background: var(--raised);
  }
  .name {
    flex: 1;
    min-width: 0;
    display: grid;
    text-align: left;
    padding: 5px 8px;
  }
  .name span,
  .name small {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .rename {
    flex: 1;
    margin: 2px 0;
  }
  .empty {
    color: var(--faint);
    padding: 4px 0;
  }
  .empty:hover {
    background: none;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
