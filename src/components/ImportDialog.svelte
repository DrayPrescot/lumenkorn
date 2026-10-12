<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { api, isTauri, listen, pickFolder, type ImportItem } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { lang, num, t } from "../lib/i18n";
  import PathDialog from "./PathDialog.svelte";

  let { onclose }: { onclose: () => void } = $props();

  let source = $state<string | null>(null);
  let cards = $state<string[]>([]);
  let target = $state<string | null>(null);
  let event = $state("");
  let deleteAfter = $state(false);
  let items = $state.raw<ImportItem[]>([]);
  const selected = new SvelteSet<number>();
  let scanning = $state(false);
  let found = $state(0);
  let pathFor = $state<"source" | "target" | null>(null);
  let started = $state(false);

  const progress = $derived(lib.importProgress);

  // --- Einlesen ---------------------------------------------------------------

  let scanToken = 0;
  async function scan(path: string) {
    const token = ++scanToken;
    source = path;
    scanning = true;
    found = 0;
    resetPreviews();
    items = [];
    selected.clear();
    try {
      const res = await api.importScan(path);
      if (token !== scanToken) return;
      items = res;
      // Neue Fotos vorausgewählt, schon importierte nicht
      for (const i of res) if (!i.already) selected.add(i.id);
    } catch (e) {
      if (token === scanToken) lib.notify(String(e), "error");
    } finally {
      if (token === scanToken) scanning = false;
    }
  }

  let unlisten: (() => void) | undefined;
  onMount(async () => {
    unlisten = await listen("import-scan-progress", (p) => (found = p.found));
    try {
      const d = await api.importDefaults();
      cards = d.cards;
      target = d.settings.target;
      deleteAfter = d.settings.deleteAfter;
      // Eingesteckte Karte zuerst, sonst die zuletzt benutzte Quelle
      const first = d.cards[0] ?? d.settings.source;
      if (first) scan(first);
    } catch (e) {
      lib.notify(String(e), "error");
    }
  });

  async function choose(which: "source" | "target") {
    const title = which === "source" ? t("Fotos importieren von …") : t("Zielordner für den Import");
    const d = isTauri ? await pickFolder(title) : null;
    if (d) {
      if (which === "source") scan(d);
      else target = d;
    } else if (!isTauri || navigator.userAgent.includes("Android")) pathFor = which;
  }

  // --- Vorschauen: nur sichtbare laden, höchstens vier gleichzeitig --------------

  let previews = $state<Record<number, string>>({});
  let generation = 0;
  const requested = new Set<number>();
  const queue: number[] = [];
  let active = 0;

  function resetPreviews() {
    generation++;
    for (const u of Object.values(previews)) URL.revokeObjectURL(u);
    previews = {};
    requested.clear();
    queue.length = 0;
  }

  function want(id: number) {
    if (requested.has(id)) return;
    requested.add(id);
    queue.push(id);
    pump();
  }

  function pump() {
    while (active < 4 && queue.length) {
      const id = queue.shift()!;
      const gen = generation;
      active++;
      api
        .importPreview(id)
        .then((b) => {
          if (gen === generation) previews[id] = URL.createObjectURL(b);
        })
        .catch(() => {})
        .finally(() => {
          active--;
          pump();
        });
    }
  }

  const observer =
    typeof IntersectionObserver === "undefined"
      ? null
      : new IntersectionObserver(
          (entries) => {
            for (const e of entries) if (e.isIntersecting) want(Number((e.target as HTMLElement).dataset.id));
          },
          { rootMargin: "400px" },
        );

  function lazy(node: HTMLElement, id: number) {
    node.dataset.id = String(id);
    if (observer) observer.observe(node);
    else want(id);
    return { destroy: () => observer?.unobserve(node) };
  }

  onDestroy(() => {
    unlisten?.();
    observer?.disconnect();
    resetPreviews();
  });

  // --- Tage und Auswahl ----------------------------------------------------------

  const days = $derived.by(() => {
    const m = new Map<string, ImportItem[]>();
    for (const i of items) {
      const d = i.capturedAt.slice(0, 10);
      let list = m.get(d);
      if (!list) m.set(d, (list = []));
      list.push(i);
    }
    return [...m.entries()];
  });

  const dayLabel = (d: string) =>
    new Date(`${d}T12:00:00`).toLocaleDateString(lang === "de" ? "de-DE" : "en-GB", {
      weekday: "short",
      day: "numeric",
      month: "long",
      year: "numeric",
    });

  function toggle(id: number) {
    if (selected.has(id)) selected.delete(id);
    else selected.add(id);
  }

  function setDay(list: ImportItem[], on: boolean) {
    for (const i of list) {
      if (on) selected.add(i.id);
      else selected.delete(i.id);
    }
  }

  function selectAll(mode: "all" | "none" | "new") {
    selected.clear();
    if (mode !== "none") for (const i of items) if (mode === "all" || !i.already) selected.add(i.id);
  }

  const picked = $derived(items.filter((i) => selected.has(i.id)));
  const bytes = $derived(picked.reduce((n, i) => n + i.size, 0));
  const fmt = (v: number, d: number) => num(v, { maximumFractionDigits: d });
  const size = (b: number) =>
    b >= 1e9 ? `${fmt(b / 1e9, 1)} GB` : b >= 1e6 ? `${fmt(b / 1e6, 0)} MB` : `${fmt(Math.max(1, b / 1e3), 0)} KB`;

  // Ordnername wie im Backend (`import::clean_event`)
  const cleanEvent = (e: string) =>
    e
      .trim()
      .replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_")
      .slice(0, 60)
      .replace(/[. ]+$/, "");
  const sep = $derived(target?.includes("\\") ? "\\" : "/");
  /** Beispiel-Zielordner des ersten gewählten Tages */
  const example = $derived.by(() => {
    if (!target || !picked.length) return null;
    const d = picked[0].capturedAt.slice(0, 10);
    const ev = cleanEvent(event);
    const dir = `${d.slice(0, 4)}${sep}${d.replaceAll("-", ".")}${ev ? `_${ev}` : ""}`;
    const daysCount = new Set(picked.map((i) => i.capturedAt.slice(0, 10))).size;
    return { path: `${target.replace(/[\\/]+$/, "")}${sep}${dir}`, more: daysCount - 1 };
  });

  // --- Import ----------------------------------------------------------------------

  async function start() {
    if (!target || !picked.length || lib.importRunning) return;
    started = true;
    lib.importRunning = true;
    lib.importProgress = { done: 0, total: picked.length, bytes: 0, bytesTotal: bytes, current: "" };
    try {
      await api.importStart(
        picked.map((i) => i.id),
        target,
        event,
        deleteAfter,
      );
    } catch (e) {
      started = false;
      lib.importRunning = false;
      lib.importProgress = null;
      lib.notify(String(e), "error");
    }
  }

  // Fertig: schließen, die Zusammenfassung meldet der Store
  $effect(() => {
    if (started && !lib.importRunning) onclose();
  });

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "F11") return;
    if (pathFor) return;
    e.stopPropagation();
    if (e.key === "Escape") onclose(); // ein laufender Import geht im Hintergrund weiter
  }
</script>

<svelte:window onkeydowncapture={onkeydown} />

<div class="backdrop" role="presentation" onclick={onclose}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
  <form class="dialog" onclick={(e) => e.stopPropagation()} onsubmit={(e) => { e.preventDefault(); start(); }}>
    <header>
      <h3>{t("Fotos importieren")}</h3>
      <div class="line">
        <span class="label">{t("Von")}</span>
        <span class="dir" title={source ?? ""}>{source ?? t("Noch keine Quelle gewählt")}</span>
        <button type="button" class="btn small" onclick={() => choose("source")} disabled={started}>{t("Wählen …")}</button>
      </div>
      {#if cards.length > 1 || (cards.length === 1 && cards[0] !== source)}
        <div class="line cards">
          <span class="label">{t("Karten")}</span>
          {#each cards as c (c)}
            <button type="button" class="chip" class:active={c === source} onclick={() => scan(c)} disabled={started}>{c}</button>
          {/each}
        </div>
      {/if}
    </header>

    <div class="body">
      {#if scanning}
        <p class="state">{found ? t("{n} Aufnahmen gefunden …", { n: found }) : t("Karte wird gelesen …")}</p>
      {:else if source && !items.length}
        <p class="state">{t("Keine Fotos in diesem Ordner.")}</p>
      {:else if !source}
        <p class="state">{t("Wähle die Speicherkarte oder ihren Ordner „DCIM“.")}</p>
      {:else}
        <div class="bar">
          <span>{t("{n} Aufnahmen", { n: items.length })}</span>
          <span class="sep">·</span>
          <button type="button" class="link" onclick={() => selectAll("new")}>{t("Nur neue")}</button>
          <button type="button" class="link" onclick={() => selectAll("all")}>{t("Alle")}</button>
          <button type="button" class="link" onclick={() => selectAll("none")}>{t("Keine")}</button>
        </div>
        {#each days as [day, list] (day)}
          {@const on = list.filter((i) => selected.has(i.id)).length}
          {@const done = list.filter((i) => i.already).length}
          <section>
            <label class="day">
              <input
                type="checkbox"
                checked={on === list.length}
                indeterminate={on > 0 && on < list.length}
                onchange={(e) => setDay(list, e.currentTarget.checked)}
                disabled={started}
              />
              <span class="date">{dayLabel(day)}</span>
              <span class="count">
                {t(list.length === 1 ? "{n} Foto" : "{n} Fotos", { n: list.length })}{done ? ` · ${t("{n} schon importiert", { n: done })}` : ""}
              </span>
            </label>
            <div class="tiles">
              {#each list as it (it.id)}
                <button
                  type="button"
                  class="tile"
                  class:on={selected.has(it.id)}
                  class:already={it.already}
                  use:lazy={it.id}
                  onclick={() => toggle(it.id)}
                  disabled={started}
                  title={`${it.name}${it.extra.length ? ` + ${it.extra.join(", ")}` : ""}\n→ ${it.newName}${it.dateFromFile ? `\n${t("Ohne Aufnahmedatum – Änderungsdatum der Datei verwendet")}` : ""}${it.already ? `\n${t("Schon in der Bibliothek")}` : ""}`}
                >
                  {#if previews[it.id]}<img src={previews[it.id]} alt="" draggable="false" />{/if}
                  <span class="check" aria-hidden="true"></span>
                  {#if it.already}<span class="badge">{t("Importiert")}</span>
                  {:else if it.extra.some((x) => !x.toLowerCase().endsWith(".xmp"))}<span class="badge">{t("+ JPEG")}</span>{/if}
                </button>
              {/each}
            </div>
          </section>
        {/each}
      {/if}
    </div>

    <footer>
      <div class="line">
        <span class="label">{t("Nach")}</span>
        <span class="dir" title={target ?? ""}>{target ?? t("Noch kein Ziel gewählt")}</span>
        <button type="button" class="btn small" onclick={() => choose("target")} disabled={started}>{t("Wählen …")}</button>
      </div>
      <div class="line">
        <label class="label" for="import-event">{t("Anlass")}</label>
        <input id="import-event" type="text" bind:value={event} placeholder={t("optional, z. B. Hochzeit")} maxlength="60" disabled={started} />
      </div>
      {#if example}
        <p class="hint">
          {t("Ordner: {path}", { path: example.path })}{example.more ? ` ${t(example.more === 1 ? "(und {n} weiterer Tag)" : "(und {n} weitere Tage)", { n: example.more })}` : ""}
          <br />{t("Dateinamen: Aufnahmedatum_Uhrzeit_Bildnummer, z. B. {name}", { name: picked[0].newName })}
        </p>
      {/if}
      <label class="check-line">
        <input type="checkbox" bind:checked={deleteAfter} disabled={started} />
        {t("Danach von der Karte löschen")}
      </label>
      {#if deleteAfter}
        <p class="warn">{t("Gelöscht wird jede Datei erst, nachdem ihre Kopie Byte für Byte geprüft ist.")}</p>
      {/if}

      {#if progress && started}
        <div class="progress" role="status">
          <div class="track"><div class="fill" style:width="{progress.bytesTotal ? (100 * progress.bytes) / progress.bytesTotal : 0}%"></div></div>
          <span>
            {t("{done} von {total}", { done: progress.done, total: progress.total })} · {size(progress.bytes)} / {size(progress.bytesTotal)}{progress.current ? ` · ${progress.current}` : ""}
          </span>
        </div>
      {/if}

      <div class="actions">
        {#if started}
          <button type="button" class="btn quiet" onclick={() => api.importCancel()}>{t("Import abbrechen")}</button>
          <button type="button" class="btn" onclick={onclose}>{t("Im Hintergrund weiter")}</button>
        {:else}
          <span class="total">{picked.length ? t("{n} ausgewählt, {size}", { n: picked.length, size: size(bytes) }) : ""}</span>
          <button type="button" class="btn quiet" onclick={onclose}>{t("Abbrechen")}</button>
          <button type="submit" class="btn primary" disabled={!target || !picked.length || lib.importRunning}>
            {t(picked.length === 1 ? "{n} Foto importieren" : "{n} Fotos importieren", { n: picked.length })}
          </button>
        {/if}
      </div>
    </footer>
  </form>
</div>

{#if pathFor}
  <PathDialog
    title={pathFor === "source" ? t("Quelle") : t("Zielordner")}
    hint={t("Vollständiger Pfad eines vorhandenen Ordners.")}
    onsubmit={(p) => {
      const which = pathFor;
      pathFor = null;
      if (which === "source") scan(p);
      else target = p;
    }}
    oncancel={() => (pathFor = null)}
  />
{/if}

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
    width: min(920px, 100%);
    height: min(820px, calc(100vh - 32px));
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    display: grid;
    grid-template-rows: auto minmax(0, 1fr) auto;
    overflow: hidden;
  }
  header,
  footer {
    padding: 14px 18px;
    display: grid;
    gap: 10px;
  }
  header {
    border-bottom: 1px solid var(--line);
  }
  footer {
    border-top: 1px solid var(--line);
  }
  h3 {
    margin: 0;
    font-size: var(--step-1);
    font-weight: 600;
  }
  .line {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }
  .label {
    width: 56px;
    flex: none;
    color: var(--muted);
  }
  .dir {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .line input[type="text"] {
    flex: 1;
    min-width: 0;
  }
  .cards {
    flex-wrap: wrap;
  }
  .chip {
    border: 1px solid var(--line);
    background: var(--raised);
    border-radius: 999px;
    padding: 3px 10px;
  }
  .chip.active {
    border-color: var(--accent);
    color: var(--accent);
  }
  .body {
    overflow: auto;
    padding: 0 18px 18px;
  }
  .state {
    color: var(--muted);
    text-align: center;
    margin: 48px 0;
  }
  .bar {
    position: sticky;
    top: 0;
    z-index: 1;
    display: flex;
    gap: 10px;
    align-items: center;
    padding: 10px 0 8px;
    background: var(--panel);
    color: var(--muted);
  }
  .sep {
    color: var(--faint);
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    color: var(--text);
    text-decoration: underline;
    text-decoration-color: var(--faint);
    text-underline-offset: 3px;
  }
  .link:hover {
    color: var(--accent);
  }
  section {
    margin-top: 10px;
  }
  .day {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 4px 0 8px;
  }
  .date {
    font-weight: 600;
  }
  .count {
    color: var(--muted);
    font-size: var(--step--1);
  }
  input[type="checkbox"] {
    accent-color: var(--accent);
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(112px, 1fr));
    gap: 6px;
  }
  .tile {
    position: relative;
    aspect-ratio: 1;
    padding: 0;
    border: 2px solid transparent;
    border-radius: var(--radius-s);
    background: var(--raised);
    overflow: hidden;
    cursor: pointer;
  }
  .tile img {
    width: 100%;
    height: 100%;
    object-fit: contain;
    display: block;
    opacity: 0.45;
    transition: opacity 0.12s;
  }
  .tile.on {
    border-color: var(--accent);
  }
  .tile.on img {
    opacity: 1;
  }
  .tile.already:not(.on) img {
    opacity: 0.25;
  }
  .check {
    position: absolute;
    top: 5px;
    left: 5px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: 1.5px solid rgb(255 255 255 / 0.7);
    background: rgb(0 0 0 / 0.35);
  }
  .tile.on .check {
    background: var(--accent);
    border-color: var(--accent);
    box-shadow: inset 0 0 0 3px var(--accent-ink);
  }
  .badge {
    position: absolute;
    right: 4px;
    bottom: 4px;
    padding: 1px 5px;
    border-radius: var(--radius-s);
    background: rgb(0 0 0 / 0.6);
    color: #fff;
    font-size: 11px;
  }
  .hint {
    margin: 0;
    color: var(--muted);
    font-size: var(--step--1);
    overflow-wrap: anywhere;
  }
  .check-line {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .warn {
    margin: 0;
    color: var(--muted);
    font-size: var(--step--1);
  }
  .progress {
    display: grid;
    gap: 6px;
    font-size: var(--step--1);
    color: var(--muted);
  }
  .track {
    height: 6px;
    background: var(--raised);
    border-radius: 3px;
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.15s;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    align-items: center;
    gap: 8px;
  }
  .total {
    margin-right: auto;
    color: var(--muted);
  }
  .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
</style>
