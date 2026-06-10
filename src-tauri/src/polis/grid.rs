//! Polis Map — WORLD-GRID road pathfinding (emergent street network).
//!
//! After `layout()` assigns building coords, import roads are still just
//! abstract `from`/`to` building pairs. Drawn naively they become ugly straight
//! diagonals that cut through other buildings. This module routes each road as a
//! STREET on a shared world grid (à la Caesar III / Pharaoh): an A* path that
//!
//!   1. avoids building-occupied tiles (obstacles), and
//!   2. PREFERS tiles already used by previously-routed roads (a discount), so
//!      later roads merge onto existing trunks instead of running N independent
//!      parallel paths. The result is an emergent, shared street network.
//!
//! HEURISTIC: because a discounted (shared) step can cost as little as
//! `SHARED_STEP_COST` (< `STEP_COST`), the A* heuristic is `manhattan *
//! SHARED_STEP_COST` — the MINIMUM possible step cost. Scaling by `STEP_COST`
//! would OVERESTIMATE on discounted routes (inadmissible) and yield suboptimal
//! trunk-merging; scaling by `SHARED_STEP_COST` keeps the heuristic admissible
//! (it never exceeds the true remaining cost), so A* returns provably optimal
//! discounted paths and shares trunks as aggressively as the cost model allows.
//!
//! Everything here is PURE and DETERMINISTIC (no RNG, no map-iteration-order
//! dependence): roads are processed in a stable sorted order while a shared
//! "road usage" set accumulates, so a re-scan reproduces the exact same network.
//!
//! The computed polyline (corner waypoints in WORLD/tile coords, endpoints at
//! the two building cell centers) is stored on `Road::path`. If no path is found
//! within the search budget, `path` stays `None` and the renderer falls back to
//! an honest straight line.

use crate::polis::footprint::building_footprint;
use crate::polis::model::{Building, Coords, Road};
// DETERMINISM: A* and the shared-usage set use ORDERED maps/sets (BTree*), not
// hash maps. `std::collections::HashMap`/`HashSet` use a per-process randomly
// seeded hasher (`RandomState`); even though we never iterate them for ordering,
// using ordered containers removes any doubt and guarantees byte-identical road
// networks across separate process runs (the dev-fixture dump runs in its own
// process). No RNG anywhere in this module.
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

// ---------------------------------------------------------------------------
// Tuning knobs (VISUAL TUNING — see the module/task notes).
// ---------------------------------------------------------------------------

/// Margin (in cells) added around the building bbox so roads can route slightly
/// outside the tightest hull when the direct corridor is blocked.
const GRID_MARGIN: i32 = 4;

/// Margin (in cells) added around a single road's (from,to) bounding box to
/// bound that road's A* search window. Keeps A* cheap without amputating useful
/// detours.
const SEARCH_MARGIN: i32 = 6;

/// Hard cap on A* node expansions per road. If exceeded, the road gets no grid
/// path (`None`) and the renderer falls back to a straight line — we never hang.
const MAX_EXPANSIONS: usize = 6_000;

/// Base cost of a single 4-connected step (scaled integer math, no floats in
/// the priority queue so ordering is exact and deterministic).
const STEP_COST: u32 = 100;

/// Cost of a step that lands on a tile ALREADY used by a previously-routed road.
/// Strictly less than `STEP_COST` so later roads are pulled onto existing
/// streets (shared trunks). Half cost = strong merge tendency.
const SHARED_STEP_COST: u32 = 50;

/// Upper bound on grid area (cells). Above this we skip gridding entirely and
/// leave every road as a straight-line fallback (safety on pathological inputs).
const MAX_GRID_CELLS: i64 = 4_000_000;

// ---------------------------------------------------------------------------
// Cell — an integer world-grid coordinate (a rounded tile position).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Cell {
    x: i32,
    y: i32,
}

impl Cell {
    #[inline]
    fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// Manhattan distance for 4-connected movement. The A* heuristic scales THIS
    /// by `SHARED_STEP_COST` (the minimum possible per-step cost under the
    /// shared-segment discount), which keeps the heuristic admissible —
    /// `SHARED_STEP_COST * manhattan` never exceeds the true remaining cost — so
    /// A* still returns provably optimal discounted paths.
    #[inline]
    fn manhattan(self, other: Cell) -> u32 {
        ((self.x - other.x).unsigned_abs()) + ((self.y - other.y).unsigned_abs())
    }

    #[inline]
    fn to_coords(self) -> Coords {
        Coords::new(self.x as f64, self.y as f64)
    }
}

/// Round a building's (possibly fractional) coords to its grid cell center.
#[inline]
fn cell_of(c: Coords) -> Cell {
    Cell::new(c.x.round() as i32, c.y.round() as i32)
}

/// A building's footprint in integer tiles `(W, D)`, from the kit-mirrored
/// table. Always >= 1 in each axis.
#[inline]
fn footprint_cells(b: &Building) -> (i32, i32) {
    let (fw, fd) = building_footprint(&b.purpose, &b.visual_tier);
    (fw.max(1) as i32, fd.max(1) as i32)
}

/// An axis-aligned tile rectangle `[x0, x0+w) x [y0, y0+h)` (a building's
/// footprint). Used to keep both endpoints of a road reachable even though their
/// footprints are obstacles for every OTHER road.
#[derive(Debug, Clone, Copy)]
struct Rect {
    x0: i32,
    y0: i32,
    w: i32,
    h: i32,
}

impl Rect {
    #[inline]
    fn contains(&self, c: Cell) -> bool {
        c.x >= self.x0 && c.x < self.x0 + self.w && c.y >= self.y0 && c.y < self.y0 + self.h
    }

    /// The footprint rect of a building (anchor cell = origin tile).
    fn of(b: &Building) -> Self {
        let c = cell_of(b.coords);
        let (w, h) = footprint_cells(b);
        Rect {
            x0: c.x,
            y0: c.y,
            w,
            h,
        }
    }
}

// ---------------------------------------------------------------------------
// Occupancy grid over the building bbox (+ margin).
// ---------------------------------------------------------------------------

/// A deterministic occupancy grid: a building's FULL footprint tiles are
/// obstacles (not just its single anchor cell), so roads route AROUND the real
/// building shape and thread through the GAP tiles the layout leaves between
/// buildings. The footprint comes from the same kit-mirrored table the layout
/// uses (`footprint::building_footprint`), so obstacles match the drawn art.
struct OccupancyGrid {
    min_x: i32,
    min_y: i32,
    w: i32,
    h: i32,
    occupied: Vec<bool>,
}

impl OccupancyGrid {
    /// Build the grid from all building footprints. The bbox spans every
    /// building's full footprint (origin .. origin+footprint), padded by
    /// `GRID_MARGIN`. Returns `None` if the buildings span an unreasonably large
    /// area (safety: don't allocate a giant grid).
    fn build(buildings: &[Building]) -> Option<Self> {
        if buildings.is_empty() {
            return None;
        }
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        for b in buildings {
            let c = cell_of(b.coords);
            let (fw, fd) = footprint_cells(b);
            min_x = min_x.min(c.x);
            min_y = min_y.min(c.y);
            // Footprint extends to origin + (W-1, D-1) inclusive.
            max_x = max_x.max(c.x + fw - 1);
            max_y = max_y.max(c.y + fd - 1);
        }
        min_x -= GRID_MARGIN;
        min_y -= GRID_MARGIN;
        max_x += GRID_MARGIN;
        max_y += GRID_MARGIN;

        let w = (max_x - min_x + 1).max(1);
        let h = (max_y - min_y + 1).max(1);
        if (w as i64) * (h as i64) > MAX_GRID_CELLS {
            return None;
        }

        let mut grid = OccupancyGrid {
            min_x,
            min_y,
            w,
            h,
            occupied: vec![false; (w * h) as usize],
        };

        // Mark every footprint tile of every building as an obstacle. The road
        // A* may still ENTER an endpoint building's own cells (handled in
        // `astar`, which excludes the goal), so connections stay reachable.
        for b in buildings {
            let c = cell_of(b.coords);
            let (fw, fd) = footprint_cells(b);
            for dy in 0..fd {
                for dx in 0..fw {
                    grid.set(Cell::new(c.x + dx, c.y + dy), true);
                }
            }
        }
        Some(grid)
    }

    #[inline]
    fn in_bounds(&self, c: Cell) -> bool {
        c.x >= self.min_x
            && c.y >= self.min_y
            && c.x < self.min_x + self.w
            && c.y < self.min_y + self.h
    }

    #[inline]
    fn idx(&self, c: Cell) -> usize {
        ((c.y - self.min_y) * self.w + (c.x - self.min_x)) as usize
    }

    #[inline]
    fn set(&mut self, c: Cell, val: bool) {
        if self.in_bounds(c) {
            let i = self.idx(c);
            self.occupied[i] = val;
        }
    }

    #[inline]
    fn is_occupied(&self, c: Cell) -> bool {
        self.in_bounds(c) && self.occupied[self.idx(c)]
    }
}

// ---------------------------------------------------------------------------
// A* node for the binary heap (min-heap via Reverse semantics in Ord).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct Frontier {
    /// f = g + h (lower is better).
    f: u32,
    /// g = cost so far.
    g: u32,
    cell: Cell,
}

// We want a MIN-heap on `f` (then `g`, then cell) for fully deterministic tie
// breaks. `BinaryHeap` is a max-heap, so reverse the comparison.
impl Ord for Frontier {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .f
            .cmp(&self.f)
            .then_with(|| other.g.cmp(&self.g))
            .then_with(|| other.cell.cmp(&self.cell))
    }
}
impl PartialOrd for Frontier {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// 4-connected neighbor offsets in a FIXED order (determinism).
const NEIGHBORS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// Run A* from `start` to `goal` on the grid, treating occupied cells as
/// obstacles EXCEPT the two endpoint BUILDINGS' footprints (`from_rect` /
/// `to_rect`) — you must be able to reach the buildings you connect, and with
/// full-footprint obstacles their anchor cells are otherwise walled in by their
/// own footprint tiles. Cells inside either endpoint rect are always passable;
/// every OTHER building's footprint blocks. Tiles in `usage` get a per-step
/// DISCOUNT so later roads merge onto existing streets. Search is restricted to
/// the (start,goal) bbox expanded by `SEARCH_MARGIN` and capped at
/// `MAX_EXPANSIONS`; on failure returns `None`.
fn astar(
    grid: &OccupancyGrid,
    start: Cell,
    goal: Cell,
    from_rect: Rect,
    to_rect: Rect,
    usage: &BTreeSet<Cell>,
) -> Option<Vec<Cell>> {
    if start == goal {
        return Some(vec![start]);
    }
    // A cell is walkable if it is free, OR it belongs to one of the two endpoint
    // buildings' footprints (so the road can enter/leave the buildings it joins).
    let walkable =
        |c: Cell| -> bool { !grid.is_occupied(c) || from_rect.contains(c) || to_rect.contains(c) };

    // Per-road search window (bbox of start/goal + margin), clamped to the grid.
    let win_min_x = start.x.min(goal.x) - SEARCH_MARGIN;
    let win_min_y = start.y.min(goal.y) - SEARCH_MARGIN;
    let win_max_x = start.x.max(goal.x) + SEARCH_MARGIN;
    let win_max_y = start.y.max(goal.y) + SEARCH_MARGIN;
    let in_window =
        |c: Cell| c.x >= win_min_x && c.x <= win_max_x && c.y >= win_min_y && c.y <= win_max_y;

    let mut g_score: BTreeMap<Cell, u32> = BTreeMap::new();
    let mut came_from: BTreeMap<Cell, Cell> = BTreeMap::new();
    let mut open = BinaryHeap::new();

    g_score.insert(start, 0);
    open.push(Frontier {
        // DISCOUNTED-ADMISSIBLE heuristic: scale by the MINIMUM possible step cost
        // (`SHARED_STEP_COST`), not `STEP_COST`. Because a step can cost as little
        // as `SHARED_STEP_COST` (on a shared/discounted street cell), the true
        // remaining cost is always >= `manhattan * SHARED_STEP_COST`, so this never
        // overestimates -> A* stays admissible -> provably optimal discounted paths.
        f: start.manhattan(goal) * SHARED_STEP_COST,
        g: 0,
        cell: start,
    });

    let mut expansions = 0usize;

    while let Some(Frontier { g, cell, .. }) = open.pop() {
        if cell == goal {
            return Some(reconstruct(&came_from, goal));
        }
        // Skip stale heap entries (a better g was already recorded).
        if g > *g_score.get(&cell).unwrap_or(&u32::MAX) {
            continue;
        }
        expansions += 1;
        if expansions > MAX_EXPANSIONS {
            return None; // budget exhausted — honest straight-line fallback
        }

        for (dx, dy) in NEIGHBORS {
            let n = Cell::new(cell.x + dx, cell.y + dy);
            if !in_window(n) {
                continue;
            }
            // Obstacles block, EXCEPT cells inside either endpoint building's
            // footprint (so the road can reach the buildings it connects).
            // `start` is never re-entered (it is already closed with g=0).
            if !walkable(n) {
                continue;
            }
            // Shared-segment discount: stepping onto an existing street is cheaper.
            let step = if usage.contains(&n) {
                SHARED_STEP_COST
            } else {
                STEP_COST
            };
            let tentative_g = g.saturating_add(step);
            if tentative_g < *g_score.get(&n).unwrap_or(&u32::MAX) {
                came_from.insert(n, cell);
                g_score.insert(n, tentative_g);
                // Discounted-admissible heuristic (see `start` push): the minimum
                // per-step cost is `SHARED_STEP_COST`, so this lower-bounds the true
                // remaining cost and never overestimates.
                let f = tentative_g.saturating_add(n.manhattan(goal) * SHARED_STEP_COST);
                open.push(Frontier {
                    f,
                    g: tentative_g,
                    cell: n,
                });
            }
        }
    }
    None
}

/// Walk `came_from` back from `goal` to the start, producing start..=goal order.
fn reconstruct(came_from: &BTreeMap<Cell, Cell>, goal: Cell) -> Vec<Cell> {
    let mut path = vec![goal];
    let mut cur = goal;
    while let Some(&prev) = came_from.get(&cur) {
        path.push(prev);
        cur = prev;
    }
    path.reverse();
    path
}

/// Collapse colinear runs of cells into corner waypoints (endpoints always
/// kept). The renderer then draws straight segments between corners instead of
/// one vertex per cell. Pure; preserves order.
fn simplify(cells: &[Cell]) -> Vec<Cell> {
    if cells.len() <= 2 {
        return cells.to_vec();
    }
    let mut out = Vec::with_capacity(cells.len());
    out.push(cells[0]);
    for i in 1..cells.len() - 1 {
        let a = cells[i - 1];
        let b = cells[i];
        let c = cells[i + 1];
        // Keep `b` only if direction changes (a->b not colinear with b->c).
        let d1 = (b.x - a.x, b.y - a.y);
        let d2 = (c.x - b.x, c.y - b.y);
        if d1 != d2 {
            out.push(b);
        }
    }
    out.push(cells[cells.len() - 1]);
    out
}

// ---------------------------------------------------------------------------
// Public entry — route all import roads on a shared world grid.
// ---------------------------------------------------------------------------

/// Statistics returned by [`route_roads`] for reporting/diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RouteStats {
    /// Number of roads that received a grid path.
    pub routed: usize,
    /// Number of roads left as a straight-line fallback (path = None).
    pub fallback: usize,
    /// Total waypoints across all routed roads (for an average).
    pub total_waypoints: usize,
}

/// Route every road as a street on a shared world grid, filling `Road::path`.
///
/// DETERMINISM: roads are sorted by a stable key (`(from, to, road_id)`) and
/// processed in that fixed order while a shared `usage` set accumulates the
/// cells of already-routed roads. Same input -> identical paths every run.
///
/// Endpoints of each stored path are the two building CELL CENTERS (so the road
/// visibly meets the buildings). Building-occupied cells are obstacles except a
/// road's own two endpoints. Roads whose A* fails within budget keep `path =
/// None` (renderer falls back to a straight line).
pub fn route_roads(buildings: &[Building], roads: &mut [Road]) -> RouteStats {
    let mut stats = RouteStats::default();

    let grid = match OccupancyGrid::build(buildings) {
        Some(g) => g,
        None => {
            // No grid (empty / pathological) — everything is a straight fallback.
            for r in roads.iter_mut() {
                r.path = None;
            }
            stats.fallback = roads.len();
            return stats;
        }
    };

    // building file_id -> its footprint rect (anchor cell = rect origin).
    // (BTreeMap: ordered, hash-seed-free.)
    let mut rect_by_id: BTreeMap<&str, Rect> = BTreeMap::new();
    for b in buildings {
        rect_by_id.insert(b.file_id.as_str(), Rect::of(b));
    }

    // STABLE processing order: sort road indices by (from, to, road_id). We
    // route in this fixed order so the shared-usage discount is deterministic.
    let mut order: Vec<usize> = (0..roads.len()).collect();
    order.sort_by(|&a, &b| {
        (&roads[a].from, &roads[a].to, &roads[a].road_id).cmp(&(
            &roads[b].from,
            &roads[b].to,
            &roads[b].road_id,
        ))
    });

    // Shared "road usage" set — cells used by previously-routed roads. Later
    // roads get the discount on these so they merge into shared trunks.
    // (BTreeSet: ordered, hash-seed-free — see the module-level use comment.)
    let mut usage: BTreeSet<Cell> = BTreeSet::new();

    for idx in order {
        let (from_rect, to_rect) = match (
            rect_by_id.get(roads[idx].from.as_str()),
            rect_by_id.get(roads[idx].to.as_str()),
        ) {
            (Some(&f), Some(&t)) => (f, t),
            // Endpoint missing (should not happen for import roads) — fallback.
            _ => {
                roads[idx].path = None;
                stats.fallback += 1;
                continue;
            }
        };
        let from_cell = Cell::new(from_rect.x0, from_rect.y0);
        let to_cell = Cell::new(to_rect.x0, to_rect.y0);

        match astar(&grid, from_cell, to_cell, from_rect, to_rect, &usage) {
            Some(cells) => {
                // Accumulate the FULL cell run into shared usage (so the next
                // road can merge anywhere along this street, not just corners).
                for &c in &cells {
                    usage.insert(c);
                }
                let simplified = simplify(&cells);
                let path: Vec<Coords> = simplified.iter().map(|c| c.to_coords()).collect();
                stats.total_waypoints += path.len();
                stats.routed += 1;
                roads[idx].path = Some(path);
            }
            None => {
                roads[idx].path = None;
                stats.fallback += 1;
            }
        }
    }

    stats
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    // The vocabulary constant modules (`purpose`, `visual_tier`, ...) live in
    // `crate::polis::model`; bring them into the test scope explicitly.
    use crate::polis::model::{
        building_status, purpose, purpose_source, road_style, road_type, visual_tier,
    };

    fn mk_building(id: &str, x: f64, y: f64, tier: &str) -> Building {
        Building {
            file_id: id.into(),
            file_path: format!("src/{id}.ts"),
            district_id: String::new(),
            purpose: purpose::HOUSE.into(),
            purpose_source: purpose_source::DEFAULT.into(),
            feature_id: String::new(),
            feature_source: String::new(),
            provider: None,
            lines_of_code: 50,
            visual_tier: tier.into(),
            coords: Coords::new(x, y),
            status: building_status::NORMAL.into(),
            label: id.into(),
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

    fn mk_road(id: &str, from: &str, to: &str) -> Road {
        Road {
            road_id: id.into(),
            from: from.into(),
            to: to.into(),
            road_type: road_type::IMPORT.into(),
            style: road_style::LASTRICATA.into(),
            weight: 1,
            path: None,
        }
    }

    /// A* finds a path around an obstacle.
    #[test]
    fn astar_routes_around_obstacle() {
        // Buildings at (0,0) and (4,0); obstacle wall at x=2 for y in -1..=1.
        // (We model the wall as occupied cells via extra buildings.)
        let buildings = vec![
            mk_building("a", 0.0, 0.0, visual_tier::KALYBE),
            mk_building("b", 4.0, 0.0, visual_tier::KALYBE),
            mk_building("w1", 2.0, -1.0, visual_tier::KALYBE),
            mk_building("w2", 2.0, 0.0, visual_tier::KALYBE),
            mk_building("w3", 2.0, 1.0, visual_tier::KALYBE),
        ];
        let grid = OccupancyGrid::build(&buildings).unwrap();
        let start = Cell::new(0, 0);
        let goal = Cell::new(4, 0);
        // 1x1 endpoint rects (kalybe house footprint = 1x1).
        let sr = Rect {
            x0: 0,
            y0: 0,
            w: 1,
            h: 1,
        };
        let gr = Rect {
            x0: 4,
            y0: 0,
            w: 1,
            h: 1,
        };
        let path = astar(&grid, start, goal, sr, gr, &BTreeSet::new()).expect("path exists");
        assert_eq!(path.first().copied(), Some(start));
        assert_eq!(path.last().copied(), Some(goal));
        // The direct cell (2,0) is occupied (non-endpoint) — must be avoided.
        assert!(
            !path.contains(&Cell::new(2, 0)),
            "path must route around the wall"
        );
    }

    /// A routed road's path avoids occupied (non-endpoint) cells.
    #[test]
    fn routed_path_avoids_occupied_non_endpoint_cells() {
        let buildings = vec![
            mk_building("a", 0.0, 0.0, visual_tier::KALYBE),
            mk_building("b", 4.0, 0.0, visual_tier::KALYBE),
            mk_building("obstacle", 2.0, 0.0, visual_tier::KALYBE),
        ];
        let mut roads = vec![mk_road("r0", "a", "b")];
        route_roads(&buildings, &mut roads);
        let path = roads[0].path.as_ref().expect("road routed");
        // The obstacle building cell (2,0) must not be a waypoint.
        // (Endpoints are a=(0,0) and b=(4,0).)
        let occupied = Cell::new(2, 0);
        // Reconstruct dense cells from corner waypoints to be thorough: check no
        // corner equals the obstacle. Since corners are a subset, and the dense
        // A* avoided it, corners cannot equal it either.
        for p in path {
            assert_ne!(cell_of(*p), occupied, "no waypoint on the obstacle cell");
        }
    }

    /// Shared-discount makes two roads with nearby endpoints SHARE cells.
    #[test]
    fn shared_discount_merges_streets() {
        // Three buildings in a row: hub at (0,0), and two leaves at (6,2),(6,-2).
        // Roads hub->leaf1 and hub->leaf2 should share the trunk near the hub.
        let buildings = vec![
            mk_building("hub", 0.0, 0.0, visual_tier::KALYBE),
            mk_building("leaf1", 6.0, 2.0, visual_tier::KALYBE),
            mk_building("leaf2", 6.0, -2.0, visual_tier::KALYBE),
        ];
        let mut roads = vec![mk_road("r0", "hub", "leaf1"), mk_road("r1", "hub", "leaf2")];
        route_roads(&buildings, &mut roads);

        // Densify each road's polyline back to cells and compare overlap.
        let cells0 = densify(roads[0].path.as_ref().unwrap());
        let cells1 = densify(roads[1].path.as_ref().unwrap());
        let set0: BTreeSet<Cell> = cells0.into_iter().collect();
        let overlap = densify_overlap(&set0, roads[1].path.as_ref().unwrap());
        assert!(
            overlap > 0,
            "roads with a shared hub must share >0 cells; got {overlap}"
        );
        // Sanity: leaf2 path is non-trivial.
        assert!(roads[1].path.as_ref().unwrap().len() >= 2);
        let _ = set0;
        let _ = cells1;
    }

    /// Determinism: same input -> identical paths, run twice.
    #[test]
    fn routing_is_deterministic() {
        let make = || {
            vec![
                mk_building("a", 0.0, 0.0, visual_tier::KALYBE),
                mk_building("b", 5.0, 3.0, visual_tier::KALYBE),
                mk_building("c", 3.0, 6.0, visual_tier::KALYBE),
                mk_building("d", 8.0, 1.0, visual_tier::KALYBE),
            ]
        };
        let mk_roads = || {
            vec![
                mk_road("r0", "a", "b"),
                mk_road("r1", "a", "c"),
                mk_road("r2", "b", "d"),
            ]
        };
        let b1 = make();
        let mut r1 = mk_roads();
        route_roads(&b1, &mut r1);

        let b2 = make();
        let mut r2 = mk_roads();
        route_roads(&b2, &mut r2);

        for (x, y) in r1.iter().zip(r2.iter()) {
            assert_eq!(x.path, y.path, "routing must be deterministic");
        }
    }

    /// Determinism under PRESSURE: a dense grid of buildings + a hub-and-spoke
    /// road set (lots of equal-cost A* ties and heavy shared-usage merging),
    /// routed 5 times. Any heap-tie / hashmap-iteration nondeterminism would
    /// surface here as differing paths between runs.
    #[test]
    fn routing_is_deterministic_under_pressure() {
        // 7x7 lattice of buildings (some cells deliberately collide-adjacent),
        // with a central hub every leaf connects to (forces shared trunks) plus
        // ring roads (many equal-cost detours around obstacles).
        let make = || {
            let mut v = Vec::new();
            for gy in 0..7 {
                for gx in 0..7 {
                    let id = format!("b{gx}_{gy}");
                    // 2-tile spacing so there are free corridors AND obstacles.
                    v.push(mk_building(
                        &id,
                        (gx * 2) as f64,
                        (gy * 2) as f64,
                        visual_tier::KALYBE,
                    ));
                }
            }
            v
        };
        let mk_roads = || {
            let mut v = Vec::new();
            let hub = "b3_3";
            let mut n = 0;
            for gy in 0..7 {
                for gx in 0..7 {
                    let id = format!("b{gx}_{gy}");
                    if id != hub {
                        v.push(mk_road(&format!("r{n}"), hub, &id));
                        n += 1;
                    }
                }
            }
            // A few ring roads with many equal-cost routings.
            v.push(mk_road(&format!("r{n}"), "b0_0", "b6_6"));
            v.push(mk_road(&format!("r{}", n + 1), "b6_0", "b0_6"));
            v
        };

        // Route once as the reference.
        let b0 = make();
        let mut ref_roads = mk_roads();
        route_roads(&b0, &mut ref_roads);
        let reference: Vec<_> = ref_roads.iter().map(|r| r.path.clone()).collect();

        // Route 5 more times; every run must reproduce the reference exactly.
        for run in 0..5 {
            let b = make();
            let mut roads = mk_roads();
            route_roads(&b, &mut roads);
            for (i, r) in roads.iter().enumerate() {
                assert_eq!(
                    r.path, reference[i],
                    "run {run} road {} ({}->{}) diverged — routing must be deterministic",
                    r.road_id, r.from, r.to
                );
            }
        }
        // Sanity: the stress set actually routes most roads (not all-fallback).
        let routed = ref_roads.iter().filter(|r| r.path.is_some()).count();
        assert!(routed > 0, "stress set should route at least some roads");
    }

    /// No-path within budget -> None fallback (and never panics).
    #[test]
    fn unreachable_endpoint_falls_back_to_none() {
        // Box `b` in by surrounding all 4 neighbors with obstacle buildings so
        // it is fully enclosed and unreachable from `a`.
        let buildings = vec![
            mk_building("a", 0.0, 0.0, visual_tier::KALYBE),
            mk_building("b", 5.0, 0.0, visual_tier::KALYBE),
            mk_building("e", 6.0, 0.0, visual_tier::KALYBE),
            mk_building("w", 4.0, 0.0, visual_tier::KALYBE),
            mk_building("n", 5.0, 1.0, visual_tier::KALYBE),
            mk_building("s", 5.0, -1.0, visual_tier::KALYBE),
        ];
        let mut roads = vec![mk_road("r0", "a", "b")];
        let stats = route_roads(&buildings, &mut roads);
        assert!(roads[0].path.is_none(), "enclosed target -> None fallback");
        assert_eq!(stats.fallback, 1);
        assert_eq!(stats.routed, 0);
    }

    /// Colinear simplification reduces vertex count vs the dense cell run.
    #[test]
    fn simplify_collapses_colinear_runs() {
        // A straight east run of 6 cells should collapse to 2 corners.
        let cells: Vec<Cell> = (0..6).map(|x| Cell::new(x, 0)).collect();
        let simplified = simplify(&cells);
        assert_eq!(simplified.len(), 2, "straight run -> 2 endpoints only");
        assert_eq!(simplified[0], Cell::new(0, 0));
        assert_eq!(simplified[1], Cell::new(5, 0));

        // An L-shape (east then north) collapses to 3 corners.
        let mut l = vec![Cell::new(0, 0), Cell::new(1, 0), Cell::new(2, 0)];
        l.push(Cell::new(2, 1));
        l.push(Cell::new(2, 2));
        let s2 = simplify(&l);
        assert_eq!(s2.len(), 3, "L-shape -> 3 corners");
        assert_eq!(s2[1], Cell::new(2, 0), "the corner is preserved");
    }

    /// A routed path's endpoints are the building cell centers, and it has >=2
    /// waypoints. Also checks the routed stats average is sane.
    #[test]
    fn path_endpoints_are_building_cells() {
        let buildings = vec![
            mk_building("a", 1.0, 1.0, visual_tier::KALYBE),
            mk_building("b", 7.0, 4.0, visual_tier::KALYBE),
        ];
        let mut roads = vec![mk_road("r0", "a", "b")];
        let stats = route_roads(&buildings, &mut roads);
        let path = roads[0].path.as_ref().unwrap();
        assert!(path.len() >= 2);
        assert_eq!(cell_of(path[0]), Cell::new(1, 1));
        assert_eq!(cell_of(path[path.len() - 1]), Cell::new(7, 4));
        assert_eq!(stats.routed, 1);
        assert!(stats.total_waypoints >= 2);
    }

    // ---- test helpers ----

    /// Sign of an `i32` as `-1`/`0`/`1` (avoids any `signum` resolution quirk).
    fn sign(v: i32) -> i32 {
        match v.cmp(&0) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    }

    /// Expand a corner-polyline back into the dense set of cells it passes
    /// through (each consecutive corner pair is colinear by construction).
    fn densify(path: &[Coords]) -> Vec<Cell> {
        let mut out = Vec::new();
        if path.is_empty() {
            return out;
        }
        out.push(cell_of(path[0]));
        for w in path.windows(2) {
            let a = cell_of(w[0]);
            let b = cell_of(w[1]);
            let dx = sign(b.x - a.x);
            let dy = sign(b.y - a.y);
            let mut cur = a;
            while cur != b {
                cur = Cell::new(cur.x + dx, cur.y + dy);
                out.push(cur);
            }
        }
        out
    }

    /// Count how many densified cells of `path` are in `set`.
    fn densify_overlap(set: &BTreeSet<Cell>, path: &[Coords]) -> usize {
        densify(path)
            .into_iter()
            .filter(|c| set.contains(c))
            .count()
    }
}
