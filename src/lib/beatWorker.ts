// Takterkennung im Hintergrund (ein Stück von 4 Minuten braucht etwa eine
// halbe Sekunde – die Oberfläche soll dabei nicht stehen).
import { detectBeats } from "./beats";

self.onmessage = (e: MessageEvent<{ samples: Float32Array; rate: number }>) => {
  self.postMessage(detectBeats(e.data.samples, e.data.rate));
};
