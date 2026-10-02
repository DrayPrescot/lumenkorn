<script lang="ts">
  import { SLIDER_KEYS } from "../lib/keys";
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
    oninput: (v: number) => void;
    oncommit: () => void;
  } = $props();

  let editing = $state(false);
  const id = `s-${Math.random().toString(36).slice(2, 8)}`;

  const fmt = (v: number) => {
    const s = v.toLocaleString("de-DE", { minimumFractionDigits: decimals, maximumFractionDigits: decimals });
    return v > 0 && neutral === 0 && min < 0 ? `+${s}` : s;
  };
  // Position des Neutralwerts, damit die Füllung von dort ausgeht
  const pct = (v: number) => ((v - min) / (max - min)) * 100;
  const fill = $derived.by(() => {
    const a = pct(Math.min(value, neutral));
    const b = pct(Math.max(value, neutral));
    return `linear-gradient(to right, transparent ${a}%, var(--fill) ${a}%, var(--fill) ${b}%, transparent ${b}%)`;
  });

  function reset() {
    oninput(neutral);
    oncommit();
  }

  function typed(e: Event) {
    const raw = (e.currentTarget as HTMLInputElement).value.replace(",", ".").replace("+", "");
    const v = Number(raw);
    editing = false;
    if (Number.isFinite(v)) {
      oninput(Math.min(max, Math.max(min, v)));
      oncommit();
    }
  }
</script>

<div class="slider" class:changed={value !== neutral}>
  <label for={id} ondblclick={reset} title="Doppelklick setzt zurück">{label}</label>
  {#if editing}
    <!-- svelte-ignore a11y_autofocus -->
    <input
      class="num"
      type="text"
      value={fmt(value)}
      autofocus
      onblur={typed}
      onkeydown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") (e.currentTarget as HTMLInputElement).blur();
        if (e.key === "Escape") editing = false;
      }}
    />
  {:else}
    <button class="value" onclick={() => (editing = true)} title="Wert eingeben">{fmt(value)}{unit}</button>
  {/if}
  <input
    {id}
    type="range"
    {min}
    {max}
    {step}
    {value}
    style:background-image="{fill}, {track ?? 'linear-gradient(var(--line), var(--line))'}"
    oninput={(e) => oninput(Number(e.currentTarget.value))}
    onchange={oncommit}
    ondblclick={reset}
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
  .value,
  .num {
    font-size: var(--step--1);
    color: var(--muted);
    text-align: right;
    min-width: 46px;
    padding: 1px 4px;
    border-radius: var(--radius-s);
  }
  .value:hover {
    background: var(--raised);
    color: var(--text);
  }
  .changed .value {
    color: var(--text);
  }
  .num {
    width: 56px;
    background: var(--raised);
    border: 1px solid var(--faint);
    outline: none;
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
