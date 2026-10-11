<script lang="ts">
  // KI-Dialog → „KI-Agenten (MCP)“: Schnittstelle ein/aus und die Angaben,
  // die Claude, ChatGPT/Codex & Co. zum Verbinden brauchen.
  import { onMount } from "svelte";
  import { api, type McpStatus } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";

  let st = $state<McpStatus | null>(null);
  let busy = $state(false);
  let showToken = $state(false);
  let tab = $state<"desktop" | "code" | "codex">("desktop");

  onMount(async () => {
    try {
      st = await api.mcpStatus();
    } catch {
      st = null;
    }
  });

  async function toggle(e: Event) {
    const on = (e.currentTarget as HTMLInputElement).checked;
    busy = true;
    try {
      st = await api.mcpSetEnabled(on);
    } catch (err) {
      lib.notify(String(err), "error");
    }
    busy = false;
  }

  async function newToken() {
    if (!confirm(t("Neuen Zugangsschlüssel erzeugen? Agenten, die über HTTP verbunden sind, brauchen danach den neuen Schlüssel."))) return;
    try {
      st = await api.mcpNewToken();
      lib.notify(t("Neuer Schlüssel erzeugt"));
    } catch (err) {
      lib.notify(String(err), "error");
    }
  }

  /** Pfad für JSON/TOML (Backslashes unter Windows verdoppeln) */
  const esc = (s: string) => s.replace(/\\/g, "\\\\");

  const snippet = $derived.by(() => {
    if (!st) return "";
    if (tab === "desktop")
      return `{\n  "mcpServers": {\n    "lumenkorn": {\n      "command": "${esc(st.command)}",\n      "args": ["--mcp"]\n    }\n  }\n}`;
    if (tab === "codex") return `[mcp_servers.lumenkorn]\ncommand = "${esc(st.command)}"\nargs = ["--mcp"]`;
    return `claude mcp add --transport http lumenkorn ${st.url ?? "http://127.0.0.1:47823/mcp"} --header "Authorization: Bearer ${st.token}"`;
  });
  const where = $derived(
    tab === "desktop"
      ? t("In die Datei claude_desktop_config.json (Claude Desktop → Einstellungen → Entwickler → Konfiguration bearbeiten), dann Claude neu starten.")
      : tab === "codex"
        ? t("In ~/.codex/config.toml (OpenAI Codex). Andere Agenten mit stdio-Servern brauchen dieselben zwei Angaben: Programm und --mcp.")
        : t("Im Terminal ausführen (Claude Code). Andere Agenten mit HTTP-Unterstützung nutzen dieselbe Adresse und denselben Schlüssel."),
  );

  async function copy() {
    try {
      await navigator.clipboard.writeText(snippet);
      lib.notify(t("Kopiert"));
    } catch {
      lib.notify(t("Kopieren nicht möglich – bitte von Hand markieren."), "error");
    }
  }
</script>

<div class="sep"></div>
<h4>{t("KI-Agenten (MCP)")}</h4>
<p class="muted">
  {t("Über das Model Context Protocol können KI-Agenten wie Claude oder ChatGPT/Codex Lumenkorn bedienen: Fotos suchen und ansehen, bewerten, verschlagworten, entwickeln, Masken anlegen und exportieren. Löschen können sie nichts. Die Schnittstelle ist nur auf diesem Rechner erreichbar.")}
</p>
{#if st}
  <label class="check">
    <input type="checkbox" checked={st.enabled} disabled={busy} onchange={toggle} />
    {t("Steuerung durch KI-Agenten erlauben")}
  </label>
  {#if st.enabled}
    <dl class="mcp">
      <dt>{t("Adresse")}</dt>
      <dd><code>{st.url ?? t("startet …")}</code></dd>
      <dt>{t("Schlüssel")}</dt>
      <dd>
        <code>{showToken ? st.token : "••••••••••••"}</code>
        <button class="btn small quiet" onclick={() => (showToken = !showToken)}>{showToken ? t("Verbergen") : t("Zeigen")}</button>
        <button class="btn small quiet" onclick={newToken}>{t("Neu")}</button>
      </dd>
    </dl>
    <div class="tabs" role="tablist">
      <button role="tab" aria-selected={tab === "desktop"} class:on={tab === "desktop"} onclick={() => (tab = "desktop")}>Claude Desktop</button>
      <button role="tab" aria-selected={tab === "code"} class:on={tab === "code"} onclick={() => (tab = "code")}>Claude Code</button>
      <button role="tab" aria-selected={tab === "codex"} class:on={tab === "codex"} onclick={() => (tab = "codex")}>OpenAI Codex</button>
    </div>
    <p class="muted small">{where}</p>
    <pre class="snippet">{snippet}</pre>
    <div class="actions">
      <button class="btn" onclick={copy}>{t("Kopieren")}</button>
    </div>
  {/if}
{/if}

<style>
  /* wie im KI-Dialog */
  .sep {
    border-top: 1px solid var(--line);
  }
  h4 {
    margin: 0;
    font-size: var(--step-0);
    font-weight: 600;
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
  .mcp {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 12px;
    margin: 8px 0;
    align-items: center;
  }
  .mcp dt {
    color: var(--muted);
  }
  .mcp dd {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  code {
    font-size: var(--step--1);
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tabs {
    display: flex;
    gap: 4px;
    margin: 10px 0 4px;
  }
  .tabs button {
    padding: 4px 10px;
    border-radius: var(--radius-s);
    border: 1px solid var(--line);
    color: var(--muted);
    font-size: var(--step--1);
  }
  .tabs button.on {
    color: var(--text);
    border-color: var(--accent);
  }
  .snippet {
    margin: 6px 0;
    padding: 10px 12px;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-all;
  }
  .small {
    font-size: var(--step--1);
  }
</style>
