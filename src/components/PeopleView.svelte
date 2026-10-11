<script lang="ts">
  // Personenübersicht: benannte Personen und automatisch gebildete Gruppen.
  // Gruppen benennen, Vorschläge prüfen, Fotos einer Person öffnen.
  import { api, faceUrl, type FaceInfo, type Person } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import Icon from "./Icon.svelte";
  import PersonName from "./PersonName.svelte";
  import { isTyping } from "../lib/keys";
  import { onMount, untrack } from "svelte";
  import { t } from "../lib/i18n";

  const named = $derived(lib.people.filter((p) => p.name));
  const groups = $derived(lib.people.filter((p) => !p.name));
  /** Alle unbenannten Gruppen stehen unter „Unbenannt“ (eingeklappt) – bei
   *  großen Bibliotheken sind das Hunderte, die die Übersicht verstopfen. Die
   *  größten zuerst (Reihenfolge vom Backend). */
  const singles = $derived(groups.filter((g) => g.faceCount < 2).length);
  /** „Unbenannt“ aufgeklappt; Karten werden erst dann gezeichnet */
  let showOther = $state(false);
  /** Einzelne Gesichter (Gruppen aus nur einem Gesicht) zunächst ausblenden */
  let showSmall = $state(false);
  const otherGroups = $derived(showSmall ? groups : groups.filter((g) => g.faceCount >= 2));

  // --- Personengruppen (Familie, Freunde …) ---------------------------------
  const ungrouped = $derived(named.filter((p) => p.groupId === null || !lib.personGroups.some((g) => g.id === p.groupId)));
  let creating = $state(false);
  let newGroup = $state("");
  let renamingGroup = $state<number | null>(null);
  /** Karte, deren Gruppen-Menü offen ist */
  let groupMenu = $state<number | null>(null);
  /** Gruppe unter der gezogenen Karte (`null` = „Sonstige“, keine Gruppe) */
  let dropTarget = $state<number | null | undefined>(undefined);
  /** Eine Karte wird gezogen (zeigt „Sonstige“ als Ablage, auch wenn leer) */
  let dragging = $state(false);

  async function createGroup(persons: number[] = []) {
    const name = newGroup.trim();
    if (!name) return;
    const id = await lib.groupAction(() => api.personGroupCreate(name, persons));
    if (id === null) return;
    newGroup = "";
    creating = false;
    groupMenu = null;
    // Neue Gruppe aufgeklappt zeigen
    if (lib.collapsed[`pgroup:${id}`]) lib.toggleSection(`pgroup:${id}`);
    lib.notify(persons.length ? t("Gruppe „{name}“ angelegt und zugeordnet", { name }) : t("Gruppe „{name}“ angelegt", { name }));
  }

  async function renameGroup(id: number, name: string) {
    if (renamingGroup !== id) return;
    renamingGroup = null;
    const g = lib.personGroups.find((x) => x.id === id);
    if (!name.trim() || name.trim() === g?.name) return;
    const other = lib.personGroups.find((x) => x.id !== id && x.name.toLowerCase() === name.trim().toLowerCase());
    if (other && !confirm(t("Die Gruppe „{name}“ gibt es schon. Beide zusammenlegen?", { name: other.name }))) return;
    await lib.groupAction(() => api.personGroupRename(id, name));
  }

  async function deleteGroup(id: number, name: string) {
    if (!confirm(t("Gruppe „{name}“ löschen? Die Personen bleiben erhalten, nur ohne Gruppe.", { name }))) return;
    await lib.groupAction(() => api.personGroupDelete(id));
  }

  async function setGroup(p: Person, group: number | null) {
    groupMenu = null;
    if (p.groupId === group) return;
    await lib.groupAction(() => api.personSetGroup([p.id], group));
  }

  function dragStart(e: DragEvent, p: Person) {
    e.dataTransfer?.setData("application/x-lumenkorn-person", String(p.id));
    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
    dragging = true;
  }
  function dragOver(e: DragEvent, group: number | null) {
    if (!e.dataTransfer?.types.includes("application/x-lumenkorn-person")) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    dropTarget = group;
  }
  function drop(e: DragEvent, group: number | null) {
    dropTarget = undefined;
    dragging = false;
    const id = Number(e.dataTransfer?.getData("application/x-lumenkorn-person"));
    const p = lib.people.find((x) => x.id === id);
    if (!p) return;
    e.preventDefault();
    setGroup(p, group);
  }

  let review = $state<{ person: Person; faces: FaceInfo[] } | null>(null);
  let names = $state<Record<number, string>>({});
  /** Benannte Person, deren Name gerade bearbeitet wird */
  let editing = $state<number | null>(null);

  /** Führt eine Änderung aus und lädt neu. Liefert, ob sie gelang – nur dann
   *  dürfen Aufrufer Erfolg melden (sonst überschriebe das die Fehlermeldung). */
  async function act(f: () => Promise<unknown>): Promise<boolean> {
    let ok = true;
    try {
      await f();
    } catch (e) {
      ok = false;
      lib.notify(String(e), "error");
    }
    await lib.refreshPeople();
    if (review) {
      const p = lib.people.find((x) => x.id === review!.person.id);
      review = p ? { person: p, faces: await api.personFaces(p.id) } : null;
    }
    return ok;
  }

  async function rename(p: Person) {
    const name = (names[p.id] ?? "").trim();
    if (!name) return;
    names[p.id] = "";
    if (!(await act(() => api.personRename(p.id, name)))) return;
    lib.notify(t("„{name}“ gespeichert. Ähnliche Gesichter werden als Vorschläge zugeordnet.", { name }));
  }

  /** Nach Umbenennen/Zuordnen: Prüfung der Person zeigen, die jetzt die
   *  Gesichter hat (bei Zuordnung eine andere als vorher). */
  async function reopen(id: number) {
    const p = lib.people.find((x) => x.id === id);
    if (p) await openReview(p);
    else review = null;
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
    if (!(await act(() => api.facesConfirm(ids)))) return;
    picked = new Set();
    lib.notify(t(ids.length === 1 ? "{n} Gesicht bestätigt" : "{n} Gesichter bestätigt", { n: ids.length }));
  }

  async function rejectFaces(ids: number[]) {
    if (!ids.length) return;
    if (!(await act(() => api.facesReassign(ids, null)))) return;
    picked = new Set();
    lib.notify(t(ids.length === 1 ? "{n} Gesicht herausgenommen" : "{n} Gesichter herausgenommen", { n: ids.length }));
  }

  function onkeydown(e: KeyboardEvent) {
    if (groupMenu !== null && e.key === "Escape") {
      e.stopPropagation();
      groupMenu = null;
      return;
    }
    if (!review || isTyping(e)) return;
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a") {
      e.preventDefault();
      picked = new Set(review.faces.map((f) => f.id));
    } else if (e.key === "Escape" && picked.size) {
      e.stopPropagation();
      picked = new Set();
    } else if (e.key === "Escape") {
      e.stopPropagation();
      review = null;
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

  // Werkzeugleiste: „‹ Personen“ statt „‹ Raster“, solange eine Prüfung offen ist
  $effect(() => {
    lib.peopleReviewOpen = !!review;
    return () => (lib.peopleReviewOpen = false);
  });
  let backSeen = lib.peopleBack;
  $effect(() => {
    if (lib.peopleBack !== backSeen) {
      backSeen = lib.peopleBack;
      untrack(() => (review = null));
    }
  });

  const st = $derived(lib.peopleStatus);
</script>

<svelte:window {onkeydown} onpointerdown={(e) => groupMenu !== null && !(e.target as Element).closest?.(".gwrap") && (groupMenu = null)} />

<div class="people scroll">
  {#if review}
    {@const p = review.person}
    <div class="review-head">
      <h2><PersonName person={p} big onrenamed={reopen} /></h2>
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

    <h3>
      {t("Personen")} <span class="muted">{named.length}</span>
      {#if named.length}
        <button class="btn small quiet" onclick={() => (creating = true)} title={t("Gruppe für Personen anlegen, z. B. Familie oder Freunde")}>
          <Icon name="plus" size={11} /> {t("Gruppe")}
        </button>
      {/if}
    </h3>
    {#if !named.length}
      <p class="muted hint">{t("Gib unten einer Gruppe einen Namen – ähnliche Gesichter auf anderen Fotos werden dann dieser Person vorgeschlagen.")}</p>
    {/if}
    {#if creating}
      <form class="newgroup" onsubmit={(e) => (e.preventDefault(), createGroup())}>
        <!-- svelte-ignore a11y_autofocus -->
        <input
          type="text"
          placeholder={t("Name der Gruppe, z. B. Familie")}
          bind:value={newGroup}
          autofocus
          onkeydown={(e) => {
            e.stopPropagation();
            if (e.key === "Escape") creating = false;
          }}
        />
        <button class="btn primary small" type="submit" disabled={!newGroup.trim()}>{t("Anlegen")}</button>
        <button class="btn quiet small" type="button" onclick={() => (creating = false)}>{t("Abbrechen")}</button>
      </form>
    {/if}

    {#each lib.personGroups as g (g.id)}
      {@const members = named.filter((p) => p.groupId === g.id)}
      {@const open = !lib.collapsed[`pgroup:${g.id}`]}
      <section
        class="pgroup"
        class:drop={dropTarget === g.id}
        role="group"
        aria-label={g.name}
        ondragover={(e) => dragOver(e, g.id)}
        ondragleave={() => dropTarget === g.id && (dropTarget = undefined)}
        ondrop={(e) => drop(e, g.id)}
      >
        <h3>
          {#if renamingGroup === g.id}
            <form onsubmit={(e) => (e.preventDefault(), renameGroup(g.id, (e.currentTarget.elements.namedItem("n") as HTMLInputElement).value))}>
              <!-- svelte-ignore a11y_autofocus -->
              <input
                type="text"
                name="n"
                value={g.name}
                autofocus
                onfocus={(e) => e.currentTarget.select()}
                onblur={(e) => renameGroup(g.id, e.currentTarget.value)}
                onkeydown={(e) => {
                  e.stopPropagation();
                  if (e.key === "Escape") renamingGroup = null;
                }}
              />
            </form>
          {:else}
            <button class="toggle" aria-expanded={open} onclick={() => lib.toggleSection(`pgroup:${g.id}`)} ondblclick={() => (renamingGroup = g.id)} title={t("Klick: auf-/zuklappen · Doppelklick: umbenennen")}>
              <Icon name="chevron" size={12} />
              {g.name} <span class="muted">{members.length}</span>
            </button>
            <span class="group-actions">
              <button class="icon-btn" title={t("Gruppe umbenennen")} aria-label={t("Gruppe umbenennen")} onclick={() => (renamingGroup = g.id)}>✎</button>
              <button class="icon-btn" title={t("Gruppe löschen (Personen bleiben)")} aria-label={t("Gruppe löschen")} onclick={() => deleteGroup(g.id, g.name)}>
                <Icon name="close" size={12} />
              </button>
            </span>
          {/if}
        </h3>
        {#if open}
          <div class="cards">
            {#each members as p (p.id)}
              {@render personCard(p)}
            {:else}
              <p class="muted hint">{t("Noch leer – Personen hierher ziehen oder an der Karte über das Ordnersymbol zuordnen.")}</p>
            {/each}
          </div>
        {/if}
      </section>
    {/each}

    {#if ungrouped.length || (lib.personGroups.length && dragging)}
      {@const open = !lib.collapsed["pgroup:other"]}
      <section
        class="pgroup"
        class:drop={dropTarget === null}
        role="group"
        aria-label={t("Sonstige")}
        ondragover={(e) => dragOver(e, null)}
        ondragleave={() => dropTarget === null && (dropTarget = undefined)}
        ondrop={(e) => drop(e, null)}
      >
        <h3>
          <button class="toggle" aria-expanded={open} onclick={() => lib.toggleSection("pgroup:other")}>
            <Icon name="chevron" size={12} />
            {t("Sonstige")} <span class="muted">{ungrouped.length}</span>
          </button>
          <span class="muted note">{t("benannt, in keiner Gruppe")}</span>
        </h3>
        {#if open}
          <div class="cards">
            {#each ungrouped as p (p.id)}
              {@render personCard(p)}
            {:else}
              <p class="muted hint">{t("Hierher ziehen, um eine Person aus ihrer Gruppe zu nehmen.")}</p>
            {/each}
          </div>
        {/if}
      </section>
    {/if}

    {#if groups.length}
      <h3>
        <button class="toggle" aria-expanded={showOther} onclick={() => (showOther = !showOther)}>
          <Icon name="chevron" size={12} />
          {t("Unbenannt")} <span class="muted">{otherGroups.length}</span>
        </button>
        <span class="muted note">{t("noch ohne Namen, die größten zuerst")}</span>
        {#if showOther && singles}
          <label class="check">
            <input type="checkbox" bind:checked={showSmall} /> {t("Einzelne Gesichter zeigen ({n})", { n: singles })}
          </label>
        {/if}
      </h3>
      {#if showOther}
        <div class="cards">
          {#each otherGroups as p (p.id)}
            {@render groupCard(p)}
          {:else}
            <p class="muted hint">{t("Nur einzelne Gesichter – Häkchen oben zeigt sie.")}</p>
          {/each}
        </div>
      {/if}
    {/if}
    <datalist id="person-names">
      {#each named as p (p.id)}<option value={p.name}></option>{/each}
    </datalist>
  {/if}
</div>

{#snippet personCard(p: Person)}
  <div class="card" role="group" aria-label={p.name} draggable="true" ondragstart={(e) => dragStart(e, p)} ondragend={() => ((dropTarget = undefined), (dragging = false))}>
    <button class="avatar" onclick={() => open(p)} title={t("Fotos von {name}", { name: p.name ?? "" })}>
      {#if p.coverFace}<img src={faceUrl(p.coverFace)} alt="" draggable="false" />{/if}
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
      <span class="gwrap">
        <button
          class="icon-btn"
          class:on={groupMenu === p.id}
          title={t("Gruppe zuordnen")}
          aria-label={t("Gruppe zuordnen")}
          aria-haspopup="menu"
          aria-expanded={groupMenu === p.id}
          onclick={() => (groupMenu = groupMenu === p.id ? null : p.id)}
        >
          <Icon name="folder" size={12} />
        </button>
        {#if groupMenu === p.id}
          <div class="gmenu" role="menu">
            {#each lib.personGroups as g (g.id)}
              <button role="menuitemradio" aria-checked={p.groupId === g.id} onclick={() => setGroup(p, g.id)}>
                <span class="mark">{p.groupId === g.id ? "✓" : ""}</span>{g.name}
              </button>
            {/each}
            {#if p.groupId !== null}
              <button role="menuitem" onclick={() => setGroup(p, null)}><span class="mark"></span>{t("Aus der Gruppe nehmen (Sonstige)")}</button>
            {/if}
            {#if lib.personGroups.length}<hr />{/if}
            <form class="gnew" onsubmit={(e) => (e.preventDefault(), createGroup([p.id]))}>
              <input
                type="text"
                placeholder={t("Neue Gruppe …")}
                bind:value={newGroup}
                onkeydown={(e) => {
                  e.stopPropagation();
                  if (e.key === "Escape") groupMenu = null;
                }}
              />
            </form>
          </div>
        {/if}
      </span>
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
{/snippet}

{#snippet groupCard(p: Person)}
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
{/snippet}

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
  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font: inherit;
    font-weight: 600;
  }
  .toggle :global(svg) {
    transition: transform 0.15s;
  }
  .toggle[aria-expanded="true"] :global(svg) {
    transform: rotate(90deg);
  }
  .pgroup {
    border-radius: var(--radius-m);
    margin: 0 -8px;
    padding: 0 8px 4px;
    outline: 2px dashed transparent;
    outline-offset: -2px;
  }
  .pgroup.drop {
    outline-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 6%, transparent);
  }
  .pgroup h3 {
    margin: 14px 0 10px;
  }
  .group-actions {
    display: inline-flex;
    gap: 2px;
    opacity: 0;
  }
  .pgroup h3:hover .group-actions,
  .group-actions:focus-within {
    opacity: 1;
  }
  .pgroup h3 form input {
    font: inherit;
    font-weight: 600;
    width: 220px;
  }
  .newgroup {
    display: flex;
    gap: 6px;
    align-items: center;
    margin: 0 0 8px;
  }
  .newgroup input {
    width: 240px;
  }
  .card[draggable="true"] {
    cursor: grab;
  }
  .gwrap {
    position: relative;
  }
  .icon-btn.on {
    color: var(--accent);
  }
  .gmenu {
    position: absolute;
    z-index: 20;
    top: calc(100% + 4px);
    left: 50%;
    transform: translateX(-50%);
    min-width: 180px;
    padding: 4px;
    display: grid;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.45);
    text-align: left;
  }
  .gmenu button {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    border-radius: var(--radius-s);
    white-space: nowrap;
  }
  .gmenu button:hover {
    background: var(--raised);
  }
  .gmenu .mark {
    width: 12px;
    color: var(--accent);
  }
  .gmenu hr {
    border: 0;
    border-top: 1px solid var(--line);
    margin: 4px 2px;
  }
  .gmenu .gnew input {
    width: 100%;
    text-align: left;
  }
  .note {
    font-weight: 400;
    font-size: var(--step--1);
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
