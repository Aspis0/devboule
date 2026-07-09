// Polis P3.1 — pure model for the project-wide Anomalies panel.
//
// Given the full sin ledger, partitions records into open / ignored, attaches
// severity-ranked rows with age formatting. Pure (no DOM, no Tauri) so it is
// fully unit-testable.

import type { SinRecord } from "../../types/city";
import { normalizeRelPath } from "./anomalyLedgerModel";

// ---------------------------------------------------------------------------
// Severity ranking (matches anomalyLedgerModel.ts pattern)
// ---------------------------------------------------------------------------

const SEVERITY_RANK: Record<string, number> = {
  inferno: 3,
  fire: 2,
  smoke: 1,
};

function severityRank(s: string): number {
  return SEVERITY_RANK[s] ?? 0;
}

// ---------------------------------------------------------------------------
// Age formatting
// ---------------------------------------------------------------------------

/** Coarse human-readable age from an ISO timestamp. Inject `now` for testability. */
export function formatAge(createdAt: string, now: number): string {
  const parsed = new Date(createdAt).getTime();
  if (Number.isNaN(parsed)) return "—";
  const ms = now - parsed;
  if (ms < 0) return "just now";
  const secs = Math.floor(ms / 1000);
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  return `${days}d`;
}

// ---------------------------------------------------------------------------
// Row type
// ---------------------------------------------------------------------------

export interface AnomalyRow {
  sin: SinRecord;
  /** Project-relative path (convenience alias). */
  relPath: string;
  /** The building's fileId for flyTo, or null if the sin's file is not in the city. */
  fileId: string | null;
  /** Coarse age label. */
  age: string;
}

// ---------------------------------------------------------------------------
// Panel model
// ---------------------------------------------------------------------------

export interface AnomaliesPanelModel {
  /** Open sins, sorted severity desc then oldest first. */
  open: AnomalyRow[];
  /** Ignored sins, sorted severity desc then oldest first. */
  ignored: AnomalyRow[];
  /** Open count (for badge). */
  openCount: number;
}

/**
 * Sort comparator: severity DESC, then oldest first (ascending createdAt).
 */
function compareSeverityThenAge(
  a: AnomalyRow,
  b: AnomalyRow,
): number {
  const d = severityRank(b.sin.severity) - severityRank(a.sin.severity);
  if (d !== 0) return d;
  return (
    new Date(a.sin.createdAt).getTime() - new Date(b.sin.createdAt).getTime()
  );
}

/**
 * Build the project-wide anomalies panel model.
 *
 * @param records  The full sin ledger from `polis_list_sins` (may be null).
 * @param buildingFileIds  Map from normalized relPath → building fileId
 *                         (derived from cityState.buildings).
 * @param now      Current timestamp (ms) for age computation. Inject for tests.
 */
export function buildAnomaliesPanelModel(
  records: SinRecord[] | null,
  buildingFileIds: Map<string, string>,
  now: number,
): AnomaliesPanelModel {
  if (!records || records.length === 0) {
    return { open: [], ignored: [], openCount: 0 };
  }

  const open: AnomalyRow[] = [];
  const ignored: AnomalyRow[] = [];

  for (const sin of records) {
    const row: AnomalyRow = {
      sin,
      relPath: sin.relPath,
      fileId: buildingFileIds.get(normalizeRelPath(sin.relPath)) ?? null,
      age: formatAge(sin.createdAt, now),
    };
    if (sin.disposition === "open") open.push(row);
    else if (sin.disposition === "ignored") ignored.push(row);
    // "fixed" sins are excluded from both tabs.
  }

  open.sort(compareSeverityThenAge);
  ignored.sort(compareSeverityThenAge);

  return { open, ignored, openCount: open.length };
}
