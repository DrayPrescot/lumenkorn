<script lang="ts">
  // Diashow bearbeiten: alle Dias in Reihenfolge (Ziehen ordnet um), rechts
  // eine laufende Vorschau des gewählten Dias, Dauer, Text, Ersatzfotos und die
  // Einstellungen der ganzen Diashow.
  import { onDestroy } from "svelte";
  import { imageUrl, isTauri } from "../lib/api";
  import { focusPosition, type AlbumCandidate, type CaptionMode } from "../lib/album";
  import { PLACE_KM, coverId, musicPlaces, textStyle, durationOf, formatDuration, frameAt, isContain, MAX_SECONDS, MIN_SECONDS, MAP_MIN_KM, photoDuration, slideAlternatives, slideSeconds, timeline, type Transition } from "../lib/slideshow";
  import { slideshow } from "../lib/slideshow.svelte";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";
  import { isTyping } from "../lib/keys";
  import { VIDEO_CODECS, VIDEO_SIZES, encodableCodecs, type VideoCodecKey, type VideoSize } from "../lib/slideshowVideo";
  import SlideStage from "./SlideStage.svelte";
  import SlideTextFields from "./SlideTextFields.svelte";
  import FontSelect from "./FontSelect.svelte";
  import Icon from "./Icon.svelte";

  /** Bildformat der Vorschau (wie ein Bildschirm) */
  const ASPECT = 16 / 9;
  const SIZE_KEY = "lumenkorn.slideshow.videoSize";
  const CODEC_KEY = "lumenkorn.slideshow.videoCodec";

  function stored<T extends string>(key: string, valid: Record<string, unknown>, fallback: T): T {
    try {
      const v = localStorage.getItem(key);
      return v && v in valid ? (v as T) : fallback;
    } catch {
      return fallback;
    }
  }

  /** Auflösung und Codec des Videos (zuletzt benutzte) */
  let videoSize = $state<VideoSize>(stored(SIZE_KEY, VIDEO_SIZES, "1080p"));
  let videoCodec = $state<VideoCodecKey>(stored(CODEC_KEY, VIDEO_CODECS, "avc"));
  /** Codecs, die dieses System in der gewählten Größe kodieren kann (`null`: wird geprüft) */
  let codecs = $state<Set<VideoCodecKey> | null>(null);
  $effect(() => {
    const size = videoSize;
    codecs = null;
    encodableCodecs(size).then((c) => {
      if (videoSize !== size) return;
      codecs = c;
      // Gewählter Codec geht nicht: den ersten, der geht
      if (!c.has(videoCodec)) videoCodec = (Object.keys(VIDEO_CODECS) as VideoCodecKey[]).find((k) => c.has(k)) ?? videoCodec;
    });
  });
  function exportVideo() {
    try {
      localStorage.setItem(SIZE_KEY, videoSize);
      localStorage.setItem(CODEC_KEY, videoCodec);
    } catch {
      // nur für diese Sitzung
    }
    slideshow.exportVideo(videoSize, videoCodec).catch((e) => lib.notify(String(e), "error"));
  }
  const exportPct = $derived(slideshow.exporting ? Math.round((100 * slideshow.exporting.done) / Math.max(1, slideshow.exporting.total)) : 0);

  const a = $derived(slideshow.state!);
  const s = $derived(a.settings);
  const tl = $derived(timeline(a, slideshow.byId));
  /** Kommt vor dem Foto `j` eine Karte? */
  const mapBefore = (j: number) => tl.entries[tl.photoEntry[j] - 1]?.kind === "map";
  /** Gibt es vor dem Foto `j` einen anderen Aufnahmeort (Karte möglich)? */
  function canMap(j: number): boolean {
    const c = slideshow.byId.get(a.slides[j]?.id);
    if (c?.lat == null) return false;
    for (let k = j - 1; k >= 0; k--) if (slideshow.byId.get(a.slides[k].id)?.lat != null) return true;
    return false;
  }
  const sel = $derived(slideshow.selection != null && slideshow.selection < a.slides.length ? slideshow.selection : null);
  const selSlide = $derived(sel != null ? a.slides[sel] : undefined);
  const selCand = $derived(selSlide ? slideshow.byId.get(selSlide.id) : undefined);
  const unused = $derived(a.candidates.length - a.slides.length);

  const alts = $derived.by(() => {
    if (selCand) return slideAlternatives(a, { group: selCand.group, events: [selCand.event] });
    return slideAlternatives(a, { events: [] });
  });

  function dateOf(c: AlbumCandidate | undefined): string {
    if (!c?.capturedAt) return "";
    const d = new Date(c.capturedAt);
    return Number.isNaN(d.getTime()) ? c.capturedAt : d.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
  }

  // --- Vorschau: das gewählte Dia in Schleife (sonst der Anfang) ---------------------
  let previewTime = $state(0);
  let raf = 0;
  let last = 0;
  function tick(now: number) {
    raf = requestAnimationFrame(tick);
    // Während des Abspielens im Vollbild ruht die Vorschau
    if (slideshow.playing) {
      last = 0;
      return;
    }
    // Gewähltes Foto samt Karte davor; ohne Auswahl der Anfang (Titelbild)
    const i = sel ?? 0;
    const e = sel == null ? 0 : (tl.photoEntry[i] ?? 0);
    const e0 = sel != null && mapBefore(i) ? e - 1 : e;
    const start = tl.starts[e0] ?? 0;
    const dur = a.slides[i] ? tl.starts[e] - start + durationOf(tl, e) : 1;
    const dt = last ? (now - last) / 1000 : 0;
    last = now;
    let next = previewTime + dt;
    if (next < start || next >= start + dur) next = start;
    previewTime = next;
  }
  raf = requestAnimationFrame(tick);
  onDestroy(() => cancelAnimationFrame(raf));
  const frame = $derived(frameAt(a, tl, previewTime, ASPECT, slideshow.byId));
  const secs = (v: number) => v.toLocaleString(undefined, { maximumFractionDigits: 1 });
  // Mit Musik im Takt bzw. angepasst weicht die Dauer von der eingestellten ab
  const musicTimed = $derived(!!a.music && (a.music.sync || (a.music.fit && a.music.mode === "sequence")));
  /** Foto des Titelbilds */
  const cover = $derived(coverId(a, slideshow.byId));
  /** Dauer ergibt sich aus der Länge der Musik */
  const fitted = $derived(!!a.music?.fit && a.music.mode === "sequence");
  /** „Lieder nach Ort“: die Orte, denen Lieder zugeordnet werden können */
  const places = $derived(a.music?.mode === "place" ? musicPlaces(a, slideshow.byId) : []);

  /** Browser-Vorschau: Musik über ein Dateifeld */
  let fileInput = $state<HTMLInputElement>();
  function chooseMusic() {
    if (isTauri) slideshow.addMusic();
    else fileInput?.click();
  }

  // --- Umordnen per Ziehen -------------------------------------------------------
  let dragFrom = $state<number | null>(null);
  let dropAt = $state<number | null>(null);

  function dragStart(e: DragEvent, i: number) {
    dragFrom = i;
    e.dataTransfer?.setData("text/plain", String(i));
    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
  }

  function dragOver(e: DragEvent, i: number) {
    if (dragFrom === null) return;
    e.preventDefault();
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    // Vor oder hinter dem Dia einfügen, je nach Mausposition
    dropAt = e.clientX < r.left + r.width / 2 ? i : i + 1;
  }

  function drop(e: DragEvent) {
    e.preventDefault();
    if (dragFrom !== null && dropAt !== null) {
      const to = dropAt > dragFrom ? dropAt - 1 : dropAt;
      slideshow.move(dragFrom, to);
    }
    dragFrom = dropAt = null;
  }

  const CAPTIONS: Record<CaptionMode, string> = {
    both: t("Datum und Ort"),
    date: t("Nur Datum"),
    place: t("Nur Ort"),
    none: t("Keine"),
  };
  const TRANSITIONS: Record<Transition, string> = {
    fade: t("Weich überblenden"),
    slide: t("Schieben"),
    none: t("Harter Schnitt"),
  };

  function onkeydown(e: KeyboardEvent) {
    if (isTyping(e) || e.ctrlKey || e.metaKey || e.altKey || slideshow.playing) return;
    if (e.key === "Escape" && sel != null) slideshow.selection = null;
    else if ((e.key === "Delete" || e.key === "Backspace") && sel != null) slideshow.remove(sel);
    else if (e.key === "ArrowRight" && a.slides.length) slideshow.selection = Math.min(a.slides.length - 1, (sel ?? -1) + 1);
    else if (e.key === "ArrowLeft" && a.slides.length) slideshow.selection = Math.max(0, (sel ?? 1) - 1);
    else if (e.key === " ") slideshow.play(sel ?? 0);
    else return;
    e.preventDefault();
  }
</script>

<svelte:window {onkeydown} />

<div class="show">
  <header>
    <div class="info">
      <strong>{a.title || t("Diashow")}</strong>
      <span class="muted">{t("{n} Fotos · {duration}", { n: a.slides.length, duration: formatDuration(tl.total) })}</span>
    </div>
    <div class="spacer"></div>
    {#if slideshow.exporting}
      <span class="muted">{t("Video speichern – {pct} %", { pct: exportPct })}</span>
      <div class="progress" role="progressbar" aria-valuenow={exportPct} aria-valuemin={0} aria-valuemax={100}><div class="bar" style:width="{exportPct}%"></div></div>
      <button class="btn quiet" onclick={() => slideshow.cancelExport()}>{t("Abbrechen")}</button>
    {:else}
      <button class="btn quiet" onclick={() => (slideshow.dialog = { scope: a.scope, title: a.title })} title={t("Mit anderem Thema oder Umfang neu erstellen")}>
        <Icon name="sliders" size={14} /> {t("Neu erstellen …")}
      </button>
      <span class="video">
        <select bind:value={videoSize} aria-label={t("Auflösung des Videos")} title={t("Auflösung des Videos")}>
          {#each Object.keys(VIDEO_SIZES) as k (k)}
            <option value={k}>{k}</option>
          {/each}
        </select>
        <select bind:value={videoCodec} aria-label={t("Videocodec")} title={t("Videocodec: H.264 spielt fast überall, H.265 und AV1 sind bei gleicher Qualität kleiner, brauchen aber neuere Geräte.")}>
          {#each Object.entries(VIDEO_CODECS) as [k, label] (k)}
            <option value={k} disabled={codecs !== null && !codecs.has(k as VideoCodecKey)}>{label}{codecs !== null && !codecs.has(k as VideoCodecKey) ? ` – ${t("nicht verfügbar")}` : ""}</option>
          {/each}
        </select>
        <button class="btn" onclick={exportVideo} disabled={!a.slides.length || (codecs !== null && !codecs.has(videoCodec))} title={t("Als MP4-Video speichern (mit Musik), z. B. für den Fernseher oder zum Teilen")}>
          <Icon name="export" size={14} /> {t("Als Video speichern …")}
        </button>
      </span>
      <button class="btn primary" onclick={() => slideshow.play(0)} disabled={!a.slides.length} title={t("Im Vollbild abspielen (Leertaste)")}>
        <Icon name="play" size={14} /> {t("Abspielen")}
      </button>
    {/if}
  </header>

  <div class="body">
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="strip" onclick={() => (slideshow.selection = null)} ondragover={(e) => e.preventDefault()} ondrop={drop}>
      {#each a.slides as slide, i (slide.id)}
        {@const c = slideshow.byId.get(slide.id)}
        {@const contain = isContain(slide, c, ASPECT)}
        {@const pos = contain ? [0.5, 0.5] : focusPosition(c?.aspect ?? 1.5, ASPECT, c?.focus)}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="slide"
          class:selected={sel === i}
          class:drop-before={dropAt === i && dragFrom !== null}
          class:drop-after={dropAt === i + 1 && i === a.slides.length - 1 && dragFrom !== null}
          class:dragging={dragFrom === i}
          draggable="true"
          ondragstart={(e) => dragStart(e, i)}
          ondragover={(e) => dragOver(e, i)}
          ondragend={() => (dragFrom = dropAt = null)}
          onclick={(e) => {
            e.stopPropagation();
            slideshow.selection = i;
          }}
          ondblclick={() => slideshow.play(i)}
          role="button"
          tabindex="-1"
          aria-label={t("Foto {n}", { n: i + 1 })}
        >
          <div class="thumb" class:contain>
            <img
              src={imageUrl("thumb", slide.id, lib.thumbVersion(slide.id))}
              alt=""
              loading="lazy"
              draggable="false"
              style:object-position="{pos[0] * 100}% {pos[1] * 100}%"
            />
            {#if slide.caption}
              <span class="cap" title={slide.caption}>{slide.caption}</span>
            {/if}
            {#if mapBefore(i)}
              <span class="mapmark" title={t("Davor fliegt eine Karte vom vorigen Ort hierher")}><Icon name="map" size={12} /></span>
            {/if}
          </div>
          <div class="meta">
            <span>{i + 1}</span>
            <span class:own={slide.seconds !== undefined}>{secs(photoDuration(tl, i))} s</span>
          </div>
        </div>
      {/each}
      {#if !a.slides.length}
        <p class="muted">{t("Die Diashow ist leer – rechts Fotos hinzufügen.")}</p>
      {/if}
    </div>

    <aside class="side">
      <div class="preview"><SlideStage {frame} title={a.title} /></div>

      {#if sel != null && selSlide && selCand}
        <h4>{t("Foto {n} von {total}", { n: sel + 1, total: a.slides.length })}</h4>
        <p class="muted small">{dateOf(selCand)}{selCand.place ? ` · ${selCand.place}` : ""}</p>
        <label class="field">
          <span>{t("Dauer")}</span>
          <span class="inline">
            <input
              type="range"
              min={MIN_SECONDS}
              max={15}
              step="0.5"
              value={slideSeconds(a, selSlide)}
              oninput={(e) => slideshow.setSlideSeconds(sel, Number(e.currentTarget.value))}
            />
            <span class="muted small">{secs(photoDuration(tl, sel))} s</span>
            {#if selSlide.seconds !== undefined}
              <button class="icon-btn" title={t("Wie eingestellt ({s} s)", { s: s.seconds })} aria-label={t("Wie eingestellt ({s} s)", { s: s.seconds })} onclick={() => slideshow.setSlideSeconds(sel, null)}>
                <Icon name="refresh" size={12} />
              </button>
            {/if}
          </span>
        </label>
        {#if musicTimed}
          <p class="muted small">{a.music?.sync ? t("Mit Musik: Das Foto steht mindestens so lange und wechselt auf dem nächsten passenden Schlag.") : t("Mit Musik: Die Dias ohne eigene Dauer teilen sich die Länge des Stücks.")}</p>
        {/if}
        <label class="field">
          <span>{t("Text")}</span>
          <textarea
            rows="2"
            maxlength="1000"
            value={selSlide.caption ?? ""}
            placeholder={t("ohne Text")}
            oninput={(e) => slideshow.setCaption(sel, e.currentTarget.value)}
            onkeydown={(e) => e.stopPropagation()}
          ></textarea>
        </label>
        {#if selSlide.auto === false}
          <button class="btn quiet link" onclick={() => slideshow.setCaption(sel, null)}>{t("Wieder automatisch beschriften")}</button>
        {/if}
        {#if selSlide.caption}
          <details class="style" open={!!selSlide.style}>
            <summary>{selSlide.style ? t("Eigener Stil für diesen Text") : t("Stil dieses Textes ändern")}</summary>
            <SlideTextFields stacked value={textStyle(selSlide.style, s.captionStyle)} onchange={(v) => slideshow.setSlideStyle(sel, v)} />
            {#if selSlide.style}
              <button class="btn quiet link" onclick={() => slideshow.setSlideStyle(sel, null)}>{t("Stil wie alle Texte")}</button>
            {/if}
          </details>
        {/if}
        <label class="check" title={canMap(sel) ? "" : t("Dieses oder die vorigen Fotos haben keinen Aufnahmeort.")}>
          <input type="checkbox" checked={mapBefore(sel)} disabled={!canMap(sel)} onchange={(e) => slideshow.setSlideMap(sel, e.currentTarget.checked)} />
          {t("Davor Karte vom vorigen Ort")}
        </label>
        <label class="check">
          <input
            type="checkbox"
            checked={isContain(selSlide, selCand, ASPECT)}
            onchange={(e) => slideshow.setFit(sel, e.currentTarget.checked ? "contain" : "cover")}
          />
          {t("Ganzes Foto zeigen (nicht beschneiden)")}
        </label>
        {#if a.title.trim()}
          <label class="check">
            <input type="checkbox" checked={cover === selSlide.id} disabled={cover === selSlide.id} onchange={() => slideshow.setCover(selSlide.id)} />
            {t("Als Titelbild verwenden")}
          </label>
        {/if}
        <div class="actions">
          <button class="btn quiet" disabled={sel === 0} onclick={() => slideshow.move(sel, sel - 1)}><Icon name="back" size={13} /> {t("Früher")}</button>
          <button class="btn quiet" disabled={sel >= a.slides.length - 1} onclick={() => slideshow.move(sel, sel + 1)}>{t("Später")} <Icon name="chevron" size={13} /></button>
          <button class="btn quiet" onclick={() => slideshow.play(sel)}><Icon name="play" size={13} /> {t("Ab hier abspielen")}</button>
          <button class="btn quiet danger" onclick={() => slideshow.remove(sel)}><Icon name="trash" size={13} /> {t("Entfernen")}</button>
        </div>
        <h4>{t("Ersetzen durch")}</h4>
        {#if alts.length}
          <div class="alts">
            {#each alts as c (c.id)}
              <button class="alt" class:same={c.group === selCand.group} title={dateOf(c)} onclick={() => slideshow.replace(sel, c.id)}>
                <img src={imageUrl("thumb", c.id, lib.thumbVersion(c.id))} alt="" loading="lazy" />
              </button>
            {/each}
          </div>
        {:else}
          <p class="muted small">{t("Alle Fotos sind schon in der Diashow.")}</p>
        {/if}
      {:else}
        <h4>{t("Diashow anpassen")}</h4>
        <p class="muted small">{t("Klicke ein Foto an, um Dauer und Text zu ändern oder es zu tauschen. Ziehen ändert die Reihenfolge, Doppelklick spielt ab dort ab.")}</p>
        {#if !a.themeUsed}
          <p class="muted small">{t("Das Thema wurde nicht berücksichtigt, weil die Fotos noch nicht von der KI analysiert sind.")}</p>
        {:else if a.unanalyzed}
          <p class="muted small">{t("{n} Fotos sind noch nicht von der KI analysiert und zählen nur nach Qualität.", { n: a.unanalyzed })}</p>
        {/if}
        <label class="field">
          <span>{t("Titel")}</span>
          <input type="text" maxlength="500" value={a.title} placeholder={t("ohne Titel")} oninput={(e) => slideshow.setTitle(e.currentTarget.value)} onkeydown={(e) => e.stopPropagation()} />
        </label>
        {#if a.title.trim() && cover !== undefined}
          <div class="cover">
            <img src={imageUrl("thumb", cover, lib.thumbVersion(cover))} alt="" />
            <span class="muted small">
              {a.cover !== undefined ? t("Titelbild: eigene Wahl") : t("Titelbild: das am besten bewertete Foto")}
              {#if a.cover !== undefined}
                <button class="btn quiet link" onclick={() => slideshow.setCover(null)}>{t("Wieder das beste Foto")}</button>
              {/if}
            </span>
          </div>
        {/if}
        <h4>{t("Texte")}</h4>
        <label class="field">
          <span>{t("Schrift")}</span>
          <FontSelect value={s.font} onchange={(f) => slideshow.setFont(f)} />
        </label>
        <details class="style">
          <summary>{t("Datum, Ort und eigene Texte")}</summary>
          <SlideTextFields stacked value={s.captionStyle} onchange={(v) => slideshow.setTextStyle("captionStyle", v)} />
        </details>
        {#if a.title.trim()}
          <details class="style">
            <summary>{t("Titel")}</summary>
            <SlideTextFields stacked value={s.titleStyle} onchange={(v) => slideshow.setTextStyle("titleStyle", v)} />
          </details>
        {/if}
        <h4>{t("Musik")}</h4>
        {#if a.music}
          <ol class="tracks">
            {#each a.music.tracks as tr, i (tr.path)}
              <li>
                <span class="name" title={tr.path}>♪ {tr.name}</span>
                <span class="muted small">{formatDuration(tr.beats.duration)}{tr.beats.bpm ? ` · ${t("{bpm} Schläge/min", { bpm: Math.round(tr.beats.bpm) })}` : ""}</span>
                <span class="trackbtns">
                  <button class="icon-btn" disabled={i === 0} title={t("Früher")} aria-label={t("Früher")} onclick={() => slideshow.moveTrack(i, -1)}><Icon name="back" size={12} /></button>
                  <button class="icon-btn" disabled={i === a.music.tracks.length - 1} title={t("Später")} aria-label={t("Später")} onclick={() => slideshow.moveTrack(i, 1)}><Icon name="chevron" size={12} /></button>
                  <button class="icon-btn" title={t("Entfernen")} aria-label={t("Entfernen")} onclick={() => slideshow.removeTrack(i)}><Icon name="trash" size={12} /></button>
                </span>
              </li>
            {/each}
          </ol>
          {#if a.music.tracks.length > 1}
            <div class="seg">
              <button class="btn quiet" class:on={a.music.mode === "sequence"} onclick={() => slideshow.setMusic({ mode: "sequence" })}>{t("Hintereinander mit Überblenden")}</button>
              <button class="btn quiet" class:on={a.music.mode === "place"} onclick={() => slideshow.setMusic({ mode: "place" })}>{t("Lieder nach Ort")}</button>
            </div>
            {#if a.music.mode === "place"}
              <p class="muted small">{t("Ein neuer Ort beginnt ab {km} km Entfernung. Hake je Ort die Lieder an, die dort nacheinander laufen sollen; ohne Haken bekommt der Ort reihum das nächste Lied. Beim Ortswechsel wird übergeblendet.", { km: PLACE_KM })}</p>
              <div class="places">
                {#each places as pl, pi (pi)}
                  {@const own = a.music.assign?.[pl.key] ?? []}
                  <div class="place">
                    <span class="pname">{pl.label || (pl.key ? t("Ort {n}", { n: pi + 1 }) : t("Ohne Aufnahmeort"))} <span class="muted small">· {t(pl.photos === 1 ? "{n} Foto" : "{n} Fotos", { n: pl.photos })}</span></span>
                    {#if pl.key}
                      {#each a.music.tracks as tr (tr.path)}
                        <label class="check small">
                          <input type="checkbox" checked={own.includes(tr.path)} onchange={(e) => slideshow.assignTrack(pl.key, tr.path, e.currentTarget.checked)} />
                          <span class="tname">{tr.name}</span>
                        </label>
                      {/each}
                      {#if !own.length}<span class="muted small">{t("automatisch")}</span>{/if}
                    {:else}
                      <span class="muted small">{t("automatisch")}</span>
                    {/if}
                  </div>
                {/each}
              </div>
            {/if}
          {/if}
          <label class="check">
            <input type="checkbox" checked={a.music.sync} disabled={!a.music.tracks.some((x) => x.beats.bpm)} onchange={(e) => slideshow.setMusic({ sync: e.currentTarget.checked })} />
            {t("Bildwechsel im Takt der Musik")}
          </label>
          {#if a.music.mode === "sequence"}
            <label class="check">
              <input type="checkbox" checked={a.music.fit} onchange={(e) => slideshow.setMusic({ fit: e.currentTarget.checked })} />
              {t("Diashow endet mit der Musik")}
            </label>
          {/if}
          <label class="field">
            <span>{t("Lautstärke")}</span>
            <input type="range" min="0" max="1" step="0.05" value={a.music.volume} oninput={(e) => slideshow.setMusic({ volume: Number(e.currentTarget.value) })} />
          </label>
          <div class="actions">
            <button class="btn quiet" disabled={slideshow.musicBusy} onclick={chooseMusic}><Icon name="plus" size={13} /> {t("Lied hinzufügen …")}</button>
            <button class="btn quiet danger" onclick={() => slideshow.removeMusic()}><Icon name="trash" size={13} /> {t("Ohne Musik")}</button>
          </div>
        {:else}
          <p class="muted small">{t("Mit Musik wechseln die Bilder auf Wunsch im Takt, und die Diashow endet mit dem Stück.")}</p>
          <div class="actions">
            <button class="btn quiet" disabled={slideshow.musicBusy} onclick={chooseMusic}><Icon name="plus" size={13} /> {t("Musik wählen …")}</button>
          </div>
        {/if}
        {#if slideshow.musicBusy}
          <p class="muted small">{t("Musik wird gelesen und der Takt erkannt …")}</p>
        {/if}
        {#if !isTauri}
          <input
            bind:this={fileInput}
            class="hidden"
            type="file"
            accept="audio/*"
            multiple
            onchange={(e) => {
              const files = [...(e.currentTarget.files ?? [])];
              if (files.length) slideshow.addMusic(files);
              e.currentTarget.value = "";
            }}
          />
        {/if}
        <h4>{t("Bilder")}</h4>
        <label class="field">
          <span>{fitted ? t("Dauer je Foto (ergibt sich aus der Musik)") : a.music?.sync ? t("Dauer je Foto (mindestens, bis zum nächsten Schlag)") : t("Dauer je Foto")}</span>
          <span class="inline">
            <input type="range" min={MIN_SECONDS} max={Math.min(15, MAX_SECONDS)} step="0.5" value={s.seconds} disabled={fitted} oninput={(e) => slideshow.setSeconds(Number(e.currentTarget.value))} />
            <span class="muted small">{fitted && a.slides.length ? `≈ ${secs(tl.total / a.slides.length)}` : secs(s.seconds)} s</span>
          </span>
        </label>
        <label class="field">
          <span>{t("Übergang")}</span>
          <select value={s.transition} onchange={(e) => slideshow.setTransition(e.currentTarget.value as Transition)}>
            {#each Object.entries(TRANSITIONS) as [v, l]}
              <option value={v}>{l}</option>
            {/each}
          </select>
        </label>
        <label class="check">
          <input type="checkbox" checked={s.kenBurns} onchange={(e) => slideshow.setKenBurns(e.currentTarget.checked)} />
          {t("Langsam zoomen und schwenken")}
        </label>
        <label class="check" title={t("Wenn der nächste Aufnahmeort mehr als {km} km entfernt ist", { km: MAP_MIN_KM })}>
          <input type="checkbox" checked={s.maps} onchange={(e) => slideshow.setMaps(e.currentTarget.checked)} />
          {t("Animierte Karte beim Ortswechsel")}
        </label>
        <label class="check">
          <input type="checkbox" checked={s.endMap} onchange={(e) => slideshow.setEndMap(e.currentTarget.checked)} />
          {t("Zum Schluss Karte mit allen Orten")}
        </label>
        <label class="field">
          <span>{t("Datum und Ort")}</span>
          <select value={s.captions} onchange={(e) => slideshow.setCaptions(e.currentTarget.value as CaptionMode)}>
            {#each Object.entries(CAPTIONS) as [v, l]}
              <option value={v}>{l}</option>
            {/each}
          </select>
        </label>
        <p class="muted small">{t("Wird eingeblendet, wo ein neuer Tag bzw. Ort beginnt. Eigene Texte bleiben beim Ändern erhalten.")}</p>
        <h4>{t("Foto hinzufügen")}</h4>
        <p class="muted small">{t(unused === 1 ? "{n} weiteres Foto steht zur Auswahl." : "{n} weitere Fotos stehen zur Auswahl.", { n: unused })}</p>
        {#if alts.length}
          <div class="alts">
            {#each alts as c (c.id)}
              <button class="alt" title={dateOf(c)} onclick={() => slideshow.add(c.id)}>
                <img src={imageUrl("thumb", c.id, lib.thumbVersion(c.id))} alt="" loading="lazy" />
              </button>
            {/each}
          </div>
        {/if}
        <div class="actions">
          <button class="btn quiet" onclick={() => slideshow.close()}>{t("Zurück zu den Fotos")}</button>
          <button class="btn quiet danger" onclick={() => slideshow.discard()}>{t("Diashow verwerfen")}</button>
        </div>
      {/if}
    </aside>
  </div>
</div>

<style>
  .show {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 14px;
    border-bottom: 1px solid var(--line);
    flex-wrap: wrap;
  }
  .info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .spacer {
    flex: 1;
  }
  .video {
    display: inline-flex;
    gap: 4px;
  }
  .progress {
    width: 160px;
    height: 6px;
    background: var(--raised);
    border-radius: 3px;
    overflow: hidden;
  }
  .bar {
    height: 100%;
    background: var(--accent);
  }
  .muted {
    color: var(--muted);
  }
  .small {
    font-size: var(--step--1);
    margin: 0;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 1fr 320px;
  }
  .strip {
    overflow: auto;
    padding: 16px;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(168px, 1fr));
    align-content: start;
    gap: 14px 10px;
    background: var(--bg);
  }
  .slide {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 4px;
    cursor: pointer;
    border-radius: var(--radius-s);
  }
  .slide.dragging {
    opacity: 0.4;
  }
  .slide.drop-before::before,
  .slide.drop-after::after {
    content: "";
    position: absolute;
    top: 0;
    bottom: 22px;
    width: 3px;
    border-radius: 2px;
    background: var(--accent);
  }
  .slide.drop-before::before {
    left: -7px;
  }
  .slide.drop-after::after {
    right: -7px;
  }
  .thumb {
    position: relative;
    aspect-ratio: 16 / 9;
    overflow: hidden;
    border-radius: var(--radius-s);
    background: #000;
    box-shadow: 0 1px 4px rgb(0 0 0 / 0.35);
  }
  .slide.selected .thumb {
    outline: 3px solid var(--accent);
    outline-offset: 1px;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
    pointer-events: none;
  }
  .thumb.contain img {
    object-fit: contain;
  }
  .cap {
    position: absolute;
    left: 6px;
    right: 6px;
    bottom: 5px;
    color: #fff;
    font-size: 11px;
    text-shadow: 0 0 4px rgb(0 0 0 / 0.9);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .mapmark {
    position: absolute;
    left: 5px;
    top: 5px;
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: rgb(0 0 0 / 0.6);
    color: #fff;
  }
  .meta {
    display: flex;
    justify-content: space-between;
    color: var(--muted);
    font-size: var(--step--1);
    padding: 0 2px;
  }
  .meta .own {
    color: var(--accent);
  }
  .side {
    border-left: 1px solid var(--line);
    background: var(--panel);
    padding: 14px;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .preview {
    position: relative;
    aspect-ratio: 16 / 9;
    border-radius: var(--radius-s);
    overflow: hidden;
    flex-shrink: 0;
  }
  h4 {
    margin: 4px 0 0;
    font-size: var(--step-0);
    font-weight: 600;
  }
  .field {
    display: grid;
    gap: 4px;
  }
  .field > span:first-child {
    color: var(--muted);
    font-size: var(--step--1);
  }
  .inline {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .inline input[type="range"] {
    flex: 1;
  }
  textarea {
    width: 100%;
    resize: vertical;
    font: inherit;
  }
  .link {
    align-self: flex-start;
  }
  .tracks {
    margin: 0;
    padding: 0;
    list-style: none;
    display: grid;
    gap: 6px;
  }
  .tracks li {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 0 6px;
    min-width: 0;
  }
  .tracks .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .trackbtns {
    grid-row: span 2;
    display: flex;
    align-items: center;
  }
  .style {
    display: grid;
    gap: 8px;
  }
  .style[open] {
    padding-bottom: 4px;
  }
  .style summary {
    cursor: pointer;
    color: var(--muted);
    font-size: var(--step--1);
  }
  .style[open] summary {
    margin-bottom: 6px;
  }
  .places {
    display: grid;
    gap: 8px;
  }
  .place {
    display: grid;
    gap: 2px;
    padding: 6px 8px;
    border: 1px solid var(--line);
    border-radius: var(--radius-s);
  }
  .pname {
    font-weight: 500;
  }
  .tname {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .seg {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .seg .on {
    color: var(--accent);
    border-color: var(--accent);
  }
  .cover {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .cover img {
    width: 64px;
    height: 36px;
    object-fit: cover;
    border-radius: var(--radius-s);
  }
  .cover .small {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
  }
  .hidden {
    display: none;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .danger {
    color: var(--reject);
  }
  .alts {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 4px;
  }
  .alt {
    padding: 0;
    border: 2px solid transparent;
    border-radius: var(--radius-s);
    background: var(--raised);
    aspect-ratio: 1;
    overflow: hidden;
    cursor: pointer;
  }
  .alt.same {
    border-color: color-mix(in srgb, var(--accent) 60%, transparent);
  }
  .alt:hover {
    border-color: var(--accent);
  }
  .alt img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  @media (max-width: 800px) {
    .body {
      grid-template-columns: 1fr;
      grid-template-rows: 1fr auto;
    }
    .side {
      border-left: 0;
      border-top: 1px solid var(--line);
      max-height: 45vh;
    }
  }
</style>
