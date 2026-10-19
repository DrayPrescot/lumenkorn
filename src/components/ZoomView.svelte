<script lang="ts">
  // Bildanzeige mit Zoom (Scrollrad, Tasten über ZoomState), Verschieben per
  // Ziehen und Doppelklick (Einpassen ↔ 100 %). Reicht die Auflösung des
  // angezeigten Bildes nicht mehr, wird nachgeladen: entweder das ganze Bild
  // in voller Auflösung (`hiSrc`, Bibliothek, nur beim Zoomen) oder der
  // sichtbare Ausschnitt (`detail`, Entwickeln – die Bearbeitung ändert sich
  // laufend). Im Entwickeln auch eingepasst: Ist der Proxy kleiner als die
  // Bühne in Gerätepixeln, kommt das ganze Bild in Bühnenauflösung aus der
  // vollen Datei.
  import type { Snippet } from "svelte";
  import { onDestroy, untrack } from "svelte";
  import { t } from "../lib/i18n";
  import { MAX_ZOOM, ZoomState, loadHi } from "../lib/zoom.svelte";

  type Region = { x: number; y: number; w: number; h: number };

  let {
    src,
    hiSrc = null,
    hiPriority = false,
    detail = null,
    detailKey = null,
    fullLong = null,
    srcLong = null,
    zoom,
    alt = "",
    dim = false,
    showBadge = true,
    onselect,
    children,
  }: {
    src: string | null;
    /** Ganzes Bild in voller Auflösung */
    hiSrc?: string | null;
    /** Volle Auflösung vor anderen Ansichten laden (hervorgehobenes Foto im Vergleich) */
    hiPriority?: boolean;
    /** Liefert eine Bild-URL für einen Ausschnitt in hoher Auflösung */
    detail?: ((r: Region, scale: number) => Promise<string | null>) | null;
    /** Ändert sich, wenn der Bildinhalt sich ändert (Ausschnitt verwerfen) */
    detailKey?: unknown;
    /** Längste Kante in voller Auflösung (Originalpixel) */
    fullLong?: number | null;
    /** Längste Kante der Quelle, aus der `src` entstand (sonst: natürliche Größe) */
    srcLong?: number | null;
    zoom: ZoomState;
    alt?: string;
    dim?: boolean;
    showBadge?: boolean;
    /** Klick ohne Verschieben */
    onselect?: () => void;
    /** Überlagerungen (Masken, Zuschnitt …). Bekommen beim Zoomen die Lage
     *  des Bildes in der Ansicht, eingepasst `null`. */
    children?: Snippet<[Region | null]>;
  } = $props();

  let el: HTMLDivElement;
  let w = $state(0);
  let h = $state(0);
  let natW = $state(0);
  let natH = $state(0);
  const dpr = typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1;

  // --- Ganzes Bild in voller Auflösung (Bibliothek) -------------------------
  let hi = $state<{ src: string; w: number; h: number } | null>(null);
  let hiLoading = $state<string | null>(null);
  const hiReady = $derived(!!hi && hi.src === hiSrc);

  // Maße in voller Auflösung
  const upscale = $derived.by(() => {
    if (!fullLong || !natW) return 1;
    const nat = Math.max(natW, natH);
    return Math.max(1, fullLong / (srcLong ?? nat));
  });
  const shownW = $derived(hiReady ? hi!.w : natW);
  const shownH = $derived(hiReady ? hi!.h : natH);
  const fullW = $derived(hiReady ? hi!.w : natW * upscale);
  const fullH = $derived(hiReady ? hi!.h : natH * upscale);
  const fitZoom = $derived(fullW && w && h ? Math.min(w / fullW, h / fullH) * dpr : 0);
  $effect(() => {
    if (fitZoom > 0) zoom.fitZoom = fitZoom;
  });

  // --- Lage des Bildes beim Zoomen ------------------------------------------
  const dw = $derived((fullW * zoom.zoom) / dpr);
  const dh = $derived((fullH * zoom.zoom) / dpr);
  const clampC = (c: number, d: number, view: number) => {
    if (d <= view) return 0.5;
    const m = view / 2 / d;
    return Math.min(1 - m, Math.max(m, c));
  };
  const cx = $derived(clampC(zoom.cx, dw, w));
  const cy = $derived(clampC(zoom.cy, dh, h));
  const left = $derived(w / 2 - cx * dw);
  const top = $derived(h / 2 - cy * dh);

  /** Bildpunkt (normiert) an einer Stelle der Ansicht */
  function imagePoint(mx: number, my: number): [number, number] {
    if (zoom.fit) {
      const fw = (fullW * fitZoom) / dpr;
      const fh = (fullH * fitZoom) / dpr;
      return [0.5 + (mx - w / 2) / fw, 0.5 + (my - h / 2) / fh];
    }
    return [cx + (mx - w / 2) / dw, cy + (my - h / 2) / dh];
  }

  // Scrollrad: zum Mauszeiger hin zoomen (nicht-passiv, sonst scrollt die Seite)
  $effect(() => {
    const onwheel = (e: WheelEvent) => {
      if (!fullW || !fitZoom) return;
      e.preventDefault();
      const r = el.getBoundingClientRect();
      const mx = e.clientX - r.left;
      const my = e.clientY - r.top;
      const [ix, iy] = imagePoint(mx, my);
      const factor = Math.exp(-e.deltaY * (e.deltaMode === 1 ? 0.05 : 0.0015));
      const nz = Math.min(MAX_ZOOM, zoom.effective * factor);
      const ndw = (fullW * nz) / dpr;
      const ndh = (fullH * nz) / dpr;
      zoom.set(nz, ix - (mx - w / 2) / ndw, iy - (my - h / 2) / ndh);
    };
    el.addEventListener("wheel", onwheel, { passive: false });
    return () => el.removeEventListener("wheel", onwheel);
  });

  // Ziehen verschiebt, Klick wählt aus
  let drag: { x: number; y: number; cx: number; cy: number; moved: boolean } | null = null;
  function onpointerdown(e: PointerEvent) {
    if (e.button !== 0) return;
    drag = { x: e.clientX, y: e.clientY, cx, cy, moved: false };
    if (!zoom.fit) el.setPointerCapture(e.pointerId);
  }
  function onpointermove(e: PointerEvent) {
    if (!drag) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
    if (drag.moved && !zoom.fit) {
      zoom.cx = drag.cx - dx / dw;
      zoom.cy = drag.cy - dy / dh;
    }
  }
  function onpointerup() {
    if (drag && !drag.moved) onselect?.();
    drag = null;
  }
  function ondblclick(e: MouseEvent) {
    const r = el.getBoundingClientRect();
    const [ix, iy] = imagePoint(e.clientX - r.left, e.clientY - r.top);
    zoom.toggle(Math.min(1, Math.max(0, ix)), Math.min(1, Math.max(0, iy)));
  }

  // Reicht die Auflösung des angezeigten Bildes nicht mehr?
  const needMore = $derived(!zoom.fit && shownW > 0 && dw * dpr > shownW * 1.05);

  // Lage des Bildes in der Ansicht, eingepasst (wie `object-fit: contain`)
  // oder gezoomt
  const viewW = $derived(zoom.fit ? (fullW * fitZoom) / dpr : dw);
  const viewH = $derived(zoom.fit ? (fullH * fitZoom) / dpr : dh);
  const viewL = $derived(zoom.fit ? (w - viewW) / 2 : left);
  const viewT = $derived(zoom.fit ? (h - viewH) / 2 : top);
  /** Ausschnitt in hoher Auflösung nötig (auch eingepasst, siehe oben) */
  const needDetail = $derived(!!detail && !hiSrc && shownW > 0 && viewW * dpr > shownW * 1.05);

  // Nur sichtbare Ansichten laden die volle Auflösung (Vergleich: viele Zellen)
  let visible = $state(true);
  $effect(() => {
    const io = new IntersectionObserver((e) => (visible = e.some((x) => x.isIntersecting)));
    io.observe(el);
    return () => io.disconnect();
  });

  $effect(() => {
    if (!needMore || !visible || !hiSrc || hiReady) return;
    const want = hiSrc;
    hiLoading = want;
    // Über eine gemeinsame Warteschlange (höchstens zwei gleichzeitig); wird die
    // volle Auflösung nicht mehr gebraucht, bricht das Aufräumen ab.
    const cancel = loadHi(want, untrack(() => hiPriority), (img) => {
      if (img && hiSrc === want) hi = { src: want, w: img.naturalWidth, h: img.naturalHeight };
      if (hiLoading === want) hiLoading = null;
    });
    return () => {
      cancel();
      if (untrack(() => hiLoading) === want) hiLoading = null;
    };
  });

  // --- Ausschnitt in hoher Auflösung (Entwickeln) ----------------------------
  let det = $state<{ url: string; r: Region; key: unknown } | null>(null);
  let detPending = $state(false);
  let detSeq = 0;
  let detTimer: ReturnType<typeof setTimeout> | undefined;

  $effect(() => {
    const need = needDetail;
    const key = detailKey;
    const [L, T, DW, DH, W, H] = [viewL, viewT, viewW, viewH, w, h];
    const Z = zoom.fit ? fitZoom : zoom.zoom;
    clearTimeout(detTimer);
    if (!need) {
      detPending = false;
      return;
    }
    const fn = detail!;
    detPending = true;
    detTimer = setTimeout(async () => {
      const c = (v: number) => Math.min(1, Math.max(0, v));
      const x0 = c(-L / DW);
      const y0 = c(-T / DH);
      const r = { x: x0, y: y0, w: c((W - L) / DW) - x0, h: c((H - T) / DH) - y0 };
      if (r.w <= 0 || r.h <= 0) return;
      const seq = ++detSeq;
      const url = await fn(r, Math.min(1, Z)).catch(() => null);
      if (seq !== detSeq) {
        if (url) URL.revokeObjectURL(url);
        return;
      }
      const old = untrack(() => det);
      if (old) URL.revokeObjectURL(old.url);
      det = url ? { url, r, key } : null;
      detPending = false;
    }, 220);
  });
  const showDet = $derived(!!det && det.key === detailKey && needDetail);

  // Beim Schließen: laufende Anfrage verwerfen, Ausschnitt freigeben
  onDestroy(() => {
    clearTimeout(detTimer);
    detSeq++;
    if (det) URL.revokeObjectURL(det.url);
  });
</script>

<div
  class="zv"
  class:zoomed={!zoom.fit}
  bind:this={el}
  bind:clientWidth={w}
  bind:clientHeight={h}
  {onpointerdown}
  {onpointermove}
  {onpointerup}
  onpointercancel={() => (drag = null)}
  {ondblclick}
  role="presentation"
>
  {#if src}
    <img
      class:fit={zoom.fit}
      class:dim
      src={hiReady ? hiSrc : src}
      {alt}
      draggable="false"
      bind:naturalWidth={natW}
      bind:naturalHeight={natH}
      style={zoom.fit ? "" : `left:${left}px;top:${top}px;width:${dw}px;height:${dh}px`}
    />
    {#if showDet && det}
      <img
        class="detail"
        src={det.url}
        alt=""
        draggable="false"
        style={`left:${viewL + det.r.x * viewW}px;top:${viewT + det.r.y * viewH}px;width:${det.r.w * viewW}px;height:${det.r.h * viewH}px`}
      />
    {/if}
  {/if}

  {@render children?.(zoom.fit ? null : { x: left, y: top, w: dw, h: dh })}

  {#if showBadge && fitZoom > 0}
    <div class="badge" class:busy={!!hiLoading || (detPending && needDetail)}>
      <button title={t("Verkleinern (Strg −)")} onclick={(e) => { e.stopPropagation(); zoom.stepOut(); }}>−</button>
      <button class="pct" title={t("Einpassen ↔ 100 % (Z, Doppelklick)")} onclick={(e) => { e.stopPropagation(); zoom.toggle(cx, cy); }}>
        {zoom.fit ? t("Einpassen · {n} %", { n: String(Math.round(fitZoom * 100)) }) : `${Math.round(zoom.zoom * 100)} %`}
      </button>
      <button title={t("Vergrößern (Strg +, Scrollrad)")} onclick={(e) => { e.stopPropagation(); zoom.stepIn(); }}>+</button>
      {#if hiLoading || (detPending && needDetail)}<span class="spin" aria-label={t("Lädt hohe Auflösung")}></span>{/if}
    </div>
  {/if}
</div>

<style>
  .zv {
    position: absolute;
    inset: 0;
    overflow: hidden;
    touch-action: none;
  }
  .zv.zoomed {
    cursor: grab;
  }
  .zv.zoomed:active {
    cursor: grabbing;
  }
  img {
    position: absolute;
    max-width: none;
    user-select: none;
  }
  img.fit {
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  img.dim {
    opacity: 0.55;
    filter: saturate(0.6);
  }
  .badge {
    position: absolute;
    left: 6px;
    bottom: 6px;
    display: flex;
    align-items: center;
    gap: 1px;
    padding: 2px;
    border-radius: var(--radius-s);
    background: rgb(20 20 22 / 0.72);
    border: 1px solid var(--line);
    font-size: var(--step--1);
    color: var(--muted);
    opacity: 0.75;
    transition: opacity 0.12s;
  }
  .zv:hover .badge,
  .badge.busy {
    opacity: 1;
  }
  .badge button {
    padding: 1px 7px;
    border-radius: 3px;
    color: inherit;
  }
  .badge button:hover {
    color: var(--text);
    background: var(--raised);
  }
  .pct {
    min-width: 64px;
    font-variant-numeric: tabular-nums;
  }
  .spin {
    width: 10px;
    height: 10px;
    margin: 0 5px;
    border: 2px solid var(--faint);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
