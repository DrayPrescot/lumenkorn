<script lang="ts">
  // Einmal nach dem Start: Ist die KI an, eine Grafikkarte da, die GPU-Laufzeit
  // aber nicht geladen oder auf „Nur Prozessor“ gestellt, fragen, ob sie
  // geladen und eingeschaltet werden soll – sonst rechnet die KI unbemerkt
  // viel langsamer auf dem Prozessor. „Nein“ wird gemerkt, Escape nicht.
  import { api, type AiStatus } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { num, t } from "../lib/i18n";
  import Icon from "./Icon.svelte";

  let open = $state(false);
  let phase = $state<"ask" | "download" | "restart">("ask");
  let checked = false;

  /** Braucht es die Frage? `null`: nein, sonst was fehlt */
  function need(s: AiStatus): "download" | "enable" | null {
    const st = s.settings;
    if (!st.enabled || st.gpuPromptDeclined) return null;
    // Keine GPU-Laufzeit für diese Plattform oder keine Grafikkarte erkannt
    if (!s.gpuRuntimes.length || !s.accel.devices.length) return null;
    if (!s.gpuRuntimes.some((g) => g.installed)) return "download";
    return st.accel === "cpu" ? "enable" : null;
  }

  const what = $derived(lib.ai ? need(lib.ai) : null);
  const runtime = $derived(lib.ai?.gpuRuntimes[0]);
  const device = $derived(lib.ai?.accel.devices[0]);
  const mb = (b: number) => `${num(Math.round(b / 1e6))} MB`;

  // Nur beim ersten Status nach dem Start prüfen, kurz verzögert, damit
  // Katalog und Vorschaubilder zuerst laden
  $effect(() => {
    if (checked || !lib.ai) return;
    checked = true;
    setTimeout(() => {
      if (lib.ai && need(lib.ai)) open = true;
    }, 2000);
  });

  // Download fertig: Ist schon eine Laufzeit (für den Prozessor) geladen,
  // wird die neue erst nach einem Neustart genommen
  let seen = false;
  $effect(() => {
    if (phase !== "download" || !lib.ai) return;
    if (lib.aiPending(runtime?.backend ?? "webgpu")) {
      seen = true;
      return;
    }
    if (lib.ai.gpuRuntimes.some((g) => g.installed)) finish();
    // Gescheitert: Die Fehlermeldung kommt vom Store, der Dialog geht zu
    else if (seen) open = false;
  });

  async function yes() {
    const s = lib.ai;
    if (!s || !runtime) return;
    try {
      if (s.settings.accel === "cpu") await api.aiSetSettings({ ...s.settings, accel: "auto" });
      if (what === "download") {
        phase = "download";
        await api.aiDownload(runtime.backend);
        await lib.refreshAi();
      } else {
        await lib.refreshAi();
        finish();
      }
    } catch (e) {
      lib.notify(String(e), "error");
      open = false;
    }
  }

  function finish() {
    if (lib.ai?.accel.restartRequired) phase = "restart";
    else {
      open = false;
      lib.notify(t("Die KI rechnet jetzt auf der Grafikkarte."));
    }
  }

  async function no() {
    open = false;
    if (!lib.ai) return;
    try {
      await api.aiSetSettings({ ...lib.ai.settings, gpuPromptDeclined: true });
      await lib.refreshAi();
    } catch (e) {
      lib.notify(String(e), "error");
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (!open || e.key === "F11") return;
    // Auch die Einstellungen dahinter (nach einem Absturz offen) nicht erreichen
    e.stopImmediatePropagation();
    // Escape schließt nur – beim nächsten Start kommt die Frage wieder
    if (e.key === "Escape" && phase !== "download") open = false;
  }
</script>

<svelte:window onkeydowncapture={onkeydown} />

{#if open && runtime}
  <div class="backdrop" role="presentation">
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="gpu-title">
      <h3 id="gpu-title"><Icon name="sparkle" size={16} /> {t("KI auf der Grafikkarte rechnen lassen?")}</h3>
      {#if device}
        <p class="meta">{t("Erkannt: {device}", { device: `${device.vendor} ${device.name}` })}</p>
      {/if}

      {#if phase === "ask"}
        <p>
          {what === "download"
            ? t("Die KI rechnet bisher auf dem Prozessor. Mit der GPU-Unterstützung laufen Schlagworte, Masken und Rauschreduzierung um ein Vielfaches schneller. Lumenkorn lädt dafür einmalig etwa {size} ({label}) und nutzt den normalen Grafiktreiber.", { size: mb(runtime.downloadSize), label: runtime.label })
            : t("Die GPU-Unterstützung ist installiert, die KI ist aber auf „Nur Prozessor“ gestellt. Auf der Grafikkarte laufen Schlagworte, Masken und Rauschreduzierung um ein Vielfaches schneller.")}
        </p>
        <p class="small">{t("Später lässt sich das unter Einstellungen → Grafikkarte ändern.")}</p>
        <div class="actions">
          <button class="btn quiet" onclick={no}>{t("Nein, nicht mehr fragen")}</button>
          <button class="btn primary" onclick={yes}>{what === "download" ? t("Herunterladen und einschalten") : t("Einschalten")}</button>
        </div>
      {:else if phase === "download"}
        {@const d = lib.aiDownload?.component === runtime.backend ? lib.aiDownload.progress : null}
        <div class="progress" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={d?.total ? Math.round((d.done / d.total) * 100) : undefined}>
          <span style:width={d?.total ? `${(d.done / d.total) * 100}%` : "30%"} class:busy={!d?.total}></span>
        </div>
        <p class="small">
          {d?.total ? t("Wird geladen: {done} von {total}", { done: mb(d.done), total: mb(d.total) }) : lib.aiPending(runtime.backend) ? t("Wird geladen …") : t("Wartet …")}
        </p>
        <div class="actions">
          <button class="btn quiet" onclick={() => { open = false; }}>{t("Im Hintergrund weiter")}</button>
        </div>
      {:else}
        <p>{t("Die GPU-Unterstützung ist eingerichtet und wird nach einem Neustart von Lumenkorn geladen.")}</p>
        <div class="actions">
          <button class="btn quiet" onclick={() => (open = false)}>{t("Später")}</button>
          <button class="btn primary" onclick={() => api.appRestart()}>{t("Jetzt neu starten")}</button>
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 65;
    display: grid;
    place-items: center;
    background: rgb(0 0 0 / 0.5);
    padding: 16px;
  }
  .dialog {
    width: min(480px, 100%);
    max-height: calc(100vh - 32px);
    overflow: auto;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    padding: 18px;
    display: grid;
    gap: 12px;
  }
  h3 {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--step-1);
    font-weight: 600;
  }
  h3 :global(svg) {
    color: var(--accent);
  }
  p {
    margin: 0;
  }
  .meta {
    margin: -4px 0 0;
    color: var(--muted);
  }
  .small {
    font-size: var(--step--1);
    color: var(--faint);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .progress {
    height: 6px;
    border-radius: 3px;
    background: var(--raised);
    overflow: hidden;
  }
  .progress span {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.2s;
  }
  .progress span.busy {
    animation: slide 1.2s ease-in-out infinite;
  }
  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(340%);
    }
  }
</style>
