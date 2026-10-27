<script lang="ts">
  // Reiter „Reparatur“: Bereiche auswählen (Pinsel, Objekt per Klick, Motiv),
  // mehrere Auswahlen zu einer Reparatur zusammenfassen, erst nach
  // Bestätigung mit KI füllen.
  import { api, type MaskShape, type RepairLayer } from "../lib/api";
  import { describeShape, dev, type NewMaskKind } from "../lib/develop.svelte";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";
  import { withKey } from "../lib/keymap.svelte";
  import { untrack } from "svelte";
  import Icon from "./Icon.svelte";
  import Slider from "./Slider.svelte";

  let menu = $state<{ mode: "new" | "add" | "subtract" } | null>(null);
  let renaming = $state<number | null>(null);

  const segReady = $derived(!!lib.ai?.masksReady);
  const ready = $derived(!!lib.ai?.repairReady);

  const kinds: { kind: NewMaskKind; label: string; hint: string; ai?: boolean }[] = [
    { kind: "brush", label: t("Pinsel"), hint: t("Bereich übermalen") },
    { kind: "object", label: t("Objekt"), hint: t("Auf ein Objekt klicken"), ai: true },
  ];

  const repairs = $derived(dev.params.repairs);
  const active = $derived(dev.activeMask !== null ? (repairs[dev.activeMask] ?? null) : null);

  type RepairState = "new" | "stale" | "done";
  /** Zustand je Reparatur. Der Fingerabdruck serialisiert alle Formen – nur je
   *  Verlaufsschritt (Loslassen, Rückgängig …) rechnen, nicht bei jedem
   *  Pinselpunkt (sonst quadratisch je Strich). Während eines Strichs bleibt
   *  der vorige Zustand stehen. */
  const states = $derived.by(() => {
    void dev.committedVersion;
    return untrack(() => {
      const m = new Map<string, RepairState>();
      // Mit Objektivkorrektur berechnete Flicken passen nur zum korrigierten
      // Bild; ist die Korrektur aus, setzt die Pipeline sie nicht ein
      const lensOn = dev.params.lensCorrection && !!dev.lensInfo;
      for (const r of dev.params.repairs) {
        if (!r.result) m.set(r.id, "new");
        else if (r.lens && !lensOn) m.set(r.id, "stale");
        else m.set(r.id, r.resultKey === dev.repairKey($state.snapshot(r.shapes) as MaskShape[]) ? "done" : "stale");
      }
      return m;
    });
  });
  function repairState(r: RepairLayer): RepairState {
    return states.get(r.id) ?? (r.result ? "stale" : "new");
  }

  function pick(kind: NewMaskKind) {
    const m = menu;
    menu = null;
    if (!m) return;
    if (m.mode === "new") dev.addMask(kind);
    else dev.addShape(kind, m.mode === "subtract");
  }

  function removeShape(i: number) {
    active?.shapes.splice(i, 1);
    dev.showOverlay = true;
    dev.schedule();
    dev.commit();
  }

  const hint = $derived.by(() => {
    if (dev.maskBusy) return dev.maskBusy;
    if (dev.tool === "brush") return t("Übermale, was verschwinden soll – lieber etwas großzügiger. Alt radiert.");
    if (dev.tool === "object") return t("Klicke auf das Objekt. Weitere Klicks verfeinern, Alt-Klick nimmt Bereiche heraus.");
    return null;
  });
</script>

<svelte:window onclick={() => (menu = null)} />

<section class="repair">
  <header>
    <h4>{t("Reparatur")} <span class="ki">{t("KI")}</span></h4>
    {#if ready}
      <div class="anchor">
        <button class="btn small" disabled={!!dev.repairing} onclick={(e) => { e.stopPropagation(); menu = menu?.mode === "new" ? null : { mode: "new" }; }}>
          <Icon name="plus" size={12} /> {t("Neu")}
        </button>
        {#if menu?.mode === "new"}{@render kindMenu()}{/if}
      </div>
    {/if}
  </header>

  {#if !ready}
    <p class="empty">
      {t("Entfernt Störendes – Staubflecken, Leitungen, Passanten – und füllt die Stelle passend aus der Umgebung (LaMa).")}
      {t("Benötigt einmalig etwa {n} MB.", { n: Math.round((lib.ai?.repairDownloadSize ?? 92.6e6) / 1e6) })}
    </p>
    <button class="btn small" disabled={lib.aiPending("repair")} onclick={() => api.aiDownload("repair").then(() => lib.refreshAi())}>
      {lib.aiDownload?.component === "repair" ? t("Wird geladen …") : lib.aiPending("repair") ? t("Wartet …") : t("Herunterladen")}
    </button>
  {:else if repairs.length}
    <ul class="layers">
      {#each repairs as r, i (r.id)}
        {@const st = repairState(r)}
        <li class:active={dev.activeMask === i} class:off={!r.enabled}>
          <button
            class="eye"
            aria-label={r.enabled ? t("Reparatur ausblenden") : t("Reparatur einblenden")}
            title={r.enabled ? t("Ausblenden") : t("Einblenden")}
            disabled={!r.result}
            onclick={() => { r.enabled = !r.enabled; dev.schedule(); dev.commit(); }}
          >
            <span class="dot" class:pending={st !== "done"}></span>
          </button>
          {#if renaming === i}
            <!-- svelte-ignore a11y_autofocus -->
            <input
              type="text"
              value={r.name}
              autofocus
              onkeydown={(e) => { e.stopPropagation(); if (e.key === "Enter") e.currentTarget.blur(); }}
              onblur={(e) => { r.name = e.currentTarget.value.trim() || r.name; renaming = null; dev.commit(); }}
            />
          {:else}
            <button class="name" onclick={() => dev.selectMask(dev.activeMask === i ? null : i)} ondblclick={() => (renaming = i)} title={t("Doppelklick zum Umbenennen")}>
              {r.name}
              <span class="state">{st === "new" ? t("Auswahl") : st === "stale" ? t("geändert") : ""}</span>
            </button>
          {/if}
          <button class="icon-btn" aria-label={t("Reparatur löschen")} title={t("Reparatur löschen")} disabled={!!dev.repairing} onclick={() => dev.removeMask(i)}>
            <Icon name="trash" size={13} />
          </button>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="empty">
      {t("Wähle mit „Neu“ aus, was verschwinden soll. Mehrere Pinselstriche und Objekte lassen sich zu einer Reparatur zusammenfassen; gerechnet wird erst, wenn du „Reparieren“ klickst.")}
    </p>
  {/if}

  {#if hint}
    <p class="hint" class:busy={!!dev.maskBusy}>{hint}</p>
  {/if}

  {#if ready && active}
    {@const i = dev.activeMask!}
    {@const st = repairState(active)}
    <div class="detail">
      <ul class="shapes">
        {#each active.shapes as s, k}
          <li>
            <span class="kind" class:sub={s.subtract}>{s.subtract ? "−" : "+"} {describeShape(s)}</span>
            <button class="icon-btn" aria-label={t("Auswahl entfernen")} disabled={!!dev.repairing} onclick={() => removeShape(k)}><Icon name="close" size={11} /></button>
          </li>
        {:else}
          <li class="muted">{dev.tool ? t("Noch keine Auswahl – siehe Hinweis oben.") : t("Noch keine Auswahl – füge eine hinzu.")}</li>
        {/each}
      </ul>

      <div class="row">
        <div class="anchor">
          <button class="btn small" disabled={!!dev.repairing} onclick={(e) => { e.stopPropagation(); menu = menu?.mode === "add" ? null : { mode: "add" }; }}>{t("Hinzufügen")}</button>
          {#if menu?.mode === "add"}{@render kindMenu()}{/if}
        </div>
        <div class="anchor">
          <button class="btn small" disabled={!!dev.repairing || !active.shapes.length} onclick={(e) => { e.stopPropagation(); menu = menu?.mode === "subtract" ? null : { mode: "subtract" }; }}>{t("Abziehen")}</button>
          {#if menu?.mode === "subtract"}{@render kindMenu()}{/if}
        </div>
        <button class="btn small" class:on={dev.showOverlay} onclick={() => dev.toggleOverlay()} title={withKey(t("Auswahl rot anzeigen"), "maskOverlay")}>{t("Auswahl zeigen")}</button>
      </div>

      {#if dev.tool === "brush"}
        <div class="brush">
          <div class="seg">
            <button class:on={!dev.brush.erase} onclick={() => (dev.brush.erase = false)}>{t("Malen")}</button>
            <button class:on={dev.brush.erase} onclick={() => (dev.brush.erase = true)}>{t("Radieren")}</button>
          </div>
          <Slider label={t("Größe")} value={Math.round(dev.brush.size * 1000) / 10} min={0.3} max={20} step={0.1} decimals={1} oninput={(v) => (dev.brush.size = v / 100)} oncommit={() => {}} neutral={3} />
        </div>
      {:else if active.shapes.some((s) => s.type === "brush") && !dev.repairing}
        <button class="btn small wide" onclick={() => { dev.tool = "brush"; dev.showOverlay = true; dev.schedule(); }}>{t("Weiter malen")}</button>
      {/if}

      {#if dev.repairing && dev.repairing.index === i}
        {@const p = dev.repairing}
        <p class="hint busy">{p.total ? t("Bereich {n} von {total} wird gefüllt …", { n: Math.min(p.done + 1, p.total), total: p.total }) : t("Wird vorbereitet (volle Auflösung) …")}</p>
        <progress max={p.total || 1} value={p.done}></progress>
        <button class="btn small" onclick={() => api.repairCancel()}>{t("Abbrechen")}</button>
      {:else}
        <button
          class="btn primary wide"
          disabled={!active.shapes.length || st === "done" || !!dev.repairing}
          onclick={() => dev.computeRepair(i)}
          title={t("Auswahl bestätigen und mit KI füllen")}
        >
          {st === "new" ? t("Reparieren") : st === "stale" ? t("Neu berechnen") : t("Repariert")}
        </button>
        {#if st === "stale"}
          <p class="hint">{t("Die Auswahl wurde geändert. Bis zur Neuberechnung bleibt das bisherige Ergebnis sichtbar.")}</p>
        {/if}
      {/if}
    </div>
  {/if}
</section>

{#snippet kindMenu()}
  <div class="menu" role="menu" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
    {#each kinds as k (k.kind)}
      {@const disabled = k.ai && !segReady}
      <button role="menuitem" {disabled} onclick={() => pick(k.kind)} title={disabled ? t("Benötigt die KI-Masken (unten links unter „KI“)") : k.hint}>
        {#if k.ai}<Icon name="sparkle" size={12} />{:else}<span class="sp"></span>{/if}
        <span class="l">{k.label}</span>
        <span class="h">{disabled ? t("KI-Masken fehlen") : k.hint}</span>
      </button>
    {/each}
  </div>
{/snippet}

<style>
  .repair {
    padding: 12px 16px;
    border-bottom: 1px solid var(--line);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  h4 {
    margin: 0;
    font-size: var(--step--1);
    font-weight: 600;
    color: var(--muted);
  }
  .ki {
    font-size: 10px;
    color: var(--accent);
    margin-left: 4px;
  }
  .btn.small {
    padding: 3px 8px;
    font-size: var(--step--1);
  }
  .btn.on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .btn.wide {
    width: 100%;
    justify-content: center;
  }
  .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .anchor {
    position: relative;
  }
  .menu {
    position: absolute;
    right: 0;
    top: calc(100% + 4px);
    z-index: 30;
    width: 240px;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    box-shadow: 0 12px 32px rgb(0 0 0 / 0.45);
    padding: 4px;
  }
  .row .anchor:first-child .menu {
    left: 0;
    right: auto;
  }
  .menu button {
    display: grid;
    grid-template-columns: 14px auto 1fr;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 8px;
    border-radius: var(--radius-s);
    text-align: left;
  }
  .menu button:hover:not(:disabled) {
    background: var(--line);
  }
  .menu button:disabled {
    opacity: 0.5;
  }
  .menu :global(svg) {
    color: var(--accent);
  }
  .menu .h {
    color: var(--faint);
    font-size: 11px;
    text-align: right;
  }
  .sp {
    width: 12px;
  }
  .layers {
    list-style: none;
    padding: 0;
    margin: 10px 0 0;
    display: grid;
    gap: 2px;
  }
  .layers li {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 2px 2px 4px;
    border-radius: var(--radius-s);
  }
  .layers li.active {
    background: var(--raised);
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .layers li.off .name {
    color: var(--faint);
  }
  .eye {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    border: 1.5px solid var(--muted);
  }
  li:not(.off) .dot:not(.pending) {
    background: var(--accent);
    border-color: var(--accent);
  }
  .dot.pending {
    border-style: dashed;
    border-color: var(--accent);
  }
  .name {
    flex: 1;
    display: flex;
    justify-content: space-between;
    text-align: left;
    padding: 3px 2px;
  }
  .state {
    color: var(--faint);
    font-size: 11px;
  }
  .layers input {
    flex: 1;
    padding: 2px 6px;
  }
  .empty,
  .hint {
    margin: 8px 0;
    color: var(--faint);
    font-size: var(--step--1);
  }
  .hint {
    color: var(--muted);
  }
  .hint.busy {
    color: var(--accent);
  }
  .detail {
    margin-top: 10px;
    display: grid;
    gap: 10px;
  }
  .shapes {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 2px;
    font-size: var(--step--1);
  }
  .shapes li {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .kind {
    flex: 1;
  }
  .kind.sub {
    color: #e0a07f;
  }
  .row {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .brush {
    display: grid;
    gap: 8px;
  }
  .seg {
    display: flex;
    padding: 2px;
    gap: 2px;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
  }
  .seg button {
    flex: 1;
    padding: 3px 8px;
    border-radius: 6px;
    color: var(--muted);
  }
  .seg button.on {
    background: var(--raised);
    color: var(--text);
  }
  progress {
    width: 100%;
    height: 4px;
    accent-color: var(--accent);
  }
</style>
