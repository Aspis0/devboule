// Polis P1.4 — pure model for the Augure anomaly ledger section.
//
// Given the full sin ledger and a specific building's filePath, partitions the
// records into open / ignored / fixed buckets. Pure (no DOM, no Tauri) so it
// is fully unit-testable.

import type { SinRecord } from "../../types/city";

/** Severity sort weight: higher = more urgent. Unknown severities sort last. */
const SEVERITY_RANK: Record<string, number> = {
  inferno: 3,
  fire: 2,
  smoke: 1,
};

function severityRank(s: string): number {
  return SEVERITY_RANK[s] ?? 0;
}

/** Compare two records: severity descending, then ruleId ascending (stable tiebreak). */
function compareSeverityThenRule(a: SinRecord, b: SinRecord): number {
  const d = severityRank(b.severity) - severityRank(a.severity);
  return d !== 0 ? d : a.ruleId.localeCompare(b.ruleId);
}

export interface AnomalyLedgerModel {
  /** disposition === "open", sorted severity desc then ruleId. */
  open: SinRecord[];
  /** disposition === "ignored", sorted severity desc then ruleId. */
  ignored: SinRecord[];
  /** Count of disposition === "fixed" (shown as a subtle line, not listed). */
  fixedCount: number;
}

/** Normalize a project-relative path for comparison: unify separators to `/`
 *  and strip leading `./` so backslash and dot-prefixed paths match. */
function normalizeRelPath(p: string): string {
  return p.replace(/\\/g, "/").replace(/^\.\//, "");
}

/**
 * Build the ledger model for a single building.
 *
 * @param records  The full sin ledger from `polis_list_sins`.
 * @param filePath The building's project-relative path (`Building.filePath`).
 */
export function buildAnomalyLedgerModel(
  records: SinRecord[],
  filePath: string,
): AnomalyLedgerModel {
  const filtered = records.filter(
    (r) => normalizeRelPath(r.relPath) === normalizeRelPath(filePath),
  );

  const open: SinRecord[] = [];
  const ignored: SinRecord[] = [];
  let fixedCount = 0;

  for (const r of filtered) {
    if (r.disposition === "open") open.push(r);
    else if (r.disposition === "ignored") ignored.push(r);
    else if (r.disposition === "fixed") fixedCount += 1;
  }

  open.sort(compareSeverityThenRule);
  ignored.sort(compareSeverityThenRule);

  return { open, ignored, fixedCount };
}
