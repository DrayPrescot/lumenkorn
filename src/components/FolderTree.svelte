<script lang="ts">
  import type { FolderNode } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";
  import Icon from "./Icon.svelte";
  import FolderTree from "./FolderTree.svelte";

  let {
    node,
    depth = 0,
    oncontext,
  }: { node: FolderNode; depth?: number; oncontext: (node: FolderNode, e: MouseEvent) => void } = $props();

  // Nur der Startzustand hängt von der Tiefe ab.
  // svelte-ignore state_referenced_locally
  let open = $state(depth === 0);
  const scope = $derived({ type: "folder" as const, path: node.path, recursive: true });
  const active = $derived(lib.isScope(scope));
  const picked = $derived(lib.folderSel.has(node.path));

  /** Sichtbare Ordner in Anzeigereihenfolge (für Umschalt-Klick) */
  const visibleOrder = () =>
    [...document.querySelectorAll<HTMLElement>(".tree [data-folder]")].map((e) => e.dataset.folder!);

  function onclick(e: MouseEvent) {
    if (e.ctrlKey || e.metaKey) lib.selectFolder(node.path, "toggle", visibleOrder());
    else if (e.shiftKey) lib.selectFolder(node.path, "range", visibleOrder());
    else {
      lib.clearFolderSel();
      lib.setScope(scope);
    }
  }
</script>

<li>
  <div
    class="row"
    role="treeitem"
    tabindex="-1"
    aria-selected={active}
    aria-expanded={node.children.length ? open : undefined}
    class:active
    class:picked
    data-folder={node.path}
    style:padding-left="{6 + depth * 14}px"
    oncontextmenu={(e) => {
      e.preventDefault();
      oncontext(node, e);
    }}
  >
    {#if node.children.length}
      <button class="twisty" class:open aria-label={open ? t("Zuklappen") : t("Aufklappen")} onclick={() => (open = !open)}>
        <Icon name="chevron" size={12} />
      </button>
    {:else}
      <span class="twisty"></span>
    {/if}
    <button class="label" title={`${node.path}\n${t("Strg-/Umschalt-Klick wählt mehrere Ordner")}`} {onclick}>
      <span class="name">{node.name}</span>
      <span class="count">{node.total}</span>
    </button>
    <button class="more" aria-label={t("Zu Projekt hinzufügen")} title={t("Zu Projekt hinzufügen")} onclick={(e) => oncontext(node, e)}>
      <Icon name="stack" size={13} />
    </button>
  </div>
  {#if open && node.children.length}
    <ul>
      {#each node.children as child (child.path)}
        <FolderTree node={child} depth={depth + 1} {oncontext} />
      {/each}
    </ul>
  {/if}
</li>

<style>
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .row {
    display: flex;
    align-items: center;
    height: 26px;
    border-radius: var(--radius-s);
    padding-right: 4px;
  }
  .row:hover {
    background: var(--raised);
  }
  .row.active {
    background: var(--raised);
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .row.picked {
    background: color-mix(in srgb, var(--accent) 22%, transparent);
  }
  .twisty {
    flex: none;
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    color: var(--faint);
    transition: transform 0.12s;
  }
  .twisty.open {
    transform: rotate(90deg);
  }
  .label {
    flex: 1;
    display: flex;
    min-width: 0;
    gap: 8px;
    text-align: left;
    padding: 3px 4px;
  }
  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    color: var(--faint);
    font-size: var(--step--1);
  }
  .more {
    display: none;
    color: var(--muted);
    padding: 3px;
    border-radius: var(--radius-s);
  }
  .row:hover .more {
    display: grid;
  }
  .more:hover {
    color: var(--accent);
  }
  .row:hover .count {
    display: none;
  }
</style>
