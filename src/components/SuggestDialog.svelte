<script lang="ts">
  // „Orte vorschlagen“ (nur auf Anfrage): Fotos ohne Ort bekommen den Ort
  // zeitgleicher Fotos mit Ort (Handy neben der Kamera), die Abweichung der
  // Kamerauhr wird ausgeglichen. Übernommen wird nur, was angehakt ist.
  import { onMount } from "svelte";
  import { api, imageUrl, type LocationItem, type TimeSuggestions } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { num, t } from "../lib/i18n";
  import Icon from "./Icon.svelte";

  let { onclose }: { onclose: () => void } = $props();

  let dialog = $state<HTMLDivElement>();
  onMount(() => dialog?.focus());

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "F11") return;
    e.stopPropagation();
    if (e.key === "Escape" && !busy) onclose();
  }

  /** Bei einer Auswahl nur deren Fotos, sonst die ganze Ansicht */
  const only = $derived(lib.selection.size > 1 ? new Set(lib.selection) : null);
  const keep = (id: number) => !only || only.has(id);

  let busy = $state<string | null>(null);
  let error = $state<string | null>(null);

  let auto = $state(true);
  let sign = $state<1 | -1>(1);
  let hours = $state(0);
  let minutes = $state(0);
  let windowMin = $state(10);
  let time = $state<TimeSuggestions | null>(null);
  /** Abgewählte Orte (Schlüssel: Ortsname bzw. Koordinaten) */
  let timeOff = $state(new Set<string>());

  const timeMatches = $derived((time?.matches ?? []).filter((m) => keep(m.id)));
  const placeKey = (m: { place: string | null; lat: number; lon: number }) => m.place ?? `${m.lat.toFixed(3)}, ${m.lon.toFixed(3)}`;
  const timeGroups = $derived.by(() => {
    const g = new Map<string, { key: string; ids: number[]; pairs: [number, number][] }>();
    for (const m of timeMatches) {
      const k = placeKey(m);
      if (!g.has(k)) g.set(k, { key: k, ids: [], pairs: [] });
      g.get(k)!.ids.push(m.id);
      g.get(k)!.pairs.push([m.id, m.refId]);
    }
    return [...g.values()].sort((a, b) => b.ids.length - a.ids.length);
  });

  async function findTime() {
    busy = t("Suche zeitgleiche Fotos …");
    error = null;
    try {
      const offset = auto ? null : sign * (hours * 3600 + minutes * 60);
      time = await api.geoTimeSuggest(lib.currentQuery(), offset, windowMin * 60);
      timeOff = new Set();
    } catch (e) {
      error = String(e);
    } finally {
      busy = null;
    }
  }

  function offsetText(s: number) {
    const a = Math.abs(s);
    const h = Math.floor(a / 3600);
    const m = Math.round((a % 3600) / 60);
    const span = h ? t("{h} h {m} min", { h, m }) : t("{m} min", { m });
    return s === 0 ? t("keine Abweichung") : s > 0 ? t("geht {span} nach", { span }) : t("geht {span} vor", { span });
  }

  async function applyTime() {
    const items: LocationItem[] = timeMatches
      .filter((m) => !timeOff.has(placeKey(m)))
      .map((m) => ({ id: m.id, lat: m.lat, lon: m.lon, place: m.place }));
    await lib.applyLocations(items);
    lib.notify(t(items.length === 1 ? "Ort für 1 Foto übernommen." : "Orte für {n} Fotos übernommen.", { n: items.length }));
    onclose();
  }

  function toggle(set: Set<string>, key: string): Set<string> {
    const s = new Set(set);
    if (s.has(key)) s.delete(key);
    else s.add(key);
    return s;
  }

  const chosenTime = $derived(timeGroups.filter((g) => !timeOff.has(g.key)).reduce((n, g) => n + g.ids.length, 0));
</script>

<svelte:window onkeydowncapture={onkeydown} />

<div class="backdrop" role="presentation" onclick={() => !busy && onclose()}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
  <div bind:this={dialog} class="dialog" role="dialog" aria-labelledby="suggest-title" tabindex="-1" onclick={(e) => e.stopPropagation()}>
    <header>
      <h3 id="suggest-title"><Icon name="pin" size={15} /> {t("Orte vorschlagen")}</h3>
      <button class="icon-btn" aria-label={t("Schließen")} onclick={onclose} disabled={!!busy}><Icon name="close" size={14} /></button>
    </header>
    <p class="muted small">
      {only
        ? t("Für die {n} ausgewählten Fotos ohne Ort. Übernommen wird erst, was du bestätigst.", { n: only.size })
        : t("Für alle Fotos ohne Ort in dieser Ansicht. Übernommen wird erst, was du bestätigst.")}
    </p>
    <p class="small">
      {t("Übernimmt den Ort eines Fotos mit Ort (z. B. vom Handy), das kurz davor oder danach entstand. Weicht die Uhr der Kamera ab, wird das ausgeglichen.")}
    </p>
    <div class="form">
      <label><input type="radio" bind:group={auto} value={true} /> {t("Abweichung der Kamerauhr selbst schätzen")}</label>
      <label class="row">
        <input type="radio" bind:group={auto} value={false} />
        {t("Kamerauhr")}
        <select bind:value={sign} disabled={auto}>
          <option value={1}>{t("geht nach um")}</option>
          <option value={-1}>{t("geht vor um")}</option>
        </select>
        <input type="number" min="0" max="23" bind:value={hours} disabled={auto} aria-label={t("Stunden")} /> h
        <input type="number" min="0" max="59" bind:value={minutes} disabled={auto} aria-label={t("Minuten")} /> min
      </label>
      <label class="row">
        {t("Höchstens")}
        <input type="number" min="1" max="120" bind:value={windowMin} aria-label={t("Minuten")} />
        {t("Minuten Abstand zum Partnerfoto")}
      </label>
    </div>
    <div class="actions left">
      <button class="btn" onclick={findTime} disabled={!!busy}>{t("Suchen")}</button>
    </div>
    {#if time}
      {#if time.offsets.length}
        <ul class="offsets small">
          {#each time.offsets as o (o.camera ?? "")}
            <li>
              <strong>{o.camera ?? t("Unbekannte Kamera")}</strong>:
              {#if o.uncertain}
                <span class="warn">{t("Abweichung der Uhr nicht eindeutig – bitte oben selbst angeben.")}</span>
              {:else}
                {t("Uhr {offset}", { offset: offsetText(o.offsetS) })}
                <span class="muted">
                  {o.estimated ? t("(geschätzt aus {n} Übereinstimmungen)", { n: o.matched }) : t("(angegeben)")}
                </span>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
      {#if timeGroups.length}
        <p class="muted small">{t("Zum Prüfen: links das Foto ohne Ort, rechts das Foto, dessen Ort es bekommt.")}</p>
        <ul class="groups">
          {#each timeGroups as g (g.key)}
            <li>
              <label>
                <input type="checkbox" checked={!timeOff.has(g.key)} onchange={() => (timeOff = toggle(timeOff, g.key))} />
                <span class="name">{g.key}</span>
                <span class="muted">{t(g.ids.length === 1 ? "1 Foto" : "{n} Fotos", { n: g.ids.length })}</span>
              </label>
              <div class="thumbs">
                {#each g.pairs.slice(0, 3) as [id, ref] (id)}
                  <span class="pair">
                    <img src={imageUrl("thumb", id)} alt="" />
                    <span class="muted">→</span>
                    <img src={imageUrl("thumb", ref)} alt="" />
                  </span>
                {/each}
              </div>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="muted small">
          {t("Keine zeitgleichen Fotos mit Ort gefunden ({n} Fotos ohne Ort untersucht).", { n: num(time.candidates) })}
        </p>
      {/if}
      {#if time.withoutTime}
        <p class="muted small">{t("{n} Fotos ohne Aufnahmezeit übersprungen.", { n: num(time.withoutTime) })}</p>
      {/if}
    {/if}

    {#if busy}<p class="busy small">{busy}</p>{/if}
    {#if error}<p class="err small">{error}</p>{/if}

    <div class="actions">
      <button class="btn quiet" onclick={onclose} disabled={!!busy}>{t("Abbrechen")}</button>
      {#if timeGroups.length}
        <button class="btn primary" onclick={applyTime} disabled={!!busy || !chosenTime}>
          {t(chosenTime === 1 ? "Für 1 Foto übernehmen" : "Für {n} Fotos übernehmen", { n: chosenTime })}
        </button>
      {/if}
    </div>
  </div>
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
    width: min(620px, 100%);
    max-height: calc(100vh - 32px);
    overflow-y: auto;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    padding: 18px;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 12px;
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  h3 {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--step-1);
    font-weight: 600;
  }
  h3 :global(svg) {
    color: var(--accent);
  }
  p {
    margin: 0;
  }
  .small {
    font-size: var(--step--1);
  }
  .muted {
    color: var(--muted);
  }
  .err {
    color: var(--danger, #e5484d);
  }
  .busy {
    color: var(--accent);
  }
  .form {
    display: grid;
    gap: 8px;
    font-size: var(--step--1);
  }
  .form label {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  .form input[type="number"] {
    width: 4.5em;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .actions.left {
    justify-content: flex-start;
  }
  .offsets {
    margin: 0;
    padding-left: 18px;
  }
  .groups {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 8px;
    max-height: 42vh;
    overflow-y: auto;
  }
  .groups li {
    display: grid;
    gap: 6px;
    padding: 8px;
    background: var(--raised);
    border-radius: var(--radius-s);
  }
  .groups label {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .groups .name {
    flex: 1;
    min-width: 0;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .thumbs {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 14px;
  }
  .pair {
    display: flex;
    align-items: center;
    gap: 3px;
  }
  .warn {
    color: var(--danger, #e5484d);
  }
  .thumbs img {
    width: 66px;
    height: 44px;
    object-fit: cover;
    border-radius: 3px;
  }
</style>
