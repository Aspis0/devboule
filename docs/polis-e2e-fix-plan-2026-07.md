# Polis live-e2e fix plan — 2026-07-09

Owner-reported defects from the first live e2e of the two Polis plans (phase1/infra),
mapped to root causes by two deepseek-v4-flash recon passes, split into dispatchable
tasks. Execution model (owner rules 2026-07-09):

- **Coders: mimo-v2.5 / mimo-v2.5-pro via pi** (thinking high). Claude never codes inline.
- **Per-task reviewer: deepseek-v4-pro via pi** (replaces Sonnet for step reviews).
- **Recon: deepseek-v4-flash.** Sonnet only in the final max-recall.
- **Safety (after the v4-pro `git checkout -- src/` incident):** every pi spec opens with
  a ban on state-mutating git; commit after every verified task; test-count baseline
  check after every pi run (silent-wipe detector); coders never run cargo.
- Cadence per task: commit prior phase → mimo dispatch → Claude verifies on disk +
  runs vitest/cargo (counts vs baseline) → deepseek review → mimo fix pass (same pi
  session, `-c`) → commit.

Baseline at plan start: commit `48308e4`, 3336 cargo lib, 2551 vitest (2550 + 1 new), 20 pytest.

---

## T0 — DONE (`48308e4`)
Transparent bottom-bar panels: `bg-white/97` and `bg-*/8` are dead classes in
Tailwind 3.4 (opacity scale is 5-step) → 4 of 6 panels had NO background. Fixed to
`/95` / `/10` app-wide + regression test. Filters panel now lists all 9 sin rule ids
(added complexity, god-file, test-gap, clone — pipeline was already generic).

## T1 — Review follow-ups + provider structures optional, OFF by default
**Executor: mimo-v2.5 · Reviewer: deepseek-v4-pro**

a) Micro-fixes from the T0 review: append `\u{FE0F}` (VS16) to `\u{1F3DB}` and
`\u{1F573}` glyphs (Emoji_Presentation=No); update the stale rule-id doc comment at
`src-tauri/src/polis/augure/mod.rs:42`; add one integration test: click "Complexity"
checkbox → `FilterState.categories` contains `"complexity"`.

b) External providers (scaleway/cloudflare harbour structures, `ExternalServiceLayer`)
become **opt-in, default OFF**:
- New persisted setting `polis:visibleProviders` (localStorage, same pattern as
  `polis:lastFolder`, `cityStore.ts:24-44`): `string[]`, default `[]`.
- `cityStore`: `visibleProviders` + `setProviderVisible(provider, on)`; survives
  folder switch (it's a user preference, NOT reset like `filter`).
- Renderer: `ExternalServiceLayer` visibility = LOD gate AND provider enabled
  (compose with existing `setLodVisible`, per-provider filtering of its structures).
- Legend (`PolisBottomBar.tsx` LegendOverlay): provider pennants + cloud-harbour
  sections get per-provider toggle switches; disabled providers show greyed with an
  "off" hint so users discover they can enable them. Era monuments stay always-on.
- Tests: store persistence round-trip, default-off, layer gating per provider.

## T2 — Pathing hardening: walkers never cross water/buildings, never leave roads
**Executor: mimo-v2.5-pro · Reviewer: deepseek-v4-pro** (hardest task)

Root causes (recon #2): (a) `buildSplineLeg` Catmull-Rom bows outside the road
polyline at corners (`locomotion.ts:66`), (b) `applyPerpendicularOffset` (±4px lane
shift) is unclamped, (c) the final approach leg road-end→door from
`SlotAllocator.positionFor` (`AgentLayer.ts:~909`) is unguarded, (d) no walker ever
checks building footprints (`occupiedTiles` exists only for props, `props.ts:40`).

- New shared `walkBlocked(gx,gy)` predicate module: water blocker
  (`makeWaterBlocker`, bridges walkable) ∪ building footprints (extract the
  `occupiedTiles` builder from `props.ts` into a shared helper; footprint only, not
  the 4-neighborhood — walkers may hug walls).
- Clamp at **leg-build time** (never per frame): sample the spline leg; if any sampled
  point's tile is blocked, degrade that leg to linear polyline interpolation; if the
  linear polyline itself touches a blocked tile (shouldn't — roadGraph guarantees),
  keep the previous behavior (fade-teleport for agents, idle for ambients).
- Lane offset: after offset, check the offset sample tile; if blocked, use offset 0
  for that leg.
- Final approach leg: validate the slot position tile with `walkBlocked`; fallback to
  the door tile centre.
- All layers adopt it: AmbientLayer, AgentLayer (TradeRouteLayer already exact-linear
  + edge-rejected — no change).
- Tests: spline-clamp unit tests (corner overshoot into water → linear fallback),
  lane-offset clamp, slot fallback, plus a regression test with a concave road along
  a shore.
- Perf constraint: no new per-frame allocations; predicate is a closure over Sets
  built once per city load.

## T3 — Bridges worth looking at
**Executor: mimo-v2.5 · Reviewer: deepseek-v4-pro**

`drawBridgeDeck` (`terrain.ts:312-349`) is a flat plank quad. Rebuild procedurally in
the kitcd idiom (Caesar III stone bridge as STYLE reference only — original game
assets are proprietary and are NOT copied):
- Stone arch profile on the water-facing sides (visible arch + pier shadows on water),
  parapet walls with coping stones, deck pavers matching road texture, lamp posts at
  ends reusing the light-halo pass at night (`dayPhase`).
- Multi-tile bridges get repeated arches; single-tile gets one arch.
- Static geometry drawn once per terrain frame build (existing chunked path) — zero
  per-frame cost. Deterministic via `rng.ts`.

## T4 — Walkers = real agent taxonomy (pi era) + carried package for porters
**Executor: mimo-v2.5 · Reviewer: deepseek-v4-pro**

Roster facts: `attach_agents` (`scanner.rs:5328`) emits types orchestrator / coder /
verifier / augur + verbatim pass-through for unknown roles; mini-coders are coders
with `parentAgentId`; pi-spawned sessions (`pi_sidecar::spawn_agent_session`,
`lib.rs:806`) appear with whatever role they registered; `AgentSession.model`
(`model.rs:1145`) is stored but unused.

- **Distinct figure per real agent type** (`figureForType`/`figureForAgent`,
  `AgentLayer.ts:70-108`): orchestrator→noble, main coder→builder,
  mini coder→watercarrier (existing), verifier→citizen with scroll accent, augur→NEW
  `priest` figure in `kitcd/people.ts` (long robe + laurel), unknown/pass-through
  roles→NEW `foreigner` figure (hooded traveller) so pi-external sessions are visible
  instead of invisible.
- **Provider livery**: when `agent.model` contains "mimo" → jade tunic tint;
  "deepseek" → indigo; "claude"/"sonnet"/"opus" → terracotta; else default. Requires
  plumbing `model` into `CityAgent` (Rust `attach_agents` + TS type + AgentLayer) —
  small, additive, serde camelCase.
- **Trade porters carry a visible package**: merchants' sack (`people.ts:228`) is on
  the back and reads poorly. Add a two-hands carried crate in FRONT of the figure
  (simple box with rope cross, slight bob sync with walk phase) for
  `TradeRouteLayer` porters via a `carrying: boolean` param of `drawCitizen` —
  merchant elsewhere keeps the back sack.
- Legend/help text updated (agents section) to the new taxonomy.
- Tests: figure mapping per type/role/model-livery, unknown-role → foreigner,
  drawCitizen carrying param, existing AgentLayer suites stay green.

## T5 — Fill the empty map: farmland belts, orchards, gardens
**Executor: mimo-v2.5-pro · Reviewer: deepseek-v4-pro** (biggest visual task)

Why the map is half empty (recon #2): buildings pack with GAP=3, districts spaced
DISTRICT_MARGIN=8, zero-coupling districts pushed east of the bbox
(`scanner.rs:4113`) → big bare value-noise plains; props capped at `MAX_PROPS=1500`
with ~24% fill on empty tiles; no farm/crop primitives exist.

Caesar III / Zeus farmland is the style target (reference only, procedural kitcd
drawing): cultivated plots with visible crop rows, olive orchards in grids, vineyards,
fallow fields, garden beds near houses.

- New kitcd/detail.ts primitives: `cropRows(g, proj, gx, gy, w, d, seed)` (ridged
  furrows + green rows), `vineyard` (post-and-wire rows), `orchardGrid` (olive/cypress
  grid), `fallowField` (tilled texture), `haystack`, `farmShed` (tiny non-building
  prop).
- **Field parcel layout** (new `fields.ts`): deterministically partition empty
  rectangles between district bboxes (computed frontend-side from building coords)
  into parcels of 4×3..8×6 tiles separated by dirt paths; assign each parcel a
  primitive by seeded rng weighted by distance from city centre (near = gardens,
  mid = crops/vineyards, far = orchards/fallow). Parcels never overlap roads, water,
  building footprints (+1 margin), or props.
- Rendering: parcels draw into the existing chunked static terrain Graphics
  (terrain.ts chunk path) — one-time build cost, zero per-frame cost; LOD: parcels
  visible from the mid zoom band (≥0.3), simplified fill below.
- Rebalance props: keep MAX_PROPS for scatter but exclude parcel tiles.
- Tests: parcel partition determinism, no-overlap invariants (roads/water/buildings),
  chunk build perf bound (existing perfBounds pattern), LOD gating.

## T6 — Final max-recall (whole cumulative diff T0..T5)
3 hostile reviewers on different angles (deepseek-v4-pro line-by-line, mimo-v2.5-pro
cross-file/interaction, Sonnet removed-behavior + perf/UX altitude) + adversarial
verify of every CONFIRMED finding, then fixes, then full suites (cargo, vitest,
pytest) and a fresh live-e2e checklist for the owner (panel opacity, filters hide each
sin kind, providers off by default + toggeable, no walker off-road in 5 min of
watching, bridges, farm belts, agent taxonomy + porter crates).

---

## Research notes (web, 2026-07-09)

- Open-source Caesar III remakes (Julius, Augustus, CaesarIA) are engine
  re-implementations that REQUIRE the original proprietary assets — nothing to reuse.
- CC0 isometric packs (Kenney/KayKit/Buggy Studio/OpenGameArt) are modern-city or
  low-poly 3D styles that clash with our procedural olive-palette kit. Decision: stay
  procedural; adopt Caesar III *conventions*, not assets.
- Caesar III visual conventions to encode in T2–T5 prompts: walkers NEVER leave roads
  (road-tile-locked movement is the defining look), farms are rectangular parcels of
  crop rows with a farmhouse and visible growth stages, goods move as visible carried
  loads (carts/amphorae/sacks held in front), bridges are low stone arches with piers
  and parapets, coastline gets docks/breakwater texture rather than bare water edge.

Non-goals / explicitly rejected: ripping Caesar III / Empire Earth assets (proprietary
— style reference only); grid.rs re-layout (district spacing is semantic, the fix is
filling the space, not compacting it); Censor coupling into Polis (owner rule).
