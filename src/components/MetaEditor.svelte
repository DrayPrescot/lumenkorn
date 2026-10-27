<script lang="ts">
  // Metadaten bearbeiten (Titel, Beschreibung, Autor, Aufnahmedaten …) für
  // ein oder mehrere Fotos. Gespeichert wird im Katalog und in einer
  // XMP-Begleitdatei neben dem Foto; die Bilddatei bleibt unverändert.
  import { api, type ImageMeta, type MetaEdit, type MetaPatch } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";
  import Icon from "./Icon.svelte";

  let { ids, onclose }: { ids: number[]; onclose: () => void } = $props();

  type Kind = "text" | "area" | "time" | "iso" | "exposure" | "aperture" | "focal";
  type Def = { key: keyof Omit<ImageMeta, "id">; kind: Kind; label: string; unit?: string; hint?: string };

  const defs: Def[] = [
    { key: "title", kind: "text", label: t("Titel") },
    { key: "description", kind: "area", label: t("Beschreibung") },
    { key: "creator", kind: "text", label: t("Autor") },
    { key: "copyright", kind: "text", label: t("Urheberrecht") },
    { key: "capturedAt", kind: "time", label: t("Aufnahme") },
    { key: "camera", kind: "text", label: t("Kamera") },
    { key: "lens", kind: "text", label: t("Objektiv") },
    { key: "focalLength", kind: "focal", label: t("Brennweite"), unit: "mm", hint: "50" },
    { key: "aperture", kind: "aperture", label: t("Blende"), unit: "f/", hint: "2.8" },
    { key: "exposure", kind: "exposure", label: t("Belichtung"), unit: "s", hint: "1/250" },
    { key: "iso", kind: "iso", label: t("ISO"), hint: "400" },
  ];

  type Row = {
    text: string;
    /** Wert geändert oder zurückgesetzt – nur solche Felder werden gespeichert */
    touched: boolean;
    /** Wert aus der Datei wiederherstellen */
    reset: boolean;
    /** Fotos haben verschiedene Werte */
    mixed: boolean;
    /** bei mindestens einem Foto vom Nutzer geändert */
    over: boolean;
    /** Wert in der Datei (nur bei einem Foto oder wenn alle gleich) */
    file: string;
    error: string;
  };

  let rows = $state<Record<string, Row>>({});
  let orig: Record<string, Row> = {};
  let loaded = $state(false);
  let busy = $state(false);
  let failed = $state<{ id: number; path: string; error: string }[]>([]);

  // Zeit: festlegen oder verschieben
  let timeMode = $state<"set" | "shift">("set");
  let sign = $state<1 | -1>(1);
  let shift = $state({ d: "", h: "", m: "", s: "" });

  const units: { k: "d" | "h" | "m" | "s"; label: string }[] = [
    { k: "d", label: t("Tage") },
    { k: "h", label: t("Std.") },
    { k: "m", label: t("Min.") },
    { k: "s", label: t("Sek.") },
  ];

  const many = $derived(ids.length > 1);

  /** Anzeige eines Wertes im Eingabefeld */
  function show(kind: Kind, v: unknown): string {
    if (v === null || v === undefined) return "";
    if (kind === "time") return String(v).slice(0, 19);
    if (kind === "exposure") return String(v).replace(/\s*s$/, "");
    if (kind === "focal") return String(Math.round(Number(v) * 10) / 10);
    return String(v);
  }

  async function load() {
    let metas: ImageMeta[] = [];
    try {
      metas = await api.metaGetMany(ids);
    } catch (e) {
      lib.notify(String(e), "error");
      onclose();
      return;
    }
    const next: Record<string, Row> = {};
    for (const d of defs) {
      const fields = metas.map((m) => m[d.key] as { value: unknown; file: unknown; overridden: boolean });
      const vals = fields.map((f) => show(d.kind, f.value));
      const files = fields.map((f) => show(d.kind, f.file));
      const same = vals.every((v) => v === vals[0]);
      next[d.key] = {
        text: same ? (vals[0] ?? "") : "",
        touched: false,
        reset: false,
        mixed: !same,
        over: fields.some((f) => f.overridden),
        file: files.every((f) => f === files[0]) ? (files[0] ?? "") : "",
        error: "",
      };
    }
    orig = structuredClone(next);
    rows = next;
    loaded = true;
  }
  void load();

  function touch(key: string) {
    const r = rows[key];
    r.touched = true;
    r.reset = false;
    r.error = "";
  }

  /** Auf den Wert aus der Datei zurücksetzen (beim Speichern) */
  function resetField(key: string) {
    const r = rows[key];
    r.touched = true;
    r.reset = true;
    r.text = "";
    r.error = "";
  }

  /** Rückgängig: Feld wieder wie geladen */
  function undo(key: string) {
    rows[key] = { ...orig[key] };
  }

  const num = (s: string) => Number(s.trim().replace(",", "."));

  /** Prüft die Eingabe; leerer Text ergibt `null` (Wert aus der Datei). */
  function parse(d: Def, raw: string): { ok: true; edit: MetaEdit<never> } | { ok: false; error: string } {
    const s = raw.trim();
    const done = (v: unknown) => ({ ok: true as const, edit: (v === null ? "reset" : { set: v }) as MetaEdit<never> });
    if (!s) return done(null);
    switch (d.kind) {
      case "text":
      case "area":
        return done(s);
      case "time":
        return /^\d{4}-\d\d-\d\dT\d\d:\d\d(:\d\d)?$/.test(s) ? done(s.length === 16 ? `${s}:00` : s) : { ok: false, error: t("Ungültiges Datum") };
      case "iso": {
        const v = num(s);
        return Number.isInteger(v) && v >= 1 && v <= 10_000_000 ? done(v) : { ok: false, error: t("Ganze Zahl erwartet") };
      }
      case "aperture": {
        const v = num(s.replace(/^f\s*\/?\s*/i, ""));
        return Number.isFinite(v) && v > 0 && v <= 1000 ? done(v) : { ok: false, error: t("Zahl wie 2,8 erwartet") };
      }
      case "focal": {
        const v = num(s.replace(/\s*mm$/i, ""));
        return Number.isFinite(v) && v > 0 && v <= 100_000 ? done(v) : { ok: false, error: t("Zahl in mm erwartet") };
      }
      case "exposure":
        return /^\d+([.,]\d+)?(\s*\/\s*\d+([.,]\d+)?)?\s*s?$/.test(s) && !/\/\s*0+([.,]0*)?\s*s?$/.test(s) && !/^0+([.,]0*)?\s*s?$/.test(s)
          ? done(s.replace(/\s+/g, "").replace(/s$/i, ""))
          : { ok: false, error: t("Wie 1/250 oder 2 (Sekunden)") };
    }
  }

  const shiftSeconds = $derived.by(() => {
    if (timeMode !== "shift") return 0;
    const n = (s: string) => Math.trunc(Number(s) || 0);
    return sign * (n(shift.d) * 86400 + n(shift.h) * 3600 + n(shift.m) * 60 + n(shift.s));
  });
  const shiftInvalid = $derived(
    timeMode === "shift" && Object.values(shift).some((v) => v !== "" && !/^\d+$/.test(v)),
  );

  const dirty = $derived(
    Object.entries(rows).some(([k, r]) => r.touched && !(timeMode === "shift" && k === "capturedAt")) || shiftSeconds !== 0,
  );

  async function save() {
    if (busy || !dirty || shiftInvalid) return;
    const patch: Record<string, MetaEdit<unknown>> = {};
    let bad = false;
    for (const d of defs) {
      const r = rows[d.key];
      if (!r.touched || (timeMode === "shift" && d.key === "capturedAt")) continue;
      if (r.reset) {
        patch[d.key] = "reset";
        continue;
      }
      const p = parse(d, r.text);
      if (!p.ok) {
        r.error = p.error;
        bad = true;
      } else patch[d.key] = p.edit;
    }
    if (bad) return;
    busy = true;
    const report = await lib.editMeta(ids, patch as MetaPatch, shiftSeconds);
    busy = false;
    if (report.failed.length) {
      failed = report.failed;
      await load();
      timeMode = "set";
      shift = { d: "", h: "", m: "", s: "" };
      if (report.changed.length) lib.notify(t(report.changed.length === 1 ? "{n} Foto gespeichert" : "{n} Fotos gespeichert", { n: report.changed.length }));
      return;
    }
    if (report.changed.length) {
      lib.notify(t(report.changed.length === 1 ? "Metadaten von {n} Foto gespeichert" : "Metadaten von {n} Fotos gespeichert", { n: report.changed.length }));
      onclose();
    }
  }

  function key(e: KeyboardEvent) {
    e.stopPropagation(); // Tastenkürzel der Bibliothek nicht auslösen
    if (e.key === "Escape") onclose();
    else if (e.key === "Enter" && (e.ctrlKey || e.metaKey || (e.target as HTMLElement).tagName === "INPUT")) {
      e.preventDefault();
      void save();
    }
  }
</script>

<div class="editor" role="group" aria-label={t("Metadaten bearbeiten")}>
  <p class="note">
    {many
      ? t("Nur geänderte Felder gelten für alle {n} Fotos.", { n: ids.length })
      : t("Gespeichert im Katalog und in einer XMP-Datei neben dem Foto. Das Foto selbst bleibt unverändert.")}
  </p>
  {#if loaded}
    {#each defs as d (d.key)}
      {@const r = rows[d.key]}
      <div class="row" class:wide={d.kind === "area"}>
        <label for={`meta-${d.key}`}>
          {d.label}
          {#if r.over && !r.reset}<span class="dot" title={t("Von dir geändert")}></span>{/if}
        </label>
        <div class="field">
          {#if d.kind === "time" && timeMode === "shift"}
            <div class="shift">
              <select bind:value={sign} aria-label={t("Richtung")} onkeydown={key}>
                <option value={1}>+</option>
                <option value={-1}>−</option>
              </select>
              {#each units as u (u.k)}
                <input type="text" inputmode="numeric" placeholder={u.label} aria-label={u.label} bind:value={shift[u.k]} onkeydown={key} />
              {/each}
            </div>
          {:else if d.kind === "area"}
            <textarea
              id={`meta-${d.key}`}
              rows="3"
              placeholder={r.reset ? t("leer") : r.mixed ? t("verschiedene Werte") : ""}
              bind:value={r.text}
              oninput={() => touch(d.key)}
              onkeydown={key}
            ></textarea>
          {:else}
            <div class="inp" class:err={!!r.error}>
              {#if d.unit === "f/"}<span class="unit">f/</span>{/if}
              <input
                id={`meta-${d.key}`}
                type={d.kind === "time" ? "datetime-local" : "text"}
                step={d.kind === "time" ? 1 : undefined}
                inputmode={d.kind === "iso" ? "numeric" : undefined}
                placeholder={r.reset ? (many && !r.file ? t("Wert aus der Datei") : r.file || t("leer")) : r.mixed ? t("verschiedene Werte") : (d.hint ?? "")}
                bind:value={r.text}
                oninput={() => touch(d.key)}
                onkeydown={key}
              />
              {#if d.unit && d.unit !== "f/"}<span class="unit">{d.unit}</span>{/if}
            </div>
          {/if}
          {#if d.kind === "time"}
            <button class="link small" type="button" onclick={() => (timeMode = timeMode === "set" ? "shift" : "set")}>
              {timeMode === "set" ? t("Verschieben …") : t("Festlegen …")}
            </button>
          {/if}
          {#if r.error}<small class="error">{r.error}</small>{/if}
        </div>
        {#if (r.over || r.touched) && !(d.kind === "time" && timeMode === "shift")}
          <button
            class="icon-btn"
            type="button"
            class:on={r.reset}
            title={many || !r.file ? t("Auf den Wert aus der Datei zurücksetzen") : t("Zurücksetzen auf {value}", { value: r.file })}
            aria-label={t("Auf den Wert aus der Datei zurücksetzen")}
            aria-pressed={r.reset}
            onclick={() => (r.reset ? undo(d.key) : resetField(d.key))}
          >
            <Icon name="refresh" size={13} />
          </button>
        {:else}
          <span class="gap"></span>
        {/if}
      </div>
    {/each}

    {#if failed.length}
      <div class="failed" role="alert">
        <strong>{t(failed.length === 1 ? "{n} Foto konnte nicht gespeichert werden:" : "{n} Fotos konnten nicht gespeichert werden:", { n: failed.length })}</strong>
        <ul>
          {#each failed.slice(0, 5) as f (f.id)}<li title={f.path}>{f.error}</li>{/each}
          {#if failed.length > 5}<li>…</li>{/if}
        </ul>
      </div>
    {/if}

    <div class="buttons">
      <button class="btn quiet" type="button" onclick={onclose}>{t("Abbrechen")}</button>
      <button class="btn primary" type="button" disabled={busy || !dirty || shiftInvalid} onclick={save}>
        {busy ? t("Speichern …") : t("Speichern")}
      </button>
    </div>
  {:else}
    <p class="note">{t("Lade …")}</p>
  {/if}
</div>

<style>
  .editor {
    display: grid;
    gap: 8px;
  }
  .note {
    margin: 0 0 2px;
    color: var(--faint);
    font-size: var(--step--1);
  }
  .row {
    display: grid;
    grid-template-columns: 76px minmax(0, 1fr) auto;
    gap: 6px 8px;
    align-items: start;
  }
  label {
    color: var(--muted);
    padding-top: 6px;
  }
  .dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    margin-left: 4px;
    border-radius: 50%;
    background: var(--accent);
    vertical-align: middle;
  }
  .field {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 3px;
    min-width: 0;
  }
  .inp {
    box-sizing: border-box;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
    padding: 0 9px;
  }
  .inp.err {
    border-color: var(--reject);
  }
  .inp input {
    flex: 1;
    /* Ohne feste Grundbreite: die natürliche (20 Zeichen) sprengt die Spalte */
    width: 0;
    min-width: 0;
    background: none;
    border: 0;
    padding: 5px 0;
    outline: none;
  }
  .inp:focus-within {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .unit {
    color: var(--faint);
    font-size: var(--step--1);
  }
  textarea {
    box-sizing: border-box;
    width: 100%;
    resize: vertical;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
    padding: 5px 9px;
    font: inherit;
    color: inherit;
  }
  .shift {
    display: grid;
    grid-template-columns: 44px repeat(4, minmax(0, 1fr));
    gap: 4px;
  }
  .shift :global(input),
  .shift :global(select) {
    padding: 5px 6px;
    width: 100%;
    text-align: center;
  }
  .link {
    justify-self: start;
    color: var(--accent);
  }
  .link.small {
    font-size: var(--step--1);
  }
  .link:hover {
    text-decoration: underline;
  }
  .icon-btn {
    margin-top: 2px;
  }
  /* Ohne Zurücksetzen-Knopf keine leere Spalte */
  .gap {
    margin-left: -8px;
  }
  .icon-btn.on {
    color: var(--accent);
    background: var(--raised);
  }
  .error {
    color: var(--reject);
  }
  .failed {
    border: 1px solid var(--reject);
    border-radius: var(--radius-s);
    padding: 6px 9px;
    font-size: var(--step--1);
  }
  .failed ul {
    margin: 4px 0 0;
    padding-left: 16px;
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
    margin-top: 4px;
  }
</style>
