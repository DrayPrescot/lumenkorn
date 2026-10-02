<script lang="ts">
  import { imageUrl } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import Icon from "./Icon.svelte";

  let strip: HTMLDivElement;

  // Ausschnitt um das aktuelle Bild – der Streifen muss nicht alle Fotos kennen.
  const items = $derived.by(() => {
    const from = Math.max(0, lib.focus - 30);
    return lib.images.slice(from, lib.focus + 31).map((im, i) => ({ im, index: from + i }));
  });

  $effect(() => {
    lib.focus;
    strip?.querySelector(".current")?.scrollIntoView({ inline: "center", block: "nearest" });
  });

  function onclick(e: MouseEvent, index: number) {
    lib.select(index, e.shiftKey ? "range" : e.ctrlKey || e.metaKey ? "toggle" : "single");
  }
</script>

<div class="strip scroll" bind:this={strip}>
  {#each items as { im, index } (im.id)}
    <button
      class:current={index === lib.focus}
      class:selected={lib.selection.has(im.id) && index !== lib.focus}
      class:rejected={im.flag === -1}
      onclick={(e) => onclick(e, index)}
      oncontextmenu={(e) => lib.openMenu(e, index)}
      aria-label={im.fileName}
      title={im.fileName}
    >
      <img src={imageUrl("thumb", im.id, lib.thumbVersion[im.id] ?? 0)} alt="" draggable="false" />
      {#if im.edited}<span class="badge"><Icon name="sliders" size={10} /></span>{/if}
    </button>
  {/each}
</div>

<style>
  .rejected img {
    opacity: 0.32;
  }
  .strip {
    flex: none;
    display: flex;
    gap: 4px;
    height: 76px;
    padding: 6px 8px;
    border-top: 1px solid var(--line);
    background: var(--panel);
    overflow-y: hidden;
  }
  button {
    position: relative;
    flex: none;
    height: 100%;
    border-radius: 2px;
    overflow: hidden;
    opacity: 0.6;
  }
  button:hover {
    opacity: 0.9;
  }
  button.selected {
    opacity: 0.9;
    outline: 1px solid var(--faint);
    outline-offset: -1px;
  }
  button.current {
    opacity: 1;
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  img {
    height: 100%;
    display: block;
  }
  .badge {
    position: absolute;
    right: 3px;
    bottom: 3px;
    display: grid;
    padding: 2px;
    border-radius: 3px;
    background: rgb(0 0 0 / 0.55);
    color: #fff;
  }
</style>
