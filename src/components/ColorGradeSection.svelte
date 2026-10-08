<script lang="ts">
  // Color Grading im Reiter „Basis“ (unter HSL): Tiefen, Mitteltöne, Lichter
  // und Global als Farbräder; die Regler darunter gelten dem gewählten Rad.
  import { DEFAULT_PARAMS, type GradeRange } from "../lib/api";
  import { dev } from "../lib/develop.svelte";
  import ColorWheel from "./ColorWheel.svelte";
  import Slider from "./Slider.svelte";
  import { t } from "../lib/i18n";

  const ranges: { id: GradeRange; label: string }[] = [
    { id: "shadows", label: t("Tiefen") },
    { id: "midtones", label: t("Mitteltöne") },
    { id: "highlights", label: t("Lichter") },
    { id: "global", label: t("Global") },
  ];

  const g = $derived(dev.params.colorGrade ?? DEFAULT_PARAMS.colorGrade);
  const sel = $derived(g[dev.gradeRange]);
  const selLabel = $derived(ranges.find((r) => r.id === dev.gradeRange)?.label ?? "");
  const changed = $derived(JSON.stringify(g) !== JSON.stringify(DEFAULT_PARAMS.colorGrade));

  const HUE_TRACK = "linear-gradient(to right, #f00, #ff0, #0f0, #0ff, #00f, #f0f, #f00)";

  function reset() {
    dev.params.colorGrade = JSON.parse(JSON.stringify(DEFAULT_PARAMS.colorGrade));
    dev.schedule();
    dev.commit();
  }
</script>

<details open class="grade">
  <summary tabindex="-1">{t("Color Grading")}</summary>
  <div class="wheels">
    {#each ranges as r (r.id)}
      <ColorWheel
        label={r.label}
        hue={g[r.id].hue}
        saturation={g[r.id].saturation}
        selected={dev.gradeRange === r.id}
        onselect={() => (dev.gradeRange = r.id)}
        oninput={(h, s) => {
          dev.setGrade(r.id, "hue", h);
          dev.setGrade(r.id, "saturation", s);
        }}
        oncommit={() => dev.commit()}
      />
    {/each}
  </div>

  <div class="sel">{selLabel}</div>
  <div class="sliders">
    <Slider
      label={t("Farbton")}
      value={sel.hue}
      min={0}
      max={360}
      unit="°"
      track={HUE_TRACK}
      oninput={(v) => dev.setGrade(dev.gradeRange, "hue", v)}
      oncommit={() => dev.commit()}
    />
    <Slider
      label={t("Sättigung")}
      value={sel.saturation}
      min={0}
      max={100}
      track="linear-gradient(to right, #8a8a8a, hsl({sel.hue} 90% 50%))"
      oninput={(v) => dev.setGrade(dev.gradeRange, "saturation", v)}
      oncommit={() => dev.commit()}
    />
    <Slider
      label={t("Luminanz")}
      value={sel.luminance}
      min={-100}
      max={100}
      track="linear-gradient(to right, #1d1d1d, #8a8a8a, #f2f2f2)"
      oninput={(v) => dev.setGrade(dev.gradeRange, "luminance", v)}
      oncommit={() => dev.commit()}
    />
  </div>
  <div class="sliders more">
    <Slider
      label={t("Überblenden")}
      value={g.blending}
      min={0}
      max={100}
      neutral={50}
      oninput={(v) => dev.setGrade(null, "blending", v)}
      oncommit={() => dev.commit()}
    />
    <Slider
      label={t("Abgleich")}
      value={g.balance}
      min={-100}
      max={100}
      oninput={(v) => dev.setGrade(null, "balance", v)}
      oncommit={() => dev.commit()}
    />
  </div>
  {#if changed}
    <button class="btn small reset" tabindex="-1" onclick={reset}>{t("Color Grading zurücksetzen")}</button>
  {/if}
</details>

<style>
  /* wie die übrigen Abschnitte in DevelopPanel */
  .grade {
    padding: 12px 16px;
    border-bottom: 1px solid var(--line);
  }
  .wheels {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px 8px;
    margin-top: 10px;
  }
  .sel {
    margin-top: 12px;
    font-size: var(--step--1);
    font-weight: 600;
  }
  .sliders {
    display: grid;
    gap: 8px;
    margin-top: 6px;
  }
  .sliders.more {
    margin-top: 12px;
    padding-top: 10px;
    border-top: 1px dashed var(--line);
  }
  .reset {
    margin-top: 10px;
  }
</style>
