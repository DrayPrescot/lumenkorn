<script lang="ts">
  import { api, type LabelCount, type PhotoStats, type ValueCount } from "../lib/api";
  import { groupTypes, typeExts } from "../lib/filetypes";
  import { locale, num, t } from "../lib/i18n";
  import { lib } from "../lib/store.svelte";
  import Icon from "./Icon.svelte";
  import StatBars, { type BarItem } from "./StatBars.svelte";
  import StatColumns from "./StatColumns.svelte";

  // Statistik der angezeigten Fotos (Bereich und Filter wie im Raster).
  // Ein Klick auf Kamera, Objektiv, Schlagwort … filtert danach; die
  // Statistik rechnet dann für die gefilterten Fotos neu.

  let stats = $state<PhotoStats | null>(null);
  let error = $state<string | null>(null);
  let token = 0;

  $effect(() => {
    // Neu laden, sobald die Fotoliste neu geladen wurde (Filter, Bereich)
    void lib.listVersion;
    const my = ++token;
    api
      .photoStats(lib.currentQuery())
      .then((s) => {
        if (my === token) {
          stats = s;
          error = null;
        }
      })
      .catch((e) => {
        if (my === token) error = String(e);
      });
  });

  const fotos = (n: number) => t(n === 1 ? "{n} Foto" : "{n} Fotos", { n: num(n) });
  const pct = (n: number, total: number) => num(total ? (100 * n) / total : 0, { maximumFractionDigits: 0 });
  const labels = (l: LabelCount[]): BarItem[] => l.map((x) => ({ label: x.label, count: x.count }));
  const sum = (l: { count: number }[]) => l.reduce((n, x) => n + x.count, 0);

  /** Werte in Bereiche einteilen: `edges` sind die Obergrenzen. */
  function buckets(values: ValueCount[], edges: number[], label: (lo: number | null, hi: number | null) => string): BarItem[] {
    const out = edges.map((_, i) => ({ label: label(i ? edges[i - 1] : null, edges[i]), count: 0 }));
    out.push({ label: label(edges[edges.length - 1], null), count: 0 });
    for (const v of values) {
      const i = edges.findIndex((e) => v.value <= e + 1e-9);
      out[i < 0 ? edges.length : i].count += v.count;
    }
    // Leere Bereiche am Rand weglassen, innen behalten (Verteilung bleibt ehrlich)
    let a = 0;
    let b = out.length;
    while (a < b && !out[a].count) a++;
    while (b > a && !out[b - 1].count) b--;
    return out.slice(a, b);
  }

  /** Genaue Werte, solange es wenige sind, sonst Bereiche */
  function valuesOrBuckets(values: ValueCount[], fmt: (v: number) => string, edges: number[], label: (lo: number | null, hi: number | null) => string): BarItem[] {
    if (values.length <= 14) return values.map((v) => ({ label: fmt(v.value), count: v.count }));
    return buckets(values, edges, label);
  }

  // Ohne Tausenderpunkt, wie auf der Kamera („1/8000 s“, „ISO 3200“)
  const plain = (v: number) => num(v, { useGrouping: false, maximumFractionDigits: 1 });
  const mm = (v: number) => `${plain(v)} mm`;
  const focal = $derived(
    stats
      ? valuesOrBuckets(stats.focalLengths, mm, [16, 24, 35, 50, 85, 135, 200, 400], (lo, hi) =>
          lo === null ? `≤ ${mm(hi!)}` : hi === null ? `> ${mm(lo)}` : `${num(lo + 1)}–${mm(hi)}`,
        )
      : [],
  );
  const fnum = (v: number) => `f/${num(v, { maximumFractionDigits: 1 })}`;
  const aperture = $derived(
    stats
      ? valuesOrBuckets(stats.apertures, fnum, [1.4, 2, 2.8, 4, 5.6, 8, 11, 16], (lo, hi) =>
          lo === null ? `≤ ${fnum(hi!)}` : hi === null ? `> ${fnum(lo)}` : `${fnum(lo)} – ${fnum(hi)}`,
        )
      : [],
  );
  const iso = $derived(
    stats
      ? valuesOrBuckets(stats.isos, (v) => `ISO ${plain(v)}`, [100, 200, 400, 800, 1600, 3200, 6400, 12800], (lo, hi) =>
          lo === null ? `≤ ISO ${plain(hi!)}` : hi === null ? `> ISO ${plain(lo)}` : `ISO ${plain(lo + 1)}–${plain(hi)}`,
        )
      : [],
  );

  /** Belichtungszeit in Sekunden („1/250“ → 0,004) */
  function seconds(s: string): number {
    const m = s.trim().match(/^(\d+(?:[.,]\d+)?)\s*\/\s*(\d+(?:[.,]\d+)?)$/);
    const f = (x: string) => parseFloat(x.replace(",", "."));
    return m ? f(m[1]) / f(m[2]) : f(s);
  }
  const sec = (v: number) => (v >= 1 ? `${plain(v)} s` : `1/${plain(Math.round(1 / v))} s`);
  const exposure = $derived.by(() => {
    if (!stats) return [];
    const vals = stats.exposures
      .map((e) => ({ value: seconds(e.label), count: e.count, label: e.label }))
      .filter((e) => e.value > 0 && Number.isFinite(e.value))
      .sort((a, b) => a.value - b.value);
    // Gleiche Zeiten in verschiedener Schreibweise zusammenfassen
    const merged: ValueCount[] = [];
    for (const v of vals) {
      const last = merged[merged.length - 1];
      if (last && Math.abs(last.value - v.value) < last.value * 1e-6) last.count += v.count;
      else merged.push({ value: v.value, count: v.count });
    }
    return valuesOrBuckets(merged, sec, [1 / 1000, 1 / 500, 1 / 250, 1 / 125, 1 / 60, 1 / 30, 1 / 8, 1], (lo, hi) =>
      lo === null ? t("{v} und kürzer", { v: sec(hi!) }) : hi === null ? t("länger als {v}", { v: sec(lo) }) : t("bis {v}", { v: sec(hi) }),
    );
  });

  const types = $derived(stats ? groupTypes(stats.fileTypes) : []);
  const ratings = $derived(
    stats ? stats.ratings.map((count, i) => ({ label: i ? "★".repeat(i) : t("Ohne Sterne"), count })).filter((r) => r.count) : [],
  );

  // Zeitverlauf: Monate, bei mehr als drei Jahren Jahre; Lücken als leere Säulen
  const timeline = $derived.by((): { items: BarItem[]; years: boolean } => {
    if (!stats || !stats.months.length) return { items: [], years: false };
    const by = new Map(stats.months.map((m) => [m.label, m.count]));
    const [y0, m0] = stats.months[0].label.split("-").map(Number);
    const [y1, m1] = stats.months[stats.months.length - 1].label.split("-").map(Number);
    const span = (y1 - y0) * 12 + (m1 - m0) + 1;
    if (span > 36) {
      const items: BarItem[] = [];
      for (let y = y0; y <= y1; y++) {
        let n = 0;
        for (let m = 1; m <= 12; m++) n += by.get(`${y}-${String(m).padStart(2, "0")}`) ?? 0;
        items.push({ label: String(y), count: n });
      }
      return { items, years: true };
    }
    const fmt = new Intl.DateTimeFormat(locale, { month: "long", year: "numeric" });
    const items: BarItem[] = [];
    for (let k = 0; k < span; k++) {
      const y = y0 + Math.floor((m0 - 1 + k) / 12);
      const m = ((m0 - 1 + k) % 12) + 1;
      const key = `${y}-${String(m).padStart(2, "0")}`;
      items.push({ label: key, count: by.get(key) ?? 0, hint: fmt.format(new Date(y, m - 1, 1)) });
    }
    return { items, years: false };
  });
  const monthShort = new Intl.DateTimeFormat(locale, { month: "short" });
  function timeTick(i: number): string | null {
    const items = timeline.items;
    const n = items.length;
    if (timeline.years) {
      const step = Math.ceil(n / 8);
      return i % step === 0 || i === n - 1 ? items[i].label : null;
    }
    const [y, m] = items[i].label.split("-").map(Number);
    if (n <= 12) return monthShort.format(new Date(y, m - 1, 1));
    return m === 1 || i === 0 ? String(y) : null;
  }
  const hours = $derived(stats ? stats.hours.map((count, h) => ({ label: String(h), count, hint: t("{h} bis {e} Uhr", { h, e: h + 1 }) })) : []);

  const dateFmt = new Intl.DateTimeFormat(locale, { dateStyle: "medium" });
  const day = (iso: string | null) => (iso ? dateFmt.format(new Date(iso.slice(0, 10) + "T12:00:00")) : "–");

  /** Nach einem Eintrag filtern (über die Suche, die Kamera, Objektiv,
   *  Schlagworte, Personen und Orte umfasst) */
  function searchFor(text: string) {
    lib.semantic = false;
    lib.search = text;
    void lib.loadImages(true);
  }
  function filterType(item: BarItem) {
    const ft = types.find((x) => x.label === item.label);
    if (!ft) return;
    lib.fileTypes = typeExts(ft.exts[0]);
    void lib.loadImages(true);
  }
  const pickHint = t("Klicken filtert nach diesem Eintrag");
  const unknown = (total: number, part: number) => (total - part > 0 ? t("{n} ohne Angabe", { n: fotos(total - part) }) : "");
</script>

<div class="stats">
  <header>
    <div>
      <h2>{t("Statistik")}</h2>
      <p class="muted">
        {lib.scopeTitle}
        {#if lib.search.trim()}· {t("Suche „{s}“", { s: lib.search.trim() })}{/if}
        {#if lib.minRating}· {t("ab {n} Sternen", { n: lib.minRating })}{/if}
        {#if lib.editedFilter !== null}· {lib.editedFilter ? t("nur bearbeitete") : t("nur unbearbeitete")}{/if}
        {#if lib.fileTypes.length}· {[...new Set(types.length ? lib.fileTypes.map((e) => types.find((x) => x.exts.includes(e))?.label ?? e.toUpperCase()) : lib.fileTypes)].join(", ")}{/if}
      </p>
    </div>
    <span class="grow"></span>
    {#if lib.filtered}
      <button class="btn quiet" onclick={() => lib.resetFilters()}>{t("Filter zurücksetzen")}</button>
    {/if}
    <button class="btn" onclick={() => (lib.view = "grid")}><Icon name="photos" size={14} /> {t("Fotos zeigen")}</button>
  </header>

  {#if error}
    <p class="error">{error}</p>
  {:else if !stats}
    <p class="muted">{t("Wird geladen …")}</p>
  {:else if !stats.total}
    <p class="muted">{t("Keine Fotos in dieser Ansicht.")}</p>
  {:else}
    {@const s = stats}
    <section class="tiles" aria-label={t("Überblick")}>
      <div class="tile"><span class="big">{num(s.total)}</span><span>{t("Fotos")}</span></div>
      <div class="tile"><span class="big">{pct(s.raw, s.total)} %</span><span>{t("RAW ({n})", { n: num(s.raw) })}</span></div>
      <div class="tile"><span class="big">{pct(s.edited, s.total)} %</span><span>{t("bearbeitet ({n})", { n: num(s.edited) })}</span></div>
      <div class="tile"><span class="big">{pct(s.withLocation, s.total)} %</span><span>{t("mit Aufnahmeort")}</span></div>
      <div class="tile wide"><span class="big date">{day(s.first)} – {day(s.last)}</span><span>{t("Zeitraum")}</span></div>
    </section>

    <section class="cards">
      {#if s.cameras.length}
        <article class="card">
          <h3>{t("Kameras")}</h3>
          <StatBars items={labels(s.cameras)} total={s.total} onpick={(i) => searchFor(i.label)} {pickHint} />
          <p class="foot">{unknown(s.total, sum(s.cameras))}</p>
        </article>
      {/if}
      {#if s.lenses.length}
        <article class="card">
          <h3>{t("Objektive")}</h3>
          <StatBars items={labels(s.lenses)} total={s.total} onpick={(i) => searchFor(i.label)} {pickHint} />
          <p class="foot">{unknown(s.total, sum(s.lenses))}</p>
        </article>
      {/if}
      {#if focal.length}
        <article class="card">
          <h3>{t("Brennweite")}</h3>
          <StatBars items={focal} total={s.total} limit={14} />
          <p class="foot">{unknown(s.total, sum(s.focalLengths))}</p>
        </article>
      {/if}
      {#if aperture.length}
        <article class="card">
          <h3>{t("Blende")}</h3>
          <StatBars items={aperture} total={s.total} limit={14} />
          <p class="foot">{unknown(s.total, sum(s.apertures))}</p>
        </article>
      {/if}
      {#if exposure.length}
        <article class="card">
          <h3>{t("Belichtungszeit")}</h3>
          <StatBars items={exposure} total={s.total} limit={14} />
          <p class="foot">{unknown(s.total, sum(s.exposures))}</p>
        </article>
      {/if}
      {#if iso.length}
        <article class="card">
          <h3>{t("ISO")}</h3>
          <StatBars items={iso} total={s.total} limit={14} />
          <p class="foot">{unknown(s.total, sum(s.isos))}</p>
        </article>
      {/if}
      {#if timeline.items.length}
        <article class="card">
          <h3>{timeline.years ? t("Aufnahmen je Jahr") : t("Aufnahmen je Monat")}</h3>
          <StatColumns items={timeline.items} ticks={timeTick} />
        </article>
      {/if}
      {#if s.hours.some((h) => h)}
        <article class="card">
          <h3>{t("Tageszeit")}</h3>
          <StatColumns items={hours} ticks={(i) => (i % 6 === 0 ? `${i}:00` : null)} />
        </article>
      {/if}
      {#if s.keywords.length}
        <article class="card">
          <h3>{t("Schlagworte")}</h3>
          <StatBars items={labels(s.keywords)} total={s.total} limit={12} onpick={(i) => searchFor(i.label)} {pickHint} />
        </article>
      {/if}
      {#if s.people.length}
        <article class="card">
          <h3>{t("Personen")}</h3>
          <StatBars items={labels(s.people)} total={s.total} onpick={(i) => searchFor(i.label)} {pickHint} />
        </article>
      {/if}
      {#if s.countries.length}
        <article class="card">
          <h3>{t("Länder")}</h3>
          <StatBars items={labels(s.countries)} total={s.total} onpick={(i) => searchFor(i.label)} {pickHint} />
          <p class="foot">{unknown(s.total, sum(s.countries))}</p>
        </article>
      {/if}
      <article class="card">
        <h3>{t("Dateitypen")}</h3>
        <StatBars items={types.map((x) => ({ label: x.label, count: x.count }))} total={s.total} onpick={filterType} {pickHint} />
      </article>
      {#if ratings.length}
        <article class="card">
          <h3>{t("Bewertung")}</h3>
          <StatBars items={ratings} total={s.total} />
        </article>
      {/if}
    </section>
  {/if}
</div>

<style>
  .stats {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 20px 24px 40px;
  }
  header {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    margin-bottom: 18px;
  }
  h2 {
    margin: 0 0 2px;
    font-size: var(--step-2);
    font-weight: 600;
  }
  header p {
    margin: 0;
    font-size: var(--step--1);
  }
  .grow {
    flex: 1;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 10px;
    margin-bottom: 14px;
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 2px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    padding: 12px 14px;
    color: var(--muted);
    font-size: var(--step--1);
  }
  .tile.wide {
    grid-column: span 2;
  }
  .big {
    color: var(--text);
    font-size: 22px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .big.date {
    font-size: var(--step-1);
    line-height: 30px;
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(min(100%, 380px), 1fr));
    gap: 12px;
    align-items: start;
  }
  .card {
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    padding: 14px 14px 10px;
    min-width: 0;
  }
  h3 {
    margin: 0 0 10px;
    font-size: var(--step-0);
    font-weight: 600;
  }
  .foot {
    margin: 6px 4px 0;
    min-height: 0;
    color: var(--faint);
    font-size: var(--step--1);
  }
  .foot:empty {
    display: none;
  }
  .error {
    color: var(--reject);
  }
</style>
