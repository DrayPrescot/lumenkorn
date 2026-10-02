<script lang="ts">
  // Personenübersicht: benannte Personen und automatisch gebildete Gruppen.
  // Gruppen benennen, Vorschläge prüfen, Fotos einer Person öffnen.
  import { api, faceUrl, type FaceInfo, type Person } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import Icon from "./Icon.svelte";

  const named = $derived(lib.people.filter((p) => p.name));
  const groups = $derived(lib.people.filter((p) => !p.name));
  /** Kleine Gruppen (einzelne Gesichter) zunächst ausblenden */
  let showSmall = $state(false);
  const shownGroups = $derived(showSmall ? groups : groups.filter((g) => g.faceCount >= 2));

  let review = $state<{ person: Person; faces: FaceInfo[] } | null>(null);
  let names = $state<Record<number, string>>({});

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
    lib.notify(`„${name}“ gespeichert. Ähnliche Gesichter werden als Vorschläge zugeordnet.`);
  }

  async function openReview(p: Person) {
    review = { person: p, faces: await api.personFaces(p.id) };
  }

  function open(p: Person) {
    lib.setScope({ type: "person", id: p.id });
    lib.view = "grid";
  }

  const st = $derived(lib.peopleStatus);
</script>

<div class="people scroll">
  {#if review}
    {@const p = review.person}
    <div class="review-head">
      <button class="btn quiet" onclick={() => (review = null)}><Icon name="back" size={14} /> Personen</button>
      <h2>{p.name ?? "Unbenannte Gruppe"}</h2>
      <span class="muted">{p.faceCount} Gesichter auf {p.imageCount} Fotos{p.suggested ? `, ${p.suggested} vorgeschlagen` : ""}</span>
      <span class="spacer"></span>
      {#if p.suggested}
        <button class="btn" onclick={() => act(async () => { for (const f of review!.faces.filter((x) => !x.confirmed)) await api.faceConfirm(f.id); })}>
          Alle bestätigen
        </button>
      {/if}
      <button class="btn" onclick={() => open(p)}>Fotos anzeigen</button>
    </div>
    <p class="muted hint">✓ bestätigt das Gesicht, ✕ nimmt es heraus (es wird dieser Person nicht erneut vorgeschlagen).</p>
    <div class="faces">
      {#each review.faces as f (f.id)}
        <div class="face" class:suggested={!f.confirmed}>
          <img src={faceUrl(f.id)} alt="" loading="lazy" />
          <div class="face-actions">
            {#if !f.confirmed}
              <button title="Bestätigen" aria-label="Bestätigen" onclick={() => act(() => api.faceConfirm(f.id))}>✓</button>
            {/if}
            <button title="Nicht {p.name ?? 'diese Person'}" aria-label="Herausnehmen" onclick={() => act(() => api.faceReject(f.id))}>✕</button>
          </div>
        </div>
      {/each}
    </div>
  {:else}
    {#if st && !st.ready}
      <div class="box">
        <p>Die Personenerkennung ist noch nicht installiert. Öffne „KI“ unten links und lade sie herunter (etwa {Math.round(st.downloadSize / 1e6)} MB).</p>
      </div>
    {:else if st && st.analyzed < st.total}
      <p class="muted progress">Gesichter werden gesucht: {st.analyzed} von {st.total} Fotos, {st.faces} Gesichter gefunden …</p>
    {/if}

    <h3>Personen <span class="muted">{named.length}</span></h3>
    {#if !named.length}
      <p class="muted hint">Gib unten einer Gruppe einen Namen – ähnliche Gesichter auf anderen Fotos werden dann dieser Person vorgeschlagen.</p>
    {/if}
    <div class="cards">
      {#each named as p (p.id)}
        <div class="card">
          <button class="avatar" onclick={() => open(p)} title="Fotos von {p.name}">
            {#if p.coverFace}<img src={faceUrl(p.coverFace)} alt="" />{/if}
          </button>
          <div class="name">{p.name}</div>
          <div class="muted small">{p.imageCount} {p.imageCount === 1 ? "Foto" : "Fotos"}</div>
          <div class="row">
            {#if p.suggested}
              <button class="btn small accent" onclick={() => openReview(p)}>{p.suggested} prüfen</button>
            {:else}
              <button class="btn small quiet" onclick={() => openReview(p)}>Gesichter</button>
            {/if}
            <button
              class="icon-btn"
              title="Person löschen (Fotos bleiben)"
              aria-label="Person löschen"
              onclick={() => confirm(`„${p.name}“ löschen? Die Gesichter werden wieder unbenannt.`) && act(() => api.personDelete(p.id))}
            >
              <Icon name="close" size={12} />
            </button>
          </div>
        </div>
      {/each}
    </div>

    <h3>
      Unbenannte Gruppen <span class="muted">{shownGroups.length}</span>
      {#if groups.length !== shownGroups.length || showSmall}
        <label class="check">
          <input type="checkbox" bind:checked={showSmall} /> Einzelne Gesichter zeigen ({groups.length - groups.filter((g) => g.faceCount >= 2).length})
        </label>
      {/if}
    </h3>
    <div class="cards">
      {#each shownGroups as p (p.id)}
        <div class="card">
          <button class="avatar" onclick={() => openReview(p)} title="Gesichter dieser Gruppe">
            {#if p.coverFace}<img src={faceUrl(p.coverFace)} alt="" loading="lazy" />{/if}
          </button>
          <form onsubmit={(e) => (e.preventDefault(), rename(p))}>
            <input
              type="text"
              placeholder="Wer ist das?"
              list="person-names"
              bind:value={names[p.id]}
              onkeydown={(e) => e.stopPropagation()}
            />
          </form>
          <div class="muted small">{p.faceCount} {p.faceCount === 1 ? "Gesicht" : "Gesichter"}, {p.imageCount} {p.imageCount === 1 ? "Foto" : "Fotos"}</div>
        </div>
      {:else}
        <p class="muted hint">Keine unbenannten Gruppen.</p>
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
