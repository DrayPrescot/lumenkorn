<script lang="ts">
  // Rechtsklick-Menü auf Fotos (Raster, Filmstreifen, Einzelansicht, Entwickeln):
  // Basis-Einstellungen kopieren und einfügen, Fotos zusammenfügen.
  import { dev } from "../lib/develop.svelte";
  import { lib } from "../lib/store.svelte";

  let { x, y, onclose }: { x: number; y: number; onclose: () => void } = $props();

  const targets = $derived(lib.pasteTargets());
  const many = $derived(lib.selection.size >= 2);
  const busy = $derived(!!lib.mergeProgress && !lib.mergeProgress.finished);
  let el = $state<HTMLDivElement>();

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
    <span>Basis-Einstellungen kopieren</span>
    <kbd>Strg+Umschalt+C</kbd>
  </button>
  <button role="menuitem" disabled={!dev.clipboard || !targets.length} onclick={() => run(() => dev.pasteBasic(targets))}>
    <span>
      Basis-Einstellungen einfügen{targets.length > 1 ? ` (${targets.length} Fotos)` : ""}
      {#if dev.clipboard}<small>von {dev.clipboard.name}</small>{/if}
    </span>
    <kbd>Strg+Umschalt+V</kbd>
  </button>
  <hr />
  <button role="menuitem" disabled={!many} onclick={() => run(() => lib.compare())}>
    <span>Groß nebeneinander vergleichen{many ? ` (${Math.min(lib.selection.size, 12)} Fotos)` : ""}</span>
    <kbd>C</kbd>
  </button>
  <hr />
  <div class="label">Zusammenfügen{many ? ` (${lib.selection.size} Fotos)` : ""}</div>
  <button role="menuitem" disabled={!many || busy} onclick={() => run(() => lib.openMerge("hdr"))}>
    <span>HDR aus Belichtungsreihe …</span>
    <kbd>Strg+H</kbd>
  </button>
  <button role="menuitem" disabled={!many || busy} onclick={() => run(() => lib.openMerge("pano"))}>
    <span>Panorama …</span>
    <kbd>Strg+M</kbd>
  </button>
  <button role="menuitem" disabled={!many || busy} onclick={() => run(() => lib.openMerge("focus"))}>
    <span>Focus Stacking …</span>
    <kbd>Strg+Umschalt+F</kbd>
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
