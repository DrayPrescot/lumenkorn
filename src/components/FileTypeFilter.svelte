<script lang="ts">
  import { api } from "../lib/api";
  import { groupTypes, typeLabel, type FileType } from "../lib/filetypes";
  import { num, t } from "../lib/i18n";
  import { lib } from "../lib/store.svelte";
  import Icon from "./Icon.svelte";

  let open = $state(false);
  let types = $state<FileType[]>([]);
  let loading = $state(false);

  // Gewählte Filter für den Knopf: „Bearbeitet · ARW“
  const chosen = $derived([...new Set(lib.fileTypes.map(typeLabel))]);
  const label = $derived.by(() => {
    const parts: string[] = [];
    if (lib.editedFilter !== null) parts.push(lib.editedFilter ? t("Bearbeitet") : t("Unbearbeitet"));
    if (chosen.length) parts.push(chosen.length <= 2 ? chosen.join(", ") : t("{n} Dateitypen", { n: chosen.length }));
    return parts.length ? parts.join(" · ") : t("Filter");
  });
  const active = $derived(chosen.length > 0 || lib.editedFilter !== null);

  function setEdited(v: boolean | null) {
    lib.editedFilter = v;
    void lib.loadImages(true, true);
  }

  async function toggleOpen(e: MouseEvent) {
    e.stopPropagation();
    open = !open;
    if (!open) return;
    loading = true;
    try {
      const list = groupTypes(await api.fileTypes(lib.currentQuery()));
      // Gewählte Typen, die hier nicht vorkommen, trotzdem zeigen (abwählbar)
      for (const l of chosen) {
        if (!list.some((x) => x.label === l)) {
          const exts = lib.fileTypes.filter((e) => typeLabel(e) === l);
          list.push({ label: l, exts, raw: false, count: 0 });
        }
      }
      types = list;
    } catch (err) {
      lib.notify(String(err), "error");
    } finally {
      loading = false;
    }
  }

  const isOn = (ft: FileType) => ft.exts.some((e) => lib.fileTypes.includes(e));

  function apply(exts: string[]) {
    lib.fileTypes = [...new Set(exts)];
    // Im Entwickeln-Modus beim Foto bleiben, solange es noch passt
    void lib.loadImages(true, true);
  }

  function toggle(ft: FileType) {
    apply(isOn(ft) ? lib.fileTypes.filter((e) => !ft.exts.includes(e)) : [...lib.fileTypes, ...ft.exts]);
  }

  function toggleGroup(group: FileType[]) {
    const all = group.every(isOn);
    const exts = group.flatMap((g) => g.exts);
    apply(all ? lib.fileTypes.filter((e) => !exts.includes(e)) : [...lib.fileTypes, ...exts]);
  }

  const groups = $derived([
    { title: t("RAW"), items: types.filter((x) => x.raw) },
    { title: t("Andere Formate"), items: types.filter((x) => !x.raw) },
  ].filter((g) => g.items.length));
</script>

<svelte:window onclick={() => (open = false)} />

<div class="ftf">
  <button
    class="btn"
    class:on={active}
    aria-expanded={open}
    aria-haspopup="dialog"
    title={t("Nach Bearbeitung und Dateityp filtern (RAW, JPEG, PNG …)")}
    onclick={toggleOpen}
  >
    <Icon name="filter" size={14} />
    <span class="lbl">{label}</span>
  </button>
  {#if open}
    <div
      class="pop"
      role="dialog"
      aria-label={t("Filter")}
      tabindex="-1"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.key === "Escape" && (open = false)}
    >
      <div class="group">
        <div class="title">{t("Bearbeitung")}</div>
        {#each [[null, t("Alle Fotos")], [true, t("Nur bearbeitete")], [false, t("Nur unbearbeitete")]] as [v, l] (String(v))}
          <label class="opt">
            <input type="radio" name="edited" checked={lib.editedFilter === v} onchange={() => setEdited(v as boolean | null)} />
            <span>{l}</span>
          </label>
        {/each}
      </div>
      <div class="title types">{t("Dateityp")}</div>
      {#if loading && !types.length}
        <p class="muted">{t("Wird geladen …")}</p>
      {:else if !types.length}
        <p class="muted">{t("Keine Fotos in dieser Ansicht.")}</p>
      {:else}
        {#each groups as g (g.title)}
          <div class="group">
            <label class="head">
              <input
                type="checkbox"
                checked={g.items.every(isOn)}
                indeterminate={g.items.some(isOn) && !g.items.every(isOn)}
                onchange={() => toggleGroup(g.items)}
              />
              <span>{g.title}</span>
              <span class="n">{num(g.items.reduce((n, x) => n + x.count, 0))}</span>
            </label>
            {#each g.items as ft (ft.label)}
              <label class="row">
                <input type="checkbox" checked={isOn(ft)} onchange={() => toggle(ft)} />
                <span>{ft.label}</span>
                <span class="n">{num(ft.count)}</span>
              </label>
            {/each}
          </div>
        {/each}
        <div class="actions">
          <span class="muted small">{t("Ohne Auswahl werden alle Typen gezeigt.")}</span>
          <button class="btn quiet" disabled={!lib.fileTypes.length} onclick={() => apply([])}>{t("Alle Typen")}</button>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .ftf {
    position: relative;
    flex: none;
  }
  .btn.on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .lbl {
    max-width: 14ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pop {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 40;
    width: 260px;
    max-height: min(70vh, 520px);
    overflow: auto;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    box-shadow: 0 12px 32px rgb(0 0 0 / 0.45);
    padding: 8px;
  }
  .group + .group {
    margin-top: 8px;
    padding-top: 8px;
    border-top: 1px solid var(--line);
  }
  label {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 6px;
    border-radius: var(--radius-s);
    cursor: pointer;
  }
  label:hover {
    background: var(--panel);
  }
  .head {
    font-weight: 600;
  }
  .title {
    padding: 4px 6px 2px;
    font-size: var(--step--1);
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .title.types {
    margin-top: 8px;
    padding-top: 10px;
    border-top: 1px solid var(--line);
  }
  .row {
    padding-left: 22px;
  }
  .n {
    margin-left: auto;
    color: var(--muted);
    font-size: var(--step--1);
  }
  input[type="checkbox"],
  input[type="radio"] {
    accent-color: var(--accent);
    margin: 0;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 8px;
    padding: 8px 6px 2px;
    border-top: 1px solid var(--line);
  }
  .actions .muted {
    flex: 1;
  }
  .small {
    font-size: var(--step--1);
  }
  p {
    margin: 6px;
  }
</style>
