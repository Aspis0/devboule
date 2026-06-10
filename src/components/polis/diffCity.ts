// Pure city-diff for the Polis LIVE update path.
//
// When the backend fs-watcher re-scans on a file change it EMITS the full new
// CityState (`polis://city-updated`). Rather than tear the whole scene down and
// recenter the camera, the renderer applies an IN-PLACE diff: only the buildings
// that actually changed are rebuilt. This module is the pure, deterministic core
// of that diff — given the previous and next building lists it returns the
// fileId sets {added, changed, removed}. It allocates nothing per frame (it runs
// once per fs event) and fabricates nothing (1:1 with backend data).
//
// Kept as a standalone PURE function (no PIXI, no DOM) so it is trivially
// unit-testable. The project currently has no JS test runner; see
// `diffCity.test.ts` for the spec that runs once one is added.

import type { Building, CityState, SinSeverity } from "../../types/city";

/**
 * Severity rank for an urban sin — higher is worse. Used to collapse a
 * building's `sins[]` to a single "worst severity" scalar so the diff rebuilds
 * the node only when the WORST sin gets better/worse, not when sins are merely
 * reordered or a lesser sin is added/removed below the current worst.
 *   none < smoke < fire < inferno
 */
function sinSeverityRank(s: SinSeverity): number {
  switch (s) {
    case "inferno":
      return 3;
    case "fire":
      return 2;
    case "smoke":
      return 1;
    default:
      // Unknown/Oracle-introduced severity: treat as the lowest "present" rank
      // so it still ranks above "no sins" without out-ranking a known fire.
      return 1;
  }
}

/** The rank of a building's worst sin, or 0 when it has none. */
function worstSinRank(b: Building): number {
  let worst = 0;
  for (const sin of b.sins) {
    const r = sinSeverityRank(sin.severity);
    if (r > worst) worst = r;
  }
  return worst;
}

/**
 * The WORST sin severity on a building, or `null` when it has none — the single
 * scalar the on-map disaster overlay is keyed on (a node with no sins gets no
 * overlay; smoke/fire/inferno select the overlay intensity). Shares the SAME
 * `none < smoke < fire < inferno` rank ordering the diff rebuild uses
 * (`worstSinRank`), so the overlay the renderer attaches always agrees with the
 * severity transition that triggered the rebuild — there is ONE source of truth
 * for "worst severity". An unknown/Oracle-introduced severity ranks as the
 * lowest "present" tier and is therefore reported as `"smoke"` (the visual floor
 * for "this building has a problem"). Pure: 1:1 with real `building.sins`.
 */
export function worstSinSeverity(b: Building): SinSeverity | null {
  let worst = 0;
  let sev: SinSeverity | null = null;
  for (const sin of b.sins) {
    const r = sinSeverityRank(sin.severity);
    if (r > worst) {
      worst = r;
      // Normalize to a KNOWN severity tier from the rank so an unknown input
      // maps to the same visual the diff already treats it as (rank 1 = smoke).
      sev = r >= 3 ? "inferno" : r >= 2 ? "fire" : "smoke";
    }
  }
  return sev;
}

/** fileId partition of a city transition. */
export interface CityDiff {
  /** fileIds present in `next` but not in `prev` — build + add. */
  added: string[];
  /** fileIds present in both whose visual representation changed — rebuild. */
  changed: string[];
  /** fileIds present in `prev` but not in `next` — destroy + remove. */
  removed: string[];
}

/**
 * The fields that, if any differ, mean a building must be rebuilt for the diff
 * to look right. This is intentionally the set of inputs that feed
 * `buildBuilding` + placement + the smoke/agent overlays:
 *   - visualTier  → box size (the headline "instant resize")
 *   - purpose     → archetype silhouette
 *   - coords      → iso position / chunk / depth sort
 *   - provider    → tech-livery pennant (baked into the node at build time)
 *   - status      → active state (drives smoke/activity)
 *   - agentPresent→ active state (drives smoke/activity)
 *   - sins        → worst-sin severity (drives flames/smoke)
 *   - suspectOfCardId → "under investigation" smoke (bug-investigation P3)
 *
 * Returns true when the visual representation of `a` and `b` would differ.
 */
export function buildingChanged(a: Building, b: Building): boolean {
  // Every field below is baked into the building node at build time, so a change
  // to any of them must force a teardown+rebuild for the kit to react on a live
  // re-scan.
  if (a.visualTier !== b.visualTier) return true;
  if (a.purpose !== b.purpose) return true;
  if (a.coords.x !== b.coords.x || a.coords.y !== b.coords.y) return true;
  // TECH LIVERY (F4): the provider pennant is baked into the building node at
  // build time, so a provider change must trigger a rebuild for the livery to
  // appear/disappear/recolor on a live re-scan.
  if ((a.provider ?? null) !== (b.provider ?? null)) return true;
  // STATUS (L2 growth / disaster system): normal→active / normal→burning etc.
  // drives the node's active-state visuals (smoke, flames, activity), baked in
  // at build time, so a status change must rebuild.
  if (a.status !== b.status) return true;
  // SINS (disaster system): the WORST sin severity drives flames/smoke. Collapse
  // sins[] to a worst-severity rank and rebuild only when that scalar differs —
  // cosmetic reordering of sins, or adding/removing a lesser sin below the
  // current worst, must NOT churn the node.
  if (worstSinRank(a) !== worstSinRank(b)) return true;
  // SUSPECT (bug-investigation P3): the investigative-smoke overlay is baked into
  // the node at build time keyed on `suspectOfCardId`, so a building entering or
  // leaving "under investigation" (a bug card created / resolved) must rebuild the
  // node for the smoke to appear/clear on a live re-scan. Independent of sins, so a
  // building can be BOTH a suspect (smoke) and a confirmed disaster (fire) at once.
  if ((a.suspectOfCardId ?? null) !== (b.suspectOfCardId ?? null)) return true;
  // `agentPresent`: the glow under a building keys off it.
  return (a.agentPresent ?? null) !== (b.agentPresent ?? null);
}


/**
 * Compute the fileId partition between two building lists. Pure + deterministic:
 * a tier change lands a fileId in `changed`; a brand-new file in `added`; a
 * deleted file in `removed`; an untouched file in none of them.
 *
 * O(prev + next): one map of prev by fileId, one pass over next.
 */
export function diffBuildings(
  prev: readonly Building[],
  next: readonly Building[],
): CityDiff {
  const prevById = new Map<string, Building>();
  for (const b of prev) prevById.set(b.fileId, b);

  const added: string[] = [];
  const changed: string[] = [];
  const nextIds = new Set<string>();

  for (const b of next) {
    nextIds.add(b.fileId);
    const old = prevById.get(b.fileId);
    if (!old) {
      added.push(b.fileId);
    } else if (buildingChanged(old, b)) {
      changed.push(b.fileId);
    }
    // else: unchanged → costs nothing.
  }

  const removed: string[] = [];
  for (const b of prev) {
    if (!nextIds.has(b.fileId)) removed.push(b.fileId);
  }

  return { added, changed, removed };
}

/** Convenience wrapper over `diffBuildings` for whole CityStates. */
export function diffCities(prev: CityState, next: CityState): CityDiff {
  return diffBuildings(prev.buildings, next.buildings);
}

/** True if buildings were added or removed.
 *
 *  NOTE: this is NOT a complete "does terrain need a redraw?" predicate. A pure
 *  TIER CHANGE (no add/remove) can still grow a building's FOOTPRINT, which moves
 *  the map's `max_x` → the backend recomputes `sea_x` and the sea band, so the
 *  terrain DOES change without any add/remove. The renderer therefore gates its
 *  terrain redraw on a TERRAIN SIGNATURE diff (see `PolisRenderer.terrainSignature`),
 *  not on this function. Kept for the add/remove fast-path only. */
export function extentMayHaveChanged(diff: CityDiff): boolean {
  return diff.added.length > 0 || diff.removed.length > 0;
}
