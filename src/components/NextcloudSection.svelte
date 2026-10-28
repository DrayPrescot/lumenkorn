<script lang="ts">
  // Einstellungen → Nextcloud: verbinden (Anmeldung im Browser, Login Flow
  // v2), trennen und den Ordner wählen, unter dem Projekte landen.
  import { onDestroy, onMount } from "svelte";
  import { api, listen, type NcStatus } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";

  let st = $state<NcStatus | null>(null);
  let loaded = $state(false);
  let server = $state("");
  let waiting = $state(false);
  let error = $state<string | null>(null);
  let base = $state("");

  let unlisten: (() => void) | null = null;
  onMount(async () => {
    unlisten = await listen("nextcloud-login", async (e) => {
      waiting = false;
      if (e.ok) {
        await refresh();
        lib.notify(t("Mit Nextcloud verbunden"));
      } else error = e.error;
    });
    await refresh();
  });
  onDestroy(() => unlisten?.());

  async function refresh() {
    try {
      st = await api.ncStatus();
    } catch {
      st = null;
    }
    void lib.refreshNcShared();
    base = st?.baseFolder ?? "";
    loaded = true;
  }

  async function connect() {
    error = null;
    try {
      await api.ncLoginStart(server);
      waiting = true;
    } catch (e) {
      error = String(e);
    }
  }

  async function cancel() {
    waiting = false;
    await api.ncLoginCancel().catch(() => {});
  }

  async function disconnect() {
    if (!confirm(t("Verbindung zu Nextcloud trennen? Lumenkorns App-Passwort wird dort widerrufen; hochgeladene Dateien und geteilte Links bleiben."))) return;
    try {
      await api.ncLogout();
    } catch (e) {
      lib.notify(String(e), "error");
    }
    await refresh();
  }

  async function saveBase() {
    if (!st || base.trim() === st.baseFolder) return;
    try {
      base = await api.ncSetBaseFolder(base);
      st = { ...st, baseFolder: base };
    } catch (e) {
      lib.notify(String(e), "error");
      base = st.baseFolder;
    }
  }
</script>

<p class="muted small">
  {t("Lädt die exportierten Bilder eines Projekts in einen Ordner deiner Nextcloud und teilt ihn auf Wunsch als Link. Rechtsklick auf ein Projekt → „Zu Nextcloud hochladen …“.")}
</p>

{#if !loaded}
  <p class="muted">{t("Wird geladen …")}</p>
{:else if st}
  <div class="field">
    <span class="label">{t("Verbunden")}</span>
    <p class="who">{t("{user} auf {server}", { user: st.loginName, server: st.server })}</p>
    <div class="row">
      <button class="btn" onclick={disconnect}>{t("Trennen")}</button>
    </div>
  </div>
  <div class="field">
    <label class="label" for="nc-base">{t("Ordner in Nextcloud")}</label>
    <input
      id="nc-base"
      class="text"
      bind:value={base}
      placeholder="Lumenkorn"
      spellcheck="false"
      onchange={saveBase}
      onkeydown={(e) => {
        if (e.key === "Enter") (e.currentTarget as HTMLInputElement).blur();
      }}
    />
    <p class="muted small">{t("Darin bekommt jedes Projekt einen eigenen Unterordner; beim Hochladen lässt er sich ändern.")}</p>
  </div>
{:else}
  <div class="field">
    <label class="label" for="nc-server">{t("Adresse deiner Nextcloud")}</label>
    <div class="row">
      <input
        id="nc-server"
        class="text"
        bind:value={server}
        placeholder="cloud.example.org"
        spellcheck="false"
        disabled={waiting}
        onkeydown={(e) => {
          if (e.key === "Enter" && server.trim() && !waiting) connect();
        }}
      />
      {#if waiting}
        <button class="btn" onclick={cancel}>{t("Abbrechen")}</button>
      {:else}
        <button class="btn primary" disabled={!server.trim()} onclick={connect}>{t("Verbinden")}</button>
      {/if}
    </div>
    {#if waiting}
      <p class="small">{t("Im Browser hat sich die Anmeldeseite deiner Nextcloud geöffnet. Melde dich dort an und erlaube den Zugriff – Lumenkorn wartet hier.")}</p>
    {/if}
    {#if error}<p class="small error">{error}</p>{/if}
    <p class="muted small">
      {t("Lumenkorn bekommt ein eigenes App-Passwort, das du in Nextcloud unter Einstellungen → Sicherheit jederzeit widerrufen kannst. Es liegt in der Windows-Anmeldeinformationsverwaltung, nicht im Datenordner. Lumenkorn verbindet sich nur mit diesem Server.")}
    </p>
  </div>
{/if}

<style>
  .field {
    display: grid;
    gap: 6px;
    margin-top: 12px;
  }
  .label {
    font-weight: 600;
  }
  .row {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .who {
    margin: 0;
  }
  .text {
    flex: 1;
    min-width: 0;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
    padding: 5px 9px;
    font: inherit;
    color: inherit;
  }
  .small {
    margin: 0;
    font-size: var(--step--1);
  }
  .error {
    color: var(--reject);
  }
</style>
