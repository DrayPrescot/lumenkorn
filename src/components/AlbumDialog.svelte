<script lang="ts">
  // Fotoalbum erstellen: Thema, Format, Umfang. Die Auswahl der Fotos läuft im
  // Backend (Vorschauen messen, KI-Thema), danach öffnet sich die Album-Ansicht.
  import { onMount, untrack } from "svelte";
  import type { Scope } from "../lib/api";
  import { DEFAULT_SETTINGS, FORMATS, formatName, textStyleOf, autoPages, MAX_PER_PAGE, THEME_KEYS, type AlbumSettings, type ThemeKey } from "../lib/album";
  import { album } from "../lib/album.svelte";
  import TextStyleFields from "./TextStyleFields.svelte";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";

  let { scope, title, onclose }: { scope: Scope; title: string; onclose: () => void } = $props();

  const THEMES: Record<ThemeKey, string> = {
    vacation: t("Urlaub"),
    travel: t("Reise"),
    wedding: t("Hochzeit"),
    birthday: t("Geburtstag"),
    family: t("Familie"),
    baby: t("Baby und Kind"),
    nature: t("Natur und Landschaft"),
    party: t("Feier und Party"),
    sport: t("Sport"),
    animals: t("Tiere"),
    custom: t("Eigenes Thema …"),
  };

  // Fotos, aus denen gewählt werden kann (abgelehnte zählen nie, unbearbeitete
  // RAWs nur, wenn erlaubt)
  const usable = (skipRaw: boolean) => lib.images.filter((i) => i.flag !== -1 && !(skipRaw && i.kind === "raw" && !i.edited)).length;

  const s = $state<AlbumSettings>(
    untrack(() => {
      const prev = {
        ...DEFAULT_SETTINGS,
        ...(album.state && JSON.stringify(album.state.scope) === JSON.stringify(scope) ? album.state.settings : album.lastSettings()),
      };
      return { ...prev, count: Math.max(1, Math.min(prev.count, usable(prev.skipUneditedRaw))) };
    }),
  );
  const available = $derived(usable(s.skipUneditedRaw));
  let running = $state(false);
  let autoPagesOn = $state(untrack(() => s.pages === 0));
  let manualPages = $state(untrack(() => (s.pages > 0 ? s.pages : autoPages(s.count))));

  const pages = $derived(autoPagesOn ? autoPages(s.count) : manualPages);
  const tooMany = $derived(s.count > pages * MAX_PER_PAGE);
  const tooFew = $derived(s.count < pages);
  const sizeOk = $derived(s.widthMm >= 50 && s.heightMm >= 50 && s.widthMm <= 600 && s.heightMm <= 600);
  const canStart = $derived(
    !running && sizeOk && s.count >= 1 && s.count <= available && !tooMany && !tooFew && (s.theme !== "custom" || s.customTheme.trim().length > 0),
  );

  function pickFormat(key: string) {
    s.format = key;
    const f = FORMATS.find((x) => x.key === key);
    if (f) {
      s.widthMm = f.w;
      s.heightMm = f.h;
    }
  }

  const progress = $derived(album.progress);
  const pct = $derived(progress && progress.total ? Math.round((100 * progress.done) / progress.total) : 0);
  let startBtn = $state<HTMLButtonElement>();
  onMount(() => startBtn?.focus());

  async function start() {
    if (!canStart) return;
    running = true;
    const settings = { ...$state.snapshot(s), pages: autoPagesOn ? 0 : manualPages } as AlbumSettings;
    const ok = await album.create(settings, scope, title);
    running = false;
    if (ok) onclose();
  }

  function cancel() {
    if (running) album.cancelCreate();
    onclose();
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "F11") return;
    e.stopPropagation();
    if (e.key === "Escape") cancel();
    else if (e.key === "Enter" && !running && !(e.target instanceof HTMLButtonElement)) {
      e.preventDefault();
      start();
    }
  }
</script>

<svelte:window onkeydowncapture={onkeydown} />

<div class="backdrop" role="presentation" onclick={() => !running && onclose()}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
  <form class="dialog" onclick={(e) => e.stopPropagation()} onsubmit={(e) => { e.preventDefault(); start(); }}>
    <h3>{t("Fotoalbum aus „{name}“", { name: title })}</h3>
    <p class="meta">
      {t("Lumenkorn wählt die besten Fotos zum Thema – scharf, gut belichtet, ohne Serien und Doppelte, verteilt über alle Tage – und ordnet sie auf den Seiten an. Fotos mit 4 oder 5 Sternen sind immer dabei. Danach lässt sich alles anpassen.")}
    </p>

    <fieldset disabled={running}>
      <label class="row">
        <span>{t("Thema")}</span>
        <select bind:value={s.theme}>
          {#each THEME_KEYS as k}
            <option value={k}>{THEMES[k]}</option>
          {/each}
        </select>
      </label>
      {#if s.theme === "custom"}
        <label class="row">
          <span>{t("Beschreibung")}</span>
          <input type="text" bind:value={s.customTheme} placeholder={t("z. B. Skiurlaub in den Bergen")} />
        </label>
      {/if}

      <label class="row">
        <span>{t("Format")}</span>
        <select value={s.format} onchange={(e) => pickFormat(e.currentTarget.value)}>
          {#each FORMATS as f}
            <option value={f.key}>{formatName(f.key, f.w, f.h)}</option>
          {/each}
          <option value="custom">{t("Eigenes Format …")}</option>
        </select>
      </label>
      {#if s.format === "custom"}
        <div class="row">
          <span>{t("Seitengröße")}</span>
          <span class="dims">
            <input type="number" min="50" max="600" step="1" bind:value={s.widthMm} aria-label={t("Breite in mm")} />
            ×
            <input type="number" min="50" max="600" step="1" bind:value={s.heightMm} aria-label={t("Höhe in mm")} />
            mm
          </span>
        </div>
      {/if}

      <label class="row">
        <span>{t("Anzahl Fotos")}</span>
        <span class="dims">
          <input type="number" min="1" max={available} step="1" bind:value={s.count} />
          <span class="muted">{t("von {n}", { n: available })}</span>
        </span>
      </label>
      <div class="row">
        <span></span>
        <label class="check">
          <input type="checkbox" bind:checked={s.skipUneditedRaw} />
          {t("Nur bearbeitete Fotos und JPEGs (keine unbearbeiteten RAW-Dateien)")}
        </label>
      </div>

      <div class="row">
        <span>{t("Seiten")}</span>
        <span class="dims">
          <label><input type="checkbox" bind:checked={autoPagesOn} /> {t("automatisch")}</label>
          {#if autoPagesOn}
            <span class="muted">{t("{n} Seiten", { n: pages })}</span>
          {:else}
            <input type="number" min="1" max="400" step="1" bind:value={manualPages} aria-label={t("Seitenzahl")} />
          {/if}
        </span>
      </div>

      <div class="row">
        <span>{t("Gestaltung")}</span>
        <span class="dims">
          <label><input type="radio" bind:group={s.style} value="margins" /> {t("Mit Rand")}</label>
          <label><input type="radio" bind:group={s.style} value="borderless" /> {t("Randlos")}</label>
        </span>
      </div>
      <div class="row">
        <span>{t("Hintergrund")}</span>
        <span class="dims">
          <label><input type="radio" bind:group={s.background} value="white" /> {t("Weiß")}</label>
          <label><input type="radio" bind:group={s.background} value="gray" /> {t("Hellgrau")}</label>
          <label><input type="radio" bind:group={s.background} value="black" /> {t("Schwarz")}</label>
        </span>
      </div>
      <label class="row">
        <span>{t("Datum und Ort")}</span>
        <select bind:value={s.captions}>
          <option value="both">{t("Datum und Ort")}</option>
          <option value="date">{t("Nur Datum")}</option>
          <option value="place">{t("Nur Ort")}</option>
          <option value="none">{t("Keine")}</option>
        </select>
      </label>
      <label class="row">
        <span>{t("Karte")}</span>
        <select bind:value={s.map}>
          <option value="none">{t("Keine")}</option>
          <option value="first">{t("Auf der ersten Seite")}</option>
          <option value="last">{t("Auf der letzten Seite")}</option>
        </select>
      </label>
      <label class="row">
        <span>{t("Schrift")}</span>
        <select bind:value={s.font}>
          <option value="sans">Instrument Sans</option>
          <option value="serif">Lora</option>
        </select>
      </label>
      <TextStyleFields value={textStyleOf(s)} onchange={(v) => (s.textStyle = v)} />
      <p class="meta small">{t("Schriftstil für Datum und Ort und als Vorgabe für eigene Texte – einzelne Texte lassen sich später anpassen.")}</p>
    </fieldset>

    {#if available === 0}
      <p class="warn">{t("Hier gibt es keine bearbeiteten Fotos oder JPEGs. Bearbeite zuerst ein paar Fotos oder nimm unbearbeitete RAW-Dateien dazu.")}</p>
    {:else if s.count > available}
      <p class="warn">{t("Es stehen nur {n} Fotos zur Auswahl.", { n: available })}</p>
    {:else if !sizeOk}
      <p class="warn">{t("Die Seitengröße muss zwischen 50 und 600 mm liegen.")}</p>
    {:else if tooMany}
      <p class="warn">{t("Auf {pages} Seiten passen höchstens {max} Fotos.", { pages, max: pages * MAX_PER_PAGE })}</p>
    {:else if tooFew}
      <p class="warn">{t("Für {pages} Seiten braucht es mindestens {pages} Fotos.", { pages })}</p>
    {/if}
    {#if !lib.ai?.modelReady}
      <p class="meta small">{t("Die KI ist nicht eingerichtet: Die Fotos werden nur nach Schärfe, Belichtung und Bewertung gewählt, das Thema bleibt unberücksichtigt.")}</p>
    {/if}

    {#if running}
      <div class="progress" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
        <div class="bar" style:width="{pct}%"></div>
      </div>
      <p class="meta">
        {progress && progress.total ? t("Fotos bewerten – {done} von {total}", { done: progress.done, total: progress.total }) : t("Fotos bewerten …")}
      </p>
    {:else}
      <p class="meta small">{t("Gespeichert wird als druckfertiges PDF (300 dpi, 3 mm Beschnittzugabe) und zusätzlich jede Seite als JPG.")}</p>
    {/if}

    <div class="actions">
      <button type="button" class="btn quiet" onclick={cancel}>{t("Abbrechen")}</button>
      <button bind:this={startBtn} type="submit" class="btn primary" disabled={!canStart}>{t("Album erstellen")}</button>
    </div>
  </form>
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
    width: min(560px, 100%);
    max-height: calc(100vh - 32px);
    overflow: auto;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    padding: 18px;
    display: grid;
    gap: 14px;
  }
  h3 {
    margin: 0;
    font-size: var(--step-1);
    font-weight: 600;
  }
  .meta {
    margin: 0;
    color: var(--muted);
  }
  .small {
    font-size: var(--step--1);
  }
  .warn {
    margin: 0;
    color: var(--reject);
  }
  fieldset {
    border: 0;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 10px;
  }
  .row {
    display: grid;
    grid-template-columns: 130px 1fr;
    align-items: center;
    gap: 10px;
  }
  .row > span:first-child {
    color: var(--muted);
  }
  .dims {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 12px;
  }
  .check {
    display: flex;
    align-items: flex-start;
    gap: 6px;
  }
  .dims label {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  input[type="number"] {
    width: 80px;
  }
  .muted {
    color: var(--muted);
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
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
