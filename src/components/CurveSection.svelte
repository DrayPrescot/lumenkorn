<script lang="ts">
  // Gradationskurve im Reiter „Basis“ (unter Color Grading), wie darktables
  // „RGB-Kurve“: RGB und je Kanal. Klicken setzt einen Punkt, Ziehen
  // verschiebt ihn, Doppelklick oder Rechtsklick entfernt ihn. Dahinter das
  // Histogramm des Kanals.
  import { DEFAULT_PARAMS } from "../lib/api";
  import { dev } from "../lib/develop.svelte";
  import { CURVE_GAP, CURVE_POINTS, IDENTITY, spline, type CurveChannel, type CurvePoint } from "../lib/curve";
  import { loadBins, peakOf, type Bins } from "../lib/histogram";
  import { t } from "../lib/i18n";

  const channels: { id: CurveChannel; label: string; color: string }[] = [
    { id: "rgb", label: t("RGB"), color: "var(--text)" },
    { id: "red", label: t("Rot"), color: "#e5534b" },
    { id: "green", label: t("Grün"), color: "#57ab5a" },
    { id: "blue", label: t("Blau"), color: "#539bf5" },
  ];
  const BIN_OF: Record<CurveChannel, number> = { rgb: 3, red: 0, green: 1, blue: 2 };

  const tc = $derived(dev.params.toneCurve ?? DEFAULT_PARAMS.toneCurve);
  const ch = $derived(dev.curveChannel);
  /** Punkte während des Ziehens: Ein neuer Punkt auf der Diagonalen wird
   * gespeichert als „unverändert“ (leer) – gezogen wird er trotzdem. */
  let work = $state<CurvePoint[] | null>(null);
  const pts = $derived<CurvePoint[]>(work ?? (tc[ch].length ? (tc[ch] as CurvePoint[]) : IDENTITY));
  const color = $derived(channels.find((c) => c.id === ch)?.color ?? "var(--text)");
  const changed = $derived(channels.some((c) => tc[c.id].length > 0));

  /** Kurve als SVG-Pfad (Ansicht 0…256) */
  const path = $derived.by(() => {
    const f = spline(pts);
    const steps = 128;
    let d = "";
    for (let i = 0; i <= steps; i++) {
      const x = i / steps;
      d += `${i ? "L" : "M"}${(x * 256).toFixed(1)},${((1 - f(x)) * 256).toFixed(1)}`;
    }
    return d;
  });

  // --- Histogramm des gewählten Kanals --------------------------------------------
  let bins = $state<Bins | null>(null);
  $effect(() => {
    const src = dev.url;
    if (!src) return;
    let cancelled = false;
    loadBins(src)
      .then((b) => !cancelled && (bins = b))
      .catch(() => {});
    return () => (cancelled = true);
  });
  const histPath = $derived.by(() => {
    if (!bins) return "";
    const b = bins.bins[BIN_OF[ch]];
    const peak = peakOf(b);
    let d = "M0,256";
    for (let i = 0; i < 256; i++) d += `L${i + 0.5},${(256 - Math.sqrt(Math.min(1, b[i] / peak)) * 250).toFixed(1)}`;
    return d + "L256,256Z";
  });

  // --- Bedienung -------------------------------------------------------------------
  let svg = $state<SVGSVGElement>();
  let drag = $state<number | null>(null);
  let hover = $state<CurvePoint | null>(null);

  const clamp = (v: number, lo = 0, hi = 1) => Math.min(hi, Math.max(lo, v));
  function norm(e: PointerEvent | MouseEvent): CurvePoint {
    const r = svg!.getBoundingClientRect();
    return [clamp((e.clientX - r.left) / r.width), clamp(1 - (e.clientY - r.top) / r.height)];
  }
  /** Punkt in der Nähe (8 px) oder −1 */
  function hit(p: CurvePoint): number {
    const tol = 8 / svg!.getBoundingClientRect().width;
    let best = -1;
    let bestD = tol;
    pts.forEach(([x, y], i) => {
      const d = Math.hypot(x - p[0], y - p[1]);
      if (d <= bestD) {
        best = i;
        bestD = d;
      }
    });
    return best;
  }

  function onpointerdown(e: PointerEvent) {
    if (e.button !== 0) return;
    const p = norm(e);
    let i = hit(p);
    if (i < 0) {
      if (pts.length >= CURVE_POINTS) return;
      const list = pts.map((q) => [...q] as CurvePoint);
      i = list.findIndex((q) => q[0] > p[0]);
      if (i < 0) i = list.length;
      if ((i > 0 && p[0] - list[i - 1][0] < CURVE_GAP) || (i < list.length && list[i][0] - p[0] < CURVE_GAP)) return;
      list.splice(i, 0, p);
      dev.setCurve(ch, list);
      work = list;
    } else {
      work = pts.map((q) => [...q] as CurvePoint);
    }
    drag = i;
    svg!.setPointerCapture(e.pointerId);
    e.preventDefault();
  }

  function onpointermove(e: PointerEvent) {
    const p = norm(e);
    hover = p;
    if (drag === null) return;
    const list = pts.map((q) => [...q] as CurvePoint);
    const lo = drag === 0 ? 0 : list[drag - 1][0] + CURVE_GAP;
    const hi = drag === list.length - 1 ? 1 : list[drag + 1][0] - CURVE_GAP;
    list[drag] = [clamp(p[0], lo, hi), p[1]];
    work = list;
    dev.setCurve(ch, list);
  }

  function onpointerup() {
    if (drag === null) return;
    drag = null;
    work = null;
    dev.commit();
  }

  /** Punkt entfernen (die Endpunkte bleiben) */
  function remove(e: MouseEvent) {
    e.preventDefault();
    const i = hit(norm(e));
    if (i <= 0 || i >= pts.length - 1) return;
    const list = pts.filter((_, k) => k !== i);
    dev.setCurve(ch, list);
    dev.commit();
  }

  function resetChannel() {
    dev.setCurve(ch, []);
    dev.commit();
  }

  /** Angezeigter Punkt: der gezogene, sonst die Kurve unter dem Mauszeiger */
  const readout = $derived.by(() => {
    if (drag !== null && pts[drag]) return pts[drag];
    if (hover) return [hover[0], spline(pts)(hover[0])] as CurvePoint;
    return null;
  });
  const v255 = (v: number) => Math.round(v * 255);
</script>

<details open class="curve">
  <summary tabindex="-1">{t("Gradationskurve")}</summary>
  <div class="tabs" role="tablist" aria-label={t("Kanal")}>
    {#each channels as c (c.id)}
      <button
        role="tab"
        tabindex="-1"
        aria-selected={ch === c.id}
        class:on={ch === c.id}
        style:--c={c.color}
        onclick={() => (dev.curveChannel = c.id)}
      >
        {c.label}{#if tc[c.id].length}<span class="dot" aria-label={t("geändert")}></span>{/if}
      </button>
    {/each}
  </div>

  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <svg
    bind:this={svg}
    viewBox="0 0 256 256"
    class:dragging={drag !== null}
    role="application"
    aria-label={t("Gradationskurve: Klicken setzt einen Punkt, Ziehen verschiebt ihn, Doppelklick entfernt ihn")}
    {onpointerdown}
    {onpointermove}
    {onpointerup}
    onpointercancel={onpointerup}
    onpointerleave={() => drag === null && (hover = null)}
    ondblclick={remove}
    oncontextmenu={remove}
  >
    {#if histPath}<path class="hist" d={histPath} style:--c={color} />{/if}
    {#each [64, 128, 192] as g (g)}
      <line class="grid" x1={g} y1="0" x2={g} y2="256" />
      <line class="grid" x1="0" y1={g} x2="256" y2={g} />
    {/each}
    <line class="diag" x1="0" y1="256" x2="256" y2="0" />
    <path class="line" d={path} style:--c={color} />
    {#each pts as [x, y], i (i)}
      <circle class="pt" class:active={drag === i} cx={x * 256} cy={(1 - y) * 256} r="5" style:--c={color} />
    {/each}
  </svg>

  <div class="foot">
    <span class="readout">
      {#if readout}{t("Eingang {i} · Ausgabe {o}", { i: v255(readout[0]), o: v255(readout[1]) })}{:else}&nbsp;{/if}
    </span>
    {#if tc[ch].length}
      <button class="btn small quiet" tabindex="-1" onclick={resetChannel}>{t("Zurücksetzen")}</button>
    {/if}
  </div>
  {#if ch === "rgb"}
    <label class="check" title={t("Die RGB-Kurve wirkt auf die Helligkeit; Farbton und Sättigung bleiben. Aus: auf jeden Kanal einzeln, mehr Kontrast macht dann auch die Farben kräftiger.")}>
      <input type="checkbox" checked={tc.preserveColors} onchange={(e) => dev.setCurvePreserve(e.currentTarget.checked)} />
      {t("Farben erhalten")}
    </label>
  {/if}
  {#if !changed}
    <p class="hint">{t("Klicken setzt einen Punkt, Ziehen verschiebt ihn, Doppelklick entfernt ihn.")}</p>
  {/if}
</details>

<style>
  /* wie die übrigen Abschnitte in DevelopPanel */
  .curve {
    padding: 12px 16px;
    border-bottom: 1px solid var(--line);
  }
  .tabs {
    display: flex;
    gap: 4px;
    margin: 10px 0 8px;
  }
  .tabs button {
    flex: 1;
    padding: 4px 0;
    border-radius: var(--radius-s);
    color: var(--muted);
    font-size: var(--step--1);
    border: 1px solid transparent;
  }
  .tabs button.on {
    color: var(--c);
    border-color: var(--line);
    background: var(--raised);
  }
  .dot {
    display: inline-block;
    width: 5px;
    height: 5px;
    margin-left: 4px;
    border-radius: 50%;
    background: var(--accent);
    vertical-align: middle;
  }
  svg {
    display: block;
    width: 100%;
    aspect-ratio: 1;
    background: var(--raised);
    border-radius: var(--radius-s);
    cursor: crosshair;
    touch-action: none;
    overflow: visible;
  }
  svg.dragging {
    cursor: grabbing;
  }
  .hist {
    fill: var(--c);
    opacity: 0.12;
  }
  .grid {
    stroke: var(--line);
    stroke-width: 1;
    vector-effect: non-scaling-stroke;
  }
  .diag {
    stroke: var(--faint);
    stroke-width: 1;
    stroke-dasharray: 3 4;
    vector-effect: non-scaling-stroke;
  }
  .line {
    fill: none;
    stroke: var(--c);
    stroke-width: 1.75;
    vector-effect: non-scaling-stroke;
  }
  .pt {
    fill: var(--panel);
    stroke: var(--c);
    stroke-width: 1.5;
    vector-effect: non-scaling-stroke;
  }
  .pt.active {
    fill: var(--c);
  }
  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    min-height: 28px;
    margin-top: 6px;
  }
  .readout {
    white-space: nowrap;
    font-size: var(--step--1);
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--step--1);
  }
  .hint {
    margin: 6px 0 0;
    font-size: var(--step--1);
    color: var(--muted);
  }
</style>
