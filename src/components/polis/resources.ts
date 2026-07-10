// Resources — deterministic quarry/mine resource site planner.
//
// One site per district whose assetCensus total (images + fonts + media) >= 8.
// Sites are placed OUTSIDE the district bounds in the countryside, in the 4
// compass side midpoints at a margin of ~4 tiles beyond the district bbox.
// Deterministic: same input → identical output. No Math.random().
//
// PERFORMANCE: pure function, computed at setCityState time; rendering is ONE
// Sprite per site + scattered prop:rock sprites (y-sorted).

import { hashString } from "./rng";
import type { Bounds, TerrainData } from "../../types/city";

/** Minimal structural type for the planner — avoids importing CityState
 *  (which has gridSize, agents, etc.) so tsc catches missing fields. */
export type ResourceCity = {
  districts: { districtId: string; name: string; bounds: Bounds; assetCensus?: { images: number; fonts: number; media: number } }[];
  buildings: { coords: { x: number; y: number } }[];
  roads: { path?: { x: number; y: number }[] }[];
  terrain?: TerrainData;
};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface ResourceSite {
  id: string;
  districtId: string;
  districtLabel: string;
  kind: "quarry" | "mine";
  variant: number;
  gx: number;
  gy: number;
  census: { images: number; fonts: number; media: number };
}

// Footprint sizes in tiles (width x height, bottom-center anchor).
const MINE_FOOTPRINT = { w: 5, h: 5 };
const QUARRY_FOOTPRINT = { w: 3, h: 3 };

// Minimum total assetCensus to qualify.
const CENSUS_THRESHOLD = 8;

// Initial margin beyond district bbox in tiles.
const BASE_MARGIN = 4;
// Expansion step when all 4 side midpoints are blocked.
const MARGIN_STEP = 3;
const MAX_ATTEMPTS = 3;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/** Total asset census count (sparse wire: absent means 0). */
function censusTotal(c?: { images: number; fonts: number; media: number }): number {
  if (!c) return 0;
  return c.images + c.fonts + c.media;
}

/** Deterministic small hash of a string → non-negative int. */
function smallHash(s: string): number {
  return hashString(s);
}

/**
 * Determine kind and variant from districtId.
 * kind: images strictly greater than fonts+media → "quarry", else "mine".
 * variant: deterministic from districtId hash. Quarry picks v0/v1; mine has one variant.
 */
function resolveKindVariant(
  districtId: string,
  census: { images: number; fonts: number; media: number },
): { kind: "quarry" | "mine"; variant: number } {
  const kind: "quarry" | "mine" = census.images > census.fonts + census.media ? "quarry" : "mine";
  const h = smallHash(`resource:${districtId}`);
  const variant = kind === "quarry" ? (h % 2) : 0;
  return { kind, variant };
}

/**
 * Build the set of blocked tile keys from city data.
 * Blocked = building footprint tiles + road path tiles + water tiles + sand tiles.
 */
function buildBlockedTiles(city: CityState): Set<string> {
  const blocked = new Set<string>();

  // Building footprints (with 1-tile Chebyshev margin).
  for (const b of city.buildings) {
    const tx = Math.round(b.coords.x);
    const ty = Math.round(b.coords.y);
    for (let dy = -1; dy <= 1; dy++) {
      for (let dx = -1; dx <= 1; dx++) {
        blocked.add(`${tx + dx},${ty + dy}`);
      }
    }
  }

  // Road path tiles.
  for (const r of city.roads) {
    if (!r.path) continue;
    for (const p of r.path) {
      blocked.add(`${Math.round(p.x)},${Math.round(p.y)}`);
    }
  }

  // Water tiles.
  if (city.terrain?.water) {
    for (const w of city.terrain.water) {
      blocked.add(`${w.gx},${w.gy}`);
    }
  }

  // Sand tiles.
  if (city.terrain?.sand) {
    for (const s of city.terrain.sand) {
      blocked.add(`${s.gx},${s.gy}`);
    }
  }

  return blocked;
}

/**
 * Check if a footprint rectangle (gx, gy, w, h) overlaps any blocked tile
 * or any other district's bounds.
 */
function footprintOverlaps(
  gx: number,
  gy: number,
  w: number,
  h: number,
  blocked: Set<string>,
  otherBounds: Bounds[],
  placedSites: { gx: number; gy: number; w: number; h: number }[],
): boolean {
  for (let dy = 0; dy < h; dy++) {
    for (let dx = 0; dx < w; dx++) {
      const key = `${gx + dx},${gy + dy}`;
      if (blocked.has(key)) return true;
    }
  }
  // Check overlap with other district bounds.
  for (const ob of otherBounds) {
    // Two rects overlap iff they share at least one tile.
    if (gx + w > ob.x && gx < ob.x + ob.w && gy + h > ob.y && gy < ob.y + ob.h) {
      return true;
    }
  }
  // Check overlap with already-placed sites.
  for (const ps of placedSites) {
    if (gx + w > ps.gx && gx < ps.gx + ps.w && gy + h > ps.gy && gy < ps.gy + ps.h) {
      return true;
    }
  }
  return false;
}

/**
 * Get the 4 compass side midpoints of a district bounds at a given margin.
 * Order: E, S, W, N (deterministic).
 * Each midpoint is the centre of the site's footprint placed outside the district.
 */
function sideMidpoints(
  bounds: Bounds,
  siteW: number,
  siteH: number,
  margin: number,
): { gx: number; gy: number }[] {
  const cx = bounds.x + bounds.w / 2;
  const cy = bounds.y + bounds.h / 2;

  return [
    // E: right side, vertically centred
    { gx: Math.round(bounds.x + bounds.w + margin), gy: Math.round(cy - siteH / 2) },
    // S: bottom side, horizontally centred
    { gx: Math.round(cx - siteW / 2), gy: Math.round(bounds.y + bounds.h + margin) },
    // W: left side, vertically centred
    { gx: Math.round(bounds.x - siteW - margin), gy: Math.round(cy - siteH / 2) },
    // N: top side, horizontally centred
    { gx: Math.round(cx - siteW / 2), gy: Math.round(bounds.y - siteH - margin) },
  ];
}

// ---------------------------------------------------------------------------
// Plan function (pure)
// ---------------------------------------------------------------------------

/**
 * Plan resource sites for qualifying districts.
 *
 * Rules:
 * - One site per district whose assetCensus total >= CENSUS_THRESHOLD.
 * - kind: images > fonts+media → quarry, else mine.
 * - Position: countryside OUTSIDE the district bounds, 4 side midpoints at
 *   margin ~4 tiles, expanding up to 3 times. First non-overlapping spot wins.
 * - Deterministic: same input → identical output.
 */
export function planResourceSites(city: ResourceCity): ResourceSite[] {
  const blocked = buildBlockedTiles(city);
  // Terrain band guard: reject candidates past the sea edge or outside the
  // terrain y-band.  When terrain is absent, no edge guard is needed (the city
  // lives in arbitrary negative-space coords — a spiral layout).
  const seaX = city.terrain?.seaX ?? Infinity;
  const bandMinY = city.terrain?.minY ?? -Infinity;
  const bandMaxY = city.terrain?.maxY ?? Infinity;
  const hasTerrain = city.terrain != null;
  const sites: ResourceSite[] = [];
  const placedFootprints: { gx: number; gy: number; w: number; h: number }[] = [];
  const otherBounds: Bounds[] = [];

  // Build list of OTHER district bounds for overlap checking.
  // We process districts in deterministic order (by districtId).
  const sortedDistricts = [...city.districts].sort((a, b) =>
    a.districtId.localeCompare(b.districtId),
  );

  for (const district of sortedDistricts) {
    const census = district.assetCensus ?? { images: 0, fonts: 0, media: 0 };
    if (censusTotal(census) < CENSUS_THRESHOLD) continue;

    const { kind, variant } = resolveKindVariant(district.districtId, census);
    const fp = kind === "quarry" ? QUARRY_FOOTPRINT : MINE_FOOTPRINT;
    const siteId = `res:${district.districtId}`;

    let placed = false;
    for (let attempt = 0; attempt < MAX_ATTEMPTS && !placed; attempt++) {
      const margin = BASE_MARGIN + attempt * MARGIN_STEP;
      const candidates = sideMidpoints(district.bounds, fp.w, fp.h, margin);

      for (const cand of candidates) {
        // Terrain band guard: reject if any footprint tile is past the sea
        // edge (gx >= seaX) or outside the terrain y-band.
        const outOfTerrainBand = hasTerrain && (
          cand.gx >= seaX || cand.gx + fp.w > seaX ||
          cand.gy < bandMinY || cand.gy + fp.h > bandMaxY
        );
        if (
          !outOfTerrainBand &&
          !footprintOverlaps(cand.gx, cand.gy, fp.w, fp.h, blocked, otherBounds, placedFootprints)
        ) {
          sites.push({
            id: siteId,
            districtId: district.districtId,
            districtLabel: district.name,
            kind,
            variant,
            gx: cand.gx,
            gy: cand.gy,
            census,
          });
          placedFootprints.push({ gx: cand.gx, gy: cand.gy, w: fp.w, h: fp.h });
          placed = true;
          break;
        }
      }
    }
    // If all attempts fail (all candidates blocked or out-of-bounds), no site is placed.

    // Add this district's bounds to the "other" set for subsequent districts.
    otherBounds.push(district.bounds);
  }

  return sites;
}

// ---------------------------------------------------------------------------
// Tile set helpers (for props/field blockers)
// ---------------------------------------------------------------------------

/**
 * Return the set of tile keys covered by resource site footprints.
 * Used to exclude props, fields, and forest patches from site areas.
 */
export function resourceSiteTiles(sites: ResourceSite[]): Set<string> {
  const tiles = new Set<string>();
  for (const site of sites) {
    const fp = site.kind === "quarry" ? QUARRY_FOOTPRINT : MINE_FOOTPRINT;
    for (let dy = 0; dy < fp.h; dy++) {
      for (let dx = 0; dx < fp.w; dx++) {
        tiles.add(`${site.gx + dx},${site.gy + dy}`);
      }
    }
  }
  return tiles;
}

/**
 * Return the footprint dimensions for a site.
 */
export function siteFootprint(kind: "quarry" | "mine"): { w: number; h: number } {
  return kind === "quarry" ? QUARRY_FOOTPRINT : MINE_FOOTPRINT;
}
