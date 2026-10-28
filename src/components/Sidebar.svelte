<script lang="ts">
  import { onMount } from "svelte";
  import { api, faceUrl, isTauri, pickFolder, type FolderNode, type Person, type Project, type Root } from "../lib/api";
  import { lib, topFolders } from "../lib/store.svelte";
  import { t, tc } from "../lib/i18n";
  import { withKey } from "../lib/keymap.svelte";
  import FolderTree from "./FolderTree.svelte";
  import Icon from "./Icon.svelte";
  import PathDialog from "./PathDialog.svelte";
  import SettingsDialog, { type SettingsPage } from "./SettingsDialog.svelte";
  import NextcloudDialog from "./NextcloudDialog.svelte";
  import AboutDialog from "./AboutDialog.svelte";

  let platform = $state("");
  let settingsOpen = $state<SettingsPage | null>(null);
  let aboutOpen = $state(false);
  // Nach einem Absturz beim letzten Start: Dialog mit Erklärung öffnen
  let crashShown = false;
  $effect(() => {
    if (lib.ai?.crashNotice && !crashShown) {
      crashShown = true;
      settingsOpen = "ai";
    }
  });
  let pathDialog = $state(false);
  let newProject = $state<string | null>(null);
  // `paths`: mehrere gewählte Ordner (Strg-/Umschalt-Klick), sonst nur `node`
  let menu = $state<{ x: number; y: number; node: FolderNode; root?: Root; paths: string[] } | null>(null);
  /** Ordner im Kontextmenü: bezieht er seine Unterordner ein? */
  const menuSubfolders = $derived(menu && menu.paths.length <= 1 ? lib.subfoldersIncluded(menu.node.path) : true);

  function toggleSubfolders() {
    if (!menu) return;
    const path = menu.node.path;
    const on = menuSubfolders;
    menu = null;
    lib.setFolderSubfolders(path, !on);
  }

  /** Kontextmenü eines Projekts und offener Nextcloud-Dialog */
  let projMenu = $state<{ x: number; y: number; project: Project } | null>(null);
  let ncProject = $state<Project | null>(null);

  async function uploadProject(p: Project) {
    projMenu = null;
    const st = await api.ncStatus().catch(() => null);
    if (st) ncProject = p;
    else {
      lib.notify(t("Zuerst mit Nextcloud verbinden"));
      settingsOpen = "nextcloud";
    }
  }
  let menuEl = $state<HTMLDivElement | undefined>();
  // Im Fenster halten: nach dem Öffnen ausmessen (die Höhe hängt von der
  // Zahl der Projekte ab), unten zu wenig Platz → nach oben schieben
  const menuPos = $derived.by(() => {
    if (!menu) return { left: 0, top: 0 };
    const w = menuEl?.offsetWidth ?? 260;
    const h = menuEl?.offsetHeight ?? 280;
    return {
      left: Math.max(8, Math.min(menu.x, window.innerWidth - w - 8)),
      top: Math.max(8, Math.min(menu.y, window.innerHeight - h - 8)),
    };
  });
  let renaming = $state<number | null>(null);
  let renamingPerson = $state<number | null>(null);

  // Personen nach Gruppen (Familie, Freunde …), benannte ohne Gruppe unter
  // „Sonstige“; Personen lassen sich auf eine Gruppe ziehen, auf „Sonstige“
  // gezogen verlassen sie ihre Gruppe
  const namedPeople = $derived(lib.people.filter((p) => p.name));
  const groupedAny = $derived(namedPeople.some((p) => p.groupId !== null && lib.personGroups.some((g) => g.id === p.groupId)));
  const ungroupedPeople = $derived(namedPeople.filter((p) => p.groupId === null || !lib.personGroups.some((g) => g.id === p.groupId)));
  let groupDrop = $state<number | null | undefined>(undefined);
  /** Eine Person wird gerade gezogen (zeigt „Sonstige“ als Ablage, auch wenn leer) */
  let draggingPerson = $state(false);
  function personDragOver(e: DragEvent, group: number | null) {
    if (!e.dataTransfer?.types.includes("application/x-lumenkorn-person")) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    groupDrop = group;
  }
  function personDrop(e: DragEvent, group: number | null) {
    groupDrop = undefined;
    draggingPerson = false;
    const id = Number(e.dataTransfer?.getData("application/x-lumenkorn-person"));
    const p = lib.people.find((x) => x.id === id);
    if (!p) return;
    e.preventDefault();
    if (p.groupId !== group) lib.groupAction(() => api.personSetGroup([p.id], group));
  }

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
    menu = { x, y, node, root, paths };
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

{#snippet personItem(p: Person)}
  {#if renamingPerson === p.id}
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
            draggable="true"
            ondragstart={(e) => {
              e.dataTransfer?.setData("application/x-lumenkorn-person", String(p.id));
              if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
              draggingPerson = true;
            }}
            ondragend={() => {
              groupDrop = undefined;
              draggingPerson = false;
            }}
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
{/snippet}

{#snippet toggle(key: "projects" | "people" | "folders", title: string, count: number)}
  {@const closed = !!lib.collapsed[key]}
  <button class="toggle" aria-expanded={!closed} title={closed ? t("{title} aufklappen", { title }) : t("{title} einklappen", { title })} onclick={() => lib.toggleSection(key)}>
    <span class="chev" class:closed><Icon name="chevron" size={11} /></span>
    <h2>{title}</h2>
    {#if closed && count}<span class="count">{count}</span>{/if}
  </button>
{/snippet}

<svelte:window
  onclick={() => {
    menu = null;
    projMenu = null;
  }}
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
            onclick={(e) => {
              // Klick auf die Wolke: Nextcloud-Dialog mit dem Link
              if ((e.target as Element).closest(".shared")) uploadProject(p);
              else lib.setScope({ type: "project", id: p.id });
            }}
            ondblclick={() => (renaming = p.id)}
            oncontextmenu={(e) => {
              e.preventDefault();
              e.stopPropagation();
              menu = null;
              projMenu = { x: e.clientX, y: e.clientY, project: p };
            }}
            title={t("{n} Ordner. Doppelklick zum Umbenennen.", { n: String(p.folders.length) })}
          >
            <Icon name="stack" size={15} />
            <span>{p.name}</span>
            {#if lib.ncShared[p.id]}
              <span class="shared" title={t("In Nextcloud geteilt: {url}", { url: lib.ncShared[p.id] })}><Icon name="cloud" size={14} /></span>
            {/if}
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
      {#each lib.personGroups as g (g.id)}
        {@const members = namedPeople.filter((p) => p.groupId === g.id)}
        {#if members.length}
          {@const closed = !!lib.collapsed[`sgroup:${g.id}`]}
          {#if !lib.collapsed.people}
            <button
              class="item pgroup"
              class:drop={groupDrop === g.id}
              aria-expanded={!closed}
              title={closed ? t("{title} aufklappen", { title: g.name }) : t("{title} einklappen", { title: g.name })}
              onclick={() => lib.toggleSection(`sgroup:${g.id}`)}
              ondragover={(e) => personDragOver(e, g.id)}
              ondragleave={() => groupDrop === g.id && (groupDrop = undefined)}
              ondrop={(e) => personDrop(e, g.id)}
            >
              <span class="chev" class:closed><Icon name="chevron" size={11} /></span>
              <span class="gname">{g.name}</span>
              <span class="count">{members.length}</span>
            </button>
          {/if}
          {#each members as p (p.id)}
            {#if (!closed && !lib.collapsed.people) || lib.isScope({ type: "person", id: p.id })}
              <div class="in-group">{@render personItem(p)}</div>
            {/if}
          {/each}
        {/if}
      {/each}
      {#if ungroupedPeople.length || (draggingPerson && groupedAny)}
        {@const closed = !!lib.collapsed["sgroup:other"]}
        {#if !lib.collapsed.people}
          <button
            class="item pgroup"
            class:drop={groupDrop === null}
            aria-expanded={!closed}
            title={closed ? t("{title} aufklappen", { title: t("Sonstige") }) : t("{title} einklappen", { title: t("Sonstige") })}
            onclick={() => lib.toggleSection("sgroup:other")}
            ondragover={(e) => personDragOver(e, null)}
            ondragleave={() => groupDrop === null && (groupDrop = undefined)}
            ondrop={(e) => personDrop(e, null)}
          >
            <span class="chev" class:closed><Icon name="chevron" size={11} /></span>
            <span class="gname">{t("Sonstige")}</span>
            <span class="count">{ungroupedPeople.length}</span>
          </button>
        {/if}
        {#each ungroupedPeople as p (p.id)}
          {#if (!closed && !lib.collapsed.people) || lib.isScope({ type: "person", id: p.id })}
            <div class="in-group">{@render personItem(p)}</div>
          {/if}
        {/each}
      {/if}
      {#if lib.collapsed.people}
        <!-- eingeklappt -->
      {:else if lib.peopleStatus?.ready}
        {@const unnamed = lib.people.filter((p) => !p.name && p.faceCount >= 2).length}
        <button class="item" class:active={lib.view === "people" && lib.mode === "library"} onclick={() => lib.openPeople()}>
          <Icon name="sparkle" size={15} />
          <span>{unnamed ? t(unnamed === 1 ? "{n} unbenannte Gruppe" : "{n} unbenannte Gruppen", { n: String(unnamed) }) : t("Personen verwalten")}</span>
        </button>
      {:else}
        <p class="hint">{t("Erkennt Gesichter und gruppiert gleiche Personen – einrichten über „KI“ unten links.")}</p>
      {/if}
    </section>

    <section>
      <header>
        {@render toggle("folders", t("Ordner"), lib.roots.length)}
        <button class="icon-btn" aria-label={t("Fotos importieren")} title={withKey(t("Fotos importieren"), "import")} onclick={() => (lib.importOpen = true)}>
          <Icon name="import" size={14} />
        </button>
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
      <button class="ai" onclick={() => (settingsOpen = "general")} title={t("Einstellungen: Sprache, Ordner, KI, Grafikkarte, Tastenkürzel …")}>
        <Icon name="settings" size={13} />
        <span>{t("Einstellungen")}</span>
        <!-- KI-Fortschritt klein daneben, solange etwas läuft -->
        {#if lib.aiDownload}
          <span class="status">{t("KI lädt …")}</span>
        {:else if lib.ai?.modelReady && lib.ai.analyzed < lib.ai.total}
          <span class="status" title={t("KI: {done} von {total} analysiert", { done: String(lib.ai.analyzed), total: String(lib.ai.total) })}>{t("KI {done}/{total}", { done: String(lib.ai.analyzed), total: String(lib.ai.total) })}</span>
        {/if}
      </button>
      <button class="icon-btn about" aria-label={t("Über Lumenkorn")} title={t("Über Lumenkorn")} onclick={() => (aboutOpen = true)}>
        <Icon name="info" size={14} />
      </button>
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
  <div class="menu" role="menu" tabindex="-1" bind:this={menuEl} style:left="{menuPos.left}px" style:top="{menuPos.top}px" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
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
    {:else}
      <hr />
      <button role="menuitemcheckbox" aria-checked={menuSubfolders} onclick={toggleSubfolders}>
        <span class="check-mark">{menuSubfolders ? "✓" : ""}</span> {t("Unterordner einbeziehen")}
      </button>
    {/if}
    {#if !many && m.root}
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

{#if projMenu}
  {@const pm = projMenu}
  <div class="menu" role="menu" tabindex="-1" style:left="{pm.x}px" style:top="{pm.y}px" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
    <div class="menu-title">{pm.project.name}</div>
    <button role="menuitem" onclick={() => uploadProject(pm.project)}>
      <Icon name="cloud" size={14} /> {t("Zu Nextcloud hochladen …")}
    </button>
    <button role="menuitem" onclick={() => { projMenu = null; renaming = pm.project.id; }}>
      <Icon name="stack" size={14} /> {t("Umbenennen")}
    </button>
  </div>
{/if}

{#if ncProject}
  <NextcloudDialog project={ncProject} onclose={() => (ncProject = null)} />
{/if}

{#if settingsOpen}
  <SettingsDialog page={settingsOpen} onclose={() => (settingsOpen = null)} />
{/if}
{#if aboutOpen}
  <AboutDialog onclose={() => (aboutOpen = false)} />
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
  .item.pgroup {
    height: 26px;
    gap: 5px;
    color: var(--muted);
    font-size: var(--step--1);
    font-weight: 600;
  }
  .item.pgroup:hover {
    color: var(--text);
  }
  .item.pgroup .chev {
    flex: none;
  }
  .item.pgroup .gname {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .item.pgroup.drop {
    outline: 1px dashed var(--accent);
    outline-offset: -1px;
  }

  .in-group {
    padding-left: 12px;
  }
  .count {
    color: var(--faint);
    font-size: var(--step--1);
  }
  .check-mark {
    display: inline-block;
    width: 14px;
    text-align: center;
  }
  .shared {
    display: inline-flex;
    cursor: pointer;
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
  .about {
    flex: none;
    color: var(--faint);
  }
  .about:hover {
    color: var(--text);
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
  /* KI-Fortschritt neben „Einstellungen“ */
  .ai .status {
    margin-left: auto;
    font-size: 11px;
    color: var(--faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
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
    /* Sehr viele Projekte: im Menü scrollen statt aus dem Fenster laufen */
    max-height: calc(100vh - 16px);
    overflow-y: auto;
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
