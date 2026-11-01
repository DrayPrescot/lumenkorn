<script lang="ts">
  // Diashow erstellen: Thema, Umfang, Dauer, Übergang. Die Auswahl der Fotos
  // läuft im Backend wie beim Fotoalbum, danach öffnet sich die Diashow-Ansicht.
  import { onMount, untrack } from "svelte";
  import type { Scope } from "../lib/api";
  import { THEME_KEYS, photoAllowed, type PhotoSource, type ThemeKey } from "../lib/album";
  import { DEFAULT_SLIDESHOW, MAP_MIN_KM, MAX_SECONDS, MIN_SECONDS, formatDuration, type SlideshowSettings } from "../lib/slideshow";
  import { slideshow } from "../lib/slideshow.svelte";
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

  // Fotos, aus denen gewählt werden kann (abgelehnte zählen nie)
  const usable = (src: PhotoSource) => lib.images.filter((i) => photoAllowed(i, src)).length;
  const SOURCES: [PhotoSource, string][] = [
    ["edited", t("Nur bearbeitete")],
    ["editedJpeg", t("Bearbeitete und JPEGs")],
    ["all", t("Alle")],
  ];

  const s = $state<SlideshowSettings>(
    untrack(() => {
      const prev = {
        ...DEFAULT_SLIDESHOW,
        ...(slideshow.state && JSON.stringify(slideshow.state.scope) === JSON.stringify(scope) ? slideshow.state.settings : slideshow.lastSettings()),
      };
      return { ...prev, count: Math.max(1, Math.min(prev.count, usable(prev.photos))) };
    }),
  );
  const available = $derived(usable(s.photos));
  let running = $state(false);
  const canStart = $derived(
    !running && s.count >= 1 && s.count <= available && s.seconds >= MIN_SECONDS && s.seconds <= MAX_SECONDS && (s.theme !== "custom" || s.customTheme.trim().length > 0),
  );

  const progress = $derived(slideshow.progress);
  const pct = $derived(progress && progress.total ? Math.round((100 * progress.done) / progress.total) : 0);
  let startBtn = $state<HTMLButtonElement>();
  onMount(() => startBtn?.focus());

  async function start() {
    if (!canStart) return;
    running = true;
    const ok = await slideshow.create($state.snapshot(s) as SlideshowSettings, scope, title);
    running = false;
    if (ok) onclose();
  }

  function cancel() {
    if (running) slideshow.cancelCreate();
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
    <h3>{t("Diashow aus „{name}“", { name: title })}</h3>
    <p class="meta">
      {t("Lumenkorn wählt die besten Fotos zum Thema – wie beim Fotoalbum: scharf, gut belichtet, ohne Serien und Doppelte, verteilt über alle Tage. Fotos mit 4 oder 5 Sternen sind immer dabei. Danach lassen sich Reihenfolge, Dauer und Texte anpassen.")}
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
        <span>{t("Anzahl Fotos")}</span>
        <span class="dims">
          <input type="number" min="1" max={available} step="1" bind:value={s.count} />
          <span class="muted">{t("von {n}", { n: available })}</span>
        </span>
      </label>
      <label class="row">
        <span>{t("Fotos")}</span>
        <select bind:value={s.photos} title={t("JPEGs: auch unbearbeitete fertige Bilddateien (JPEG, PNG, TIFF …), aber keine unbearbeiteten RAW-Dateien")}>
          {#each SOURCES as [v, label] (v)}
            <option value={v}>{label}</option>
          {/each}
        </select>
      </label>

      <label class="row">
        <span>{t("Dauer je Foto")}</span>
        <span class="dims">
          <input type="number" min={MIN_SECONDS} max={MAX_SECONDS} step="0.5" bind:value={s.seconds} />
          <span class="muted">{t("Sekunden – insgesamt etwa {total}", { total: formatDuration(s.count * (s.seconds || 0)) })}</span>
        </span>
      </label>
      <label class="row">
        <span>{t("Übergang")}</span>
        <select bind:value={s.transition}>
          <option value="fade">{t("Weich überblenden")}</option>
          <option value="slide">{t("Schieben")}</option>
          <option value="none">{t("Harter Schnitt")}</option>
        </select>
      </label>
      <div class="row">
        <span>{t("Bewegung")}</span>
        <label class="dims"><input type="checkbox" bind:checked={s.kenBurns} /> {t("Langsam zoomen und schwenken (zu Gesichtern hin)")}</label>
      </div>
      <div class="row">
        <span>{t("Karte")}</span>
        <label class="dims" title={t("Wenn der nächste Aufnahmeort mehr als {km} km entfernt ist", { km: MAP_MIN_KM })}>
          <input type="checkbox" bind:checked={s.maps} /> {t("Animierte Karte beim Ortswechsel")}
        </label>
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
    </fieldset>

    {#if available === 0}
      <p class="warn">
        {s.photos === "edited"
          ? t("Hier gibt es keine bearbeiteten Fotos. Bearbeite zuerst ein paar Fotos oder nimm JPEGs bzw. alle Fotos dazu.")
          : t("Hier gibt es keine bearbeiteten Fotos oder JPEGs. Bearbeite zuerst ein paar Fotos oder nimm unbearbeitete RAW-Dateien dazu.")}
      </p>
    {:else if s.count > available}
      <p class="warn">{t("Es stehen nur {n} Fotos zur Auswahl.", { n: available })}</p>
    {:else if !(s.seconds >= MIN_SECONDS && s.seconds <= MAX_SECONDS)}
      <p class="warn">{t("Die Dauer je Foto muss zwischen {min} und {max} Sekunden liegen.", { min: MIN_SECONDS, max: MAX_SECONDS })}</p>
    {/if}
    {#if !lib.aiOn}
      <p class="meta small">{t("Die KI ist ausgeschaltet: Die Fotos werden nur nach Schärfe, Belichtung und Bewertung gewählt, das Thema bleibt unberücksichtigt.")}</p>
    {:else if !lib.ai?.modelReady}
      <p class="meta small">{t("Die KI ist nicht eingerichtet: Die Fotos werden nur nach Schärfe, Belichtung und Bewertung gewählt, das Thema bleibt unberücksichtigt.")}</p>
    {/if}

    {#if running}
      <div class="progress" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
        <div class="bar" style:width="{pct}%"></div>
      </div>
      <p class="meta">
        {progress && progress.total ? t("Fotos bewerten – {done} von {total}", { done: progress.done, total: progress.total }) : t("Fotos bewerten …")}
      </p>
    {/if}

    <div class="actions">
      <button type="button" class="btn quiet" onclick={cancel}>{t("Abbrechen")}</button>
      <button bind:this={startBtn} type="submit" class="btn primary" disabled={!canStart}>{t("Diashow erstellen")}</button>
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
