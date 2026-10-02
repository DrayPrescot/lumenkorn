<script lang="ts">
  import type { DevelopParams } from "../lib/api";
  import { dev } from "../lib/develop.svelte";
  import { lib } from "../lib/store.svelte";
  import Histogram from "./Histogram.svelte";
  import MaskPanel from "./MaskPanel.svelte";
  import HslSection from "./HslSection.svelte";
  import RepairPanel from "./RepairPanel.svelte";
  import Icon from "./Icon.svelte";
  import Slider from "./Slider.svelte";
  import { frameSize } from "../lib/frame";
  import { api } from "../lib/api";
  import { untrack } from "svelte";

  let { onexport }: { onexport: () => void } = $props();

  type Key = Exclude<keyof DevelopParams, "version" | "rotate" | "flipHorizontal" | "crop">;
  interface Def {
    key: Key;
    label: string;
    min: number;
    max: number;
    step?: number;
    decimals?: number;
    unit?: string;
    track?: string;
  }

  const groups: { title: string; sliders: Def[] }[] = [
    {
      title: "Weißabgleich",
      sliders: [
        { key: "temperature", label: "Temperatur", min: -100, max: 100, track: "linear-gradient(to right, #4f78c4, #8a8a8a, #d6b04a)" },
        { key: "tint", label: "Tönung", min: -100, max: 100, track: "linear-gradient(to right, #5aa35a, #8a8a8a, #b65aa8)" },
      ],
    },
    {
      title: "Licht",
      sliders: [
        { key: "exposure", label: "Belichtung", min: -5, max: 5, step: 0.05, decimals: 2 },
        { key: "contrast", label: "Kontrast", min: -100, max: 100 },
        { key: "highlights", label: "Lichter", min: -100, max: 100 },
        { key: "shadows", label: "Tiefen", min: -100, max: 100 },
        { key: "whites", label: "Weiß", min: -100, max: 100 },
        { key: "blacks", label: "Schwarz", min: -100, max: 100 },
      ],
    },
    {
      title: "Farbe",
      sliders: [
        { key: "vibrance", label: "Dynamik", min: -100, max: 100 },
        { key: "saturation", label: "Sättigung", min: -100, max: 100 },
      ],
    },
    {
      title: "Details",
      sliders: [
        { key: "clarity", label: "Klarheit", min: -100, max: 100 },
        { key: "sharpen", label: "Schärfen", min: 0, max: 150 },
      ],
    },
  ];

  let snapName = $state("");
  const running = $derived(!!dev.denoising && dev.denoising.imageId === dev.imageId);

  // Seitenverhältnisse als Breite/Höhe; „Original“ = Verhältnis des Rahmens
  const aspects: { label: string; value: number | null | "orig" }[] = [
    { label: "Frei", value: null },
    { label: "Original", value: "orig" },
    { label: "1:1", value: 1 },
    { label: "4:5", value: 4 / 5 },
    { label: "3:2", value: 3 / 2 },
    { label: "16:9", value: 16 / 9 },
  ];
  let aspectKey = $state("Frei");
  const frame = $derived(frameSize(dev.params, [dev.fullSize[0] || 1, dev.fullSize[1] || 1]));

  function setAspect(a: (typeof aspects)[number]) {
    aspectKey = a.label;
    const v = a.value === "orig" ? frame[0] / frame[1] : a.value;
    dev.setAspect(v, frame);
  }

  function swapAspect() {
    if (dev.cropAspect) dev.setAspect(1 / dev.cropAspect, frame);
  }

  // --- Reiter ----------------------------------------------------------------
  type Tab = "basic" | "masks" | "repair" | "crop";
  const tabs: { id: Tab; label: string; title: string }[] = [
    { id: "basic", label: "Basis", title: "Licht, Farbe, Details, Rauschreduzierung" },
    { id: "masks", label: "Masken", title: "Lokale Anpassungen" },
    { id: "repair", label: "Reparatur", title: "Störendes mit KI entfernen" },
    { id: "crop", label: "Zuschnitt", title: "Zuschneiden, Geraderichten, Drehen (R)" },
  ];

  function setTab(t: Tab) {
    if (t === dev.tab) return;
    returnTo = null; // Reiter von Hand gewählt: kein automatischer Rücksprung
    if (dev.tab === "crop" && dev.cropMode) dev.applyCrop();
    if (dev.tab === "masks" || dev.tab === "repair") {
      dev.tool = null;
      dev.selectMask(null);
      if (dev.showOverlay) dev.toggleOverlay();
    }
    dev.tab = t;
    dev.hslPick = false;
    if (t === "crop") dev.enterCrop();
  }

  // Zuschneiden per Taste (R) öffnet den Reiter „Zuschnitt“ und kehrt nach
  // Enter/Esc/R zum vorherigen Reiter zurück. Per Klick auf den Reiter bleibt man dort.
  let wasCrop = false;
  let returnTo: Tab | null = null;
  $effect(() => {
    const c = dev.cropMode;
    untrack(() => {
      if (c && !wasCrop && dev.tab !== "crop") {
        returnTo = dev.tab;
        dev.tab = "crop";
      } else if (!c && wasCrop && returnTo && dev.tab === "crop") {
        dev.tab = returnTo;
        returnTo = null;
      }
      wasCrop = c;
    });
  });
  // Maskenwerkzeug oder Maske auf dem Bild gewählt → Reiter „Masken“
  $effect(() => {
    const t = untrack(() => dev.tab);
    if ((dev.activeMask !== null || dev.tool) && t !== "masks" && t !== "repair") dev.tab = "masks";
  });
</script>

<aside class="scroll">
  <section class="hist">
    <Histogram src={dev.showBefore ? dev.beforeUrl : dev.url} />
    {#if dev.fullSize[0]}
      <p class="meta">
        {dev.fullSize[0]} × {dev.fullSize[1]} px{dev.raw ? ", RAW" : ""}
      </p>
    {/if}
  </section>

  <div class="tabs" role="tablist" aria-label="Bearbeitung">
    {#each tabs as t (t.id)}
      <button role="tab" aria-selected={dev.tab === t.id} class:on={dev.tab === t.id} title={t.title} onclick={() => setTab(t.id)}>
        {t.label}{#if t.id === "masks" && dev.params.masks.length}<span class="count">{dev.params.masks.length}</span>{/if}{#if t.id === "repair" && dev.params.repairs.length}<span class="count">{dev.params.repairs.length}</span>{/if}
      </button>
    {/each}
  </div>

  {#if dev.tab === "masks"}
    <MaskPanel />
  {:else if dev.tab === "repair"}
    <RepairPanel />
  {:else if dev.tab === "crop"}
    {@render cropSection()}
  {:else}
    {#each groups as g (g.title)}
      <details open>
        <summary>{g.title}</summary>
        <div class="sliders">
          {#each g.sliders as s (s.key)}
            <Slider
              label={s.label}
              value={dev.params[s.key] as number}
              min={s.min}
              max={s.max}
              step={s.step ?? 1}
              decimals={s.decimals ?? 0}
              track={s.track}
              oninput={(v) => dev.set(s.key, v)}
              oncommit={() => dev.commit()}
            />
          {/each}
        </div>
        {#if g.title === "Details"}
          {@render denoiseSection()}
        {/if}
      </details>
      {#if g.title === "Farbe"}
        <HslSection />
      {/if}
    {/each}

    <details>
      <summary>Schnappschüsse {dev.snapshots.length ? `(${dev.snapshots.length})` : ""}</summary>
      <form class="snap" onsubmit={(e) => { e.preventDefault(); dev.snapshot(snapName); snapName = ""; }}>
        <input type="text" placeholder="Name, z. B. Schwarzweiß" bind:value={snapName} onkeydown={(e) => e.stopPropagation()} />
        <button class="btn" type="submit" disabled={dev.isDefault}>Festhalten</button>
      </form>
      <ul class="snaps">
        {#each dev.snapshots as s (s.id)}
          <li>
            <button class="name" onclick={() => dev.restore(s)} title="Diesen Stand übernehmen">{s.name}</button>
            <span class="when">{new Date(s.createdAt * 1000).toLocaleDateString("de-DE", { day: "numeric", month: "short" })}</span>
            <button class="icon-btn" aria-label="Schnappschuss löschen" onclick={() => dev.deleteSnapshot(s)}><Icon name="close" size={12} /></button>
          </li>
        {:else}
          <li class="empty">Hält den aktuellen Stand fest, um später dorthin zurückzukehren.</li>
        {/each}
      </ul>
    </details>
  {/if}

  <footer>
    <div class="row">
      <button class="btn" disabled={dev.isDefault} onclick={() => dev.reset()} title="Alle Regler zurücksetzen">Zurücksetzen</button>
    </div>
    <button class="btn primary wide" onclick={onexport}>Exportieren …</button>
  </footer>
</aside>

{#snippet denoiseSection()}
  <div class="denoise">
    <div class="dn-head">
      <span class="dn-title">Rauschreduzierung <span class="ki">KI</span></span>
      {#if dev.params.denoise && !running}
        <button class="icon-btn" aria-label="Rauschreduzierung entfernen" title="Entfernen" onclick={() => dev.removeDenoise()}><Icon name="close" size={12} /></button>
      {/if}
    </div>
    {#if running}
      {@const d = dev.denoising!}
      <p class="dn-note">
        {d.total ? `Kachel ${d.done} von ${d.total}` : "Wird vorbereitet …"} – läuft im Hintergrund weiter.
      </p>
      <progress max={d.total || 1} value={d.done}></progress>
      <button class="btn small" onclick={() => api.denoiseCancel()}>Abbrechen</button>
    {:else if dev.params.denoise}
      <Slider
        label="Mischung"
        value={dev.params.denoise.amount}
        min={0}
        max={100}
        neutral={100}
        oninput={(v) => { if (dev.params.denoise) { dev.params.denoise.amount = v; dev.schedule(); } }}
        oncommit={() => dev.commit()}
      />
      <div class="dn-again">
        <Slider label="Stärke" value={dev.denoiseStrength} min={0} max={100} neutral={65} oninput={(v) => (dev.denoiseStrength = v)} oncommit={() => {}} />
        <button class="btn small" onclick={() => dev.startDenoise()} title="Mit dieser Stärke neu berechnen">Neu berechnen</button>
      </div>
    {:else if !lib.ai?.denoiseReady}
      <p class="dn-note">Entfernt Bildrauschen mit DRUNet, ohne Details zu verwischen. Benötigt einmalig etwa {Math.round((lib.ai?.denoiseDownloadSize ?? 131e6) / 1e6)} MB.</p>
      <button class="btn small" disabled={!!lib.aiDownload} onclick={() => api.aiDownload("denoise").then(() => lib.refreshAi())}>
        {lib.aiDownload ? "Wird geladen …" : "Herunterladen"}
      </button>
    {:else}
      <Slider label="Stärke" value={dev.denoiseStrength} min={0} max={100} neutral={65} oninput={(v) => (dev.denoiseStrength = v)} oncommit={() => {}} />
      <p class="dn-note">Berechnet das Foto in voller Auflösung – je nach Größe und Rechner eine bis mehrere Minuten. Danach lässt sich die Wirkung ohne Neuberechnung abschwächen.</p>
      <button class="btn small" disabled={!!dev.denoising} onclick={() => dev.startDenoise()}>Entrauschen</button>
    {/if}
  </div>
{/snippet}

{#snippet cropSection()}
  <section class="crop">
    <div class="row">
      {#if dev.cropMode}
        <button class="btn primary" onclick={() => dev.applyCrop()} title="Übernehmen (Enter)">Fertig</button>
        <button class="btn quiet" onclick={() => dev.cancelCrop()} title="Abbrechen (Esc)">Abbrechen</button>
      {:else}
        <button class="btn" onclick={() => dev.enterCrop()} title="Zuschneiden (R)">Zuschneiden</button>
      {/if}
      <button class="btn quiet" disabled={!dev.params.crop && !dev.params.straighten} onclick={() => dev.resetCrop()}>Zurücksetzen</button>
    </div>
    {#if dev.cropMode}
      <div class="aspects" role="group" aria-label="Seitenverhältnis">
        {#each aspects as a (a.label)}
          <button class:on={aspectKey === a.label} onclick={() => setAspect(a)}>{a.label}</button>
        {/each}
        <button class="swap" title="Hoch- und Querformat tauschen" disabled={!dev.cropAspect || dev.cropAspect === 1} onclick={swapAspect}>⇄</button>
      </div>
    {/if}
    <div class="straighten">
      <Slider
        label="Geraderichten"
        value={dev.params.straighten}
        min={-45}
        max={45}
        step={0.05}
        decimals={2}
        unit="°"
        oninput={(v) => dev.set("straighten", v)}
        oncommit={() => { if (!dev.cropMode) dev.commit(); }}
      />
      <button class="btn small" onclick={() => dev.autoStraighten()} title="Horizont und Kanten automatisch gerade stellen">Auto</button>
    </div>
    <div class="row">
      <button class="btn" onclick={() => dev.rotate(-1)} title="Links drehen ([)">↺ Links</button>
      <button class="btn" onclick={() => dev.rotate(1)} title="Rechts drehen (])">↻ Rechts</button>
      <button
        class="btn"
        class:on={dev.params.flipHorizontal}
        onclick={() => { dev.set("flipHorizontal", !dev.params.flipHorizontal); dev.commit(); dev.renderBefore(); }}
      >
        Spiegeln
      </button>
    </div>
  </section>
{/snippet}

<style>
  aside {
    background: var(--panel);
    border-left: 1px solid var(--line);
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .tabs {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    gap: 2px;
    padding: 6px 12px;
    background: var(--panel);
    border-bottom: 1px solid var(--line);
  }
  .tabs button {
    flex: 1;
    padding: 5px 6px;
    border-radius: var(--radius-s);
    color: var(--muted);
    font-size: var(--step--1);
    font-weight: 600;
  }
  .tabs button:hover {
    color: var(--text);
    background: var(--raised);
  }
  .tabs button.on {
    color: var(--text);
    background: var(--raised);
    box-shadow: inset 0 -2px 0 var(--accent);
  }
  .count {
    margin-left: 5px;
    padding: 0 5px;
    border-radius: 8px;
    background: var(--line);
    font-size: 10px;
  }
  .crop .row:first-child {
    margin-top: 0;
  }
  section,
  details,
  footer {
    padding: 12px 16px;
    border-bottom: 1px solid var(--line);
  }
  .meta {
    margin: 8px 0 0;
    color: var(--faint);
    font-size: var(--step--1);
  }
  summary {
    cursor: pointer;
    font-weight: 600;
    font-size: var(--step--1);
    color: var(--muted);
    list-style: none;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  summary::before {
    content: "";
    width: 0;
    height: 0;
    border-left: 4px solid currentColor;
    border-top: 4px solid transparent;
    border-bottom: 4px solid transparent;
    transition: transform 0.12s;
  }
  details[open] > summary::before {
    transform: rotate(90deg);
  }
  summary::-webkit-details-marker {
    display: none;
  }
  .sliders {
    display: grid;
    gap: 8px;
    margin-top: 10px;
  }
  .row {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    margin-top: 10px;
  }
  footer .row {
    margin-top: 0;
  }
  .denoise {
    display: grid;
    gap: 8px;
    margin-top: 14px;
    padding-top: 12px;
    border-top: 1px dashed var(--line);
  }
  .dn-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .dn-title {
    color: var(--text);
  }
  .ki {
    font-size: 10px;
    font-weight: 600;
    color: var(--accent);
    margin-left: 4px;
  }
  .dn-note {
    margin: 0;
    color: var(--faint);
    font-size: var(--step--1);
  }
  .denoise .btn {
    justify-self: start;
  }
  .dn-again {
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: end;
    gap: 8px;
  }
  .denoise progress {
    width: 100%;
    height: 4px;
    accent-color: var(--accent);
  }
  .aspects {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 10px;
  }
  .aspects button {
    padding: 2px 8px;
    border-radius: 4px;
    border: 1px solid var(--line);
    color: var(--muted);
    font-size: var(--step--1);
  }
  .aspects button.on {
    color: var(--accent);
    border-color: var(--accent);
  }
  .aspects button:disabled {
    opacity: 0.4;
  }
  .straighten {
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: end;
    gap: 8px;
    margin-top: 10px;
  }
  .btn.small {
    padding: 3px 8px;
    font-size: var(--step--1);
  }
  .btn.on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .wide {
    width: 100%;
    justify-content: center;
    margin-top: 10px;
  }
  .snap {
    display: flex;
    gap: 6px;
    margin-top: 10px;
  }
  .snap input {
    flex: 1;
  }
  .snaps {
    list-style: none;
    padding: 0;
    margin: 8px 0 0;
    display: grid;
    gap: 2px;
  }
  .snaps li {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .snaps .name {
    flex: 1;
    text-align: left;
    padding: 4px 6px;
    border-radius: var(--radius-s);
  }
  .snaps .name:hover {
    background: var(--raised);
  }
  .when {
    color: var(--faint);
    font-size: var(--step--1);
  }
  .empty {
    color: var(--faint);
    font-size: var(--step--1);
  }
  footer {
    margin-top: auto;
    border-bottom: 0;
    border-top: 1px solid var(--line);
    position: sticky;
    bottom: 0;
    background: var(--panel);
  }
</style>
