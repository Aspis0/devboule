//! Polis Map — NAVIGATION (walkability + A*) — the canonical "citizens walk only
//! on roads/plaza/bridges, NEVER on sea/river/buildings" model (HANDOFF `nav.rs`).
//!
//! THE GUARANTEE (the user's ask — "cittadini che camminano solo sulle strade"):
//!   - `walkable(t)` is TRUE only for `Road | Plaza | Bridge`.
//!   - a navigation node is a walkable tile that no building footprint occupies.
//!   - A* is 4-neighbour (no diagonals): it never corner-cuts across an angle of
//!     water. The graph excludes `Sea`/`River`/footprints, so BY CONSTRUCTION a
//!     returned path is entirely walkable — it never touches water or a building,
//!     and the ONLY way it crosses a river is over a `Bridge` tile.
//!   - if no path exists (e.g. an island of road behind an un-bridged river) A*
//!     returns `None`; the agent then stays put and never sconfina.
//!
//! WHERE THIS SITS. The terrain frame (`terrain.rs`) already emits, sparsely, the
//! exact non-grass tiles the frontend draws + walks: `water` (sea+river),
//! `bridges` (road∩river), `sand`. Roads are routed by `grid::route_roads` AROUND
//! building footprints (so a road tile is never on a building), and a road tile
//! that lands on a river column is marked `Bridge` in `build_terrain`. This module
//! reconstructs, from those SAME inputs (`&[Building]`, `&[Road]`, `&TerrainData`),
//! the per-tile walkability the citizens depend on, and adds:
//!   - `NavGrid::terrain_at` — the effective `Terrain` of any tile (occupancy-aware
//!     via `classify` + the emitted bridge set);
//!   - `NavGrid::is_node` / `astar` — the canonical road pathfinder;
//!   - `road_path_tiles` + `road_paths_all_walkable` — the VALIDATION that the
//!     polylines the existing frontend citizens walk are entirely walkable by
//!     construction (the load-bearing guarantee: if a routed `Road.path` tile were
//!     ever Sea / un-bridged River / a footprint, that is the bug to fix in the
//!     bridge-marking / routing, not to paper over downstream).
//!
//! DETERMINISM. A* uses a `BinaryHeap` with a total tie-break order
//! (`f`, then `g`, then `Tile`) and a FIXED 4-neighbour offset order, plus
//! `BTree*` scratch (no `HashMap` iteration order, no RNG, no `Date`). The same
//! inputs reproduce a byte-identical path.

use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use crate::polis::model::{Building, Road};
use crate::polis::terrain::{classify, Terrain, TerrainData, Tile};

/// Walkability predicate — the heart of the guarantee. A citizen/agent/porter may
/// stand ONLY on these terrains; everything else (`Grass`, `Sand`, `River`, `Sea`)
/// is off-limits. Mirrors HANDOFF `walkable(t) = Road | Plaza | Bridge` exactly.
#[inline]
pub fn walkable(t: Terrain) -> bool {
    matches!(t, Terrain::Road | Terrain::Plaza | Terrain::Bridge)
}

/// A precomputed walkability grid over a laid-out city, derived from the SAME
/// inputs as the wire terrain (`build_terrain`): the routed road tiles, the
/// emitted river/sea water + bridge sets, and the building footprints. It answers
/// `terrain_at` / `is_node` per tile and runs the canonical 4-neighbour A*.
///
/// It holds only ordered sets (deterministic; no hash-seed dependence).
pub struct NavGrid {
    /// `gx >= sea_x` (within `[min_y, max_y)`) is open `Sea`. Mirrors `TerrainData`.
    sea_x: i32,
    min_y: i32,
    max_y: i32,
    /// River channel columns (with `gx < sea_x`), copied from `TerrainData::rivers`.
    rivers: Vec<(i32, i32)>,
    /// Tiles a routed road covers (rasterized from every `Road::path`). A road tile
    /// over a river column is a `Bridge`; otherwise `Road`.
    roads: BTreeSet<Tile>,
    /// Tiles marked `Bridge` (road∩river) — the SOLE river crossing. A subset of
    /// `roads`, copied from `TerrainData::bridges`.
    bridges: BTreeSet<Tile>,
    /// Tiles occupied by a building footprint (blocked for navigation).
    occ: BTreeSet<Tile>,
    /// Westmost building column — passed to `classify` for its `min_x`-gated rules
    /// (only matters for `Sand`, which is non-walkable anyway, so any value is safe
    /// for the walkable verdict; we use the real one for an exact classification).
    min_x: i32,
}

impl NavGrid {
    /// Build the navigation grid from a laid-out city's buildings + routed roads +
    /// the emitted terrain frame. Pure + deterministic.
    ///
    /// The `terrain` MUST be the one produced by `build_terrain(buildings, roads,
    /// _)` for the same `buildings`/`roads` (it is, at the `generate_city_state`
    /// integration point) so the bridge set lines up with the road tiles.
    pub fn new(buildings: &[Building], roads: &[Road], terrain: &TerrainData) -> Self {
        let roads = road_path_tiles(roads);
        let bridges: BTreeSet<Tile> = terrain.bridges.iter().copied().collect();
        let occ = building_tiles(buildings);
        let rivers = terrain
            .rivers
            .iter()
            .map(|r| (r.gx_min, r.gx_max))
            .collect();
        let min_x = buildings
            .iter()
            .map(|b| b.coords.x.round() as i32)
            .min()
            .unwrap_or(0);
        Self {
            sea_x: terrain.sea_x,
            min_y: terrain.min_y,
            max_y: terrain.max_y,
            rivers,
            roads,
            bridges,
            occ,
            min_x,
        }
    }

    /// The EFFECTIVE terrain of a tile for navigation, occupancy-aware:
    ///   - a routed road tile over a river column is a `Bridge` (walkable);
    ///   - a routed road tile elsewhere is a `Road`;
    ///   - otherwise the geometric `classify` verdict (Sea/River/Sand/Grass).
    ///
    /// Note `classify` already returns `Bridge` for a road-over-river tile when
    /// told the tile is a road; we additionally honour the emitted `bridges` set so
    /// the result agrees with the wire payload the frontend draws/walks.
    pub fn terrain_at(&self, t: Tile) -> Terrain {
        let is_road = self.roads.contains(&t);
        let base = classify(
            t.gx,
            t.gy,
            self.min_x,
            self.min_y,
            self.max_y,
            self.sea_x,
            &rivers_as_slice(&self.rivers),
            is_road,
        );
        // A tile explicitly marked Bridge stays a Bridge even if (defensively)
        // `classify` disagreed; this keeps `terrain_at` consistent with the emitted
        // wire `bridges`. The override is GATED on road membership (matching the
        // construction invariant `bridges ⊆ road_tiles`): a stray bridge tile that
        // is NOT a routed road would otherwise become silently walkable. Otherwise
        // `classify` is the source of truth.
        if is_road && self.bridges.contains(&t) {
            return Terrain::Bridge;
        }
        base
    }

    /// A navigation NODE: a walkable tile not occupied by a building footprint.
    /// Sea / un-bridged River / Grass / Sand and any footprint tile are NOT nodes,
    /// so a path can never include them.
    pub fn is_node(&self, t: Tile) -> bool {
        walkable(self.terrain_at(t)) && !self.occ.contains(&t)
    }

    /// Deterministic 4-neighbour A* (Manhattan heuristic, NO diagonals) over
    /// walkable, unoccupied tiles. Returns the tile path `start..=goal` (inclusive)
    /// or `None` when `start`/`goal` is not a node or no path exists.
    ///
    /// Because the graph contains ONLY nodes (walkable + unoccupied) and steps are
    /// 4-connected, EVERY tile of the returned path is walkable — never Sea, never
    /// an un-bridged River, never a building — and the only way it crosses a river
    /// is over a `Bridge` tile (the sole river node). No corner-cutting across an
    /// angle of water is possible without a diagonal step.
    pub fn astar(&self, start: Tile, goal: Tile) -> Option<Vec<Tile>> {
        if !self.is_node(start) || !self.is_node(goal) {
            return None;
        }
        if start == goal {
            return Some(vec![start]);
        }

        let mut g_score: BTreeMap<Tile, u32> = BTreeMap::new();
        let mut came_from: BTreeMap<Tile, Tile> = BTreeMap::new();
        let mut open: BinaryHeap<Frontier> = BinaryHeap::new();

        g_score.insert(start, 0);
        open.push(Frontier {
            f: manhattan(start, goal),
            g: 0,
            tile: start,
        });

        while let Some(Frontier { g, tile, .. }) = open.pop() {
            if tile == goal {
                return Some(reconstruct(&came_from, goal));
            }
            // Skip stale heap entries (a better g was already recorded).
            if g > *g_score.get(&tile).unwrap_or(&u32::MAX) {
                continue;
            }
            for n in neighbors(tile) {
                if !self.is_node(n) {
                    continue;
                }
                let tentative = g.saturating_add(1);
                if tentative < *g_score.get(&n).unwrap_or(&u32::MAX) {
                    came_from.insert(n, tile);
                    g_score.insert(n, tentative);
                    let f = tentative.saturating_add(manhattan(n, goal));
                    open.push(Frontier {
                        f,
                        g: tentative,
                        tile: n,
                    });
                }
            }
        }
        None
    }
}

/// 4-connected neighbour offsets in a FIXED order (determinism — no diagonals).
const NEIGHBORS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

#[inline]
fn neighbors(t: Tile) -> [Tile; 4] {
    [
        Tile::new(t.gx + NEIGHBORS[0].0, t.gy + NEIGHBORS[0].1),
        Tile::new(t.gx + NEIGHBORS[1].0, t.gy + NEIGHBORS[1].1),
        Tile::new(t.gx + NEIGHBORS[2].0, t.gy + NEIGHBORS[2].1),
        Tile::new(t.gx + NEIGHBORS[3].0, t.gy + NEIGHBORS[3].1),
    ]
}

#[inline]
fn manhattan(a: Tile, b: Tile) -> u32 {
    (a.gx - b.gx).unsigned_abs() + (a.gy - b.gy).unsigned_abs()
}

/// A* frontier node for a MIN-heap on `(f, g, tile)` with a total tie-break order,
/// so the path is byte-identical across runs (no `HashMap`/RNG nondeterminism).
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct Frontier {
    f: u32,
    g: u32,
    tile: Tile,
}

impl Ord for Frontier {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse so `BinaryHeap` (a max-heap) pops the SMALLEST `f`; deterministic
        // tie-break by smaller `g`, then by `Tile` Ord.
        other
            .f
            .cmp(&self.f)
            .then_with(|| other.g.cmp(&self.g))
            .then_with(|| other.tile.cmp(&self.tile))
    }
}
impl PartialOrd for Frontier {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Walk `came_from` back from `goal`, producing `start..=goal` order.
fn reconstruct(came_from: &BTreeMap<Tile, Tile>, goal: Tile) -> Vec<Tile> {
    let mut path = vec![goal];
    let mut cur = goal;
    while let Some(&prev) = came_from.get(&cur) {
        path.push(prev);
        cur = prev;
    }
    path.reverse();
    path
}

/// `rivers` stored as `(min, max)` pairs → the `terrain::River` slice `classify`
/// expects. Local helper so `NavGrid` can keep the compact pair form.
fn rivers_as_slice(rivers: &[(i32, i32)]) -> Vec<crate::polis::terrain::River> {
    rivers
        .iter()
        .map(|&(gx_min, gx_max)| crate::polis::terrain::River { gx_min, gx_max })
        .collect()
}

/// All tiles a building footprint occupies, in absolute tile space —
/// `[coords.x, coords.x + W) x [coords.y, coords.y + D)`. Mirrors
/// `terrain::building_tiles` (kept private there) so nav blocks the same cells.
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
/// IDENTICAL stepping to `terrain::road_tiles` (kept private there) so the nav
/// road set agrees tile-for-tile with the terrain's bridge marking. Roads with no
/// `path` contribute nothing (the renderer draws them straight; they carve no
/// terrain and are not walkable streets).
pub fn road_path_tiles(roads: &[Road]) -> BTreeSet<Tile> {
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
            let mut x = prev.gx;
            let mut y = prev.gy;
            let dx = (cur.gx - x).signum();
            let dy = (cur.gy - y).signum();
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

/// THE LOAD-BEARING GUARANTEE CHECK. Every tile of every routed `Road.path` (the
/// polylines the existing frontend citizens/porters/agents walk) must be walkable
/// — `Road` or `Bridge`, never `Sea`, never an un-bridged `River`, never a
/// building footprint. Returns `Ok(())` if so, else `Err` naming the first
/// offending tile + its terrain (the bug to fix in bridge-marking / routing).
///
/// Roads are routed AROUND footprints and a road-over-river tile is marked
/// `Bridge` in `build_terrain`, so this holds BY CONSTRUCTION; the check (and its
/// test) makes the invariant explicit and regression-proof.
pub fn road_paths_all_walkable(
    buildings: &[Building],
    roads: &[Road],
    terrain: &TerrainData,
) -> Result<(), String> {
    let nav = NavGrid::new(buildings, roads, terrain);
    for tile in road_path_tiles(roads) {
        let t = nav.terrain_at(tile);
        if !walkable(t) {
            return Err(format!(
                "routed road tile {tile:?} is {t:?}, not walkable (Road/Bridge) — \
                 a road over water must be marked Bridge, or the router must not run \
                 a road over the sea"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::polis::model::{
        building_status, purpose, purpose_source, road_style, road_type, visual_tier, Coords,
    };
    use crate::polis::terrain::build_terrain;

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

    /// Same wide spread of small houses as the terrain tests, so a river + sea
    /// frame is generated to navigate around.
    fn wide_city() -> Vec<Building> {
        let mut v = Vec::new();
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

    // (a) `walkable` is TRUE only for Road/Plaza/Bridge.
    #[test]
    fn walkable_true_only_for_road_plaza_bridge() {
        assert!(walkable(Terrain::Road));
        assert!(walkable(Terrain::Plaza));
        assert!(walkable(Terrain::Bridge));
        assert!(!walkable(Terrain::Grass));
        assert!(!walkable(Terrain::Sand));
        assert!(!walkable(Terrain::River));
        assert!(!walkable(Terrain::Sea));
    }

    // (b) An A* path between two road tiles contains ONLY walkable tiles — never
    // Sea / un-bridged River / a building footprint.
    #[test]
    fn astar_path_is_entirely_walkable() {
        let buildings = wide_city();
        // A long east-west road plus a north-south road so there is a connected
        // street network to route over.
        let roads = vec![
            road_with_path("r0", vec![(0, 1), (24, 1)]),
            road_with_path("r1", vec![(2, 0), (2, 8)]),
        ];
        let terrain = build_terrain(&buildings, &roads, 0);
        let nav = NavGrid::new(&buildings, &roads, &terrain);

        let start = Tile::new(0, 1);
        let goal = Tile::new(24, 1);
        assert!(nav.is_node(start), "start road tile is a node");
        assert!(nav.is_node(goal), "goal road tile is a node");

        let path = nav
            .astar(start, goal)
            .expect("path exists between road tiles");
        assert_eq!(path.first().copied(), Some(start));
        assert_eq!(path.last().copied(), Some(goal));
        for tile in &path {
            let t = nav.terrain_at(*tile);
            assert!(
                walkable(t),
                "every path tile must be walkable; {tile:?} is {t:?}"
            );
            assert!(
                !nav.occ.contains(tile),
                "path tile {tile:?} must not be on a building footprint"
            );
        }
        // And every consecutive step is 4-connected (Manhattan distance 1) — no
        // diagonal corner-cut.
        for w in path.windows(2) {
            assert_eq!(
                manhattan(w[0], w[1]),
                1,
                "steps are 4-connected, no diagonals"
            );
        }
    }

    // (c) A* across a river uses ONLY the Bridge tile as the crossing (the sole
    // river node); the path never stands on an un-bridged river tile.
    #[test]
    fn astar_crosses_a_river_only_via_the_bridge() {
        let buildings = wide_city();
        // First, find the river the terrain places for this city.
        let t0 = build_terrain(&buildings, &[], 0);
        let river = t0.rivers[0];
        let cross_y = t0.min_y + 1;

        // A single east-west road crossing the river channel at `cross_y`, with a
        // span on BOTH banks so start/goal sit on opposite sides of the water.
        let west = river.gx_min - 3;
        let east = river.gx_max + 3;
        let road = road_with_path("bridge-road", vec![(west, cross_y), (east, cross_y)]);
        let terrain = build_terrain(&buildings, &[road.clone()], 0);
        let nav = NavGrid::new(&buildings, &[road], &terrain);

        let bridge = Tile::new(river.gx_min, cross_y);
        assert_eq!(
            nav.terrain_at(bridge),
            Terrain::Bridge,
            "the road tile over the river is a Bridge"
        );

        let start = Tile::new(west, cross_y);
        let goal = Tile::new(east, cross_y);
        let path = nav.astar(start, goal).expect("path crosses the river");

        // The path MUST include the bridge tile (the only walkable river tile) and
        // MUST NOT stand on any un-bridged river tile.
        assert!(
            path.contains(&bridge),
            "the path crosses via the Bridge tile"
        );
        for tile in &path {
            let t = nav.terrain_at(*tile);
            assert_ne!(
                t,
                Terrain::River,
                "path never stands on un-bridged River at {tile:?}"
            );
            assert_ne!(t, Terrain::Sea, "path never stands on Sea at {tile:?}");
            assert!(walkable(t), "path tile {tile:?} is walkable ({t:?})");
        }
    }

    // (d) A* returns None when the goal is unreachable — an island of road
    // separated from the start by an un-bridged river. The agent stays put.
    #[test]
    fn astar_none_when_unreachable_across_unbridged_river() {
        let buildings = wide_city();
        let t0 = build_terrain(&buildings, &[], 0);
        let river = t0.rivers[0];
        let row = t0.min_y + 1;

        // Two road STUBS on opposite banks that DO NOT cross the river (no bridge):
        // a west stub strictly west of the channel, an east stub strictly east.
        let west_stub = road_with_path(
            "west",
            vec![(river.gx_min - 3, row), (river.gx_min - 1, row)],
        );
        let east_stub = road_with_path(
            "east",
            vec![(river.gx_max + 1, row), (river.gx_max + 3, row)],
        );
        let roads = vec![west_stub, east_stub];
        let terrain = build_terrain(&buildings, &roads, 0);
        let nav = NavGrid::new(&buildings, &roads, &terrain);

        // No bridge exists (no road crossed the channel).
        assert!(
            terrain.bridges.is_empty(),
            "no road crossed the river → no bridge"
        );

        let start = Tile::new(river.gx_min - 3, row);
        let goal = Tile::new(river.gx_max + 3, row);
        assert!(
            nav.is_node(start) && nav.is_node(goal),
            "both stubs are walkable nodes"
        );
        assert!(
            nav.astar(start, goal).is_none(),
            "with no bridge, the far bank is unreachable → None (agent stays put)"
        );
    }

    // (d') A* returns None when the goal itself is not a node (e.g. open sea or a
    // building tile) — you can never route ONTO water or a building.
    #[test]
    fn astar_none_when_goal_is_not_walkable() {
        let buildings = wide_city();
        let roads = vec![road_with_path("r0", vec![(0, 1), (10, 1)])];
        let terrain = build_terrain(&buildings, &roads, 0);
        let nav = NavGrid::new(&buildings, &roads, &terrain);

        let start = Tile::new(0, 1);
        // A sea tile (east of sea_x) is not a node.
        let sea_goal = Tile::new(terrain.sea_x + 1, terrain.min_y);
        assert_eq!(nav.terrain_at(sea_goal), Terrain::Sea);
        assert!(
            nav.astar(start, sea_goal).is_none(),
            "cannot route onto the sea"
        );

        // A building footprint tile is not a node.
        let bldg_goal = Tile::new(0, 0); // house "b0-0" anchor
        assert!(nav.occ.contains(&bldg_goal), "(0,0) is a building tile");
        assert!(
            nav.astar(start, bldg_goal).is_none(),
            "cannot route onto a building"
        );
    }

    // (e) 4-neighbour: A* never makes a diagonal step, so it can never corner-cut
    // across a diagonal angle of water. We route an L-shaped road (a horizontal run
    // meeting a vertical run at a corner) and assert EVERY step of the returned path
    // is 4-connected (Manhattan distance exactly 1) — a diagonal hop (the only way
    // to skim a water corner) is structurally impossible. The L forces a direction
    // change at the corner so the test is not satisfied by a trivial straight line.
    #[test]
    fn astar_is_four_neighbour_no_diagonal_corner_cut() {
        let buildings = wide_city();
        // An L on dry land (clear of buildings/water): east along y=1 from x=10..=14,
        // then north along x=14 from y=1..=4. The corner at (14,1) forces a turn, so
        // a diagonal-capable pathfinder could try to cut it; 4-neighbour cannot.
        let road = road_with_path("L", vec![(10, 1), (14, 1), (14, 4)]);
        let terrain = build_terrain(&buildings, &[road.clone()], 0);
        let nav = NavGrid::new(&buildings, &[road], &terrain);

        let start = Tile::new(10, 1);
        let goal = Tile::new(14, 4);
        assert!(
            nav.is_node(start) && nav.is_node(goal),
            "L endpoints are nodes"
        );
        let path = nav.astar(start, goal).expect("L-path exists");
        assert!(path.len() >= 3, "an L path turns a corner (>= 3 tiles)");
        for w in path.windows(2) {
            assert_eq!(
                manhattan(w[0], w[1]),
                1,
                "every step is 4-connected (no diagonal corner-cut over water): {:?} -> {:?}",
                w[0],
                w[1]
            );
        }
    }

    // THE load-bearing guarantee: every routed Road.path tile is walkable.
    #[test]
    fn every_routed_road_path_tile_is_walkable() {
        let buildings = wide_city();
        let t0 = build_terrain(&buildings, &[], 0);
        let river = t0.rivers[0];
        // Roads that DO cross the river (so they exercise the bridge path) and roads
        // that run on land — all must be entirely walkable.
        let roads = vec![
            road_with_path(
                "cross",
                vec![
                    (river.gx_min - 3, t0.min_y + 1),
                    (river.gx_max + 3, t0.min_y + 1),
                ],
            ),
            road_with_path("land", vec![(0, 2), (24, 2)]),
            road_with_path("vert", vec![(2, 0), (2, 8)]),
        ];
        let terrain = build_terrain(&buildings, &roads, 0);
        road_paths_all_walkable(&buildings, &roads, &terrain)
            .expect("every routed road tile must be walkable (Road or Bridge)");

        // Sanity: the crossing road actually produced a bridge tile (so the check
        // really exercised the road-over-river case, not a vacuous all-land set).
        assert!(
            !terrain.bridges.is_empty(),
            "the crossing road marks a bridge"
        );
    }

    // The validation FAILS loudly if a road were (hypothetically) routed straight
    // over the open sea with no land/bridge — proving the check is not vacuous.
    #[test]
    fn road_paths_check_flags_a_road_over_the_sea() {
        let buildings = wide_city();
        let t0 = build_terrain(&buildings, &[], 0);
        // A degenerate road whose path runs east INTO the open sea (gx > sea_x).
        let into_sea = road_with_path(
            "bad",
            vec![(t0.sea_x - 1, t0.min_y), (t0.sea_x + 3, t0.min_y)],
        );
        let roads = vec![into_sea];
        let terrain = build_terrain(&buildings, &roads, 0);
        let err = road_paths_all_walkable(&buildings, &roads, &terrain)
            .expect_err("a road running onto the sea must be flagged");
        assert!(
            err.contains("not walkable"),
            "error explains the violation: {err}"
        );
    }

    // Determinism: the A* path is byte-identical regardless of input slice order.
    #[test]
    fn astar_is_deterministic() {
        let buildings = wide_city();
        let roads = vec![
            road_with_path("r0", vec![(0, 1), (24, 1)]),
            road_with_path("r1", vec![(2, 0), (2, 8)]),
            road_with_path("r2", vec![(10, 1), (10, 8)]),
        ];
        let terrain = build_terrain(&buildings, &roads, 0);
        let nav = NavGrid::new(&buildings, &roads, &terrain);
        let start = Tile::new(0, 1);
        let goal = Tile::new(10, 8);
        let p1 = nav.astar(start, goal).expect("path exists");

        // Rebuild with reversed slices (a permutation that changes iteration order
        // without changing geometry) and re-route — the path must be identical.
        let mut br = buildings.clone();
        br.reverse();
        let mut rr = roads.clone();
        rr.reverse();
        let terrain2 = build_terrain(&br, &rr, 0);
        let nav2 = NavGrid::new(&br, &rr, &terrain2);
        let p2 = nav2.astar(start, goal).expect("path exists");
        assert_eq!(
            p1, p2,
            "A* path must be deterministic regardless of input order"
        );
    }

    // `terrain_at` honours an explicitly-emitted bridge even on a river column, and
    // classifies sea / river / road correctly.
    #[test]
    fn terrain_at_matches_emitted_frame() {
        let buildings = wide_city();
        let t0 = build_terrain(&buildings, &[], 0);
        let river = t0.rivers[0];
        let cross_y = t0.min_y + 1;
        let road = road_with_path(
            "r",
            vec![(river.gx_min - 2, cross_y), (river.gx_max + 2, cross_y)],
        );
        let terrain = build_terrain(&buildings, &[road.clone()], 0);
        let nav = NavGrid::new(&buildings, &[road], &terrain);

        // Road over land.
        assert_eq!(
            nav.terrain_at(Tile::new(river.gx_min - 2, cross_y)),
            Terrain::Road
        );
        // Bridge over the river.
        assert_eq!(
            nav.terrain_at(Tile::new(river.gx_min, cross_y)),
            Terrain::Bridge
        );
        // Un-bridged river tile elsewhere.
        assert_eq!(
            nav.terrain_at(Tile::new(river.gx_min, t0.min_y)),
            Terrain::River
        );
        // Open sea.
        assert_eq!(
            nav.terrain_at(Tile::new(terrain.sea_x + 1, t0.min_y)),
            Terrain::Sea
        );
    }

    // FIX 2 (defensive): the Bridge override is GATED on road membership. A tile in
    // the `bridges` set that is NOT also a routed road tile must NOT be classified
    // Bridge — by construction `bridges ⊆ road_tiles`, but a future stray bridge
    // tile off the road network must never become silently walkable.
    #[test]
    fn bridge_tile_not_in_road_set_is_not_bridge() {
        let buildings = wide_city();
        let t0 = build_terrain(&buildings, &[], 0);
        let river = t0.rivers[0];
        let cross_y = t0.min_y + 1;
        // A real crossing road so the terrain frame + a legitimate bridge exist.
        let road = road_with_path(
            "r",
            vec![(river.gx_min - 2, cross_y), (river.gx_max + 2, cross_y)],
        );
        let terrain = build_terrain(&buildings, &[road.clone()], 0);
        let mut nav = NavGrid::new(&buildings, &[road], &terrain);

        // Synthesize a stray bridge tile that is NOT a routed road tile (violating
        // the construction invariant). Pick a river column on a row no road covers.
        let stray = Tile::new(river.gx_min, t0.min_y + 3);
        assert!(
            !nav.roads.contains(&stray),
            "stray tile is not a routed road"
        );
        nav.bridges.insert(stray);

        // Because the override is road-gated, the stray bridge tile is classified by
        // its geometry (an un-bridged river column → River), NOT silently Bridge.
        assert_eq!(
            nav.terrain_at(stray),
            Terrain::River,
            "a bridge tile NOT in the road set must NOT be classified Bridge"
        );
        assert!(
            !walkable(nav.terrain_at(stray)),
            "stray bridge tile stays non-walkable"
        );

        // Sanity: the LEGITIMATE bridge (road ∩ river) is still a Bridge.
        assert_eq!(
            nav.terrain_at(Tile::new(river.gx_min, cross_y)),
            Terrain::Bridge
        );
    }
}
