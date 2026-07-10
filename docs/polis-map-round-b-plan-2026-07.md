# Polis round B — map characterization (2026-07-09)

Owner feedback after the sprite-art round (A0–A8): the map is much better, but
(1) water/rivers still look bad, (2) the countryside lacks forests/mines/character,
(3) houses may be too far apart inside a district, (4) lighthouse/harbor buildings
sit mid-map with no relation to water, (5) inter-district roads are semantic
(import edges) and must be clickable, (6) providers stay opt-in (confirmed).

Everything clickable added this round opens a small explanation panel (same
callback→React pattern as building selection). Art: no new downloads — SBS
Elements water textures (CC0) + UH mine/quarry/rock sprites (CC-BY-SA), both
already vendored, ledgered sources.

## B1 — Real water (TS, visual loop in the dev harness)

Today (`terrain.ts`): flat single-color diamonds + sine-line shimmer. Replace with:

- `tex:water` + `tex:waterdeep` singles from SBS Elements (17/19), recolored in
  normalize (hue toward the palette's sea blue, desaturated) — repeating fills
  need standalone pow2 singles, `textureSpace:"global"`.
- ONE full-water-bounds `TilingSprite` per water body class, masked by the union
  of water diamonds (one Graphics mask total, not per chunk), `tilePosition`
  drifted in the ticker (UV offset — allocation-free, no geometry rebuild).
  Second TilingSprite (deep texture, low alpha) drifting the opposite way for
  parallax. Depth read = deep overlay alpha by `deep` flag diamonds.
- Shore: sand diamonds get a textured fill; foam = static light stroke at the
  water/sand boundary + existing shimmer retuned as foam glints.
- Fallback `?sprites=0`: keep the current flat fills.

Verify: harness see-fix-see (mandatory), perf budget — steady-state frame must
stay ≤ 10.5ms (baseline 9.5ms); masks: exactly 1–2 stencil masks total.

## B2 — Water-affine lighthouse/harbor + rivers near ports (Rust, pi coder)

- `scanner.rs`/`terrain.rs`: river channel choice prefers lanes adjacent to
  districts containing `harbor`/`lighthouse` purpose buildings.
- After `pack_district`, snap harbor/lighthouse placements to the district edge
  facing the nearest water (sea east or river lane): swap with an equal-size
  reserved cell on that edge when one exists, else leave (never overlap).
- Deterministic; regen `polis-dev-city.json` fixture afterwards.

## B3 — Forests (decor) + quarries/mines (semantic-lite, clickable)

Rust: per-folder static-asset census while scanning (ext groups: images
png/jpg/jpeg/webp/gif/svg/ico; fonts ttf/otf/woff/woff2; media mp3/wav/ogg/mp4/webm).
Threshold ≥ 8 assets → district gets a `resource` site: `quarry` (images-heavy)
or `mine` (mixed/fonts/media). New `CityState.resources` records
`{ id, districtId (folder), kind, gx, gy, counts by group }`, placed in the
countryside just outside the owning district, clear of roads/fields/water/buildings.

TS: new sprite pipeline spec (UH: `as_mine5x5` = mine, `as_stonedeposit0` +
`as_stone_pit0` = quarry, `as_rock0..4` ambient rocks; scale ×1.5 like trees,
ledger entries + CREDITS). Rendered on the props/terrain band, y-sorted,
**clickable**: `pointertap` → `onSelectResource(resource)` → React mini panel
("Quarry of <folder> — N static assets: 40 png, 12 svg, 3 fonts…").

Forests: TS-only decor in `props.ts` — 3–5 seeded forest patches per map in the
16-phase lattice scan (density boost inside patch radius, existing UH tree
sprites, respect TREE_CLEARANCE and field/road avoidance). Not clickable.

## B4 — District compactness A/B (Rust one-liner + harness verdict)

`GAP 3 → 2` (reserved cell = footprint + GAP; internal A* streets survive at 2).
`DISTRICT_MARGIN` stays 8 so inter-district semantic roads stay readable.
Regen fixture, screenshot same viewport before/after, owner picks.

## B5 — Clickable inter-district roads (TS, pi coder)

Roads already carry `from`/`to` fileIds, `weight`, `provenance`. Add hit
containers along road polylines (segment-buffer hitArea polygon, `eventMode:
"static"`, cursor pointer) for roads whose endpoints live in different districts;
`pointertap` → existing `onSelectConnection(from, to)` channel (porters already
use it) → panel: the two files/folders, import weight, provenance. Hover:
highlight the polyline (tint overlay redraw of just that road). Hit layer must
not steal building taps (roads sit below buildings; stopPropagation only on hit).

## Process

Same rails as round A: ALL coding via pi coders — hy3:free while the free daily
quota lasts, mimo-v2.5 fallback (thinking high where supported) — with the
git-ban preamble; per-step reviews deepseek-v4-pro (thinking high); recon
deepseek-v4-flash. Fable only orchestrates, runs suites, verifies in the harness,
and routes fix-passes back to the author's pi session. Commit per phase,
test-count baseline check after every pi task, max-recall fleet at the end
(1 Sonnet + 1 deepseek-v4-pro + 2 coders) + adversarial verify. Branch
`phase1/infra`, owner pushes.
