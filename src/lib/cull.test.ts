import { describe, expect, it } from "vitest";
import type { CullItem, CullReason } from "./api";
import { cullPlan, isRejected, judge, type Verdict } from "./cull";

const THRESHOLDS: Record<CullReason, number> = { series: 0.5, blur: 0.5, eyes: 0.5, exposure: 0.5, accident: 0.5 };

function item(id: number, over: Partial<CullItem> = {}): CullItem {
  return { id, capturedAt: null, series: 0, rank: 0, keep: true, protected: false, findings: [], ...over };
}

/** Eine Serie wie vom Backend: 1 und 2 behalten (2 geschützt), 3 Vorschlag, 4 Grenzfall */
const items = [
  item(1),
  item(2, { rank: 1, protected: true }),
  item(3, { rank: 2, keep: false, findings: [{ reason: "series", severity: 0.7 }] }),
  item(4, { rank: 3, findings: [{ reason: "blur", severity: 0.4 }] }),
];

function verdictsOf(list: CullItem[]): Map<number, Verdict> {
  const m = new Map<number, Verdict>();
  for (const it of list) {
    const v = judge(it, THRESHOLDS, "normal");
    if (v) m.set(it.id, v);
  }
  return m;
}

describe("Aussortieren: Entscheidungen im Dialog", () => {
  const verdicts = verdictsOf(items);

  it("übernimmt ohne Klick die Vorschläge", () => {
    expect(verdicts.get(3)?.state).toBe("suggest");
    expect(verdicts.get(4)?.state).toBe("border");
    expect(verdicts.has(1) || verdicts.has(2)).toBe(false);
    expect(items.map((it) => isRejected(it.id, verdicts, {}))).toEqual([false, false, true, false]);
    expect(cullPlan(items, verdicts, {})).toEqual({
      reject: [3],
      learn: [
        { id: 3, reason: "series", severity: 0.7, reject: true },
        { id: 4, reason: "blur", severity: 0.4, reject: false },
      ],
    });
  });

  it("lehnt auch Fotos ab, die der Assistent behalten wollte", () => {
    const plan = cullPlan(items, verdicts, { 1: true, 2: true });
    expect(plan.reject).toEqual([1, 2, 3]);
    // Behaltene lernt das Backend wie eine Ablehnung von Hand, nicht als Entscheidung im Assistenten
    expect(plan.learn.map((d) => d.id)).toEqual([3, 4]);
  });

  it("ein Klick nimmt Vorschlag und Grenzfall zurück bzw. lehnt sie ab", () => {
    const plan = cullPlan(items, verdicts, { 3: false, 4: true });
    expect(plan.reject).toEqual([4]);
    expect(plan.learn).toEqual([
      { id: 3, reason: "series", severity: 0.7, reject: false },
      { id: 4, reason: "blur", severity: 0.4, reject: true },
    ]);
  });

  it("ein behaltenes Foto, zweimal angeklickt, bleibt", () => {
    expect(cullPlan(items, verdicts, { 1: false }).reject).toEqual([3]);
  });
});
