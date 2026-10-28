// Aussortier-Assistent: aus den Befunden des Backends (Schweregrad je Grund)
// und den gelernten Schwellen ableiten, was vorgeschlagen wird. Die Strenge
// verschiebt alle Schwellen gleich; das wirkt sofort, ohne neue Analyse.
import type { CullItem, CullReason } from "./api";
import { t } from "./i18n";

export type Strictness = "cautious" | "normal" | "strict";

/** Verschiebung der Schwellen je Strenge */
export const OFFSET: Record<Strictness, number> = { cautious: 0.15, normal: 0, strict: -0.15 };
/** So weit unter der Schwelle gilt ein Befund als Grenzfall */
export const BORDER = 0.15;

export const REASONS: CullReason[] = ["series", "blur", "eyes", "exposure", "accident"];

export function reasonLabel(r: CullReason): string {
  switch (r) {
    case "series":
      return t("Serie: bessere Fotos vorhanden");
    case "blur":
      return t("Unscharf");
    case "eyes":
      return t("Augen geschlossen");
    case "exposure":
      return t("Stark falsch belichtet");
    case "accident":
      return t("Versehentlich ausgelöst");
  }
}

export interface Verdict {
  /** Ausschlaggebender Grund (am weitesten über bzw. am nächsten an seiner Schwelle) */
  reason: CullReason;
  severity: number;
  state: "suggest" | "border";
}

/** Vorschlag, Grenzfall oder nichts (`null`) für ein Foto */
export function judge(item: CullItem, thresholds: Record<CullReason, number>, strictness: Strictness): Verdict | null {
  if (item.protected) return null;
  let best: { reason: CullReason; severity: number; margin: number } | null = null;
  for (const f of item.findings) {
    const margin = f.severity - ((thresholds[f.reason] ?? 0.5) + OFFSET[strictness]);
    if (!best || margin > best.margin) best = { reason: f.reason, severity: f.severity, margin };
  }
  if (!best || best.margin < -BORDER) return null;
  return { reason: best.reason, severity: best.severity, state: best.margin >= 0 ? "suggest" : "border" };
}
