// Aussortier-Assistent: aus den Befunden des Backends (Schweregrad je Grund)
// und den gelernten Schwellen ableiten, was vorgeschlagen wird. Die Strenge
// verschiebt alle Schwellen gleich; das wirkt sofort, ohne neue Analyse.
import type { CullDecision, CullItem, CullReason } from "./api";
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

/** Wird abgelehnt: was der Nutzer angeklickt hat, sonst der Vorschlag */
export function isRejected(id: number, verdicts: Map<number, Verdict>, manual: Record<number, boolean>): boolean {
  return manual[id] ?? verdicts.get(id)?.state === "suggest";
}

/**
 * Was „Ablehnen“ ausführt. `reject`: alle angezeigten Fotos, die abgelehnt werden,
 * auch die, die der Assistent behalten wollte. `learn`: nur Fotos mit Vorschlag
 * oder Grenzfall. Ein behaltenes Foto hat keinen Befund nahe an seiner Schwelle;
 * als Entscheidung im Assistenten (volles Gewicht) zöge es die Schwelle weit nach
 * unten. Seine Ablehnung wertet das Backend wie jede von Hand aus (`setFlag` →
 * `note_decisions`), und die lehrt nur einen Grenzfall.
 */
export function cullPlan(
  items: CullItem[],
  verdicts: Map<number, Verdict>,
  manual: Record<number, boolean>,
): { reject: number[]; learn: CullDecision[] } {
  const reject: number[] = [];
  const learn: CullDecision[] = [];
  for (const it of items) {
    const no = isRejected(it.id, verdicts, manual);
    if (no) reject.push(it.id);
    const v = verdicts.get(it.id);
    if (v) learn.push({ id: it.id, reason: v.reason, severity: v.severity, reject: no });
  }
  return { reject, learn };
}
