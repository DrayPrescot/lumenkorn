<script lang="ts">
  import { api, type AccelPreference, type Backend, type Benchmark } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import Icon from "./Icon.svelte";
  import McpSection from "./McpSection.svelte";
  import { onMount } from "svelte";
  import { t, num } from "../lib/i18n";

  let { onclose }: { onclose: () => void } = $props();

  const s = $derived(lib.ai);
  // Status beim Öffnen frisch holen (Modelle können inzwischen geladen sein)
  let dialog = $state<HTMLDivElement>();
  onMount(() => {
    void lib.refreshAi();
    void lib.refreshPeople();
    dialog?.focus();
  });

  // Solange der Dialog offen ist, erreichen Tasten die Ansichten dahinter nicht
  // (sonst bewerten/markieren X, P, 0–5 … die Fotos hinter dem Dialog) – auch
  // wenn der Fokus gerade nicht im Dialog liegt.
  function onkeydown(e: KeyboardEvent) {
    if (e.key === "F11") return; // Vollbild bleibt auch bei offenem Dialog erreichbar
    e.stopPropagation();
    if (e.key === "Escape") onclose();
  }
  const d = $derived(lib.aiDownload?.progress);
  const mb = (b: number) => `${num(Math.round(b / 1_000_000))} MB`;
  const pct = $derived(d && d.total ? Math.min(100, (d.done / d.total) * 100) : null);

  const ps = $derived(lib.peopleStatus);
  async function toggleFaces(e: Event) {
    if (!s) return;
    await api.aiSetSettings({ ...s.settings, faces: (e.currentTarget as HTMLInputElement).checked });
    await Promise.all([lib.refreshAi(), lib.refreshPeople()]);
  }

  async function download(component: "clip" | "masks" | "denoise" | "faces" | Backend = "clip") {
    try {
      await api.aiDownload(component);
      await lib.refreshAi();
    } catch (e) {
      lib.notify(String(e), "error");
    }
  }

  async function toggleAuto(e: Event) {
    if (!s) return;
    await api.aiSetSettings({ ...s.settings, autoTag: (e.currentTarget as HTMLInputElement).checked });
    await lib.refreshAi();
  }

  // --- Grafikkarte ---------------------------------------------------------
  const gb = (b: number) => `${Math.round(b / 2 ** 30)} GB`;
  const gpuInstalled = $derived(s?.gpuRuntimes.filter((g) => g.installed) ?? []);
  const gpuMissing = $derived(s?.gpuRuntimes.filter((g) => !g.installed) ?? []);
  const device = $derived(s?.accel.devices[0]);
  const dev = $derived(s?.accel.develop);
  // Name aus dem Betriebssystem (mit Speicher) bevorzugt, sonst vom Grafiktreiber
  const deviceLabel = $derived(
    device
      ? `${device.vendor} ${device.name}${device.memory ? ` · ${t("{size} Grafikspeicher", { size: gb(device.memory) })}` : ""}`
      : dev?.info && dev.info.kind !== "software"
        ? dev.info.name
        : null,
  );
  const devText = $derived.by(() => {
    if (!dev) return "";
    if (!dev.enabled) return t("Regler, Masken und Export rechnet der Prozessor.");
    if (dev.available === null) return t("Grafikkarte wird gesucht …");
    if (!dev.available) return t("Nicht verfügbar – es rechnet der Prozessor ({error}).", { error: dev.error ?? t("unbekannt") });
    const base = t("Aktiv: {name} über {backend}.", { name: String(dev.info?.name), backend: String(dev.info?.backend) });
    return dev.error ? `${base} ${t("Letzter Fehler, dabei auf den Prozessor ausgewichen: {error}", { error: dev.error })}` : base;
  });

  async function toggleDevelopGpu(e: Event) {
    if (!s) return;
    await api.aiSetSettings({ ...s.settings, developGpu: (e.currentTarget as HTMLInputElement).checked });
    await lib.refreshAi();
    // Erkennung läuft im Hintergrund – kurz danach erneut nachsehen
    setTimeout(() => void lib.refreshAi(), 1500);
  }
  const MODEL_LABELS: Record<string, string> = {
    "drunet.onnx": t("Rauschreduzierung"),
    "subject.onnx": t("Motiv"),
    "sam-encoder.onnx": t("Objekt per Klick"),
    "vision.onnx": t("Schlagworte"),
  };
  const placements = $derived(
    (s?.accel.active.placements ?? []).filter((p) => MODEL_LABELS[p.model]).map((p) => ({ ...p, label: MODEL_LABELS[p.model] })),
  );
  const activeLabel = $derived.by(() => {
    const lib_ = s?.accel.active.library;
    if (!lib_) return null;
    if (lib_ === "cpu" || !s?.accel.active.gpuEnabled) return t("Prozessor (CPU)");
    return s?.gpuRuntimes.find((g) => g.backend === lib_)?.label ?? lib_;
  });

  let bench = $state<Benchmark | null>(null);
  let benchRunning = $state(false);

  async function setAccel(e: Event) {
    if (!s) return;
    bench = null;
    await api.aiSetSettings({ ...s.settings, accel: (e.currentTarget as HTMLSelectElement).value as AccelPreference });
    await lib.refreshAi();
  }

  async function benchmark() {
    benchRunning = true;
    bench = null;
    try {
      bench = await api.aiBenchmark();
    } catch (e) {
      lib.notify(String(e), "error");
    } finally {
      benchRunning = false;
      await lib.refreshAi();
    }
  }

  const secs = (v: number) =>
    v < 1 ? `${num(v, { maximumFractionDigits: 2 })} s`
    : v < 90 ? `${Math.round(v)} s`
    : `${Math.round(v / 60)} min`;

  async function retag() {
    await api.aiRetagAll();
    await lib.refreshAi();
    lib.notify(t("Alle Fotos werden neu analysiert."));
  }
</script>

<svelte:window onkeydowncapture={onkeydown} />

<div class="backdrop" role="presentation" onclick={onclose}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
  <div
    bind:this={dialog}
    class="dialog"
    role="dialog"
    aria-label={t("KI")}
    tabindex="-1"
    onclick={(e) => e.stopPropagation()}
  >
    <header>
      <h3><Icon name="sparkle" size={15} /> {t("Lokale KI")}</h3>
      <button class="icon-btn" aria-label={t("Schließen")} onclick={onclose}><Icon name="close" size={14} /></button>
    </header>

    <p class="lead">
      {t("Lumenkorn erkennt Motive, Orte, Licht und Wetter und vergibt passende Schlagworte. Außerdem kannst du nach Bildinhalten suchen, etwa „Hund am Strand“. Alles läuft auf diesem Gerät, kein Foto verlässt den Rechner.")}
    </p>

    {#if s?.crashNotice}
      <div class="box crash">
        {#each s.crashNotice.split("\n\n") as para, i (i)}
          <p>{para}</p>
        {/each}
        <button
          class="btn"
          onclick={async () => {
            await api.aiDismissNotice();
            await lib.refreshAi();
          }}>{t("Verstanden")}</button
        >
      </div>
    {/if}

    {#if !s}
      <p class="muted">{t("Status wird geladen …")}</p>
    {:else if lib.aiDownload}
      <div class="progress">
        <div class="line">
          <span>{d ? t("{item} ({index} von {count})", { item: d.item, index: d.index, count: d.count }) : t("Verbinde …")}</span>
          <span class="muted">{d ? `${mb(d.done)}${d.total ? t(" von {size}", { size: mb(d.total) }) : ""}` : ""}</span>
        </div>
        <progress max="100" value={pct ?? undefined}></progress>
        <button class="btn quiet" onclick={() => api.aiCancelDownload()}>{t("Abbrechen")}</button>
      </div>
    {:else if !s.modelReady || !s.runtimePath}
      <div class="box">
        <p>
          {t("Benötigt einmalig etwa")} <strong>{mb(s.downloadSize)}</strong>{t(": die ONNX-Laufzeit (von GitHub) und das Bildmodell CLIP ViT-B/32 (von Hugging Face).")}
        </p>
        {#if !s.runtimeDownloadable && !s.runtimePath}
          <p class="warn">
            {t("Für diese Plattform gibt es keinen automatischen Download der Laufzeit. Lege die ONNX-Runtime-Bibliothek unter")}
            <code>{s.modelDir.replace(/clip-vit-b32$/, "runtime")}</code>{t(" ab.")}
          </p>
        {/if}
        <button class="btn primary" onclick={() => download("clip")}>{t("KI-Komponenten herunterladen")}</button>
      </div>
    {:else}
      <dl>
        <dt>{t("Analysiert")}</dt>
        <dd>
          {t("{done} von {total} Fotos", { done: s.analyzed, total: s.total })}
          {#if s.total}<progress max={s.total} value={s.analyzed}></progress>{/if}
        </dd>
        <dt>{t("Modell")}</dt>
        <dd>CLIP ViT-B/32{s.loaded ? t(", geladen") : ""}</dd>
      </dl>
      {#if s.error}
        <p class="warn">{t("Das Modell konnte nicht geladen werden: {error}", { error: s.error })}</p>
      {/if}
      <label class="check">
        <input type="checkbox" checked={s.settings.autoTag} onchange={toggleAuto} />
        {t("Neue Fotos automatisch verschlagworten")}
      </label>
      <div class="actions">
        <button class="btn" onclick={retag}>{t("Alle neu analysieren")}</button>
      </div>
    {/if}

    {#if s && !lib.aiDownload}
      <div class="sep"></div>
      <h4>{t("KI-Masken")}</h4>
      {#if s.masksReady && s.skyReady && s.partsReady}
        <p class="muted">{t("Installiert: „Motiv“, „Objekt per Klick“, KI-„Himmel“, „Person“ und „Tier“ im Entwickeln-Modus unter „Masken“.")}</p>
      {:else}
        <div class="box">
          <p>
            {t("„Motiv auswählen“ (IS-Net), „Objekt per Klick“ (Segment Anything), die KI-Himmelserkennung (skyseg) sowie Personen und Tiere (OWLv2, MediaPipe) benötigen einmalig etwa")}
            <strong>{mb(s.masksDownloadSize)}</strong> {t("von GitHub und Hugging Face. „Himmel“ funktioniert auch ohne, wird damit aber deutlich genauer.")}
          </p>
          <button class="btn primary" onclick={() => download("masks")}>{t("KI-Masken herunterladen")}</button>
        </div>
      {/if}
    {/if}

    {#if s && !lib.aiDownload}
      <div class="sep"></div>
      <h4>{t("KI-Rauschreduzierung")}</h4>
      {#if s.denoiseReady}
        <p class="muted">{t("Installiert: im Entwickeln-Modus unter „Details“.")}</p>
      {:else}
        <div class="box">
          <p>
            {t("DRUNet entfernt Bildrauschen, ohne Details zu verwischen. Lumenkorn lädt die Original-Gewichte (etwa")}
            <strong>{mb(s.denoiseDownloadSize)}</strong>{t(", MIT-Lizenz) von GitHub und bereitet das Modell lokal vor.")}
          </p>
          <button class="btn primary" onclick={() => download("denoise")}>{t("Rauschreduzierung herunterladen")}</button>
        </div>
      {/if}
    {/if}

    {#if s && !lib.aiDownload && ps}
      <div class="sep"></div>
      <h4>{t("Personenerkennung")}</h4>
      {#if ps.ready}
        <dl>
          <dt>{t("Analysiert")}</dt>
          <dd>
            {t("{done} von {total} Fotos, {faces} Gesichter", { done: ps.analyzed, total: ps.total, faces: ps.faces })}
            {#if ps.total}<progress max={ps.total} value={ps.analyzed}></progress>{/if}
          </dd>
        </dl>
        {#if ps.error}<p class="warn">{t("Modelle konnten nicht geladen werden: {error}", { error: ps.error })}</p>{/if}
        <label class="check">
          <input type="checkbox" checked={s.settings.faces} onchange={toggleFaces} />
          {t("Gesichter in neuen Fotos suchen")}
        </label>
        <div class="actions">
          <button class="btn" onclick={async () => { await api.peopleRescan(); await lib.refreshPeople(); lib.notify(t("Alle Fotos werden erneut nach Gesichtern durchsucht. Namen bleiben erhalten.")); }}>
            {t("Alle neu durchsuchen")}
          </button>
        </div>
      {:else}
        <div class="box">
          <p>
            {t("Findet Gesichter (YuNet) und gruppiert gleiche Personen (SFace), damit du sie benennen und danach filtern kannst. Einmalig etwa")}
            <strong>{mb(ps.downloadSize)}</strong> {t("aus dem OpenCV Model Zoo (MIT bzw. Apache-2.0). Gesichtsdaten bleiben auf diesem Rechner.")}
          </p>
          <button class="btn primary" onclick={() => download("faces").then(() => lib.refreshPeople())}>{t("Personenerkennung herunterladen")}</button>
        </div>
      {/if}
    {/if}

    {#if !lib.aiDownload}
      <McpSection />
    {/if}

    {#if s && !lib.aiDownload}
      <div class="sep"></div>
      <h4>{t("Grafikkarte")}</h4>
      {#if deviceLabel}
        <p class="muted">{t("Erkannt: {device}", { device: deviceLabel })}</p>
      {/if}
      <div class="sub">
        <label class="check">
          <input type="checkbox" checked={s.settings.developGpu} onchange={toggleDevelopGpu} />
          {t("Bildbearbeitung auf der Grafikkarte")}
        </label>
        <p class="muted small" class:warn={dev?.enabled && (dev.available === false || dev.error)}>{devText}</p>
      </div>
      <h5>{t("KI-Modelle")}</h5>
      {#if s.gpuRuntimes.length === 0}
        <p class="muted">{t("Auf dieser Plattform rechnet die KI auf dem Prozessor.")}</p>
      {:else if gpuInstalled.length === 0}
        <div class="box">
          <p>
            {t("Die KI braucht dafür eine GPU-fähige ONNX Runtime. Damit laufen Rauschreduzierung und Motiv-Erkennung um ein Vielfaches schneller. Funktioniert mit AMD-, NVIDIA- und Intel-Grafik über den normalen Treiber")}{s.gpuRuntimes[0].backend === "webgpu"
              ? t(" (Vulkan – unter Linux genügt Mesa, ROCm ist nicht nötig)")
              : " (DirectX 12)"}{t(". Einmalig etwa")} <strong>{mb(s.gpuRuntimes[0].downloadSize)}</strong>.
          </p>
          <div class="actions">
            <button class="btn primary" onclick={() => download(s.gpuRuntimes[0].backend)}>
              {t("GPU-Unterstützung herunterladen")}
            </button>
            {#each s.gpuRuntimes.slice(1) as alt (alt.backend)}
              <button class="btn quiet" onclick={() => download(alt.backend)}>{t("Alternative: {label}", { label: alt.label })}</button>
            {/each}
          </div>
        </div>
      {:else}
        <dl>
          <dt>{t("Rechenwerk")}</dt>
          <dd>
            <select value={s.settings.accel} onchange={setAccel}>
              <option value="auto">{t("Automatisch (Grafikkarte)")}</option>
              {#if gpuInstalled.length > 1}
                {#each gpuInstalled as g (g.backend)}
                  <option value={g.backend}>{g.label}</option>
                {/each}
              {/if}
              <option value="cpu">{t("Nur Prozessor (CPU)")}</option>
            </select>
          </dd>
          <dt>{t("Aktiv")}</dt>
          <dd>{activeLabel ?? t("– (wird beim ersten Einsatz geladen)")}</dd>
          {#if placements.length}
            <dt>{t("Modelle")}</dt>
            <dd class="chips">
              {#each placements as p (p.model)}
                <span class="chip" class:gpu={p.gpu} title={p.gpu ? t("Rechnet auf der Grafikkarte") : t("Rechnet auf dem Prozessor – nicht alle Operationen dieses Modells laufen auf der Grafikkarte")}>
                  {p.label}: {p.gpu ? "GPU" : "CPU"}
                </span>
              {/each}
            </dd>
          {/if}
        </dl>
        {#if s.accel.restartRequired}
          <div class="box">
            <p>{t("Die GPU-Laufzeit ist installiert und wird nach einem Neustart von Lumenkorn geladen.")}</p>
            <button class="btn primary" onclick={() => api.appRestart()}>{t("Jetzt neu starten")}</button>
          </div>
        {/if}
        {#if s.accel.active.gpuError}
          <p class="warn">
            {t("Die Grafikkarte konnte nicht genutzt werden, die KI rechnet auf dem Prozessor: {error}", { error: s.accel.active.gpuError })}
          </p>
        {/if}
        {#if s.denoiseReady && !s.accel.restartRequired}
          <div class="actions bench">
            <button class="btn" onclick={benchmark} disabled={benchRunning}>
              {benchRunning ? t("Messe …") : t("Geschwindigkeit testen")}
            </button>
            {#if bench}
              <span>
                {bench.gpu ? `GPU (${bench.backend})` : "CPU"}: {t("{size}×{size} Pixel in {time} – Rauschreduzierung eines 24-MP-Fotos etwa {time24}", { size: String(bench.size), time: secs(bench.seconds), time24: secs(bench.seconds24mp) })}
              </span>
            {/if}
          </div>
        {/if}
        {#if gpuMissing.length}
          <div class="actions">
            {#each gpuMissing as alt (alt.backend)}
              <button class="btn quiet" onclick={() => download(alt.backend)}>
                {t("Zusätzlich {label} laden ({size})", { label: alt.label, size: mb(alt.downloadSize) })}
              </button>
            {/each}
          </div>
        {/if}
      {/if}
    {/if}

    <p class="note">
      {t("KI-Schlagworte sind gestrichelt umrandet und lassen sich wie eigene entfernen. Eigene Schlagworte werden nie überschrieben.")}
    </p>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: grid;
    place-items: center;
    background: rgb(0 0 0 / 0.5);
    padding: 16px;
  }
  .dialog {
    width: min(520px, 100%);
    max-height: calc(100vh - 32px);
    overflow-y: auto;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    padding: 18px;
    display: grid;
    /* Spalte nie breiter als der Dialog: sonst schiebt ein langer Pfad ohne
       Leerzeichen (z. B. in der Absturzmeldung) alles seitlich heraus. */
    grid-template-columns: minmax(0, 1fr);
    gap: 14px;
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  h3 {
    margin: 0;
    font-size: var(--step-1);
    font-weight: 600;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  h3 :global(svg) {
    color: var(--accent);
  }
  p {
    margin: 0;
  }
  .lead {
    color: var(--muted);
    max-width: 60ch;
  }
  .box {
    display: grid;
    gap: 12px;
    padding: 12px;
    background: var(--raised);
    border-radius: var(--radius-m);
  }
  .box .btn {
    justify-self: start;
  }
  .progress {
    display: grid;
    gap: 8px;
  }
  .line {
    display: flex;
    justify-content: space-between;
    gap: 12px;
  }
  progress {
    width: 100%;
    height: 4px;
    appearance: none;
    border: 0;
    background: var(--line);
  }
  progress::-webkit-progress-bar {
    background: var(--line);
  }
  progress::-webkit-progress-value {
    background: var(--accent);
  }
  progress::-moz-progress-bar {
    background: var(--accent);
  }
  .progress .btn {
    justify-self: start;
  }
  dl {
    display: grid;
    grid-template-columns: 90px 1fr;
    gap: 8px 12px;
    margin: 0;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
    display: grid;
    gap: 6px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .check input {
    accent-color: var(--accent);
  }
  .actions {
    display: flex;
    gap: 8px;
  }
  .warn {
    color: #e0a07f;
  }
  .sub {
    display: grid;
    gap: 4px;
  }
  .small {
    font-size: var(--step--1);
  }
  h5 {
    margin: 4px 0 0;
    font-size: var(--step--1);
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .crash {
    border: 1px solid #e0a07f;
  }
  .crash p {
    color: #e0a07f;
    /* Pfade wie C:\Users\…\lumenkorn.log dürfen überall umbrechen */
    overflow-wrap: anywhere;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    padding: 1px 8px;
    border-radius: 999px;
    background: var(--raised);
    color: var(--muted);
    font-size: var(--step--1);
  }
  .chip.gpu {
    color: var(--accent);
  }
  .bench {
    align-items: center;
    flex-wrap: wrap;
  }
  .bench span {
    color: var(--muted);
    font-size: var(--step--1);
  }
  select {
    justify-self: start;
  }
  .sep {
    border-top: 1px solid var(--line);
  }
  h4 {
    margin: 0;
    font-size: var(--step-0);
    font-weight: 600;
  }
  .note {
    color: var(--faint);
    font-size: var(--step--1);
  }
  code {
    font-size: var(--step--1);
    word-break: break-all;
  }
</style>
