import { describe, expect, it } from "vitest";
import type { AiStatus } from "./api";
import { gpuLabel, gpuPromptNeed } from "./gpuPrompt";

const AMD = { vendor: "AMD", name: "Radeon RX 7900 XTX", memory: null, discrete: null };
const WEBGPU = { backend: "webgpu", label: "WebGPU", version: "1", installed: false, downloadSize: 25_000_000 };

/** Status, wie das Backend ihn liefert. Grundfall: Windows mit erkannter Karte. */
function status(over: { settings?: object; devices?: unknown[]; runtimes?: unknown[]; info?: unknown } = {}): AiStatus {
  return {
    settings: { enabled: true, autoTag: true, accel: "auto", developGpu: true, faces: true, learnStyle: true, gpuPromptDeclined: false, ...over.settings },
    gpuRuntimes: over.runtimes ?? [WEBGPU],
    accel: {
      devices: over.devices ?? [AMD],
      develop: { enabled: true, available: !!over.info, info: over.info ?? null, error: null },
    },
  } as unknown as AiStatus;
}

describe("Frage nach der GPU-Unterstützung", () => {
  it("kommt, wenn die Laufzeit fehlt", () => {
    expect(gpuPromptNeed(status())).toBe("download");
  });

  it("kommt, wenn die Laufzeit da ist, die KI aber auf dem Prozessor rechnet", () => {
    const runtimes = [{ ...WEBGPU, installed: true }];
    expect(gpuPromptNeed(status({ runtimes, settings: { accel: "cpu" } }))).toBe("enable");
    expect(gpuPromptNeed(status({ runtimes }))).toBeNull();
  });

  it("kommt nicht, wenn die KI aus ist oder „Nein“ gemerkt wurde", () => {
    expect(gpuPromptNeed(status({ settings: { enabled: false } }))).toBeNull();
    expect(gpuPromptNeed(status({ settings: { gpuPromptDeclined: true } }))).toBeNull();
  });

  it("kommt nicht ohne Laufzeit für die Plattform", () => {
    expect(gpuPromptNeed(status({ runtimes: [] }))).toBeNull();
  });

  // Nicht jede Plattform liefert eine Geräteliste: Das war unter Windows lange so,
  // und die Frage erschien dort nie.
  it("erkennt die Karte auch ohne Geräteliste über den Grafiktreiber", () => {
    const info = { name: "Apple M3", backend: "Metal", kind: "integrated", driver: "" };
    expect(gpuPromptNeed(status({ devices: [], info }))).toBe("download");
  });

  it("zählt einen Software-Adapter nicht als Grafikkarte", () => {
    const info = { name: "llvmpipe", backend: "Vulkan", kind: "software", driver: "" };
    expect(gpuPromptNeed(status({ devices: [], info }))).toBeNull();
    expect(gpuPromptNeed(status({ devices: [] }))).toBeNull();
  });
});

describe("Name der Grafikkarte", () => {
  it("zeigt den Hersteller, wenn er bekannt ist", () => {
    expect(gpuLabel(AMD)).toBe("AMD Radeon RX 7900 XTX");
    expect(gpuLabel({ ...AMD, vendor: "", name: "VMware SVGA 3D" })).toBe("VMware SVGA 3D");
  });
});
