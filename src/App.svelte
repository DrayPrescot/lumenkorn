<script lang="ts">
  import { isTyping } from "./lib/keys";
  import { keymap } from "./lib/keymap.svelte";
  import { onMount } from "svelte";
  import Grid from "./components/Grid.svelte";
  import InfoPanel from "./components/InfoPanel.svelte";
  import Loupe from "./components/Loupe.svelte";
  import Compare from "./components/Compare.svelte";
  import PeopleView from "./components/PeopleView.svelte";
  import Sidebar from "./components/Sidebar.svelte";
  import Toolbar from "./components/Toolbar.svelte";
  import DevelopView from "./components/DevelopView.svelte";
  import DevelopPanel from "./components/DevelopPanel.svelte";
  import ExportDialog from "./components/ExportDialog.svelte";
  import DeleteRejectedDialog from "./components/DeleteRejectedDialog.svelte";
  import RenameDialog from "./components/RenameDialog.svelte";
  import ContextMenu from "./components/ContextMenu.svelte";
  import MergeDialog from "./components/MergeDialog.svelte";
  import ImportDialog from "./components/ImportDialog.svelte";
  import UpdateDialog from "./components/UpdateDialog.svelte";
  import GpuPromptDialog from "./components/GpuPromptDialog.svelte";
  import CatalogTooNew from "./components/CatalogTooNew.svelte";
  import PresetDialog from "./components/PresetDialog.svelte";
  import AlbumDialog from "./components/AlbumDialog.svelte";
  import CullDialog from "./components/CullDialog.svelte";
  import AlbumView from "./components/AlbumView.svelte";
  import StatsView from "./components/StatsView.svelte";
  import { album } from "./lib/album.svelte";
  import { api, isTauri, type ImageSummary } from "./lib/api";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { dev } from "./lib/develop.svelte";
  import { lib } from "./lib/store.svelte";
  import { t } from "./lib/i18n";

  let sidebarOpen = $state(false); // nur schmale Bildschirme
  let infoOpen = $state(typeof window !== "undefined" ? window.innerWidth > 1100 : true);
  let exportIds = $state<number[] | null>(null);

  function openExport() {
    const ids = lib.targets();
    if (ids.length) exportIds = ids;
    else lib.notify(t("Wähle zuerst Fotos zum Exportieren aus."));
  }

  let renameIds = $state<number[] | null>(null);

  function openRename() {
    // Reihenfolge wie in der Ansicht (für die Nummerierung „nach Ansicht“)
    const want = new Set(lib.targets());
    const ids = lib.images.filter((i) => want.has(i.id)).map((i) => i.id);
    if (ids.length) renameIds = ids;
    else lib.notify(t("Wähle zuerst Fotos zum Umbenennen aus."));
  }

  let deleteImages = $state<ImageSummary[] | null>(null);

  function openDeleteRejected() {
    if (lib.rejectedCount) deleteImages = lib.rejectedList();
    else lib.notify(t("In dieser Ansicht sind keine Fotos abgelehnt ({key}).", { key: keymap.hint("reject") }));
  }

  // Beim Verlassen des Entwickeln-Modus offene Änderungen speichern.
  $effect(() => {
    if (lib.mode !== "develop") dev.close();
  });

  /** Katalog von einer neueren Version: nur das Update anbieten */
  let tooNew = $state<string | null>(null);

  // Beim Schließen des Fensters ausstehende Bearbeitung sichern. Tauri wartet
  // auf den Handler und zerstört das Fenster danach selbst (destroy).
  onMount(() => {
    if (!isTauri) return;
    let unlisten: (() => void) | undefined;
    let gone = false;
    getCurrentWindow()
      .onCloseRequested(async () => {
        try {
          await Promise.race([dev.flushForClose(), new Promise((r) => setTimeout(r, 5000))]);
        } catch (e) {
          console.error("Speichern beim Schließen fehlgeschlagen", e);
        }
      })
      .then((u) => (gone ? u() : (unlisten = u)))
      .catch((e) => console.error(e));
    return () => {
      gone = true;
      unlisten?.();
    };
  });

  onMount(async () => {
    const info = await api.appInfo().catch(() => null);
    if (info?.catalogTooNew) tooNew = info.catalogTooNew;
    else lib.init();
  });

  // Tastenkürzel wie in gängigen Fotoverwaltungen
  /** F11: echtes Vollbild (ohne Titelleiste und Taskleiste) ein/aus. Das
   *  Fenster startet maximiert. */
  async function toggleFullscreen() {
    if (isTauri) {
      const w = getCurrentWindow();
      await w.setFullscreen(!(await w.isFullscreen()));
    } else if (document.fullscreenElement) await document.exitFullscreen();
    else await document.documentElement.requestFullscreen().catch(() => {});
  }

  function onkeydown(e: KeyboardEvent) {
    if (keymap.is(e, "fullscreen")) {
      e.preventDefault();
      toggleFullscreen();
      return;
    }
    if (isTyping(e) || exportIds || renameIds || deleteImages || lib.merge || lib.importOpen) return;
    // Personen- und Albumansicht: Kein Foto ist zu sehen – Bewerten oder
    // Ablehnen träfe unbemerkt das zuletzt gewählte
    const noPhoto = lib.mode === "library" && (lib.view === "people" || lib.view === "album" || lib.view === "stats");
    // Befehl, Wirkung, nur mit sichtbarem Foto (Belegung: Einstellungen → Tastenkürzel)
    const actions: [string, () => void, boolean?][] = [
      ["import", () => (lib.importOpen = true)],
      ["export", openExport],
      ["rename", openRename],
      ["deleteRejected", openDeleteRejected],
      ["copySettings", () => lib.focused && dev.copyBasic(lib.focused.id)],
      ["pasteSettings", () => dev.pasteBasic(lib.pasteTargets())],
      ["hdr", () => lib.openMerge("hdr")],
      ["pano", () => lib.openMerge("pano")],
      ["focus", () => lib.openMerge("focus")],
      ["rejectNext", () => lib.rejectAndAdvance(), true],
      ...[0, 1, 2, 3, 4, 5].map((n) => [`rate${n}`, () => lib.rate(n), true] as [string, () => void, boolean]),
      ["pick", () => lib.flag(1), true],
      ["reject", () => lib.flag(-1), true],
      ["unflag", () => lib.flag(0), true],
      ["info", () => (infoOpen = !infoOpen)],
      [
        "grid",
        () => {
          lib.mode = "library";
          lib.view = "grid";
        },
      ],
      ["large", () => lib.focused && lib.openLarge(), true],
      // Vergleich auch aus dem Entwickeln (Auswahl im Filmstreifen)
      ["compare", () => lib.compare(), true],
      ["develop", () => lib.enterDevelop(), true],
      ["map", () => lib.openMap()],
    ];
    for (const [id, run, needsPhoto] of actions) {
      if (!keymap.is(e, id)) continue;
      if (needsPhoto && noPhoto) return;
      e.preventDefault();
      run();
      return;
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="app" class:info={(infoOpen && lib.view !== "album" && lib.view !== "stats") || lib.mode === "develop"}>
  <div class="sidebar" class:open={sidebarOpen}>
    <Sidebar />
  </div>
  {#if sidebarOpen}
    <button class="scrim" aria-label={t("Seitenleiste schließen")} onclick={() => (sidebarOpen = false)}></button>
  {/if}

  <main>
    <Toolbar
      ontogglesidebar={() => (sidebarOpen = !sidebarOpen)}
      ontoggleinfo={() => (infoOpen = !infoOpen)}
      ondeleterejected={openDeleteRejected}
    />
    {#if lib.mode === "develop" && lib.focused}
      <DevelopView />
    {:else if lib.view === "people" && lib.mode === "library"}
      <PeopleView />
    {:else if lib.view === "map" && lib.mode === "library" && lib.mapEnabled}
      <!-- Karte (mit Leaflet) erst beim Öffnen laden -->
      {#await import("./components/MapView.svelte") then m}
        <m.default />
      {/await}
    {:else if lib.view === "album" && lib.mode === "library" && album.state}
      <AlbumView />
    {:else if lib.view === "stats" && lib.mode === "library"}
      <StatsView />
    {:else if lib.view === "compare" && lib.compareItems.length >= 2}
      <Compare />
    {:else if lib.view === "loupe" && lib.focused}
      <Loupe />
    {:else}
      <Grid />
    {/if}
  </main>

  {#if lib.mode === "develop" && lib.focused}
    <div class="info-panel"><DevelopPanel onexport={openExport} /></div>
  {:else if infoOpen && lib.view !== "album" && lib.view !== "stats"}
    <div class="info-panel"><InfoPanel onexport={openExport} /></div>
  {/if}
</div>

{#if exportIds}
  <ExportDialog ids={exportIds} onclose={() => (exportIds = null)} />
{/if}

{#if renameIds}
  <RenameDialog ids={renameIds} onclose={() => (renameIds = null)} />
{/if}

{#if lib.contextMenu}
  <ContextMenu x={lib.contextMenu.x} y={lib.contextMenu.y} onclose={() => (lib.contextMenu = null)} onrename={openRename} />
{/if}

{#if album.dialog}
  <AlbumDialog scope={album.dialog.scope} title={album.dialog.title} onclose={() => (album.dialog = null)} />
{/if}

{#if lib.cull}
  <CullDialog scope={lib.cull.scope} title={lib.cull.title} onclose={() => (lib.cull = null)} />
{/if}

{#if lib.merge}
  <MergeDialog kind={lib.merge.kind} ids={lib.merge.ids} onclose={() => (lib.merge = null)} />
{/if}

{#if lib.importOpen}
  <ImportDialog onclose={() => (lib.importOpen = false)} />
{/if}

{#if dev.presetDialog}
  <PresetDialog />
{/if}

{#if tooNew}
  <CatalogTooNew message={tooNew} />
{:else}
  <UpdateDialog />
  <GpuPromptDialog />
{/if}

{#if deleteImages}
  <DeleteRejectedDialog images={deleteImages} onclose={() => (deleteImages = null)} />
{/if}

{#if dev.exportProgress && !dev.exportProgress.finished}
  <div class="toast" role="status">
    {t("Exportiere {done} von {total}", { done: dev.exportProgress.done + 1, total: dev.exportProgress.total })}{dev.exportProgress.current ? `: ${dev.exportProgress.current}` : ""}
  </div>
{:else if lib.importRunning && lib.importProgress && !lib.importOpen}
  <div class="toast" role="status">
    {t("Import: {done} von {total}", { done: lib.importProgress.done, total: lib.importProgress.total })}{lib.importProgress.current ? ` · ${lib.importProgress.current}` : ""}
  </div>
{:else if lib.mergeProgress && !lib.mergeProgress.finished && !lib.merge}
  <div class="toast" role="status">
    {t("Zusammenfügen: {stage}", { stage: lib.mergeProgress.stage })}{lib.mergeProgress.total > 1
      ? ` (${t("{done} von {total}", { done: lib.mergeProgress.done, total: lib.mergeProgress.total })})`
      : " …"}
  </div>
{:else if lib.message}
  <div class="toast" class:error={lib.message.kind === "error"} role="status">{lib.message.text}</div>
{/if}

<style>
  .app {
    display: grid;
    grid-template-columns: var(--sidebar) minmax(0, 1fr);
    height: 100%;
  }
  .app.info {
    grid-template-columns: var(--sidebar) minmax(0, 1fr) var(--info);
  }
  .sidebar,
  .info-panel {
    min-height: 0;
  }
  main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .scrim {
    display: none;
  }
  .toast {
    position: fixed;
    left: 50%;
    bottom: 20px;
    transform: translateX(-50%);
    z-index: 70;
    max-width: min(560px, calc(100vw - 32px));
    padding: 9px 14px;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    box-shadow: 0 10px 28px rgb(0 0 0 / 0.4);
  }
  .toast.error {
    border-color: var(--reject);
  }

  @media (max-width: 800px) {
    .app,
    .app.info {
      grid-template-columns: minmax(0, 1fr);
    }
    .sidebar {
      position: fixed;
      inset: 0 auto 0 0;
      width: min(var(--sidebar), 85vw);
      z-index: 30;
      transform: translateX(-100%);
      transition: transform 0.18s ease-out;
    }
    .sidebar.open {
      transform: none;
    }
    .scrim {
      display: block;
      position: fixed;
      inset: 0;
      z-index: 20;
      background: rgb(0 0 0 / 0.5);
    }
    .info-panel {
      position: fixed;
      inset: auto 0 0 0;
      max-height: 55vh;
      z-index: 15;
      border-top: 1px solid var(--line);
    }
  }
</style>
