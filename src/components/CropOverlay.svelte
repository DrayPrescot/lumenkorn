<script lang="ts">
  // Zuschnittrahmen über dem ungeschnittenen Bild: Ecken und Kanten ziehen,
  // innen verschieben, optional mit festem Seitenverhältnis. Drittel-Raster
  // hilft beim Bildaufbau.
  import { dev } from "../lib/develop.svelte";
  import { containRect, frameSize } from "../lib/frame";
  import { t } from "../lib/i18n";

  let { stageW, stageH, imgW, imgH }: { stageW: number; stageH: number; imgW: number; imgH: number } = $props();

  const rect = $derived(containRect(stageW, stageH, imgW, imgH));

  const c = $derived(dev.params.crop ?? { x: 0, y: 0, w: 1, h: 1 });
  const frame = $derived(frameSize(dev.params, [dev.fullSize[0] || 1, dev.fullSize[1] || 1]));
  const box = $derived({ x: rect.x + c.x * rect.w, y: rect.y + c.y * rect.h, w: c.w * rect.w, h: c.h * rect.h });

  type Handle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w" | "move";
  let svg: SVGSVGElement;
  let drag: { handle: Handle; start: [number, number]; orig: { x: number; y: number; w: number; h: number } } | null = null;
  const MIN = 0.03;

  function norm(e: PointerEvent): [number, number] {
    const r = svg.getBoundingClientRect();
    return [(e.clientX - r.left - rect.x) / rect.w, (e.clientY - r.top - rect.y) / rect.h];
  }

  function start(e: PointerEvent, handle: Handle) {
    e.stopPropagation();
    e.preventDefault();
    drag = { handle, start: norm(e), orig: { ...c } };
    svg.setPointerCapture(e.pointerId);
  }

  function onmove(e: PointerEvent) {
    if (!drag) return;
    const [px, py] = norm(e);
    const dx = px - drag.start[0];
    const dy = py - drag.start[1];
    const o = drag.orig;
    let { x, y, w, h } = o;
    const hnd = drag.handle;

    if (hnd === "move") {
      x = Math.min(Math.max(0, o.x + dx), 1 - o.w);
      y = Math.min(Math.max(0, o.y + dy), 1 - o.h);
    } else {
      let left = o.x;
      let top = o.y;
      let right = o.x + o.w;
      let bottom = o.y + o.h;
      if (hnd.includes("w")) left = Math.min(Math.max(0, o.x + dx), right - MIN);
      if (hnd.includes("e")) right = Math.max(Math.min(1, o.x + o.w + dx), left + MIN);
      if (hnd.includes("n")) top = Math.min(Math.max(0, o.y + dy), bottom - MIN);
      if (hnd.includes("s")) bottom = Math.max(Math.min(1, o.y + o.h + dy), top + MIN);
      x = left;
      y = top;
      w = right - left;
      h = bottom - top;

      const a = dev.cropAspect;
      if (a && hnd.length === 2) {
        // Höhe aus der Breite ableiten, gegenüberliegende Ecke bleibt fest
        let nh = (w * frame[0]) / (frame[1] * a);
        const maxH = hnd.includes("n") ? o.y + o.h : 1 - o.y;
        if (nh > maxH) {
          nh = maxH;
          w = (nh * frame[1] * a) / frame[0];
          if (hnd.includes("w")) x = o.x + o.w - w;
        }
        h = nh;
        y = hnd.includes("n") ? o.y + o.h - h : o.y;
      }
    }
    dev.params.crop = { x, y, w, h };
  }

  function onup(e: PointerEvent) {
    if (!drag) return;
    drag = null;
    if (svg.hasPointerCapture(e.pointerId)) svg.releasePointerCapture(e.pointerId);
  }

  const handles = $derived.by(() => {
    const { x, y, w, h } = box;
    const all: [Handle, number, number][] = [
      ["nw", x, y],
      ["n", x + w / 2, y],
      ["ne", x + w, y],
      ["e", x + w, y + h / 2],
      ["se", x + w, y + h],
      ["s", x + w / 2, y + h],
      ["sw", x, y + h],
      ["w", x, y + h / 2],
    ];
    // Mit festem Seitenverhältnis nur Ecken
    return dev.cropAspect ? all.filter(([n]) => n.length === 2) : all;
  });

  function corner(name: Handle, x: number, y: number): string {
    const L = 16;
    const sx = name.includes("w") ? 1 : -1;
    const sy = name.includes("n") ? 1 : -1;
    return `M${x} ${y + sy * L}V${y}H${x + sx * L}`;
  }

  const cursors: Record<Handle, string> = {
    nw: "nwse-resize",
    se: "nwse-resize",
    ne: "nesw-resize",
    sw: "nesw-resize",
    n: "ns-resize",
    s: "ns-resize",
    e: "ew-resize",
    w: "ew-resize",
    move: "move",
  };
</script>

<svg bind:this={svg} class="crop" width={stageW} height={stageH} onpointermove={onmove} onpointerup={onup} onpointercancel={onup} role="application" aria-label={t("Zuschnitt")}>
  <!-- Außenbereich abdunkeln -->
  <path
    class="shade"
    fill-rule="evenodd"
    d="M{rect.x} {rect.y}h{rect.w}v{rect.h}h{-rect.w}z M{box.x} {box.y}h{box.w}v{box.h}h{-box.w}z"
  />
  <rect class="inner" x={box.x} y={box.y} width={box.w} height={box.h} style:cursor="move" onpointerdown={(e) => start(e, "move")} role="presentation" />
  {#each [1, 2] as i}
    <line class="third" x1={box.x + (box.w * i) / 3} y1={box.y} x2={box.x + (box.w * i) / 3} y2={box.y + box.h} />
    <line class="third" x1={box.x} y1={box.y + (box.h * i) / 3} x2={box.x + box.w} y2={box.y + (box.h * i) / 3} />
  {/each}
  <rect class="border" x={box.x} y={box.y} width={box.w} height={box.h} />
  {#each handles as [name, hx, hy] (name)}
    {#if name.length === 2}
      <path class="corner" d={corner(name, hx, hy)} />
    {/if}
    <rect
      class="hit"
      x={hx - 10}
      y={hy - 10}
      width="20"
      height="20"
      style:cursor={cursors[name]}
      onpointerdown={(e) => start(e, name)}
      role="presentation"
    />
    {#if name.length === 1}
      <rect class="edge" x={hx - 4} y={hy - 4} width="8" height="8" />
    {/if}
  {/each}
</svg>

<style>
  .crop {
    position: absolute;
    inset: 0;
    touch-action: none;
  }
  .shade {
    fill: rgb(10 10 12 / 0.62);
    pointer-events: none;
  }
  .inner {
    fill: transparent;
  }
  .third {
    stroke: rgb(255 255 255 / 0.35);
    stroke-width: 1;
    pointer-events: none;
  }
  .border {
    fill: none;
    stroke: #fff;
    stroke-width: 1;
    pointer-events: none;
  }
  .corner {
    fill: none;
    stroke: #fff;
    stroke-width: 3;
    pointer-events: none;
  }
  .edge {
    fill: #fff;
    pointer-events: none;
  }
  .hit {
    fill: transparent;
  }
</style>
