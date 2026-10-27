<script lang="ts">
  import { SLIDER_KEYS } from "../lib/keys";
  import { t, locale } from "../lib/i18n";
  // Regler wie in Fotoeditoren: Doppelklick setzt zurück, Werte lassen sich
  // auch direkt eintippen, der Verlauf erhält erst beim Loslassen einen Schritt.
  let {
    label,
    value,
    min,
    max,
    step = 1,
    neutral = 0,
    decimals = 0,
    unit = "",
    track,
    scale,
    oninput,
    oncommit,
  }: {
    label: string;
    value: number;
    min: number;
    max: number;
    step?: number;
    neutral?: number;
    decimals?: number;
    unit?: string;
    /** CSS-Hintergrund der Schiene, z. B. ein Verlauf für den Weißabgleich */
    track?: string;
    /** Nichtlineare Skala: Wert → Position 0…1 und zurück (z. B. Kelvin in Mired) */
    scale?: { to: (v: number) => number; from: (p: number) => number };
    oninput: (v: number) => void;
    oncommit: () => void;
  } = $props();

  /** Text im Eingabefeld, solange es den Fokus hat (sonst der formatierte Wert) */
  let draft = $state<string | null>(null);
  const id = `s-${Math.random().toString(36).slice(2, 8)}`;

  const fmt = (v: number) => {
    // Ohne Tausenderpunkt: „5.350“ läse sich beim Eintippen als 5,35
    const s = v.toLocaleString(locale, { minimumFractionDigits: decimals, maximumFractionDigits: decimals, useGrouping: false });
    return v > 0 && neutral === 0 && min < 0 ? `+${s}` : s;
  };
  // Position des Neutralwerts, damit die Füllung von dort ausgeht
  const pct = (v: number) => (scale ? scale.to(v) : (v - min) / (max - min)) * 100;
  // Mit Skala arbeitet die Schiene in Positionen 0…STEPS
  const STEPS = 1000;
  const toPos = (v: number) => (scale ? Math.round(scale.to(v) * STEPS) : v);
  const fromPos = (p: number) => (scale ? scale.from(p / STEPS) : p);
  const fill = $derived.by(() => {
    const a = pct(Math.min(value, neutral));
    const b = pct(Math.max(value, neutral));
    return `linear-gradient(to right, transparent ${a}%, var(--fill) ${a}%, var(--fill) ${b}%, transparent ${b}%)`;
  });

  function reset() {
    oninput(neutral);
    oncommit();
  }

  const clamp = (v: number) => Math.min(max, Math.max(min, v));

  // Mit Tab erreichbar und sofort beschreibbar: Beim Fokus wird der Wert
  // markiert, Tippen ersetzt ihn, Enter/Tab übernimmt, Esc verwirft.
  let justFocused = false;
  function focus(e: FocusEvent) {
    draft = fmt(value);
    const el = e.currentTarget as HTMLInputElement;
    el.value = draft;
    el.select();
    justFocused = true;
  }

  function typed() {
    if (draft === null) return;
    const raw = draft.replace(",", ".").replace("+", "").trim();
    draft = null;
    const v = Number(raw);
    if (raw !== "" && Number.isFinite(v) && clamp(v) !== value) {
      oninput(clamp(v));
      oncommit();
    }
  }

  function keydown(e: KeyboardEvent) {
    const el = e.currentTarget as HTMLInputElement;
    if (e.key === "Enter") {
      // Wert übernehmen und Fokus abgeben; Enter geht weiter an die Ansicht
      // (beim Zuschneiden: übernehmen, sonst blättern die Pfeile wieder)
      typed();
      el.blur();
      return;
    }
    // Tastenkürzel der Ansicht nicht auslösen, während hier getippt wird
    e.stopPropagation();
    if (e.key === "Escape") {
      draft = null;
      el.blur();
    } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
      // Pfeiltasten verstellen in Schritten (mit Umschalt zehnfach)
      e.preventDefault();
      const d = (e.key === "ArrowUp" ? 1 : -1) * step * (e.shiftKey ? 10 : 1);
      oninput(clamp(value + d));
      oncommit();
      draft = fmt(clamp(value + d));
      el.value = draft;
      el.select();
    }
  }
</script>

<div class="slider" class:changed={value !== neutral}>
  <label for={id} ondblclick={reset} title={t("Doppelklick setzt zurück")}>{label}</label>
  <input
    class="value"
    type="text"
    inputmode="decimal"
    aria-label={t("{label} (Wert eingeben)", { label })}
    title={t("Wert eingeben – Tab springt zum nächsten Regler, ↑/↓ verstellen")}
    value={draft ?? `${fmt(value)}${unit}`}
    onfocus={focus}
    onmouseup={(e) => {
      // Der Klick, der fokussiert hat, soll die Markierung nicht aufheben
      if (justFocused) e.preventDefault();
      justFocused = false;
    }}
    oninput={(e) => {
      draft = e.currentTarget.value;
      justFocused = false;
    }}
    onblur={typed}
    onkeydown={keydown}
  />
  <input
    {id}
    type="range"
    min={scale ? 0 : min}
    max={scale ? STEPS : max}
    step={scale ? 1 : step}
    value={toPos(value)}
    style:background-image="{fill}, {track ?? 'linear-gradient(var(--line), var(--line))'}"
    oninput={(e) => oninput(clamp(fromPos(Number(e.currentTarget.value))))}
    onchange={oncommit}
    ondblclick={reset}
    tabindex="-1"
    onkeydown={(e) => SLIDER_KEYS.has(e.key) && e.stopPropagation()}
    onkeyup={(e) => SLIDER_KEYS.has(e.key) && oncommit()}
  />
</div>

<style>
  .slider {
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: center;
    row-gap: 2px;
    --fill: rgb(227 169 58 / 0.55);
  }
  label {
    color: var(--muted);
    cursor: default;
  }
  .changed label {
    color: var(--text);
  }
  .value {
    font: inherit;
    font-size: var(--step--1);
    color: var(--muted);
    text-align: right;
    width: 60px;
    padding: 1px 4px;
    border-radius: var(--radius-s);
    border: 1px solid transparent;
    background: transparent;
    outline: none;
  }
  .value:hover {
    background: var(--raised);
    color: var(--text);
  }
  .value:focus {
    background: var(--raised);
    border-color: var(--accent);
    color: var(--text);
  }
  .changed .value {
    color: var(--text);
  }
  input[type="range"] {
    grid-column: 1 / -1;
    width: 100%;
    height: 18px;
    margin: 0;
    appearance: none;
    background-color: transparent;
    background-repeat: no-repeat;
    background-size: 100% 3px;
    background-position: center;
    cursor: pointer;
  }
  input[type="range"]::-webkit-slider-runnable-track {
    height: 3px;
    border-radius: 2px;
    background: transparent;
  }
  input[type="range"]::-webkit-slider-thumb {
    appearance: none;
    width: 12px;
    height: 12px;
    margin-top: -4.5px;
    border-radius: 50%;
    background: var(--text);
    border: 2px solid var(--panel);
    box-shadow: 0 0 0 1px var(--faint);
  }
  input[type="range"]::-moz-range-track {
    height: 3px;
    background: transparent;
  }
  input[type="range"]::-moz-range-thumb {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--text);
    border: 2px solid var(--panel);
  }
  input[type="range"]:focus-visible {
    outline: none;
  }
  input[type="range"]:focus-visible::-webkit-slider-thumb {
    box-shadow: 0 0 0 2px var(--accent);
  }
</style>
