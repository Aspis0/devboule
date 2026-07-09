# Polis live-e2e fix plan — 2026-07-09

Owner-reported defects from the first live e2e of the two Polis plans (phase1/infra),
mapped to root causes by two deepseek-v4-flash recon passes, split into dispatchable
tasks.

**Division of labour (owner-approved):** a **Sonnet orchestrator executes T1→T4** by
following the runbook below, commits each task, and **STOPS after T4** with a handoff
report. **Fable executes T5 and the final max-recall**, and may declare an **optional
discretionary T6 polish round** if the end result is still visually unsatisfying.

Baseline at plan start: commit `48308e4` (T0), 3336 cargo lib, ~2551 vitest, 20 pytest.

---

# ORCHESTRATOR RUNBOOK (read this first, follow it verbatim)

## Roles
- **All coding goes to pi coders.** The orchestrator NEVER writes production code
  inline — not even one-liners. Coder model: `mimo-v2.5` (or `mimo-v2.5-pro` where a
  task says so). Reviewer model: `deepseek-v4-pro`. Both via the `pi` CLI.
- The orchestrator itself only: writes task-spec files, runs pi, reads reports,
  verifies files on disk, runs vitest/cargo, arbitrates review findings, commits.

## pi commands (exact — do not improvise flags)
From the repo root, prompt ALWAYS via `$(cat file)`, ALWAYS `< /dev/null`, stdout
ALWAYS redirected to a file you then Read (terminal stdout is untrusted here). NEVER
run pi with run_in_background.

```sh
# coder (fresh task)
pi -ne --provider xiaomi-token-plan-sgp --model "mimo-v2.5" --thinking high \
  -t read,bash,edit,write -p "$(cat SPEC.md)" < /dev/null > OUT.md 2>&1
# coder, harder task
... --model "mimo-v2.5-pro" ...
# reviewer (read-only tools!)
pi -ne --provider deepseek --model "deepseek-v4-pro" --thinking high \
  -t read,bash -p "$(cat REVIEW-SPEC.md)" < /dev/null > REVIEW-OUT.md 2>&1
# fix pass — SAME session as the original author, from the same cwd
pi -ne -c --provider xiaomi-token-plan-sgp --model "mimo-v2.5" --thinking high \
  -t read,bash,edit,write -p "$(cat FIXES.md)" < /dev/null > OUT2.md 2>&1
```
Thinking is ALWAYS `high`. If the 10-min Bash timeout cuts a run, resume with
`-c ... -p "Continue exactly where you left off"`. Put spec files in the scratchpad
dir, not in the repo.

## Safety preamble — PREPEND VERBATIM to every spec file (coder AND reviewer)
A deepseek pi task once ran `git checkout -- src/` mid-task and silently wiped ~600
lines of uncommitted work. Hence:

> ABSOLUTE BAN: never run state-mutating git commands (checkout, restore, stash,
> reset, clean, commit, push) and never delete/revert files you did not create in
> this task. The dirty working tree is intentional. Read-only git (status, diff,
> log, show) is allowed. Do NOT run `cargo` (cold compile exceeds your timeout) and
> do NOT run the full vitest suite; you may run targeted tests only:
> `npx vitest run <specific-file>`.

## Known coder pitfalls — paste this list into every CODER spec
- mimo loses earlier edits when it rewrites the same file region across fix passes:
  after EVERY fix pass, re-verify EVERY previously-completed item of that task on
  disk (grep for each), not just the item being fixed.
- Emoji/unicode in source must be written as `\u{...}` escapes, never literal glyphs
  (mimo emits mojibake literals).
- Do not edit files via Python scripts (quote-style mismatches corrupt edits) — use
  your editor tools directly.
- Follow existing code idioms; no `Math.random()`/`Date.now()` in Polis rendering
  code — use `rng.ts` helpers (`hashString`, `rngFromString`, `rngFromCoords`,
  `valueNoise`).
- Write tests in the same style as the neighbouring `*.test.ts` files.

## Per-task cadence (repeat for each of T1..T4)
1. Confirm `git status --porcelain` is CLEAN (previous task committed).
2. Write the coder spec file (safety preamble + pitfalls + the task section below,
   copied whole) to the scratchpad. Dispatch the coder (command above).
3. Read the coder's OUT file. Then verify GROUND TRUTH on disk: grep each claimed
   change in the actual files. `git status --porcelain` must list ONLY expected
   files — anything reverted/missing ⇒ treat as wipe, `git checkout` is banned for
   pi but YOU may restore from git since everything is committed.
4. Run the task's test commands (listed per task). Also run the SILENT-WIPE CHECK:
   full `npx vitest run` count must be ≥ the baseline at the top of this doc plus
   all tests added so far; cargo: `cargo test --lib` in `src-tauri` run BY YOU (the
   orchestrator), only for tasks that touch Rust.
5. Write the reviewer spec (safety preamble + "be hostile and paranoid, attack:
   perf/re-renders/allocations-in-render-path, null crashes, stale closures, race
   conditions, memory leaks (uncleaned listeners/tickers), edge cases
   (empty/null/undefined), determinism (no Math.random), and the task's own
   acceptance criteria" + the diff scope `git diff HEAD`). Dispatch deepseek-v4-pro.
6. Arbitrate findings: CONFIRMED BLOCKER/MAJOR ⇒ always fix; CONFIRMED MINOR ⇒ fix
   if cheap, else note in the handoff; PLAUSIBLE ⇒ read the code yourself and
   verify before acting; REFUTED/NIT ⇒ ignore. Fixes go back to the AUTHOR's pi
   session with `-c` (step 2's session). Then re-verify (steps 3–4) — including the
   re-verify-every-earlier-item rule.
7. Commit with message `polis: <short description> (T<n>)` +
   `Co-Authored-By:` trailer per house rules. Never `git add -A`; add named paths.
8. Append 3–5 lines to the handoff file
   (scratchpad `handoff-t1-t4.md`): what shipped, test counts, review verdicts,
   anything deferred.

## Hard rules
- ONE stateful tool call at a time; never parallelize a coder with the reviewer of
  the same work.
- If a task fails twice on the same blocker: stop that task, write the blocker in
  the handoff file, move to the next task.
- STOP after T4's commit. Do not start T5. Final message = the handoff summary.

---

## T0 — DONE (`48308e4`)
Transparent bottom-bar panels: `bg-white/97` and `bg-*/8` are dead classes in
Tailwind 3.4 (opacity scale is 5-step) → 4 of 6 panels had NO background. Fixed to
`/95` / `/10` app-wide + regression test. Filters panel now lists all 9 sin rule ids
(added complexity, god-file, test-gap, clone — the downstream pipeline was already
generic; verified by deepseek review: no BLOCKER/MAJOR).

---

## T1 — Review follow-ups + provider structures optional, OFF by default
**Coder: mimo-v2.5 · Reviewer: deepseek-v4-pro · Rust touched: yes (doc comment only — no cargo needed)**

### T1a — micro-fixes from the T0 review
1. `src/components/polis/PolisBottomBar.tsx` `RULE_GLYPH`: append `\u{FE0F}` (VS16)
   to the `"god-file"` (`\u{1F3DB}`) and `"test-gap"` (`\u{1F573}`) values — they
   have `Emoji_Presentation=No` and render monochrome without it. Result strings:
   `"\u{1F3DB}\u{FE0F}"`, `"\u{1F573}\u{FE0F}"`.
2. `src-tauri/src/polis/augure/mod.rs:42-45`: the `SinRecord.rule_id` doc comment
   lists only the 5 original rule ids — extend it to all 9 (`secret`, `dep-cycle`,
   `todo-density`, `dead-export`, `env-missing`, `complexity`, `god-file`,
   `test-gap`, `clone`). Comment-only change; no cargo run needed.
3. New integration test in `PolisBottomBar.test.tsx`: open the Filters panel, click
   the "Complexity" checkbox, assert the store's `setFilter` mock was called with
   `categories` containing `"complexity"`. Note the store is mocked at the top of
   the file (`vi.mock("../../store/cityStore", ...)`) — assert on that mock's
   `setFilter` (you may need to hoist the mock fn to reference it; follow the
   existing mock structure).

### T1b — external providers opt-in, default OFF
Facts: `ExternalServiceLayer` (`src/components/polis/ExternalServiceLayer.ts`)
renders `city.externalServices[]` as harbour/outpost structures; each entry has a
`provider` field (`"scaleway" | "cloudflare" | string`, plus `"monument"` for era
monuments — see `src/types/city.ts:202-215`, Rust `src-tauri/src/polis/cloud.rs`).
Today the layer is always on (only zoom-LOD gates it, via `setLodVisible`,
`ExternalServiceLayer.ts:152`). Persistence pattern to copy: `readLastFolder` /
`writeLastFolder` on localStorage key `polis:lastFolder` (`cityStore.ts:24-44`).

Changes:
1. `src/store/cityStore.ts`:
   - Helpers `readVisibleProviders(): string[]` / `writeVisibleProviders(list)` on
     localStorage key `"polis:visibleProviders"`, same try/catch style as
     `readLastFolder`. Malformed JSON ⇒ `[]`.
   - Store state `visibleProviders: string[]` initialised from the helper
     (**default `[]` = all providers OFF**), plus action
     `setProviderVisible(provider: string, on: boolean)` that updates the array
     (dedup, stable order) and persists.
   - `visibleProviders` is a user preference: it must NOT be reset by the
     folder-switch path that resets `filter` (`cityStore.ts:~547`).
2. `ExternalServiceLayer.ts`: add `setVisibleProviders(providers: ReadonlySet<string>)`.
   The layer groups its structures per provider (it knows each structure's
   `provider` when building). Effective visibility of a structure =
   `lodVisible && (provider === "monument" || providers.has(provider))`. Era
   monuments (`provider === "monument"`) are ALWAYS visible — they are city
   history, not cloud providers. Keep the existing `setLodVisible` semantics;
   compose, don't replace. No per-frame work: apply visibility when either input
   changes.
3. Wiring: wherever `PolisRenderer`/`PolisView` currently pushes store state to the
   renderer (the `setFilter` push path), also push `visibleProviders` (as a Set) to
   the layer on change. Follow the existing subscription pattern in `PolisView.tsx`.
4. Legend UI (`LegendOverlay` in `PolisBottomBar.tsx:~794-880`): the provider
   pennant + cloud-harbour sections become toggleable rows. Provider list = union
   of `["cloudflare", "scaleway"]` and the distinct providers present in
   `city.externalServices` (excluding `"monument"`). Each row: existing swatch +
   name + a small toggle (reuse the checkbox/toggle idiom already used in
   `FiltersPanel`); when OFF, grey the row and show hint text "hidden — toggle to
   show on the map". Reads `visibleProviders`/`setProviderVisible` from
   `useCityStore`. Era-monument section stays as-is (no toggle).
5. Help panel: one sentence in the harbour/providers section noting providers are
   off by default and enabled from the Legend.

Tests (all in existing files' style):
- `cityStore` test: defaults to `[]`; `setProviderVisible("scaleway", true)`
  persists (localStorage spy) and survives a folder switch; turning off removes.
- `ExternalServiceLayer.test.ts`: with providers `[]` nothing visible even when
  lodVisible=true; enabling `"scaleway"` shows only scaleway structures;
  `"monument"` visible regardless.
- `PolisBottomBar.test.tsx`: legend renders a toggle per provider; clicking calls
  `setProviderVisible`.

Verification commands: `npx vitest run src/components/polis/ExternalServiceLayer.test.ts
src/components/polis/PolisBottomBar.test.tsx <cityStore test file>` then full
`npx vitest run` for the wipe check.

Acceptance: fresh app ⇒ no harbour/cloud structures on the map; legend shows the
providers greyed with toggles; enabling one shows exactly its structures; monuments
unaffected; preference survives restart (localStorage) and folder switch.

---

## T2 — Pathing hardening: walkers never cross water/buildings, never leave roads
**Coder: mimo-v2.5-pro · Reviewer: deepseek-v4-pro · Rust touched: no**

Root causes (recon, all confirmed by code reading):
(a) `buildSplineLeg` (`src/components/polis/locomotion.ts:~66`) Catmull-Rom bows
outside the road polyline at corners — figures cut across water/buildings at bends;
(b) `applyPerpendicularOffset` (`locomotion.ts:~137`, ±4px lane shift) is unclamped
— pushes figures off road edges;
(c) the final approach leg road-end→door from `SlotAllocator.positionFor`
(`AgentLayer.ts:~909`) is never validated;
(d) no walker checks building footprints anywhere (`occupiedTiles` exists only for
props, `props.ts:~40`).
Non-causes (do NOT touch): route SELECTION is already road-constrained —
`RoadGraph.findRoute` (`roadGraph.ts`) only walks real road polylines and its
constructor already rejects water-crossing edges via `makeWaterBlocker`
(`navWalkable.ts:~64`); `TradeRouteLayer` walks polylines linearly and is already
safe; the null-route fallbacks (agent fade-teleport, ambient stay-idle) are correct
and must be preserved.

### Design (pre-decided — implement exactly this)
1. **Shared blocker, built once per city load.** In `navWalkable.ts` add:
   - `makeBuildingBlocker(buildings): (gx, gy) => boolean` — true on building
     FOOTPRINT tiles only (no neighbourhood padding: walkers may hug walls).
     Extract the footprint-tile iteration that `props.ts` uses for `occupiedTiles`
     into a shared exported helper so props and this blocker use one source;
     `props.ts` keeps its own 4-neighbourhood expansion locally.
   - `combineBlockers(...blockers): Blocked` — OR composition.
   - The walk blocker = `combineBlockers(makeWaterBlocker(terrain), makeBuildingBlocker(buildings))`
     (bridges stay walkable — `makeWaterBlocker` already excludes them). Build it
     in the renderer's world-setup (where RoadGraph is constructed) and pass it to
     AgentLayer and AmbientLayer. No per-frame construction.
2. **Spline clamping at leg-BUILD time** (never per frame). In `locomotion.ts` add
   `buildSafeSplineLeg(waypoints, blocked)`: build the spline leg as today, sample
   it at the SAME density the walk stepping uses, convert each sample to grid tile;
   if ANY sample tile is blocked ⇒ degrade THAT LEG to plain linear interpolation
   over the same waypoints (the polyline itself is road-guaranteed). Return which
   mode was chosen (for tests). AgentLayer and AmbientLayer switch every call of
   `buildSplineLeg` to `buildSafeSplineLeg`.
3. **Lane-offset clamping, per leg.** At leg build, test the offset extreme
   (`offsetPx` applied perpendicular at each sample): if any offset sample lands on
   a blocked tile ⇒ force lane offset 0 for that whole leg. One boolean per leg; no
   per-frame checks.
4. **Slot validation.** Where `SlotAllocator.positionFor` supplies the door-approach
   position (`AgentLayer.ts:~909`): if the slot position's tile is blocked ⇒ fall
   back to the building's door/origin tile centre. Keep the origin-captured-at-walk-
   start rule (fileId mutates at decision time — see existing comments/tests).
5. **Fallback semantics unchanged:** if even the linear polyline is blocked
   (theoretically impossible for graph legs), keep today's behavior: agents
   fade-teleport, ambients go idle and repick. Do not invent new movement.

Tests (`locomotion.test.ts`, `AgentLayer.*.test.ts`, `navWalkable.test.ts` styles):
- Concave 3+ waypoint route bending around a water tile where the raw spline's
  samples enter the water ⇒ `buildSafeSplineLeg` picks linear mode; same route with
  no hazard ⇒ spline mode.
- Lane offset forced to 0 when the road hugs a shore (offset sample on water);
  normal offset elsewhere.
- Slot position on a blocked tile ⇒ door-tile fallback.
- Building blocker: footprint tiles blocked, adjacent street tiles NOT blocked.
- Property test (seeded rng, existing pattern): for a small synthetic city, step an
  ambient walker N legs; assert no sampled position ever maps to a blocked tile.
- Regression: TradeRouteLayer behavior byte-identical (its tests untouched and green).

Perf acceptance (reviewer must attack): blockers are closures over prebuilt Sets;
leg validation is O(samples) at leg build only; ZERO new allocations in tickers;
no change to the fade/possession state machines.

Verification: `npx vitest run src/components/polis/locomotion.test.ts
src/components/polis/navWalkable.test.ts src/components/polis/AgentLayer.figures.test.ts
src/components/polis/AgentLayer.possession.test.ts src/components/polis/AmbientLayer.claim.test.ts`
then full `npx vitest run`.

---

## T3 — Bridges worth looking at
**Coder: mimo-v2.5 · Reviewer: deepseek-v4-pro · Rust touched: no**

`drawBridgeDeck` (`src/components/polis/terrain.ts:~312-349`) is a flat plank quad.
Rebuild it procedurally in the kitcd idiom (Caesar III stone bridge is the STYLE
reference — conventions only, no assets). Bridges are static geometry drawn once in
the chunked terrain build (`buildTerrainFrame`) — zero per-frame cost. Determinism
via `rngFromCoords`.

Pre-decided design:
1. **Orientation:** infer bridge direction from adjacent bridge tiles (consecutive
   bridge tiles share an axis); a lone bridge tile uses the orientation of the road
   entering it (fallback: x-axis).
2. **Draw order per bridge tile:** (a) pier shadow — darkened translucent ellipse on
   the water beneath each pier; (b) stone side profile on BOTH long sides: pier
   blocks at tile ends + an arch between them (approximate the arch with a 5-6 point
   poly cutout; stone colors = new `BRIDGE_STONE`/`BRIDGE_STONE_DARK` constants in
   `palette.ts` derived with `darken()/lighten()` from the existing stone family);
   (c) deck: paver pattern reusing the `pavers` seam idiom (`kitcd/detail.ts:238`),
   slight camber (mid-tile 1px lift); (d) parapets: low walls along both sides with
   a lighter coping line; (e) at bridge END tiles only: short ramp skirt + two small
   stone end-posts.
3. **Multi-tile bridges:** consecutive tiles repeat the arch module seamlessly
   (share pier positions at tile boundaries so arches meet on a pier).
4. NO lamp glow integration (the halo pass belongs to fire.ts pools — out of scope;
   end-posts are plain stone).
5. Keep the function signature compatible with its call site; if orientation needs
   neighbour info, compute a bridge-tile Set once in `buildTerrainFrame` and pass it
   down (build-time only).

Tests (`terrain.test.ts` style): orientation inference (horizontal run, vertical
run, lone tile); end-tile vs middle-tile draw differences (ramp/end-posts only at
ends); determinism (two builds with same input produce identical command streams);
no `Math.random`; water shimmer and shore rendering untouched (existing tests green).

Verification: `npx vitest run src/components/polis/terrain.test.ts` then full run.

Acceptance: bridges read as low stone arch bridges with piers, parapets and ramps at
the ends, consistent with the muted olive palette, at every zoom ≥ the terrain LOD.

---

## T4 — Walkers = real agent taxonomy (pi era) + carried package for porters
**Coder: mimo-v2.5 · Reviewer: deepseek-v4-pro · Rust touched: YES (orchestrator runs cargo)**

Facts: `attach_agents` (`src-tauri/src/polis/scanner.rs:5328`) fills
`city.agents` from live sessions; types come from `agent_type_for_role`
(`scanner.rs:~4919-4936`): orchestrator/coder/verifier/augur + **verbatim
pass-through for unknown role strings**. Mini-coders are coders with
`parent_agent_id`. pi-spawned sessions (`pi_sidecar::spawn_agent_session`,
`lib.rs:806`) register whatever role they were given. `AgentSession.model`
(`model.rs:1145`, `Option<String>`) is stored but never exposed to Polis.
Frontend mapping: `figureForType`/`figureForAgent` (`AgentLayer.ts:70-108`);
figure vocabulary (`kitcd/people.ts:36-41`): citizen, builder, firefighter,
watercarrier, merchant, noble. Merchant's goods sack: `people.ts:~228-232` (drawn
on the BACK — reads poorly in motion).

Changes:
1. **Rust — expose the model.** Add `model: Option<String>` to the Polis agent
   struct that serializes to TS `CityAgent` (find it in `src-tauri/src/polis/model.rs`;
   serde camelCase like its siblings). Copy it from the session in `attach_agents`.
   Update/extend the existing `attach_agents` unit tests (one new assertion: model
   passes through; None stays None). THE ORCHESTRATOR runs
   `cargo test --lib polis` in `src-tauri` afterwards — the pi coder must NOT run
   cargo.
2. **TS type:** `CityAgent` in `src/types/city.ts` gains `model?: string | null`.
3. **Two new figures in `kitcd/people.ts`** (same drawing idiom, walk-cycle
   consistent with existing types, `\u` escapes only, rng helpers only):
   - `"priest"` (for augur): long white robe with a purple trim band, laurel circlet,
     no tools. Augurs are currently never drawn — after this task `figureForType`
     maps `augur → priest` and augurs DO appear.
   - `"foreigner"` (for unknown/pass-through roles, i.e. pi-external sessions):
     hooded travel cloak in muted teal, walking staff. Guarantees any unknown agent
     role is VISIBLE instead of skipped.
4. **Mapping (`AgentLayer.ts`):** orchestrator→noble, coder→builder, mini-coder
   (parentAgentId)→watercarrier, verifier→citizen, augur→priest, anything
   else→foreigner. Keep censor presence = firefighter (not part of city.agents).
5. **Provider livery:** small pure helper `liveryTint(model?: string|null)` in
   AgentLayer (unit-testable): model string containing `"mimo"` ⇒ jade tunic tint;
   `"deepseek"` ⇒ indigo; `"claude" | "sonnet" | "opus" | "fable"` ⇒ terracotta;
   otherwise `undefined` ⇒ role's default tunic. Case-insensitive substring match.
   Colors derived from `palette.ts` (add three PALETTE-derived constants; do not
   hardcode raw hex outside palette.ts). Apply where the agent's `drawCitizen`
   tunic color is chosen.
6. **Carried package for trade porters:** `drawCitizen` gains an optional param
   `carrying?: "crate"` that draws a simple two-hands-carried wooden crate IN FRONT
   of the figure at hand height (box + rope cross, ~4×3 px at scale 1), with a
   subtle vertical bob synced to the existing walk phase. `TradeRouteLayer` passes
   `carrying: "crate"` for its porters; the merchant type elsewhere keeps its back
   sack unchanged.
7. **Docs in UI:** update HelpPanel "Agents vs. townsfolk" and LegendOverlay agents
   entries to the new taxonomy (one line per figure incl. priest/foreigner/livery
   note). English copy.

Tests:
- `AgentLayer.figures.test.ts`: augur→priest; unknown role "sherpa"→foreigner;
  mini-coder→watercarrier still; `liveryTint` unit cases (mimo/deepseek/claude/
  none/undefined/mixed-case).
- `people` drawing test (follow existing kitcd test pattern if present, else a
  smoke test that `drawCitizen` accepts the new types and `carrying` without
  throwing and emits >0 graphics commands).
- `TradeRouteLayer.test.ts`: porters draw with `carrying: "crate"`.
- Rust: `attach_agents` model pass-through test.

Verification: `npx vitest run src/components/polis/AgentLayer.figures.test.ts
src/components/polis/TradeRouteLayer.test.ts` + full vitest; orchestrator runs
`cargo test --lib polis` (from `src-tauri`, expect ≥ baseline count, all green).

Acceptance: every live agent type has a distinct recognizable figure; pi sessions
with exotic roles show up as hooded foreigners; agents driven by mimo/deepseek/
claude are distinguishable by tunic tint; trade porters visibly carry a crate.

---

# HANDOFF BOUNDARY — Sonnet STOPS here

After T4's commit: run full `npx vitest run` and `cargo test --lib` (src-tauri),
write final counts + per-task summary + deferred MINORs into the scratchpad handoff
file, and end with that summary as the final message. Do NOT start T5.

---

## T5 — Fill the empty map: farmland belts, orchards, gardens (FABLE)
**Executor: Fable-orchestrated (mimo-v2.5-pro coder) · Reviewer: deepseek-v4-pro**

Why the map is half empty (recon): buildings pack with GAP=3 tiles, district boxes
spaced DISTRICT_MARGIN=8 (`scanner.rs:3694-3699`), zero-coupling districts pushed
east of the bbox (`scanner.rs:4113`) → big bare value-noise plains; props capped at
`MAX_PROPS=1500` (`props.ts:26`) with ~24% fill on empty tiles; no farm/crop
primitives exist (closest: `gardenBed`, `kitcd/detail.ts:112`).

Direction (detailed spec written by Fable at execution time): new kitcd primitives
(cropRows, vineyard, orchardGrid, fallowField, haystack, farmShed) + a deterministic
`fields.ts` parcel partitioner over the empty rectangles between district bboxes
(4×3..8×6 tile parcels, dirt paths between, weighted by distance from centre:
near=gardens, mid=crops/vineyards, far=orchards/fallow), rendered into the chunked
static terrain build, LOD ≥ ~0.3, parcels never overlapping roads/water/building
footprints (+1 margin)/props; props rebalanced to exclude parcel tiles. Caesar III
farm conventions per the research notes below.

## MAX-RECALL (FABLE) — whole cumulative diff T0..T5
3 hostile reviewers on different angles (deepseek-v4-pro line-by-line, mimo-v2.5-pro
cross-file/interaction, Sonnet removed-behavior + perf/UX altitude) + adversarial
verify of every CONFIRMED finding, fixes, full suites (cargo/vitest/pytest), then a
fresh live-e2e checklist for the owner (panel opacity, per-sin filters, providers
off-by-default + toggles, no walker off-road in 5 minutes of watching, bridges,
farm belts, agent taxonomy + porter crates).

## T6 — OPTIONAL discretionary polish round (FABLE)
Owner-authorized: if, after max-recall, Fable judges the live visual result still
unsatisfying (walker motion quality, map richness, bridge look), Fable may define
and run ONE more polish task round (same cadence, mimo coder + deepseek review)
before handing back for live e2e.

---

## Research notes (web, 2026-07-09)
- Open-source Caesar III remakes (Julius, Augustus, CaesarIA) are engine
  re-implementations that REQUIRE the original proprietary assets — nothing to reuse.
- CC0 isometric packs (Kenney/KayKit/Buggy Studio/OpenGameArt) are modern-city or
  low-poly 3D styles that clash with our procedural olive-palette kit. Decision: stay
  procedural; adopt Caesar III *conventions*, not assets.
- Caesar III visual conventions encoded above: walkers NEVER leave roads, farms are
  rectangular parcels of crop rows with a farmhouse and visible growth stages, goods
  move as visible carried loads, bridges are low stone arches with piers and
  parapets, coastline gets docks/breakwater texture rather than bare water edge.

Non-goals / explicitly rejected: ripping Caesar III / Empire Earth assets
(proprietary — style reference only); grid.rs re-layout (district spacing is
semantic; the fix is filling the space, not compacting it); Censor coupling into
Polis (owner rule).
