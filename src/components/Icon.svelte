<script lang="ts" module>
  // Schlanke Strich-Icons (24er Raster), damit keine Icon-Bibliothek nötig ist.
  const paths = {
    folder: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
    plus: "M12 5v14M5 12h14",
    import: "M12 4v11M7 10l5 5 5-5M4 19h16",
    chevron: "M9 6l6 6-6 6",
    star: "M12 3.5l2.6 5.3 5.9.9-4.3 4.1 1 5.8L12 16.9l-5.2 2.7 1-5.8-4.3-4.1 5.9-.9z",
    search: "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14zM20 20l-4-4",
    close: "M6 6l12 12M18 6L6 18",
    refresh: "M20 11a8 8 0 0 0-14.6-4.5M4 4v4h4M4 13a8 8 0 0 0 14.6 4.5M20 20v-4h-4",
    stack: "M12 3l9 5-9 5-9-5zM3 13l9 5 9-5",
    photos: "M4 5h16v14H4zM4 15l4-4 4 4 3-3 5 5",
    trash: "M5 7h14M10 7V4h4v3M7 7l1 13h8l1-13",
    flag: "M6 21V4M6 4h11l-2 4 2 4H6",
    reject: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM5.6 5.6l12.8 12.8",
    menu: "M4 7h16M4 12h16M4 17h16",
    info: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 11v5M12 8h.01",
    back: "M15 6l-6 6 6 6",
    sliders: "M4 6h10M18 6h2M4 12h4M12 12h8M4 18h12M20 18h0M14 4v4M8 10v4M16 16v4",
    develop: "M12 3v3M12 18v3M3 12h3M18 12h3M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z",
    export: "M12 3v12M7 8l5-5 5 5M5 15v5h14v-5",
    pipette: "M14.5 6.5l3 3M5 19l1-3 8.5-8.5 2 2L8 18l-3 1zM14 6l1.5-1.5a2.1 2.1 0 0 1 3 3L17 9",
    sparkle: "M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z",
    mono: "M12 3a9 9 0 1 0 0 18 9 9 0 1 0 0-18zM12 3v18M12 7h4.5M12 11h5.5M12 15h5",
  };
  export type IconName = keyof typeof paths;
</script>

<script lang="ts">
  let { name, size = 16, fill = false }: { name: IconName; size?: number; fill?: boolean } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill={fill ? "currentColor" : "none"}
  stroke="currentColor"
  stroke-width="1.7"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  <path d={paths[name]} />
</svg>
