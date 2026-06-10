//! Polis Map — building footprints (the SINGLE source of truth, mirrored from
//! the frontend Claude Design kit).
//!
//! The building art (`src/components/polis/kitcd/buildings.ts`) draws each
//! building at a tile footprint `[W, D]` that varies by `purpose` (the builder)
//! AND `visual_tier` (the `L` level 0..=4). The scanner's `layout()` MUST know
//! these real footprints so it can pack buildings without overlap and route
//! roads through the gaps. The kit is the source of truth; the table below is a
//! 1:1 mirror of the `sizes`/`foot` arrays in each kit builder.
//!
//! Tier -> kit level mapping:
//!   kalybe = L0, oikia = L1, synoikia = L2, megaron = L3, mnemeion = L4.
//!
//! Any unknown purpose slug OR unknown tier falls back to `(1, 1)` — the same
//! footprint the kit's `unknown` builder draws — so an Oracle-introduced purpose
//! never panics the layout (it just gets a 1x1 cell).
//!
//! NOTE ON `conduit`: the kit's conduit builder uses `len = [2,3,3,4,5][L]`
//! (footprint `[1, len]`). The original task brief quoted `len = [3,4,5,6,8][L]`,
//! but the brief explicitly says "the kit wins" on any discrepancy, so we mirror
//! the KIT values here. (See the deviation note in the handoff report.)

use crate::polis::model::visual_tier;

/// Tier slug -> kit level index `L` (0..=4). Unknown tiers map to `0` (smallest)
/// — a conservative default that never over-claims space.
fn tier_level(tier: &str) -> usize {
    match tier {
        visual_tier::KALYBE => 0,
        visual_tier::OIKIA => 1,
        visual_tier::SYNOIKIA => 2,
        visual_tier::MEGARON => 3,
        visual_tier::MNEMEION => 4,
        _ => 0,
    }
}

/// Real tile footprint `(W, D)` for a building of the given `purpose` and
/// `visual_tier`, mirroring the frontend kit's builder `sizes`/`foot` arrays
/// 1:1. Pure, total, deterministic. Unknown slug/tier -> `(1, 1)`.
///
/// `W` is the extent along the building's local +x (cart-x) and `D` along local
/// +y (cart-y); the renderer anchors the kit's local origin `(0,0)` at
/// `cartToIso(coords)`, so a building placed at `coords` occupies the tile span
/// `[coords.x, coords.x + W) x [coords.y, coords.y + D)`.
pub fn building_footprint(purpose: &str, tier: &str) -> (u32, u32) {
    let l = tier_level(tier);

    // Each row is the kit builder's `sizes`/`foot` array, indexed by level L.
    // (W, D) per level 0..=4.
    let table: [(u32, u32); 5] = match purpose {
        // temple: L0[2,3] L1[2,3] L2[3,4] L3[3,5] L4[4,6]
        "temple" => [(2, 3), (2, 3), (3, 4), (3, 5), (4, 6)],
        // house: L0[1,1] L1[1,1] L2[2,2] L3[2,2] L4[3,3]
        "house" => [(1, 1), (1, 1), (2, 2), (2, 2), (3, 3)],
        // fortress: L0[2,2] L1[2,2] L2[3,3] L3[3,4] L4[4,4]
        "fortress" => [(2, 2), (2, 2), (3, 3), (3, 4), (4, 4)],
        // tower: L0[1,1] L1[1,1] L2[1,1] L3[2,2] L4[2,2]
        "tower" => [(1, 1), (1, 1), (1, 1), (2, 2), (2, 2)],
        // lighthouse: all [2,2]
        "lighthouse" => [(2, 2), (2, 2), (2, 2), (2, 2), (2, 2)],
        // market: L0[2,2] L1[2,3] L2[3,3] L3[3,4] L4[4,4]
        "market" => [(2, 2), (2, 3), (3, 3), (3, 4), (4, 4)],
        // warehouse: L0[2,2] L1[2,3] L2[3,3] L3[4,3] L4[4,4]
        "warehouse" => [(2, 2), (2, 3), (3, 3), (4, 3), (4, 4)],
        // workshop: L0[1,1] L1[2,2] L2[2,2] L3[3,2] L4[3,3]
        "workshop" => [(1, 1), (2, 2), (2, 2), (3, 2), (3, 3)],
        // conduit: [1, len], len = [2,3,3,4,5][L]  (KIT values — kit wins).
        "conduit" => [(1, 2), (1, 3), (1, 3), (1, 4), (1, 5)],
        // baths: L0[2,2] L1[2,3] L2[3,3] L3[3,4] L4[4,4]
        "baths" => [(2, 2), (2, 3), (3, 3), (3, 4), (4, 4)],
        // theater: L0[3,2] L1[3,3] L2[4,3] L3[4,4] L4[5,4]
        "theater" => [(3, 2), (3, 3), (4, 3), (4, 4), (5, 4)],
        // harbor: L0[2,2] L1[3,2] L2[3,3] L3[4,3] L4[4,4]
        "harbor" => [(2, 2), (3, 2), (3, 3), (4, 3), (4, 4)],
        // library: L0[2,2] L1[3,2] L2[3,3] L3[4,3] L4[4,3]
        "library" => [(2, 2), (3, 2), (3, 3), (4, 3), (4, 3)],
        // townhall: L0[2,2] L1[3,3] L2[3,3] L3[4,4] L4[4,5]
        "townhall" => [(2, 2), (3, 3), (3, 3), (4, 4), (4, 5)],
        // unknown + any Oracle-introduced slug -> [1,1] at every level.
        _ => [(1, 1), (1, 1), (1, 1), (1, 1), (1, 1)],
    };

    table[l]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin a representative set of values straight from the kit so a future
    /// drift in either the kit OR this table is caught.
    #[test]
    fn building_footprint_matches_kit_table() {
        // temple grows from 2x3 (kalybe) to 4x6 (mnemeion) — the biggest.
        assert_eq!(building_footprint("temple", visual_tier::KALYBE), (2, 3));
        assert_eq!(building_footprint("temple", visual_tier::MNEMEION), (4, 6));
        // house: tiny 1x1 kalybe, up to 3x3 courtyard at mnemeion.
        assert_eq!(building_footprint("house", visual_tier::KALYBE), (1, 1));
        assert_eq!(building_footprint("house", visual_tier::SYNOIKIA), (2, 2));
        assert_eq!(building_footprint("house", visual_tier::MNEMEION), (3, 3));
        // lighthouse: always 2x2 regardless of tier.
        for t in [
            visual_tier::KALYBE,
            visual_tier::OIKIA,
            visual_tier::SYNOIKIA,
            visual_tier::MEGARON,
            visual_tier::MNEMEION,
        ] {
            assert_eq!(building_footprint("lighthouse", t), (2, 2));
        }
        // tower stays 1x1 until megaron, then 2x2.
        assert_eq!(building_footprint("tower", visual_tier::SYNOIKIA), (1, 1));
        assert_eq!(building_footprint("tower", visual_tier::MEGARON), (2, 2));
        // conduit is a long 1-wide aqueduct; KIT len = [2,3,3,4,5].
        assert_eq!(building_footprint("conduit", visual_tier::KALYBE), (1, 2));
        assert_eq!(building_footprint("conduit", visual_tier::MNEMEION), (1, 5));
        // theater is wide (W>=3 even at the smallest tier).
        assert_eq!(building_footprint("theater", visual_tier::KALYBE), (3, 2));
        assert_eq!(building_footprint("theater", visual_tier::MNEMEION), (5, 4));
        // townhall mnemeion is the deepest civic block.
        assert_eq!(
            building_footprint("townhall", visual_tier::MNEMEION),
            (4, 5)
        );
    }

    #[test]
    fn unknown_slug_and_tier_fall_back_to_one_by_one() {
        // Unknown purpose slug -> 1x1 at every tier.
        assert_eq!(building_footprint("unknown", visual_tier::KALYBE), (1, 1));
        assert_eq!(building_footprint("unknown", visual_tier::MNEMEION), (1, 1));
        // An Oracle-introduced slug we don't know -> 1x1.
        assert_eq!(
            building_footprint("laboratory", visual_tier::MEGARON),
            (1, 1)
        );
        // Unknown tier on a known slug -> level 0 (smallest), never panics.
        assert_eq!(building_footprint("temple", "weird_tier"), (2, 3));
    }
}
