<script lang="ts">
  // Name einer Person bzw. unbenannten Gruppe als Überschrift, per Klick
  // änderbar. Ein vorhandener Name ordnet die Gesichter dieser Person zu.
  import type { Person } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";

  let {
    person,
    big = false,
    onrenamed,
  }: {
    person: Person;
    /** Größere Schrift (Prüfansicht) */
    big?: boolean;
    /** Nach Erfolg mit der ID der Person, die jetzt die Gesichter hat */
    onrenamed?: (id: number) => void;
  } = $props();

  let editing = $state(false);
  const others = $derived(lib.people.filter((p) => p.name && p.id !== person.id));
  const listId = $derived(`person-name-${person.id}`);

  async function submit(value: string) {
    editing = false;
    const target = await lib.renamePerson(person.id, value);
    if (target !== null) onrenamed?.(target);
  }
</script>

{#if editing}
  <form
    class:big
    onsubmit={(e) => {
      e.preventDefault();
      submit((e.currentTarget.elements.namedItem("n") as HTMLInputElement).value);
    }}
  >
    <!-- svelte-ignore a11y_autofocus -->
    <input
      type="text"
      name="n"
      value={person.name ?? ""}
      placeholder={t("Wer ist das?")}
      list={listId}
      autofocus
      onfocus={(e) => e.currentTarget.select()}
      onblur={() => (editing = false)}
      onkeydown={(e) => {
        e.stopPropagation();
        if (e.key === "Escape") editing = false;
      }}
    />
    <datalist id={listId}>
      {#each others as p (p.id)}<option value={p.name}></option>{/each}
    </datalist>
  </form>
{:else}
  <button
    class="name"
    class:big
    class:unnamed={!person.name}
    title={person.name ? t("Klicken zum Umbenennen") : t("Klicken, um einen Namen zu geben oder einer Person zuzuordnen")}
    onclick={() => (editing = true)}
  >
    {person.name ?? t("Unbenannte Gruppe – Name?")}
  </button>
{/if}

<style>
  .name {
    font: inherit;
    font-weight: 600;
    font-size: var(--step-1);
    padding: 1px 4px;
    margin: 0 -4px;
    border-radius: 4px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .name.big {
    font-size: var(--step-2);
  }
  .name:hover {
    background: var(--raised);
  }
  .name.unnamed {
    color: var(--muted);
  }
  form {
    min-width: 0;
  }
  input {
    font: inherit;
    font-weight: 600;
    font-size: var(--step-0);
    width: 16em;
    max-width: 100%;
  }
  form.big input {
    font-size: var(--step-1);
  }
</style>
