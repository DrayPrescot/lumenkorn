<script lang="ts">
  // Zuschnittrahmen über dem ungeschnittenen Bild: Ecken und Kanten ziehen,
  // innen verschieben, optional mit festem Seitenverhältnis. Drittel-Raster
  // hilft beim Bildaufbau, beim Geraderichten ein feines Raster beim Ausrichten.
  // Im Linienmodus zieht man eine Linie entlang von Horizont oder Kante.
  import { dev } from "../lib/develop.svelte";
  import { containRect, frameSize, warpForward } from "../lib/frame";
  import { t } from "../lib/i18n";

  let {
    stageW,
    stageH,
    imgW,
    imgH,
    view = null,
  }: {
    stageW: number;
    stageH: number;
    imgW: number;
    imgH: number;
    /** Lage des Bildes beim Zoomen (ZoomView), sonst eingepasst */
    view?: { x: number; y: number; w: number; h: number } | null;
  } = $props();

  const rect = $derived(view ?? containRect(stageW, stageH, imgW, imgH));

  const c = $derived(dev.params.crop ?? { x: 0, y: 0, w: 1, h: 1 });
  const frame = $derived(frameSize(dev.params, [dev.fullSize[0] || 1, dev.fullSize[1] || 1]));
  const box = $derived({ x: rect.x + c.x * rect.w, y: rect.y + c.y * rect.h, w: c.w * rect.w, h: c.h * rect.h });

  // Feines Raster: quadratische Felder von etwa 30 px, am Ausschnitt ausgerichtet
  const FINE = 30;
  const fine = $derived.by(() => {
    if (!dev.straightening || box.w < 1 || box.h < 1) return { xs: [] as number[], ys: [] as number[] };
    const nx = Math.max(2, Math.round(box.w / FINE));
    const ny = Math.max(2, Math.round(box.h / FINE));
    return {
      xs: Array.from({ length: nx - 1 }, (_, i) => box.x + (box.w * (i + 1)) / nx),
      ys: Array.from({ length: ny - 1 }, (_, i) => box.y + (box.h * (i + 1)) / ny),
    };
  });

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

  /** Gespeicherte Hilfslinien, wie sie jetzt im Rahmen liegen (Bühnenpixel) */
  const shownGuides = $derived.by(() => {
    const full: [number, number] = [dev.fullSize[0] || 1, dev.fullSize[1] || 1];
    const at = (q: [number, number]) => {
      const [x, y] = warpForward(dev.params, full, q[0], q[1]);
      return [rect.x + x * rect.w, rect.y + y * rect.h];
    };
    return dev.guides.map((g) => {
      const [x1, y1] = at(g.a);
      const [x2, y2] = at(g.b);
      return { x1, y1, x2, y2, kind: g.kind };
    });
  });

  /** Gezogene Linie in Bühnenpixeln */
  let line = $state<{ x1: number; y1: number; x2: number; y2: number } | null>(null);

  function stagePoint(e: PointerEvent): [number, number] {
    const r = svg.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  }

  function lineStart(e: PointerEvent) {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.preventDefault();
    const [x, y] = stagePoint(e);
    line = { x1: x, y1: y, x2: x, y2: y };
    svg.setPointerCapture(e.pointerId);
  }

  function onmove(e: PointerEvent) {
    if (line) {
      const [x, y] = stagePoint(e);
      line = { ...line, x2: x, y2: y };
      return;
    }
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
    if (line) {
      const l = line;
      line = null;
      if (svg.hasPointerCapture(e.pointerId)) svg.releasePointerCapture(e.pointerId);
      if (e.type === "pointerup")
        dev.addGuide((l.x1 - rect.x) / rect.w, (l.y1 - rect.y) / rect.h, (l.x2 - rect.x) / rect.w, (l.y2 - rect.y) / rect.h);
      return;
    }
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
  {#each fine.xs as x (x)}
    <line class="fine" x1={x} y1={box.y} x2={x} y2={box.y + box.h} />
  {/each}
  {#each fine.ys as y (y)}
    <line class="fine" x1={box.x} y1={y} x2={box.x + box.w} y2={y} />
  {/each}
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
  {#if dev.lineMode}
    <!-- Über allem: Ziehen zeichnet die Linie statt den Rahmen zu verändern -->
    <rect class="catch" x={rect.x} y={rect.y} width={rect.w} height={rect.h} onpointerdown={lineStart} role="presentation" />
    {#each shownGuides as g, i (i)}
      <line class="ruler-shadow" x1={g.x1} y1={g.y1} x2={g.x2} y2={g.y2} />
      <line class="ruler" x1={g.x1} y1={g.y1} x2={g.x2} y2={g.y2} />
      {#each [[g.x1, g.y1], [g.x2, g.y2]] as [cx, cy]}
        <circle class="end" {cx} {cy} r="4" />
      {/each}
      <!-- Rechtsklick auf die Linie entfernt sie -->
      <line
        class="guide-hit"
        x1={g.x1}
        y1={g.y1}
        x2={g.x2}
        y2={g.y2}
        role="presentation"
        onpointerdown={lineStart}
        oncontextmenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          dev.removeGuide(i);
        }}
      ><title>{t("Rechtsklick entfernt die Linie")}</title></line>
    {/each}
    {#if line}
      <line class="ruler-shadow" x1={line.x1} y1={line.y1} x2={line.x2} y2={line.y2} />
      <line class="ruler" x1={line.x1} y1={line.y1} x2={line.x2} y2={line.y2} />
    {:else}
      <text class="hint" x={rect.x + rect.w / 2} y={rect.y + 28}
        >{dev.guides.length
          ? t("Weitere Linie ziehen – zwei senkrechte richten stürzende Linien auf, zwei waagerechte die seitliche Perspektive")
          : t("Linie entlang einer Kante ziehen, die senkrecht oder waagerecht stehen soll")}</text
      >
    {/if}
  {/if}
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
  .fine {
    stroke: rgb(255 255 255 / 0.28);
    stroke-width: 1;
    pointer-events: none;
    shape-rendering: crispEdges;
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
  .catch {
    fill: transparent;
    cursor: crosshair;
  }
  .ruler {
    stroke: #ffd24a;
    stroke-width: 2;
    pointer-events: none;
  }
  .end {
    fill: #ffd24a;
    stroke: rgb(0 0 0 / 0.6);
    pointer-events: none;
  }
  .guide-hit {
    stroke: transparent;
    stroke-width: 12;
    cursor: context-menu;
  }
  .ruler-shadow {
    stroke: rgb(0 0 0 / 0.6);
    stroke-width: 4;
    pointer-events: none;
  }
  .hint {
    fill: #fff;
    font-size: 13px;
    text-anchor: middle;
    paint-order: stroke;
    stroke: rgb(0 0 0 / 0.7);
    stroke-width: 3px;
    pointer-events: none;
  }
</style>
