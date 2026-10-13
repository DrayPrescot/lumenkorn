<script lang="ts">
  // Masken direkt auf dem Bild bearbeiten: Verläufe und Ellipsen ziehen, mit dem
  // Pinsel malen, Objekte anklicken. Formen leben in Koordinaten des aufrechten
  // Originals; hier wird zwischen Original und Anzeige (gedreht, gespiegelt,
  // zugeschnitten) umgerechnet.
  import { oklabToCss, type MaskShape, type Stroke } from "../lib/api";
  import { dev } from "../lib/develop.svelte";
  import { t } from "../lib/i18n";
  import { containRect, cropToFrame, frameSize, frameToCrop, frameToSource, sourceToFrame, straightenScale } from "../lib/frame";

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

  // Angezeigtes Bild (object-fit: contain)
  const rect = $derived(view ?? containRect(stageW, stageH, imgW, imgH));

  const q = $derived(((dev.params.rotate % 4) + 4) % 4);
  const crop = $derived(dev.params.crop ?? { x: 0, y: 0, w: 1, h: 1 });
  const src = $derived({ w: dev.fullSize[0] || 1, h: dev.fullSize[1] || 1 });
  const long = $derived(Math.max(src.w, src.h));
  const zoom = $derived.by(() => {
    const [fw, fh] = frameSize(dev.params, [src.w, src.h]);
    return dev.params.straighten ? straightenScale(fw, fh, dev.params.straighten) : 1;
  });
  /** Anzeigepixel pro Originalpixel */
  const scale = $derived((rect.w / ((q % 2 ? src.h : src.w) * crop.w)) * zoom);

  /** Original (normiert) → Anzeige (Pixel in der Bühne) */
  function toStage(sx: number, sy: number): [number, number] {
    const [fx, fy] = sourceToFrame(dev.params, [src.w, src.h], sx, sy);
    const [x, y] = frameToCrop(dev.params, fx, fy);
    return [rect.x + x * rect.w, rect.y + y * rect.h];
  }

  /** Anzeige → Original (normiert) */
  function toSource(px: number, py: number): [number, number] {
    const [fx, fy] = cropToFrame(dev.params, (px - rect.x) / rect.w, (py - rect.y) / rect.h);
    return frameToSource(dev.params, [src.w, src.h], fx, fy);
  }

  // Ellipsenradien in der Anzeige (bei 90°/270° vertauschen sich die Achsen)
  const radii = (rx: number, ry: number): [number, number] => {
    const a = rx * long * scale;
    const b = ry * long * scale;
    return q % 2 ? [b, a] : [a, b];
  };

  let svg: SVGSVGElement;
  type Drag =
    | { kind: "new-linear"; index: number }
    | { kind: "new-radial"; index: number; cx: number; cy: number }
    | { kind: "linear-end"; index: number; end: 0 | 1 }
    | { kind: "linear-move"; index: number; start: [number, number]; orig: MaskShape }
    | { kind: "radial-move"; index: number; start: [number, number]; orig: MaskShape }
    | { kind: "radial-r"; index: number; axis: "x" | "y" }
    | { kind: "paint"; stroke: Stroke; last: [number, number] };
  let drag: Drag | null = null;
  let cursor = $state<[number, number] | null>(null);

  const layer = $derived(dev.layer);

  function local(e: PointerEvent): [number, number] {
    const r = svg.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  }

  function shapeAt(i: number) {
    return layer?.shapes[i];
  }

  function ondown(e: PointerEvent) {
    if (!layer || e.button !== 0) return;
    const [px, py] = local(e);
    const [sx, sy] = toSource(px, py);
    const subtract = dev.pendingSubtract;
    if (dev.tool === "linear") {
      layer.shapes.push({ type: "linear", x0: sx, y0: sy, x1: sx, y1: sy + 1e-3, subtract });
      drag = { kind: "new-linear", index: layer.shapes.length - 1 };
    } else if (dev.tool === "radial") {
      layer.shapes.push({ type: "radial", cx: sx, cy: sy, rx: 0.01, ry: 0.01, feather: 0.5, subtract });
      drag = { kind: "new-radial", index: layer.shapes.length - 1, cx: sx, cy: sy };
    } else if (dev.tool === "brush") {
      let shape = layer.shapes.at(-1);
      if (!shape || shape.type !== "brush" || !!shape.subtract !== subtract) {
        layer.shapes.push({ type: "brush", strokes: [], subtract });
        shape = layer.shapes.at(-1)!;
      }
      const stroke: Stroke = {
        points: [[sx, sy]],
        radius: dev.brush.size,
        feather: dev.brush.feather,
        opacity: 1,
        erase: dev.brush.erase || e.altKey,
      };
      (shape as Extract<MaskShape, { type: "brush" }>).strokes.push(stroke);
      // Auf das reaktive Objekt im Zustand zeigen, nicht auf die lokale Kopie
      const live = (shape as Extract<MaskShape, { type: "brush" }>).strokes.at(-1)!;
      drag = { kind: "paint", stroke: live, last: [px, py] };
      dev.schedule();
    } else if (dev.tool === "object") {
      e.stopPropagation();
      dev.objectClick({ x: sx, y: sy, positive: !e.altKey });
      return;
    } else if (dev.tool === "color") {
      e.stopPropagation();
      dev.colorClick(sx, sy, e.altKey);
      return;
    } else {
      // Ohne Werkzeug: Ziehen verschiebt das gezoomte Bild (ZoomView)
      return;
    }
    // Werkzeug aktiv: nicht zusätzlich das gezoomte Bild verschieben
    e.stopPropagation();
    svg.setPointerCapture(e.pointerId);
    e.preventDefault();
  }

  function onmove(e: PointerEvent) {
    const [px, py] = local(e);
    cursor = [px, py];
    if (!drag || !layer) return;
    const [sx, sy] = toSource(px, py);
    const s = drag.kind !== "paint" ? shapeAt(drag.index) : null;
    switch (drag.kind) {
      case "new-linear":
      case "linear-end":
        if (s?.type === "linear") {
          if (drag.kind === "linear-end" && drag.end === 0) {
            s.x0 = sx;
            s.y0 = sy;
          } else {
            s.x1 = sx;
            s.y1 = sy;
          }
        }
        break;
      case "new-radial": {
        if (s?.type === "radial") {
          const dx = (sx - drag.cx) * src.w;
          const dy = (sy - drag.cy) * src.h;
          const r = Math.max(0.005, Math.hypot(dx, dy) / long);
          s.rx = r;
          s.ry = r;
        }
        break;
      }
      case "radial-r":
        if (s?.type === "radial") {
          const d = drag.axis === "x" ? Math.abs(sx - s.cx) * src.w : Math.abs(sy - s.cy) * src.h;
          if (drag.axis === "x") s.rx = Math.max(0.005, d / long);
          else s.ry = Math.max(0.005, d / long);
        }
        break;
      case "linear-move":
      case "radial-move": {
        const [ox, oy] = drag.start;
        const o = drag.orig;
        const dx = sx - ox;
        const dy = sy - oy;
        if (s?.type === "linear" && o.type === "linear") {
          s.x0 = o.x0 + dx;
          s.y0 = o.y0 + dy;
          s.x1 = o.x1 + dx;
          s.y1 = o.y1 + dy;
        } else if (s?.type === "radial" && o.type === "radial") {
          s.cx = o.cx + dx;
          s.cy = o.cy + dy;
        }
        break;
      }
      case "paint": {
        // Punkte nur in sinnvollem Abstand aufnehmen
        const minDist = Math.max(2, dev.brush.size * long * scale * 0.25);
        if (Math.hypot(px - drag.last[0], py - drag.last[1]) < minDist) return;
        drag.stroke.points.push([sx, sy]);
        drag.last = [px, py];
        break;
      }
    }
    // Live rendern; den Verlaufsschritt legt erst onup() an
    dev.schedule();
  }

  function onup(e: PointerEvent) {
    if (!drag) return;
    drag = null;
    if (svg.hasPointerCapture(e.pointerId)) svg.releasePointerCapture(e.pointerId);
    // Nach dem Erstellen eines Verlaufs oder einer Ellipse zum Bearbeiten wechseln
    if (dev.tool === "linear" || dev.tool === "radial") dev.tool = null;
    dev.pendingSubtract = false;
    dev.commit();
  }

  function startHandle(e: PointerEvent, d: Drag) {
    e.stopPropagation();
    e.preventDefault();
    drag = d;
    svg.setPointerCapture(e.pointerId);
  }

  function startMove(e: PointerEvent, index: number) {
    if (dev.tool) return;
    const [px, py] = local(e);
    const orig = $state.snapshot(layer!.shapes[index]) as MaskShape;
    startHandle(e, { kind: orig.type === "linear" ? "linear-move" : "radial-move", index, start: toSource(px, py), orig });
  }

  /** Senkrechte Linie durch einen Punkt, für die Darstellung des Verlaufs */
  function perpendicular(a: [number, number], b: [number, number], through: [number, number]) {
    const dx = b[0] - a[0];
    const dy = b[1] - a[1];
    const len = Math.hypot(dx, dy) || 1;
    const nx = (-dy / len) * 2000;
    const ny = (dx / len) * 2000;
    return { x1: through[0] - nx, y1: through[1] - ny, x2: through[0] + nx, y2: through[1] + ny };
  }

  const brushR = $derived(dev.brush.size * long * scale);
  const cursorClass = $derived(dev.tool === "brush" ? "none" : dev.tool ? "crosshair" : "default");
</script>

<svg
  bind:this={svg}
  class="overlay"
  class:passive={!layer}
  width={stageW}
  height={stageH}
  style:cursor={cursorClass}
  onpointerdown={ondown}
  onpointermove={onmove}
  onpointerup={onup}
  onpointercancel={onup}
  onpointerleave={() => (cursor = null)}
  ondblclick={(e) => dev.tool && e.stopPropagation()}
  role="application"
  aria-label={t("Masken-Editor")}
>
  <defs>
    <clipPath id="img-clip"><rect x={rect.x} y={rect.y} width={rect.w} height={rect.h} /></clipPath>
  </defs>
  {#if layer}
    <g clip-path="url(#img-clip)">
      {#each layer.shapes as s, i (i)}
        {#if s.type === "linear"}
          {@const a = toStage(s.x0, s.y0)}
          {@const b = toStage(s.x1, s.y1)}
          {@const mid = toStage((s.x0 + s.x1) / 2, (s.y0 + s.y1) / 2)}
          <g class="shape" class:sub={s.subtract}>
            <line class="guide strong" {...perpendicular(a, b, a)} />
            <line class="guide" {...perpendicular(a, b, mid)} />
            <line class="guide" {...perpendicular(a, b, b)} />
            <line class="axis" x1={a[0]} y1={a[1]} x2={b[0]} y2={b[1]} />
            <circle class="move" cx={mid[0]} cy={mid[1]} r="7" onpointerdown={(e) => startMove(e, i)} role="presentation" />
            <circle class="handle" cx={a[0]} cy={a[1]} r="6" onpointerdown={(e) => startHandle(e, { kind: "linear-end", index: i, end: 0 })} role="presentation" />
            <circle class="handle" cx={b[0]} cy={b[1]} r="6" onpointerdown={(e) => startHandle(e, { kind: "linear-end", index: i, end: 1 })} role="presentation" />
          </g>
        {:else if s.type === "radial"}
          {@const c = toStage(s.cx, s.cy)}
          {@const [rxs, rys] = radii(s.rx, s.ry)}
          {@const inner = 1 - (s.feather ?? 0.5)}
          {@const hx = toStage(s.cx + (s.rx * long) / src.w, s.cy)}
          {@const hy = toStage(s.cx, s.cy - (s.ry * long) / src.h)}
          <g class="shape" class:sub={s.subtract}>
            <ellipse class="axis" cx={c[0]} cy={c[1]} rx={rxs} ry={rys} transform="rotate({dev.params.straighten} {c[0]} {c[1]})" />
            <ellipse class="guide" cx={c[0]} cy={c[1]} rx={rxs * inner} ry={rys * inner} transform="rotate({dev.params.straighten} {c[0]} {c[1]})" />
            <circle class="move" cx={c[0]} cy={c[1]} r="7" onpointerdown={(e) => startMove(e, i)} role="presentation" />
            <circle class="handle" cx={hx[0]} cy={hx[1]} r="6" onpointerdown={(e) => startHandle(e, { kind: "radial-r", index: i, axis: "x" })} role="presentation" />
            <circle class="handle" cx={hy[0]} cy={hy[1]} r="6" onpointerdown={(e) => startHandle(e, { kind: "radial-r", index: i, axis: "y" })} role="presentation" />
          </g>
        {:else if s.type === "color"}
          {#each s.samples as c}
            {@const p = toStage(c.x, c.y)}
            <g class="sample" class:sub={s.subtract}>
              <circle cx={p[0]} cy={p[1]} r="7" fill={oklabToCss(c.lab)} />
            </g>
          {/each}
        {:else if s.type === "raster" && s.kind === "object"}
          {#each s.prompt ?? [] as p}
            {@const c = toStage(p.x, p.y)}
            <g class="point" class:neg={!p.positive}>
              <circle cx={c[0]} cy={c[1]} r="8" />
              <line x1={c[0] - 4} y1={c[1]} x2={c[0] + 4} y2={c[1]} />
              {#if p.positive}<line x1={c[0]} y1={c[1] - 4} x2={c[0]} y2={c[1] + 4} />{/if}
            </g>
          {/each}
        {/if}
      {/each}
    </g>
    {#if dev.tool === "brush" && cursor}
      <circle class="brush" class:erase={dev.brush.erase} cx={cursor[0]} cy={cursor[1]} r={brushR} />
      <circle class="brush inner" cx={cursor[0]} cy={cursor[1]} r={brushR * (1 - dev.brush.feather)} />
    {/if}
  {/if}
</svg>

<style>
  .overlay {
    position: absolute;
    inset: 0;
    touch-action: none;
  }
  .overlay.passive {
    pointer-events: none;
  }
  .axis,
  .guide,
  .brush {
    fill: none;
    stroke: #fff;
    stroke-width: 1.5;
    filter: drop-shadow(0 0 1px rgb(0 0 0 / 0.9));
    pointer-events: none;
  }
  .guide {
    stroke-dasharray: 4 4;
    stroke-width: 1;
    opacity: 0.8;
  }
  .guide.strong {
    stroke-dasharray: none;
    opacity: 1;
  }
  .sub .axis,
  .sub .guide {
    stroke: #ff9b8f;
  }
  .handle,
  .move {
    fill: #fff;
    stroke: rgb(0 0 0 / 0.6);
    stroke-width: 1;
    cursor: grab;
  }
  .move {
    fill: var(--accent);
    cursor: move;
  }
  .brush.erase {
    stroke: #ff9b8f;
  }
  .brush.inner {
    stroke-dasharray: 3 3;
    opacity: 0.7;
  }
  .sample circle {
    stroke: #fff;
    stroke-width: 2;
    filter: drop-shadow(0 0 2px rgb(0 0 0 / 0.8));
    pointer-events: none;
  }
  .sample.sub circle {
    stroke-dasharray: 3 2;
  }
  .point circle {
    fill: #2f9e5a;
    stroke: #fff;
    stroke-width: 1.5;
  }
  .point.neg circle {
    fill: #c9574b;
  }
  .point line {
    stroke: #fff;
    stroke-width: 2;
  }
</style>
