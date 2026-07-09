// P3.2 — Pure filter precomputation.
//
// Given the city, sin ledger, and FilterState, computes the two sets the
// renderer's one-pass applyFilter() uses. Pure (no DOM, no Tauri, no PIXI).
//
// Two-axis semantics (normative):
//   categories / minSeverity → HIDE EFFECTS only (fire/overlays). Never ghost
//     buildings. A building is effects-hidden when ALL of its open sins are
//     filtered out (category union + severity floor).
//   features / pathGlob → GHOST (or hide) buildings. Buildings kept by
//     features or matching the glob are shown; all others are ghosted.

import type { CityState, FilterState, SinRecord } from "../../types/city";
import { normalizeRelPath } from "./anomalyLedgerModel";

export interface FilterSets {
  /** Building fileIds whose building body is ghosted (alpha 0.15/hidden) */
  ghostedFileIds: Set<string>;
  /** Building fileIds whose sin-effects are hidden (fire/overlays only) */
  effectsHiddenFileIds: Set<string>;
  /** "ghost" | "hide" */
  mode: "ghost" | "hide";
  /** Counts for the footer result line */
  shownBuildings: number;
  totalBuildings: number;
  shownAnomalies: number;
  totalAnomalies: number;
}

/** Test a normalized relPath against a simple glob.
 *  `*` matches any characters INCLUDING `/` (cross-segment).
 *  When no `*` is present, falls back to case-insensitive substring match.
 *  Anchored: if the glob doesn't start with `*`, the first literal must match at
 *  position 0; if it doesn't end with `*`, the last literal must match at the end. */
export function matchGlob(path: string, glob: string): boolean {
  if (!glob) return true;
  const p = normalizeRelPath(path).toLowerCase();
  const g = normalizeRelPath(glob).toLowerCase();
  if (!g.includes("*")) {
    return p.includes(g);
  }
  const parts = g.split("*");
  // If the glob starts with a literal, anchor to position 0.
  let pos = 0;
  if (parts[0]) {
    if (!p.startsWith(parts[0])) return false;
    pos = parts[0].length;
  }
  for (let i = 1; i < parts.length - 1; i++) {
    const part = parts[i];
    if (!part) continue;
    const idx = p.indexOf(part, pos);
    if (idx === -1) return false;
    pos = idx + part.length;
  }
  // If the glob ends with a literal, anchor to the end of the path.
  const last = parts[parts.length - 1];
  if (last) {
    // Must be found at or after current pos, and reach the end.
    const idx = p.indexOf(last, pos);
    if (idx === -1 || idx + last.length !== p.length) return false;
  }
  return true;
}

/** Compute the filter sets for the renderer. Pure — no side effects. */
export function computeFilterSets(
  city: CityState | null,
  sinRecords: SinRecord[] | null,
  f: FilterState,
): FilterSets {
  const buildings = city?.buildings ?? [];
  const records = sinRecords ?? [];

  // --- Path/features → ghostedFileIds ---
  const featureSet = new Set(f.features);
  const hasFeatures = featureSet.size > 0;
  const hasGlob = f.pathGlob !== "";

  // Build relPath → fileId lookup
  const relPathToFileId = new Map<string, string>();
  for (const b of buildings) {
    relPathToFileId.set(normalizeRelPath(b.filePath), b.fileId);
  }

  // Ghosted: buildings NOT kept by features/pathGlob (when those axes are active)
  const ghostedFileIds = new Set<string>();

  for (const b of buildings) {
    const relPath = normalizeRelPath(b.filePath);
    let keep = true;

    if (hasFeatures) {
      // Must match at least one feature id
      const featureId = b.featureId ?? "";
      if (featureId && featureSet.has(featureId)) {
        keep = true;
      } else {
        keep = false;
      }
    }

    if (hasGlob) {
      // Must match glob to be kept
      keep = keep && matchGlob(relPath, f.pathGlob);
    }

    if (!keep) {
      ghostedFileIds.add(b.fileId);
    }
  }

  // Not ghosted = everything else
  const shownBuildings = buildings.length - ghostedFileIds.size;

  // --- Categories / severity → effectsHiddenFileIds ---
  const catSet = new Set(f.categories);
  const hasCats = catSet.size > 0;
  const hasSev = f.minSeverity !== null;

  // Build fileId → open sin records
  const openByFileId = new Map<string, SinRecord[]>();
  for (const r of records) {
    if (r.disposition !== "open") continue;
    const fileId = relPathToFileId.get(normalizeRelPath(r.relPath));
    if (!fileId) continue;
    let arr = openByFileId.get(fileId);
    if (!arr) {
      arr = [];
      openByFileId.set(fileId, arr);
    }
    arr.push(r);
  }

  const severityRank: Record<string, number> = { smoke: 1, fire: 2, inferno: 3 };

  const effectsHiddenFileIds = new Set<string>();

  // For the severity floor: show only if severity >= threshold
  const sevThreshold = f.minSeverity ? severityRank[f.minSeverity] : 0;

  for (const b of buildings) {
    const openSins = openByFileId.get(b.fileId) ?? [];
    if (openSins.length === 0) continue; // no sins → nothing to hide

    // A building is effects-hidden when ALL its open sins are filtered out.
    let allFiltered = true;
    for (const sin of openSins) {
      let filtered = false;
      // Category filter: sin's ruleId in the hidden set
      if (hasCats && catSet.has(sin.ruleId)) {
        filtered = true;
      }
      // Severity floor: sin below the threshold
      if (hasSev && severityRank[sin.severity] < sevThreshold) {
        filtered = true;
      }
      if (!filtered) {
        allFiltered = false;
        break;
      }
    }

    if (allFiltered && openSins.length > 0 && (hasCats || hasSev)) {
      effectsHiddenFileIds.add(b.fileId);
    }
  }

  // --- Anomaly counts ---
  const totalAnomalies = records.filter((r) => r.disposition === "open").length;

  // Shown anomalies: open sins on buildings that are NOT ghosted AND whose
  // fileId is NOT effects-hidden
  let shownAnomalies = 0;
  const ghostedOrHidden = new Set([...ghostedFileIds, ...effectsHiddenFileIds]);
  for (const r of records) {
    if (r.disposition !== "open") continue;
    const fileId = relPathToFileId.get(normalizeRelPath(r.relPath));
    if (!fileId || ghostedOrHidden.has(fileId)) continue;
    // Also check: is this sin's ruleId/category hidden? The spec says
    // "shownAnomalies" counts anomalies that are VISIBLE. So if the sin's
    // effect is hidden (by category union + severity), it's also not "shown."
    let sinVisible = true;
    if (hasCats && catSet.has(r.ruleId)) sinVisible = false;
    if (hasSev && severityRank[r.severity] < sevThreshold) sinVisible = false;
    if (sinVisible) shownAnomalies++;
  }

  return {
    ghostedFileIds,
    effectsHiddenFileIds,
    mode: f.mode,
    shownBuildings,
    totalBuildings: buildings.length,
    shownAnomalies,
    totalAnomalies,
  };
}
