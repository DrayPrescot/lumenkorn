<script lang="ts">
  // Reiter „Masken“: Ebenenliste, Formen der aktiven Ebene, Pinsel und lokale
  // Regler. Aufbau wie RepairPanel, aber mit anderen Werkzeugen, Zuständen und
  // Abständen – deshalb bewusst getrennt; gemeinsam ist nur `describeShape`.
  import { oklabToCss, type LocalAdjust } from "../lib/api";
  import { ANIMAL_KINDS, describeShape, dev, PERSON_PARTS, type NewMaskKind } from "../lib/develop.svelte";
  import { lib } from "../lib/store.svelte";
  import { t } from "../lib/i18n";
  import Icon from "./Icon.svelte";
  import Slider from "./Slider.svelte";

  let menu = $state<{ mode: "new" | "add" | "subtract" } | null>(null);
  /** Aufgeklappte Gruppe im Menü */
  let group = $state<"person" | "animal" | null>(null);
  let renaming = $state<number | null>(null);

  const aiReady = $derived(!!lib.ai?.masksReady);
  const partsReady = $derived(aiReady && !!lib.ai?.partsReady);

  const kinds: { kind: NewMaskKind; label: string; hint: string; ai?: boolean }[] = [
    { kind: "subject", label: t("Motiv"), hint: t("Hauptmotiv automatisch erkennen"), ai: true },
    { kind: "sky", label: t("Himmel"), hint: t("Himmel automatisch erkennen") },
    { kind: "object", label: t("Objekt"), hint: t("Auf ein Objekt klicken"), ai: true },
    { kind: "brush", label: t("Pinsel"), hint: t("Bereich übermalen") },
    { kind: "color", label: t("Farbe"), hint: t("Farbe mit der Pipette wählen") },
    { kind: "linear", label: t("Verlauf"), hint: t("Auf dem Bild ziehen") },
    { kind: "radial", label: t("Radial"), hint: t("Ellipse auf dem Bild ziehen") },
  ];

  const sliders: { key: keyof LocalAdjust; label: string; min: number; max: number; step?: number; decimals?: number; track?: string }[] = [
    { key: "exposure", label: t("Belichtung"), min: -4, max: 4, step: 0.05, decimals: 2 },
    { key: "contrast", label: t("Kontrast"), min: -100, max: 100 },
    { key: "highlights", label: t("Lichter"), min: -100, max: 100 },
    { key: "shadows", label: t("Tiefen"), min: -100, max: 100 },
    { key: "clarity", label: t("Klarheit"), min: -100, max: 100 },
    { key: "temperature", label: t("Temperatur"), min: -100, max: 100, track: "linear-gradient(to right, #4f78c4, #8a8a8a, #d6b04a)" },
    { key: "tint", label: t("Tönung"), min: -100, max: 100, track: "linear-gradient(to right, #5aa35a, #8a8a8a, #b65aa8)" },
    { key: "saturation", label: t("Sättigung"), min: -100, max: 100 },
  ];

  function pick(kind: NewMaskKind) {
    const m = menu;
    menu = null;
    group = null;
    if (!m) return;
    if (m.mode === "new") dev.addMask(kind);
    else dev.addShape(kind, m.mode === "subtract");
  }

  function toggleShape(i: number, key: "invert" | "subtract") {
    const s = dev.layer?.shapes[i];
    if (!s || (key === "invert" && s.type === "brush")) return;
    (s as { invert?: boolean; subtract?: boolean })[key] = !(s as Record<string, unknown>)[key];
    dev.schedule();
    dev.commit();
  }

  function setRange(i: number, v: number) {
    const s = dev.layer?.shapes[i];
    if (s?.type !== "color") return;
    s.range = v;
    if (!dev.showOverlay) dev.showOverlay = true; // beim Einstellen die Auswahl zeigen
    dev.schedule();
  }

  function removeShape(i: number) {
    dev.layer?.shapes.splice(i, 1);
    dev.schedule();
    dev.commit();
  }

  const hint = $derived.by(() => {
    if (dev.maskBusy) return dev.maskBusy;
    switch (dev.tool) {
      case "linear":
        return t("Ziehe auf dem Bild von der vollen zur ausgeblendeten Wirkung.");
      case "radial":
        return t("Ziehe vom Mittelpunkt nach außen.");
      case "brush":
        return t("Male auf dem Bild. Mit gedrückter Alt-Taste radierst du.");
      case "object":
        return t("Klicke auf das Objekt. Weitere Klicks verfeinern, Alt-Klick nimmt Bereiche heraus.");
      case "color":
        return t("Klicke mit der Pipette auf die gewünschte Farbe. Weitere Klicks nehmen Farbtöne hinzu, Alt-Klick entfernt den nächstgelegenen.");
    }
    return null;
  });
</script>

<svelte:window onclick={() => ((menu = null), (group = null))} />

<section class="masks">
  <header>
    <h4>{t("Masken")}</h4>
    <div class="anchor">
      <button class="btn small" onclick={(e) => { e.stopPropagation(); menu = menu?.mode === "new" ? null : { mode: "new" }; }}>
        <Icon name="plus" size={12} /> {t("Neu")}
      </button>
      {#if menu?.mode === "new"}
        {@render kindMenu()}
      {/if}
    </div>
  </header>

  {#if dev.params.masks.length}
    <ul class="layers">
      {#each dev.params.masks as m, i (m.id)}
        <li class:active={dev.activeMask === i} class:off={!m.enabled}>
          <button
            class="eye"
            aria-label={m.enabled ? t("Ebene ausblenden") : t("Ebene einblenden")}
            title={m.enabled ? t("Ausblenden") : t("Einblenden")}
            onclick={() => { m.enabled = !m.enabled; dev.schedule(); dev.commit(); }}
          >
            <span class="dot"></span>
          </button>
          {#if renaming === i}
            <!-- svelte-ignore a11y_autofocus -->
            <input
              type="text"
              value={m.name}
              autofocus
              onkeydown={(e) => { e.stopPropagation(); if (e.key === "Enter") e.currentTarget.blur(); }}
              onblur={(e) => { m.name = e.currentTarget.value.trim() || m.name; renaming = null; dev.commit(); }}
            />
          {:else}
            <button class="name" onclick={() => dev.selectMask(dev.activeMask === i ? null : i)} ondblclick={() => (renaming = i)} title={t("Doppelklick zum Umbenennen")}>
              {m.name}
            </button>
          {/if}
          <button class="icon-btn" aria-label={t("Maske löschen")} title={t("Maske löschen")} onclick={() => dev.removeMask(i)}>
            <Icon name="trash" size={13} />
          </button>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="empty">{t("Lokale Anpassungen nur für einen Bereich: Himmel abdunkeln, Motiv aufhellen, Ecken abwedeln.")}</p>
  {/if}

  {#if hint}
    <p class="hint" class:busy={!!dev.maskBusy}>{hint}</p>
  {/if}

  {#if dev.maskLayer}
    {@const layer = dev.maskLayer}
    <div class="detail">
      <ul class="shapes">
        {#each layer.shapes as s, i}
          <li>
            <span class="kind" class:sub={s.subtract}>{s.subtract ? "−" : "+"} {describeShape(s)}</span>
            {#if s.type === "color"}
              <span class="swatches">
                {#each s.samples as c}<span class="swatch" style:background={oklabToCss(c.lab)}></span>{/each}
              </span>
            {/if}
            {#if s.type !== "brush"}
              <button class="tog" class:on={s.invert} onclick={() => toggleShape(i, "invert")} title={t("Umkehren")}>{t("Umkehren")}</button>
            {/if}
            <button class="icon-btn" aria-label={t("Form entfernen")} onclick={() => removeShape(i)}><Icon name="close" size={11} /></button>
          </li>
        {:else}
          <li class="muted">{dev.tool ? t("Noch keine Form – siehe Hinweis oben.") : t("Noch keine Form – füge eine hinzu.")}</li>
        {/each}
      </ul>

      <div class="row">
        <div class="anchor">
          <button class="btn small" onclick={(e) => { e.stopPropagation(); menu = menu?.mode === "add" ? null : { mode: "add" }; }}>{t("Hinzufügen")}</button>
          {#if menu?.mode === "add"}{@render kindMenu()}{/if}
        </div>
        <div class="anchor">
          <button class="btn small" onclick={(e) => { e.stopPropagation(); menu = menu?.mode === "subtract" ? null : { mode: "subtract" }; }}>{t("Abziehen")}</button>
          {#if menu?.mode === "subtract"}{@render kindMenu()}{/if}
        </div>
        <button class="btn small" class:on={dev.showOverlay} onclick={() => dev.toggleOverlay()} title={t("Maske rot anzeigen (O)")}>{t("Maske zeigen")}</button>
      </div>

      {#if dev.tool === "brush"}
        <div class="brush">
          <div class="seg">
            <button class:on={!dev.brush.erase} onclick={() => (dev.brush.erase = false)}>{t("Malen")}</button>
            <button class:on={dev.brush.erase} onclick={() => (dev.brush.erase = true)}>{t("Radieren")}</button>
          </div>
          <Slider label={t("Größe")} value={Math.round(dev.brush.size * 1000) / 10} min={0.3} max={20} step={0.1} decimals={1} oninput={(v) => (dev.brush.size = v / 100)} oncommit={() => {}} neutral={3} />
          <Slider label={t("Weiche Kante")} value={Math.round(dev.brush.feather * 100)} min={0} max={100} oninput={(v) => (dev.brush.feather = v / 100)} oncommit={() => {}} neutral={60} />
        </div>
      {:else if layer.shapes.some((s) => s.type === "brush")}
        <button class="btn small wide" onclick={() => (dev.tool = "brush")}>{t("Weiter malen")}</button>
      {/if}

      {#each layer.shapes as s, i}
        {#if s.type === "color"}
          {@const n = layer.shapes.filter((x) => x.type === "color").length}
          <Slider
            label={n > 1 ? t("Farbbereich {n}", { n: layer.shapes.slice(0, i + 1).filter((x) => x.type === "color").length }) : t("Farbbereich")}
            value={s.range}
            min={0}
            max={100}
            neutral={30}
            oninput={(v) => setRange(i, v)}
            oncommit={() => dev.commit()}
          />
        {/if}
      {/each}
      {#if dev.tool !== "color" && layer.shapes.some((s) => s.type === "color")}
        <button class="btn small wide" onclick={() => (dev.tool = "color")}>{t("Weitere Farbtöne wählen")}</button>
      {/if}

      <div class="sliders">
        {#each sliders as s (s.key)}
          <Slider
            label={s.label}
            value={layer.adjust[s.key]}
            min={s.min}
            max={s.max}
            step={s.step ?? 1}
            decimals={s.decimals ?? 0}
            track={s.track}
            oninput={(v) => dev.setLocal(s.key, v)}
            oncommit={() => dev.commit()}
          />
        {/each}
      </div>
    </div>
  {/if}
</section>

{#snippet kindMenu()}
  <div class="menu" role="menu" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
    {#each kinds as k (k.kind)}
      {@const disabled = k.ai && !aiReady}
      <button role="menuitem" {disabled} onclick={() => pick(k.kind)} title={disabled ? t("Benötigt die KI-Masken (unten links unter „KI“)") : k.hint}>
        {#if k.ai || k.kind === "sky"}<Icon name="sparkle" size={12} />{:else}<span class="sp"></span>{/if}
        <span class="l">{k.label}</span>
        <span class="h">{disabled ? t("KI-Masken fehlen") : k.hint}</span>
      </button>
      {#if k.kind === "sky"}
        {@render groupItem("person", t("Person"), t("Ganz oder einzelne Teile"), PERSON_PARTS.map(([p, l]) => [`person:${p}`, l] as [NewMaskKind, string]))}
        {@render groupItem("animal", t("Tier"), t("Vögel, Säugetiere, Reptilien"), ANIMAL_KINDS.map(([a, l]) => [`animal:${a}`, l] as [NewMaskKind, string]))}
      {/if}
    {/each}
  </div>
{/snippet}

{#snippet groupItem(id: "person" | "animal", label: string, hint: string, items: [NewMaskKind, string][])}
  {@const disabled = !partsReady}
  <button
    role="menuitem"
    aria-haspopup="menu"
    aria-expanded={group === id}
    {disabled}
    onclick={() => (group = group === id ? null : id)}
    title={disabled ? t("Benötigt die KI-Modelle für Personen und Tiere (unten links unter „KI“ nachladen)") : hint}
  >
    <Icon name="sparkle" size={12} />
    <span class="l">{label} <span class="chev" class:open={group === id}>▸</span></span>
    <span class="h">{disabled ? t("KI-Modelle fehlen") : hint}</span>
  </button>
  {#if group === id}
    <div class="sub" role="group" aria-label={label}>
      {#each items as [kind, l] (kind)}
        <button role="menuitem" onclick={() => pick(kind)}><span class="sp"></span><span class="l">{l}</span><span></span></button>
      {/each}
    </div>
  {/if}
{/snippet}

<style>
  .swatches {
    display: inline-flex;
    gap: 2px;
    margin-left: 4px;
  }
  .swatch {
    width: 10px;
    height: 10px;
    border-radius: 2px;
    border: 1px solid rgb(255 255 255 / 0.25);
  }
  .masks {
    padding: 12px 16px;
    border-bottom: 1px solid var(--line);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  h4 {
    margin: 0;
    font-size: var(--step--1);
    font-weight: 600;
    color: var(--muted);
  }
  .btn.small {
    padding: 3px 8px;
    font-size: var(--step--1);
  }
  .btn.on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .btn.wide {
    width: 100%;
    justify-content: center;
    margin-top: 8px;
  }
  .anchor {
    position: relative;
  }
  .menu {
    position: absolute;
    right: 0;
    top: calc(100% + 4px);
    z-index: 30;
    width: 260px;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    box-shadow: 0 12px 32px rgb(0 0 0 / 0.45);
    padding: 4px;
  }
  .row .anchor:first-child .menu {
    left: 0;
    right: auto;
  }
  .menu button {
    display: grid;
    grid-template-columns: 14px auto 1fr;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 8px;
    border-radius: var(--radius-s);
    text-align: left;
  }
  .menu button:hover:not(:disabled) {
    background: var(--line);
  }
  .menu button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .menu :global(svg) {
    color: var(--accent);
  }
  .menu .sub {
    margin: 0 0 4px 22px;
    padding-left: 6px;
    border-left: 1px solid var(--line);
  }
  .menu .sub button {
    padding: 4px 8px;
  }
  .menu .chev {
    display: inline-block;
    margin-left: 4px;
    color: var(--faint);
    transition: transform 0.12s;
  }
  .menu .chev.open {
    transform: rotate(90deg);
  }
  .menu .h {
    color: var(--faint);
    font-size: 11px;
    text-align: right;
  }
  .layers {
    list-style: none;
    padding: 0;
    margin: 10px 0 0;
    display: grid;
    gap: 2px;
  }
  .layers li {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 2px 2px 4px;
    border-radius: var(--radius-s);
  }
  .layers li.active {
    background: var(--raised);
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .layers li.off .name {
    color: var(--faint);
  }
  .eye {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    border: 1.5px solid var(--muted);
  }
  li:not(.off) .dot {
    background: var(--accent);
    border-color: var(--accent);
  }
  .name {
    flex: 1;
    text-align: left;
    padding: 3px 2px;
  }
  .layers input {
    flex: 1;
    padding: 2px 6px;
  }
  .empty,
  .hint {
    margin: 8px 0 0;
    color: var(--faint);
    font-size: var(--step--1);
  }
  .hint {
    color: var(--muted);
  }
  .hint.busy {
    color: var(--accent);
  }
  .detail {
    margin-top: 10px;
    display: grid;
    gap: 10px;
  }
  .shapes {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 2px;
    font-size: var(--step--1);
  }
  .shapes li {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .kind {
    flex: 1;
  }
  .kind.sub {
    color: #e0a07f;
  }
  .tog {
    font-size: 11px;
    padding: 1px 6px;
    border-radius: 4px;
    border: 1px solid var(--line);
    color: var(--faint);
  }
  .tog.on {
    color: var(--accent);
    border-color: var(--accent);
  }
  .row {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .brush {
    display: grid;
    gap: 8px;
  }
  .seg {
    display: flex;
    padding: 2px;
    gap: 2px;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
  }
  .seg button {
    flex: 1;
    padding: 3px 8px;
    border-radius: 6px;
    color: var(--muted);
  }
  .seg button.on {
    background: var(--raised);
    color: var(--text);
  }
  .sliders {
    display: grid;
    gap: 8px;
  }
  .sp {
    width: 12px;
  }
</style>
