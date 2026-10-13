<script lang="ts">
  // Der Katalog stammt von einer neueren Lumenkorn-Version: Statt sich zu
  // beenden, sucht die App sofort nach dem Update und bietet es an. Ohne
  // gefundenes Update (offline, Entwicklungsstand) bleibt der Weg über die
  // Release-Seite.
  import { onMount } from "svelte";
  import { api, listen, type UpdateInfo } from "../lib/api";
  import { REPO_URL } from "../lib/credits";
  import { num, t } from "../lib/i18n";
  import Icon from "./Icon.svelte";

  let { message }: { message: string } = $props();

  let phase = $state<"check" | "found" | "none" | "download" | "install">("check");
  let info = $state<UpdateInfo | null>(null);
  let done = $state(0);
  let total = $state<number | null>(null);
  let error = $state<string | null>(null);

  async function check() {
    phase = "check";
    error = null;
    try {
      info = await api.updateCheck();
    } catch {
      info = null;
    }
    phase = info ? "found" : "none";
  }

  onMount(() => {
    void check();
    let off: (() => void) | undefined;
    listen("update-progress", (p) => {
      done = p.done;
      total = p.total;
      if (p.installing) phase = "install";
    }).then((u) => (off = u));
    return () => off?.();
  });

  async function install() {
    phase = "download";
    try {
      await api.updateInstall();
    } catch (e) {
      error = String(e);
      phase = "found";
    }
  }

  const mb = (b: number) => `${num(b / 1e6, { maximumFractionDigits: 1 })} MB`;
</script>

<div class="screen">
  <div class="card" role="alertdialog" aria-labelledby="too-new-title">
    <h3 id="too-new-title"><Icon name="refresh" size={16} /> {t("Lumenkorn muss aktualisiert werden")}</h3>
    <p>{message}</p>
    <p class="small">{t("Dein Katalog bleibt unverändert. Nach dem Update startet Lumenkorn neu und öffnet ihn wie gewohnt.")}</p>

    {#if phase === "check"}
      <p class="small muted">{t("Suche nach Updates …")}</p>
    {:else if phase === "found" && info}
      <p>{t("Lumenkorn {version} ist verfügbar – installiert ist {current}.", { version: info.version, current: info.current })}</p>
      {#if error}<p class="error">{error}</p>{/if}
      <div class="actions">
        <button class="btn quiet" onclick={() => api.appExit()}>{t("Beenden")}</button>
        <button class="btn primary" onclick={install}>{t("Jetzt installieren")}</button>
      </div>
    {:else if phase === "none"}
      <p class="small">{t("Es wurde kein Update gefunden (keine Verbindung zu GitHub?). Die aktuelle Version gibt es auch auf der Release-Seite.")}</p>
      <div class="actions">
        <button class="btn quiet" onclick={() => api.appExit()}>{t("Beenden")}</button>
        <button class="btn" onclick={check}>{t("Erneut suchen")}</button>
        <button class="btn primary" onclick={() => api.openUrl(`${REPO_URL}/releases/latest`)}>{t("Release-Seite öffnen")}</button>
      </div>
    {:else}
      <div class="progress" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={total ? Math.round((done / total) * 100) : undefined}>
        <span style:width={phase === "install" ? "100%" : total ? `${(done / total) * 100}%` : "30%"}></span>
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

<style>
  .screen {
    position: fixed;
    inset: 0;
    z-index: 80;
    display: grid;
    place-items: center;
    background: var(--bg);
    padding: 16px;
  }
  .card {
    width: min(520px, 100%);
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    padding: 20px;
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
  .small {
    font-size: var(--step--1);
  }
  .muted {
    color: var(--muted);
  }
  .error {
    color: var(--danger, #e5484d);
    font-size: var(--step--1);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    flex-wrap: wrap;
  }
  .progress {
    height: 4px;
    background: var(--raised);
    border-radius: 2px;
    overflow: hidden;
  }
  .progress span {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.2s;
  }
</style>
