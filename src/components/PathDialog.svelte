<script lang="ts">
  import { t } from "../lib/i18n";

  let {
    title,
    hint,
    onsubmit,
    oncancel,
  }: { title: string; hint: string; onsubmit: (path: string) => void; oncancel: () => void } = $props();

  let value = $state("");
</script>

<div class="backdrop" role="presentation" onclick={oncancel}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <form
    class="dialog"
    onclick={(e) => e.stopPropagation()}
    onkeydown={(e) => e.key === "Escape" && oncancel()}
    onsubmit={(e) => {
      e.preventDefault();
      if (value.trim()) onsubmit(value.trim());
    }}
  >
    <h3>{title}</h3>
    <!-- svelte-ignore a11y_autofocus -->
    <input type="text" bind:value placeholder={t("/pfad/zum/ordner")} autofocus spellcheck="false" />
    <p>{hint}</p>
    <div class="actions">
      <button type="button" class="btn quiet" onclick={oncancel}>{t("Abbrechen")}</button>
      <button type="submit" class="btn primary" disabled={!value.trim()}>{t("Hinzufügen")}</button>
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
    width: min(460px, 100%);
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    padding: 18px;
    display: grid;
    gap: 10px;
  }
  h3 {
    margin: 0;
    font-size: var(--step-1);
    font-weight: 600;
  }
  input {
    width: 100%;
    padding: 8px 10px;
  }
  p {
    margin: 0;
    color: var(--muted);
    font-size: var(--step--1);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 6px;
  }
</style>
