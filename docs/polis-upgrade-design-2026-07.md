# Polis Upgrade — Technical Design (2026-07)

**Status:** design, not yet implemented.
**Scope:** the eight deliverables from the Polis-upgrade brief, reconciled against the actual codebase (three code recons: frontend, Rust backend, Oracle/Censor — 2026-07-08).
**Prime directive kept:** Polis renders ONLY what the backend provides; anything shown as a bug comes from deterministic, repeatable analysis.

---

## 0. Ground truth vs the brief — discrepancies first

The brief was a wishlist written from memory. Inspection found it wrong or stale in these places; **the code wins**:

| # | Brief assumed | Reality (evidence) |
|---|---|---|
| 1 | "Open file externally" is to be designed | **Already shipped.** `polis_open_in_editor` (`src-tauri/src/polis/commands.rs:1492`) opens notepad / VS Code / Cursor / Insiders / reveal-in-Explorer, path-validated under scan root; wired to the inspect sidebar footer (`InspectSidebar.tsx:1502`). What's missing is only a *persisted user preference* and auto-detection of installed editors (the `provider_detect::resolve_program` machinery already exists for IDEs in `changes.rs:328`). |
| 2 | Semantic districts are new | **Largely shipped.** F1 feature registry (directory spine + import hubs, persisted in `.aspis-meta.json`) and F2 Oracle reclassification (`polis_reclassify_features`, fail-closed, with merges) exist (`polis/model.rs:609`, `polis/commands.rs:1008`). What's missing: F3 *layout by feature* (fields persisted, layout not applied) and any embedding-driven clustering. |
| 3 | Oracle exposes tree-sitter import/call graphs Polis can consume | **FALSE — the critical finding.** No queryable import/call graph exists anywhere. The CKG store defines `IMPORT`/`CALL` edge kinds but populates **only CONTAIN edges** ("Empty until CALL edges land (B3)", `oracle/store/ckg_store.py:116`). `structure.rs` is a *name-resolution heuristic* (symbol-name matching, phantom-edge prone), not an import graph. Polis roads come from **regex** import extraction (`scanner.rs:1017`). D3 must *build* the graph, not merely consume it. |
| 4 | "Rust-OOD" tree-sitter weakness | **Not found in the codebase** (zero matches). Rust is among the *best*-supported grammars (`censor/extract.rs`). The real weaknesses: (a) no grammar for Shell/YAML/SQL/Dockerfile/CSS, (b) name-resolution phantom edges on common identifiers (`new`, `Config`), (c) macro bodies are opaque to AST metrics (external literature confirms this for tree-sitter-rust). Graceful degradation targets *these*, not a Rust gap. |
| 5 | Bug detection is "hardcoded-constant-style checks" | Understated. Two systems exist: **Polis Augure** (`polis/sins.rs:34-130` — secrets, cyclic imports, TODO density, orphan exports, missing env vars; ephemeral, recomputed per scan) and **Censor** (32 deterministic runners + local-LLM tier, findings persisted in `.aspis-censor/` shards with content-hash supersede). Per the owner's direction (2026-07-08), **Censor stays out of Polis's core loop** — Polis grows its own deterministic rail (D7); Censor remains an optional, clearly-labelled adjacent source at most. |
| 6 | No fix-dispatch path | Partially false. `spawn_main_coder_directive(project_id, task, files)` exists (`backend/main_coder.rs:96`) and `MiniCoderDirective` can pre-seed findings. D8 is mostly *wiring a button*, not building a dispatch system. |
| 7 | Rendering is naive | No. Chunked incremental build with viewport priority, chunk culling, hardware-adaptive LOD profile, texture atlas (fixed the ~1MB/building heap), 30fps StepClock, signature-guarded rebuilds (`PolisRenderer.ts`). D4/D5 refine a mature renderer; no rewrite. |
| 8 | Oracle "new semantic capabilities" ready | Partially. `/similar/{node_id}` (cosine), `/embed-bounded` (raw Qwen3 vectors), hybrid dense+lexical `/context` exist. **No algorithmic clustering** (`cluster_semantic` is a classifier *label*), no `/clusters` list, no `/graph` route, `/snapshot.edge_count` hardcoded 0. D1 needs server-side additions. |
| 9 | Filters / zoom controls / minimap | None exist. Bottom bar has Guide, Legend, File-types (extension filter — the only filter today), Oracle search (`PolisBottomBar.tsx:122-135`). No bug-category/path/severity filters, no zoom buttons, no minimap. |
| 10 | Ignore/resolve on map | Absent on the map. `censor_dispose_finding` exists only in the separate Censor panel. Polis sins have no disposition concept at all (they're ephemeral — D7 must persist them before D8 can ignore them). |

---

## 1. References grounding this design

Full annotated bibliography gathered 2026-07-08 (16 sources). Short-form citations used below:

**Crowds/pathfinding** — [R1] Reynolds, *Boids / Steering Behaviors* (SIGGRAPH '87, GDC '99); [R2] Treuille et al., *Continuum Crowds* (TOG 2006); [R3] van den Berg et al., *ORCA/RVO2* (2011); [R4] Emerson, *Flow Field Tiles* (Game AI Pro ch. 23, SupCom2); [R5] Caesar III/Zeus walker mechanics (community reverse-engineering: random roamers + minority "forced walkers").
**Isometric/VFX** — [R6] Bellanger, *Isometric Tiles Math*; [R7] Reeves, *Particle Systems* (SIGGRAPH '83); [R8] Sanglard, *DOOM PSX fire* (cellular-automaton fire); [R9] PixiJS *Performance Tips* + *ParticleContainer v8* (1M particles vs ~200K sprites; batching broken by texture/blend switches; culling opt-in).
**Semantic code viz/search** — [R10] Wettel & Lanza, *CodeCity* (VISSOFT'07/ICSE'08 — metric→geometry channel discipline); [R11] Steinbrückner & Lewerentz, *EvoStreets* (streets from the *stable* hierarchy, not the volatile graph); [R12] Husain et al., *CodeSearchNet* (2019 — learned embeddings ≫ token baselines for semantic grouping); [R13] Qwen3-Embedding report (2025 — the embedder Oracle already runs).
**Tree-sitter/static analysis** — [R14] tree-sitter design docs + error-recovery PR#101 (incremental parsing; rust macro opacity); [R15] Creager & van Antwerpen, *Stack Graphs* (EVCS 2023 — build-free name binding over tree-sitter; the model for real import edges); [R16] McCabe 1976 (cyclomatic complexity) + PMD-CPD/jscpd (Rabin-Karp clones) + Tarjan 1972 (SCC for dependency cycles) + ast-grep (tree-sitter-native structural rules).

Design maxims taken from the literature: streets from the stable directory hierarchy, graph edges as overlays [R11]; 2–3 metrics on independent visual channels, never overloaded [R10]; roamers cheap + forced-walkers routed [R5]; flow fields only above ~50–100 concurrent walkers to one destination [R4]; hero-particles + cheap CA fire for the crowd of anomalies [R7][R8]; ParticleContainer + explicit culling at scale [R9]; SCC = its own anomaly class [R16]; real embeddings for districts, not keywords [R12][R13].

---

## 2. System architecture (target)

```
                       ┌────────────────────────────────────────────┐
                       │  Rust backend (src-tauri)                  │
 file system  ──walk──▶│ polis/scanner ── imports(regex, fallback)  │
                       │      │                                     │
 tree-sitter ──parse──▶│ backend/graph  (NEW: real import edges,    │──persist──▶ .aspis-meta.json / graph.json
 (one shared parser:   │   extends censor/extract + ckg.rs; Tarjan  │
  censor/extract.rs)   │   SCC; per-file metrics)                   │
                       │      │                                     │
 Oracle (Python) ◀─────│ polis/semantic (NEW: async client for      │
  /similar             │   /similar /clusters /embed-bounded;       │
  /clusters (NEW)      │   SemanticCache, never blocks a scan)      │
  /context /ask        │      │                                     │
                       │ polis/augure  (NEW module: deterministic   │──persist──▶ .aspis-polis/sins/*.json
                       │   check roster, scheduler, sin ledger      │             (shard-per-file, content-hash,
                       │   with dispositions)                       │              Censor-pattern, Polis-owned)
                       │      │                                     │
                       │ polis/commands ── CityState assembly ──────│──event──▶ polis://city-updated
                       │ backend/main_coder ◀── D8 fix dispatch     │
                       └────────────────────────────────────────────┘
                                          │
                       ┌────────────────────────────────────────────┐
                       │  Frontend (React + PixiJS v8)              │
                       │  cityStore → PolisRenderer (chunks, LOD)   │
                       │  + FilterState (NEW)                       │
                       │  PolisBottomBar → CommandDeck (D6)         │
                       │  InspectSidebar → parchment additions (D6) │
                       │  effects: fire tiers + light halo (D4)     │
                       │  AgentLayer/AmbientLayer: queueing (D5)    │
                       └────────────────────────────────────────────┘
```

**Transport (flagged open question in the brief, now decided):** Polis keeps its existing channels — Tauri commands for pulls, the single `polis://city-updated` event for pushes, files under the project root for persistence. No broker, no new event bus. Oracle stays HTTP-behind-Rust; the frontend never talks to Oracle directly. This is the current architecture and nothing in the eight deliverables needs more.

**Censor's place (owner direction):** none in the core loop. The Augure rail (D7) is Polis's own; it may *reuse the shard-ledger pattern* from Censor (proven design) but not its pipeline, providers, or trust gate semantics beyond the project-trust check.

---

## 3. The eight deliverables

### D1 — Oracle semantic integration

**Query surface Polis needs (and its gaps):**

| Need | Endpoint | Status |
|---|---|---|
| "files semantically similar to X" → semantic roads, parchment "kin buildings" | `GET /similar/{node_id}?limit=N` | exists |
| "embedding clusters" → district layout (F3) | `GET /clusters` + `GET /cluster/{id}/members` | **must be added server-side**: HDBSCAN (fallback k-means) over `chunks.lancedb` vectors aggregated per file (mean-pool per file_id), persisted in a new SQLite table `file_clusters(file_id, cluster_id, score)` recomputed at end of index runs. HDBSCAN because k is unknown and noise files should stay unclustered rather than forced into a district [R12]. |
| per-building blurb/dossier | `/ask`, `/context`, dossier commands | exists (session-cached) |
| raw vectors for ad-hoc similarity | `POST /embed-bounded` | exists |

**Mapping to visuals:** cluster_id feeds `Feature`/`District` assignment as a third provenance tier: `featureSource: "directory" | "oracle" | "embedding"`. Precedence: user override > oracle (F2) > embedding > directory. Embedding clusters never *move* placed buildings on their own (coord stability is owned by `.aspis-meta.json`); they influence placement only for *new* buildings and on explicit "Re-classify" — same consent model F2 already uses.

**Caching/refresh (never block a scan):** new `polis/semantic.rs` holding a `SemanticCache`:
- Persisted in `.aspis-meta.json` (`semantic: {epoch, per_file: {file_id: {similar: [(file_id, score)], cluster_id}}}`).
- Scan reads cache only. After each scan, a background task (existing `tauri::async_runtime`) refreshes stale entries — staleness = Oracle index `last_updated` newer than cache `epoch`, checked via existing `/health`.
- Refresh completion re-emits `polis://city-updated` through the existing diff path (signature guard already dedupes no-op deliveries, `cityStore.ts:393-417`).
- Oracle down / not indexed ⇒ cache serves last-known; empty cache ⇒ city renders exactly as today. **Fail-open to structural, never to nothing.**

**Migration:** purely additive. `road_type: "semantic"` and `featureSource` already exist in both type systems (declared, never emitted — `model.rs:427` + `:450`, `scanner.rs:23`). Old frontends ignore unknown provenance strings; no `CITY_STATE_VERSION` bump needed. Semantic roads ship behind the existing render-profile gating so LEAN/MINIMAL profiles skip them.

### D2 — Forward-compatible integration point (entire.io)

**Design: a Rust trait, not an MCP server.** Polis's data sources are in-process today (scanner, Oracle client, cloud inventory). The cheapest future-proof boundary is a trait at the CityState-assembly seam:

```rust
// src-tauri/src/polis/source.rs (NEW)
pub trait CityDataSource: Send + Sync {
    fn id(&self) -> &'static str;                        // "oracle", "entire-io", ...
    fn entities(&self, ctx: &ScanContext) -> SourceResult<Vec<EntityPatch>>;      // buildings/services metadata
    fn relationships(&self, ctx: &ScanContext) -> SourceResult<Vec<RelationEdge>>; // typed edges w/ weight+provenance
    fn health_signals(&self, ctx: &ScanContext) -> SourceResult<Vec<HealthSignal>>; // ONLY deterministic-provenance signals
    fn freshness(&self) -> SourceFreshness;              // epoch/staleness for the cache layer
}
```

- `EntityPatch` *decorates* scanner-discovered files (never invents buildings — data-purity contract stands, `model.rs:465`); a source may also contribute `ExternalService`-shaped nodes (the harbour) for non-file entities like tickets/deploys.
- `RelationEdge {from, to, kind, weight, provenance}` — merged into roads with provenance-tagged styling.
- `HealthSignal` carries `rule_id`, `deterministic: true` attestation and evidence (file, line range, message). Signals lacking deterministic provenance are rendered ONLY as investigation-smoke ("?"), never as fire — the existing suspect visual (`Building.suspectOfCardId`) generalizes to this.
- The Oracle integration (D1) becomes the first implementor; the assembly in `commands.rs:93-137` (today: hardcoded calls to agents/suspects/cloud attach) refactors into an ordered `Vec<Box<dyn CityDataSource>>` fold. Conflict rule: scanner truth < source patches, later sources never override earlier ones on the same field, all patches logged into `scan_note`.
- **What entire.io would supply:** entities (its work items/projects → harbour nodes; file annotations → EntityPatch), relationships (item↔file links → `kind: "tracked-by"` overlay edges), health (SLA breaches, stale items → HealthSignal or suspect-smoke).
- **Assumptions to relax later (documented, not built):** async/remote sources (trait is sync today; wrap with the SemanticCache pattern — cache-serving, background-refreshing), authentication (would live in the existing vault/Settings pattern), and rate limits. An MCP-shaped remote adapter can be added later as one more `CityDataSource` impl that proxies; the trait doesn't preclude it.

### D3 — Tree-sitter-driven road network

**Reality check first:** there is nothing to "consume" — real import/call edges don't exist (finding #3). So D3 = *complete the long-planned B3 edge extraction once, in Rust, at the single existing parse site*, then let both Oracle (CKG) and Polis (roads) consume it. That honors "no double-parsing" in the only way that's actually true.

**Where:** extend `backend/ckg.rs` (which already reuses `structure.rs`'s walk and `censor/extract.rs`'s parser — one parse, three consumers) with an `extract_edges` pass:
- **IMPORT edges** per language via tree-sitter queries on real import nodes: Rust `use_declaration`, TS/JS `import_statement` / `call_expression[require]`, Python `import_statement`/`import_from_statement`, Go `import_spec`, Kotlin `import_header`, C/C++ `preproc_include`. Resolution is *path-based* (relative-path + module-root heuristics per language), NOT full name binding — stack-graphs-grade resolution [R15] is explicitly out of scope for alpha.
- **Edge weight** = distinct imported symbols count (fallback 1). Call-graph edges stay deferred (declared kind, unpopulated — as today, but now honestly documented in code).
- **Persistence:** `ckg_edges` finally gets IMPORT rows (Oracle side unchanged — `find_imports()` starts returning data); Polis-side the scanner consumes the same extraction *in-process* (no HTTP hop, no CLI re-spawn: `scanner.rs` calls into `backend/graph::import_edges(root)` which shares extract.rs' parser and a per-scan memo).
- **Tarjan SCC** (hand-rolled iterative Tarjan, ~60 lines — `petgraph` is NOT currently a dependency; add it only if more graph algorithms accrue) runs on the import graph per scan → feeds the *tangled-quarter* anomaly (D7) and upgrades today's regex "cyclic import" sin to a real one.

**Road mapping:**
- Primary street *layout* stays hierarchy/grid-driven (stable city, [R11]); import edges determine which building pairs get roads, `weight` drives trunk vs minor (existing `ROAD_WEIGHT_TRUNK` bands, `PolisRenderer.ts:125`), routed by the existing `grid::route_roads`.
- `Road.provenance: "ast" | "regex" | "semantic"` (new field, additive).

**Graceful degradation:** per-language ladder — (1) tree-sitter grammar present → AST edges (`provenance:"ast"`); (2) no grammar (Shell/YAML/SQL/CSS/Dockerfile) or parse error → today's regex extractor stays as fallback (`provenance:"regex"`, rendered as the existing faint minor-road style); (3) neither matches → no road, and if the file has *chunk-level* `symbols_used` hints from Oracle only, at most a dashed "path under construction" style — visible, never invented as solid. Macro-opaque Rust regions [R14] under-report edges; accepted and documented, not silently wrong.

**Migration:** dual-emit for one release (AST where available, regex fill-in elsewhere). **Required pre-work (found in review):** today's `road_id` is *position-derived* (`format!("road-import-{i}")` over a sorted pair list, `scanner.rs:3069-3079`), so any change to the edge set shifts most ids and the frontend's `roadSignature` triggers a full road-layer destroy+rebuild (`PolisRenderer.ts:1292`, `:2008`). D3 must first switch `road_id` to a content-stable key (e.g. hash of `(from, to, road_type)`); only then does the signature guard behave as intended and the AST/regex transition avoids rebuild storms.

### D4 — Graphics & animation fidelity

Constraints kept: 30fps StepClock, RenderProfile tiers, atlas discipline, no per-frame allocations (`PolisRenderer.ts:8-14`).

- **Fire, two-tier** [R7][R8]: (a) **Hero fire** — a PixiJS v8 `ParticleContainer` [R9] per *on-screen, zoomed-in* burning building (cap: `RenderProfile.maxHeroFires`, e.g. RICH=6/LEAN=3/MINIMAL=0), flame+ember+smoke particles from a small shared spritesheet (one texture — batching preserved), flicker via per-particle seeded phase (deterministic, no `Math.random` — house rule). (b) **Crowd fire** — all other burning buildings keep an upgraded version of today's procedural `Flame` (`kitcd/anims.ts:51`): pre-render 8 flame frames into the existing `BuildingTextureAtlas` per severity band and flip-book them (kills the per-frame `clear()+redraw` cost that scales with anomaly count). Severity (smoke/fire/inferno) scales particle rate, flame frame-set, and halo radius.
- **Light-radius falloff:** one additive-blended radial-gradient halo sprite per burning building on the existing `effects` layer, alpha stepped with the fire flicker, radius = severity. Halos are sprites from one shared gradient texture (single batch). At night-phase of the existing day-cycle tint the halo alpha rises — fire reads farther at dusk. No per-tile lightmap in alpha (over-engineering for a 2D tint pipeline).
- **Shadows/lighting:** keep baked atlas drop-shadows; add a global shadow-direction skew parameter driven by the day-cycle phase (skew the shadow sprite transform — free, no re-render) — subtle, consistent with the isometric style [R6].
- **Frame budget:** extend StepClock into a measured budget: effects tick gets a per-frame allowance (e.g. 3ms); a simple accumulator demotes hero fires to crowd fires when exceeded, then halves ambient walker animation rate. Deterministic demotion order (severity desc, distance-to-center asc) so what stays "hero" is explainable. Cull halos/fires with their building's chunk (existing chunk culling).

### D5 — Citizen locomotion & behavior

Much already matches the literature: roamers on the real road graph with weight-biased destinations (`AmbientLayer.ts:225`), role→figure mapping (`AgentLayer.ts:76`), claim-on-activation possession, forum lingerers, deterministic seeding. Per [R5], the genre standard is exactly this two-tier split — so D5 is targeted refinement, **not** a crowd-sim rebuild:

- **Path smoothing:** Catmull-Rom easing through road-polyline waypoints instead of segment-linear lerp (AgentMover + ambient stroll) — cheap, kills corner snapping.
- **Queueing at buildings:** per-building entry slots (max 3): a walker arriving at an occupied building claims slot *i* and idles in a short queue offset along the road; slot release on departure. Visualizes "hot" files as queues — data-honest (more agents on a file = a visible line). Plain counter map, no reservation system.
- **Overlap avoidance:** lane offset — each walker gets a deterministic perpendicular offset (hash of id, ±4px) on shared road segments, plus opposite-direction walkers keep to their side. No ORCA [R3], no flow fields [R4] at current scale (≤40 ambient + few agents); noted as the upgrade path *only if* a future feature pushes >100 concurrent walkers toward one destination.
- **Idle/work loops:** extend the existing pose system (hammer/magnifier/legs) with 2 idle variants (look-around, sit at forum) on the stepped 30fps clock; role archetypes stay the kitcd figures (no new art pipeline — house rule: don't hand-author pixel art beyond the existing kit).

### D6 — UI overhaul: Command Deck + parchment

**Bottom bar → "Command Deck"** (Empire-Earth-style segmented bar; extends `PolisBottomBar.tsx`, reusing existing panel/popover patterns and design tokens — follow-existing-UX rule):
- Segments: **Guide · Legend · Files · Filters(NEW) · Anomalies(NEW) · Oracle · Zoom(NEW)**.
- **Filters panel:** toggle chips for (a) anomaly categories (the D7 roster), (b) severity floor (smoke/fire/inferno), (c) path glob + feature/district multi-select, (d) building types (existing extension filter folds in here). Implementation: a `FilterState` in `cityStore` — buildings stay placed, filtered ones drop to a ghost style (15% alpha, no effects, no labels) so the city doesn't reflow; roads/agents referencing ghosted buildings dim likewise. Filtering never triggers a rebuild, but a one-shot visibility pass is NOT enough: the renderer builds chunks incrementally with viewport priority (`viewportPriorityChunks`/`orderBuildQueue`, `PolisRenderer.ts:~852-878`), so `FilterState` must also be threaded into the chunk-build/placement path — buildings placed *after* a filter toggle are born already ghosted, not patched later.
- **Zoom segment:** −/+/1:1/fit buttons calling the existing pixi-viewport (`clampZoom` bounds), with 250ms eased `viewport.animate`; LOD transitions already handle the rest. Optional minimap deferred (new idea #4 below).
- **Parchment (InspectSidebar) additions** — the panel is already rich (Oracle blurb, dossier, connections, sins, agent, notes — `InspectSidebar.tsx`); add: (a) **Kin buildings** — top-5 from D1 `similar` cache with score bars, click-to-navigate; (b) **Anomaly ledger section** — per-building open sins from the Augure ledger with category icon, severity, evidence line, and the two D8 actions; (c) **recent activity** — last agent visits + last content-hash change (data already in meta/agents stores). In-universe copy tone (scroll voice) with real data — existing pattern (confidence badge "grounded ✅ vs guess ?") extends to every new field.
- **Open-file, finish rather than build:** add Settings-persisted `preferredEditor` (config.json, existing RMW saver pattern) + auto-detection of installed editors via `provider_detect::resolve_program` (exists) to populate the picker; parchment footer becomes one-click "Open in ⟨preferred⟩" + overflow menu of the other detected editors + "Reveal in folder". macOS: extend the hardcoded allowlists in `polis_open_in_editor` with `open -a` handling and TextEdit/Zed/Cursor slugs (cross-platform rule: cfg-gate, don't fork logic).

### D7 — Expanded deterministic bug detection (the Augure rail)

**Owner constraint honored:** this is Polis's own rail; Censor is not in the loop. But its *ledger pattern* is proven, so we copy the pattern, not the pipe.

**New module `polis/augure/`** (splits out of `sins.rs`):
- `checks/` — pure check functions; `ledger.rs` — persisted shards at `.aspis-polis/sins/<sha256(relPath)>.json` `{relPath, contentHash, updatedAt, sins: [SinRecord]}` with a content-hash supersede identical in spirit to Censor's (dispositions survive re-scan at same hash; changed file ⇒ fresh evaluation); `scheduler.rs` — runs the roster **opportunistically**: fine checks piggyback the existing debounced watcher re-scan (400ms quiet, `watcher.rs:45`); coarse checks (cycles, clones, coverage) run on an idle timer (e.g. ≥60s idle, cancel on activity) and never during a scan. Persisted results mean the city renders complete sins immediately on startup, before any check re-runs — the brief's "survive restarts" requirement.
- `SinRecord` = today's `UrbanSin` + `rule_id`, `line: Option<u32>`, `evidence: String`, `content_hash`, `disposition: Open|Ignored|Fixed`, deterministic `id = sha256(relPath, rule_id, line, evidence_key)`.

**Roster → distinct visuals** (each check deterministic, explainable — the parchment shows rule, threshold, measured value):

| rule_id | Check (mechanism) | Visual anomaly (severity scaling) |
|---|---|---|
| `secret` | existing entropy/pattern scan | **Inferno** — hero fire (existing) |
| `env-missing` | existing `.env.example` diff | **Fire** at the building door |
| `dep-cycle` | Tarjan SCC on D3 import graph [R16] | **Tangled roads**: SCC roads re-tinted sickly-green + knot glyph at cycle centroid; severity = SCC size |
| `complexity` | per-function cyclomatic count via tree-sitter branch-node walk on the shared parse (McCabe [R16]); thresholds 15/25/40 | **Overgrown building**: ivy/crack overlay tier 1-3 (new atlas variants) |
| `clone` | token-hash duplicate detection (Rabin-Karp windows over tree-sitter token stream [R16]; jscpd-equivalent, in-process, no subprocess) | **Twin banners**: matching pennant glyph on both buildings + a dashed "smugglers' path" road between clones |
| `dead-export` | existing orphan-export check, upgraded to D3 AST import data | **Boarded windows** overlay |
| `todo-density` | existing TODO/FIXME counter, threshold density per KLOC | **Laundry lines / clutter** ground decoration (smoke severity only) |
| `god-file` | LOC + fan-in both above P95 of the project | **Strained tower**: cracks + dust puffs |
| `test-gap` | deterministic *presence* heuristic only: exported symbols in `src` with zero references from test files (data from D3 references) — NOT coverage instrumentation (out of alpha scope) | **Missing wall segment** around the building |

Severity scales the *intensity within* a visual (overlay tier, particle rate, halo radius), category picks the *kind* — the "not everything is generic fire" requirement. Legend panel gains one entry per glyph. LLM involvement: **none** in this rail. (If ever added, it must follow the suspect-smoke rule from D2: undetermined provenance renders as "?", never as a confirmed anomaly.)

### D8 — Bug-click resolution workflow

Clicking a building with active sins opens the parchment at the Anomaly ledger; each sin row offers exactly two actions:

- **Ignore** → `polis_dispose_sin(project_id, sin_id, "ignored")` sets `disposition: Ignored` in the Augure ledger. Semantics: ignored-at-this-content-hash; if the file's hash changes, the sin re-evaluates fresh (so ignores don't rot into false negatives), while an unchanged file keeps it hidden. Visual effect clears on the next event emit. **Ignored ≠ fixed:** `Fixed` is only ever set by the checker itself observing the condition gone at a new hash. Review path: the Command Deck **Anomalies** panel lists Ignored sins per project with un-ignore; the building parchment shows a small "𝑛 ignored" line so suppression is never invisible.
- **Send to main coder** → builds a scoped directive and calls the existing `spawn_main_coder_directive` (`main_coder.rs:96`; ≤4000 chars, ≤10 files — validated fit). Template:

```
Fix a single, precisely-scoped issue detected by deterministic analysis.
File: {rel_path} (lines {line_start}-{line_end})
Rule: {rule_id} — {title}
Evidence: {evidence}   (measured: {value}, threshold: {threshold})
Context: {oracle /context-bounded excerpt for this file+symbol, ≤2 chunks}
Constraints: touch only this file unless the fix is impossible without a
counterpart change; do not suppress or ignore the rule; state clearly if
you believe this is a false positive instead of "fixing" it.
```

- **Feedback loop into the city** — reuses existing visuals end-to-end: dispatch sets `agent_present` on the building ⇒ scaffolding + builder figure appear (already implemented, `growthEffects.ts`); the sin row shows *fix in flight* (directive id from the ledger). On directive completion: re-run the fine checks for that file — condition gone ⇒ `Fixed` ⇒ effect clears with the existing **golden seal**; still present ⇒ sin stays Open with a `fix_attempted` provenance entry and the parchment shows *fix failed — needs review* (amber tone); coder claimed false-positive ⇒ surfaced as a proposed-FP badge for the human to Ignore or keep. The checker, not the coder, is the arbiter of "fixed" — deterministic close-out, same philosophy as the detection side.

---

## 4. Module boundaries (concrete)

**Rust (new/changed):**
- `polis/source.rs` — `CityDataSource` trait + registry (D2); refactor of the attach-chain in `polis/commands.rs`.
- `polis/semantic.rs` — Oracle semantic client + `SemanticCache` (D1).
- `polis/augure/{mod,checks,ledger,scheduler}.rs` — D7 rail; `sins.rs` folds in.
- `backend/graph.rs` — import-edge extraction + Tarjan, built on `censor/extract.rs`'s parser and `structure.rs`'s walk (D3); `ckg.rs` emits IMPORT edges from it.
- `polis/commands.rs` — new commands: `polis_dispose_sin`, `polis_list_sins`, `polis_fix_sin`, `polis_detect_editors`, `polis_set_preferred_editor`.
- Oracle (Python): `/clusters` + members route, `file_clusters` table, cluster job at index completion (D1).

**Frontend (new/changed):**
- `cityStore`: `FilterState` + sin-ledger slice + new commands.
- `PolisBottomBar.tsx` → `CommandDeck` segments (Filters, Anomalies, Zoom).
- `InspectSidebar.tsx`: Kin buildings, Anomaly ledger + two-action rows, editor preference footer.
- `PolisRenderer.ts` + `growthEffects.ts` + new `fire.ts`: filter ghost pass, two-tier fire, halos, budget accumulator.
- `AgentLayer/AmbientLayer`: spline easing, entry slots, lane offsets.
- `types/city.ts`: additive fields (`Road.provenance`, `SinRecord`, filter types).

---

## 5. Prioritized implementation order (alpha)

Ordered by dependency and by user-visible value per unit of risk; each phase independently shippable:

1. **P1 — D7 Augure ledger + D8 workflow** (persist sins, dispositions, ignore/fix actions, dispatch button). Highest product value; depends on nothing new; upgrades existing checks before adding new ones.
2. **P2 — D3 import edges + Tarjan** (backend/graph.rs, roads provenance, dep-cycle sin upgrade, CKG IMPORT rows). Unblocks D7's graph-based checks (clone/dead-export/test-gap upgrades, god-file fan-in) and Oracle's `find_imports`.
3. **P3 — D6 Command Deck + parchment additions** (filters, zoom, anomaly panel, editor preference). Depends on P1 for the anomaly panel data.
4. **P4 — D7 new checks roster** (complexity, clones, god-file, test-gap, todo-density thresholds) + their visuals.
5. **P5 — D4 fire/lighting tiers + D5 locomotion polish.** Pure-visual; safe last among funded work.
6. **P6 — D1 clustering server-side + semantic roads/districts** (needs Oracle-side work + a "Re-classify"-style consent flow). D1's *cache + /similar* consumption can land earlier inside P3's Kin-buildings section (read-only, low risk).
7. **P7 (design-only now) — D2 trait refactor.** Do the refactor when the second source is real; until then keep the trait definition + the attach-chain fold, which P1–P6 already respect.

Per-phase: implement → verify on disk → one hostile reviewer → fix; whole-diff max-recall review at the end (house cadence).

---

## 6. Open questions & explicit assumptions

1. **HDBSCAN dependency** in Oracle's venv (P6): assumed acceptable; if not, k-means with silhouette-picked k, stdlib-only, worse noise handling.
2. **Import-path resolution accuracy** (D3): path-heuristic resolution will miss aliased/tsconfig-paths/re-exported imports; assumed acceptable for roads (visual weight, not build correctness). Stack-graphs-grade binding is explicitly deferred [R15].
3. **`spawn_main_coder_directive` semantics**: assumed a fire-and-track queue with completion observable from the agent ledger (recon shows the ledger + executor claim loop; the *completion callback* wiring for D8's re-check needs verification in `mini_coder_executor.rs` during P1).
4. **Idle detection** for the coarse scheduler: assumed derivable from the existing watcher debounce + agent-activity state; if not, a plain interval timer with a "not while scanning" guard is the fallback.
5. **Atlas variants budget** (D7 visuals): 6 new overlay glyph sets assumed to fit the existing `BuildingTextureAtlas` texture budget on LEAN profile; MINIMAL profile renders category-colored smoke only.
6. **entire.io contract details** are speculative by definition (D2); the trait is shaped by Oracle's real needs plus the documented relax-list, nothing more.

---

## 7. New ideas (proposed, not scoped)

1. **Churn heat → foot traffic:** bias ambient-walker destination weights by git-log touch frequency (deterministic, cheap via `git log --name-only` cache) — busy files visibly bustle; dovetails with the existing weight-biased strolls.
2. **Era time-lapse:** the era archive already stores full CityState snapshots (`eras/`); a parchment "chronicle" mode could scrub the city through past snapshots — zero new data collection.
3. **Aqueduct = test coverage** (post-alpha): if a coverage artifact (lcov) exists in the repo, render tested districts as served by an aqueduct network; deterministic (parsed artifact), no instrumentation by Polis itself.
4. **Minimap obelisk:** a corner minimap rendered once per rebuild from building coords (one small offscreen render target), viewport rectangle overlay; helps the P3 zoom work on big cities.
5. **Anomaly weather:** when city-wide open-sin density crosses thresholds, subtle global weather (haze) — a single screen tint reusing the day-cycle pipeline; the "how healthy is this repo" glance from across the room.
6. **Night watch:** while the D7 idle scheduler runs coarse checks, a lantern-bearing watchman walker patrols (claim from ambient crowd like Censor's firefighter did) — makes the invisible scheduler visible and honest.

---

## 8. Data-flow summary (one paragraph)

Filesystem walk (scanner) discovers buildings; the shared tree-sitter parse (extract.rs → graph.rs) produces import edges, complexity/clone/dead-export metrics, and CKG rows; the Augure rail evaluates its deterministic roster over scan + graph outputs, persists sin shards, and its dispositions gate what the city shows; Oracle enriches asynchronously through the SemanticCache (similar files, clusters, blurbs) without ever blocking a scan; CityState assembly folds scanner truth + source patches (D2 trait) + agents + suspects + cloud inventory and ships one snapshot over `polis://city-updated`; the frontend diffs it, applies FilterState as a visibility pass, renders anomalies by category-specific visuals with severity-scaled intensity under a measured frame budget, and the parchment exposes per-entity evidence with exactly two bug actions — Ignore (ledger disposition, hash-scoped) or Send-to-main-coder (scoped directive; the checker, not the coder, confirms the fix and clears the flame with a golden seal).
