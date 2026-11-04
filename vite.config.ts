import { readFileSync } from "node:fs";
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Tauri erwartet einen festen Port und keine Bildschirm-Löschung.
const host = process.env.TAURI_DEV_HOST;

// Dieselbe CSP wie im fertigen Programm: Was sie blockiert, soll schon in der
// Browser-Vorschau und in `tauri dev` auffallen, nicht erst nach dem Release.
const csp = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8")).app.security.csp as string;

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    headers: host ? undefined : { "Content-Security-Policy": csp },
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**", "**/target/**"] },
  },
  build: {
    target: "es2022",
    outDir: "dist",
    emptyOutDir: true,
  },
});
