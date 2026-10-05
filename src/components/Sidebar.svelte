<script lang="ts">
  import { onMount } from "svelte";
  import { api, faceUrl, isTauri, pickFolder, type FolderNode, type Root } from "../lib/api";
  import { lib, topFolders } from "../lib/store.svelte";
  import { lang, setLang, t, tc } from "../lib/i18n";
  import FolderTree from "./FolderTree.svelte";
  import Icon from "./Icon.svelte";
  import PathDialog from "./PathDialog.svelte";
  import AiDialog from "./AiDialog.svelte";

  let platform = $state("");
  let aiOpen = $state(false);
  // Nach einem Absturz beim letzten Start: Dialog mit Erklärung öffnen
  let crashShown = false;
  $effect(() => {
    if (lib.ai?.crashNotice && !crashShown) {
      crashShown = true;
      aiOpen = true;
    }
  });
  let pathDialog = $state(false);
  let newProject = $state<string | null>(null);
  // `paths`: mehrere gewählte Ordner (Strg-/Umschalt-Klick), sonst nur `node`
  let menu = $state<{ x: number; y: number; node: FolderNode; root?: Root; paths: string[] } | null>(null);
  let renaming = $state<number | null>(null);
  let renamingPerson = $state<number | null>(null);

  onMount(async () => {
    platform = (await api.appInfo()).platform;
  });

  // Auf Android gibt es keinen Ordnerdialog mit echten Dateipfaden.
  const nativePicker = $derived(isTauri && platform !== "android" && platform !== "ios");

  async function addFolder() {
    if (!nativePicker) {
      pathDialog = true;
      return;
    }
    const path = await pickFolder(t("Ordner zur Bibliothek hinzufügen"));
    if (path) await lib.addRoot(path);
  }

  function openMenu(node: FolderNode, e: MouseEvent, root?: Root) {
    const r = (e.currentTarget as HTMLElement | null)?.getBoundingClientRect();
    const x = e.type === "contextmenu" || !r ? e.clientX : r.right;
    const y = e.type === "contextmenu" || !r ? e.clientY : r.top;
    // Rechtsklick in die Mehrfachauswahl: gilt für alle; außerhalb: nur dieser Ordner
    const many = lib.folderSel.size > 1 && lib.folderSel.has(node.path);
    if (!many) lib.clearFolderSel();
    const paths = many ? topFolders([...lib.folderSel]) : [node.path];
    menu = { x: Math.max(8, Math.min(x, window.innerWidth - 330)), y: Math.max(8, Math.min(y, window.innerHeight - 280)), node, root, paths };
  }

  async function createProjectFromMenu() {
    if (!menu) return;
    const { node, paths } = menu;
    menu = null;
    // Mehrere Ordner mit gemeinsamem Elternordner: nach diesem benennen
    const parent = (p: string) => p.replace(/[\\/]+$/, "").replace(/[\\/][^\\/]*$/, "");
    const common = paths.length > 1 && paths.every((p) => parent(p) === parent(paths[0])) ? parent(paths[0]) : null;
    const name = paths.length > 1 ? (common?.split(/[\\/]/).pop() || t("Neues Projekt")) : node.name;
    const p = await lib.createProject(name);
    if (p) {
      await lib.addFoldersToProject(p.id, paths);
      lib.clearFolderSel();
      lib.setScope({ type: "project", id: p.id });
      // Bei mehreren Ordnern passt der Vorschlag selten: Namen gleich bearbeiten
      if (paths.length > 1) renaming = p.id;
    }
  }

  function addToProject(projectId: number) {
    if (!menu) return;
    const paths = menu.paths;
    menu = null;
    lib.addFoldersToProject(projectId, paths).then(() => lib.clearFolderSel());
  }

  async function submitNewProject() {
    const name = newProject?.trim();
    newProject = null;
    if (!name) return;
    const p = await lib.createProject(name);
    if (p) lib.setScope({ type: "project", id: p.id });
  }

  function rootFor(node: FolderNode) {
    return lib.roots.find((r) => r.path === node.path);
  }

  const scanningRoot = $derived(lib.scan ? lib.roots.find((r) => r.id === lib.scan!.rootId) : undefined);
</script>

{#snippet toggle(key: "projects" | "people" | "folders", title: string, count: number)}
  {@const closed = !!lib.collapsed[key]}
  <button class="toggle" aria-expanded={!closed} title={closed ? t("{title} aufklappen", { title }) : t("{title} einklappen", { title })} onclick={() => lib.toggleSection(key)}>
    <span class="chev" class:closed><Icon name="chevron" size={11} /></span>
    <h2>{title}</h2>
    {#if closed && count}<span class="count">{count}</span>{/if}
  </button>
{/snippet}

<svelte:window
  onclick={() => (menu = null)}
  onkeydown={(e) => {
    if (e.key !== "Escape") return;
    if (menu) menu = null;
    else lib.clearFolderSel();
  }}
/>

<aside>
  <div class="brand">
    <span class="mark" aria-hidden="true"></span>
    Lumenkorn
  </div>

  <nav class="scroll">
    <button class="item" class:active={lib.isScope({ type: "all" })} onclick={() => lib.setScope({ type: "all" })}>
      <Icon name="photos" size={15} />
      <span>{t("Alle Fotos")}</span>
      <span class="count">{lib.roots.reduce((s, r) => s + r.imageCount, 0)}</span>
    </button>

    <section>
      <header>
        {@render toggle("projects", t("Projekte"), lib.projects.length)}
        <button class="icon-btn" aria-label={t("Neues Projekt")} title={t("Neues Projekt")} onclick={() => (newProject = "")}>
          <Icon name="plus" size={14} />
        </button>
      </header>
      {#if newProject !== null && !lib.collapsed.projects}
        <form onsubmit={(e) => (e.preventDefault(), submitNewProject())}>
          <!-- svelte-ignore a11y_autofocus -->
          <input
            type="text"
            placeholder={t("Name des Projekts")}
            bind:value={newProject}
            autofocus
            onblur={submitNewProject}
            onkeydown={(e) => e.key === "Escape" && (newProject = null)}
          />
        </form>
      {/if}
      {#each lib.projects as p (p.id)}
        {#if lib.collapsed.projects && !lib.isScope({ type: "project", id: p.id })}
          <!-- eingeklappt: nur das gerade gewählte Projekt zeigen -->
        {:else if renaming === p.id}
          <form
            onsubmit={(e) => {
              // Übernehmen über onblur (eine Stelle für Enter und Wegklicken)
              e.preventDefault();
              (e.currentTarget.elements.namedItem("n") as HTMLInputElement).blur();
            }}
          >
            <!-- svelte-ignore a11y_autofocus -->
            <input
              type="text"
              name="n"
              value={p.name}
              autofocus
              onfocus={(e) => e.currentTarget.select()}
              onblur={(e) => {
                // Verlassen übernimmt wie Enter
                const v = e.currentTarget.value;
                renaming = null;
                if (v.trim() && v !== p.name) lib.renameProject(p.id, v);
              }}
              onkeydown={(e) => {
                e.stopPropagation();
                if (e.key === "Escape") {
                  e.currentTarget.value = p.name; // verwerfen
                  e.currentTarget.blur();
                }
              }}
            />
          </form>
        {:else}
          <button
            class="item"
            class:active={lib.isScope({ type: "project", id: p.id })}
            onclick={() => lib.setScope({ type: "project", id: p.id })}
            ondblclick={() => (renaming = p.id)}
            title={t("{n} Ordner. Doppelklick zum Umbenennen.", { n: String(p.folders.length) })}
          >
            <Icon name="stack" size={15} />
            <span>{p.name}</span>
            <span class="count">{p.imageCount}</span>
          </button>
        {/if}
      {:else}
        {#if newProject === null && !lib.collapsed.projects}
          <p class="hint">{t("Ein Projekt bündelt Ordner von beliebigen Orten. Rechtsklick auf einen Ordner fügt ihn hinzu – mit Strg- oder Umschalt-Klick auch mehrere auf einmal.")}</p>
        {/if}
      {/each}
    </section>

    <section>
      <header>
        {@render toggle("people", t("Personen"), lib.people.filter((p) => p.name).length)}
        <button class="icon-btn" aria-label={t("Alle Personen")} title={t("Personen und Gruppen verwalten")} onclick={() => lib.openPeople()}>
          <Icon name="stack" size={14} />
        </button>
      </header>
      {#each lib.people.filter((p) => p.name) as p (p.id)}
        {#if lib.collapsed.people && !lib.isScope({ type: "person", id: p.id })}
          <!-- eingeklappt: nur die gerade gewählte Person zeigen -->
        {:else if renamingPerson === p.id}
          <form
            onsubmit={async (e) => {
              e.preventDefault();
              const v = (e.currentTarget.elements.namedItem("n") as HTMLInputElement).value;
              renamingPerson = null;
              await lib.renamePerson(p.id, v);
            }}
          >
            <!-- svelte-ignore a11y_autofocus -->
            <input
              type="text"
              name="n"
              value={p.name}
              autofocus
              onfocus={(e) => e.currentTarget.select()}
              onblur={() => (renamingPerson = null)}
              onkeydown={(e) => {
                e.stopPropagation();
                if (e.key === "Escape") renamingPerson = null;
              }}
            />
          </form>
        {:else}
          <button
            class="item person"
            class:active={lib.isScope({ type: "person", id: p.id })}
            onclick={() => lib.setScope({ type: "person", id: p.id })}
            ondblclick={() => (renamingPerson = p.id)}
            oncontextmenu={(e) => {
              e.preventDefault();
              renamingPerson = p.id;
            }}
            title={t("Doppelklick oder Rechtsklick zum Umbenennen")}
          >
            {#if p.coverFace}<img class="avatar" src={faceUrl(p.coverFace)} alt="" />{:else}<span class="avatar"></span>{/if}
            <span>{p.name}</span>
            <span class="count">{p.imageCount}</span>
          </button>
        {/if}
      {/each}
      {#if lib.collapsed.people}
        <!-- eingeklappt -->
      {:else if lib.peopleStatus?.ready}
        {@const unnamed = lib.people.filter((p) => !p.name && p.faceCount >= 2).length}
        <button class="item" class:active={lib.view === "people" && lib.mode === "library"} onclick={() => lib.openPeople()}>
          <Icon name="sparkle" size={15} />
          <span>{unnamed ? t("{n} unbenannte Gruppen", { n: String(unnamed) }) : t("Personen verwalten")}</span>
        </button>
      {:else}
        <p class="hint">{t("Erkennt Gesichter und gruppiert gleiche Personen – einrichten über „KI“ unten links.")}</p>
      {/if}
    </section>

    <section>
      <header>
        {@render toggle("folders", t("Ordner"), lib.roots.length)}
        <button class="icon-btn" aria-label={t("Ordner hinzufügen")} title={t("Ordner hinzufügen")} onclick={addFolder}>
          <Icon name="plus" size={14} />
        </button>
      </header>
      {#each lib.roots as root (root.id)}
        {@const tree = lib.trees[root.id]}
        {#if tree && !lib.collapsed.folders}
          <ul class="tree">
            <FolderTree node={tree} oncontext={(n, e) => openMenu(n, e, rootFor(n))} />
          </ul>
        {/if}
      {:else}
        <div class="empty" hidden={lib.collapsed.folders}>
          <p>{t("Wähle einen Ordner auf deinem Rechner. Die Bilder bleiben, wo sie sind – Lumenkorn liest sie nur ein.")}</p>
          <button class="btn primary" onclick={addFolder}><Icon name="folder" size={14} /> {t("Ordner hinzufügen")}</button>
        </div>
      {/each}
    </section>
  </nav>

  <footer>
    <div class="foot-row">
      <button class="ai" onclick={() => (aiOpen = true)} title={t("Lokale KI: Verschlagwortung und Inhaltssuche")}>
        <Icon name="sparkle" size={13} />
        {#if lib.aiDownload}
          <span>{t("KI wird heruntergeladen …")}</span>
        {:else if !lib.ai || !lib.ai.modelReady}
          <span>{t("KI einrichten")}</span>
        {:else if lib.ai.analyzed < lib.ai.total}
          <span>{t("KI: {done} von {total} analysiert", { done: String(lib.ai.analyzed), total: String(lib.ai.total) })}</span>
        {:else}
          <span>{t("KI: alle Fotos analysiert")}</span>
        {/if}
      </button>
      <!-- Sprache der Oberfläche (Wechsel lädt neu) -->
      <div class="lang" role="group" aria-label="Sprache / Language">
        <button class:on={lang === "de"} aria-pressed={lang === "de"} title="Deutsch" onclick={() => lang !== "de" && setLang("de")}>DE</button>
        <button class:on={lang === "en"} aria-pressed={lang === "en"} title="English" onclick={() => lang !== "en" && setLang("en")}>EN</button>
      </div>
    </div>
    {#if lib.scan}
      <div class="scan">
        <span>
          {scanningRoot?.name ?? tc("one", "Ordner")}:
          {lib.scan.phase === "walking" ? t("{n} Dateien gefunden", { n: String(lib.scan.done) }) : lib.scan.phase === "reading" ? t("lese Metadaten") : t("speichere")}
        </span>
        {#if lib.scan.total}
          <progress max={lib.scan.total} value={lib.scan.done}></progress>
        {/if}
      </div>
    {/if}
  </footer>
</aside>

{#if menu}
  {@const m = menu}
  {@const many = m.paths.length > 1}
  <div class="menu" role="menu" tabindex="-1" style:left="{m.x}px" style:top="{m.y}px" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
    <div class="menu-title">{many ? t("{n} Ordner", { n: String(m.paths.length) }) : m.node.name}</div>
    {#each lib.projects as p (p.id)}
      {@const already = m.paths.every((path) => p.folders.some((f) => f.path === path))}
      <button role="menuitem" disabled={already} onclick={() => addToProject(p.id)}>
        <Icon name="stack" size={14} /> {t("Zu „{name}“ hinzufügen", { name: p.name })} {already ? t("(enthalten)") : ""}
      </button>
    {/each}
    <button role="menuitem" onclick={createProjectFromMenu}>
      <Icon name="plus" size={14} /> {many ? t("Neues Projekt aus diesen Ordnern") : t("Neues Projekt aus diesem Ordner")}
    </button>
    {#if many}
      <p class="menu-note">{t("Strg-/Umschalt-Klick ändert die Auswahl, Esc hebt sie auf.")}</p>
    {:else if m.root}
      {@const root = m.root}
      <hr />
      <button role="menuitem" onclick={() => { menu = null; api.rescanRoot(root.id); }}>
        <Icon name="refresh" size={14} /> {t("Neu einlesen")}
      </button>
      <button role="menuitem" class="danger" onclick={() => { menu = null; lib.removeRoot(root.id); }}>
        <Icon name="trash" size={14} /> {t("Aus Bibliothek entfernen")}
      </button>
      <p class="menu-note">{t("Dateien auf der Platte bleiben erhalten.")}</p>
    {/if}
  </div>
{/if}

{#if aiOpen}
  <AiDialog onclose={() => (aiOpen = false)} />
{/if}

{#if pathDialog}
  <PathDialog
    title={t("Ordner hinzufügen")}
    hint={platform === "android" ? t("Zum Beispiel /storage/emulated/0/DCIM. Lumenkorn braucht dafür die Berechtigung „Zugriff auf alle Dateien“.") : t("Vollständiger Pfad des Ordners.")}
    onsubmit={(p) => { pathDialog = false; lib.addRoot(p); }}
    oncancel={() => (pathDialog = false)}
  />
{/if}

<style>
  aside {
    display: flex;
    flex-direction: column;
    background: var(--panel);
    border-right: 1px solid var(--line);
    min-height: 0;
    height: 100%;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    height: var(--toolbar);
    padding: 0 16px;
    font-weight: 650;
    font-size: var(--step-1);
    letter-spacing: -0.01em;
    flex: none;
  }
  .mark {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: radial-gradient(circle at 50% 50%, #ffd68c 0 28%, transparent 30%), conic-gradient(var(--accent), #b9792a, var(--accent));
  }
  nav {
    flex: 1;
    min-height: 0;
    padding: 4px 10px 16px;
  }
  section {
    margin-top: 18px;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 4px 0 8px;
    margin-bottom: 4px;
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 5px;
    flex: 1;
    min-width: 0;
    height: 24px;
    margin-left: -4px;
    padding: 0 4px;
    border-radius: var(--radius-s);
    text-align: left;
  }
  .toggle:hover h2 {
    color: var(--text);
  }
  .chev {
    display: inline-flex;
    color: var(--faint);
    transform: rotate(90deg);
    transition: transform 0.12s;
  }
  .chev.closed {
    transform: none;
  }
  h2 {
    margin: 0;
    font-size: var(--step--1);
    font-weight: 600;
    color: var(--muted);
  }
  .item {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    height: 30px;
    padding: 0 8px;
    border-radius: var(--radius-s);
    text-align: left;
    color: var(--text);
  }
  .item span:first-of-type {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .item:hover {
    background: var(--raised);
  }
  .item.active {
    background: var(--raised);
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .item :global(svg) {
    color: var(--muted);
    flex: none;
  }
  .count {
    color: var(--faint);
    font-size: var(--step--1);
  }
  .tree {
    list-style: none;
    margin: 0 0 6px;
    padding: 0;
  }
  form input {
    width: 100%;
    margin: 2px 0 4px;
  }
  .hint,
  .empty p {
    margin: 2px 8px 8px;
    color: var(--faint);
    font-size: var(--step--1);
    max-width: 30ch;
  }
  .empty {
    padding: 4px 0;
  }
  .empty .btn {
    margin-left: 8px;
  }
  footer {
    flex: none;
    border-top: 1px solid var(--line);
    padding: 10px 16px;
    min-height: 44px;
    font-size: var(--step--1);
    color: var(--muted);
    display: grid;
    gap: 8px;
  }
  .foot-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .foot-row .ai {
    flex: 1;
    min-width: 0;
  }
  .lang {
    display: flex;
    flex: none;
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
    overflow: hidden;
  }
  .lang button {
    padding: 2px 6px;
    font-size: 11px;
    color: var(--faint);
  }
  .lang button.on {
    background: var(--raised);
    color: inherit;
  }
  .ai {
    display: flex;
    align-items: center;
    gap: 7px;
    color: var(--muted);
    text-align: left;
    padding: 3px 0;
  }
  .ai :global(svg) {
    color: var(--accent);
    flex: none;
  }
  .ai:hover {
    color: var(--text);
  }
  .scan {
    display: grid;
    gap: 6px;
  }
  progress {
    width: 100%;
    height: 3px;
    appearance: none;
    border: 0;
    background: var(--line);
  }
  progress::-webkit-progress-bar {
    background: var(--line);
  }
  progress::-webkit-progress-value {
    background: var(--accent);
  }
  progress::-moz-progress-bar {
    background: var(--accent);
  }

  .menu {
    position: fixed;
    z-index: 50;
    min-width: 240px;
    max-width: 320px;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    box-shadow: 0 12px 32px rgb(0 0 0 / 0.45);
    padding: 5px;
  }
  .menu-title {
    padding: 6px 9px 4px;
    color: var(--muted);
    font-size: var(--step--1);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .menu button {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 9px;
    border-radius: var(--radius-s);
    text-align: left;
  }
  .menu button:hover:not(:disabled) {
    background: var(--line);
  }
  .menu button:disabled {
    color: var(--faint);
    cursor: default;
  }
  .menu .danger {
    color: #e08a7f;
  }
  .menu hr {
    border: 0;
    border-top: 1px solid var(--line);
    margin: 5px 0;
  }
  .menu-note {
    margin: 2px 9px 4px;
    color: var(--faint);
    font-size: var(--step--1);
  }
  .person .avatar {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    object-fit: cover;
    background: var(--raised);
    flex: none;
  }
</style>
