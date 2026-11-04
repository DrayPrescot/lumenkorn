// Wann Lumenkorn beim Start fragt, ob die KI auf der Grafikkarte rechnen soll.
// Als eigene Funktion, damit sie sich mit dem Status jeder Plattform testen lässt.
import type { AiStatus, GpuDevice } from "./api";

/** `null`: nicht fragen, sonst was fehlt */
export function gpuPromptNeed(s: AiStatus): "download" | "enable" | null {
  const st = s.settings;
  if (!st.enabled || st.gpuPromptDeclined) return null;
  // Keine GPU-Laufzeit für diese Plattform oder keine Grafikkarte erkannt
  if (!s.gpuRuntimes.length || !gpuPresent(s)) return null;
  if (!s.gpuRuntimes.some((g) => g.installed)) return "download";
  return st.accel === "cpu" ? "enable" : null;
}

/** Eine Grafikkarte laut Betriebssystem oder laut Grafiktreiber der Bildbearbeitung
 *  (nicht jede Plattform liefert eine Geräteliste). */
function gpuPresent(s: AiStatus): boolean {
  const info = s.accel.develop.info;
  return s.accel.devices.length > 0 || (!!info && info.kind !== "software");
}

/** Anzeigename; unter Windows ist der Hersteller nicht immer bekannt */
export const gpuLabel = (d: GpuDevice) => `${d.vendor} ${d.name}`.trim();
