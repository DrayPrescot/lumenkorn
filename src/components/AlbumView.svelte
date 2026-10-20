<script lang="ts">
  // Fotoalbum: Doppelseiten ansehen und anpassen. Klick wählt ein Foto (oder
  // eine Seite), Ziehen im gewählten Foto verschiebt den Ausschnitt, rechts
  // stehen Ersatzfotos und Aktionen.
  import { api, imageUrl } from "../lib/api";
  import { alternatives, effectiveDpi, formatName, type AlbumCandidate, type AlbumCell } from "../lib/album";
  import { album } from "../lib/album.svelte";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";
  import { isTyping } from "../lib/keys";
  import Icon from "./Icon.svelte";

  /** Unter dieser Auflösung (dpi) wird im Druck sichtbar unscharf */
  const LOW_DPI = 150;

  const a = $derived(album.state!);
  const s = $derived(a.settings);
  const aspect = $derived(s.widthMm / s.heightMm);
  const BG: Record<string, string> = { white: "#fff", gray: "#e8e6e2", black: "#121212" };
  const formatLabel = $derived(formatName(s.format, s.widthMm, s.heightMm));

  // Doppelseiten: 1–2, 3–4 …
  const spreads = $derived(Array.from({ length: Math.ceil(a.pages.length / 2) }, (_, i) => [2 * i, 2 * i + 1].filter((p) => p < a.pages.length)));

  const sel = $derived(album.selection);
  const selCand = $derived(sel?.id != null ? album.byId.get(sel.id) : undefined);
  const selCell = $derived(sel?.id != null ? album.layouts[sel.page]?.cells.find((c) => c.id === sel.id) : undefined);
  const unused = $derived(a.candidates.length - album.photoCount);
  const emptyPages = $derived(a.pages.filter((p) => !p.ids.length).length);

  const alts = $derived.by(() => {
    if (!sel) return [];
    if (selCand) return alternatives(a, { group: selCand.group, events: [selCand.event] });
    const events = a.pages[sel.page]?.ids.map((id) => album.byId.get(id)?.event ?? -1) ?? [];
    // Leere Seite: Ereignisse der Nachbarseiten
    if (!events.length) for (const p of [sel.page - 1, sel.page + 1]) for (const id of a.pages[p]?.ids ?? []) events.push(album.byId.get(id)?.event ?? -1);
    return alternatives(a, { events });
  });

  function lowRes(c: AlbumCandidate | undefined, cell: AlbumCell): number | null {
    const d = effectiveDpi(c, cell);
    return d !== null && d < LOW_DPI ? Math.round(d) : null;
  }

  function dateOf(c: AlbumCandidate | undefined): string {
    if (!c?.capturedAt) return "";
    const d = new Date(c.capturedAt);
    return Number.isNaN(d.getTime()) ? c.capturedAt : d.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
  }

  // --- Ausschnitt mit der Maus verschieben ----------------------------------
  let drag: { id: number; x: number; y: number; ox: number; oy: number; w: number; h: number; pa: number; ca: number; moved: boolean } | null = null;

  function down(e: PointerEvent, page: number, cell: AlbumCell) {
    e.stopPropagation();
    if (e.button !== 0) return;
    const isSel = sel?.id === cell.id && sel.page === page;
    if (!isSel) {
      album.selection = { page, id: cell.id };
      return;
    }
    const el = e.currentTarget as HTMLElement;
    const r = el.getBoundingClientRect();
    el.setPointerCapture(e.pointerId);
    drag = { id: cell.id, x: e.clientX, y: e.clientY, ox: cell.ox, oy: cell.oy, w: r.width, h: r.height, pa: album.byId.get(cell.id)?.aspect ?? 1.5, ca: cell.w / cell.h, moved: false };
  }

  function move(e: PointerEvent) {
    if (!drag) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    if (Math.abs(dx) + Math.abs(dy) > 2) drag.moved = true;
    // Überstand des Fotos über das Feld in Bildschirmpixeln
    const overX = drag.pa > drag.ca ? drag.h * drag.pa - drag.w : 0;
    const overY = drag.pa < drag.ca ? drag.w / drag.pa - drag.h : 0;
    const clamp = (v: number) => Math.min(1, Math.max(0, v));
    album.setOffset(drag.id, [overX > 1 ? clamp(drag.ox - dx / overX) : 0.5, overY > 1 ? clamp(drag.oy - dy / overY) : 0.5]);
  }

  function up() {
    if (drag?.moved) album.commitOffset();
    drag = null;
  }

  function onkeydown(e: KeyboardEvent) {
    if (isTyping(e) || e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === "Escape" && sel) {
      album.selection = null;
      e.preventDefault();
    } else if ((e.key === "Delete" || e.key === "Backspace") && sel?.id != null) {
      album.remove(sel.page, sel.id);
      e.preventDefault();
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="album">
  <header>
    <div class="info">
      <strong>{a.title}</strong>
      <span class="muted">
        {t("{photos} Fotos auf {pages} Seiten · {format}", { photos: album.photoCount, pages: a.pages.length, format: formatLabel })}
        {#if a.pages.length % 2}· {t("ungerade Seitenzahl")}{/if}
        {#if emptyPages}· {t(emptyPages === 1 ? "{n} leere Seite" : "{n} leere Seiten", { n: emptyPages })}{/if}
      </span>
    </div>
    <div class="spacer"></div>
    {#if album.exporting}
      <span class="muted">{t("Speichern – Seite {done} von {total}", { done: album.exporting.done, total: album.exporting.total })}</span>
      <button class="btn quiet" onclick={() => api.albumExportCancel()}>{t("Abbrechen")}</button>
    {:else}
      <button class="btn quiet" onclick={() => album.redistribute()} title={t("Die Fotos des Albums neu auf die Seiten verteilen und anordnen")}>
        <Icon name="refresh" size={14} /> {t("Neu verteilen")}
      </button>
      <button class="btn quiet" onclick={() => (album.dialog = { scope: a.scope, title: a.title })} title={t("Mit anderem Thema, Format oder Umfang neu erstellen")}>
        <Icon name="sliders" size={14} /> {t("Neu erstellen …")}
      </button>
      <button class="btn primary" onclick={() => album.exportPdf()} disabled={!album.photoCount}>
        <Icon name="export" size={14} /> {t("Als PDF speichern …")}
      </button>
    {/if}
  </header>

  <div class="body">
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="spreads" onclick={() => (album.selection = null)}>
      {#each spreads as spread, si (si)}
        <div class="spread">
          {#each spread as p (p)}
            {@const layout = album.layouts[p]}
            <div class="page-wrap" style:--aspect={aspect}>
              <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
              <div
                class="page"
                class:selected={sel?.page === p && sel.id == null}
                style:aspect-ratio="{s.widthMm} / {s.heightMm}"
                style:background={BG[s.background]}
                onclick={(e) => {
                  e.stopPropagation();
                  album.selection = { page: p, id: null };
                }}
              >
                {#each layout?.cells ?? [] as cell (cell.id)}
                  {@const c = album.byId.get(cell.id)}
                  {@const low = lowRes(c, cell)}
                  <div
                    class="cell"
                    class:selected={sel?.id === cell.id && sel.page === p}
                    style:left="{(cell.x / s.widthMm) * 100}%"
                    style:top="{(cell.y / s.heightMm) * 100}%"
                    style:width="{(cell.w / s.widthMm) * 100}%"
                    style:height="{(cell.h / s.heightMm) * 100}%"
                    onpointerdown={(e) => down(e, p, cell)}
                    onpointermove={move}
                    onpointerup={up}
                    onpointercancel={up}
                    onclick={(e) => e.stopPropagation()}
                    role="button"
                    tabindex="-1"
                    aria-label={t("Foto")}
                  >
                    <img
                      src={imageUrl("preview", cell.id, lib.thumbVersion(cell.id))}
                      alt=""
                      draggable="false"
                      style:object-position="{cell.ox * 100}% {cell.oy * 100}%"
                    />
                    {#if low !== null}
                      <span class="low" title={t("Geringe Auflösung für diese Größe: etwa {dpi} dpi. Im Druck kann das Foto unscharf wirken.", { dpi: low })}>{low} dpi</span>
                    {/if}
                  </div>
                {/each}
                {#if !a.pages[p].ids.length}
                  <span class="empty">{t("Leere Seite – rechts Fotos hinzufügen")}</span>
                {/if}
              </div>
              <div class="label">
                <span>{t("Seite {n}", { n: p + 1 })}</span>
                {#if (layout?.variants ?? 0) > 1}
                  <button class="icon-btn" title={t("Anders anordnen")} aria-label={t("Anders anordnen")} onclick={(e) => { e.stopPropagation(); album.relayout(p); }}>
                    <Icon name="refresh" size={13} />
                  </button>
                {/if}
              </div>
            </div>
          {/each}
        </div>
      {/each}
    </div>

    <aside class="side">
      {#if sel && selCand && selCell}
        <h4>{t("Foto auf Seite {n}", { n: sel.page + 1 })}</h4>
        <p class="muted small">{dateOf(selCand)}</p>
        <p class="muted small">{t("Ziehen verschiebt den Ausschnitt. Entf nimmt das Foto heraus.")}</p>
        <div class="actions">
          <button class="btn quiet" disabled={sel.page === 0} onclick={() => album.move(sel.page, selCand.id, -1)}>
            <Icon name="back" size={13} /> {t("Vorige Seite")}
          </button>
          <button class="btn quiet" disabled={sel.page >= a.pages.length - 1} onclick={() => album.move(sel.page, selCand.id, 1)}>
            {t("Nächste Seite")} <Icon name="chevron" size={13} />
          </button>
          {#if a.offsets[selCand.id]}
            <button class="btn quiet" onclick={() => album.resetOffset(selCand.id)}>{t("Ausschnitt zurücksetzen")}</button>
          {/if}
          <button class="btn quiet danger" onclick={() => album.remove(sel.page, selCand.id)}>
            <Icon name="trash" size={13} /> {t("Entfernen")}
          </button>
        </div>
        <h4>{t("Ersetzen durch")}</h4>
        {#if alts.length}
          <div class="alts">
            {#each alts as c (c.id)}
              <button class="alt" class:same={c.group === selCand.group} title={dateOf(c)} onclick={() => album.replace(sel.page, selCand.id, c.id)}>
                <img src={imageUrl("thumb", c.id, lib.thumbVersion(c.id))} alt="" loading="lazy" />
              </button>
            {/each}
          </div>
        {:else}
          <p class="muted small">{t("Alle Fotos sind schon im Album.")}</p>
        {/if}
      {:else if sel}
        <h4>{t("Seite {n}", { n: sel.page + 1 })}</h4>
        <div class="actions">
          {#if (album.layouts[sel.page]?.variants ?? 0) > 1}
            <button class="btn quiet" onclick={() => album.relayout(sel.page)}><Icon name="refresh" size={13} /> {t("Anders anordnen")}</button>
          {/if}
          <button class="btn quiet" onclick={() => album.addPage(sel.page)}><Icon name="plus" size={13} /> {t("Seite danach einfügen")}</button>
          <button class="btn quiet danger" disabled={a.pages.length <= 1} onclick={() => album.deletePage(sel.page)}>
            <Icon name="trash" size={13} /> {t("Seite löschen")}
          </button>
        </div>
        <h4>{t("Foto hinzufügen")}</h4>
        {#if a.pages[sel.page].ids.length >= 6}
          <p class="muted small">{t("Auf einer Seite haben höchstens 6 Fotos Platz.")}</p>
        {:else if alts.length}
          <div class="alts">
            {#each alts as c (c.id)}
              <button class="alt" title={dateOf(c)} onclick={() => album.add(sel.page, c.id)}>
                <img src={imageUrl("thumb", c.id, lib.thumbVersion(c.id))} alt="" loading="lazy" />
              </button>
            {/each}
          </div>
        {:else}
          <p class="muted small">{t("Alle Fotos sind schon im Album.")}</p>
        {/if}
      {:else}
        <h4>{t("Album anpassen")}</h4>
        <p class="muted small">{t("Klicke ein Foto an, um es zu tauschen, zu verschieben oder herauszunehmen. Ein Klick auf den Seitenrand wählt die Seite: Dort lassen sich Fotos hinzufügen und die Anordnung wechseln.")}</p>
        <p class="muted small">{t(unused === 1 ? "{n} weiteres Foto steht zur Auswahl." : "{n} weitere Fotos stehen zur Auswahl.", { n: unused })}</p>
        {#if !a.themeUsed}
          <p class="muted small">{t("Das Thema wurde nicht berücksichtigt, weil die Fotos noch nicht von der KI analysiert sind.")}</p>
        {:else if a.unanalyzed}
          <p class="muted small">{t("{n} Fotos sind noch nicht von der KI analysiert und zählen nur nach Qualität.", { n: a.unanalyzed })}</p>
        {/if}
        <div class="actions">
          <button class="btn quiet" onclick={() => album.close()}>{t("Zurück zu den Fotos")}</button>
          <button class="btn quiet danger" onclick={() => album.discard()}>{t("Album verwerfen")}</button>
        </div>
      {/if}
    </aside>
  </div>
</div>

<style>
  .album {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 14px;
    border-bottom: 1px solid var(--line);
    flex-wrap: wrap;
  }
  .info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .spacer {
    flex: 1;
  }
  .muted {
    color: var(--muted);
  }
  .small {
    font-size: var(--step--1);
    margin: 0;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 1fr 280px;
  }
  .spreads {
    overflow: auto;
    padding: 20px 16px 40px;
    display: flex;
    flex-direction: column;
    gap: 28px;
    background: var(--bg);
  }
  .spread {
    display: flex;
    justify-content: center;
    gap: 2px;
  }
  .page-wrap {
    /* Zwei Seiten nebeneinander, höchstens so hoch wie das Fenster */
    width: min(calc(50% - 1px), calc((100vh - 220px) * var(--aspect)));
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .page {
    position: relative;
    box-shadow: 0 1px 6px rgb(0 0 0 / 0.35);
    overflow: hidden;
    cursor: default;
  }
  .page.selected {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .cell {
    position: absolute;
    overflow: hidden;
    cursor: pointer;
    touch-action: none;
  }
  .cell.selected {
    outline: 3px solid var(--accent);
    outline-offset: -3px;
    cursor: grab;
    z-index: 1;
  }
  .cell img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
    user-select: none;
    pointer-events: none;
  }
  .low {
    position: absolute;
    left: 6px;
    bottom: 6px;
    padding: 1px 6px;
    border-radius: 3px;
    background: rgb(180 40 30 / 0.85);
    color: #fff;
    font-size: 11px;
  }
  .empty {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    color: #888;
    font-size: var(--step--1);
    text-align: center;
    padding: 12px;
  }
  .label {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    color: var(--muted);
    font-size: var(--step--1);
    min-height: 24px;
  }
  .side {
    border-left: 1px solid var(--line);
    background: var(--panel);
    padding: 14px;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  h4 {
    margin: 4px 0 0;
    font-size: var(--step-0);
    font-weight: 600;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .danger {
    color: var(--reject);
  }
  .alts {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 4px;
  }
  .alt {
    padding: 0;
    border: 2px solid transparent;
    border-radius: var(--radius-s);
    background: var(--raised);
    aspect-ratio: 1;
    overflow: hidden;
    cursor: pointer;
  }
  .alt.same {
    border-color: color-mix(in srgb, var(--accent) 60%, transparent);
  }
  .alt:hover {
    border-color: var(--accent);
  }
  .alt img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  @media (max-width: 800px) {
    .body {
      grid-template-columns: 1fr;
      grid-template-rows: 1fr auto;
    }
    .side {
      border-left: 0;
      border-top: 1px solid var(--line);
      max-height: 40vh;
    }
  }
</style>
