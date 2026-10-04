<script lang="ts">
  import { api, faceUrl, type FaceInfo, type ImageDetail } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import Icon from "./Icon.svelte";

  let { onexport }: { onexport: () => void } = $props();
  let detail = $state<ImageDetail | null>(null);
  let newTag = $state("");

  // --- Personen ---------------------------------------------------------------
  let faces = $state<FaceInfo[]>([]);
  let faceNames = $state<Record<number, string>>({});
  async function loadFaces(id: number) {
    try {
      const got = await api.imageFaces(id);
      if (lib.focused?.id === id) faces = got; // veraltete Antwort verwerfen
    } catch {
      if (lib.focused?.id === id) faces = [];
    }
  }
  // Beim Durchblättern (Pfeiltaste gehalten) erst laden, wenn man kurz verweilt
  const SETTLE = 120;
  $effect(() => {
    const f = lib.focused;
    const v = f ? lib.facesVersion[f.id] : 0;
    void v;
    if (!f) {
      faces = [];
      return;
    }
    const t = setTimeout(() => loadFaces(f.id), SETTLE);
    return () => clearTimeout(t);
  });
  async function faceAct(fn: () => Promise<unknown>) {
    try {
      await fn();
    } catch (e) {
      lib.notify(String(e), "error");
    }
    await facesChanged();
  }

  /** Nach jeder Änderung an Gesichtern: Anzeigen und – in der Ansicht einer
   *  Person – die Fotoliste neu laden (falsch zugeordnete verschwinden). */
  async function facesChanged(target: number | null = null) {
    await lib.refreshPeople();
    // Ansicht einer Person, die es danach nicht mehr gibt (alle Fotos verschoben,
    // unbenannte Gruppe aufgelöst): zur neuen Person bzw. zu allen Fotos wechseln
    const s = lib.scope;
    if (s.type === "person" && !lib.people.some((p) => p.id === s.id)) {
      lib.setScope(target !== null ? { type: "person", id: target } : { type: "all" });
      return;
    }
    if (s.type === "person") await lib.loadImages(true);
    if (lib.focused) await loadFaces(lib.focused.id);
    if (selIds.length) await loadSelFaces(selIds);
  }

  // --- Personen in einer Mehrfachauswahl -------------------------------------
  let selFaces = $state<FaceInfo[]>([]);
  let selNames = $state<Record<string, string>>({});
  let selBusy = $state(false);
  const selIds = $derived(lib.selection.size > 1 ? [...lib.selection] : []);
  async function loadSelFaces(ids: number[]) {
    try {
      const got = await api.facesOfImages(ids);
      if (ids === selIds) selFaces = got; // veraltete Antwort verwerfen
    } catch {
      selFaces = [];
    }
  }
  $effect(() => {
    const ids = selIds;
    if (!ids.length) {
      selFaces = [];
      return;
    }
    const t = setTimeout(() => loadSelFaces(ids), SETTLE);
    return () => clearTimeout(t);
  });

  type FaceGroup = {
    key: string;
    personId: number | null;
    label: string;
    named: boolean;
    faceIds: number[];
    /** davon nur vorgeschlagen (noch zu bestätigen) */
    open: number[];
    images: number;
  };
  /** Gesichter der Auswahl nach Person: benannte, dann Gruppen, dann ohne Zuordnung.
   *  In der Ansicht einer Person steht diese zuerst. */
  const selGroups = $derived.by((): FaceGroup[] => {
    const by = new Map<string, FaceGroup & { imgs: Set<number> }>();
    for (const f of selFaces) {
      const key = f.personId === null ? "none" : String(f.personId);
      let g = by.get(key);
      if (!g) {
        const label = f.personName ?? (f.personId === null ? "Ohne Zuordnung" : "Unbenannte Gruppe");
        g = { key, personId: f.personId, label, named: !!f.personName, faceIds: [], open: [], images: 0, imgs: new Set() };
        by.set(key, g);
      }
      g.faceIds.push(f.id);
      if (f.personId !== null && !f.confirmed) g.open.push(f.id);
      g.imgs.add(f.imageId);
    }
    const scopePerson = lib.scope.type === "person" ? lib.scope.id : null;
    const rank = (g: FaceGroup) =>
      g.personId !== null && g.personId === scopePerson ? 0 : g.named ? 1 : g.personId !== null ? 2 : 3;
    return [...by.values()]
      .map(({ imgs, ...g }) => ({ ...g, images: imgs.size }))
      .sort((a, b) => rank(a) - rank(b) || b.images - a.images || a.label.localeCompare(b.label));
  });

  /** Vorschläge einer Person in allen ausgewählten Fotos bestätigen. */
  async function confirmGroup(g: FaceGroup) {
    if (selBusy || !g.open.length) return;
    selBusy = true;
    try {
      const n = await api.facesConfirm(g.open);
      lib.notify(`${n} ${n === 1 ? "Vorschlag" : "Vorschläge"} für ${g.label} bestätigt`);
    } catch (e) {
      lib.notify(String(e), "error");
    }
    selBusy = false;
    await facesChanged();
  }

  /** Alle Gesichter einer Gruppe in der Auswahl einer Person zuordnen
   *  (`name`) oder aus ihrer Person herausnehmen (`null`). */
  async function reassignGroup(g: FaceGroup, name: string | null) {
    if (selBusy) return;
    if (name !== null && !name.trim()) return;
    selBusy = true;
    selNames[g.key] = "";
    let target: number | null = null;
    try {
      target = await api.facesReassign(g.faceIds, name === null ? null : name.trim());
      const n = g.images;
      lib.notify(
        name === null
          ? `${g.label} aus ${n} ${n === 1 ? "Foto" : "Fotos"} entfernt`
          : `${n} ${n === 1 ? "Foto" : "Fotos"} „${name.trim()}“ zugeordnet`,
      );
    } catch (e) {
      lib.notify(String(e), "error");
    }
    selBusy = false;
    await facesChanged(target);
  }
  async function assign(f: FaceInfo) {
    const name = (faceNames[f.id] ?? "").trim();
    if (!name) return;
    faceNames[f.id] = "";
    await faceAct(() => api.faceAssign(f.id, name));
  }

  $effect(() => {
    const f = lib.focused;
    const rating = f?.rating; // bei Bewertungsänderung neu laden
    const flag = f?.flag;
    const tags = f ? lib.tagsVersion[f.id] : 0;
    void rating, flag, tags;
    if (!f) {
      detail = null;
      return;
    }
    let cancelled = false;
    const t = setTimeout(() => {
      api.imageDetail(f.id).then((d) => !cancelled && (detail = d)).catch(() => {});
    }, SETTLE);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  });

  const date = $derived.by(() => {
    if (!detail?.capturedAt) return null;
    const d = new Date(detail.capturedAt);
    return {
      day: d.toLocaleDateString("de-DE", { weekday: "short", day: "numeric", month: "long", year: "numeric" }),
      time: d.toLocaleTimeString("de-DE", { hour: "2-digit", minute: "2-digit" }),
    };
  });

  const exp = $derived.by(() => {
    const d = detail;
    if (!d) return [];
    return [
      d.exposure && { v: `${d.exposure} s`, l: "Belichtung" },
      d.aperture && { v: `f/${d.aperture.toLocaleString("de-DE")}`, l: "Blende" },
      d.iso && { v: String(d.iso), l: "ISO" },
      d.focalLength && { v: `${Math.round(d.focalLength)} mm`, l: "Brennweite" },
    ].filter((x): x is { v: string; l: string } => !!x);
  });

  const shortDir = (p: string) => {
    const parts = p.split(/[\\/]/).filter(Boolean);
    return parts.length > 3 ? `… / ${parts.slice(-3).join(" / ")}` : parts.join(" / ");
  };

  const mb = (b: number) => `${(b / 1_000_000).toLocaleString("de-DE", { maximumFractionDigits: 1 })} MB`;

  async function addTag() {
    const name = newTag.trim();
    if (!name || !detail) return;
    newTag = "";
    detail.tags = await api.addTag(detail.id, name);
  }

  async function removeTag(name: string) {
    if (detail) detail.tags = await api.removeTag(detail.id, name);
  }
</script>

<aside class="scroll">
  <datalist id="info-person-names">
    {#each lib.people.filter((p) => p.name) as p (p.id)}<option value={p.name}></option>{/each}
  </datalist>
  {#if lib.selection.size > 1}
    <section>
      <h3>{lib.selection.size} Fotos ausgewählt</h3>
      {#if lib.view === "compare" && lib.mode === "library"}
        <p class="muted">Im Vergleich gelten Bewertung und Markierung für das hervorgehobene Foto, der Export für alle ausgewählten.</p>
      {:else}
        <p class="muted">Bewertung, Markierung und Export gelten für alle ausgewählten Fotos.</p>
      {/if}
      <div class="actions">
        <button class="btn" onclick={onexport}><Icon name="export" size={14} /> Exportieren …</button>
      </div>
    </section>
    {#if selGroups.length}
      <section>
        <h4>Personen in der Auswahl</h4>
        <p class="muted">Änderungen gelten für alle {lib.selection.size} ausgewählten Fotos.</p>
        <ul class="selfaces">
          {#each selGroups as g (g.key)}
            <li>
              <div class="selhead">
                <span class="pname-static">{g.label}</span>
                <span class="muted">{g.images} {g.images === 1 ? "Foto" : "Fotos"}</span>
                {#if g.open.length}
                  <button class="btn small confirm" disabled={selBusy} title="Vorschläge für {g.label} in allen ausgewählten Fotos bestätigen" onclick={() => confirmGroup(g)}>
                    ✓ {g.open.length} bestätigen
                  </button>
                {/if}
                {#if g.personId !== null}
                  <button class="icon-btn" disabled={selBusy} title="{g.label} aus allen ausgewählten Fotos entfernen" aria-label="Aus allen entfernen" onclick={() => reassignGroup(g, null)}>
                    <Icon name="close" size={11} />
                  </button>
                {/if}
              </div>
              <form onsubmit={(e) => (e.preventDefault(), reassignGroup(g, selNames[g.key] ?? ""))}>
                <input
                  type="text"
                  placeholder={g.named ? "Andere Person zuordnen …" : "Person zuordnen …"}
                  list="info-person-names"
                  bind:value={selNames[g.key]}
                  disabled={selBusy}
                  onkeydown={(e) => e.stopPropagation()}
                />
              </form>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  {/if}

  {#if detail}
    {@const d = detail}
    <section>
      <h3 class="file" title={d.path}>{d.fileName}</h3>
      <p class="path" title={d.dir}>{shortDir(d.dir)}</p>
    </section>

    <section class="rate">
      <div class="stars" role="group" aria-label="Bewertung">
        {#each [1, 2, 3, 4, 5] as n}
          <button class:on={(lib.focused?.rating ?? 0) >= n} aria-label="{n} Sterne" onclick={() => lib.rate(lib.focused?.rating === n ? 0 : n)}>
            <Icon name="star" size={18} fill={(lib.focused?.rating ?? 0) >= n} />
          </button>
        {/each}
      </div>
      <div class="flags">
        <button class="icon-btn" class:pick={lib.focused?.flag === 1} aria-label="Markieren (P)" title="Markieren (P)" onclick={() => lib.flag(1)}>
          <Icon name="flag" />
        </button>
        <button class="icon-btn" class:reject={lib.focused?.flag === -1} aria-label="Ablehnen (X)" title="Ablehnen (X) – Umschalt+X: ablehnen und weiter" onclick={() => lib.flag(-1)}>
          <Icon name="reject" />
        </button>
      </div>
    </section>

    <section>
      <dl>
        {#if date}
          <dt>Aufnahme</dt>
          <dd>{date.day}, {date.time} Uhr</dd>
        {/if}
        {#if d.camera}
          <dt>Kamera</dt>
          <dd>{d.camera}</dd>
        {/if}
        {#if d.lens}
          <dt>Objektiv</dt>
          <dd>{d.lens}</dd>
        {/if}
      </dl>
      {#if exp.length}
        <div class="exposure" style:grid-template-columns="repeat({exp.length}, 1fr)">
          {#each exp as x (x.l)}
            <div><span>{x.v}</span><small>{x.l}</small></div>
          {/each}
        </div>
      {/if}
      <dl>
        {#if d.width && d.height}
          <dt>Größe</dt>
          <dd>{d.width} × {d.height} px, {((d.width * d.height) / 1e6).toFixed(1)} MP</dd>
        {/if}
        <dt>Datei</dt>
        <dd>{d.ext.toUpperCase()}, {mb(d.size)}</dd>
      </dl>
    </section>

    {#if faces.length}
      <section>
        <h4>Personen</h4>
        <ul class="faces">
          {#each faces as f (f.id)}
            <li>
              <img src={faceUrl(f.id)} alt="" />
              {#if f.personName && f.confirmed}
                <button class="pname" onclick={() => f.personId && lib.setScope({ type: "person", id: f.personId })} title="Alle Fotos von {f.personName}">
                  {f.personName}
                </button>
                <button class="icon-btn" title="Nicht {f.personName}" aria-label="Zuordnung entfernen" onclick={() => faceAct(() => api.faceReject(f.id))}>
                  <Icon name="close" size={11} />
                </button>
              {:else if f.personName}
                <span class="suggest">{f.personName}?</span>
                <button class="mini" title="Ja, {f.personName}" onclick={() => faceAct(() => api.faceConfirm(f.id))}>✓</button>
                <button class="mini" title="Nein" onclick={() => faceAct(() => api.faceReject(f.id))}>✕</button>
              {:else}
                <form onsubmit={(e) => (e.preventDefault(), assign(f))}>
                  <input
                    type="text"
                    placeholder="Name"
                    list="info-person-names"
                    bind:value={faceNames[f.id]}
                    onkeydown={(e) => e.stopPropagation()}
                  />
                </form>
                <button class="icon-btn" title="Ignorieren (kein Gesicht / unwichtig)" aria-label="Gesicht ignorieren" onclick={() => faceAct(() => api.faceHide(f.id))}>
                  <Icon name="close" size={11} />
                </button>
              {/if}
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    <section>
      <h4>Schlagworte</h4>
      <ul class="tags">
        {#each d.tags as t (t.name)}
          <li class:ai={t.source === "ai"} title={t.source === "ai" ? `Von der KI erkannt, ${Math.round((t.confidence ?? 0) * 100)} % sicher` : "Manuell vergeben"}>
            {#if t.source === "ai"}<Icon name="sparkle" size={11} />{/if}
            {t.name}
            <button aria-label="Schlagwort {t.name} entfernen" onclick={() => removeTag(t.name)}><Icon name="close" size={10} /></button>
          </li>
        {/each}
      </ul>
      <form onsubmit={(e) => (e.preventDefault(), addTag())}>
        <input type="text" placeholder="Schlagwort hinzufügen" bind:value={newTag} onkeydown={(e) => e.stopPropagation()} />
      </form>
    </section>
  {:else if !lib.selection.size}
    <p class="none">Wähle ein Foto, um Details zu sehen.</p>
  {/if}
</aside>

<style>
  aside {
    background: var(--panel);
    border-left: 1px solid var(--line);
    height: 100%;
    min-height: 0;
    padding: 6px 0 24px;
  }
  section {
    padding: 12px 16px;
    border-bottom: 1px solid var(--line);
  }
  section:last-child {
    border-bottom: 0;
  }
  h3 {
    margin: 0;
    font-size: var(--step-1);
    font-weight: 600;
  }
  .file {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    user-select: text;
  }
  .path {
    margin: 2px 0 0;
    color: var(--faint);
    font-size: var(--step--1);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    user-select: text;
  }
  h4 {
    margin: 0 0 8px;
    font-size: var(--step--1);
    font-weight: 600;
    color: var(--muted);
  }
  .rate {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .stars button {
    color: var(--faint);
    padding: 2px;
  }
  .stars button.on {
    color: var(--accent);
  }
  .flags {
    display: flex;
    gap: 2px;
  }
  .pick {
    color: var(--text);
    background: var(--raised);
  }
  .reject {
    color: var(--reject);
    background: var(--raised);
  }
  dl {
    display: grid;
    grid-template-columns: 76px 1fr;
    gap: 5px 10px;
    margin: 0;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
    user-select: text;
  }
  .exposure {
    display: grid;
    gap: 1px;
    margin: 12px 0;
    background: var(--line);
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
    overflow: hidden;
  }
  .exposure div {
    display: grid;
    background: var(--panel);
    padding: 7px 4px 6px;
    text-align: center;
  }
  .exposure span {
    font-weight: 600;
  }
  .exposure small {
    color: var(--faint);
    font-size: 10.5px;
  }
  .tags {
    list-style: none;
    margin: 0 0 8px;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
  }
  .tags li {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 4px 2px 8px;
    border-radius: 999px;
    background: var(--raised);
    border: 1px solid var(--line);
    font-size: var(--step--1);
  }
  .tags li.ai {
    border-style: dashed;
  }
  .tags li.ai :global(svg) {
    color: var(--accent);
  }
  .tags button {
    display: grid;
    padding: 3px;
    color: var(--faint);
    border-radius: 50%;
  }
  .tags button:hover {
    color: var(--text);
  }
  form input {
    width: 100%;
  }
  .actions {
    display: flex;
    gap: 6px;
    margin-top: 10px;
  }
  .none {
    color: var(--faint);
    padding: 16px;
    margin: 0;
  }
  .selfaces {
    list-style: none;
    margin: 8px 0 0;
    padding: 0;
    display: grid;
    gap: 10px;
  }
  .selhead {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .selhead .muted {
    margin-left: auto;
    font-size: var(--step--1);
  }
  .btn.small.confirm {
    padding: 1px 7px;
    font-size: var(--step--1);
    border-color: var(--accent);
    color: var(--accent);
  }
  .pname-static {
    font-weight: 500;
  }
  .selfaces input {
    width: 100%;
    margin-top: 4px;
  }
  .faces {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 6px;
  }
  .faces li {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .faces img {
    width: 34px;
    height: 34px;
    border-radius: 50%;
    object-fit: cover;
    flex: none;
    background: var(--raised);
  }
  .faces form {
    flex: 1;
    min-width: 0;
  }
  .faces input {
    width: 100%;
  }
  .pname {
    flex: 1;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pname:hover {
    color: var(--accent);
  }
  .suggest {
    flex: 1;
    color: var(--muted);
    font-style: italic;
  }
  .mini {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    border: 1px solid var(--line);
    font-size: 11px;
  }
  .mini:hover {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
