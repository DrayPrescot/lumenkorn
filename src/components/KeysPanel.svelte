<script lang="ts">
  // Tastenkürzel ansehen und ändern. Klick auf eine Belegung nimmt die nächste
  // Tastenkombination auf (Escape bricht ab); belegt ein anderer Befehl sie
  // schon, wird das angezeigt und nichts geändert.
  import { ACTIONS, GROUP_LABELS, keymap, type KeyGroup } from "../lib/keymap.svelte";
  import { t } from "../lib/i18n";
  import Icon from "./Icon.svelte";

  /** Wird gerade aufgenommen: Befehl und Index der Belegung (−1: neue) */
  let recording = $state<{ id: string; index: number } | null>(null);
  let message = $state<{ id: string; text: string } | null>(null);
  let filter = $state("");

  const groups = $derived(
    (Object.keys(GROUP_LABELS) as KeyGroup[])
      .map((g) => ({
        group: g,
        actions: ACTIONS.filter((a) => a.group === g && (!filter.trim() || a.label.toLowerCase().includes(filter.trim().toLowerCase()))),
      }))
      .filter((g) => g.actions.length),
  );

  function start(id: string, index: number) {
    recording = { id, index };
    keymap.recording = true;
    message = null;
  }

  function stop() {
    recording = null;
    keymap.recording = false;
  }

  function onkeydowncapture(e: KeyboardEvent) {
    if (!recording) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    if (e.key === "Escape") {
      stop();
      return;
    }
    const combo = keymap.fromEvent(e);
    if (!combo) return; // nur Strg/Umschalt gedrückt – auf die eigentliche Taste warten
    const { id, index } = recording;
    const taken = keymap.conflicts(combo, id);
    if (taken.length) {
      message = { id, text: t("{key} ist schon belegt: {action}", { key: keymap.label(combo), action: taken.map((a) => a.label).join(", ") }) };
      stop();
      return;
    }
    const keys = [...keymap.keys(id)];
    if (index < 0) keys.push(combo);
    else keys[index] = combo;
    keymap.set(id, [...new Set(keys)]);
    stop();
  }

  function remove(id: string, index: number) {
    const keys = keymap.keys(id).filter((_, i) => i !== index);
    keymap.set(id, keys);
  }
</script>

<svelte:window {onkeydowncapture} />

<div class="keys">
  <p class="muted">
    {t("Klicke auf eine Taste, um sie neu zu belegen, oder auf +, um eine weitere hinzuzufügen. Pfeiltasten, Enter, Leertaste und Escape bleiben für die Bedienung fest.")}
  </p>
  <div class="top">
    <input type="search" placeholder={t("Befehl suchen")} bind:value={filter} />
    <button class="btn quiet" disabled={!Object.keys(keymap.custom).length} onclick={() => keymap.resetAll()}>{t("Alle zurücksetzen")}</button>
  </div>
  {#each groups as g (g.group)}
    <h5>{GROUP_LABELS[g.group]}</h5>
    <ul>
      {#each g.actions as a (a.id)}
        <li>
          <span class="name">{a.label}</span>
          <span class="bind">
            {#each keymap.keys(a.id) as k, i (k)}
              <span class="key-wrap">
                <button class="key" class:rec={recording?.id === a.id && recording.index === i} onclick={() => start(a.id, i)} title={t("Neu belegen")}>
                  {recording?.id === a.id && recording.index === i ? t("Taste drücken …") : keymap.label(k)}
                </button>
                {#if keymap.keys(a.id).length > 1}
                  <button class="x" aria-label={t("Belegung entfernen")} title={t("Belegung entfernen")} onclick={() => remove(a.id, i)}><Icon name="close" size={10} /></button>
                {/if}
              </span>
            {/each}
            {#if recording?.id === a.id && recording.index < 0}
              <span class="key rec">{t("Taste drücken …")}</span>
            {:else}
              <button class="add" aria-label={t("Weitere Taste")} title={t("Weitere Taste")} onclick={() => start(a.id, -1)}>+</button>
            {/if}
            {#if keymap.isCustom(a.id)}
              <button class="icon-btn" title={t("Zurücksetzen")} aria-label={t("Zurücksetzen")} onclick={() => keymap.reset(a.id)}><Icon name="refresh" size={12} /></button>
            {/if}
          </span>
          {#if message?.id === a.id}
            <span class="msg">{message.text}</span>
          {/if}
        </li>
      {/each}
    </ul>
  {/each}
</div>

<style>
  .keys {
    display: grid;
    gap: 10px;
  }
  p {
    margin: 0;
  }
  .muted {
    color: var(--muted);
  }
  .top {
    display: flex;
    gap: 8px;
  }
  .top input {
    flex: 1;
    min-width: 0;
  }
  h5 {
    margin: 8px 0 0;
    font-size: var(--step--1);
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
  }
  li {
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: center;
    gap: 2px 12px;
    padding: 5px 0;
    border-bottom: 1px solid color-mix(in srgb, var(--line) 60%, transparent);
  }
  .bind {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  .key-wrap {
    display: inline-flex;
    align-items: center;
  }
  .key {
    font: inherit;
    font-size: var(--step--1);
    min-width: 34px;
    padding: 2px 8px;
    border: 1px solid var(--line);
    border-bottom-width: 2px;
    border-radius: 5px;
    background: var(--raised);
    color: var(--text);
    cursor: pointer;
  }
  .key:hover {
    border-color: var(--accent);
  }
  .key.rec {
    border-color: var(--accent);
    color: var(--accent);
  }
  .x,
  .add {
    border: 0;
    background: none;
    color: var(--muted);
    cursor: pointer;
    padding: 2px 4px;
  }
  .add {
    font-size: var(--step-0);
  }
  .x:hover,
  .add:hover {
    color: var(--text);
  }
  .msg {
    grid-column: 1 / -1;
    color: var(--reject);
    font-size: var(--step--1);
  }
</style>
