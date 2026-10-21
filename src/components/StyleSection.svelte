<script lang="ts">
  // „Auto nach deinem Stil“ in den Einstellungen: ein-/ausschalten, was
  // gelernt ist (je Kamera), welche Stufen aktiv sind, zurücksetzen.
  import { onMount } from "svelte";
  import { api, type StyleStatus } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { num, t } from "../lib/i18n";

  let status = $state<StyleStatus | null>(null);
  let confirmReset = $state(false);

  async function refresh() {
    try {
      status = await api.styleStatus();
    } catch {
      status = null;
    }
  }
  onMount(() => {
    refresh();
    // Beispiele entstehen im Hintergrund: Stand ab und zu nachladen
    const timer = setInterval(refresh, 5000);
    return () => clearInterval(timer);
  });

  async function toggle(e: Event) {
    const s = lib.ai;
    if (!s) return;
    await api.aiSetSettings({ ...s.settings, learnStyle: (e.currentTarget as HTMLInputElement).checked });
    await lib.refreshAi();
    await refresh();
  }

  async function reset() {
    await api.styleReset();
    confirmReset = false;
    await refresh();
    lib.notify(t("Der gelernte Stil ist gelöscht. Auto verhält sich wieder wie ab Werk und lernt ab jetzt neu."));
  }

  const TONE = [t("Belichtung"), t("Kontrast"), t("Lichter"), t("Tiefen"), t("Weiß"), t("Schwarz"), t("Dynamik")];
  const EXTRA = [t("Sättigung"), t("Klarheit"), t("Dunst entfernen"), t("Schärfen")];

  /** „Schwarz −12, Dynamik +8; je nach Foto: Belichtung“ */
  function describe(tone: number[], extra: (number | null)[], adaptive: boolean[]): string {
    const sign = (v: number, d = 0) => (v > 0 ? "+" : v < 0 ? "−" : "±") + num(Math.abs(v), { maximumFractionDigits: d });
    const parts = tone
      .map((v, i) => (Math.abs(v) >= (i === 0 ? 0.05 : 1) ? `${TONE[i]} ${sign(v, i === 0 ? 2 : 0)}${i === 0 ? " EV" : ""}` : ""))
      .filter(Boolean);
    extra.forEach((v, i) => {
      if (v !== null && v !== undefined) parts.push(`${EXTRA[i]} ${num(Math.round(v))}`);
    });
    const per = TONE.filter((_, i) => adaptive?.[i]);
    const fixed = parts.join(", ");
    if (!per.length) return fixed;
    const adapt = t("je nach Foto angepasst: {list}", { list: per.join(", ") });
    return fixed ? `${fixed}; ${adapt}` : adapt;
  }
</script>

<div class="sep"></div>
<h4>{t("Auto nach deinem Stil")}</h4>
<p class="muted">
  {t("„Auto“ beim Entwickeln lernt aus deinen Bearbeitungen, wie du von seinen Vorschlägen abweichst, und passt sich nach und nach an – getrennt nach Kamera und RAW/JPEG. Ein Regler wird erst angepasst, wenn das an deinen eigenen Fotos nachweislich näher an deinem Ergebnis liegt. Alles bleibt auf diesem Rechner.")}
</p>
<label class="check">
  <input type="checkbox" checked={lib.ai?.settings.learnStyle ?? true} onchange={toggle} />
  {t("Auto lernt aus meinen Bearbeitungen")}
</label>
{#if status && status.enabled}
  <dl class="style">
    <dt>{t("Ausgewertet")}</dt>
    <dd>
      {t("{n} bearbeitete Fotos, davon {tone} mit Änderungen an den Tonreglern", { n: status.samples, tone: status.toneSamples })}
      {#if status.pending}<span class="muted">· {t("{n} werden noch ausgewertet", { n: status.pending })}</span>{/if}
    </dd>
    <dt>{t("Stufen")}</dt>
    <dd class="stages">
      <span class:on={status.summary.stage2} title={t("Ab etwa 10–20 Fotos")}>{t("Ausgleich je Kamera")}</span>
      <span class:on={status.summary.stage3} title={t("Ab etwa 50 Fotos je Kamera")}>{t("abhängig von der Bildhelligkeit")}</span>
      <span class:on={status.summary.stage4} title={t("Ab etwa 300 Fotos")}>{t("ähnliche Motive")}</span>
    </dd>
  </dl>
  {@const groups = status.summary.groups.filter((g) => g.samples > 0)}
  {#if groups.length}
    <ul class="groups">
      {#each groups as g (g.group)}
        {@const d = describe(g.tone, g.extra, g.adaptive)}
        <li>
          <strong>{g.group}</strong>
          <span class="muted">({t(g.samples === 1 ? "{n} Foto" : "{n} Fotos", { n: g.samples })})</span>:
          {d || t("noch kein deutlicher Unterschied zu Auto")}
        </li>
      {/each}
    </ul>
  {:else}
    <p class="muted small">{t("Noch nichts gelernt – dafür braucht es etwa 10 bis 20 bearbeitete Fotos.")}</p>
  {/if}
  <div class="actions">
    {#if confirmReset}
      <span>{t("Gelernten Stil wirklich löschen?")}</span>
      <button class="btn quiet" onclick={() => (confirmReset = false)}>{t("Abbrechen")}</button>
      <button class="btn danger" onclick={reset}>{t("Löschen")}</button>
    {:else}
      <button class="btn" disabled={!status.samples} onclick={() => (confirmReset = true)}>{t("Gelernten Stil zurücksetzen")}</button>
    {/if}
  </div>
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
  p {
    margin: 0;
  }
  .muted {
    color: var(--muted);
  }
  .small {
    font-size: var(--step--1);
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
    align-items: center;
    gap: 8px;
  }
  .danger {
    color: var(--reject);
  }
  .style {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 12px;
    margin: 4px 0;
  }
  .style dt {
    color: var(--muted);
  }
  .style dd {
    margin: 0;
  }
  .stages {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .stages span {
    padding: 1px 8px;
    border-radius: 10px;
    border: 1px solid var(--line);
    color: var(--muted);
    font-size: var(--step--1);
  }
  .stages span.on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .groups {
    margin: 0;
    padding-left: 18px;
    display: grid;
    gap: 4px;
  }
</style>
