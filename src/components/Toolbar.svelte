<script lang="ts">
  import { lib } from "../lib/store.svelte";
  import { album } from "../lib/album.svelte";
  import type { Scope } from "../lib/api";
  import { dev } from "../lib/develop.svelte";
  import { t } from "../lib/i18n";
  import { withKey } from "../lib/keymap.svelte";
  import Icon from "./Icon.svelte";
  import FileTypeFilter from "./FileTypeFilter.svelte";
  import ProjectFolders from "./ProjectFolders.svelte";
  import PersonName from "./PersonName.svelte";

  let {
    ontogglesidebar,
    ontoggleinfo,
    ondeleterejected,
  }: { ontogglesidebar: () => void; ontoggleinfo: () => void; ondeleterejected: () => void } = $props();

  let foldersOpen = $state(false);
  let holdStart = 0;
  const project = $derived(lib.scope.type === "project" ? lib.projects.find((p) => p.id === (lib.scope as { id: number }).id) : undefined);
  // Fotos einer Person: Name im Titel per Klick änderbar
  const scopePerson = $derived.by(() => {
    const s = lib.scope;
    return s.type === "person" ? lib.people.find((x) => x.id === s.id) : undefined;
  });
</script>

<div class="bar">
  <button class="icon-btn sidebar-toggle" aria-label={t("Seitenleiste")} onclick={ontogglesidebar}><Icon name="menu" /></button>

  <div class="modes" role="tablist" aria-label={t("Modus")}>
    <button role="tab" aria-selected={lib.mode === "library" && lib.view !== "map"} class:on={lib.mode === "library" && lib.view !== "map"} onclick={() => { lib.mode = "library"; lib.view = "grid"; }} title={withKey(t("Bibliothek"), "grid")}>
      {t("Bibliothek")}
    </button>
    <button role="tab" aria-selected={lib.mode === "develop"} class:on={lib.mode === "develop"} onclick={() => lib.enterDevelop()} disabled={!lib.images.length} title={withKey(t("Entwickeln"), "develop")}>
      {t("Entwickeln")}
    </button>
    {#if lib.mapEnabled}
    <button role="tab" aria-selected={lib.mode === "library" && lib.view === "map"} class:on={lib.mode === "library" && lib.view === "map"} onclick={() => lib.openMap()} title={withKey(t("Karte: Aufnahmeorte ansehen und setzen"), "map")}>
      {t("Karte")}
    </button>
    {/if}
  </div>

  {#if lib.mode === "library" && lib.view === "people" && lib.peopleReviewOpen}
    <!-- Wie bei den Fotos einer benannten Person: an dieser Stelle zurück zur Personenübersicht -->
    <button class="btn quiet" onclick={() => lib.peopleBack++} title={t("Zurück zur Übersicht aller Personen")}>
      <Icon name="back" size={14} /> {t("Personen")}
    </button>
  {:else if lib.mode === "library" && (lib.view === "loupe" || lib.view === "compare" || lib.view === "people" || lib.view === "album" || lib.view === "stats")}
    <button class="btn quiet" onclick={() => (lib.view = "grid")}><Icon name="back" size={14} /> {t("Raster")}</button>
  {/if}
  {#if lib.scope.type === "person" && lib.view !== "people"}
    <button class="btn quiet" onclick={() => lib.openPeople()} title={t("Zurück zur Übersicht aller Personen")}>
      <Icon name="back" size={14} /> {t("Personen")}
    </button>
  {/if}
  {#if lib.mode === "develop"}
    <!-- Vorher/Nachher: Klick schaltet um, gedrückt halten zeigt kurz das Original -->
    <button
      class="btn before"
      class:on={dev.showBefore || dev.holdBefore}
      aria-pressed={dev.showBefore}
      onpointerdown={() => {
        holdStart = performance.now();
        dev.holdBefore = true;
      }}
      onpointerup={() => (dev.holdBefore = false)}
      onpointerleave={() => (dev.holdBefore = false)}
      onclick={() => {
        // Lang gedrückt: nur kurzer Vergleich, nicht umschalten
        if (performance.now() - holdStart < 300) dev.showBefore = !dev.showBefore;
      }}
      title={`${withKey(t("Bild ohne Bearbeitung zeigen"), "before")}. ${t("Klick schaltet um, gedrückt halten zeigt es kurz.")}`}
    >
      <Icon name="develop" size={14} />
      {dev.showBefore ? t("Bearbeitung zeigen") : t("Original zeigen")}
    </button>
  {/if}
  {#if lib.mode === "library" && lib.selection.size > 1 && lib.view !== "compare"}
    <button class="btn" onclick={() => lib.compare()} title={withKey(t("Ausgewählte Fotos groß nebeneinander vergleichen"), "compare")}>
      {t("Vergleichen ({n})", { n: Math.min(lib.selection.size, 12) })}
    </button>
  {/if}

  <div class="title">
    {#if lib.view === "people" && lib.mode === "library"}
      <h1>{t("Personen")}</h1>
      <span class="muted">{t("{n} benannt", { n: String(lib.people.filter((p) => p.name).length) })}</span>
    {:else}
      {#if scopePerson}
        <h1><PersonName person={scopePerson} /></h1>
      {:else}
        <h1>{lib.scopeTitle}</h1>
      {/if}
      <span class="muted">{t(lib.images.length === 1 ? "{n} Foto" : "{n} Fotos", { n: String(lib.images.length) })}{lib.selection.size > 1 ? t(", {n} ausgewählt", { n: String(lib.selection.size) }) : ""}</span>
    {/if}
  </div>

  {#if project}
    <div class="pf">
      <button class="btn" onclick={(e) => { e.stopPropagation(); foldersOpen = !foldersOpen; }} aria-expanded={foldersOpen}>
        <Icon name="folder" size={14} /> {t("{n} Ordner", { n: String(project.folders.length) })}
      </button>
      {#if foldersOpen}
        <ProjectFolders {project} onclose={() => (foldersOpen = false)} />
      {/if}
    </div>
  {/if}

  {#if lib.mode === "library" && lib.view === "grid" && (lib.scope.type === "project" || lib.scope.type === "folder") && lib.images.length}
    <button class="btn" onclick={() => album.open($state.snapshot(lib.scope) as Scope, lib.scopeTitle)} title={t("Aus den besten Fotos dieser Ansicht ein Fotoalbum gestalten und als PDF speichern")}>
      <Icon name="book" size={14} /> {t("Fotoalbum")}
    </button>
  {/if}

  {#if lib.mode === "library" && lib.view === "grid" && lib.images.length}
    <button class="btn" onclick={() => (lib.view = "stats")} title={t("Auswertung der angezeigten Fotos: Kameras, Objektive, Brennweiten, Blenden, Schlagworte …")}>
      <Icon name="chart" size={14} /> {t("Statistik")}
    </button>
  {/if}

  <div class="spacer"></div>

  {#if lib.rejectedCount && lib.view !== "people"}
    <button class="btn reject-btn" onclick={ondeleterejected} title={withKey(t("Abgelehnte Fotos dieser Ansicht von der Festplatte löschen"), "deleteRejected")}>
      <Icon name="trash" size={14} /> {t("{n} abgelehnt", { n: String(lib.rejectedCount) })}
    </button>
  {/if}

  <label class="search" class:semantic={lib.semantic}>
    <Icon name={lib.semantic ? "sparkle" : "search"} size={14} />
    <input
      type="search"
      placeholder={lib.semantic ? t("Bildinhalt beschreiben") : t("Suchen")}
      title={lib.semantic
        ? t("Sucht nach dem, was auf den Fotos zu sehen ist, z. B. „Hund am Strand“")
        : t("Sucht in Dateiname, Ort, Kamera, Objektiv und Schlagworten")}
      bind:value={lib.search}
      oninput={() => lib.reloadSoon()}
      onkeydown={(e) => e.stopPropagation()}
    />
    {#if lib.ai?.modelReady && lib.aiOn}
      <button
        class="mode"
        class:on={lib.semantic}
        aria-pressed={lib.semantic}
        title={lib.semantic ? t("Zur Textsuche wechseln") : t("Nach Bildinhalt suchen (KI)")}
        onclick={(e) => { e.preventDefault(); lib.semantic = !lib.semantic; if (lib.search.trim()) lib.loadImages(); }}
      >
        {t("KI")}
      </button>
    {/if}
  </label>

  <div class="stars" role="group" aria-label={t("Mindestbewertung")}>
    {#each [1, 2, 3, 4, 5] as n}
      <button
        class:on={lib.minRating >= n}
        aria-label={t(n === 1 ? "Mindestens {n} Stern" : "Mindestens {n} Sterne", { n })}
        title={t(n === 1 ? "Mindestens {n} Stern" : "Mindestens {n} Sterne", { n })}
        onclick={() => { lib.minRating = lib.minRating === n ? 0 : n; lib.loadImages(); }}
      >
        <Icon name="star" size={13} fill={lib.minRating >= n} />
      </button>
    {/each}
  </div>

  {#if lib.view !== "people"}
    <FileTypeFilter />
  {/if}

  <select bind:value={lib.sort} onchange={() => lib.loadImages(true)} aria-label={t("Sortierung")}>
    <option value="capturedAsc">{t("Älteste zuerst")}</option>
    <option value="capturedDesc">{t("Neueste zuerst")}</option>
    <option value="nameAsc">{t("Dateiname")}</option>
    <option value="ratingDesc">{t("Bewertung")}</option>
  </select>

  {#if lib.mode === "library" && lib.view === "grid"}
    <input class="size" type="range" min="110" max="420" step="10" bind:value={lib.rowHeight} aria-label={t("Vorschaugröße")} />
  {/if}

  {#if lib.mode === "library"}
    <button class="icon-btn" aria-label={t("Informationen")} title={withKey(t("Informationen"), "info")} onclick={ontoggleinfo}><Icon name="info" /></button>
  {/if}
</div>

<style>
  .btn.before.on {
    border-color: var(--accent);
    color: var(--accent);
  }
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
    .title span,
    .bar > :global(.ftf) {
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
