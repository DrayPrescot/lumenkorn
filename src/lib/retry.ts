// Vorschaubilder erneut anfordern, wenn die Antwort ein Fehler war. Das
// Backend verwirft beim schnellen Scrollen die ältesten wartenden Anfragen
// (503, siehe src-tauri/src/protocol.rs); bleibt die Kachel sichtbar, soll sie
// trotzdem ihr Bild bekommen.

/** Höchstzahl der Wiederholungen je Bild-URL */
const TRIES = 4;

/** Svelte-Action für `<img>`: lädt nach einem Fehler mit kurzer Pause erneut. */
export function retryImage(img: HTMLImageElement) {
  let tries = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let base = img.getAttribute("src") ?? "";

  function onerror() {
    if (tries >= TRIES || !img.isConnected) return;
    tries++;
    clearTimeout(timer);
    timer = setTimeout(() => {
      if (!img.isConnected) return;
      const sep = base.includes("?") ? "&" : "?";
      img.src = `${base}${sep}retry=${tries}`;
    }, 300 * tries);
  }
  // Neue Bild-URL (anderes Foto, neue Version): wieder von vorn zählen
  const observer = new MutationObserver(() => {
    const src = img.getAttribute("src") ?? "";
    if (!src.includes("retry=")) {
      base = src;
      tries = 0;
      clearTimeout(timer);
    }
  });
  observer.observe(img, { attributes: true, attributeFilter: ["src"] });
  img.addEventListener("error", onerror);
  return {
    destroy() {
      clearTimeout(timer);
      observer.disconnect();
      img.removeEventListener("error", onerror);
    },
  };
}
