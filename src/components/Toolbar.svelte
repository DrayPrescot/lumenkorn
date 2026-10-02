<script lang="ts">
  import { lib } from "../lib/store.svelte";
  import Icon from "./Icon.svelte";
  import ProjectFolders from "./ProjectFolders.svelte";

  let {
    ontogglesidebar,
    ontoggleinfo,
    ondeleterejected,
  }: { ontogglesidebar: () => void; ontoggleinfo: () => void; ondeleterejected: () => void } = $props();

  let foldersOpen = $state(false);
  const project = $derived(lib.scope.type === "project" ? lib.projects.find((p) => p.id === (lib.scope as { id: number }).id) : undefined);
</script>

<div class="bar">
  <button class="icon-btn sidebar-toggle" aria-label="Seitenleiste" onclick={ontogglesidebar}><Icon name="menu" /></button>

  <div class="modes" role="tablist" aria-label="Modus">
    <button role="tab" aria-selected={lib.mode === "library"} class:on={lib.mode === "library"} onclick={() => { lib.mode = "library"; lib.view = "grid"; }} title="Bibliothek (G)">
      Bibliothek
    </button>
    <button role="tab" aria-selected={lib.mode === "develop"} class:on={lib.mode === "develop"} onclick={() => lib.enterDevelop()} disabled={!lib.images.length} title="Entwickeln (D)">
      Entwickeln
    </button>
  </div>

  {#if lib.mode === "library" && (lib.view === "loupe" || lib.view === "compare" || lib.view === "people")}
    <button class="btn quiet" onclick={() => (lib.view = "grid")}><Icon name="back" size={14} /> Raster</button>
  {/if}
  {#if lib.mode === "library" && lib.selection.size > 1 && lib.view !== "compare"}
    <button class="btn" onclick={() => lib.compare()} title="Ausgewählte Fotos groß nebeneinander vergleichen (C)">
      Vergleichen ({Math.min(lib.selection.size, 12)})
    </button>
  {/if}

  <div class="title">
    {#if lib.view === "people" && lib.mode === "library"}
      <h1>Personen</h1>
      <span class="muted">{lib.people.filter((p) => p.name).length} benannt</span>
    {:else}
      <h1>{lib.scopeTitle}</h1>
      <span class="muted">{lib.images.length} {lib.images.length === 1 ? "Foto" : "Fotos"}{lib.selection.size > 1 ? `, ${lib.selection.size} ausgewählt` : ""}</span>
    {/if}
  </div>

  {#if project}
    <div class="pf">
      <button class="btn" onclick={(e) => { e.stopPropagation(); foldersOpen = !foldersOpen; }} aria-expanded={foldersOpen}>
        <Icon name="folder" size={14} /> {project.folders.length} Ordner
      </button>
      {#if foldersOpen}
        <ProjectFolders {project} onclose={() => (foldersOpen = false)} />
      {/if}
    </div>
  {/if}

  <div class="spacer"></div>

  {#if lib.rejected.length && lib.view !== "people"}
    <button class="btn reject-btn" onclick={ondeleterejected} title="Abgelehnte Fotos dieser Ansicht von der Festplatte löschen (Strg+Entf)">
      <Icon name="trash" size={14} /> {lib.rejected.length} abgelehnt
    </button>
  {/if}

  <label class="search" class:semantic={lib.semantic}>
    <Icon name={lib.semantic ? "sparkle" : "search"} size={14} />
    <input
      type="search"
      placeholder={lib.semantic ? "Bildinhalt beschreiben" : "Suchen"}
      title={lib.semantic
        ? "Sucht nach dem, was auf den Fotos zu sehen ist, z. B. „Hund am Strand“"
        : "Sucht in Dateiname, Kamera, Objektiv und Schlagworten"}
      bind:value={lib.search}
      oninput={() => lib.reloadSoon()}
      onkeydown={(e) => e.stopPropagation()}
    />
    {#if lib.ai?.modelReady}
      <button
        class="mode"
        class:on={lib.semantic}
        aria-pressed={lib.semantic}
        title={lib.semantic ? "Zur Textsuche wechseln" : "Nach Bildinhalt suchen (KI)"}
        onclick={(e) => { e.preventDefault(); lib.semantic = !lib.semantic; if (lib.search.trim()) lib.loadImages(); }}
      >
        KI
      </button>
    {/if}
  </label>

  <div class="stars" role="group" aria-label="Mindestbewertung">
    {#each [1, 2, 3, 4, 5] as n}
      <button
        class:on={lib.minRating >= n}
        aria-label="Mindestens {n} Sterne"
        title="Mindestens {n} {n === 1 ? 'Stern' : 'Sterne'}"
        onclick={() => { lib.minRating = lib.minRating === n ? 0 : n; lib.loadImages(); }}
      >
        <Icon name="star" size={13} fill={lib.minRating >= n} />
      </button>
    {/each}
  </div>

  <select bind:value={lib.sort} onchange={() => lib.loadImages(true)} aria-label="Sortierung">
    <option value="capturedAsc">Älteste zuerst</option>
    <option value="capturedDesc">Neueste zuerst</option>
    <option value="nameAsc">Dateiname</option>
    <option value="ratingDesc">Bewertung</option>
  </select>

  {#if lib.mode === "library" && lib.view === "grid"}
    <input class="size" type="range" min="110" max="420" step="10" bind:value={lib.rowHeight} aria-label="Vorschaugröße" />
  {/if}

  {#if lib.mode === "library"}
    <button class="icon-btn" aria-label="Informationen" title="Informationen (I)" onclick={ontoggleinfo}><Icon name="info" /></button>
  {/if}
</div>

<style>
  .reject-btn {
    color: var(--reject);
    white-space: nowrap;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    height: var(--toolbar);
    padding: 0 12px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
    flex: none;
    min-width: 0;
  }
  .sidebar-toggle {
    display: none;
  }
  .modes {
    flex: none;
    display: flex;
    padding: 2px;
    gap: 2px;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
  }
  .modes button {
    padding: 4px 10px;
    border-radius: 6px;
    color: var(--muted);
  }
  .modes button:hover:not(:disabled) {
    color: var(--text);
  }
  .modes button.on {
    background: var(--raised);
    color: var(--text);
  }
  .modes button:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .title {
    flex: none;
    display: flex;
    align-items: baseline;
    gap: 10px;
    min-width: 0;
  }
  h1 {
    flex: none;
    max-width: 22ch;
    margin: 0;
    font-size: var(--step-1);
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .title span {
    white-space: nowrap;
    font-size: var(--step--1);
  }
  .pf {
    position: relative;
  }
  .spacer {
    flex: 1;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 6px;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
    padding: 0 8px;
    color: var(--faint);
    width: min(250px, 20vw);
  }
  .search.semantic {
    border-color: rgb(227 169 58 / 0.5);
  }
  .search.semantic :global(svg) {
    color: var(--accent);
  }
  .mode {
    flex: none;
    font-size: 10.5px;
    font-weight: 600;
    padding: 1px 6px;
    border-radius: 4px;
    color: var(--faint);
    border: 1px solid var(--line);
  }
  .mode:hover {
    color: var(--text);
  }
  .mode.on {
    color: var(--accent-ink);
    background: var(--accent);
    border-color: var(--accent);
  }
  .search:focus-within {
    border-color: var(--faint);
  }
  .search input {
    color: var(--text);
    border: 0;
    background: none;
    padding: 5px 0;
    flex: 1;
    outline: none;
  }
  .stars {
    display: flex;
  }
  .stars button {
    padding: 4px 2px;
    color: var(--faint);
  }
  .stars button.on {
    color: var(--accent);
  }
  select {
    flex: none;
  }
  .size {
    width: 96px;
    accent-color: var(--accent);
  }
  .bar > .icon-btn {
    flex: none;
  }
  @media (max-width: 1500px) {
    .size {
      display: none;
    }
  }
  @media (max-width: 1100px) {
    select {
      display: none;
    }
  }
  @media (max-width: 800px) {
    .sidebar-toggle {
      display: inline-grid;
    }
    .stars,
    .title span {
      display: none;
    }
    .search {
      width: auto;
      flex: 1;
    }
    .spacer {
      display: none;
    }
  }
</style>
