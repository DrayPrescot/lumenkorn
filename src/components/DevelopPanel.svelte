<script lang="ts">
  import type { DevelopParams } from "../lib/api";
  import { dev } from "../lib/develop.svelte";
  import { lib } from "../lib/store.svelte";
  import Histogram from "./Histogram.svelte";
  import MaskPanel from "./MaskPanel.svelte";
  import HslSection from "./HslSection.svelte";
  import ColorGradeSection from "./ColorGradeSection.svelte";
  import RepairPanel from "./RepairPanel.svelte";
  import Icon from "./Icon.svelte";
  import Slider from "./Slider.svelte";
  import { frameSize } from "../lib/frame";
  import { api, type LensSource } from "../lib/api";
  import { untrack } from "svelte";
  import { t, locale } from "../lib/i18n";

  let { onexport }: { onexport: () => void } = $props();

  type Key = Exclude<keyof DevelopParams, "version" | "rotate" | "flipHorizontal" | "crop">;
  interface Def {
    key: Key;
    label: string;
    min: number;
    max: number;
    step?: number;
    decimals?: number;
    track?: string;
  }

  const groups: { id: string; title: string; sliders: Def[] }[] = [
    {
      id: "wb",
      title: t("Weißabgleich"),
      sliders: [
        { key: "temperature", label: t("Temperatur"), min: -100, max: 100, track: "linear-gradient(to right, #4f78c4, #8a8a8a, #d6b04a)" },
        { key: "tint", label: t("Tönung"), min: -100, max: 100, track: "linear-gradient(to right, #5aa35a, #8a8a8a, #b65aa8)" },
      ],
    },
    {
      id: "light",
      title: t("Licht"),
      sliders: [
        { key: "exposure", label: t("Belichtung"), min: -5, max: 5, step: 0.05, decimals: 2 },
        { key: "contrast", label: t("Kontrast"), min: -100, max: 100 },
        { key: "highlights", label: t("Lichter"), min: -100, max: 100 },
        { key: "shadows", label: t("Tiefen"), min: -100, max: 100 },
        { key: "whites", label: t("Weiß"), min: -100, max: 100 },
        { key: "blacks", label: t("Schwarz"), min: -100, max: 100 },
      ],
    },
    {
      id: "color",
      title: t("Farbe"),
      sliders: [
        { key: "vibrance", label: t("Dynamik"), min: -100, max: 100 },
        { key: "saturation", label: t("Sättigung"), min: -100, max: 100 },
      ],
    },
    {
      id: "details",
      title: t("Details"),
      sliders: [
        { key: "clarity", label: t("Klarheit"), min: -100, max: 100 },
        { key: "sharpen", label: t("Schärfen"), min: 0, max: 150 },
        { key: "dehaze", label: t("Dunst entfernen"), min: -100, max: 100 },
      ],
    },
  ];

  let snapName = $state("");
  const running = $derived(!!dev.denoising && dev.denoising.imageId === dev.imageId);

  // Seitenverhältnisse als Breite/Höhe; „Original“ = Verhältnis des Rahmens
  const aspects: { key: string; label: string; value: number | null | "orig" }[] = [
    { key: "free", label: t("Frei"), value: null },
    { key: "orig", label: t("Original"), value: "orig" },
    { key: "1:1", label: "1:1", value: 1 },
    { key: "4:5", label: "4:5", value: 4 / 5 },
    { key: "3:2", label: "3:2", value: 3 / 2 },
    { key: "16:9", label: "16:9", value: 16 / 9 },
  ];
  let aspectKey = $state("orig");
  const frame = $derived(frameSize(dev.params, [dev.fullSize[0] || 1, dev.fullSize[1] || 1]));

  function setAspect(a: (typeof aspects)[number]) {
    aspectKey = a.key;
    const v = a.value === "orig" ? frame[0] / frame[1] : a.value;
    dev.setAspect(v, frame);
  }

  function swapAspect() {
    if (dev.cropAspect) dev.setAspect(1 / dev.cropAspect, frame);
  }

  // --- Reiter ----------------------------------------------------------------
  type Tab = "basic" | "masks" | "repair" | "crop";
  const tabs: { id: Tab; label: string; title: string }[] = [
    { id: "basic", label: t("Basis"), title: t("Licht, Farbe, Details, Rauschreduzierung") },
    { id: "masks", label: t("Masken"), title: t("Lokale Anpassungen") },
    { id: "repair", label: t("Reparatur"), title: t("Störendes mit KI entfernen") },
    { id: "crop", label: t("Zuschnitt"), title: t("Zuschneiden, Geraderichten, Drehen (R)") },
  ];

  // Alles verwerfen: Regler, Masken, Reparaturen, Rauschreduzierung, Zuschnitt.
  // Bleibt über Strg+Z rückgängig zu machen, fragt aber vorher nach.
  function resetAll() {
    if (dev.isDefault) return;
    const ok = confirm(
      t("Alle Bearbeitungen dieses Fotos löschen und es in den Ausgangszustand zurückversetzen?\n\nRegler, Masken, Reparaturen, Rauschreduzierung und Zuschnitt gehen verloren. Schnappschüsse bleiben erhalten; Strg+Z macht es rückgängig, solange das Foto geöffnet ist."),
    );
    if (ok) dev.reset();
  }

  // Was die Objektivkorrektur bei diesem Foto kann, und woher die Daten stammen
  const lensLines = $derived.by(() => {
    const l = dev.lensInfo;
    if (!l) return [dev.raw ? t("Für dieses RAW gibt es weder Korrekturdaten der Kamera noch ein Lensfun-Profil.") : t("Nur für RAWs – JPEGs hat die Kamera schon selbst korrigiert.")];
    const label = { sony: t("Kameradaten (Sony)"), dng: t("Daten der DNG-Datei"), lensfun: "Lensfun" } as const;
    const parts: [string, LensSource | null][] = [[t("Verzeichnung"), l.distortion], [t("Farbsäume"), l.chromatic], [t("Vignettierung"), l.vignetting]];
    const lines: string[] = [];
    for (const src of ["sony", "dng", "lensfun"] as const) {
      const what = parts.filter(([, s]) => s === src).map(([n]) => n);
      const line = `${what.join(", ")}: ${label[src]}`;
      if (what.length) lines.push(line.charAt(0).toUpperCase() + line.slice(1));
    }
    if (l.lens) lines.push(l.lens);
    return lines;
  });

  function setTab(tab: Tab) {
    if (tab === dev.tab) return;
    returnTo = null; // Reiter von Hand gewählt: kein automatischer Rücksprung
    if (dev.tab === "crop" && dev.cropMode) dev.applyCrop();
    if (dev.tab === "masks" || dev.tab === "repair") {
      dev.tool = null;
      dev.selectMask(null);
      if (dev.showOverlay) dev.toggleOverlay();
    }
    dev.tab = tab;
    dev.hslPick = false;
    if (tab === "crop") dev.enterCrop();
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
  // Beim Öffnen des Zuschnitts das Seitenverhältnis vorwählen: ohne Zuschnitt
  // „Original“, sonst das passende Verhältnis des bestehenden Zuschnitts
  // (auch hochkant), ansonsten „Frei“.
  $effect(() => {
    if (!dev.cropMode || !dev.fullSize[0]) return;
    untrack(() => {
      const c = dev.params.crop;
      const orig = frame[0] / frame[1];
      const r = c ? (c.w * frame[0]) / (c.h * frame[1]) : orig;
      const near = (v: number) => Math.abs(r / v - 1) < 0.01;
      let key = "free";
      let lock: number | null = null;
      if (near(orig)) [key, lock] = ["orig", orig];
      else {
        for (const a of aspects) {
          if (typeof a.value !== "number") continue;
          if (near(a.value)) [key, lock] = [a.key, a.value];
          else if (near(1 / a.value)) [key, lock] = [a.key, 1 / a.value];
          if (lock) break;
        }
      }
      aspectKey = key;
      dev.cropAspect = lock;
    });
  });

  // Maskenwerkzeug oder Maske auf dem Bild gewählt → Reiter „Masken“
  $effect(() => {
    const cur = untrack(() => dev.tab);
    if ((dev.activeMask !== null || dev.tool) && cur !== "masks" && cur !== "repair") dev.tab = "masks";
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

  <div class="tabs" role="tablist" aria-label={t("Bearbeitung")}>
    {#each tabs as tab (tab.id)}
      <button role="tab" aria-selected={dev.tab === tab.id} class:on={dev.tab === tab.id} title={tab.title} onclick={() => setTab(tab.id)}>
        {tab.label}{#if tab.id === "masks" && dev.params.masks.length}<span class="count">{dev.params.masks.length}</span>{/if}{#if tab.id === "repair" && dev.params.repairs.length}<span class="count">{dev.params.repairs.length}</span>{/if}
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
    <section class="auto">
      <button class="btn wide" disabled={dev.autoBusy || !dev.ready} onclick={() => dev.autoTone()} title={t("Belichtung, Kontrast, Lichter, Tiefen, Weiß, Schwarz und Dynamik automatisch einstellen (Strg+U)")}>
        <Icon name="sparkle" size={13} />
        {dev.autoBusy ? t("Wird berechnet …") : t("Auto")}
      </button>
    </section>
    {#each groups as g (g.id)}
      <details open>
        <summary tabindex="-1">{g.title}</summary>
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
        {#if g.id === "details"}
          {@render denoiseSection()}
        {/if}
      </details>
    {/each}
    <!-- HSL nach Details und Rauschreduzierung: seltener gebraucht -->
    <HslSection />
    <ColorGradeSection />

    <details open>
      <summary tabindex="-1">{t("Objektiv")}</summary>
      <label class="check" class:off={!dev.lensInfo}>
        <input type="checkbox" checked={dev.params.lensCorrection} disabled={!dev.lensInfo} onchange={(e) => dev.setLens(e.currentTarget.checked)} />
        {t("Objektivkorrektur")}
      </label>
      {#each lensLines as line, i (i)}<p class="meta" class:tight={i > 0}>{line}</p>{/each}
    </details>

    <details>
      <summary>{t("Schnappschüsse")} {dev.snapshots.length ? `(${dev.snapshots.length})` : ""}</summary>
      <form class="snap" onsubmit={(e) => { e.preventDefault(); dev.snapshot(snapName); snapName = ""; }}>
        <input type="text" placeholder={t("Name, z. B. Schwarzweiß")} bind:value={snapName} onkeydown={(e) => e.stopPropagation()} />
        <button class="btn" type="submit" disabled={dev.isDefault}>{t("Festhalten")}</button>
      </form>
      <ul class="snaps">
        {#each dev.snapshots as s (s.id)}
          <li>
            <button class="name" onclick={() => dev.restore(s)} title={t("Diesen Stand übernehmen")}>{s.name}</button>
            <span class="when">{new Date(s.createdAt * 1000).toLocaleDateString(locale, { day: "numeric", month: "short" })}</span>
            <button class="icon-btn" aria-label={t("Schnappschuss löschen")} onclick={() => dev.deleteSnapshot(s)}><Icon name="close" size={12} /></button>
          </li>
        {:else}
          <li class="empty">{t("Hält den aktuellen Stand fest, um später dorthin zurückzukehren.")}</li>
        {/each}
      </ul>
    </details>

    <section class="reset">
      <button class="btn danger wide" disabled={dev.isDefault} onclick={resetAll} title={t("Alle Bearbeitungen dieses Fotos verwerfen")}>{t("Auf Original zurücksetzen …")}</button>
    </section>
  {/if}

  <footer>
    <button class="btn primary wide" onclick={onexport}>{t("Exportieren …")}</button>
  </footer>
</aside>

{#snippet denoiseSection()}
  <div class="denoise">
    <div class="dn-head">
      <span class="dn-title">{t("Rauschreduzierung")} <span class="ki">{t("KI")}</span></span>
      {#if dev.params.denoise && !running}
        <button class="icon-btn" aria-label={t("Rauschreduzierung entfernen")} title={t("Entfernen")} onclick={() => dev.removeDenoise()}><Icon name="close" size={12} /></button>
      {/if}
    </div>
    {#if running}
      {@const d = dev.denoising!}
      <p class="dn-note">
        {d.total ? t("Kachel {done} von {total}", { done: d.done, total: d.total }) : t("Wird vorbereitet …")} {t("– läuft im Hintergrund weiter.")}
      </p>
      <progress max={d.total || 1} value={d.done}></progress>
      <button class="btn small" onclick={() => api.denoiseCancel()}>{t("Abbrechen")}</button>
    {:else if dev.params.denoise}
      <Slider
        label={t("Mischung")}
        value={dev.params.denoise.amount}
        min={0}
        max={100}
        neutral={100}
        oninput={(v) => { if (dev.params.denoise) { dev.params.denoise.amount = v; dev.schedule(); } }}
        oncommit={() => dev.commit()}
      />
      <div class="dn-again">
        <Slider label={t("Stärke")} value={dev.denoiseStrength} min={0} max={100} neutral={50} oninput={(v) => (dev.denoiseStrength = v)} oncommit={() => {}} />
        <button class="btn small" onclick={() => dev.startDenoise()} title={t("Mit dieser Stärke neu berechnen")}>{t("Neu berechnen")}</button>
      </div>
    {:else if !lib.ai?.denoiseReady}
      <p class="dn-note">{t("Entfernt Bildrauschen mit DRUNet, ohne Details zu verwischen. Benötigt einmalig etwa {n} MB.", { n: Math.round((lib.ai?.denoiseDownloadSize ?? 131e6) / 1e6) })}</p>
      <button class="btn small" disabled={!!lib.aiDownload} onclick={() => api.aiDownload("denoise").then(() => lib.refreshAi())}>
        {lib.aiDownload ? t("Wird geladen …") : t("Herunterladen")}
      </button>
    {:else}
      <Slider label={t("Stärke")} value={dev.denoiseStrength} min={0} max={100} neutral={50} oninput={(v) => (dev.denoiseStrength = v)} oncommit={() => {}} />
      <p class="dn-note">{t("Berechnet das Foto in voller Auflösung – je nach Größe und Rechner eine bis mehrere Minuten. Danach lässt sich die Wirkung ohne Neuberechnung abschwächen.")}</p>
      <button class="btn small" disabled={!!dev.denoising} onclick={() => dev.startDenoise()}>{t("Entrauschen")}</button>
    {/if}
  </div>
{/snippet}

{#snippet cropSection()}
  <section class="crop">
    <div class="row">
      {#if dev.cropMode}
        <button class="btn primary" onclick={() => dev.applyCrop()} title={t("Übernehmen (Enter)")}>{t("Fertig")}</button>
        <button class="btn quiet" onclick={() => dev.cancelCrop()} title={t("Abbrechen (Esc)")}>{t("Abbrechen")}</button>
      {:else}
        <button class="btn" onclick={() => dev.enterCrop()} title={t("Zuschneiden (R)")}>{t("Zuschneiden")}</button>
      {/if}
      <button class="btn quiet" disabled={!dev.params.crop && !dev.params.straighten} onclick={() => dev.resetCrop()}>{t("Zurücksetzen")}</button>
    </div>
    {#if dev.cropMode}
      <div class="aspects" role="group" aria-label={t("Seitenverhältnis")}>
        {#each aspects as a (a.key)}
          <button class:on={aspectKey === a.key} onclick={() => setAspect(a)}>{a.label}</button>
        {/each}
        <button class="swap" title={t("Hoch- und Querformat tauschen")} disabled={!dev.cropAspect || dev.cropAspect === 1} onclick={swapAspect}>⇄</button>
      </div>
    {/if}
    <div class="straighten">
      <Slider
        label={t("Geraderichten")}
        value={dev.params.straighten}
        min={-45}
        max={45}
        step={0.05}
        decimals={2}
        unit="°"
        oninput={(v) => dev.set("straighten", v)}
        oncommit={() => { if (!dev.cropMode) dev.commit(); }}
      />
      <button class="btn small" onclick={() => dev.autoStraighten()} title={t("Horizont und Kanten automatisch gerade stellen")}>{t("Auto")}</button>
    </div>
    <div class="row">
      <button class="btn" onclick={() => dev.rotate(-1)} title={t("Links drehen ([)")}>↺ {t("Links")}</button>
      <button class="btn" onclick={() => dev.rotate(1)} title={t("Rechts drehen (])")}>↻ {t("Rechts")}</button>
      <button
        class="btn"
        class:on={dev.params.flipHorizontal}
        onclick={() => { dev.set("flipHorizontal", !dev.params.flipHorizontal); dev.commit(); dev.renderBefore(); }}
      >
        {t("Spiegeln")}
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
  .reset {
    border-bottom: 0;
  }
  .auto .wide {
    margin-top: 0;
    gap: 6px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 10px;
  }
  .check input {
    accent-color: var(--accent);
  }
  .check.off {
    color: var(--faint);
  }
  .meta.tight {
    margin-top: 2px;
  }
  .reset .wide {
    margin-top: 0;
  }
  .btn.danger:not(:disabled) {
    color: var(--reject);
    border-color: color-mix(in srgb, var(--reject) 55%, transparent);
  }
  .btn.danger:not(:disabled):hover {
    background: var(--reject);
    border-color: var(--reject);
    color: #fff;
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
