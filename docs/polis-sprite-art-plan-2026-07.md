# Polis Sprite-Art Plan — 2026-07

**Goal.** Close the remaining gap to the Caesar III / AoE "colpo d'occhio": replace/augment the procedural vector art with **real sprite textures** (open-source assets, curated + recolored to the Polis palette). Geometry, layout, walkers' behavior, perf architecture all stay — this round is texture, variety, detail.

**Owner directives.** Fable executes (inline coding authorized for this visual/iterative work, same exception as T6). Delegate mechanical work (scripts, batch conversions) to mimo via pi; per-step reviews to deepseek-v4-pro via pi; final max-recall to Sonnet reviewers. Sources: Google only as *discovery* with license filter — every shipped asset must trace to a verifiable open license. **No Caesar III / Zeus / Pharaoh / AoE rips** (confirmed proprietary; Julius/Augustus are engines only).

**Mandatory loop.** Browser dev harness for every visual change: `POLIS_DEV=1 npx vite --port 5199` → `/polis-dev.html`, hard-reload after edits, judge via claude-in-chrome screenshots + `__POLIS_STATS()`. Never ship blind.

---

## 1. Ground truths (recon 2026-07-09, file:line verified)

- Pixi **8.18.1** + pixi-viewport 6, tile **96×48** (2:1), zoom 0.15–3.0, `depthKey = x+y`, kit light **NW** (`kitcd/iso.ts:49 SUN.dir`).
- **`buildingAtlas.ts` already renders buildings as Sprites** from a texture cache keyed `${purpose}:${level}` (15 archetypes × levels 0..4, `buildings/index.ts:82 buildBuilding`). Real textures plug into this seam — the pipeline (chunked build, LOD, culling, zIndex sort, selection, fire overlays) is untouched.
- Zero external assets today; all Graphics. `public/` → copied to `dist/` → bundled by Tauri (`frontendDist: ../dist`). CSP `img-src 'self' data:`, `connect-src 'self'` → local PNG atlases + spritesheet JSON via `PIXI.Assets` work with **no CSP change**.
- Terrain: single full-extent polygon + noise accents (`terrain.ts:83`), roads = plain fills in `PolisRenderer.syncRoads()` (ROAD palette :155-176), props/fields = 16-phase lattice scatters capped 2800/560, bridges procedural (`terrain.ts:289`).
- Walkers: procedural figures ~23px × scale 0.55 ≈ **13px on screen**, redrawn per frame via `Graphics.clear()+drawCitizen()` (`AgentLayer.ts`, `kitcd/people.ts`); crowd layer below buildings; EffectsBudget rungs 0..5.
- Render profiles rich/lean/minimal gate LOD + caps (`renderProfile.ts`).

## 2. Asset sources (licenses verified live 2026-07-09)

| Priority | Source | License | Use |
|---|---|---|---|
| 1 | **Screaming Brain Studios** (OGA "400+ Isometric Town Tiles" + itch.io Overworld/Wall/Floor/Object/**Temple** packs) | **CC0** | Base building skins (128×64 → ×0.75 fits our 96 tile), temple/columns for civic archetypes, ships **both light directions** — standardize on NW. Free to recolor to olive/terracotta. |
| 2 | **Unknown Horizons** (`github.com/unknown-horizons/unknown-horizons`, `content/gfx/`) | **CC-BY-SA 3.0** (art) | Mediterranean-trade buildings, ships, fields. Attribution + share-alike **on the art files only**: any sprite we edit gets republished under CC-BY-SA in CREDITS — app license unaffected. Curate light per-sprite (15 years of contributors). |
| 3 | **Yar's 64×64 Outside Tileset** (OGA) | CC-BY 3.0 | Trees/rocks/terrain nature (light-neutral, safe to mix). Skip its medieval buildings (wrong theme). |
| 4 | **Reiner's Tilesets** (reinerstilesets.de) | Custom: commercial OK, credit "Reiner 'Tiles' Prokein", no raw re-hosting | Candidate for **walk-cycle humans** + rural props — inventory/orientation must be verified visually (much is ¾-view RPG, not 2:1 iso). |
| 5 | **BuzPin "2D Isometric — Ancient"** (itch.io) | CC-BY-**ND** 4.0 | Accent props ONLY (monuments, fire, rocks) used **unmodified** — ND forbids recoloring. Drop if it clashes with the palette. |
| 6 | OGA CC0/CC-BY fire & smoke flipbooks; CraftPix freebies (baked-in use OK) | per-pack | FX and gap-filling; verify each pack's page. |
| — | Kenney iso packs (CC0) | CC0 | Placeholder-only (flat toy look — does not ship). |
| ✗ | Caesar/Zeus/Pharaoh, AoE, 0 A.D. (3D, needs a Blender bake rig — out of scope this round) | — | Not used. |

**Legal rail.** `public/polis/CREDITS.md` (shipped) + `docs/polis-art-ledger.json`: one entry per asset — source URL, author, license, modified yes/no, light direction. CC-BY-SA edits published (the edited PNGs are in the repo = published; CREDITS states their license). An asset without a ledger entry does not get committed.

## 3. Architecture

### 3.1 Asset layout & loader
```
public/polis/
  atlas/            buildings-0.png/.json, terrain-0.png/.json, walkers-0.png/.json, fx-0.png/.json
  CREDITS.md
tools/polis-art/    (NOT bundled)
  raw/              downloaded originals, per-source subdirs   [gitignored if huge; ledger records URLs]
  normalize.py      scale/anchor/recolor → staged/
  pack_atlas.py     staged/ → pixi spritesheet PNG+JSON (pow2 pages ≤2048²)
  manifest.py       emits src/components/polis/spriteManifest.ts (typed keys)
```
New module `src/components/polis/spriteAssets.ts`:
- `loadPolisSprites(renderer): Promise<SpriteBank | null>` — `PIXI.Assets` manifest load, non-blocking at Polis mount; `null`/partial ⇒ **procedural fallback per feature** (kit stays; product-generality + dev safety). One boot toggle `POLIS_SPRITES=0` in the harness to A/B.
- `SpriteBank.get(key): Texture | AnimFrames | null`, keys typed from the generated manifest.

### 3.2 Buildings (the seam)
- `buildBuilding()` consults the SpriteBank first: key `bld:${purpose}:${level}:v${variant}`, `variant = hash(fileId) % variantCount` (deterministic, stable across reloads, rng.ts style).
- Hit ⇒ Sprite (anchor bottom-center on the footprint front corner, `foot` metadata from the manifest, same zIndex/depth path). Miss ⇒ current procedural kit. `BuildingTextureAtlas` becomes a two-source cache (file texture | baked procedural) behind the same key — call sites unchanged.
- **Two prongs (owner decision 2026-07-09 — reuse our buildings first):**
  - **A5a — texture the existing kit (all 15 archetypes).** Keep the kit geometry (footprints, tier growth, silhouettes, roof coding all intact) and replace the flat/procedural face fills in `kitcd/iso.ts` (ashlar/plaster/marble/wood + roofs) with **real texture fills** (pixi 8 Graphics texture fill) from CC0 masonry/plaster/terracotta-tile sources. Baked once into `BuildingTextureAtlas` exactly as today ⇒ zero runtime cost delta, uniform look across every building, tier growth untouched. This is the default path for ALL buildings and the only path for large civics (temple 4×6, theater 5×4, fortress — no external asset has those footprints).
  - **A5b — selective whole-sprite replacement.** Only where a real sprite clearly beats the textured kit at tile scale in a harness A/B (likely: houses/workshops/markets, 1×1–2×2, SBS/UH singles ×0.75). Keyed `bld:purpose:level:v${variant}` so tier growth still swaps visuals per level. Composited large-civic sprites (offline assembly from the SBS Temple set) only if A5a's textured kit disappoints — escape hatch, not the plan.
  - **A5b VERDICT (2026-07-09, after the A5a harness pass): REJECTED — the textured kit wins.** Evidence: (1) UH building sources are 128×128 px — our kit bakes a 2×2 archetype at ~200–400 px (base diamond 192 px × dpr), so every candidate needs a ×1.5–3 upscale and reads soft/muddy next to the crisp vector bake, and worse at MAX_ZOOM 3; (2) only 4 of 63 UH archetypes fit the Mediterranean palette (pastryshop/winery/stonemason/farm — the rest is slate-roof/half-timber/colonial-tent northern European); (3) static sprites lose the per-level tier growth the owner explicitly wants, and UH's Fife projection sits slightly off our 2:1 grid. The `bld:` SpriteBank seam stays reserved in the key grammar (future hi-res sets can still slot in); no renderer wiring shipped. UH stays in use for trees (A4, shipped) and walkers (A6, verdict YES).
- State overlays unchanged: selection ring, fire tiers, filter dimming, pennants all operate on the container/Sprite. Tier-growth (`GrowthFx`) unchanged.
- Sin/roof color coding: where the sprite replaces roof-coded procedural roofs, encode kind via pennant/banner overlay + palette-tinted roof variants generated in the recolor step (CC0/CC-BY sources only).
- Shadows: prefer assets **without** baked ground shadow; keep our contact-shadow sprite. Per-variant manifest flag `hasBakedShadow` disables ours when unavoidable.

### 3.3 Terrain, roads, props, fields
- Ground: keep the O(1) base polygon; add a `TilingSprite` grass texture (subtle, seamless, palette-matched) at low alpha over it + sprite decals replacing part of the noise accents. LOD-gated like today.
- Roads: cobble/dirt **texture fills** (pixi 8 Graphics texture fill) on the existing trunk/minor geometry — no re-routing.
- Props: olive trees/cypress/rocks/stalls → sprite variants (3–5 each), same 16-phase scatter + caps + 80-chunk discipline (sprites batch natively; chunking then applies to containers, not fill ops).
- Fields: crop/vineyard/orchard textures per parcel kind; keep `planFields`.
- Bridges/water stay procedural this round (T6 already fixed them; water shimmer is fine).

### 3.4 Walkers
- Dedicated sourcing pass (Reiner's 2d-humans + OGA "isometric walk cycle" CC0/CC-BY). Requirements: ≥4 directions (mirror to 8), ancient/neutral dress, readable at 13–40px, NW-compatible light.
- Found ⇒ pooled `AnimatedSprite` (frames from walkers atlas, direction from velocity, StepClock-driven frame index — replaces per-frame Graphics redraw: prettier AND cheaper). Citizen-type → sprite skin mapping; livery/role tints via `.tint`.
- Not found / partial ⇒ **explicit fallback:** keep procedural figures (or bake them to flipbook textures for the perf win alone). Walkers must not block the round.

### 3.5 FX
- Fire/smoke: real flipbook frames into the existing two-tier system (`bakeFireAtlas` swaps procedural frames for loaded ones; F1/F2 + EffectsBudget logic untouched).

## 4. Perf & memory budget
- Atlas pages 2048² RGBA, target **≤6 pages** resident (~100MB GPU worst case) and **≤12MB PNG** added to the bundle; `pack_atlas.py` fails the build over budget. Walkers/FX atlases load lazily at `lodAgents`/`LOD_DISASTER` zoom.
- Frame budget unchanged: avgFrameMs ≤ ~10ms on the reference fixture (T6 baseline 8.4ms) — measure via `__POLIS_STATS()` before/after every phase; sprites batch better than Graphics, regressions mean a texture-thrash bug.
- Base assets ≥128px wide per tile-unit so max zoom (3.0) upscale ≤ ~2.3× with linear filtering (Caesar-like softness is acceptable; verify in harness).
- lean/minimal profiles: sprites follow existing LOD gates; no new per-frame allocations (pool everything — T5/P5 rule).

## 5. Phases

Cadence per phase: implement → harness screenshot before/after → vitest/tsc → **commit** → deepseek-v4-pro review (code phases) → fix → next. Test-count baseline check after every pi task. Every pi spec opens with the git-mutation ban preamble.

| Phase | Content | Who |
|---|---|---|
| **A0 Groundwork** | `spriteAssets.ts` loader + manifest types + fallback wiring + `POLIS_SPRITES` toggle; `public/polis/` + CREDITS/ledger scaffold; harness serves atlases (vite public — should be zero-config, verify). Unit tests: loader failure ⇒ fallback, key resolution, variant hash stability. | Fable |
| **A1 Sourcing & curation** | Download SBS packs, UH `content/gfx`, Yar, fire flipbooks, BuzPin free tier; build a throwaway gallery page in the harness to eyeball everything at 96-tile scale; pick light direction (NW), select per-archetype candidates; fill the ledger. Walker sourcing pass (Reiner's + OGA) decided here. | Fable (visual judgment) + Bash for fetching |
| **A2 Pipeline tooling** | `normalize.py` (scale ×k to tile grid, trim, anchor metadata, optional HSV/LUT recolor toward PALETTE olive/terracotta), `pack_atlas.py` (pixi JSON, pow2, budget guard), `manifest.py` (typed TS keys + foot/anchor/hasBakedShadow). Golden tests on fixtures. | **mimo-v2.5 via pi** (spec'd, safety preamble); Fable verifies output on disk |
| **A3 Terrain & roads** | Grass TilingSprite + decals; road texture fills; recolored where needed. Harness A/B vs T6 baseline. | Fable |
| **A4 Props & fields** | Tree/rock/stall sprite variants in the scatter; field textures per parcel kind. | Fable |
| **A5 Buildings** | **A5a first:** real texture fills into the kit faces/roofs (`kitcd/iso.ts` MAT + texture helpers) — all archetypes upgrade at once, growth/footprints untouched. **A5b second:** harness A/B per small archetype (houses/workshops/markets) — whole-sprite replacement via the SpriteBank seam only where it clearly wins; decisions recorded in the manifest. Split commits per prong/archetype group. | Fable (integration) + **mimo** for batch texture-prep scripts |
| **A6 Walkers** | Per A1 verdict: AnimatedSprite pipeline + pooling + tint livery, or documented fallback (bake procedural figures to flipbooks). | Fable |
| **A7 FX** | Fire/smoke flipbooks into F1/F2. | Fable |
| **A8 Hardening & recall** | Perf audit vs budgets (rich/lean/minimal), texture GC check (folder switch ×10 in harness, watch heap), exe-size check, CREDITS completeness vs ledger; **max-recall: 3 Sonnet reviewers (angles: correctness/perf+memory/licensing+fallback) + adversarial verify of conflicts**; owner harness e2e. | Fable + Sonnet fleet |

Ordering rationale: A3/A4 land the biggest cheap wins early (ground+vegetation richness transforms the first impression); A5 is the long pole; A6/A7 polish. Owner can call a look at the harness after any phase.

## 6. Risks & mitigations
- **Style incoherence across sources** → single light direction (NW), recolor LUT toward PALETTE, gallery-page curation before integration, per-archetype keep-procedural escape hatch.
- **No good walker set exists** → explicit A6 fallback; round does not block.
- **CC-BY-SA contamination fear** → art-only share-alike, edited UH sprites listed + licensed in CREDITS; no source code involved. ND assets never modified.
- **Exe size / GPU memory creep** → hard budgets in `pack_atlas.py` + A8 audit; lazy walker/FX atlases.
- **mimo rewrites earlier edits on fix-passes** (known pitfall) → re-verify each scripted output on disk; scripts have golden tests.
- **Harness HMR staleness / viewport scale Point** (T6 pitfalls) → hard-reload discipline; never assign a number to `viewport.scale`.

## 7. Out of scope (this round)
0 A.D. 3D→sprite bake rig; districts layer art; real-exe e2e (still owed globally, tracked separately); sin-smoke redesign beyond flipbook swap.
