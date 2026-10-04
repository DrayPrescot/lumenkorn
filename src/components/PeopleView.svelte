<script lang="ts">
  // Personenübersicht: benannte Personen und automatisch gebildete Gruppen.
  // Gruppen benennen, Vorschläge prüfen, Fotos einer Person öffnen.
  import { api, faceUrl, type FaceInfo, type Person } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import Icon from "./Icon.svelte";
  import { isTyping } from "../lib/keys";
  import { onMount } from "svelte";
  import { t } from "../lib/i18n";

  const named = $derived(lib.people.filter((p) => p.name));
  const groups = $derived(lib.people.filter((p) => !p.name));
  /** Kleine Gruppen (einzelne Gesichter) zunächst ausblenden */
  let showSmall = $state(false);
  const shownGroups = $derived(showSmall ? groups : groups.filter((g) => g.faceCount >= 2));

  let review = $state<{ person: Person; faces: FaceInfo[] } | null>(null);
  let names = $state<Record<number, string>>({});
  /** Benannte Person, deren Name gerade bearbeitet wird */
  let editing = $state<number | null>(null);

  async function act(f: () => Promise<unknown>) {
    try {
      await f();
    } catch (e) {
      lib.notify(String(e), "error");
    }
    await lib.refreshPeople();
    if (review) {
      const p = lib.people.find((x) => x.id === review!.person.id);
      review = p ? { person: p, faces: await api.personFaces(p.id) } : null;
    }
  }

  async function rename(p: Person) {
    const name = (names[p.id] ?? "").trim();
    if (!name) return;
    names[p.id] = "";
    await act(() => api.personRename(p.id, name));
    lib.notify(t("„{name}“ gespeichert. Ähnliche Gesichter werden als Vorschläge zugeordnet.", { name }));
  }

  async function openReview(p: Person) {
    picked = new Set();
    review = { person: p, faces: await api.personFaces(p.id) };
  }

  // --- Mehrere Gesichter auswählen (Klick, Strg-Klick, Umschalt-Klick) ---------
  let picked = $state<Set<number>>(new Set());
  let anchor: number | null = null;
  function pick(e: MouseEvent, faceId: number) {
    const faces = review?.faces ?? [];
    const next = new Set(picked);
    if (e.shiftKey && anchor !== null) {
      const [a, b] = [faces.findIndex((f) => f.id === anchor), faces.findIndex((f) => f.id === faceId)].sort((x, y) => x - y);
      for (const f of faces.slice(a, b + 1)) next.add(f.id);
    } else if (next.has(faceId)) {
      next.delete(faceId);
    } else {
      next.add(faceId);
    }
    anchor = faceId;
    picked = next;
  }
  const pickedOpen = $derived(review ? review.faces.filter((f) => picked.has(f.id) && !f.confirmed).map((f) => f.id) : []);

  async function confirmFaces(ids: number[]) {
    if (!ids.length) return;
    await act(() => api.facesConfirm(ids));
    picked = new Set();
    lib.notify(t(ids.length === 1 ? "{n} Gesicht bestätigt" : "{n} Gesichter bestätigt", { n: ids.length }));
  }

  async function rejectFaces(ids: number[]) {
    if (!ids.length) return;
    await act(() => api.facesReassign(ids, null));
    picked = new Set();
    lib.notify(t(ids.length === 1 ? "{n} Gesicht herausgenommen" : "{n} Gesichter herausgenommen", { n: ids.length }));
  }

  function onkeydown(e: KeyboardEvent) {
    if (!review || isTyping(e)) return;
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a") {
      e.preventDefault();
      picked = new Set(review.faces.map((f) => f.id));
    } else if (e.key === "Escape" && picked.size) {
      e.stopPropagation();
      picked = new Set();
    }
  }

  function open(p: Person) {
    // Aus der Gesichter-Prüfung heraus: „Zurück zu Personen“ kehrt hierher zurück
    lib.reviewPerson = review ? p.id : null;
    lib.setScope({ type: "person", id: p.id });
    lib.view = "grid";
  }

  // Rückkehr aus den Fotos einer Person: deren Prüfung wieder öffnen
  onMount(() => {
    const id = lib.reviewPerson;
    lib.reviewPerson = null;
    const p = id === null ? undefined : lib.people.find((x) => x.id === id);
    if (p) openReview(p);
  });

  const st = $derived(lib.peopleStatus);
</script>

<svelte:window {onkeydown} />

<div class="people scroll">
  {#if review}
    {@const p = review.person}
    <div class="review-head">
      <button class="btn quiet" onclick={() => (review = null)}><Icon name="back" size={14} /> {t("Personen")}</button>
      <h2>{p.name ?? t("Unbenannte Gruppe")}</h2>
      <span class="muted"
        >{p.suggested
          ? t("{faces} Gesichter auf {images} Fotos, {n} vorgeschlagen", { faces: p.faceCount, images: p.imageCount, n: p.suggested })
          : t("{faces} Gesichter auf {images} Fotos", { faces: p.faceCount, images: p.imageCount })}</span
      >
      <span class="spacer"></span>
      {#if p.suggested}
        <button class="btn" onclick={() => confirmFaces(review!.faces.filter((x) => !x.confirmed).map((x) => x.id))}>
          {t("Alle {n} Vorschläge bestätigen", { n: p.suggested })}
        </button>
      {/if}
      <button class="btn" onclick={() => open(p)}>{t("Fotos anzeigen")}</button>
    </div>
    {#if picked.size}
      <div class="pickbar" role="toolbar" aria-label={t("Auswahl")}>
        <span>{t("{n} ausgewählt", { n: picked.size })}</span>
        <button class="btn primary" disabled={!pickedOpen.length} onclick={() => confirmFaces(pickedOpen)}>
          ✓ {pickedOpen.length ? t("{n} bestätigen", { n: pickedOpen.length }) : t("alle schon bestätigt")}
        </button>
        <button class="btn" onclick={() => rejectFaces([...picked])}>✕ {t("Herausnehmen")}</button>
        <button class="btn quiet" onclick={() => (picked = new Set())}>{t("Auswahl aufheben")}</button>
      </div>
    {:else}
      <p class="muted hint">
        {t("Gesichter anklicken, um mehrere auszuwählen (Umschalt-Klick: Bereich, Strg+A: alle) – dann gemeinsam bestätigen oder herausnehmen.")}
        {t("Einzeln: ✓ bestätigt, ✕ nimmt heraus (wird dieser Person nicht erneut vorgeschlagen).")}
      </p>
    {/if}
    <div class="faces">
      {#each review.faces as f (f.id)}
        <div class="face" class:suggested={!f.confirmed} class:picked={picked.has(f.id)}>
          <button class="pick" aria-pressed={picked.has(f.id)} aria-label={t("Auswählen")} onclick={(e) => pick(e, f.id)}>
            <img src={faceUrl(f.id)} alt="" loading="lazy" />
          </button>
          {#if picked.has(f.id)}<span class="tick" aria-hidden="true">✓</span>{/if}
          <div class="face-actions">
            {#if !f.confirmed}
              <button title={t("Bestätigen")} aria-label={t("Bestätigen")} onclick={() => act(() => api.faceConfirm(f.id))}>✓</button>
            {/if}
            <button title={p.name ? t("Nicht {name}", { name: p.name }) : t("Nicht diese Person")} aria-label={t("Herausnehmen")} onclick={() => act(() => api.faceReject(f.id))}>✕</button>
          </div>
        </div>
      {/each}
    </div>
  {:else}
    {#if st && !st.ready}
      <div class="box">
        <p>{t("Die Personenerkennung ist noch nicht installiert. Öffne „KI“ unten links und lade sie herunter (etwa {mb} MB).", { mb: Math.round(st.downloadSize / 1e6) })}</p>
      </div>
    {:else if st && st.analyzed < st.total}
      <p class="muted progress">{t("Gesichter werden gesucht: {done} von {total} Fotos, {faces} Gesichter gefunden …", { done: st.analyzed, total: st.total, faces: st.faces })}</p>
    {/if}

    <h3>{t("Personen")} <span class="muted">{named.length}</span></h3>
    {#if !named.length}
      <p class="muted hint">{t("Gib unten einer Gruppe einen Namen – ähnliche Gesichter auf anderen Fotos werden dann dieser Person vorgeschlagen.")}</p>
    {/if}
    <div class="cards">
      {#each named as p (p.id)}
        <div class="card">
          <button class="avatar" onclick={() => open(p)} title={t("Fotos von {name}", { name: p.name ?? "" })}>
            {#if p.coverFace}<img src={faceUrl(p.coverFace)} alt="" />{/if}
          </button>
          {#if editing === p.id}
            <form
              onsubmit={async (e) => {
                e.preventDefault();
                const v = (e.currentTarget.elements.namedItem("n") as HTMLInputElement).value;
                editing = null;
                await lib.renamePerson(p.id, v);
              }}
            >
              <!-- svelte-ignore a11y_autofocus -->
              <input
                type="text"
                name="n"
                value={p.name}
                autofocus
                onfocus={(e) => e.currentTarget.select()}
                onblur={() => (editing = null)}
                onkeydown={(e) => {
                  e.stopPropagation();
                  if (e.key === "Escape") editing = null;
                }}
              />
            </form>
          {:else}
            <button class="name" title={t("Klicken zum Umbenennen")} onclick={() => (editing = p.id)}>{p.name}</button>
          {/if}
          <div class="muted small">{t(p.imageCount === 1 ? "{n} Foto" : "{n} Fotos", { n: p.imageCount })}</div>
          <div class="row">
            {#if p.suggested}
              <button class="btn small accent" onclick={() => openReview(p)}>{t("{n} prüfen", { n: p.suggested })}</button>
            {:else}
              <button class="btn small quiet" onclick={() => openReview(p)}>{t("Gesichter")}</button>
            {/if}
            <button
              class="icon-btn"
              title={t("Person löschen (Fotos bleiben)")}
              aria-label={t("Person löschen")}
              onclick={() => confirm(t("„{name}“ löschen? Die Gesichter werden wieder unbenannt.", { name: p.name ?? "" })) && act(() => api.personDelete(p.id))}
            >
              <Icon name="close" size={12} />
            </button>
          </div>
        </div>
      {/each}
    </div>

    <h3>
      {t("Unbenannte Gruppen")} <span class="muted">{shownGroups.length}</span>
      {#if groups.length !== shownGroups.length || showSmall}
        <label class="check">
          <input type="checkbox" bind:checked={showSmall} /> {t("Einzelne Gesichter zeigen ({n})", { n: groups.length - groups.filter((g) => g.faceCount >= 2).length })}
        </label>
      {/if}
    </h3>
    <div class="cards">
      {#each shownGroups as p (p.id)}
        <div class="card">
          <button class="avatar" onclick={() => openReview(p)} title={t("Gesichter dieser Gruppe")}>
            {#if p.coverFace}<img src={faceUrl(p.coverFace)} alt="" loading="lazy" />{/if}
          </button>
          <form onsubmit={(e) => (e.preventDefault(), rename(p))}>
            <input
              type="text"
              placeholder={t("Wer ist das?")}
              list="person-names"
              bind:value={names[p.id]}
              onkeydown={(e) => e.stopPropagation()}
            />
          </form>
          <div class="muted small">
            {t(p.faceCount === 1 ? "{n} Gesicht" : "{n} Gesichter", { n: p.faceCount })}, {t(p.imageCount === 1 ? "{n} Foto" : "{n} Fotos", { n: p.imageCount })}
          </div>
        </div>
      {:else}
        <p class="muted hint">{t("Keine unbenannten Gruppen.")}</p>
      {/each}
    </div>
    <datalist id="person-names">
      {#each named as p (p.id)}<option value={p.name}></option>{/each}
    </datalist>
  {/if}
</div>

<style>
  .people {
    flex: 1;
    min-height: 0;
    padding: 16px 20px 32px;
    background: var(--bg);
  }
  h2,
  h3 {
    margin: 0;
    font-weight: 600;
  }
  h3 {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 18px 0 10px;
    font-size: var(--step-0);
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(132px, 1fr));
    gap: 14px;
  }
  .card {
    display: grid;
    justify-items: center;
    gap: 4px;
    text-align: center;
    min-width: 0;
  }
  .avatar {
    width: 104px;
    height: 104px;
    border-radius: 50%;
    overflow: hidden;
    background: var(--raised);
    border: 2px solid transparent;
  }
  .avatar:hover {
    border-color: var(--accent);
  }
  .avatar img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .name {
    font-weight: 600;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding: 1px 4px;
    border-radius: var(--radius-s);
    cursor: text;
  }
  .name:hover {
    background: var(--raised);
  }
  .card input {
    width: 124px;
    text-align: center;
  }
  .small {
    font-size: var(--step--1);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .btn.small {
    padding: 2px 8px;
    font-size: var(--step--1);
  }
  .btn.accent {
    border-color: var(--accent);
    color: var(--accent);
  }
  .hint {
    font-size: var(--step--1);
    margin: 0 0 8px;
  }
  .progress {
    margin: 0;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    font-weight: 400;
    font-size: var(--step--1);
    color: var(--muted);
  }
  .box {
    padding: 12px;
    background: var(--raised);
    border-radius: var(--radius-m);
  }
  .box p {
    margin: 0;
  }
  .review-head {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    margin-bottom: 8px;
  }
  .spacer {
    flex: 1;
  }
  .faces {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(110px, 1fr));
    gap: 10px;
  }
  .face {
    position: relative;
    border-radius: var(--radius-s);
    overflow: hidden;
    aspect-ratio: 1;
    background: var(--raised);
    outline: 2px solid transparent;
  }
  .face.suggested {
    outline: 2px dashed var(--accent);
    outline-offset: -2px;
  }
  .face.picked {
    outline: 3px solid var(--accent);
    outline-offset: -3px;
  }
  .face.picked img {
    opacity: 0.8;
  }
  .pick {
    display: block;
    width: 100%;
    height: 100%;
    padding: 0;
  }
  .tick {
    position: absolute;
    left: 6px;
    top: 6px;
    width: 22px;
    height: 22px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: var(--accent);
    color: #111;
    font-size: 13px;
    font-weight: 700;
    pointer-events: none;
  }
  .pickbar {
    position: sticky;
    top: -16px;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    padding: 8px 0;
    margin-bottom: 8px;
    background: var(--bg);
  }
  .pickbar span {
    margin-right: 4px;
  }
  .face img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .face-actions {
    position: absolute;
    right: 4px;
    bottom: 4px;
    display: flex;
    gap: 4px;
  }
  .face-actions button {
    width: 24px;
    height: 24px;
    border-radius: 50%;
    background: rgb(0 0 0 / 0.65);
    color: #fff;
    font-size: 12px;
  }
  .face-actions button:hover {
    background: var(--accent);
    color: #111;
  }
</style>
