<script lang="ts">
  // Karte: Aufnahmeorte der Fotos in der aktuellen Ansicht (Ordner, Projekt,
  // Suche …) als gebündelte Marker auf OpenStreetMap. Ort setzen: Fotos im
  // Filmstreifen auswählen, dann Ort suchen und übernehmen, Fotos auf die
  // Karte ziehen oder rechts auf die Karte klicken.
  import { onMount, untrack } from "svelte";
  import L from "leaflet";
  import "leaflet/dist/leaflet.css";
  import "leaflet.markercluster/dist/MarkerCluster.css";
  import { api, tileUrlTemplate, type PlaceHit } from "../lib/api";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";
  import Filmstrip from "./Filmstrip.svelte";
  import Icon from "./Icon.svelte";

  const PHOTOS = "application/x-lumenkorn-photos";

  let el: HTMLDivElement;
  let map: L.Map | undefined;
  let cluster: L.MarkerClusterGroup | undefined;
  /** Marker je Foto mit Ort; Position zum Abgleich mit der Liste */
  const markers = new Map<number, { m: L.Marker; lat: number; lon: number }>();
  let ready = $state(false);
  let markerCount = $state(0);

  // Ortssuche
  let query = $state("");
  let hits = $state<PlaceHit[] | null>(null);
  let searching = $state(false);
  let searchError = $state<string | null>(null);
  /** Gewählter Treffer: Stecknadel auf der Karte, Übernehmen-Knopf */
  let picked = $state<PlaceHit | null>(null);
  let pin: L.Marker | undefined;
  /** Rechtsklick auf die Karte */
  let menu = $state<{ x: number; y: number; lat: number; lon: number } | null>(null);
  let dropHover = $state(false);

  /** Fotos, für die ein Ort gesetzt würde (Auswahl bzw. aktuelles Foto) */
  const count = $derived(lib.selection.size > 1 ? lib.selection.size : lib.focused ? 1 : 0);
  const withPlace = $derived(
    lib.selection.size > 1
      ? lib.images.filter((i) => lib.selection.has(i.id) && i.lat !== undefined).length
      : lib.focused?.lat !== undefined
        ? 1
        : 0,
  );

  const dot = (selected: boolean) =>
    L.divIcon({ className: selected ? "geo-dot sel" : "geo-dot", iconSize: [14, 14], html: "<span></span>" });
  const DOT = dot(false);
  const DOT_SEL = dot(true);

  function selectIds(ids: number[], add: boolean) {
    const set = new Set(add ? [...lib.selection, ...ids] : ids);
    const first = lib.images.findIndex((i) => set.has(i.id));
    if (first < 0) return;
    lib.selection = set;
    lib.focus = add && lib.focus >= 0 ? lib.focus : first;
  }

  onMount(() => {
    let disposed = false;
    // markercluster erwartet Leaflet als globales `L`
    (window as unknown as { L: typeof L }).L = L;
    import("leaflet.markercluster").then(() => {
      if (disposed) return;
      map = L.map(el, { worldCopyJump: true, zoomControl: true, attributionControl: true }).setView([30, 10], 2);
      map.attributionControl.setPrefix('<a href="https://leafletjs.com">Leaflet</a>');
      L.tileLayer(tileUrlTemplate(), {
        maxZoom: 19,
        attribution: `© <a href="https://www.openstreetmap.org/copyright">${t("OpenStreetMap-Mitwirkende")}</a>`,
      }).addTo(map);
      cluster = L.markerClusterGroup({
        chunkedLoading: true,
        showCoverageOnHover: false,
        spiderfyOnMaxZoom: true,
        maxClusterRadius: 50,
        iconCreateFunction: (c) => {
          const n = c.getChildCount();
          const size = n < 10 ? 30 : n < 100 ? 36 : n < 1000 ? 42 : 50;
          return L.divIcon({ className: "geo-cluster", iconSize: [size, size], html: `<span>${n}</span>` });
        },
      });
      // Bündel anklicken: dessen Fotos auswählen (Strg: hinzufügen) und hinzoomen
      cluster.on("clusterclick", (e: L.LeafletEvent & { layer: L.MarkerCluster; originalEvent?: MouseEvent }) => {
        const ids = e.layer.getAllChildMarkers().map((m: L.Marker) => (m.options as { photoId: number }).photoId);
        const ev = (e as unknown as { originalEvent?: MouseEvent }).originalEvent;
        selectIds(ids, !!(ev?.ctrlKey || ev?.metaKey));
      });
      cluster.on("click", (e: L.LeafletMouseEvent) => {
        const id = (e.layer.options as { photoId: number }).photoId;
        selectIds([id], e.originalEvent.ctrlKey || e.originalEvent.metaKey);
      });
      map.addLayer(cluster);
      map.on("contextmenu", (e: L.LeafletMouseEvent) => {
        menu = { x: e.containerPoint.x, y: e.containerPoint.y, lat: e.latlng.lat, lon: wrapLon(e.latlng.lng) };
      });
      map.on("click movestart", () => (menu = null));
      ready = true;
    });
    // Größe folgt dem Fenster (Info-Panel ein/aus, Seitenleiste)
    const ro = new ResizeObserver(() => map?.invalidateSize());
    ro.observe(el);
    return () => {
      disposed = true;
      ro.disconnect();
      map?.remove();
      map = undefined;
      markers.clear();
    };
  });

  const wrapLon = (lon: number) => ((((lon + 180) % 360) + 360) % 360) - 180;

  // Marker mit der Fotoliste abgleichen: nur Neues, Verschobenes und
  // Entferntes ändern (Bewertungen ändern die Liste auch, nicht die Orte)
  let fitted = false;
  $effect(() => {
    const images = lib.images;
    if (!ready || !cluster || !map) return;
    untrack(() => {
      const seen = new Set<number>();
      const add: L.Marker[] = [];
      const remove: L.Marker[] = [];
      for (const im of images) {
        if (im.lat === undefined || im.lon === undefined) continue;
        seen.add(im.id);
        const cur = markers.get(im.id);
        if (cur && cur.lat === im.lat && cur.lon === im.lon) continue;
        if (cur) remove.push(cur.m);
        const m = L.marker([im.lat, im.lon], {
          icon: lib.selection.has(im.id) ? DOT_SEL : DOT,
          photoId: im.id,
          title: im.fileName,
        } as L.MarkerOptions);
        markers.set(im.id, { m, lat: im.lat, lon: im.lon });
        add.push(m);
      }
      for (const [id, cur] of markers) {
        if (!seen.has(id)) {
          remove.push(cur.m);
          markers.delete(id);
        }
      }
      if (remove.length) cluster!.removeLayers(remove);
      if (add.length) cluster!.addLayers(add);
      markerCount = markers.size;
      // Beim ersten Mal auf alle Fotos mit Ort einpassen
      if (!fitted && markers.size) {
        fitted = true;
        map!.fitBounds(cluster!.getBounds(), { padding: [40, 40], maxZoom: 13 });
      }
    });
  });

  // Ausgewählte Fotos hervorheben (nur geänderte Marker anfassen)
  let shown = new Set<number>();
  $effect(() => {
    const sel = lib.selection;
    const focusId = lib.focused?.id;
    if (!ready) return;
    void markerCount;
    untrack(() => {
      const now = new Set(sel.size ? sel : focusId !== undefined ? [focusId] : []);
      for (const id of shown) if (!now.has(id)) markers.get(id)?.m.setIcon(DOT);
      for (const id of now) if (!shown.has(id)) markers.get(id)?.m.setIcon(DOT_SEL);
      shown = now;
    });
  });

  // Aktuelles Foto (Filmstreifen, Pfeiltasten) in den sichtbaren Bereich holen
  $effect(() => {
    const f = lib.focused;
    if (!ready || !map || f?.lat === undefined || f.lon === undefined) return;
    const ll = L.latLng(f.lat, f.lon);
    if (!map.getBounds().pad(-0.1).contains(ll)) map.panTo(ll);
  });

  async function search(e: SubmitEvent) {
    e.preventDefault();
    const q = query.trim();
    if (!q || searching) return;
    searching = true;
    searchError = null;
    try {
      hits = await api.geoSearch(q);
    } catch (err) {
      hits = null;
      searchError = String(err);
    } finally {
      searching = false;
    }
  }

  function pick(h: PlaceHit) {
    picked = h;
    hits = null;
    if (!map) return;
    pin?.remove();
    pin = L.marker([h.lat, h.lon], { icon: L.divIcon({ className: "geo-pin", iconSize: [26, 26], iconAnchor: [13, 26], html: "<span></span>" }) }).addTo(map);
    if (h.bbox) {
      map.flyToBounds(
        [
          [h.bbox[0], h.bbox[2]],
          [h.bbox[1], h.bbox[3]],
        ],
        { maxZoom: 14, duration: 0.8 },
      );
    } else map.flyTo([h.lat, h.lon], 12, { duration: 0.8 });
  }

  function clearPick() {
    picked = null;
    pin?.remove();
    pin = undefined;
  }

  async function applyPicked() {
    if (!picked || !count) return;
    await lib.setLocation(lib.targets(), picked.lat, picked.lon, picked.name);
    lib.notify(t(count === 1 ? "Ort „{place}“ gesetzt." : "Ort „{place}“ für {n} Fotos gesetzt.", { place: picked.name, n: count }));
    clearPick();
  }

  async function placeHere() {
    const m = menu;
    menu = null;
    if (!m || !count) return;
    await lib.setLocation(lib.targets(), m.lat, m.lon);
  }

  // Fotos aus dem Filmstreifen auf die Karte ziehen
  function ondragover(e: DragEvent) {
    if (!e.dataTransfer?.types.includes(PHOTOS)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
    dropHover = true;
  }
  function ondrop(e: DragEvent) {
    dropHover = false;
    const raw = e.dataTransfer?.getData(PHOTOS);
    if (!raw || !map) return;
    e.preventDefault();
    const ids = JSON.parse(raw) as number[];
    const r = el.getBoundingClientRect();
    const ll = map.containerPointToLatLng([e.clientX - r.left, e.clientY - r.top]);
    void lib.setLocation(ids, ll.lat, wrapLon(ll.lng));
  }

  // Links (Quellenangabe) im Browser öffnen statt in der App
  function onclickCapture(e: MouseEvent) {
    const a = (e.target as HTMLElement).closest?.("a[href^='http']") as HTMLAnchorElement | null;
    if (!a) return;
    e.preventDefault();
    void api.openUrl(a.href);
  }
</script>

<div class="mapview">
  <div class="bar">
    <form class="search" onsubmit={search}>
      <Icon name="search" size={14} />
      <input
        type="search"
        bind:value={query}
        placeholder={t("Ort suchen, z. B. München …")}
        aria-label={t("Ort suchen")}
        onkeydown={(e) => e.key === "Escape" && ((hits = null), (query = ""))}
      />
      <button class="btn small" type="submit" disabled={searching || !query.trim()}>{searching ? t("Suche …") : t("Suchen")}</button>
    </form>
    <span class="info">
      {#if count}
        {t(count === 1 ? "1 Foto ausgewählt" : "{n} Fotos ausgewählt", { n: count })}
        {#if withPlace}<span class="muted">· {t("{n} mit Ort", { n: withPlace })}</span>{/if}
      {:else}
        <span class="muted">{t("Wähle unten Fotos aus, um ihnen einen Ort zu geben.")}</span>
      {/if}
    </span>
    {#if withPlace}
      <button class="btn small quiet" onclick={() => lib.clearLocation(lib.targets())}>{t("Ort entfernen")}</button>
    {/if}
  </div>

  {#if hits || searchError}
    <div class="hits" role="listbox" aria-label={t("Gefundene Orte")}>
      {#if searchError}
        <p class="err">{searchError}</p>
      {:else if hits && !hits.length}
        <p class="muted">{t("Kein Ort gefunden.")}</p>
      {:else if hits}
        {#each hits as h (h.display + h.lat)}
          <button role="option" aria-selected="false" onclick={() => pick(h)}>
            <strong>{h.name}</strong>
            <span class="muted">{h.display}</span>
          </button>
        {/each}
      {/if}
    </div>
  {/if}

  {#if picked}
    <div class="picked">
      <Icon name="pin" size={14} />
      <span class="name">{picked.name}</span>
      {#if count}
        <button class="btn small primary" onclick={applyPicked}>
          {t(count === 1 ? "Für das Foto übernehmen" : "Für {n} Fotos übernehmen", { n: count })}
        </button>
      {:else}
        <span class="muted">{t("Erst Fotos auswählen.")}</span>
      {/if}
      <button class="icon-btn" aria-label={t("Schließen")} onclick={clearPick}><Icon name="close" size={12} /></button>
    </div>
  {/if}

  <!-- Menü und Hinweis neben, nicht in der Leaflet-Karte: Leaflet hält Klicks
       in seinem Element auf, Sveltes Klick-Handler (am Dokument) erreichten
       sie dort nie -->
  <div class="stage">
    <div
      class="map"
      class:drop={dropHover}
      bind:this={el}
      role="application"
      aria-label={t("Karte")}
      {ondragover}
      ondragleave={() => (dropHover = false)}
      {ondrop}
      onclickcapture={onclickCapture}
    ></div>
    {#if menu}
      <div class="menu" style="left: {menu.x}px; top: {menu.y}px" role="menu">
        <button role="menuitem" disabled={!count} onclick={placeHere}>
          <Icon name="pin" size={13} />
          {count ? t(count === 1 ? "Ausgewähltes Foto hierher" : "{n} ausgewählte Fotos hierher", { n: count }) : t("Erst Fotos auswählen")}
        </button>
      </div>
    {/if}
    {#if ready && !markerCount}
      <p class="empty">{t("Keines der Fotos in dieser Ansicht hat einen Aufnahmeort.")}</p>
    {/if}
  </div>

  <Filmstrip />
</div>

<style>
  .mapview {
    position: relative;
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
    flex-wrap: wrap;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: 0 1 380px;
    min-width: 220px;
    padding: 0 0 0 8px;
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
    background: var(--raised);
    color: var(--muted);
  }
  .search input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: transparent;
    padding: 6px 0;
    color: var(--text);
  }
  .search input:focus {
    outline: none;
  }
  .search .btn {
    border-radius: 0 var(--radius-s) var(--radius-s) 0;
  }
  .info {
    font-size: var(--step--1);
  }
  .muted {
    color: var(--muted);
  }
  .hits {
    position: absolute;
    top: 46px;
    left: 12px;
    z-index: 1000;
    width: min(460px, calc(100% - 24px));
    max-height: 50%;
    overflow-y: auto;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.35);
    padding: 4px;
  }
  .hits button {
    display: grid;
    width: 100%;
    text-align: left;
    padding: 6px 8px;
    border-radius: var(--radius-s);
    gap: 2px;
  }
  .hits button:hover {
    background: var(--raised);
  }
  .hits span,
  .hits p {
    font-size: var(--step--1);
  }
  .hits p {
    margin: 6px 8px;
  }
  .err {
    color: var(--danger, #e5484d);
  }
  .picked {
    position: absolute;
    top: 52px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 1000;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px 6px 12px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.35);
    max-width: calc(100% - 24px);
  }
  .picked :global(svg) {
    color: var(--accent);
    flex: none;
  }
  .picked .name {
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .stage {
    position: relative;
    flex: 1;
    min-height: 0;
  }
  .map {
    position: absolute;
    inset: 0;
    background: #aad3df;
  }
  .map.drop {
    outline: 2px dashed var(--accent);
    outline-offset: -2px;
  }
  .menu {
    position: absolute;
    z-index: 1100;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.35);
    padding: 4px;
  }
  .menu button {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    border-radius: var(--radius-s);
    white-space: nowrap;
  }
  .menu button:hover:not(:disabled) {
    background: var(--raised);
  }
  .empty {
    position: absolute;
    left: 50%;
    bottom: 16px;
    transform: translateX(-50%);
    z-index: 1000;
    margin: 0;
    padding: 6px 12px;
    background: var(--panel);
    border-radius: var(--radius-s);
    font-size: var(--step--1);
    color: var(--muted);
  }
  /* Marker und Bündel */
  /* Leaflet setzt `transform` am Marker selbst – Gestaltung am inneren Element */
  .map :global(.geo-dot span) {
    display: block;
    width: 100%;
    height: 100%;
    box-sizing: border-box;
    border-radius: 50%;
    background: var(--accent);
    border: 2px solid #fff;
    box-shadow: 0 1px 3px rgb(0 0 0 / 0.5);
  }
  .map :global(.geo-dot.sel) {
    z-index: 1000 !important;
  }
  .map :global(.geo-dot.sel span) {
    background: #2b7fff;
    transform: scale(1.35);
  }
  .map :global(.geo-cluster) {
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: color-mix(in srgb, var(--accent) 85%, transparent);
    border: 2px solid #fff;
    box-shadow: 0 1px 4px rgb(0 0 0 / 0.45);
    color: #1b1b1b;
    font-weight: 600;
    font-size: 12px;
  }
  .map :global(.geo-pin span) {
    display: block;
    width: 100%;
    height: 100%;
    box-sizing: border-box;
    background: #e5484d;
    border: 2px solid #fff;
    border-radius: 50% 50% 50% 0;
    transform: rotate(-45deg);
    box-shadow: 0 1px 4px rgb(0 0 0 / 0.5);
  }
  .map :global(.leaflet-control-attribution) {
    font-size: 11px;
  }
</style>
