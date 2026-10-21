<script lang="ts">
  // Update von GitHub: einmal kurz nach dem Start prüfen und fragen. „Später“
  // schließt nur – beim nächsten Start kommt die Frage wieder.
  import { onMount } from "svelte";
  import { api, listen, type UpdateInfo } from "../lib/api";
  import { dev } from "../lib/develop.svelte";
  import { lib } from "../lib/store.svelte";
  import { num, t } from "../lib/i18n";
  import Icon from "./Icon.svelte";

  let info = $state<UpdateInfo | null>(null);
  let phase = $state<"ask" | "download" | "install">("ask");
  let done = $state(0);
  let total = $state<number | null>(null);

  onMount(() => {
    // Erst nach dem Start prüfen, damit Katalog und Vorschaubilder zuerst laden
    const timer = setTimeout(async () => {
      try {
        info = await api.updateCheck();
      } catch {
        info = null;
      }
    }, 4000);
    let off: (() => void) | undefined;
    listen("update-progress", (p) => {
      done = p.done;
      total = p.total;
      if (p.installing) phase = "install";
    }).then((u) => (off = u));
    return () => {
      clearTimeout(timer);
      off?.();
    };
  });

  // Prüfen auf Wunsch (Einstellungen → Allgemein)
  $effect(() => {
    if (lib.updateRequest === 0) return;
    void (async () => {
      try {
        info = await api.updateCheck();
        if (!info) lib.notify(t("Lumenkorn ist auf dem neuesten Stand."));
      } catch (e) {
        lib.notify(t("Update-Prüfung fehlgeschlagen: {error}", { error: String(e) }), "error");
      }
    })();
  });

  /** Markdown des Releases als schlichter Text (Links ohne Adresse) */
  const notes = $derived(
    (info?.notes ?? "")
      .replace(/\[([^\]]+)\]\([^)]+\)/g, "$1")
      .replace(/^#+\s*/gm, "")
      .replace(/\*\*([^*]+)\*\*/g, "$1")
      .trim(),
  );

  async function install() {
    phase = "download";
    // Offene Bearbeitung vorher speichern – die App beendet sich beim Installieren
    await dev.flush();
    try {
      await api.updateInstall();
    } catch (e) {
      lib.notify(String(e), "error");
      phase = "ask";
    }
  }

  function later() {
    info = null;
  }

  function onkeydown(e: KeyboardEvent) {
    if (!info) return;
    if (e.key === "F11") return;
    e.stopPropagation();
    if (e.key === "Escape" && phase === "ask") later();
  }

  const mb = (b: number) => `${num(b / 1e6, { maximumFractionDigits: 1 })} MB`;
</script>

<svelte:window onkeydowncapture={onkeydown} />

{#if info}
  <div class="backdrop" role="presentation">
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="update-title">
      <h3 id="update-title"><Icon name="refresh" size={16} /> {t("Update verfügbar")}</h3>
      <p class="meta">
        {t("Lumenkorn {version} ist erschienen – installiert ist {current}.", { version: info.version, current: info.current })}
      </p>
      {#if notes}
        <pre class="notes">{notes}</pre>
      {/if}

      {#if phase === "ask"}
        <p class="small">{t("Das Update wird von GitHub geladen, auf seine Signatur geprüft und installiert; danach startet Lumenkorn neu. Katalog und Bearbeitungen bleiben erhalten.")}</p>
        <div class="actions">
          <button class="btn quiet" onclick={later}>{t("Später")}</button>
          <button class="btn primary" onclick={install}>{t("Jetzt installieren")}</button>
        </div>
      {:else}
        <div class="progress" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={total ? Math.round((done / total) * 100) : undefined}>
          <span style:width={phase === "install" ? "100%" : total ? `${(done / total) * 100}%` : "30%"} class:busy={!total && phase === "download"}></span>
        </div>
        <p class="small">
          {phase === "install"
            ? t("Wird installiert – Lumenkorn startet gleich neu …")
            : total
              ? t("Wird geladen: {done} von {total}", { done: mb(done), total: mb(total) })
              : t("Wird geladen …")}
        </p>
      {/if}
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 70;
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
  .meta {
    margin: -4px 0 0;
    color: var(--muted);
  }
  .notes {
    margin: 0;
    max-height: 240px;
    overflow: auto;
    padding: 10px 12px;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
    font: inherit;
    font-size: var(--step--1);
    white-space: pre-wrap;
    color: var(--text);
  }
  .small {
    margin: 0;
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
