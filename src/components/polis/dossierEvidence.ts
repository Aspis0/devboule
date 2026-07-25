// Polis — pure dossier EVIDENCE extraction (frontend-only).
//
// The "More details" dossier is narrative (Oracle, cached) PLUS concrete facts
// already on the loaded city state. This module builds the evidence half:
//   - importers / imports from the road graph (from = importer, to = imported)
//   - sins grouped by severity (smoke / fire / inferno)
//   - role / visual tier / district
//
// Pure (no DOM, no Tauri, no Math.random, no Map iteration for user-visible
// order) so it is unit-testable and deterministic.

import type {
  Building,
  CityState,
  SinSeverity,
  UrbanSin,
} from "../../types/city";
import { purposeLabel } from "../../types/city";

/** How many peer paths to list before collapsing the rest behind "+N more". */
export const EVIDENCE_PEER_CAP = 5;

/** Severity display order (worst first). Unknown severities sort after known. */
const SEVERITY_ORDER: readonly SinSeverity[] = ["inferno", "fire", "smoke"];

const SEVERITY_RANK: Record<string, number> = {
  inferno: 0,
  fire: 1,
  smoke: 2,
};

function severityRank(s: string): number {
  return SEVERITY_RANK[s] ?? 99;
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

export interface DossierEvidencePeer {
  fileId: string;
  filePath: string;
  label: string;
}

/** One direction of the import graph (imports out, or importers in). */
export interface DossierEvidencePeerList {
  total: number;
  /** First EVIDENCE_PEER_CAP peers, sorted deterministically. */
  shown: DossierEvidencePeer[];
  /** Exact remainder: total - shown.length (for "+N more"). */
  moreCount: number;
  /**
   * Explicit empty copy when total === 0. Never null in the "ready" graph case —
   * empty lists speak plainly so the row cannot silently disappear.
   */
  emptyMessage: string | null;
}

export interface DossierEvidenceSinGroup {
  severity: SinSeverity;
  /** Descriptions in stable order (description ASC, then sinId ASC). */
  items: Array<{ sinId: string; description: string }>;
}

export type DossierEvidenceGraph =
  | {
      kind: "unavailable";
      /** Honest reason when city/roads are not on the client. */
      message: string;
    }
  | {
      kind: "ready";
      imports: DossierEvidencePeerList;
      importers: DossierEvidencePeerList;
    };

export interface DossierEvidence {
  /** Compact identity line inputs (always from the building). */
  purpose: string;
  purposeDisplay: string;
  visualTier: string;
  districtId: string;
  /** District display name when resolvable; else null (caller falls back to id). */
  districtName: string | null;
  graph: DossierEvidenceGraph;
  /**
   * Sin groups present on this building, ordered inferno → fire → smoke.
   * Empty array when the file has no sins (UI says so explicitly).
   */
  sinGroups: DossierEvidenceSinGroup[];
}

// ---------------------------------------------------------------------------
// Empty-copy constants (exported so UI + tests share the exact strings)
// ---------------------------------------------------------------------------

export const NOTHING_IMPORTS_THIS = "nothing imports this yet";
export const IMPORTS_NOTHING = "this file imports nothing yet";
export const GRAPH_UNAVAILABLE = "import graph not available for this file";
export const NO_DETECTED_PROBLEMS = "no detected problems";

// ---------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------

/**
 * Extract a structured evidence summary for one building from the loaded city.
 * Never invents peers or sins: missing city → graph.unavailable; zero edges →
 * explicit empty messages; sins only from `building.sins`.
 */
export function buildDossierEvidence(
  building: Building,
  city: CityState | null,
): DossierEvidence {
  const districtName =
    city?.districts.find((d) => d.districtId === building.districtId)?.name ??
    null;

  return {
    purpose: building.purpose,
    purposeDisplay: purposeLabel(building.purpose),
    visualTier: building.visualTier,
    districtId: building.districtId,
    districtName,
    graph: buildGraph(building.fileId, city),
    sinGroups: groupSins(building.sins ?? []),
  };
}

/** Compact single-line identity: role · tier · district. */
export function formatRoleLine(ev: DossierEvidence): string {
  const district = ev.districtName?.trim() || ev.districtId || "—";
  return `${ev.purposeDisplay} · ${ev.visualTier} · ${district}`;
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

function buildGraph(
  fileId: string,
  city: CityState | null,
): DossierEvidenceGraph {
  if (!city) {
    return { kind: "unavailable", message: GRAPH_UNAVAILABLE };
  }

  // Stable index: buildings array order is whatever the scanner emitted; we
  // only LOOK UP by id, never iterate Map keys for user-visible lists.
  const byId = new Map<string, Building>();
  for (const b of city.buildings) {
    byId.set(b.fileId, b);
  }

  // Accumulators: collect then sort (do not rely on Map insertion order).
  const importsRaw: DossierEvidencePeer[] = [];
  const importersRaw: DossierEvidencePeer[] = [];
  // weight for sort only — parallel arrays kept in step with raw lists
  const importWeights: number[] = [];
  const importerWeights: number[] = [];

  for (const road of city.roads) {
    // Import graph orientation: from = importer (consumer), to = imported.
    // Include every road type that participates in the city graph the same way
    // the Connections section does (honest edges already on state).
    if (road.from === fileId) {
      const t = byId.get(road.to);
      if (t) {
        importsRaw.push(peerFrom(t));
        importWeights.push(road.weight ?? 0);
      }
    } else if (road.to === fileId) {
      const s = byId.get(road.from);
      if (s) {
        importersRaw.push(peerFrom(s));
        importerWeights.push(road.weight ?? 0);
      }
    }
  }

  return {
    kind: "ready",
    imports: toPeerList(importsRaw, importWeights, IMPORTS_NOTHING),
    importers: toPeerList(importersRaw, importerWeights, NOTHING_IMPORTS_THIS),
  };
}

function peerFrom(b: Building): DossierEvidencePeer {
  return {
    fileId: b.fileId,
    filePath: b.filePath,
    label: b.label,
  };
}

function toPeerList(
  peers: DossierEvidencePeer[],
  weights: number[],
  emptyMessage: string,
): DossierEvidencePeerList {
  // Deterministic: weight DESC, then filePath ASC, then fileId ASC.
  const indexed = peers.map((p, i) => ({ p, w: weights[i] ?? 0 }));
  indexed.sort(
    (a, b) =>
      b.w - a.w ||
      a.p.filePath.localeCompare(b.p.filePath) ||
      a.p.fileId.localeCompare(b.p.fileId),
  );

  // De-dupe by fileId after sort (keep highest-weight occurrence).
  const seen = new Set<string>();
  const unique: DossierEvidencePeer[] = [];
  for (const { p } of indexed) {
    if (seen.has(p.fileId)) continue;
    seen.add(p.fileId);
    unique.push(p);
  }

  const total = unique.length;
  if (total === 0) {
    return {
      total: 0,
      shown: [],
      moreCount: 0,
      emptyMessage,
    };
  }

  const shown = unique.slice(0, EVIDENCE_PEER_CAP);
  return {
    total,
    shown,
    moreCount: total - shown.length,
    emptyMessage: null,
  };
}

function groupSins(sins: UrbanSin[]): DossierEvidenceSinGroup[] {
  const buckets = new Map<string, Array<{ sinId: string; description: string }>>();
  for (const s of sins) {
    const sev = s.severity;
    let list = buckets.get(sev);
    if (!list) {
      list = [];
      buckets.set(sev, list);
    }
    list.push({
      sinId: s.sinId,
      description: s.description,
    });
  }

  // Emit known severities first (inferno → fire → smoke), then any unknown
  // slug ASC. Skip empty buckets so the list is only real problems.
  const known = SEVERITY_ORDER.filter((sev) => buckets.has(sev));
  const unknown = Array.from(buckets.keys())
    .filter((k) => severityRank(k) === 99)
    .sort((a, b) => a.localeCompare(b));

  const groups: DossierEvidenceSinGroup[] = [];
  for (const key of [...known, ...unknown]) {
    const items = buckets.get(key)!;
    items.sort(
      (a, b) =>
        a.description.localeCompare(b.description) ||
        a.sinId.localeCompare(b.sinId),
    );
    groups.push({
      severity: key as SinSeverity,
      items,
    });
  }
  return groups;
}
