<script lang="ts">
  // Aussortier-Assistent: analysiert die Fotos des Bereichs im Backend und
  // zeigt die Vorschläge zum Prüfen. Angehakte Fotos werden als abgelehnt
  // markiert, gelöscht wird hier nichts. Jede Entscheidung geht ans Backend
  // zurück, damit der Assistent lernt.
  import { onMount } from "svelte";
  import { api, imageUrl, listenWhileMounted, type CullItem, type CullResult, type Scope } from "../lib/api";
  import { judge, reasonLabel, REASONS, type Strictness, type Verdict } from "../lib/cull";
  import { lib } from "../lib/store.svelte";
  import { isCanceled, num, t } from "../lib/i18n";
  import Icon from "./Icon.svelte";

  let { scope, title, onclose }: { scope: Scope; title: string; onclose: () => void } = $props();

  const STRICTNESS_KEY = "lumenkorn.cullStrictness";
  function loadStrictness(): Strictness {
    try {
      const v = localStorage.getItem(STRICTNESS_KEY);
      if (v === "cautious" || v === "normal" || v === "strict") return v;
    } catch {
      // ohne Speicher: Standard
    }
    return "normal";
  }

  let strictness = $state<Strictness>(loadStrictness());
  let result = $state<CullResult | null>(null);
  let progress = $state({ done: 0, total: 0 });
  let running = $state(true);
  let applying = $state(false);
  let error = $state<string | null>(null);
  /** Vom Nutzer geändert: Foto → ablehnen ja/nein */
  let manual = $state<Record<number, boolean>>({});
  /** Groß angezeigtes Foto */
  let zoom = $state<number | null>(null);

  const verdicts = $derived.by(() => {
    const m = new Map<number, Verdict>();
    if (!result) return m;
    for (const it of result.items) {
      const v = judge(it, result.thresholds, strictness);
      if (v) m.set(it.id, v);
    }
    return m;
  });

  /** Serien mit mindestens einem Vorschlag oder Grenzfall, alle Fotos der Serie */
  const series = $derived.by(() => {
    const by = new Map<number, CullItem[]>();
    for (const it of result?.items ?? []) {
      if (it.series === undefined) continue;
      const list = by.get(it.series) ?? [];
      list.push(it);
      by.set(it.series, list);
    }
    return [...by.values()].filter((list) => list.some((it) => verdicts.has(it.id)));
  });

  /** Einzelne Fotos (ohne Serie) je Grund */
  const singles = $derived(
    REASONS.filter((r) => r !== "series")
      .map((reason) => ({
        reason,
        items: (result?.items ?? []).filter((it) => it.series === undefined && verdicts.get(it.id)?.reason === reason),
      }))
      .filter((g) => g.items.length),
  );

  /** Alle Fotos mit Vorschlag oder Grenzfall, in Anzeigereihenfolge */
  const shown = $derived([...series.flat().filter((it) => verdicts.has(it.id)), ...singles.flatMap((g) => g.items)]);
  /** Alle angezeigten Fotos (auch die behaltenen einer Serie) – zum Blättern in der Großansicht */
  const browse = $derived([...series.flat(), ...singles.flatMap((g) => g.items)]);
  const suggested = $derived(shown.filter((it) => verdicts.get(it.id)?.state === "suggest").length);
  const borderline = $derived(shown.length - suggested);

  function checked(id: number): boolean {
    return manual[id] ?? verdicts.get(id)?.state === "suggest";
  }
  const count = $derived(shown.filter((it) => checked(it.id)).length);

  function toggle(id: number) {
    if (!verdicts.has(id)) return;
    manual = { ...manual, [id]: !checked(id) };
  }

  function setStrictness(s: Strictness) {
    strictness = s;
    try {
      localStorage.setItem(STRICTNESS_KEY, s);
    } catch {
      // nur Bequemlichkeit
    }
  }

  onMount(() => {
    let alive = true;
    const off = listenWhileMounted("cull-progress", (p) => {
      if (alive) progress = p;
    });
    api
      .cullSuggest(scope)
      .then((r) => {
        if (alive) result = r;
      })
      .catch((e) => {
        if (alive && !isCanceled(e)) error = String(e);
      })
      .finally(() => (running = false));
    return () => {
      alive = false;
      off();
    };
  });

  function cancel() {
    if (zoom !== null) {
      zoom = null;
      return;
    }
    if (running) api.cullSuggestCancel();
    onclose();
  }

  async function apply() {
    if (applying) return;
    applying = true;
    const decisions = shown.map((it) => {
      const v = verdicts.get(it.id)!;
      return { id: it.id, reason: v.reason, severity: v.severity, reject: checked(it.id) };
    });
    const ids = decisions.filter((d) => d.reject).map((d) => d.id);
    await lib.rejectIds(ids);
    await api.cullLearn(decisions).catch((e) => console.warn("Lernen fehlgeschlagen", e));
    lib.notify(t(ids.length === 1 ? "{n} Foto als abgelehnt markiert." : "{n} Fotos als abgelehnt markiert.", { n: String(ids.length) }));
    applying = false;
    onclose();
  }

  /** In der Großansicht blättern (durch alle angezeigten Fotos) */
  function step(d: number) {
    if (zoom === null || !browse.length) return;
    const i = browse.findIndex((it) => it.id === zoom);
    zoom = browse[Math.max(0, Math.min(browse.length - 1, (i < 0 ? 0 : i) + d))].id;
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "F11") return;
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      cancel();
    } else if (zoom !== null) {
      if (e.key === "ArrowRight") step(1);
      else if (e.key === "ArrowLeft") step(-1);
      else if (e.key === "x" || e.key === "X" || e.key === " ") {
        e.preventDefault();
        toggle(zoom);
      }
    }
  }

  const pct = $derived(progress.total ? Math.round((100 * progress.done) / progress.total) : 0);
  const shortLabel = (v: Verdict) => (v.reason === "series" ? t("Serie") : reasonLabel(v.reason));
  const zoomItem = $derived(zoom === null ? null : (result?.items.find((it) => it.id === zoom) ?? null));
</script>

<svelte:window onkeydowncapture={onkeydown} />

{#snippet tile(it: CullItem)}
  {@const v = verdicts.get(it.id)}
  {@const on = v ? checked(it.id) : false}
  <div class="tile" class:on class:border={v?.state === "border"} class:kept={!v}>
    <button
      class="pick"
      disabled={!v}
      aria-pressed={v ? on : undefined}
      onclick={() => toggle(it.id)}
      title={v ? (on ? t("Wird abgelehnt – klicken zum Behalten") : t("Wird behalten – klicken zum Ablehnen")) : undefined}
    >
      <img src={imageUrl("thumb", it.id, lib.thumbVersion(it.id))} alt="" loading="lazy" />
      {#if v}<span class="box" aria-hidden="true">{#if on}<Icon name="reject" size={12} />{/if}</span>{/if}
    </button>
    <button class="zoom" onclick={() => (zoom = it.id)} title={t("Größer ansehen")} aria-label={t("Größer ansehen")}>
      <Icon name="search" size={12} />
    </button>
    <span class="label">
      {#if v}
        {v.state === "border" ? `${t("Grenzfall")} · ` : ""}{shortLabel(v)}
      {:else if it.protected}
        {t("Geschützt")}
      {:else}
        {t("Behalten")}
      {/if}
    </span>
  </div>
{/snippet}

<div class="backdrop" role="presentation" onclick={() => !applying && cancel()}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
  <div class="dialog" role="dialog" aria-modal="true" aria-label={t("Aussortieren")} tabindex="-1" onclick={(e) => e.stopPropagation()}>
    <header>
      <h3>{t("Aussortieren in „{name}“", { name: title })}</h3>
      {#if result}
        <div class="seg" role="radiogroup" aria-label={t("Strenge")}>
          {#each [["cautious", t("Vorsichtig")], ["normal", t("Normal")], ["strict", t("Streng")]] as [key, label] (key)}
            <button role="radio" aria-checked={strictness === key} class:on={strictness === key} onclick={() => setStrictness(key as Strictness)}>{label}</button>
          {/each}
        </div>
      {/if}
    </header>

    {#if running}
      <p class="meta">
        {t("Lumenkorn sucht Serien, unscharfe Fotos, geschlossene Augen, stark falsch belichtete und versehentlich ausgelöste Fotos. Fotos mit Sternen, Pick-Markierung oder Bearbeitung bleiben unangetastet.")}
      </p>
      <div class="progress" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
        <div class="bar" style:width="{pct}%"></div>
      </div>
      <p class="meta small">{t("{done} von {total} Fotos geprüft", { done: num(progress.done), total: num(progress.total) })}</p>
    {:else if error}
      <p class="error">{error}</p>
    {:else if result}
      <p class="meta">
        {#if shown.length}
          {t("{n} von {total} Fotos vorgeschlagen", { n: num(suggested), total: num(result.analyzed) })}{#if borderline}, {t("{n} Grenzfälle (nicht angehakt)", { n: num(borderline) })}{/if}.
          {t("Angehakte Fotos werden als abgelehnt markiert. Klicke ein Foto an, um es zu behalten oder doch abzulehnen; der Assistent lernt aus deinen Entscheidungen.")}
        {:else}
          {t("Keine Vorschläge – unter den {n} Fotos ist nichts Auffälliges.", { n: num(result.analyzed) })}
        {/if}
      </p>
      {#if !result.eyesChecked || !result.clipUsed}
        <p class="meta small">
          {#if !result.eyesChecked}{t("Augen werden nicht geprüft: Dafür braucht es die Gesichtserkennung und die KI-Masken für Personen (Einstellungen → KI).")}{/if}
          {#if !result.clipUsed}{t("Ohne KI-Analyse der Fotos erkennt der Assistent Serien nur am Bildvergleich und versehentliche Fotos nur, wenn sie schwarz sind.")}{/if}
        </p>
      {/if}

      <div class="list">
        {#if series.length}
          <section>
            <h4>{reasonLabel("series")} <span class="muted">({num(series.length)})</span></h4>
            <p class="meta small">{t("Aus jeder Serie bleiben je nach Größe die besten 3 bis 5 Fotos.")}</p>
            {#each series as list, i (i)}
              <div class="row series">
                {#each list as it (it.id)}{@render tile(it)}{/each}
              </div>
            {/each}
          </section>
        {/if}
        {#each singles as g (g.reason)}
          <section>
            <h4>{reasonLabel(g.reason)} <span class="muted">({num(g.items.length)})</span></h4>
            <div class="row">
              {#each g.items as it (it.id)}{@render tile(it)}{/each}
            </div>
          </section>
        {/each}
      </div>
    {/if}

    <div class="actions">
      <button class="btn quiet" onclick={cancel}>{t("Abbrechen")}</button>
      {#if result && shown.length}
        <button class="btn primary" disabled={applying} onclick={apply}>
          {t(count === 1 ? "{n} Foto ablehnen" : "{n} Fotos ablehnen", { n: num(count) })}
        </button>
      {/if}
    </div>
  </div>
</div>

{#if zoomItem}
  {@const v = verdicts.get(zoomItem.id)}
  {@const pos = browse.findIndex((it) => it.id === zoomItem.id)}
  <div class="zoomview" role="presentation" onclick={() => (zoom = null)}>
    <img src={imageUrl("preview", zoomItem.id, lib.thumbVersion(zoomItem.id))} alt="" class:rejected={v && checked(zoomItem.id)} />
    <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
    <div class="zoombar" onclick={(e) => e.stopPropagation()}>
      <span class="muted small">{pos + 1} / {browse.length}</span>
      {#if zoomItem.series !== undefined}<span class="muted small">{t("Serie, Platz {n}", { n: String(zoomItem.rank + 1) })}</span>{/if}
      <span>{v ? `${v.state === "border" ? `${t("Grenzfall")} · ` : ""}${reasonLabel(v.reason)}` : zoomItem.protected ? t("Geschützt") : t("Behalten")}</span>
      {#if v}
        <button class="btn" class:danger={checked(zoomItem.id)} onclick={() => toggle(zoomItem.id)}>
          {checked(zoomItem.id) ? t("Wird abgelehnt") : t("Wird behalten")}
        </button>
      {/if}
      <span class="muted small">{t("← → blättern · X umschalten · Esc schließen")}</span>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: grid;
    place-items: center;
    background: rgb(0 0 0 / 0.5);
    padding: 16px;
  }
  .dialog {
    width: min(1100px, 100%);
    max-height: calc(100vh - 32px);
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    padding: 18px;
    display: grid;
    grid-template-rows: auto auto auto 1fr auto;
    gap: 12px;
    min-height: 0;
  }
  header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }
  h3 {
    margin: 0;
    font-size: var(--step-1);
    font-weight: 600;
  }
  h4 {
    margin: 0 0 6px;
    font-size: var(--step-0);
    font-weight: 600;
  }
  .meta {
    margin: 0;
    color: var(--muted);
  }
  .small {
    font-size: var(--step--1);
  }
  .muted {
    color: var(--muted);
    font-weight: 400;
  }
  .error {
    margin: 0;
    color: var(--reject);
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--line);
    border-radius: var(--radius-s, 6px);
    overflow: hidden;
  }
  .seg button {
    background: none;
    border: 0;
    padding: 5px 12px;
    color: var(--muted);
    cursor: pointer;
  }
  .seg button + button {
    border-left: 1px solid var(--line);
  }
  .seg button.on {
    background: var(--raised);
    color: var(--text);
  }
  .progress {
    height: 6px;
    background: var(--raised);
    border-radius: 3px;
    overflow: hidden;
  }
  .bar {
    height: 100%;
    background: var(--accent);
    transition: width 0.2s;
  }
  .list {
    overflow: auto;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    align-content: start;
    gap: 16px;
    padding-right: 4px;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-bottom: 10px;
  }
  .tile {
    position: relative;
    flex: 0 0 132px;
    width: 132px;
    display: grid;
    gap: 3px;
  }
  .pick {
    position: relative;
    padding: 0;
    border: 2px solid transparent;
    border-radius: 6px;
    background: var(--raised);
    cursor: pointer;
    height: 100px;
    overflow: hidden;
  }
  .pick:disabled {
    cursor: default;
  }
  .pick img {
    width: 100%;
    height: 100%;
    object-fit: contain;
    display: block;
  }
  .tile.on .pick {
    border-color: var(--reject);
  }
  .tile.border .pick {
    border-style: dashed;
    border-color: var(--line);
  }
  .tile.kept .pick {
    border-color: color-mix(in srgb, var(--accent) 60%, transparent);
  }
  .box {
    position: absolute;
    top: 5px;
    left: 5px;
    width: 18px;
    height: 18px;
    border-radius: 4px;
    border: 1.5px solid #fff;
    background: rgb(0 0 0 / 0.35);
    display: grid;
    place-items: center;
    color: #fff;
  }
  .tile.on .box {
    background: var(--reject);
    border-color: var(--reject);
  }
  .zoom {
    position: absolute;
    top: 5px;
    right: 5px;
    width: 22px;
    height: 22px;
    border: 0;
    border-radius: 4px;
    background: rgb(0 0 0 / 0.45);
    color: #fff;
    display: grid;
    place-items: center;
    cursor: pointer;
    opacity: 0;
  }
  .tile:hover .zoom,
  .zoom:focus-visible {
    opacity: 1;
  }
  .label {
    font-size: var(--step--1);
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tile.on .label {
    color: var(--text);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .btn.danger {
    color: var(--reject);
    border-color: var(--reject);
  }
  .zoomview {
    position: fixed;
    inset: 0;
    z-index: 60;
    background: rgb(0 0 0 / 0.88);
    display: grid;
    grid-template-rows: 1fr auto;
    padding: 16px;
    gap: 10px;
  }
  .zoomview img {
    width: 100%;
    height: 100%;
    min-height: 0;
    object-fit: contain;
    outline: 3px solid color-mix(in srgb, var(--accent) 70%, transparent);
    outline-offset: -3px;
  }
  .zoomview img.rejected {
    outline-color: var(--reject);
  }
  .zoombar {
    justify-self: center;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    padding: 8px 14px;
  }
</style>
