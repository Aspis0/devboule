//! Polis Map — terrain (sea + rivers + shores + bridges) that FRAMES the city.
//!
//! ADDITIVE to the existing feature-district layout: after `layout()` packs the
//! buildings on land and `grid::route_roads` threads the streets, this module
//! classifies the surrounding terrain and emits a COMPACT, sparse description on
//! `CityState::terrain` for the frontend to draw water/shore/bridge tiles around
//! the unchanged city. It NEVER moves a building or reroutes a road.
//!
//! AXIS NOTE — the seaward edge is the EAST (+x) margin. `cloud::place_external_services`
//! anchors the harbour/cloud-outpost column at `max_x + GAP` (the buildings' east
//! extent), so the sea band MUST be on +x for "the city meets the cloud" to mean
//! the harbours sit ON the water. The HANDOFF demo uses `gy >= sea_y` (south); we
//! adapt rule 1 to the project's real seaward axis: `gx >= sea_x` => `Sea`. Every
//! other rule (rivers, shores, roads, bridges) is ported faithfully from
//! `js/map_app.js`'s `isRiver`/`landType`.
//!
//! DETERMINISM: classification + river placement derive purely from the building
//! extent + routed road tiles (sorted / BTree-ordered, no RNG / `Date` /
//! HashMap-iteration order), so the same input reproduces byte-identical terrain.
//!
//! PERFORMANCE: the wire form is SPARSE (only the non-grass water/sand/bridge
//! tiles + the rivers + `sea_x`), never an `n*n` array, so a huge map stays cheap
//! to serialize and the frontend can chunk/cull it. The grass land keeps its
//! existing value-noise ground (the renderer's `terrain.ts`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::polis::model::{Building, Road};
use crate::polis::scanner::{map_extent, GAP};

/// A terrain classification for a single tile. Mirrors the HANDOFF `Terrain`
/// enum. Serialized camelCase like every other Polis enum on the wire (so
/// `Grass` -> `"grass"`, etc.), though the wire form below transmits only the
/// non-grass tiles sparsely — this enum is the in-memory classification result
/// and the test/nav vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Terrain {
    Grass,
    Sand,
    Road,
    Plaza,
    River,
    Sea,
    Bridge,
}

/// A tile coordinate on the (absolute) cartesian grid. Buildings can sit at
/// negative coords (the layout spirals around the origin), so terrain tiles are
/// signed. `Ord`/`Eq` so they live in `BTreeSet` for deterministic ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tile {
    pub gx: i32,
    pub gy: i32,
}

impl Tile {
    #[inline]
    pub fn new(gx: i32, gy: i32) -> Self {
        Self { gx, gy }
    }
}

/// A river's envelope: the bounding column range `[gx_min, gx_max]` that
/// contains the river across ALL rows in the land band. The actual channel
/// meanders gently within this envelope: at each row the channel occupies
/// columns `[ch(gy), ch(gy)+1]` where `ch(gy)` = base + offset(gy) and
/// offset ∈ {-1, 0, +1}. The envelope bounds are `min(ch)` and
/// `max(ch)+1` across all rows.
///
/// The serialized form (`gxMin`/`gxMax` camelCase) is the change signature
/// the frontend uses as an envelope. Nothing else in TS reads rivers.
///
/// `channels` (skipped on the wire) is the precomputed per-row channel-start
/// lookup: `channels[gy - min_y]` = the column where the 2-tile channel
/// begins at row `gy`. Built once during `place_rivers` for O(1) classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct River {
    /// Inclusive min column of the river envelope (over all rows).
    pub gx_min: i32,
    /// Inclusive max column of the river envelope (over all rows).
    pub gx_max: i32,
    /// Precomputed channel-start per row (index = gy - min_y). O(1) lookup.
    /// Not serialized (frontend uses envelope only).
    #[serde(skip)]
    pub channels: Vec<i32>,
}

/// COMPACT, SPARSE terrain payload on `CityState::terrain`. Everything not listed
/// here is `Grass` (drawn by the existing value-noise ground), so a big map only
/// pays for its actual water/shore/bridge frame.
///
/// Coordinate conventions (all in the same absolute cartesian tile space as
/// `Building::coords`):
///   - `sea_x`        : `gx >= sea_x` (within `[min_y, max_y)`) is open `Sea` —
///                       the EAST/seaward margin, aligned with the harbour column
///                       `cloud::place_external_services` builds at `max_x + GAP`.
///   - `min_y/max_y`  : the inclusive-exclusive `y` band the sea spans. `min_y` is
///                       the land's top; `max_y` is the land's bottom EXTENDED down
///                       to cover the harbour column (so every harbour sits on
///                       water). Outside this band there is no sea row.
///   - `rivers`       : the internal river channels (each with land+shore both sides).
///   - `water`        : every `Sea`+`River` tile (so the frontend can draw + animate
///                       water as one pooled set). `deep` flags open-sea tiles
///                       (a tile >= sea_x+1) for the darker water shade.
///   - `sand`         : shore tiles adjacent to sea/river (drawn as sand).
///   - `bridges`      : tiles where a routed road crosses a river (raised walkable
///                       deck; the river still flows underneath — the tile is also
///                       present in `water`).
///
/// The three tile lists are sorted by `(gy, gx)` so the payload is deterministic
/// and the frontend can stream/chunk them in a stable order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerrainData {
    /// `gx >= sea_x` (within the y-band) is open sea.
    pub sea_x: i32,
    /// Inclusive min `y` of the terrain band.
    pub min_y: i32,
    /// Exclusive max `y` of the terrain band.
    pub max_y: i32,
    pub rivers: Vec<River>,
    pub water: Vec<WaterTile>,
    pub sand: Vec<Tile>,
    pub bridges: Vec<Tile>,
}

/// A water tile (sea or river) for the frontend's pooled water layer. `deep`
/// selects the darker open-sea shade (mirrors `deep: gy >= SEA_Y + 1` in the
/// demo, adapted to the +x sea axis).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaterTile {
    pub gx: i32,
    pub gy: i32,
    pub deep: bool,
}

impl TerrainData {
    /// An empty terrain (no water): a city with no buildings has no sea/river.
    pub fn empty() -> Self {
        Self {
            sea_x: 0,
            min_y: 0,
            max_y: 0,
            rivers: Vec::new(),
            water: Vec::new(),
            sand: Vec::new(),
            bridges: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/// Extra tiles of open sea drawn BEYOND `sea_x` so the harbour column (which sits
/// at `max_x + GAP`) is comfortably surrounded by water, not on the very first
/// sea row. The sea is rendered out to `sea_x + SEA_DEPTH`.
const SEA_DEPTH: i32 = GAP as i32 + 4;

// ---------------------------------------------------------------------------
// Meander (deterministic pure function, no RNG)
// ---------------------------------------------------------------------------

/// Deterministic xorshift-style mix of two i32 values, producing a u32.
/// Pure, no RNG state — used only for the river meander offset.
#[inline]
fn meander_hash(a: i32, b: i32) -> u32 {
    let mut x = (a as u32).wrapping_mul(0x9E3779B9) ^ (b as u32).wrapping_mul(0x85EBCA6B);
    x ^= x >> 16;
    x = x.wrapping_mul(0xC2B2AE35);
    x ^= x >> 16;
    x
}

/// Deterministic meander offset for a river at `base` column, row `gy`.
///
/// The river at row `gy` occupies columns `[ch(gy), ch(gy)+1]` where
/// `ch(gy) = base + offset(gy)` and `offset ∈ {-1, 0, +1}`.
///
/// Block structure: rows are grouped into blocks of 4 via Euclidean division
/// (`div_euclid`, which always rounds toward −∞ so blocks partition uniformly
/// for negative `gy` too). The offset at each block is derived from
/// `meander_hash(base, block_index) % 3`, mapped to {-1, 0, +1}.
/// Transitions between consecutive blocks are clamped to ±1:
/// `offset_next = clamp(prev + clamp(target(b) − prev, −1, +1), −1, +1)`.
///
/// Iterative walk anchored at block 0: starts with block 0's target offset,
/// then steps one block at a time toward the target block, applying the same
/// clamp chain at each step. Terminates in |block| ≤ |gy/4| steps. Same
/// result semantics as the recursive definition for positive gy; no stack
/// overflow for any gy.
fn meander_offset(base: i32, gy: i32) -> i32 {
    let target_block = gy.div_euclid(4);
    let raw0 = meander_hash(base, 0) % 3;
    let mut offset = raw0 as i32 - 1; // block 0

    if target_block == 0 {
        return offset;
    }

    // Walk from block 0 toward target_block, one block at a time.
    let step = if target_block > 0 { 1 } else { -1 };
    let mut block = 0i32;
    while block != target_block {
        let next = block + step;
        let raw = meander_hash(base, next) % 3;
        let target = raw as i32 - 1;
        let delta = (target - offset).max(-1).min(1);
        offset = (offset + delta).max(-1).min(1);
        block = next;
    }
    offset
}

/// Channel start column at row `gy` for a river with base column `base`.
#[inline]
fn channel_start(base: i32, gy: i32) -> i32 {
    base + meander_offset(base, gy)
}

/// Straight (no-meander) channel start: always at `base`, offset 0.
#[inline]
fn straight_channel_start(base: i32, _gy: i32) -> i32 {
    base
}

/// Build the sparse terrain frame for a laid-out city.
///
/// `buildings` must already have their final `coords` (post-`layout`), and
/// `roads` their routed `path` (post-`grid::route_roads`) — exactly the state at
/// the `generate_city_state` integration point. Pure + deterministic.
///
/// `n_external_harbours` is the number of cloud-service nodes
/// `cloud::place_external_services` will lay down the seaward column (it anchors at
/// the land's `min_y` and steps down by `cloud::ROW_PITCH`). The sea band is
/// extended downward to cover that full column so EVERY harbour sits on water, not
/// on grass below the land (FIX 3). At scan time the inventory isn't known yet, so
/// the scanner passes `0`; `cloud::attach_external_services` REBUILDS the terrain
/// with the real count once the harbours are placed.
///
/// Steps:
///   1. Land extent from `map_extent` (footprint-aware). Empty city -> empty terrain.
///   2. `sea_x = max_x` (rounded) so harbours at `max_x + GAP` sit on the sea; the
///      sea's y-band is `[min_y, max_y)` where `max_y` is extended past the land
///      bottom to cover the harbour column.
///   3. Deterministically place 1-2 internal river channels between districts (in
///      the LAND band only), 2 tiles wide per row with a gentle deterministic
///      meander, nudged so no channel/shore clips a building footprint.
///   4. Classify every tile in the band, emitting only non-grass tiles sparsely.
///   5. Mark `Bridge` on river tiles that a routed road crosses.
pub fn build_terrain(
    buildings: &[Building],
    roads: &[Road],
    n_external_harbours: usize,
) -> TerrainData {
    let (min_x, min_y_f, max_x, max_y_f) = match map_extent(buildings) {
        Some(e) => e,
        None => return TerrainData::empty(),
    };

    // Integer band. `map_extent` returns `max_*` already past the footprint, so
    // these are exclusive upper bounds; floor/ceil to integer tiles.
    let min_x = min_x.floor() as i32;
    let min_y = min_y_f.floor() as i32;
    let max_x = max_x.ceil() as i32;
    let land_max_y = max_y_f.ceil() as i32;

    // The harbour column (`cloud::place_external_services`) anchors at the land's
    // `min_y` and steps DOWN by `ROW_PITCH` per service, so with enough services
    // the lowest harbour lands BELOW the land's `max_y`. The sea band must cover
    // the FULL harbour extent or those harbours sit on grass, not water (FIX 3).
    let harbour_bottom =
        crate::polis::cloud::harbour_bottom_y(n_external_harbours, min_y_f).ceil() as i32;
    let max_y = if n_external_harbours == 0 {
        land_max_y
    } else {
        land_max_y.max(harbour_bottom + 1)
    };

    // Seaward edge = east extent of the land. `gx >= sea_x` is open sea, so the
    // harbours at `max_x + GAP` (> sea_x) land squarely on the water.
    let sea_x = max_x;
    let sea_max_x = sea_x + SEA_DEPTH;

    // --- building footprint occupancy (so a river never clips a building) ----
    let occ = building_tiles(buildings);

    // --- river placement (deterministic, between districts, off buildings) ---
    // Rivers + their shores live in the LAND band only — never in the harbour
    // extension rows below the land (there is no land there to flow through).
    let rivers = place_rivers(min_x, max_x, min_y, land_max_y, &occ, buildings);

    // --- routed road tiles (rasterized from each road's corner polyline) -----
    let road_tiles = road_tiles(roads);

    // --- classify the band, emit sparse non-grass tiles -----------------------
    let mut water: Vec<WaterTile> = Vec::new();
    let mut sand: BTreeSet<Tile> = BTreeSet::new();
    let mut bridges: BTreeSet<Tile> = BTreeSet::new();

    // The SEA spans the FULL band (`min_y..max_y`), which is extended below the
    // land to cover the harbour column (FIX 3) so every harbour sits on water.
    for gy in min_y..max_y {
        // Sea columns (east margin) for this row.
        for gx in sea_x..sea_max_x {
            water.push(WaterTile {
                gx,
                gy,
                deep: gx > sea_x,
            });
        }
    }

    // Rivers + their sand banks + the beach are LAND-band features only: they
    // require land (and a building footprint) on both sides, which only exists in
    // `[min_y, land_max_y)`. The harbour-extension rows below have no land, so no
    // river/shore/beach is emitted there.
    //
    // Per-river precomputed channel lookup: `channels[gy - min_y]` gives the
    // channel-start column at row `gy`. O(1) per query.
    for gy in min_y..land_max_y {
        // River columns for this row (land band only — `gx < sea_x`).
        for r in &rivers {
            let ch = r.channels[(gy - min_y) as usize];
            for dx in 0..2 {
                let gx = ch + dx;
                if gx >= sea_x {
                    continue; // a river never overwrites open sea
                }
                let tile = Tile::new(gx, gy);
                let is_bridge = road_tiles.contains(&tile);
                water.push(WaterTile {
                    gx,
                    gy,
                    deep: false,
                });
                if is_bridge {
                    bridges.insert(tile);
                }
            }
        }

        // Sand banks: every land tile in the band that is 4-adjacent to at least
        // one river tile and is not a building footprint tile. This replaces the
        // fixed gx±1 bank logic and handles meander elbows correctly.
        for r in &rivers {
            let ch = r.channels[(gy - min_y) as usize];
            for dx in 0..2 {
                let rx = ch + dx;
                if rx >= sea_x {
                    continue;
                }
                // 4-neighbours of this river tile
                for &(nx, ny) in &[(rx - 1, gy), (rx + 1, gy), (rx, gy - 1), (rx, gy + 1)] {
                    if ny < min_y || ny >= land_max_y {
                        continue;
                    }
                    if nx >= sea_x || nx < min_x {
                        continue;
                    }
                    let adj = Tile::new(nx, ny);
                    // Not a river tile itself, not under a building footprint
                    if !is_river_tile(&rivers, nx, ny, sea_x, min_y) && !occ.contains(&adj) {
                        sand.insert(adj);
                    }
                }
            }
        }

        // Beach: the tile just west of the sea edge. NEVER emit sand on a tile a
        // building footprint occupies — an east-edge building can sit on column
        // `sea_x-1`, and a Sand tile under it would break Phase C walkability
        // (the building would float over a beach). Mirror the river-bank land
        // guarantee with an explicit footprint check here.
        let beach = sea_x - 1;
        let beach_tile = Tile::new(beach, gy);
        if beach >= min_x
            && !is_river_col(&rivers, beach)
            && !occ.contains(&beach_tile)
        {
            sand.insert(beach_tile);
        }
    }

    // Deterministic, stable order for the wire — ALL THREE tile lists sorted by
    // `(gy, gx)` (FIX 5: the `BTreeSet` gave sand/bridges `(gx, gy)` via `Tile`'s
    // Ord, inconsistent with `water`; sort them the same way for one clean,
    // row-major payload the frontend can stream/chunk uniformly).
    water.sort_by_key(|a| (a.gy, a.gx));
    let mut sand: Vec<Tile> = sand.into_iter().collect();
    sand.sort_by_key(|a| (a.gy, a.gx));
    let mut bridges: Vec<Tile> = bridges.into_iter().collect();
    bridges.sort_by_key(|a| (a.gy, a.gx));

    TerrainData {
        sea_x,
        min_y,
        max_y,
        rivers,
        water,
        sand,
        bridges,
    }
}

/// Classify a SINGLE tile given the terrain inputs — the HANDOFF rules 1-6 in
/// priority order, adapted to the +x seaward axis. `road` = is this tile under a
/// routed road; used to derive `Road`/`Bridge`. Pure; the canonical reference
/// for the classification tests.
///
/// Priority (highest first):
///   1. `gx >= sea_x` (within band) -> `Sea`.
///   2. river tile and `gx < sea_x` -> `River` (or `Bridge` if a road crosses).
///   3. road tile -> `Road`.
///   4. adjacent to sea/river -> `Sand` (shore).
///   5. else -> `Grass`.
#[allow(clippy::too_many_arguments)]
pub fn classify(
    gx: i32,
    gy: i32,
    min_x: i32,
    min_y: i32,
    max_y: i32,
    sea_x: i32,
    rivers: &[River],
    is_road: bool,
) -> Terrain {
    // Rule 1 — open sea on the east margin (only within the terrain y-band).
    if gy >= min_y && gy < max_y && gx >= sea_x {
        return Terrain::Sea;
    }
    // Rule 2 — internal river tile (lands strictly west of the sea edge).
    if gx < sea_x && is_river_tile(rivers, gx, gy, sea_x, min_y) {
        // Rule 6 — a road crossing the channel is a raised, walkable Bridge over
        // the water (the tile is still water underneath).
        return if is_road {
            Terrain::Bridge
        } else {
            Terrain::River
        };
    }
    // Rule 3 — routed road / plaza (no plaza concept in this layout, so Road only).
    if is_road {
        return Terrain::Road;
    }
    // Rule 4 — shore sand: immediately west of the sea, or a river bank (row-aware
    // for the meandering 2-wide channel).
    let on_beach = gx == sea_x - 1;
    let on_bank = is_river_tile(rivers, gx - 1, gy, sea_x, min_y)
        || is_river_tile(rivers, gx + 1, gy, sea_x, min_y);
    if gx >= min_x && (on_beach || on_bank) {
        return Terrain::Sand;
    }
    // Rule 5 — chora.
    Terrain::Grass
}

/// Is `(gx, gy)` inside any river channel at row `gy`? Row-aware for the
/// 2-tile-wide meandering channel: at row `gy` the river occupies columns
/// `[ch(gy), ch(gy)+1]` where `ch(gy)` comes from the precomputed `River::channels`.
///
/// When `channels` is empty (e.g. a River deserialized from the wire where
/// `#[serde(skip)]` leaves channels vacant), we fall back to the envelope
/// over-approximation (`gx_min ≤ gx ≤ gx_max`).  This is *safe* — it may
/// classify a non-river tile as River, never the reverse — and it avoids
/// recomputing a meander from a wrong base.
///
/// Returns false if `gx >= sea_x` (beyond the seaward edge, where tiles are Sea).
#[inline]
fn is_river_tile(rivers: &[River], gx: i32, gy: i32, sea_x: i32, min_y: i32) -> bool {
    if gx >= sea_x {
        return false;
    }
    rivers.iter().any(|r| {
        if r.channels.is_empty() {
            // Defensive fallback: envelope over-approximation.
            gx >= r.gx_min && gx <= r.gx_max
        } else {
            if gy < min_y {
                return false;
            }
            let idx = (gy - min_y) as usize;
            if idx >= r.channels.len() {
                return false;
            }
            let ch = r.channels[idx];
            gx >= ch && gx < ch + 2
        }
    })
}

/// Is `gx` inside any river channel column range? (envelope-only, used for the
/// beach check where row-level precision is not needed since the beach is a
/// single column that is always at or beyond the envelope boundary).
#[inline]
fn is_river_col(rivers: &[River], gx: i32) -> bool {
    rivers.iter().any(|r| gx >= r.gx_min && gx <= r.gx_max)
}

/// All tiles occupied by a building footprint, in absolute tile space.
/// `[coords.x, coords.x + W) x [coords.y, coords.y + D)` (see `footprint.rs`).
fn building_tiles(buildings: &[Building]) -> BTreeSet<Tile> {
    let mut occ = BTreeSet::new();
    for b in buildings {
        let (fw, fd) = crate::polis::footprint::building_footprint(&b.purpose, &b.visual_tier);
        let x0 = b.coords.x.round() as i32;
        let y0 = b.coords.y.round() as i32;
        for dx in 0..fw as i32 {
            for dy in 0..fd as i32 {
                occ.insert(Tile::new(x0 + dx, y0 + dy));
            }
        }
    }
    occ
}

/// Rasterize every routed road's corner polyline into the set of tiles it covers.
/// Each `Road::path` is a sequence of corner waypoints of axis-aligned
/// (4-connected) runs (see `grid::route_roads`/`simplify`); we walk each segment
/// stepping by the sign of the single changing axis. Roads with no `path` (a
/// straight-line fallback) contribute no tiles (they are abstract from->to lines
/// the renderer draws straight; they don't carve terrain).
fn road_tiles(roads: &[Road]) -> BTreeSet<Tile> {
    let mut tiles = BTreeSet::new();
    for r in roads {
        let Some(path) = &r.path else { continue };
        if path.is_empty() {
            continue;
        }
        let mut prev = Tile::new(path[0].x.round() as i32, path[0].y.round() as i32);
        tiles.insert(prev);
        for p in &path[1..] {
            let cur = Tile::new(p.x.round() as i32, p.y.round() as i32);
            // Walk the (axis-aligned) segment prev -> cur.
            let mut x = prev.gx;
            let mut y = prev.gy;
            let dx = (cur.gx - x).signum();
            let dy = (cur.gy - y).signum();
            // Defensive: a non-axis-aligned segment (shouldn't happen) still
            // terminates — step diagonally until both axes match.
            while x != cur.gx || y != cur.gy {
                if x != cur.gx {
                    x += dx;
                }
                if y != cur.gy {
                    y += dy;
                }
                tiles.insert(Tile::new(x, y));
            }
            prev = cur;
        }
    }
    tiles
}

/// Deterministically place 1-2 internal river channels between the city's
/// districts, INSIDE the land band (`min_x < channel < sea_x`), nudged so neither
/// the channel NOR its shore banks clip a building footprint.
///
/// Each river is 2 tiles wide per row with a gentle deterministic meander:
/// at row `gy` the river occupies `[ch(gy), ch(gy)+1]` where `ch(gy) = base +
/// offset(gy)` and offset ∈ {-1, 0, +1}. The meander is derived from a pure
/// hash of `(base, gy / 4)` — no RNG, no HashMap iteration order.
///
/// The `River` struct is the envelope: `gx_min = min(ch)` and
/// `gx_max = max(ch)+1` across all rows in the band. Precomputed per-row
/// channel data is stored in `River::channels` for O(1) lookup during
/// classification.
///
/// Placement is derived purely from the land extent (no RNG): the land width is
/// split into thirds; a river is proposed at the 1/3 and 2/3 columns. A river is
/// only kept if the band is wide enough to host it AND a clear lane (channel +
/// both banks free of buildings, and envelope separation from other rivers) can
/// be found by nudging the proposal. A second river is only added when the band
/// is wide enough to keep the two channels from touching.
fn place_rivers(
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
    occ: &BTreeSet<Tile>,
    buildings: &[Building],
) -> Vec<River> {
    let width = max_x - min_x;
    // Too narrow to host an internal river with land+shore on both sides.
    // Need at least: bank | channel | channel | bank, plus land beyond each side.
    if width < 9 || max_y <= min_y {
        return Vec::new();
    }

    let propose = |frac_num: i32, frac_den: i32| -> i32 { min_x + (width * frac_num) / frac_den };

    // Water-affine river placement: prefer flowing near harbor/lighthouse districts.
    // Port anchors are the sorted gx centers of harbor/lighthouse buildings.
    // When anchors exist, river base candidates target those columns instead of
    // the geometric midpoint/thirds, so the river passes next to port districts.
    let port_anchors: Vec<i32> = {
        let mut gxs: Vec<i32> = buildings
            .iter()
            .filter(|b| b.purpose == "harbor" || b.purpose == "lighthouse")
            .map(|b| b.coords.x.round() as i32)
            .collect();
        gxs.sort_unstable();
        gxs.dedup();
        gxs
    };

    // Build candidate base columns. The number of candidates matches the slot
    // count (1 when width<18, 2 when width>=18). When port anchors exist, each
    // candidate targets the anchor nearest the corresponding positional slot
    // (median for 1 slot; 1/3 and 2/3 for 2 slots). When both slots resolve to
    // the same anchor (a single mid-map cluster), the anchor fills the slot it
    // is closest to and the other slot gets the original geometric thirds column
    // — preserving the 2-river count for wide cities. Without anchors, fall back
    // to the geometric midpoint/thirds — preserving original behavior verbatim.
    let num_slots = if width >= 18 { 2 } else { 1 };
    let candidates: Vec<i32> = if port_anchors.is_empty() {
        // No harbor/lighthouse buildings: geometric midpoint/thirds (original).
        if width >= 18 {
            vec![propose(1, 3), propose(2, 3)]
        } else {
            vec![propose(1, 2)]
        }
    } else if num_slots == 1 {
        // One slot: the median anchor (middle element, or single element).
        vec![port_anchors[port_anchors.len() / 2]]
    } else {
        // Two slots: the anchors nearest the 1/3 and 2/3 positional targets.
        // Targets in gx space: the geometric thirds of the land band.
        let target_third = min_x + width / 3;
        let target_two_thirds = min_x + (width * 2) / 3;
        // Find the anchor nearest each target. Ties broken by smaller gx
        // (deterministic, i64 cast so negative anchors lose correctly).
        let find_nearest = |target: i32| -> i32 {
            *port_anchors
                .iter()
                .min_by_key(|&&gx| {
                    let dist = (gx - target).unsigned_abs() as u64;
                    // Tie-break: smaller gx wins (deterministic).
                    (dist, gx as i64)
                })
                .unwrap()
        };
        let a = find_nearest(target_third);
        let b = find_nearest(target_two_thirds);
        if a == b {
            // Same anchor is nearest both thirds. Give it to the slot it is
            // CLOSEST to and fill the other slot with the original geometric
            // candidate (the plain thirds column) so we still emit 2 rivers
            // when the width allows it — a city with harbors must not silently
            // lose a river vs a harbor-less city of the same width.
            let dist_to_third = (a - target_third).unsigned_abs();
            let dist_to_two_thirds = (a - target_two_thirds).unsigned_abs();
            let mut candidates = Vec::with_capacity(2);
            if dist_to_third <= dist_to_two_thirds {
                // Anchor takes the 1/3 slot; 2/3 slot gets the geometric column.
                candidates.push(a);
                let geometric_2_3 = propose(2, 3);
                if geometric_2_3 != a {
                    candidates.push(geometric_2_3);
                }
            } else {
                // Anchor takes the 2/3 slot; 1/3 slot gets the geometric column.
                let geometric_1_3 = propose(1, 3);
                if geometric_1_3 != a {
                    candidates.push(geometric_1_3);
                }
                candidates.push(a);
            }
            candidates
        } else {
            // Deterministic order: smaller gx first.
            if a < b { vec![a, b] } else { vec![b, a] }
        }
    };

    // Try with meander first (gentle offset ±1). If no candidate finds a clear
    // lane, fall back to a straight channel (offset always 0) which needs exactly
    // 4 free columns — fitting corridors the meander cannot.
    // NOTE: the candidate list now derives from port anchors when available, so
    // the river naturally flows near harbor/lighthouse districts.
    for &channel_fn in &[
        channel_start as fn(i32, i32) -> i32,
        straight_channel_start as fn(i32, i32) -> i32,
    ] {
        let mut rivers: Vec<River> = Vec::new();

        for &cand in &candidates {
            if let Some(base) = clear_column(
                cand,
                min_x + 2,
                max_x - 2,
                min_y,
                max_y,
                occ,
                &rivers,
                channel_fn,
            ) {
                // Precompute per-row channel-start and the envelope.
                let (gx_min, gx_max) = channel_envelope(base, min_y, max_y, channel_fn);
                let band_height = (max_y - min_y) as usize;
                let mut channels = Vec::with_capacity(band_height);
                for gy in min_y..max_y {
                    channels.push(channel_fn(base, gy));
                }
                rivers.push(River {
                    gx_min,
                    gx_max,
                    channels,
                });
            }
        }

        if !rivers.is_empty() {
            return rivers;
        }
    }

    Vec::new()
}

/// Compute the actual envelope `[gx_min, gx_max]` for a river at `base` using
/// the supplied `channel_fn`.  Used by both `clear_column` (overlap check) and
/// `place_rivers` (storage) so they can never diverge.
#[inline]
fn channel_envelope(
    base: i32,
    min_y: i32,
    max_y: i32,
    channel_fn: fn(i32, i32) -> i32,
) -> (i32, i32) {
    let mut gx_min = i32::MAX;
    let mut gx_max = i32::MIN;
    for gy in min_y..max_y {
        let ch = channel_fn(base, gy);
        gx_min = gx_min.min(ch);
        gx_max = gx_max.max(ch + 1); // channel occupies [ch, ch+1]
    }
    (gx_min, gx_max)
}

/// Find a channel base column near `proposed`, within `[lo, hi]`, whose
/// 2-tile-wide channel AND both shore banks are free of building footprints for
/// the whole `y`-band, and whose ACTUAL envelope (computed from the per-row
/// channel positions) is separated from every already-placed river by at least
/// 3 columns.  This guarantees at least one land/sand column between rivers on
/// every row, even at worst-case meander.
///
/// Searches outward from `proposed` by increasing offset (bounded), so the
/// choice is deterministic.  Returns `None` if no clear column exists.
fn clear_column(
    proposed: i32,
    lo: i32,
    hi: i32,
    min_y: i32,
    max_y: i32,
    occ: &BTreeSet<Tile>,
    existing: &[River],
    channel_fn: fn(i32, i32) -> i32,
) -> Option<i32> {
    if lo > hi {
        return None;
    }
    let max_offset = (hi - lo).max(0);
    for off in 0..=max_offset {
        // Try +off then -off (deterministic, prefers the proposed/east side).
        for &base in &[proposed + off, proposed - off] {
            if base < lo || base > hi {
                continue;
            }
            if !channel_clear(base, min_y, max_y, occ, channel_fn) {
                continue;
            }
            let (env_min, env_max) = channel_envelope(base, min_y, max_y, channel_fn);
            // Require ≥3 columns of gap between envelopes on every row.  Reject
            // if the envelopes overlap or are within 3 columns of each other:
            //   env_min ≤ r.gx_max+3  AND  r.gx_min ≤ env_max+3
            if existing.iter().any(|r| {
                env_min <= r.gx_max + 3 && r.gx_min <= env_max + 3
            }) {
                continue;
            }
            return Some(base);
        }
    }
    None
}

/// Are the 2-tile-wide channel AND its two immediate bank tiles (ch-1 and ch+2)
/// free of building footprints for every row in the `y`-band? Uses the supplied
/// `channel_fn` to compute the actual per-row channel start (meander-aware or
/// straight), so only the tiles the river REALLY occupies are checked — not the
/// worst-case envelope.
fn channel_clear(
    base: i32,
    min_y: i32,
    max_y: i32,
    occ: &BTreeSet<Tile>,
    channel_fn: fn(i32, i32) -> i32,
) -> bool {
    for gy in min_y..max_y {
        let ch = channel_fn(base, gy);
        for gx in (ch - 1)..=(ch + 2) {
            if occ.contains(&Tile::new(gx, gy)) {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::polis::model::{
        building_status, purpose, purpose_source, road_style, road_type, visual_tier, Coords,
    };

    fn bld(file_id: &str, purpose: &str, tier: &str, x: f64, y: f64) -> Building {
        Building {
            file_id: file_id.into(),
            file_path: format!("src/{file_id}.rs"),
            district_id: "d".into(),
            purpose: purpose.into(),
            purpose_source: purpose_source::DEFAULT.into(),
            feature_id: "f".into(),
            feature_source: "directory".into(),
            provider: None,
            lines_of_code: 50,
            visual_tier: tier.into(),
            coords: Coords::new(x, y),
            status: building_status::NORMAL.into(),
            label: file_id.into(),
            description: String::new(),
            last_modified: String::new(),
            agent_present: None,
            suspect_of_card_id: None,
            kanban_card_id: None,
            untracked_change: None,
            sins: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn road_with_path(id: &str, path: Vec<(i32, i32)>) -> Road {
        Road {
            road_id: id.into(),
            from: "a".into(),
            to: "b".into(),
            road_type: road_type::IMPORT.into(),
            style: road_style::TERRA_BATTUTA.into(),
            weight: 1,
            path: Some(
                path.into_iter()
                    .map(|(x, y)| Coords::new(x as f64, y as f64))
                    .collect(),
            ),
            provenance: None,
        }
    }

    /// A spread of small houses across a wide band so the river placement and sea
    /// margin have room to work.
    fn wide_city() -> Vec<Building> {
        // Two clusters of houses on the left (x=0,3,5) and right (x=17,19,22)
        // leaving a wide corridor (x=6..=16) for a river.
        let x_positions = [0.0, 3.0, 5.0, 17.0, 19.0, 22.0];
        let mut v = Vec::new();
        for (i, &x) in x_positions.iter().enumerate() {
            for (j, y) in [0.0, 4.0, 8.0].iter().enumerate() {
                v.push(bld(
                    &format!("b{i}-{j}"),
                    purpose::HOUSE,
                    visual_tier::KALYBE,
                    x,
                    *y,
                ));
            }
        }
        v
    }

    #[test]
    fn empty_city_has_no_terrain() {
        let t = build_terrain(&[], &[], 0);
        assert_eq!(t, TerrainData::empty());
        assert!(t.water.is_empty());
        assert!(t.rivers.is_empty());
    }

    // Rule 1 + sea-on-harbour-margin alignment.
    #[test]
    fn sea_is_on_the_eastern_harbour_margin() {
        let buildings = wide_city();
        let (min_x_f, _min_y, max_x, _max_y) = map_extent(&buildings).unwrap();
        let min_x = min_x_f.floor() as i32;
        let t = build_terrain(&buildings, &[], 0);

        // The sea edge equals the land's east extent (rounded), so the harbour
        // column `cloud::place_external_services` builds at `max_x + GAP` sits
        // EAST of `sea_x` => on the water.
        assert_eq!(t.sea_x, max_x.ceil() as i32, "sea edge = east land extent");
        let harbour_x = (max_x + GAP as f64) as i32;
        assert!(
            harbour_x >= t.sea_x,
            "harbour column x={harbour_x} must sit on/east of sea_x={}",
            t.sea_x
        );
        // classify a harbour tile -> Sea. Pass the REAL `min_x` (FIX 5): the prior
        // `t.sea_x.min(0)` arg was wrong and only passed by luck (Rule 1 — sea —
        // fires before any `min_x`-gated rule, so the bad arg never mattered).
        let c = classify(
            harbour_x, t.min_y, min_x, t.min_y, t.max_y, t.sea_x, &t.rivers, false,
        );
        assert_eq!(c, Terrain::Sea, "the harbour tile classifies as Sea");
        // The water set actually contains sea tiles at >= sea_x.
        assert!(
            t.water.iter().any(|w| w.gx >= t.sea_x),
            "water set has open-sea tiles on the east margin"
        );
    }

    // Rule 2 + "rivers internal with shore on BOTH sides, flowing to sea".
    // Updated: banks exist on both sides row-aware for the meandering 2-wide channel.
    #[test]
    fn rivers_are_internal_with_land_and_shore_on_both_sides() {
        let buildings = wide_city();
        let (min_x_f, _min_y, _max_x_f, _max_y) = map_extent(&buildings).unwrap();
        let min_x = min_x_f.floor() as i32;
        let t = build_terrain(&buildings, &[], 0);

        assert!(!t.rivers.is_empty(), "a wide city has >= 1 internal river");
        for r in &t.rivers {
            // Internal: strictly inside the land band, not on the map edge, and
            // west of the sea.
            assert!(r.gx_min > min_x, "river not on the west map edge");
            assert!(r.gx_max < t.sea_x, "river west of the sea (flows into it)");
            // Land on both sides: the bank columns exist within the band.
            assert!(r.gx_min > min_x, "left bank is inside the band (land)");
            assert!(
                r.gx_max + 1 < t.sea_x,
                "right bank is inside the band (land)"
            );
        }

        // The river spans the whole y-band (flows from the north edge into the
        // sea band), and its banks are classified Sand — row-aware for meander.
        let r = &t.rivers[0];
        let chan_end = t.min_y + r.channels.len() as i32;
        for gy in t.min_y..chan_end {
            let ch = r.channels[(gy - t.min_y) as usize];
            // Both channel columns are River
            assert_eq!(
                classify(ch, gy, min_x, t.min_y, t.max_y, t.sea_x, &t.rivers, false),
                Terrain::River,
                "channel col 0 is River at gy={gy}"
            );
            assert_eq!(
                classify(ch + 1, gy, min_x, t.min_y, t.max_y, t.sea_x, &t.rivers, false),
                Terrain::River,
                "channel col 1 is River at gy={gy}"
            );
            // Left bank (ch-1) is Sand
            assert_eq!(
                classify(
                    ch - 1,
                    gy,
                    min_x,
                    t.min_y,
                    t.max_y,
                    t.sea_x,
                    &t.rivers,
                    false
                ),
                Terrain::Sand,
                "left bank is Sand at gy={gy}"
            );
            // Right bank (ch+2) is Sand
            assert_eq!(
                classify(
                    ch + 2,
                    gy,
                    min_x,
                    t.min_y,
                    t.max_y,
                    t.sea_x,
                    &t.rivers,
                    false
                ),
                Terrain::Sand,
                "right bank is Sand at gy={gy}"
            );
        }
        // Sand set emitted on both banks (at least one tile at the envelope
        // boundary on each side — the actual per-row bank positions depend on
        // the meander).
        assert!(
            t.sand.iter().any(|s| s.gx == r.gx_min - 1),
            "left bank sand exists at envelope boundary"
        );
        assert!(
            t.sand.iter().any(|s| s.gx == r.gx_max + 1),
            "right bank sand exists at envelope boundary"
        );
    }

    // Rule 6 — bridge only where a ROAD crosses a RIVER tile.
    // Updated: a road crossing the 2-wide river yields bridge tiles covering the
    // FULL channel width at the crossing row(s), and the path stays walkable.
    #[test]
    fn bridge_marked_only_where_a_road_crosses_a_river() {
        let buildings = wide_city();
        let t0 = build_terrain(&buildings, &[], 0);
        let r = &t0.rivers[0];
        let cross_y = t0.min_y + 1;
        let ch = r.channels[(cross_y - t0.min_y) as usize];

        // A road running east across the full river channel at row cross_y.
        let road = road_with_path("r0", vec![(ch - 2, cross_y), (ch + 3, cross_y)]);
        let t = build_terrain(&buildings, &[road], 0);

        // Both channel tiles at the crossing row are bridges.
        let crossing0 = Tile::new(ch, cross_y);
        let crossing1 = Tile::new(ch + 1, cross_y);
        assert!(
            t.bridges.contains(&crossing0),
            "road crossing the channel marks a Bridge at {crossing0:?}"
        );
        assert!(
            t.bridges.contains(&crossing1),
            "road crossing the channel marks a Bridge at {crossing1:?} (full width)"
        );
        // classify confirms Bridge at both crossing tiles (road over river) ...
        let min_x = map_extent(&buildings).unwrap().0.floor() as i32;
        assert_eq!(
            classify(ch, cross_y, min_x, t.min_y, t.max_y, t.sea_x, &t.rivers, true),
            Terrain::Bridge,
        );
        assert_eq!(
            classify(ch + 1, cross_y, min_x, t.min_y, t.max_y, t.sea_x, &t.rivers, true),
            Terrain::Bridge,
        );
        // ... and a NON-river road tile is just a Road, never a Bridge.
        let land_road = Tile::new(ch - 2, cross_y);
        assert!(
            !t.bridges.contains(&land_road),
            "land road tile is not a bridge"
        );
        assert_eq!(
            classify(
                ch - 2,
                cross_y,
                min_x,
                t.min_y,
                t.max_y,
                t.sea_x,
                &t.rivers,
                true
            ),
            Terrain::Road,
        );
        // A river tile with NO road over it stays River (no spurious bridge).
        assert!(
            !t.bridges.contains(&Tile::new(ch, t.min_y)),
            "a river tile far from the road is not a bridge"
        );
    }

    // Every row in the band has exactly 2 river columns per river; meander
    // constraints hold; envelope within [base-1, base+2]; river tiles 4-connected.
    #[test]
    fn rivers_are_two_wide_and_meander_gently() {
        let buildings = wide_city();
        let t = build_terrain(&buildings, &[], 0);

        assert!(!t.rivers.is_empty());
        for r in &t.rivers {
            // Envelope: gx_min/gx_max must be consistent with the precomputed
            // channels (not reliant on a recovered base, which is ambiguous with
            // meander).
            let band_height = (t.max_y - t.min_y) as usize;
            assert_eq!(
                r.channels.len(),
                band_height,
                "precomputed channels must cover the full band"
            );
            let computed_min = r.channels.iter().copied().min().unwrap();
            let computed_max = r.channels.iter().copied().max().unwrap() + 1; // [ch, ch+1]
            assert_eq!(r.gx_min, computed_min, "gx_min must match min(ch)");
            assert_eq!(r.gx_max, computed_max, "gx_max must match max(ch)+1");

            let mut prev_ch: Option<i32> = None;
            let mut max_ch_run = 0i32;
            let mut ch_run_len = 0i32;
            let mut prev_ch_val = None;

            // Bound by channels.len() — with n_harbours > 0, max_y may extend
            // past the land band but channels only cover the original band.
            let chan_end = t.min_y + r.channels.len() as i32;
            for gy in t.min_y..chan_end {
                let ch = r.channels[(gy - t.min_y) as usize];

                // Exactly 2 river columns at this row
                assert!(
                    is_river_tile(&t.rivers, ch, gy, t.sea_x, t.min_y),
                    "ch={} is river at gy={}",
                    ch,
                    gy
                );
                assert!(
                    is_river_tile(&t.rivers, ch + 1, gy, t.sea_x, t.min_y),
                    "ch+1={} is river at gy={}",
                    ch + 1,
                    gy
                );
                // Columns outside the 2-wide channel are NOT river
                assert!(
                    !is_river_tile(&t.rivers, ch - 1, gy, t.sea_x, t.min_y),
                    "ch-1={} must NOT be river at gy={}",
                    ch - 1,
                    gy
                );
                assert!(
                    !is_river_tile(&t.rivers, ch + 2, gy, t.sea_x, t.min_y),
                    "ch+2={} must NOT be river at gy={}",
                    ch + 2,
                    gy
                );

                // 4-connectivity: consecutive rows' channels overlap by ≥1 column
                if let Some(prev) = prev_ch {
                    let overlap = (ch + 2).min(prev + 2) - ch.max(prev);
                    assert!(
                        overlap >= 1,
                        "rows must overlap by ≥1 col: ch(gy-1)={} ch(gy)={}",
                        prev,
                        ch
                    );
                    // |ch(gy+1) - ch(gy)| ≤ 1
                    assert!(
                        (ch - prev).abs() <= 1,
                        "channel shift must be ≤1: prev={} curr={}",
                        prev,
                        ch
                    );
                }
                prev_ch = Some(ch);

                // Channel persists for at least 4 consecutive rows (block length)
                match prev_ch_val {
                    Some(prev_val) if prev_val == ch => {
                        ch_run_len += 1;
                    }
                    _ => {
                        ch_run_len = 1;
                    }
                }
                max_ch_run = max_ch_run.max(ch_run_len);
                prev_ch_val = Some(ch);
            }

            // At least one run must be ≥ 4 (the meander block structure
            // guarantees each offset persists for exactly 4 rows within a
            // complete block; the final partial block may be shorter).
            assert!(
                max_ch_run >= 4,
                "some channel must persist ≥4 rows; max run = {}",
                max_ch_run
            );
        }
    }

    // Sand hugs every meander elbow — every land tile adjacent to river is
    // sand, none under footprints.
    #[test]
    fn sand_hugs_every_meander_elbow() {
        let buildings = wide_city();
        let t = build_terrain(&buildings, &[], 0);
        let occ = building_tiles(&buildings);

        // Compute the land min_x so the bank-tile check is in-band.
        let land_min_x = map_extent(&buildings)
            .map(|(mn, _, _, _)| mn.floor() as i32)
            .unwrap_or(0);
        let mut bank_assertions = 0u32;
        for r in &t.rivers {
            let chan_end = t.min_y + r.channels.len() as i32;
            for gy in t.min_y..chan_end {
                let ch = r.channels[(gy - t.min_y) as usize];
                // Left bank
                let left = Tile::new(ch - 1, gy);
                if left.gx >= land_min_x && left.gx < t.sea_x && !occ.contains(&left) {
                    assert!(
                        t.sand.contains(&left),
                        "left bank tile {left:?} must be sand at gy={gy}"
                    );
                    bank_assertions += 1;
                }
                // Right bank
                let right = Tile::new(ch + 2, gy);
                if right.gx >= land_min_x && right.gx < t.sea_x && !occ.contains(&right) {
                    assert!(
                        t.sand.contains(&right),
                        "right bank tile {right:?} must be sand at gy={gy}"
                    );
                    bank_assertions += 1;
                }
            }
        }
        assert!(
            bank_assertions > 0,
            "the sand bank assertion must fire for at least one tile"
        );
        // No sand tile is under a building footprint
        for s in &t.sand {
            assert!(
                !occ.contains(s),
                "sand tile {s:?} must not be under a building footprint"
            );
        }
        // No sand tile is a river or sea tile
        for s in &t.sand {
            assert!(
                !is_river_tile(&t.rivers, s.gx, s.gy, t.sea_x, t.min_y),
                "sand tile {s:?} must not be a river tile"
            );
            assert!(
                s.gx < t.sea_x,
                "sand tile {s:?} must not be a sea tile"
            );
        }
    }

    // No building footprint ever lands on Sea or River — and (FIX 2) the EMITTED
    // sand set never paves a building footprint tile either.
    #[test]
    fn no_building_tile_is_water() {
        let buildings = wide_city();
        let t = build_terrain(&buildings, &[], 0);
        // sea_x = max building x + 1 (1x1 footprint). Rightmost building
        // is at x=22, so sea_x = 23.
        assert_eq!(
            t.sea_x, 23,
            "east-edge house sits on the beach column sea_x-1"
        );

        let water: BTreeSet<Tile> = t.water.iter().map(|w| Tile::new(w.gx, w.gy)).collect();
        let sand: BTreeSet<Tile> = t.sand.iter().copied().collect();

        let mut east_edge_on_beach = false;
        for b in &buildings {
            let (fw, fd) = crate::polis::footprint::building_footprint(&b.purpose, &b.visual_tier);
            let x0 = b.coords.x.round() as i32;
            let y0 = b.coords.y.round() as i32;
            for dx in 0..fw as i32 {
                for dy in 0..fd as i32 {
                    let tile = Tile::new(x0 + dx, y0 + dy);
                    assert!(
                        !water.contains(&tile),
                        "building {} tile {tile:?} must not be water",
                        b.file_id
                    );
                    assert!(
                        !sand.contains(&tile),
                        "building {} tile {tile:?} must not be Sand (breaks Phase C walkability)",
                        b.file_id
                    );
                    if tile.gx == t.sea_x - 1 {
                        east_edge_on_beach = true;
                    }
                }
            }
        }
        assert!(
            east_edge_on_beach,
            "the test city must put a building on the beach column sea_x-1 to exercise FIX 2"
        );
    }

    // Determinism — terrain is byte-identical for the same input AND independent
    // of the ORDER of the buildings/roads slices.
    #[test]
    fn terrain_is_deterministic() {
        let buildings = wide_city();
        let roads = vec![
            road_with_path("r0", vec![(0, 1), (24, 1)]),
            road_with_path("r1", vec![(2, 0), (2, 8)]),
        ];
        let a = build_terrain(&buildings, &roads, 5);

        let mut buildings_rev = buildings.clone();
        buildings_rev.reverse();
        let mut roads_rev = roads.clone();
        roads_rev.reverse();
        let b = build_terrain(&buildings_rev, &roads_rev, 5);

        assert_eq!(
            a, b,
            "terrain must be byte-identical regardless of building/road input order"
        );
        let ja = serde_json::to_string(&a).unwrap();
        let jb = serde_json::to_string(&b).unwrap();
        assert_eq!(ja, jb);
    }

    // FIX 3 — the sea band covers the FULL harbour column.
    #[test]
    fn sea_band_covers_the_full_harbour_column() {
        use crate::polis::cloud;

        let buildings = wide_city();
        let (_min_x, min_y_f, max_x, max_y_f) = map_extent(&buildings).unwrap();
        let land_max_y = max_y_f.ceil() as i32;

        let n: usize = 30;
        let t = build_terrain(&buildings, &[], n);

        let bottom_y = cloud::harbour_bottom_y(n, min_y_f).ceil() as i32;
        assert!(
            bottom_y >= land_max_y,
            "the {n}-harbour column must extend below the land bottom for this test \
             (bottom_y={bottom_y}, land_max_y={land_max_y})"
        );
        assert!(
            t.max_y > bottom_y,
            "sea band max_y={} must cover the lowest harbour row gy={bottom_y}",
            t.max_y
        );

        let harbour_x = (max_x + GAP as f64) as i32;
        let water: BTreeSet<Tile> = t.water.iter().map(|w| Tile::new(w.gx, w.gy)).collect();
        for i in 0..n {
            let gy = cloud::harbour_bottom_y(i + 1, min_y_f).ceil() as i32;
            let tile = Tile::new(harbour_x, gy);
            assert!(
                water.contains(&tile),
                "harbour {i} at {tile:?} must be a water tile"
            );
            assert_eq!(
                classify(harbour_x, gy, 0, t.min_y, t.max_y, t.sea_x, &t.rivers, false),
                Terrain::Sea,
                "harbour {i} at {tile:?} must classify as Sea"
            );
        }

        for gy in land_max_y..t.max_y {
            assert!(
                !t.sand.iter().any(|s| s.gy == gy),
                "no sand emitted in the harbour-extension row gy={gy}"
            );
            assert!(
                t.water
                    .iter()
                    .filter(|w| w.gy == gy)
                    .all(|w| w.gx >= t.sea_x),
                "extension row gy={gy} carries only sea (gx >= sea_x), no river water"
            );
        }
    }

    // A narrow city gets no river (can't host land+shore both sides) but still a sea.
    #[test]
    fn narrow_city_has_sea_but_no_river() {
        let buildings = vec![
            bld("a", purpose::HOUSE, visual_tier::KALYBE, 0.0, 0.0),
            bld("b", purpose::HOUSE, visual_tier::KALYBE, 1.0, 1.0),
        ];
        let t = build_terrain(&buildings, &[], 0);
        assert!(
            t.rivers.is_empty(),
            "too-narrow band hosts no internal river"
        );
        assert!(!t.water.is_empty(), "but the sea margin still exists");
    }

    // Wire shape is camelCase + sparse (no n*n array).
    #[test]
    fn terrain_serializes_camel_case_and_sparse() {
        let buildings = wide_city();
        let t = build_terrain(&buildings, &[], 0);
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("\"seaX\":"));
        assert!(json.contains("\"minY\":"));
        assert!(json.contains("\"maxY\":"));
        assert!(json.contains("\"rivers\":"));
        assert!(json.contains("\"water\":"));
        assert!(json.contains("\"gxMin\":"));
        assert!(json.contains("\"deep\":"));
    }

    // NEGATIVE GY REGRESSION — the original stack overflow happened because
    // `gy / 4` (truncation toward zero) gave negative blocks for gy < 0,
    // causing the recursive meander_offset to walk away from block 0 forever.
    // This test places buildings at negative gy to exercise the iterative
    // `div_euclid` fix and the precomputed channels.
    #[test]
    fn negative_gy_band_no_hang_and_channels_match() {
        // Two houses at negative gy, x spread wide enough for a river.
        let buildings = vec![
            bld("neg_a", purpose::HOUSE, visual_tier::KALYBE, 5.0, -20.0),
            bld("neg_b", purpose::HOUSE, visual_tier::KALYBE, 15.0, -14.0),
        ];
        // Must not hang (stack overflow → SIGABRT).
        let t = build_terrain(&buildings, &[], 0);

        // The band should be in negative gy territory.
        assert!(
            t.min_y < -8,
            "min_y={} must be well negative to exercise the fix",
            t.min_y
        );

        // If a river exists, validate it in the negative band.
        if !t.rivers.is_empty() {
            let r = &t.rivers[0];
            // Precomputed channels exist for every row in the band.
            let band_height = (t.max_y - t.min_y) as usize;
            assert_eq!(
                r.channels.len(),
                band_height,
                "precomputed channels must cover the full band"
            );

            let mut prev_ch: Option<i32> = None;
            let chan_end = t.min_y + r.channels.len() as i32;
            for gy in t.min_y..chan_end {
                let ch_precomputed = r.channels[(gy - t.min_y) as usize];

                // Exactly 2 river columns per row.
                assert!(
                    is_river_tile(&t.rivers, ch_precomputed, gy, t.sea_x, t.min_y),
                    "ch={} must be river at gy={}",
                    ch_precomputed,
                    gy
                );
                assert!(
                    is_river_tile(&t.rivers, ch_precomputed + 1, gy, t.sea_x, t.min_y),
                    "ch+1={} must be river at gy={}",
                    ch_precomputed + 1,
                    gy
                );

                // 4-connectivity: shift ≤ 1 between consecutive rows.
                if let Some(prev) = prev_ch {
                    assert!(
                        (ch_precomputed - prev).abs() <= 1,
                        "channel shift must be ≤1: prev={} curr={} at gy={}",
                        prev,
                        ch_precomputed,
                        gy
                    );
                }
                prev_ch = Some(ch_precomputed);
            }
        }
    }

    // Sanity: meander_offset itself works for negative gy (iterative walk,
    // not recursive — no stack overflow).
    #[test]
    fn meander_offset_works_for_negative_gy() {
        let base = 10;
        // These would have caused infinite recursion with the old recursive
        // implementation using `gy / 4` (truncation toward zero).
        for &gy in &[-100, -37, -13, -8, -4, -1, 0, 1, 4, 13, 37, 100] {
            let offset = meander_offset(base, gy);
            assert!(
                offset >= -1 && offset <= 1,
                "meander_offset(base={}, gy={}) = {} must be in [-1,0,+1]",
                base,
                gy,
                offset
            );
        }
        // Consistency: channel_start = base + meander_offset
        for &gy in &[-20, -7, 0, 5, 19] {
            let ch = channel_start(base, gy);
            let off = meander_offset(base, gy);
            assert_eq!(ch, base + off, "channel_start consistency at gy={}", gy);
        }
    }

    /// Water-affine river placement: a city with harbor/lighthouse buildings at a
    /// known gx far from the midpoint gets a river whose envelope is within a
    /// few columns of that cluster (port anchor pulls the river). A control city
    /// without harbors keeps the midpoint river.
    ///
    /// Also covers the single-anchor-both-slots case: ONE harbor at x=20 is
    /// nearest both 1/3 and 2/3 positional targets (for width=24, targets are
    /// x=8 and x=16). The anchor fills the slot it is closest to (2/3 → x=16,
    /// closest to x=20) and the other slot gets the geometric candidate (x=8),
    /// so the city still gets 2 rivers like a harbor-less city of the same width.
    #[test]
    fn rivers_prefer_port_anchor_positions() {
        // Wide city (width=24): houses at x=0..22, gap corridor in the middle.
        // ONE harbor at x=20 — nearest both thirds (8 and 16), closest to 16.
        let mut port_buildings = wide_city();
        port_buildings.push(bld("port-a", purpose::HARBOR, visual_tier::KALYBE, 20.0, 2.0));

        let t_port = build_terrain(&port_buildings, &[], 0);
        // Must get 2 rivers (width >= 18), even though the single anchor maps
        // to both positional slots — the other slot gets the geometric candidate.
        assert_eq!(
            t_port.rivers.len(),
            2,
            "single-anchor city with width>=18 must still get 2 rivers, got {}",
            t_port.rivers.len()
        );

        // One river should be near the anchor (x=20), the other near the
        // geometric 1/3 column (x=8).
        let centers: Vec<i32> = t_port
            .rivers
            .iter()
            .map(|r| (r.gx_min + r.gx_max) / 2)
            .collect();
        let near_anchor = centers.iter().any(|&c| (c - 20).unsigned_abs() <= 6);
        let near_geometric = centers.iter().any(|&c| (c - 8).unsigned_abs() <= 6);
        assert!(
            near_anchor,
            "one river should be near the harbor anchor x=20, got centers {:?}",
            centers
        );
        assert!(
            near_geometric,
            "one river should be near the geometric 1/3 x=8, got centers {:?}",
            centers
        );

        // Control: same city WITHOUT harbors. Must also get 2 rivers near the
        // geometric thirds (x=8 and x=16).
        let t_no_port = build_terrain(&wide_city(), &[], 0);
        assert_eq!(t_no_port.rivers.len(), 2, "no-port city gets 2 rivers");
        let ctrl_centers: Vec<i32> = t_no_port
            .rivers
            .iter()
            .map(|r| (r.gx_min + r.gx_max) / 2)
            .collect();
        let ctrl_near_8 = ctrl_centers.iter().any(|&c| (c - 8).unsigned_abs() <= 6);
        let ctrl_near_16 = ctrl_centers.iter().any(|&c| (c - 16).unsigned_abs() <= 6);
        assert!(
            ctrl_near_8,
            "no-port river near geometric 1/3 x=8, got {:?}",
            ctrl_centers
        );
        assert!(
            ctrl_near_16,
            "no-port river near geometric 2/3 x=16, got {:?}",
            ctrl_centers
        );
    }
}
