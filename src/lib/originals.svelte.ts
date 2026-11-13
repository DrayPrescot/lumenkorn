// Bearbeitung ins Original übernehmen: Übernahme starten und verfolgen, gesicherte
// Originale wiederherstellen oder löschen. Die Bibliothek (`lib`) zählt beim Laden mit
// (`originals.count`) und reicht `bake-progress` hierher weiter.

import { api, type BakeProgress, type DeleteOriginalsReport, type ImageSummary, type OriginalsReport } from "./api";
import { num, t } from "./i18n";
import { lib } from "./store.svelte";

/** Hat ein gesichertes Original (und die Datei ist da) */
export const hasOriginal = (i: ImageSummary) => !!i.hasOriginal && !i.missing;

class Originals {
  /** Fotos des aktuellen Bereichs mit gesichertem Original (Knopf „Originale“) */
  count = $state(0);
  progress = $state<BakeProgress | null>(null);
  #timer: ReturnType<typeof setTimeout> | undefined;

  /** Fotos mit gesichertem Original im aktuellen Bereich, bei Bedarf gebaut. */
  list(): ImageSummary[] {
    return lib.images.filter(hasOriginal);
  }

  /** Übernahme starten (läuft im Hintergrund, Fortschritt per `bake-progress`). */
  async bake(ids: number[]): Promise<boolean> {
    try {
      await api.bakeOriginals(ids);
      return true;
    } catch (e) {
      lib.notify(String(e), "error");
      return false;
    }
  }

  onProgress(p: BakeProgress) {
    this.progress = p;
    clearTimeout(this.#timer);
    if (!p.finished) return;
    const n = p.baked.length;
    if (p.failed.length) {
      lib.notify(t("{ok} von {total} Originalen ersetzt. Fehlgeschlagen: {failed}", { ok: n, total: p.total, failed: p.failed.join("; ") }), "error");
    } else {
      lib.notify(t(n === 1 ? "{n} Original ersetzt, das bisherige liegt im Ordner „Original“." : "{n} Originale ersetzt, die bisherigen liegen im Ordner „Original“.", { n }));
    }
    this.#timer = setTimeout(() => (this.progress = null), 1500);
  }

  /** Gesicherte Originale zurücklegen, Bearbeitung von davor wiederherstellen. */
  async restore(ids: number[]) {
    let report: OriginalsReport;
    try {
      report = await api.restoreOriginals(ids);
    } catch (e) {
      lib.notify(String(e), "error");
      return;
    }
    const n = report.done.length;
    if (report.failed.length) {
      lib.notify(t("{ok} Originale wiederhergestellt, {failed} nicht: {error}", { ok: n, failed: report.failed.length, error: report.failed[0] }), "error");
    } else if (n) {
      lib.notify(t(n === 1 ? "{n} Original wiederhergestellt" : "{n} Originale wiederhergestellt", { n }));
    }
    await lib.loadImages(true);
  }

  /** Gesicherte Originale löschen (Papierkorb oder endgültig). */
  async delete(ids: number[], toTrash: boolean): Promise<DeleteOriginalsReport | null> {
    let report: DeleteOriginalsReport;
    try {
      report = await api.deleteOriginals(ids, toTrash);
    } catch (e) {
      lib.notify(String(e), "error");
      return null;
    }
    const mb = num(report.bytes / 1e6, { maximumFractionDigits: report.bytes < 1e7 ? 2 : 0 });
    const k = report.deleted.length;
    const originals = t(k === 1 ? "{n} Original" : "{n} Originale", { n: k });
    if (report.failed.length) {
      const vars = { originals, failed: report.failed.length, error: report.failed[0] };
      lib.notify(
        report.toTrash
          ? t("{originals} in den Papierkorb verschoben, {failed} nicht: {error}", vars)
          : t("{originals} endgültig gelöscht, {failed} nicht: {error}", vars),
        "error",
      );
    } else {
      lib.notify(
        report.toTrash
          ? t("{originals} in den Papierkorb verschoben ({mb} MB)", { originals, mb })
          : t("{originals} endgültig gelöscht ({mb} MB)", { originals, mb }),
      );
    }
    await lib.loadImages(true);
    return report;
  }
}

export const originals = new Originals();
