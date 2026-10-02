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
    if (lib.focused) await loadFaces(lib.focused.id);
    lib.refreshPeople();
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
        <datalist id="info-person-names">
          {#each lib.people.filter((p) => p.name) as p (p.id)}<option value={p.name}></option>{/each}
        </datalist>
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
