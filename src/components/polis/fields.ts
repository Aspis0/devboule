// Fields — deterministic farmland parcel planner + drawer.
//
// Like Caesar III's agricultural belts, fields fill the empty grass plains
// between districts with crop parcels, vineyards, orchards, and fallow land.
// Purely decorative, static, deterministic — never implies a file exists.
//
// DETERMINISM: same input → identical output. Uses only rng.ts helpers.
// PERFORMANCE: drawn ONCE into Graphics at build time; no per-frame work.

import { Graphics } from "pixi.js";
import { cartToIso } from "./iso";
import { hashString, Rng } from "./rng";
import type { TerrainExtent } from "./terrain";
import type { Bounds, TerrainData } from "../../types/city";
import { Proj, Z_UNIT } from "./kitcd/iso";
import { gardenBed } from "./kitcd/detail";
import { roundTile } from "./navWalkable";
import {
  cropRows,
  vineyard,
  orchardGrid,
  fallowField,
  haystack,
  farmShed,
} from "./kitcd/farm";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_PARCELS = 160;
const PARCEL_SIZES: [number, number][] = [
  [8, 6],
  [7, 5],
  [6, 4],
  [5, 4],
  [4, 3],
];
const LATTICE_STEP = 3;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface FieldParcel {
  x: number;
  y: number;
  w: number;
  h: number;
  kind: "garden" | "crops" | "vineyard" | "orchard" | "fallow";
  accents: { shed?: { x: number; y: number }; haystack?: { x: number; y: number } };
  seed: number;
}

// ---------------------------------------------------------------------------
// buildFieldBlockedSet — pure, exported for testability
// ---------------------------------------------------------------------------

/**
 * Build the set of blocked tiles for field planning:
 * - Building footprint tiles dilated by 1 (Chebyshev +1 margin).
 * - Road tiles (from road path polylines).
 * - Water tiles.
 * - Bridge tiles.
 *
 * Returns a Set of "x,y" keys.
 */
export function buildFieldBlockedSet(
  buildings: { coords: { x: number; y: number }[] }[],
  roads: { path?: { x: number; y: number }[] }[],
  terrain?: TerrainData,
): Set<string> {
  const blocked = new Set<string>();

  // Building footprints dilated by 1 (Chebyshev +1).
  for (const b of buildings) {
    for (const c of b.coords) {
      const cx = roundTile(c.x);
      const cy = roundTile(c.y);
      for (let dx = -1; dx <= 1; dx++) {
        for (let dy = -1; dy <= 1; dy++) {
          blocked.add(`${cx + dx},${cy + dy}`);
        }
      }
    }
  }

  // Road tiles from path polylines.
  for (const r of roads) {
    if (!r.path) continue;
    for (const p of r.path) {
      const tx = Math.round(p.x);
      const ty = Math.round(p.y);
      blocked.add(`${tx},${ty}`);
    }
  }

  // Water tiles.
  if (terrain?.water) {
    for (const w of terrain.water) {
      blocked.add(`${w.gx},${w.gy}`);
    }
  }

  // Bridge tiles.
  if (terrain?.bridges) {
    for (const br of terrain.bridges) {
      blocked.add(`${br.gx},${br.gy}`);
    }
  }

  return blocked;
}

// ---------------------------------------------------------------------------
// Dilate district bounds by 1 tile (Chebyshev margin).
// ---------------------------------------------------------------------------

function dilateBounds(b: Bounds): Bounds {
  return { x: b.x - 1, y: b.y - 1, w: b.w + 2, h: b.h + 2 };
}

// ---------------------------------------------------------------------------
// planFields — deterministic parcel placement
// ---------------------------------------------------------------------------

/**
 * Plan farmland parcels on empty ground between districts.
 *
 * Algorithm: scan candidate origins on a lattice of step 3, row-major.
 * At each origin, try parcel sizes largest-first; accept the first that
 * fits on free tiles. Mark tiles + 1-tile gap ring as used. Continue.
 *
 * Kind is determined by normalized Chebyshev distance from `centre`.
 * Accents: 25% shed, 35% haystack (never on gardens, never same tile).
 */
export function planFields(input: {
  ext: TerrainExtent;
  districts: Bounds[];
  blocked: Set<string>;
  centre: { x: number; y: number };
}): FieldParcel[] {
  const { ext, districts, blocked, centre } = input;
  const dilatedDistricts = districts.map(dilateBounds);

  // Working set of occupied tiles (blocked + placed parcels + gap rings).
  const used = new Set(blocked);

  // Pre-rasterize dilated district bounds into the `used` set so the per-tile
  // placement loop is a single `used.has()` check — eliminates the O(tiles×D)
  // inner district-loop that would unboundedly slow cities with many districts.
  // Floor/ceil to integer tile coverage so fractional bounds (Rust f64) match
  // the integer lattice keys used elsewhere.
  for (const db of dilatedDistricts) {
    const ix = Math.floor(db.x), iy = Math.floor(db.y);
    const ix2 = Math.ceil(db.x + db.w), iy2 = Math.ceil(db.y + db.h);
    for (let ty = iy; ty < iy2; ty++) {
      for (let tx = ix; tx < ix2; tx++) {
        used.add(`${tx},${ty}`);
      }
    }
  }

  // Half-extent for normalized distance.  For tiny extents (both half-axes < 2)
  // the banding thresholds collapse into the mid-band and don't produce garden
  // parcels at the centre — treat everything as garden.
  const halfW = (ext.maxX - ext.minX) / 2;
  const halfH = (ext.maxY - ext.minY) / 2;
  const isTinyExtent = halfW < 2 && halfH < 2;

  const parcels: FieldParcel[] = [];

  // Scan candidate origins in row-major order (deterministic).
  for (let gy = ext.minY; gy <= ext.maxY && parcels.length < MAX_PARCELS; gy += LATTICE_STEP) {
    for (let gx = ext.minX; gx <= ext.maxX && parcels.length < MAX_PARCELS; gx += LATTICE_STEP) {
      // Try parcel sizes largest-first.
      for (const [pw, ph] of PARCEL_SIZES) {
        // Check if the parcel fits entirely within the extent.
        if (gx + pw > ext.maxX + 1 || gy + ph > ext.maxY + 1) continue;

        // Check if all tiles are free (not blocked, not in a dilated district, not already used).
        let fits = true;
        outer: for (let dy = 0; dy < ph; dy++) {
          for (let dx = 0; dx < pw; dx++) {
            if (used.has(`${gx + dx},${gy + dy}`)) {
              fits = false;
              break outer;
            }
          }
        }
        if (!fits) continue;

        // Parcel fits! Determine kind by distance from centre.
        const parcelCx = gx + pw / 2;
        const parcelCy = gy + ph / 2;
        // Normalized Chebyshev distance from centre, per-axis so elongated
        // maps (e.g. 200×40) don't compress the y-axis into inner bands.
        // Guarded: halfW/halfH can be 0 for degenerate extents.
        const dist = isTinyExtent
          ? 0
          : Math.max(
              Math.abs(parcelCx - centre.x) / Math.max(halfW, 1),
              Math.abs(parcelCy - centre.y) / Math.max(halfH, 1),
            );

        const parcelSeed = hashString(`parcel:${gx},${gy}`);
        const rng = new Rng(parcelSeed);

        let kind: FieldParcel["kind"];
        if (isTinyExtent) {
          kind = "garden";
        } else if (dist < 0.35) {
          kind = "garden";
        } else if (dist < 0.6) {
          kind = rng.float() < 0.7 ? "crops" : "vineyard";
        } else if (dist < 0.8) {
          kind = "orchard";
        } else {
          kind = rng.float() < 0.5 ? "fallow" : "orchard";
        }

        // Accents (not for garden parcels).
        const accents: FieldParcel["accents"] = {};
        if (kind !== "garden") {
          // Shed: 25% chance on a corner tile.
          if (rng.float() < 0.25) {
            const corners = [
              { x: gx, y: gy },
              { x: gx + pw - 1, y: gy },
              { x: gx, y: gy + ph - 1 },
              { x: gx + pw - 1, y: gy + ph - 1 },
            ];
            accents.shed = corners[Math.floor(rng.float() * corners.length)];
          }
          // Haystack: 35% chance on an edge tile (not same as shed).
          if (rng.float() < 0.35) {
            const edgeTiles: { x: number; y: number }[] = [];
            for (let dx2 = 0; dx2 < pw; dx2++) {
              edgeTiles.push({ x: gx + dx2, y: gy });
              if (ph > 1) edgeTiles.push({ x: gx + dx2, y: gy + ph - 1 });
            }
            for (let dy2 = 1; dy2 < ph - 1; dy2++) {
              edgeTiles.push({ x: gx, y: gy + dy2 });
              if (pw > 1) edgeTiles.push({ x: gx + pw - 1, y: gy + dy2 });
            }
            // Filter out shed tile if present.
            const filtered = accents.shed
              ? edgeTiles.filter(
                  (t) => !(t.x === accents.shed!.x && t.y === accents.shed!.y),
                )
              : edgeTiles;
            if (filtered.length > 0) {
              accents.haystack = filtered[Math.floor(rng.float() * filtered.length)];
            }
          }
        }

        parcels.push({ x: gx, y: gy, w: pw, h: ph, kind, accents, seed: parcelSeed });

        // Mark parcel tiles + 1-tile gap ring as used.
        for (let dy = -1; dy <= ph; dy++) {
          for (let dx = -1; dx <= pw; dx++) {
            used.add(`${gx + dx},${gy + dy}`);
          }
        }

        break; // Move to next lattice point after placing a parcel.
      }
    }
  }

  if (parcels.length >= MAX_PARCELS) {
    // eslint-disable-next-line no-console
    console.warn(
      `[polis:fields] parcel cap reached (${MAX_PARCELS}); some empty ground may remain unfilled.`,
    );
  }

  return parcels;
}

// ---------------------------------------------------------------------------
// parcelTiles — union of all parcel rect tiles
// ---------------------------------------------------------------------------

/**
 * Return the set of all tiles covered by parcels (used to exclude props).
 */
export function parcelTiles(parcels: FieldParcel[]): Set<string> {
  const tiles = new Set<string>();
  for (const p of parcels) {
    for (let dy = 0; dy < p.h; dy++) {
      for (let dx = 0; dx < p.w; dx++) {
        tiles.add(`${p.x + dx},${p.y + dy}`);
      }
    }
  }
  return tiles;
}

// ---------------------------------------------------------------------------
// drawFields — draw all parcels into a single Graphics
// ---------------------------------------------------------------------------

/**
 * Draw farmland parcels. Returns a Graphics (caller owns destruction).
 * Uses a cartToIso-based Proj compatible with kitcd detail primitives.
 */
export function drawFields(
  _ext: TerrainExtent,
  parcels: FieldParcel[],
): { graphics: Graphics } {
  // Build a simple Proj that maps tile coords to iso screen space.
  // This is equivalent to cartToIso but with the Proj interface the kitcd
  // primitives expect.
  const proj: Proj = {
    W: 0,
    D: 0,
    p(gx: number, gy: number, gz?: number) {
      const c = cartToIso(gx, gy);
      return { x: c.x, y: c.y - (gz || 0) * Z_UNIT };
    },
  };

  const g = new Graphics();

  for (const parcel of parcels) {
    const { x, y, w, h, kind, accents, seed } = parcel;

    switch (kind) {
      case "garden":
        gardenBed(g, proj, x, y, w, h, seed);
        break;
      case "crops":
        cropRows(g, proj, x, y, w, h, seed);
        break;
      case "vineyard":
        vineyard(g, proj, x, y, w, h, seed);
        break;
      case "orchard":
        orchardGrid(g, proj, x, y, w, h, seed);
        break;
      case "fallow":
        fallowField(g, proj, x, y, w, h, seed);
        break;
    }

    // Accents.
    if (accents.shed) {
      farmShed(g, proj, accents.shed.x, accents.shed.y, seed + 777);
    }
    if (accents.haystack) {
      haystack(g, proj, accents.haystack.x, accents.haystack.y, seed + 888);
    }
  }

  return { graphics: g };
}
