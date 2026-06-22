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

/// A river is an internal vertical channel: a closed inclusive column range
/// `[gx_min, gx_max]` running through the land band's `y` extent and flowing
/// east INTO the sea (`gy < sea_x`'s land rows). Shores (`Sand`) are the columns
/// immediately left/right of the channel, so a river always has land on BOTH
/// sides (never on the map edge). Mirrors `RIVERS = [{gx:[a,b]}]` in `map_app.js`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct River {
    /// Inclusive min column of the channel.
    pub gx_min: i32,
    /// Inclusive max column of the channel.
    pub gx_max: i32,
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
///   3. Deterministically place 1–2 internal river channels between districts (in
///      the LAND band only), nudged so no channel/shore clips a building footprint.
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
    // Extend the (exclusive) `max_y` to whichever is lower: the land bottom or the
    // last harbour row + 1 (inclusive -> exclusive). The harbour count is derived
    // from the SAME inputs `cloud.rs` uses (anchor = `min_y`, pitch = `ROW_PITCH`).
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
    let rivers = place_rivers(min_x, max_x, min_y, land_max_y, &occ);

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
    for gy in min_y..land_max_y {
        // River columns for this row (land band only — `gx < sea_x`).
        for r in &rivers {
            for gx in r.gx_min..=r.gx_max {
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
        // Shore sand: the tile immediately left/right of each river channel
        // (banks), and the column immediately WEST of the sea edge (the beach).
        // Mirrors `landType`'s sand rules. Shores are land tiles, never water.
        for r in &rivers {
            for &bank in &[r.gx_min - 1, r.gx_max + 1] {
                if bank >= min_x && bank < sea_x && !is_river_col(&rivers, bank) {
                    sand.insert(Tile::new(bank, gy));
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
        if beach >= min_x && !is_river_col(&rivers, beach) && !occ.contains(&beach_tile) {
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

/// Classify a SINGLE tile given the terrain inputs — the HANDOFF rules 1–6 in
/// priority order, adapted to the +x seaward axis. `road` = is this tile under a
/// routed road; used to derive `Road`/`Bridge`. Pure; the canonical reference
/// for the classification tests.
///
/// Priority (highest first):
///   1. `gx >= sea_x` (within band) -> `Sea`.
///   2. river column and `gx < sea_x` -> `River` (or `Bridge` if a road crosses).
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
    // Rule 2 — internal river channel (lands strictly west of the sea edge).
    if gx < sea_x && is_river_col(rivers, gx) {
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
    // Rule 4 — shore sand: immediately west of the sea, or a river bank.
    let on_beach = gx == sea_x - 1;
    let on_bank = is_river_col(rivers, gx - 1) || is_river_col(rivers, gx + 1);
    if gx >= min_x && (on_beach || on_bank) {
        return Terrain::Sand;
    }
    // Rule 5 — chora.
    Terrain::Grass
}

/// Is `gx` inside any river channel column range?
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

/// Deterministically place 1–2 internal river channels between the city's
/// districts, INSIDE the land band (`min_x < channel < sea_x`), nudged so neither
/// the channel NOR its two shore banks clip a building footprint.
///
/// Placement is derived purely from the land extent (no RNG): the land width is
/// split into thirds; a river is proposed at the 1/3 and 2/3 columns. A river is
/// only kept if the band is wide enough to host it AND a clear column (channel +
/// both banks free of buildings) can be found by nudging the proposal up to a
/// bounded number of columns. A second river is only added when the band is wide
/// enough to keep the two channels (and their banks) from touching.
fn place_rivers(
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
    occ: &BTreeSet<Tile>,
) -> Vec<River> {
    let width = max_x - min_x;
    // Too narrow to host an internal river with land+shore on both sides.
    // Need at least: bank | channel | bank, plus land beyond each side.
    if width < 9 || max_y <= min_y {
        return Vec::new();
    }

    // A single-column channel keeps rivers slim (a clean canal). Banks are the
    // immediately adjacent columns, so a "clear column" needs 3 free columns.
    let propose = |frac_num: i32, frac_den: i32| -> i32 { min_x + (width * frac_num) / frac_den };

    let mut rivers: Vec<River> = Vec::new();

    // First river ~1/3 across; second ~2/3. The order (1/3 then 2/3) is fixed so
    // the result is deterministic.
    let candidates = if width >= 18 {
        vec![propose(1, 3), propose(2, 3)]
    } else {
        vec![propose(1, 2)]
    };

    for cand in candidates {
        if let Some(col) = clear_column(cand, min_x + 2, max_x - 2, min_y, max_y, occ, &rivers) {
            rivers.push(River {
                gx_min: col,
                gx_max: col,
            });
        }
    }

    rivers
}

/// Find a single channel column near `proposed`, within `[lo, hi]`, whose channel
/// AND both shore banks are free of buildings for the whole `y`-band, and that
/// does not touch an already-placed river (>= 3 columns apart so the banks never
/// merge). Searches outward from `proposed` by increasing offset (bounded), so
/// the choice is deterministic. Returns `None` if no clear column exists.
fn clear_column(
    proposed: i32,
    lo: i32,
    hi: i32,
    min_y: i32,
    max_y: i32,
    occ: &BTreeSet<Tile>,
    existing: &[River],
) -> Option<i32> {
    if lo > hi {
        return None;
    }
    let max_offset = (hi - lo).max(0);
    for off in 0..=max_offset {
        // Try +off then -off (deterministic, prefers the proposed/east side).
        for &col in &[proposed + off, proposed - off] {
            if col < lo || col > hi {
                continue;
            }
            // Keep channels (+banks) at least 3 columns apart.
            if existing
                .iter()
                .any(|r| (r.gx_min - col).abs() < 3 || (r.gx_max - col).abs() < 3)
            {
                continue;
            }
            if column_clear(col, min_y, max_y, occ) {
                return Some(col);
            }
        }
    }
    None
}

/// Is the channel column `col` AND its two banks (`col-1`, `col+1`) free of any
/// building footprint tile across the whole `y`-band?
fn column_clear(col: i32, min_y: i32, max_y: i32, occ: &BTreeSet<Tile>) -> bool {
    for gx in (col - 1)..=(col + 1) {
        for gy in min_y..max_y {
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
        }
    }

    /// A spread of small houses across a wide band so the river placement and sea
    /// margin have room to work.
    fn wide_city() -> Vec<Building> {
        let mut v = Vec::new();
        // 0..=24 in x, 0..=10 in y, sparse 1x1 houses every 4 tiles so there are
        // clear inter-district columns for rivers.
        for (i, x) in (0..=24).step_by(4).enumerate() {
            for (j, y) in (0..=8).step_by(4).enumerate() {
                v.push(bld(
                    &format!("b{i}-{j}"),
                    purpose::HOUSE,
                    visual_tier::KALYBE,
                    x as f64,
                    y as f64,
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
        // sea band), and its banks are classified Sand.
        let r = t.rivers[0];
        for gy in t.min_y..t.max_y {
            assert_eq!(
                classify(r.gx_min, gy, min_x, t.min_y, t.max_y, t.sea_x, &t.rivers, false),
                Terrain::River,
                "channel tile is River at gy={gy}"
            );
            assert_eq!(
                classify(
                    r.gx_min - 1,
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
            assert_eq!(
                classify(
                    r.gx_max + 1,
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
        // Sand set emitted on both banks.
        assert!(t.sand.iter().any(|s| s.gx == r.gx_min - 1));
        assert!(t.sand.iter().any(|s| s.gx == r.gx_max + 1));
    }

    // Rule 6 — bridge only where a ROAD crosses a RIVER tile.
    #[test]
    fn bridge_marked_only_where_a_road_crosses_a_river() {
        let buildings = wide_city();
        let t0 = build_terrain(&buildings, &[], 0);
        let r = t0.rivers[0];
        let cross_y = t0.min_y + 1;

        // A road running east across the river channel at row cross_y.
        let road = road_with_path("r0", vec![(r.gx_min - 2, cross_y), (r.gx_max + 2, cross_y)]);
        let t = build_terrain(&buildings, &[road], 0);

        let crossing = Tile::new(r.gx_min, cross_y);
        assert!(
            t.bridges.contains(&crossing),
            "road crossing the channel marks a Bridge at {crossing:?}"
        );
        // classify confirms Bridge at the crossing (road over river) ...
        let min_x = map_extent(&buildings).unwrap().0.floor() as i32;
        assert_eq!(
            classify(r.gx_min, cross_y, min_x, t.min_y, t.max_y, t.sea_x, &t.rivers, true),
            Terrain::Bridge,
        );
        // ... and a NON-river road tile is just a Road, never a Bridge.
        let land_road = Tile::new(r.gx_min - 2, cross_y);
        assert!(
            !t.bridges.contains(&land_road),
            "land road tile is not a bridge"
        );
        assert_eq!(
            classify(
                r.gx_min - 2,
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
            !t.bridges.contains(&Tile::new(r.gx_min, t.min_y)),
            "a river tile far from the road is not a bridge"
        );
    }

    // No building footprint ever lands on Sea or River — and (FIX 2) the EMITTED
    // sand set never paves a building footprint tile either. The classifier
    // (`classify`) is intentionally occupancy-agnostic (it would call a building's
    // `sea_x-1` column `Sand`), so walkability is protected by the EMITTED
    // `TerrainData` (the wire payload Phase C walks), which `build_terrain` filters
    // against `occ`. We assert on `t.water` / `t.sand` directly.
    #[test]
    fn no_building_tile_is_water() {
        // `wide_city` has 1x1 houses out to x=24, so `max_x = sea_x = 25` and the
        // east-edge house at x=24 sits squarely on the beach column `sea_x-1` — the
        // exact case FIX 2 guards.
        let buildings = wide_city();
        let t = build_terrain(&buildings, &[], 0);
        assert_eq!(
            t.sea_x, 25,
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
    // of the ORDER of the buildings/roads slices (FIX 5: the prior test only ran
    // the SAME slice twice, which can't catch an iteration-order leak). We build
    // once, then again with both input slices REVERSED, and assert byte-identical
    // `TerrainData` + JSON — proving the classification + river placement + sparse
    // payload derive purely from the geometry, not the input order.
    #[test]
    fn terrain_is_deterministic() {
        let buildings = wide_city();
        // Two crossing roads so the road-tile set is order-sensitive if anything
        // leaked (it must not).
        let roads = vec![
            road_with_path("r0", vec![(0, 1), (24, 1)]),
            road_with_path("r1", vec![(2, 0), (2, 8)]),
        ];
        // A few harbours so the sea-band extension is exercised in the determinism
        // check too.
        let a = build_terrain(&buildings, &roads, 5);

        // Reverse BOTH slices: a deterministic permutation that changes iteration
        // order without changing the geometry.
        let mut buildings_rev = buildings.clone();
        buildings_rev.reverse();
        let mut roads_rev = roads.clone();
        roads_rev.reverse();
        let b = build_terrain(&buildings_rev, &roads_rev, 5);

        assert_eq!(
            a, b,
            "terrain must be byte-identical regardless of building/road input order"
        );
        // And serializes identically.
        let ja = serde_json::to_string(&a).unwrap();
        let jb = serde_json::to_string(&b).unwrap();
        assert_eq!(ja, jb);
    }

    // FIX 3 — the sea band covers the FULL harbour column. With enough services
    // the lowest harbour steps below the land's `max_y`; the sea must follow so it
    // sits on water, not on grass.
    #[test]
    fn sea_band_covers_the_full_harbour_column() {
        use crate::polis::cloud;

        let buildings = wide_city();
        let (_min_x, min_y_f, max_x, max_y_f) = map_extent(&buildings).unwrap();
        let land_max_y = max_y_f.ceil() as i32;

        // Enough harbours that the column extends well past the land bottom.
        let n: usize = 30;
        let t = build_terrain(&buildings, &[], n);

        // The lowest harbour row (mirrors `cloud::place_external_services`).
        let bottom_y = cloud::harbour_bottom_y(n, min_y_f).ceil() as i32;
        assert!(
            bottom_y >= land_max_y,
            "the {n}-harbour column must extend below the land bottom for this test \
             (bottom_y={bottom_y}, land_max_y={land_max_y})"
        );
        // The sea band's exclusive `max_y` must cover the lowest harbour row.
        assert!(
            t.max_y > bottom_y,
            "sea band max_y={} must cover the lowest harbour row gy={bottom_y}",
            t.max_y
        );

        // Every harbour `coords` (the seaward column at `max_x + GAP`) lands on a
        // Sea tile — both in the emitted water set AND via `classify`.
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

        // The extension rows (below the land) carry ONLY sea — no river/sand/beach
        // there (there is no land to host them).
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
        // WaterTile.deep is camelCase-trivial ("deep").
        assert!(json.contains("\"deep\":"));
    }
}
