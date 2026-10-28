<script lang="ts">
  import { keymap } from "../lib/keymap.svelte";
  import { isTyping } from "../lib/keys";
  import { imageUrl } from "../lib/api";
  import { dev } from "../lib/develop.svelte";
  import { lib } from "../lib/store.svelte";
  import Filmstrip from "./Filmstrip.svelte";
  import MaskOverlay from "./MaskOverlay.svelte";
  import HslPicker from "./HslPicker.svelte";
  import CropOverlay from "./CropOverlay.svelte";
  import ZoomView from "./ZoomView.svelte";
  import { ZoomState, zoomKey } from "../lib/zoom.svelte";
  import { t } from "../lib/i18n";

  // Zoom bleibt beim Wechsel zum nächsten Bild erhalten (Serien vergleichen)
  const zoom = new ZoomState();
  // Zuschneiden und Masken bearbeitet man in der Einpassen-Ansicht
  $effect(() => {
    if (dev.cropMode) zoom.reset();
  });

  let stageW = $state(0);
  let stageH = $state(0);
  let imgW = $state(0);
  let imgH = $state(0);

  // Vorschau in Bühnengröße × Pixeldichte rendern (in 256er-Stufen, damit der
  // Proxy-Cache greift). Größer als der Proxy (2048) wird nie gerendert.
  $effect(() => {
    const dpr = window.devicePixelRatio || 1;
    const edge = Math.max(stageW, stageH) * dpr;
    const stepped = Math.min(2048, Math.max(512, Math.ceil(edge / 256) * 256));
    if (stepped !== dev.maxEdge && stageW > 0) {
      dev.maxEdge = stepped;
      if (dev.imageId !== null && !dev.loading) {
        dev.schedule();
        dev.renderBefore();
      }
    }
  });

  // Fokuswechsel (Filmstreifen, Pfeiltasten) öffnet das jeweilige Bild.
  $effect(() => {
    const f = lib.focused;
    if (f) dev.open(f.id);
  });

  const before = $derived(dev.showBefore || dev.holdBefore);
  const shown = $derived(before ? (dev.beforeUrl ?? dev.url) : dev.url);
  const fallback = $derived(lib.focused ? imageUrl("preview", lib.focused.id, lib.thumbVersion(lib.focused.id)) : null);

  const fullLong = $derived(Math.max(dev.fullSize[0], dev.fullSize[1]) || null);
  const srcLong = $derived(fullLong ? Math.min(dev.maxEdge, fullLong) : null);
  // Auch die Vorher-Ansicht in voller Auflösung, sonst wirkt der Vergleich
  // schärfer, als er ist
  const detail = $derived.by(() => {
    if (dev.showOverlay) return null;
    const b = before;
    return (r: { x: number; y: number; w: number; h: number }, scale: number) => dev.detailUrl(r, scale, b);
  });

  /** Enter: Zuschnitt übernehmen bzw. den Fokus lösen, damit die Pfeiltasten
   *  wieder im Filmstreifen blättern statt einen Regler zu verstellen. Nicht
   *  in Dialogen und echten Textfeldern (Maskenname …); die Wertfelder der
   *  Regler übernehmen ihren Wert selbst und lassen Enter durch. */
  function enter(e: KeyboardEvent): boolean {
    if (e.key !== "Enter" || e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return false;
    const el = e.target as HTMLElement | null;
    if (el?.closest?.("dialog, [role='dialog'], [aria-modal='true']")) return false;
    if (isTyping(e) && !el?.closest?.(".slider")) return false;
    e.preventDefault();
    (document.activeElement as HTMLElement | null)?.blur?.();
    if (dev.cropMode) dev.applyCrop();
    return true;
  }

  function onkeydown(e: KeyboardEvent) {
    if (enter(e)) return;
    if (isTyping(e)) return;
    if (!dev.cropMode && zoomKey(e, zoom)) return;
    // Pfeile, Enter und Escape sind fest; der Rest folgt der Belegung
    // (Einstellungen → Tastenkürzel)
    if (e.key === "ArrowRight") lib.move(1);
    else if (e.key === "ArrowLeft") lib.move(-1);
    else if (keymap.is(e, "before")) dev.showBefore = !dev.showBefore;
    else if (keymap.is(e, "maskOverlay") && dev.activeMask !== null) dev.toggleOverlay();
    else if (keymap.is(e, "bw")) dev.setMonochrome(!dev.params.monochrome);
    else if (keymap.is(e, "crop")) dev.cropMode ? dev.applyCrop() : dev.enterCrop();
    else if (e.key === "Escape" && dev.lineMode) dev.endLines();
    else if (e.key === "Escape" && dev.cropMode) dev.cancelCrop();
    else if (e.key === "Escape" && (dev.tool || dev.activeMask !== null)) {
      if (dev.tool) dev.tool = null;
      else dev.selectMask(null);
    }
    else if (keymap.is(e, "rotateLeft")) dev.rotate(-1);
    else if (keymap.is(e, "rotateRight")) dev.rotate(1);
    else if (keymap.is(e, "redo")) dev.redo();
    else if (keymap.is(e, "undo")) dev.undo();
    else if (keymap.is(e, "auto")) dev.autoTone();
    else return;
    e.preventDefault();
  }
</script>

<svelte:window {onkeydown} />

<div class="develop">
  <div class="stage" bind:clientWidth={stageW} bind:clientHeight={stageH} role="presentation" oncontextmenu={(e) => lib.openMenu(e)}>
    <!-- Bis die eigene Entwicklung fertig ist: Vorschau aus der Bibliothek -->
    <ZoomView
      src={shown ?? fallback}
      dim={!shown}
      {zoom}
      {fullLong}
      srcLong={shown ? srcLong : null}
      {detail}
      detailKey={shown}
      alt={lib.focused?.fileName ?? ""}
      showBadge={!dev.cropMode}
    >
      {#snippet children(view)}
        {#if shown}
          <!-- Maße der Vorschau für die Überlagerungen -->
          <img class="measure" src={shown} alt="" bind:naturalWidth={imgW} bind:naturalHeight={imgH} />
        {/if}
        {#if dev.cropMode && !dev.loading}
          <CropOverlay {stageW} {stageH} {imgW} {imgH} {view} />
        {:else if dev.hslPick && dev.tab === "basic" && !before && !dev.loading}
          <HslPicker {stageW} {stageH} {imgW} {imgH} {view} />
        {:else if !before && !dev.loading}
          <MaskOverlay {stageW} {stageH} {imgW} {imgH} {view} />
        {/if}
      {/snippet}
    </ZoomView>

    {#if dev.maskBusy}
      <div class="status">{dev.maskBusy}</div>
    {/if}

    {#if dev.loading}
      <div class="status">{dev.raw ? t("RAW wird entwickelt …") : t("Wird geladen …")}</div>
    {/if}

    {#if before}
      <div class="tag">{t("Vorher")}</div>
    {/if}

  </div>
  <Filmstrip />
</div>

<style>
  .develop {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg);
  }
  .stage {
    position: relative;
    flex: 1;
    min-height: 0;
    margin: 16px;
  }
  .measure {
    position: absolute;
    width: 1px;
    height: 1px;
    opacity: 0;
    pointer-events: none;
  }
  .status,
  .tag {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    padding: 6px 12px;
    border-radius: var(--radius-m);
    background: rgb(20 20 22 / 0.8);
    color: var(--text);
    font-size: var(--step--1);
  }
  .status {
    top: 50%;
  }
  .tag {
    top: 8px;
  }
</style>
