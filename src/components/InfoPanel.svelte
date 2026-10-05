<script lang="ts">
  import { api, faceUrl, type FaceInfo, type ImageDetail } from "../lib/api";
  import { shortPath } from "../lib/format";
  import { lib } from "../lib/store.svelte";
  import Icon from "./Icon.svelte";
  import { locale, num, t, tc } from "../lib/i18n";

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
  // Nur von der ID abhängen: Bewerten/Markieren ersetzt das Objekt des
  // fokussierten Fotos, das darf keine neue Anfrage auslösen.
  const focusId = $derived(lib.focused?.id);
  $effect(() => {
    const id = focusId;
    const v = id !== undefined ? lib.facesVersion[id] : 0;
    void v;
    if (id === undefined) {
      faces = [];
      return;
    }
    const timer = setTimeout(() => loadFaces(id), SETTLE);
    return () => clearTimeout(timer);
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
  /** Ab so vielen ausgewählten Fotos keine Personen-Übersicht laden (Strg+A
   *  bei 50 000 Fotos schickte sonst alle IDs bei jeder Auswahländerung). */
  const SEL_FACES_MAX = 500;
  const selTooMany = $derived(lib.selection.size > SEL_FACES_MAX);
  const selIds = $derived(lib.selection.size > 1 && !selTooMany ? [...lib.selection] : []);
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
    const timer = setTimeout(() => loadSelFaces(ids), SETTLE);
    return () => clearTimeout(timer);
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
        const label = f.personName ?? (f.personId === null ? t("Ohne Zuordnung") : t("Unbenannte Gruppe"));
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
      lib.notify(t(n === 1 ? "{n} Vorschlag für {name} bestätigt" : "{n} Vorschläge für {name} bestätigt", { n, name: g.label }));
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
          ? t(n === 1 ? "{name} aus {n} Foto entfernt" : "{name} aus {n} Fotos entfernt", { n, name: g.label })
          : t(n === 1 ? "{n} Foto „{name}“ zugeordnet" : "{n} Fotos „{name}“ zugeordnet", { n, name: name.trim() }),
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

  // Bewertung/Markierung zeigt das Panel aus `lib.focused`; neu laden nur bei
  // anderem Foto, neuen KI-Schlagworten oder neu geladener Liste (Rescan).
  $effect(() => {
    const id = focusId;
    const tags = id !== undefined ? lib.tagsVersion[id] : 0;
    const list = lib.listVersion;
    void tags, list;
    if (id === undefined) {
      detail = null;
      return;
    }
    let cancelled = false;
    const timer = setTimeout(() => {
      api.imageDetail(id).then((d) => !cancelled && (detail = d)).catch(() => {});
    }, SETTLE);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  });

  const date = $derived.by(() => {
    if (!detail?.capturedAt) return null;
    const d = new Date(detail.capturedAt);
    return {
      day: d.toLocaleDateString(locale, { weekday: "short", day: "numeric", month: "long", year: "numeric" }),
      time: d.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit" }),
    };
  });

  const exp = $derived.by(() => {
    const d = detail;
    if (!d) return [];
    return [
      d.exposure && { v: `${d.exposure} s`, l: tc("exif", "Belichtung") },
      d.aperture && { v: `f/${num(d.aperture)}`, l: t("Blende") },
      d.iso && { v: String(d.iso), l: t("ISO") },
      d.focalLength && { v: `${Math.round(d.focalLength)} mm`, l: t("Brennweite") },
    ].filter((x): x is { v: string; l: string } => !!x);
  });

  const mb = (b: number) => `${num(b / 1_000_000, { maximumFractionDigits: 1 })} MB`;

  async function addTag() {
    const name = newTag.trim();
    if (!name || !detail) return;
    newTag = "";
    try {
      detail.tags = await api.addTag(detail.id, name);
    } catch (e) {
      lib.notify(String(e), "error");
    }
  }

  async function removeTag(name: string) {
    if (!detail) return;
    try {
      detail.tags = await api.removeTag(detail.id, name);
    } catch (e) {
      lib.notify(String(e), "error");
    }
  }
</script>

<aside class="scroll">
  <datalist id="info-person-names">
    {#each lib.people.filter((p) => p.name) as p (p.id)}<option value={p.name}></option>{/each}
  </datalist>
  {#if lib.selection.size > 1}
    <section>
      <h3>{t("{n} Fotos ausgewählt", { n: lib.selection.size })}</h3>
      {#if lib.view === "compare" && lib.mode === "library"}
        <p class="muted">{t("Im Vergleich gelten Bewertung und Markierung für das hervorgehobene Foto, der Export für alle ausgewählten.")}</p>
      {:else}
        <p class="muted">{t("Bewertung, Markierung und Export gelten für alle ausgewählten Fotos.")}</p>
      {/if}
      <div class="actions">
        <button class="btn" onclick={onexport}><Icon name="export" size={14} /> {t("Exportieren …")}</button>
      </div>
    </section>
    {#if selTooMany}
      <section>
        <h4>{t("Personen in der Auswahl")}</h4>
        <p class="muted">{t("Personen werden bei höchstens {n} ausgewählten Fotos angezeigt.", { n: SEL_FACES_MAX })}</p>
      </section>
    {:else if selGroups.length}
      <section>
        <h4>{t("Personen in der Auswahl")}</h4>
        <p class="muted">{t("Änderungen gelten für alle {n} ausgewählten Fotos.", { n: lib.selection.size })}</p>
        <ul class="selfaces">
          {#each selGroups as g (g.key)}
            <li>
              <div class="selhead">
                <span class="pname-static">{g.label}</span>
                <span class="muted">{t(g.images === 1 ? "{n} Foto" : "{n} Fotos", { n: g.images })}</span>
                {#if g.open.length}
                  <button class="btn small confirm" disabled={selBusy} title={t("Vorschläge für {name} in allen ausgewählten Fotos bestätigen", { name: g.label })} onclick={() => confirmGroup(g)}>
                    ✓ {t("{n} bestätigen", { n: g.open.length })}
                  </button>
                {/if}
                {#if g.personId !== null}
                  <button class="icon-btn" disabled={selBusy} title={t("{name} aus allen ausgewählten Fotos entfernen", { name: g.label })} aria-label={t("Aus allen entfernen")} onclick={() => reassignGroup(g, null)}>
                    <Icon name="close" size={11} />
                  </button>
                {/if}
              </div>
              <form onsubmit={(e) => (e.preventDefault(), reassignGroup(g, selNames[g.key] ?? ""))}>
                <input
                  type="text"
                  placeholder={g.named ? t("Andere Person zuordnen …") : t("Person zuordnen …")}
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
      <p class="path" title={d.dir}>{shortPath(d.dir)}</p>
    </section>

    <section class="rate">
      <div class="stars" role="group" aria-label={t("Bewertung")}>
        {#each [1, 2, 3, 4, 5] as n}
          <button class:on={(lib.focused?.rating ?? 0) >= n} aria-label={t(n === 1 ? "{n} Stern" : "{n} Sterne", { n })} onclick={() => lib.rate(lib.focused?.rating === n ? 0 : n)}>
            <Icon name="star" size={18} fill={(lib.focused?.rating ?? 0) >= n} />
          </button>
        {/each}
      </div>
      <div class="flags">
        <button class="icon-btn" class:pick={lib.focused?.flag === 1} aria-label={t("Markieren (P)")} title={t("Markieren (P)")} onclick={() => lib.flag(1)}>
          <Icon name="flag" />
        </button>
        <button class="icon-btn" class:reject={lib.focused?.flag === -1} aria-label={t("Ablehnen (X)")} title={t("Ablehnen (X) – Umschalt+X: ablehnen und weiter")} onclick={() => lib.flag(-1)}>
          <Icon name="reject" />
        </button>
      </div>
    </section>

    <section>
      <dl>
        {#if date}
          <dt>{t("Aufnahme")}</dt>
          <dd>{t("{day}, {time} Uhr", { day: date.day, time: date.time })}</dd>
        {/if}
        {#if d.camera}
          <dt>{t("Kamera")}</dt>
          <dd>{d.camera}</dd>
        {/if}
        {#if d.lens}
          <dt>{t("Objektiv")}</dt>
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
          <dt>{t("Größe")}</dt>
          <dd>{d.width} × {d.height} px, {((d.width * d.height) / 1e6).toFixed(1)} MP</dd>
        {/if}
        <dt>{t("Datei")}</dt>
        <dd>{d.ext.toUpperCase()}, {mb(d.size)}</dd>
      </dl>
    </section>

    {#if faces.length}
      <section>
        <h4>{t("Personen")}</h4>
        <ul class="faces">
          {#each faces as f (f.id)}
            <li>
              <img src={faceUrl(f.id)} alt="" />
              {#if f.personName && f.confirmed}
                <button class="pname" onclick={() => f.personId && lib.setScope({ type: "person", id: f.personId })} title={t("Alle Fotos von {name}", { name: f.personName })}>
                  {f.personName}
                </button>
                <button class="icon-btn" title={t("Nicht {name}", { name: f.personName })} aria-label={t("Zuordnung entfernen")} onclick={() => faceAct(() => api.faceReject(f.id))}>
                  <Icon name="close" size={11} />
                </button>
              {:else if f.personName}
                <span class="suggest">{f.personName}?</span>
                <button class="mini" title={t("Ja, {name}", { name: f.personName })} onclick={() => faceAct(() => api.faceConfirm(f.id))}>✓</button>
                <button class="mini" title={t("Nein")} onclick={() => faceAct(() => api.faceReject(f.id))}>✕</button>
              {:else}
                <form onsubmit={(e) => (e.preventDefault(), assign(f))}>
                  <input
                    type="text"
                    placeholder={t("Name")}
                    list="info-person-names"
                    bind:value={faceNames[f.id]}
                    onkeydown={(e) => e.stopPropagation()}
                  />
                </form>
                <button class="icon-btn" title={t("Ignorieren (kein Gesicht / unwichtig)")} aria-label={t("Gesicht ignorieren")} onclick={() => faceAct(() => api.faceHide(f.id))}>
                  <Icon name="close" size={11} />
                </button>
              {/if}
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    <section>
      <h4>{t("Schlagworte")}</h4>
      <ul class="tags">
        {#each d.tags as tag (tag.name)}
          <li
            class:ai={tag.source === "ai"}
            title={tag.source === "ai" ? t("Von der KI erkannt, {p} % sicher", { p: Math.round((tag.confidence ?? 0) * 100) }) : t("Manuell vergeben")}
          >
            {#if tag.source === "ai"}<Icon name="sparkle" size={11} />{/if}
            {tag.name}
            <button aria-label={t("Schlagwort {name} entfernen", { name: tag.name })} onclick={() => removeTag(tag.name)}><Icon name="close" size={10} /></button>
          </li>
        {/each}
      </ul>
      <form onsubmit={(e) => (e.preventDefault(), addTag())}>
        <input type="text" placeholder={t("Schlagwort hinzufügen")} bind:value={newTag} onkeydown={(e) => e.stopPropagation()} />
      </form>
    </section>
  {:else if !lib.selection.size}
    <p class="none">{t("Wähle ein Foto, um Details zu sehen.")}</p>
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
