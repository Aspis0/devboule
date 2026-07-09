# Polis Upgrade — UX / Graphics / Performance Spec (companion to `polis-upgrade-design-2026-07.md`)

**Status:** detailed spec for deliverables D4 (graphics), D5 (citizens), D6 (UI) and the cross-cutting frame-budget system. The parent doc owns rationale and references [R1–R16]; this doc owns component-level states, numbers, and acceptance criteria. Facts about current code re-verified 2026-07-08.

---

## 1. UX — the Command Deck and the parchment

### 1.1 Command Deck (evolves `PolisBottomBar.tsx`)

Today: a floating bottom-center pill with 4 panels — Guide, Legend, Files, Oracle (`PanelId` registry at `PolisBottomBar.tsx:122-135`). The Empire-Earth restyle keeps the single-popover model (one panel open at a time — already the current behavior) and grows the bar into three visual clusters:

```
┌─ status ─────────┐  ┌─ panels ────────────────────────────────┐  ┌─ zoom ─────────┐
│ 🏛 412  👤 3  🔥 7 │  │ Guide │ Legend │ File types │ Filters │ Anomalies │ Oracle │  │ − │ ▣ │ + │ 138% │
└──────────────────┘  └─────────────────────────────────────────┘  └────────────────┘
```

- **Status cluster** (read-only): building count, live agents, open-sin count with the worst-severity color. Data already in `cityStore` (header shows counts today — they *move* here in immersive mode; the header keeps them in windowed mode to avoid a regression).
- **Panels cluster**: existing four (Guide, Legend, File types, Oracle) + **Filters** + **Anomalies**. Same segmented-button styling as the current pill (reuse existing design tokens; follow-existing-UX rule — no new look, just more segments). Each new segment gets a badge: Filters shows an "active-filters" dot; Anomalies shows the open-sin count.
- **Zoom cluster**: `−` / fit / `+` buttons + live percentage. Wired to the existing pixi-viewport: `viewport.animate({scale, time: 250, ease: "easeInOutSine"})` (API present in `pixi-viewport@6.0.3`), clamped by the existing `clampZoom {0.15, 3.0}` (`createPolis.ts:169`). Fit = the existing `recenter()` iso-bounds logic (`PolisRenderer.ts:1574-1591`), wrapped in `viewport.animate` — NOT raw `viewport.fit()`, whose different algorithm and clamp range (0.15–3.0 vs recenter's 0.85–1.6) would frame the city differently than the header's Recenter and violate the single-code-path criterion (§4). Keyboard: `+`/`-`/`0`(fit); registered only while Polis is focused, unregistered on unmount.

**Immersive mode**: the Deck is the only chrome that stays visible. **Implementation task, not current behavior:** today `immersive` only makes the outer wrapper `fixed inset-0 z-50` — the Polis header itself is still rendered (`PolisView.tsx:539`, `:846-853`); the header count text must be explicitly suppressed when `immersive === true` or counts will show twice once the Deck status cluster ships.

### 1.2 Filters panel

State (in `cityStore`, NOT in the renderer):

```ts
interface FilterState {
  categories: Set<SinCategory>;   // empty = all visible
  minSeverity: "smoke" | "fire" | "inferno" | null;
  features: Set<string>;          // feature/district ids; empty = all
  pathGlob: string;               // "" = all; simple glob, matched on relPath
  mode: "ghost" | "hide";         // default "ghost"
}
```

Panel layout (top to bottom):
1. **Anomaly categories** — one chip per D7 `rule_id` group, each with its map glyph icon and a live count of open sins (from the Augure ledger slice). Toggling a category does NOT hide buildings; it hides that category's *effects* (fire/overlays) — buildings are only ghosted by the path/feature/type filters. This split matters: "hide style noise" must not make files disappear.
2. **Severity floor** — 3-position segmented control (show all / ≥fire / ≥inferno).
3. **Quarters** — multi-select of features/districts (names from the existing F1/F2 registry).
4. **Path** — one glob input, applied debounced 300ms.
5. **File types** — the existing extensions panel content folds in here as a collapsible row (the standalone "File types" segment — PanelId `filetypes` — stays for one release, then merges).
6. Footer: "Reset all" + result line ("shows 311 of 412 buildings, 5 of 7 anomalies").

**Renderer contract** (the reviewed correction is normative): `setFilter(f: FilterState)` on the renderer applies:
- a pass over *built* nodes: ghosted buildings → `alpha 0.15`, labels off, effects off, `eventMode "none"` stays clickable=false; roads with either endpoint ghosted → `alpha × 0.3`; agents targeting ghosted buildings keep walking (data-truth) but render at 0.4 alpha;
- the same predicate **threaded into the node-placement block** (`PolisRenderer.ts:2286-2348`, where alpha/labels/eventMode are set at construction) — NOT into `orderBuildQueue` (`buildQueue.ts:51-82`), which is a pure reordering function with no per-item mutation and must stay pure — so buildings built after the toggle are born ghosted — a filter toggle mid-incremental-build must produce the same city as toggling after build completes;
- **category/severity-only changes** touch per-sin `effects` visibility (fire/overlay sprites) on otherwise fully-opaque, fully-labeled buildings — they must NOT touch `alpha`/`eventMode`/labels (the two filter axes are separate mutations);
- **no rebuild, no reflow**: coords never change; `mode:"hide"` sets `visible=false` instead of alpha (chunk culling already tolerates invisible children).
- Cost target: one full pass ≤ 8ms at 1000 buildings (plain property writes, no Graphics redraw). Filter application is allowed to spread over 2 rAF frames above 2000 buildings.

### 1.3 Anomalies panel

A ledger view over the D7 sin shards (via new `polis_list_sins`):
- Two tabs: **Open** / **Ignored**.
- Rows: glyph, severity pip, rule title, `relPath:line`, age. Sort: severity desc, then age.
- Row click → fly-to the building (600ms) + opens the parchment scrolled to that sin. Fly-to is **new code**: a `flyTo(fileId)` on the renderer using `viewport.animate` toward the building's iso position — `recenter()` has no per-target parameter and no tween, so there is nothing to reuse beyond the bounds math.
- Ignored tab rows: "Un-ignore" button (`polis_dispose_sin(id, "open")`), plus the content-hash note ("ignored at revision …; will re-evaluate if the file changes").
- Empty states in scroll voice ("The augurs find the city untroubled.") — flavor tone, real counts.

### 1.4 Parchment (InspectSidebar) additions

The existing panel already has: header + confidence badge, Oracle blurb, stats, path+copy, imports/imported-by from roads, investigation section, sins list, agent section, notes, dossier, open-in-editor footer. Changes are additive, in this order in the scroll:

1. **Anomaly ledger section** replaces the current read-only "Issues" list: each open sin renders glyph + rule + evidence line ("cyclomatic 27, threshold 15 — measured, not guessed") + the two D8 actions (`Ignore`, `Send to main coder`). A sin with a fix in flight shows the builder-at-work state and disables both actions. Max 5 rows + expand.
2. **Kin buildings** (below Connections): top-5 from the D1 `similar` cache — name, score bar, click-to-navigate. Section renders only when the cache has entries (Oracle absent ⇒ section absent, per fail-open rule).
3. **Recent activity**: last agent visit (from agent ledger), last content change (mtime/hash from meta store). Two lines, no fetch — data already crosses the wire.
4. **Editor footer, finished**: primary button "Open in ⟨preferred⟩" (preference from config.json via new `polis_set_preferred_editor`), overflow "⌄" menu listing the other *detected* editors (`polis_detect_editors` → `provider_detect::resolve_program` probing) + Reveal in folder. First run with no preference: the overflow list only, and the first successful open sets the preference.

**UI performance rules** (React side): the sidebar and Deck subscribe via narrow `cityStore` selectors (sin slice keyed by fileId; filter state as its own atom) — a city-update event must not re-render the Deck unless counts changed; ledger rows memoized by `sin.id + disposition`.

---

## 2. Graphics — anomaly VFX, fire, light

### 2.1 Anomaly visual vocabulary (static tier — near-zero frame cost)

All D7 category overlays (ivy/cracks, boarded windows, twin banners, laundry clutter, missing wall, knot glyph) are **pre-rendered into `BuildingTextureAtlas`** as variant layers. **Required change:** today's atlas caches exactly one body + one shadow texture per `variantKey(purpose, level)` (`buildingAtlas.ts:85-86, 107-168`); overlays and fire frames need a widened cache key — `variantKey + overlay(category, tier)` and `variantKey-independent fireFrame(band, index)` — plus a generation path that snapshots the procedural drawing at N phase offsets. Same pattern, new key dimensions; not a drop-in reuse. Per-frame cost: zero (static sprites). Severity picks the variant tier (1–3), category picks the glyph. MINIMAL profile: skip all overlay variants, render category-tinted smoke only (atlas budget guard, parent doc assumption #5).

### 2.2 Fire, two tiers

**Tier F1 — crowd fire (default for every burning building).** Replace the per-frame `clear()+redraw` procedural `Flame` (`kitcd/anims.ts:32-93`, clear+redraw at :58/:87) with an 8-frame flip-book: the existing procedural drawing code runs ONCE per severity band into atlas frames (keeps the current art exactly — no hand-authored pixels), then each burning building is one `Sprite` whose `texture` swaps on the 30fps StepClock with a per-building seeded phase offset (deterministic, no `Math.random`). Smoke: same treatment, 6 frames, drifting via transform only.
- Cost model: texture-swap + transform per burning building per tick. Budget: 200 simultaneous crowd fires ≤ 1ms/tick.
- One atlas page for all fire/smoke frames ⇒ batching preserved [R9].

**Tier F2 — hero fire (promoted subset).** A PixiJS v8 `ParticleContainer` per hero building (risk note: v8's ParticleContainer is marked EXPERIMENTAL in pixi 8.18 — pin the pixi.js version until F2 ships, and keep Tier F1 as the fallback if a pixi bump breaks it): flame particles 28–40 + embers 8–12 + smoke puffs 6–10 (≈ 45–60 particles per fire), single shared particle spritesheet, dynamic properties limited to `position/scale/alpha/tint` (static: rotation off for flames), spawn/decay driven by the StepClock with seeded jitter.
- Promotion set: on-screen burning buildings, ranked severity desc then distance-to-viewport-center asc, capped by `RenderProfile.maxHeroFires` = RICH 6 / LEAN 3 / MINIMAL 0.
- Promotion re-evaluated on `moved`/`zoomed` (same dirty-flag path as culling) and on sin changes — never per frame. Demotion crossfades to Tier F1 over 300ms (no popping).
- Severity scaling: particle spawn rate ×1 / ×1.6 / ×2.4 (smoke/fire/inferno), flame scale +20% per band.

### 2.3 Light halos

One shared radial-gradient texture (rendered once, 256px), instanced as an **additive-blend** sprite per burning building on the `effects` layer:
- radius: 2.5 / 4 / 6 tiles by severity; alpha base 0.10 / 0.16 / 0.22;
- flicker: alpha stepped ±0.04 on the fire's seeded phase (same clock — light and flame agree);
- **night boost**: alpha ×1.8 and radius ×1.2 as the existing 4-minute day-cycle tint (`applyDayCycle()`, `PolisRenderer.ts:2900-2911`) approaches its evening phase. **Required change:** the phase value `k` is today a local inside that private method — it must be exposed as an instance field (`this.dayPhase`) for halos and shadow skew to read; small but not free.
- Caveat: additive blend breaks the sprite batch [R9] — mitigated by z-grouping all halos contiguously in one container (`effects/halos`) so the pipeline switches blend mode twice per frame, not per halo. Halo count is capped = burning-building count on screen; culled with their chunk.

### 2.4 Ambient lighting/shadow polish

- **Shadow skew**: the baked atlas drop shadows get a container-level `skew.x` in [−0.12, +0.12] driven by the same exposed `dayPhase` field (morning→evening). One transform write per tick on the `shadows` layer container — not per shadow. Zero re-render.
- **No dynamic lightmap.** Explicit non-goal: per-tile lighting or normal maps are rejected for this engine (2D tint pipeline, alpha scope) — the halo + day tint combination is the whole lighting model.

### 2.5 D5 citizen polish (visual half)

- **Spline easing**: `AgentMover` and ambient strolls interpolate through waypoints with Catmull-Rom (window of 4 points, precomputed per leg) — eliminates corner snapping at road bends; cost is a handful of multiplies per walker per tick.
- **Queueing**: per-building `entrySlots: [walkerId?, walkerId?, walkerId?]` in the layer (not in CityState — presentation state). Arriving walker takes the lowest free slot; slot i idles at `door + i × 12px` along the incoming road with the existing idle pose. Fourth-plus arrival waits at the last slot position. Slots freed on departure/possession-release. Deterministic: slot choice is by arrival order only.
- **Lane offset**: each walker applies a fixed perpendicular offset `hash(walkerId) % 9 − 4` px on shared segments; opposite directions bias to opposite signs. No collision solve — at ≤40 ambient + agents this reads as order without simulating it [R5]; ORCA/flow fields remain out of scope until walker counts change regime (parent doc D5).
- **Idle variants**: 2 new pose loops (look-around, sit-at-forum) added to the existing stepped pose system; forum lingerers (already 35% of crowd) prefer sit.

---

## 3. Performance — the measured frame budget

### 3.1 Current baseline (recon-verified, keep intact)

30fps `StepClock` with 4-frame catch-up clamp (`effects.ts:22-45`, clamp at :32); chunk culling on dirty flags; viewport-priority incremental build; texture atlas for bodies+shadows (the ~1MB/building fix); labels created/destroyed by LOD band; signature guards against no-op rebuilds; hardware-adaptive `RenderProfile` (RICH/LEAN/MINIMAL). Every addition below rides these rails; none replaces them.

**The existing hardware rail, precisely** (inspected 2026-07-08):
- **Probe (Rust)**: `detect_hardware` (`backend/hardware.rs:442`, 706 lines, unit-tested pure seams) — CPU cores + RAM via `sysinfo`; GPU via DXGI on Windows / `system_profiler SPDisplaysDataType -json` on macOS / `unknown` elsewhere. Fail-soft everywhere: any probe failure degrades to `unknown`/`null`, never blocks. Known quirks already handled in code: WARP/Basic-Render software adapters, the Optimus 0-VRAM discrete laptop part (deliberately LEAN), Apple Silicon (unified memory ⇒ `gpuKind:"integrated"`, `vramGb:null` — `hardware.rs:155-157, 205-207`).
- **Policy (TS, pure)**: `profileFor(hw)` (`renderProfile.ts:160`) — most-restrictive-wins: ≤4 cores or known VRAM <1.5GB ⇒ MINIMAL; RICH only for **discrete GPU + VRAM ≥4GB + ≥8 cores**; everything else (and `null` = probe failed) ⇒ LEAN. LOD thresholds are pinned monotonic by tests.
- **Knobs consumed** (`PolisRenderer.ts:590-608`, chosen ONCE in the constructor, logged as one `PROFILE …` debug line): 4 LOD zoom thresholds, `preloadRing` 2/1/0, `atlasResolutionCap` 2/1/1 (min'd with devicePixelRatio), `maxAmbientWalkers` 40/18/6, `antialias` on/off/off.
- **Not part of this rail**: `backend/budget.rs` is the *inference* RAM broker (oMLX/Ollama model pools) — unrelated to rendering; don't conflate them.

**Gap found (must fix in P5): Apple Silicon lands on LEAN.** An M1 Max (10 cores, 64GB unified, 32-core GPU) reports `integrated + vramGb:null`, fails the discrete-only RICH gate, and gets the LEAN profile — antialias off, 18 walkers, half-res atlas — on what is actually the strongest render box we target. The conservative default was right when the tiers only *removed* detail; with D4's hero fires gated `RICH 6 / LEAN 3` it starves the reference machine. Fix: extend the RICH gate with a unified-memory branch — `gpuKind:"integrated"` AND gpuName matches `/^Apple M/` AND `ramTotalGb ≥ 32` AND `cores ≥ 8` ⇒ RICH (unified memory *is* the VRAM pool). Pure-function change in `profileFor` + one test row; the monotonic-LOD pinning tests are unaffected. Windows discrete behavior unchanged.

**Also note for §2/§3 features**: the profile is immutable after construction ("chosen ONCE") — the `EffectsBudget` ladder (§3.2) is deliberately a *separate, dynamic* layer on top of the static tier; it must not mutate `RenderProfile`, only its own rung state. The `PROFILE` debug line should gain the ladder rung when the overlay lands.

### 3.2 The effects budget (new: `EffectsBudget`)

A per-tick accumulator around the effects update (fires, halos, walkers' anims):

- **Allowance**: 3.0ms per 33ms tick on RICH, 2.0ms LEAN, 1.0ms MINIMAL (≈9%/6%/3% of the tick).
- **Measurement**: `performance.now()` bracket around the effects pass, exponentially smoothed (α=0.2) to ignore single-frame spikes (GC).
- **Demotion ladder**, applied one rung per second while over budget:
  1. hero fires → crowd fires (highest-cost first: largest particle count);
  2. halo flicker freezes (static alpha);
  3. crowd-fire frame rate halves (15fps flip-book);
  4. ambient walker anim rate halves;
  5. ambient walkers pause (agents — real data — never pause).
- **Promotion** (hysteresis): only after 60 consecutive ticks (≈2s) under 66% of allowance, one rung per 2s. Prevents flapping.
- **Determinism/explainability**: the ladder order is fixed and the current rung is exposed on the debug overlay (§3.4) — "why did my fire get simpler" must have an inspectable answer, same philosophy as the anomaly rail.

### 3.3 Budget table (targets, RICH profile, M1-Max-class — valid only AFTER the Apple-Silicon RICH-gate fix in §3.1; today that machine classifies LEAN)

| Scenario | Target |
|---|---|
| 1000 buildings, 100 open sins, 40 walkers, 6 hero fires | ≥ 55fps render, effects pass ≤ 3ms |
| Filter toggle at 1000 buildings | ≤ 8ms single pass (or 2-frame split above 2000) |
| Zoom animate across LOD band | no frame > 25ms during the 250ms ease |
| 200 simultaneous crowd fires (pathological repo) | effects ≤ 3ms after ladder settles; no hero fires |
| Cold city build (1000 buildings) | unchanged from today (incremental path untouched) |
| LEAN profile, same city | ≥ 30fps, 3 hero fires max, halos static |

Numbers are acceptance targets for the P5 phase review, measured with the §3.4 overlay on the reference machine; they are not hard runtime asserts.

### 3.4 Instrumentation (ships with P5, dev-flag only)

A debug overlay (existing `polis_debug_log` pattern, toggled by a dev flag in the Guide panel): FPS, effects-pass ms (smoothed), particle count, hero/crowd fire counts, current ladder rung, culled/total chunks, built/total buildings, walker count. One `Text` node updated at 2Hz — negligible cost, removable by flag.

### 3.5 Memory guards

- All new textures (fire frames, particle sheet, halo gradient, overlay glyph variants) live in the existing atlas budget; the atlas already caps resolution by profile. Estimated additions: fire+smoke frames ~0.5MP, particles ~0.06MP, halo ~0.06MP, 6 glyph sets × 3 tiers ~0.75MP — comfortably one extra 2048² page on RICH, half-resolution on LEAN.
- ParticleContainers are pooled: `maxHeroFires` containers allocated once per session, re-targeted between buildings (no per-promotion allocation).
- Queue slots, lane offsets, spline windows: fixed-size arrays on the layer, allocated at layer init (allocation-free ticker rule, `PolisRenderer.ts:13-14`).

---

## 4. Acceptance criteria (UX)

- Every filter combination is reversible and survives a `polis://city-updated` refresh (FilterState lives in the store, re-applied on diff).
- A filter set during incremental build yields the same visible city as the same filter set after build (the threaded-predicate requirement).
- Anomalies panel ↔ map ↔ parchment triangle is closed: any sin reachable from any of the three, ignored sins never invisible-invisible (badge count on the Ignored tab).
- Zoom buttons, wheel, pinch and fly-to produce the same LOD behavior (single code path through the viewport).
- Open-in-editor: preference persisted, detection graceful (no installed editor ⇒ Reveal-only, no error toast storm), macOS and Windows both covered (cfg-gated).
- All new UI copy: scroll voice, English, real data behind every sentence.
