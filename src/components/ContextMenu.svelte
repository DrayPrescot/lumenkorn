<script lang="ts">
  // Rechtsklick-Menü auf Fotos (Raster, Filmstreifen, Einzelansicht, Entwickeln):
  // Basis-Einstellungen kopieren und einfügen, Presets anwenden, Fotos zusammenfügen.
  import { dev } from "../lib/develop.svelte";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";
  import type { Preset, TransferGroups } from "../lib/api";

  let { x, y, onclose }: { x: number; y: number; onclose: () => void } = $props();

  const targets = $derived(lib.pasteTargets());
  const many = $derived(lib.selection.size >= 2);
  const busy = $derived(!!lib.mergeProgress && !lib.mergeProgress.finished);
  let el = $state<HTMLDivElement>();
  let presetsOpen = $state(false);
  let presetItem = $state<HTMLDivElement>();
  $effect(() => {
    dev.loadPresets();
    // Menü zu: eine Vorschau im geöffneten Foto zurücknehmen
    return () => dev.previewPreset(null);
  });
  // Vorschau beim Überfahren nur, wenn das Preset auf das geöffnete Foto wirken würde
  const canPreview = $derived(lib.mode === "develop" && targets.length === 1 && targets[0] === dev.imageId);
  const GROUP_NAMES: [keyof TransferGroups, string][] = [
    ["whiteBalance", t("Weißabgleich")], ["light", t("Licht")], ["color", t("Farbe")], ["hsl", "HSL"],
    ["colorGrade", "Color Grading"], ["details", t("Details")], ["masks", t("Masken")], ["geometry", t("Zuschnitt und Drehen")],
  ];
  const summary = (p: Preset) => GROUP_NAMES.filter(([k]) => p.groups[k]).map(([, n]) => n).join(", ");
  // Untermenü rechts daneben, links, wenn rechts kein Platz ist
  const subLeft = $derived.by(() => {
    if (!presetsOpen || !presetItem) return false;
    return presetItem.getBoundingClientRect().right + 260 > window.innerWidth;
  });

  // Im Fenster halten
  const pos = $derived.by(() => {
    const w = el?.offsetWidth ?? 260;
    const h = el?.offsetHeight ?? 220;
    return {
      left: Math.min(x, window.innerWidth - w - 8),
      top: Math.min(y, window.innerHeight - h - 8),
    };
  });

  function run(f: () => void) {
    onclose();
    f();
  }
</script>

<svelte:window
  onpointerdown={(e) => { if (!el?.contains(e.target as Node)) onclose(); }}
  onkeydowncapture={(e) => { if (e.key === "Escape") { e.stopPropagation(); onclose(); } }}
  onblur={onclose}
  onwheel={onclose}
/>

<div bind:this={el} class="menu" role="menu" style:left="{pos.left}px" style:top="{pos.top}px">
  <button
    role="menuitem"
    disabled={!lib.focused}
    onclick={() => run(() => lib.focused && dev.copyBasic(lib.focused.id))}
  >
    <span>{t("Basis-Einstellungen kopieren")}</span>
    <kbd>{t("Strg+Umschalt+C")}</kbd>
  </button>
  <button role="menuitem" disabled={!dev.clipboard || !targets.length} onclick={() => run(() => dev.pasteBasic(targets))}>
    <span>
      {targets.length > 1 ? t("Basis-Einstellungen einfügen ({n} Fotos)", { n: targets.length }) : t("Basis-Einstellungen einfügen")}
      {#if dev.clipboard}<small>{t("von {name}", { name: dev.clipboard.name })}</small>{/if}
    </span>
    <kbd>{t("Strg+Umschalt+V")}</kbd>
  </button>
  <div
    class="sub"
    bind:this={presetItem}
    role="none"
    onmouseenter={() => (presetsOpen = true)}
    onmouseleave={() => {
      presetsOpen = false;
      dev.previewPreset(null);
    }}
  >
    <button
      role="menuitem"
      aria-haspopup="menu"
      aria-expanded={presetsOpen}
      disabled={!targets.length && !lib.focused}
      onclick={() => (presetsOpen = !presetsOpen)}
    >
      <span>{targets.length > 1 ? t("Preset anwenden ({n} Fotos)", { n: targets.length }) : t("Preset anwenden")}</span>
      <kbd>▸</kbd>
    </button>
    {#if presetsOpen}
      <div class="menu flyout" class:left={subLeft} role="menu" tabindex="-1" onmouseleave={() => dev.previewPreset(null)}>
        {#each dev.presets as p (p.id)}
          <button
            role="menuitem"
            disabled={!targets.length}
            onmouseenter={() => canPreview && dev.previewPreset(p)}
            onclick={() => run(() => dev.applyPreset(p, targets))}
            title={summary(p)}
          >
            <span>{p.name}</span>
          </button>
        {:else}
          <div class="label empty">{t("Noch keine Presets")}</div>
        {/each}
        <hr />
        <button
          role="menuitem"
          disabled={!lib.focused}
          onclick={() => run(() => lib.focused && dev.openCreatePreset(lib.focused.id, lib.focused.fileName))}
        >
          <span>{t("Neues Preset aus diesem Foto …")}</span>
        </button>
        <button role="menuitem" disabled={!dev.presets.length} onclick={() => run(() => (dev.presetDialog = { mode: "manage" }))}>
          <span>{t("Presets verwalten …")}</span>
        </button>
      </div>
    {/if}
  </div>
  <hr />
  <button role="menuitem" disabled={!many} onclick={() => run(() => lib.compare())}>
    <span>{many ? t("Groß nebeneinander vergleichen ({n} Fotos)", { n: Math.min(lib.selection.size, 12) }) : t("Groß nebeneinander vergleichen")}</span>
    <kbd>C</kbd>
  </button>
  <hr />
  <div class="label">{many ? t("Zusammenfügen ({n} Fotos)", { n: lib.selection.size }) : t("Zusammenfügen")}</div>
  <button role="menuitem" disabled={!many || busy} onclick={() => run(() => lib.openMerge("hdr"))}>
    <span>{t("HDR aus Belichtungsreihe …")}</span>
    <kbd>{t("Strg+H")}</kbd>
  </button>
  <button role="menuitem" disabled={!many || busy} onclick={() => run(() => lib.openMerge("pano"))}>
    <span>{t("Panorama …")}</span>
    <kbd>{t("Strg+M")}</kbd>
  </button>
  <button role="menuitem" disabled={!many || busy} onclick={() => run(() => lib.openMerge("focus"))}>
    <span>{t("Focus Stacking …")}</span>
    <kbd>{t("Strg+Umschalt+F")}</kbd>
  </button>
</div>

<style>
  .menu {
    position: fixed;
    z-index: 80;
    min-width: 260px;
    padding: 4px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.45);
    display: grid;
  }
  button {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
    padding: 7px 10px;
    border-radius: var(--radius-s);
    text-align: left;
  }
  button:hover:not(:disabled) {
    background: var(--raised);
  }
  button:disabled {
    opacity: 0.45;
    cursor: default;
  }
  span {
    display: grid;
  }
  hr {
    border: 0;
    border-top: 1px solid var(--line);
    margin: 4px 2px;
    width: calc(100% - 4px);
  }
  .sub {
    position: relative;
    display: grid;
  }
  .flyout {
    position: absolute;
    top: -5px;
    left: 100%;
    min-width: 250px;
    max-height: 60vh;
    overflow-y: auto;
    margin-left: 2px;
  }
  .flyout.left {
    left: auto;
    right: 100%;
    margin: 0 2px 0 0;
  }
  .label.empty {
    padding: 6px 10px;
  }
  .label {
    padding: 4px 10px 2px;
    color: var(--faint);
    font-size: var(--step--1);
  }
  small {
    color: var(--faint);
    font-size: var(--step--1);
  }
  kbd {
    color: var(--faint);
    font-family: inherit;
    font-size: var(--step--1);
    white-space: nowrap;
  }
</style>
