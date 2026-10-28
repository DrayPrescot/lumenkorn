<script lang="ts">
  // Schrift für das Fotoalbum: die mitgelieferten Schriften und alle
  // Schriftfamilien des Systems, jeweils in der eigenen Schrift angezeigt.
  import { onMount } from "svelte";
  import { BUILTIN_FONTS, fontFamily, type AlbumFont } from "../lib/album";
  import { systemFonts } from "../lib/fonts";
  import { t } from "../lib/i18n";

  let { value, onchange }: { value: AlbumFont; onchange: (font: AlbumFont) => void } = $props();

  let system = $state<string[] | null>(null);
  onMount(() => {
    systemFonts().then((list) => (system = list));
  });

  const builtin = Object.entries(BUILTIN_FONTS);
  /** Gewählte Systemschrift, die hier nicht (mehr) installiert ist */
  const missing = $derived(!!system && !Object.hasOwn(BUILTIN_FONTS, value) && !system.includes(value));
</script>

<select {value} onchange={(e) => onchange(e.currentTarget.value)} style:font-family="{fontFamily(value)}, sans-serif">
  <optgroup label={t("Mitgeliefert")}>
    {#each builtin as [key, f] (key)}
      <option value={key} style:font-family={f.family}>{f.label}</option>
    {/each}
  </optgroup>
  {#if missing}
    <option value={value}>{t("{name} (nicht installiert)", { name: value })}</option>
  {/if}
  {#if system === null}
    {#if !Object.hasOwn(BUILTIN_FONTS, value)}<option value={value}>{value}</option>{/if}
    <option disabled>{t("Schriften des Systems werden gesucht …")}</option>
  {:else if system.length}
    <optgroup label={t("Schriften des Systems")}>
      {#each system as name (name)}
        <option value={name} style:font-family={fontFamily(name)}>{name}</option>
      {/each}
    </optgroup>
  {/if}
</select>
