<script lang="ts">
  // Vergleichsansicht: die ausgewählten Fotos groß nebeneinander, mit
  // gekoppeltem Zoom. Bewerten und Markieren wirkt auf das hervorgehobene Foto.
  import { isTyping } from "../lib/keys";
  import { imageUrl } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { ZoomState, zoomKey } from "../lib/zoom.svelte";
  import { libraryDims } from "../lib/layout";
  import Icon from "./Icon.svelte";
  import ZoomView from "./ZoomView.svelte";

  let linked = $state(true);
  const shared = new ZoomState();
  const own = new Map<number, ZoomState>();
  function zoomFor(id: number): ZoomState {
    if (linked) return shared;
    let z = own.get(id);
    if (!z) {
      z = new ZoomState();
      own.set(id, z);
    }
    return z;
  }

  const items = $derived(lib.compareItems);
  let w = $state(0);
  let h = $state(0);

  // Spalten so wählen, dass die Bilder (meist 3:2) möglichst groß werden
  const cols = $derived.by(() => {
    const n = items.length;
    if (!n || !w || !h) return Math.max(1, n);
    let best = 1;
    let bestSize = 0;
    for (let c = 1; c <= n; c++) {
      const r = Math.ceil(n / c);
      const cw = w / c;
      const ch = h / r;
      const size = Math.min(cw, ch * 1.5); // Breite eines 3:2-Bildes in der Zelle
      if (size > bestSize) {
        bestSize = size;
        best = c;
      }
    }
    return best;
  });

  function onkeydown(e: KeyboardEvent) {
    if (lib.view !== "compare" || isTyping(e)) return;
    const focusedZoom = lib.focused ? zoomFor(lib.focused.id) : shared;
    if (zoomKey(e, focusedZoom)) return;
    const pos = items.findIndex((x) => x.index === lib.focus);
    if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
      const d = e.key === "ArrowRight" ? 1 : -1;
      const next = items[(pos + d + items.length) % items.length];
      if (next) lib.focus = next.index;
    } else if (e.key === "Escape") lib.view = "grid";
    else if (e.key === "Enter") lib.view = "loupe";
    else if ((e.key === "Delete" || e.key === "Backspace") && !e.ctrlKey && !e.metaKey) {
      if (lib.focused) lib.removeFromCompare(lib.focused.id);
    } else return;
    e.preventDefault();
  }
</script>

<svelte:window {onkeydown} />

<div class="compare">
  <div class="head">
    <span class="muted">{items.length} Fotos im Vergleich · Klick hebt hervor, Bewertung wirkt auf das hervorgehobene Foto</span>
    <label class="check">
      <input type="checkbox" bind:checked={linked} onchange={() => { if (linked) shared.reset(); }} />
      Zoom koppeln
    </label>
  </div>
  <div class="cells" style="grid-template-columns: repeat({cols}, minmax(0, 1fr))" bind:clientWidth={w} bind:clientHeight={h}>
    {#each items as { im, index } (im.id)}
      {@const v = lib.thumbVersion[im.id] ?? 0}
      {@const dims = libraryDims(im)}
      <div class="cell" class:focus={index === lib.focus}>
        <ZoomView
          src={imageUrl("preview", im.id, v)}
          hiSrc={imageUrl("full", im.id, v)}
          fullLong={dims.fullLong}
          srcLong={dims.srcLong}
          zoom={zoomFor(im.id)}
          alt={im.fileName}
          onselect={() => (lib.focus = index)}
        />
        <div class="label">
          <span class="name" title={im.fileName}>{im.fileName}</span>
          <span class="stars" aria-label="{im.rating} Sterne">
            {#each [1, 2, 3, 4, 5] as n}
              <Icon name="star" size={11} fill={im.rating >= n} />
            {/each}
          </span>
          {#if im.flag === 1}<span class="flag pick">Auswahl</span>{/if}
          {#if im.flag === -1}<span class="flag reject">Abgelehnt</span>{/if}
          <button
            class="remove"
            title="Aus dem Vergleich nehmen (Entf)"
            aria-label="Aus dem Vergleich nehmen"
            onclick={() => lib.removeFromCompare(im.id)}
          >
            <Icon name="close" size={12} />
          </button>
        </div>
      </div>
    {/each}
  </div>
</div>

<style>
  .compare {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg);
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    padding: 6px 12px;
    font-size: var(--step--1);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--muted);
  }
  .check input {
    accent-color: var(--accent);
  }
  .cells {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-auto-rows: minmax(0, 1fr);
    gap: 6px;
    padding: 0 8px 8px;
  }
  .cell {
    position: relative;
    min-height: 0;
    border-radius: var(--radius-s);
    outline: 1px solid var(--line);
    overflow: hidden;
    background: rgb(0 0 0 / 0.15);
  }
  .cell.focus {
    outline: 2px solid var(--accent);
  }
  .label {
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 6px 4px 8px;
    background: linear-gradient(rgb(0 0 0 / 0.6), transparent);
    font-size: var(--step--1);
    pointer-events: none;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .stars {
    display: flex;
    color: var(--accent);
    flex: none;
  }
  .flag {
    flex: none;
    font-size: 10px;
    padding: 0 5px;
    border-radius: 3px;
  }
  .flag.pick {
    background: var(--accent);
    color: #111;
  }
  .flag.reject {
    background: var(--reject);
    color: #fff;
  }
  .remove {
    margin-left: auto;
    pointer-events: auto;
    display: grid;
    padding: 3px;
    border-radius: 3px;
    color: var(--muted);
  }
  .remove:hover {
    color: var(--text);
    background: rgb(0 0 0 / 0.5);
  }
</style>
