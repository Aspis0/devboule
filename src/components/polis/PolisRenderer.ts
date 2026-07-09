// PolisRenderer — framework-agnostic isometric city renderer.
//
// Takes a CityState + a PIXI.Application (and a pixi-viewport Viewport) and
// renders the map across z-ordered layers using procedural PIXI.Graphics only
// (no sprite sheets). PRINCIPLE: render ONLY what the backend provides — never
// invent buildings, roads or agents. Terrain + props are DECORATION and never
// imply a file exists.
//
// Visual direction: late-90s/early-2000s isometric city builders (Caesar III,
// Pharaoh, Zeus). Key techniques, split into modules:
//   - terrain.ts : chunky per-tile value-noise ground + seams + dirt patches.
//   - props.ts   : deterministic flora/rocks/stalls on EMPTY tiles.
//   - buildings.ts: seeded asymmetric buildings (outline, windows/door, tiled
//                   roofs, per-type silhouettes, banners) + drop shadows.
//   - effects.ts : 30fps STEPPED ambient anim (pooled smoke, water shimmer,
//                  flame flicker, flag poses) — allocation-free.
//   - rng.ts     : deterministic hash + PRNG so a re-scan reproduces the city.
//
// Performance:
//   - ALL geometry is built ONCE in setCityState. The ticker only mutates
//     transform / alpha / visibility / tint on pre-built Graphics, or recycles a
//     fixed particle pool. No Graphics.clear()+refill per frame.
//   - Buildings group into CHUNK_SIZE-tile chunks; the cull ticker toggles whole
//     chunks visible/invisible against the viewport. Only elements in VISIBLE
//     chunks are animated.
//   - LOD hides building labels + fine details below zoom thresholds.

import {
  Application,
  Container,
  Graphics,
  Sprite,
  Text,
  TextStyle,
  Rectangle,
  FillGradient,
} from "pixi.js";
import { Viewport } from "pixi-viewport";
import type {
  CityState,
  Building,
  Road,
  District,
  Agent,
  ExternalService,
  GridSize,
  TerrainData,
} from "../../types/city";
import {
  cartToIso,
  depthKey,
  lerp,
  dist,
  darken,
  lighten,
  saturate,
  blend,
  type IsoPoint,
} from "./iso";
import { PALETTE, ALPHA, getProfile, tierScale, tierRank } from "./palette";
import { AgentLayer } from "./AgentLayer";
import { AmbientLayer, desiredAmbientCount } from "./AmbientLayer";
import { PossessionController, type PossessionEnv } from "./possession";
import {
  CensorPresence,
  CENSOR_FIGURE,
  type CensorEnv,
  type CensorDecision,
  type CensorFindingsPayload,
  type GemmaStatus,
  type ResolvedBuilding,
} from "./censorPresence";
import { TradeRouteLayer, TRADE_LOD_ZOOM } from "./TradeRouteLayer";
import { ExternalServiceLayer } from "./ExternalServiceLayer";
import { RoadGraph } from "./roadGraph";
import { makeWaterBlocker, makeBuildingBlocker, combineBlockers } from "./navWalkable";
import {
  computeExtent,
  drawTerrain,
  buildTerrainFrame,
  type TerrainChunk,
} from "./terrain";
import { occupiedTiles, drawProps } from "./props";
import { planFields, drawFields, parcelTiles, buildFieldBlockedSet } from "./fields";
import { buildBuildingParts, type BuiltParts } from "./buildings";
import { BuildingTextureAtlas } from "./buildingAtlas";
import type { AnimInstance } from "./kitcd/anims";
import { buildingChanged, worstSinSeverity } from "./diffCity";
import { SlotAllocator } from "./locomotion";
import { StepClock } from "./effects";
import type { FilterSets } from "./filterModel";
import { GrowthFx, Scaffold, Disaster, Investigation } from "./growthEffects";
import { EffectsBudget, type BudgetRung } from "./effectsBudget";
import {
  bakeFireAtlas, destroyFireAtlas, createCrowdFire, stepCrowdFire,
  createHeroFire, retargetHeroFire, stepHeroFire, parkHeroFire,
  beginDemotionCrossfade, rankForPromotion,
  type FireAtlas, type CrowdFire, type HeroFire, type FireSeverity,
  type PromotableBuilding,
} from "./fire";
import { sliceBatches, DEFAULT_BUILD_BATCH } from "./chunk";
import {
  orderBuildQueue,
  priorityFromKeys,
  expandChunkRing,
  type BuildQueueItem,
} from "./buildQueue";
import { profileFor, type RenderProfile, type HardwareInfo } from "./renderProfile";

const CHUNK_SIZE = 8; // tiles per chunk side

// LOD zoom thresholds for LABELS / DETAILS / AGENTS are now HARDWARE-ADAPTIVE
// (Phase B2c): the renderer holds per-instance fields (`lodLabelsIn`,
// `lodLabelsOut`, `lodDetails`, `lodAgents`) seeded from the chosen RenderProfile,
// and the LOD pass + the build prioritization read THOSE. The canonical RICH-tier
// values (the historical 0.62/0.58/0.4/0.35 — and the safe `null`-profile default)
// live in `renderProfile.ts` (the `RICH`/`LEAN` tiers). The HYSTERESIS rationale
// for the label IN/OUT dead-band (the only band that ALLOCATES a Text per building
// on each crossing — ~879 on a large city — so a single threshold thrashes
// create-all/destroy-all when zoom wobbles across the line) still governs the
// `lodLabelsIn`/`lodLabelsOut` pair in `updateCulling`.
//
// The DISASTER / EXTERNAL / LIVERY / ROAD-MINOR bands below are NOT profile-gated
// (they gate cheap, allocation-free toggles), so they remain fixed module constants.
// On-map DISASTER overlay (burning buildings with urban sins). Disasters MATTER,
// so they read a touch sooner than fine facade detail — but they are still hidden
// in the far overview (same band as agents/outposts) so a zoomed-out city isn't a
// field of tiny flames and the per-step fire redraw is skipped when off. When
// hidden, `Disaster.update` early-returns on `node.visible === false`.
const LOD_DISASTER = 0.35;
// Farmland parcels: too fine at the far overview; hidden below this zoom.
const LOD_FIELDS = 0.3;
// External cloud outposts (harbour nodes) at the map margin: small procedural
// structures, legible once the city itself is readable. Hidden in the far view
// (same band as agents) so the margin doesn't speckle the zoomed-out overview.
const LOD_EXTERNAL = 0.35;
// TECH LIVERY (F4): provider pennants are a fine detail — only legible when
// zoomed in past ~0.5, so they are hidden in the far view (avoids a confetti of
// tiny flags over a dense city). Reuses the same threshold band as minor roads.
const LOD_LIVERY = 0.5;
// Below this zoom only the trunk network shows; minor lanes fade out so the
// far view reads as a few avenues, not a mesh. Between this and full zoom the
// minor lanes ramp from faint to their drawn alpha.
const LOD_ROAD_MINOR = 0.5;

// Road hierarchy thresholds. A segment is a TRUNK when it carries real traffic:
// either it is shared by several routed roads (>= ROAD_SHARED_TRUNK incident
// roads) or the import itself is heavy (weight >= ROAD_WEIGHT_TRUNK). Everything
// else is a faint MINOR lane. (Fixture: segment usage runs 1..13, weights 1..5;
// these cut the 35% weight-1 / single-use noise from the cobbled trunks.)
const ROAD_SHARED_TRUNK = 3;
const ROAD_WEIGHT_TRUNK = 4;
// A genuine junction disc is only worth drawing where this many routed roads
// actually meet (raised from 3 so we stop dotting every minor kink).
const ROAD_JUNCTION_MIN = 4;

// ---------------------------------------------------------------------------
// Road palette — muted, derived from PALETTE so the cobble stops reading red
// against the green terrain. The OLD cobble used PALETTE.sandDark (0xc8b89a)
// + a stoneDark kerb at alpha 0.85, which stacked into harsh brown blobs. The
// new scheme is a calm two-tier stone/earth set:
//   - TRUNKS get a desaturated stone cobble (two close tones) + faint kerb.
//   - MINOR lanes get a single thin earth line at low alpha (a hint of a path).
// All values are pure functions of PALETTE entries (palette discipline kept).
const ROAD = {
  // Trunk cobble: PALETTE.stone desaturated toward neutral so it sits BEHIND
  // the buildings instead of competing. Two near tones for a subtle cobble
  // weave without the old red/brown clash.
  trunkStone: saturate(PALETTE.stone, -0.4),
  trunkStoneAlt: saturate(darken(PALETTE.stone, 0.1), -0.4),
  // Trunk side kerb — a soft, low-contrast edge (was full stoneDark @ .5).
  trunkKerb: saturate(PALETTE.stoneDark, -0.35),
  // Minor lane: a single faint earth line, desaturated so it whispers a path.
  minorPath: saturate(lighten(PALETTE.stoneDark, 0.18), -0.45),
  // Junction node: a subtle neutral disc, only on true trunk hubs.
  junction: saturate(PALETTE.stoneDark, -0.3),
} as const;

// Road alphas — flat per layer so OVERLAPS don't multiply into dark blobs.
// Everything in a tier draws into ONE batched Graphics at a CONSISTENT alpha;
// drawing many segments at the same alpha into the same Graphics fill does not
// re-darken shared pixels, so crossings stay clean.
const ROAD_ALPHA = {
  trunkFill: 0.7, // cobbled trunk body (was 0.85, and stacking)
  trunkKerb: 0.28, // faint trunk edge
  minor: 0.3, // faint minor lane line at full zoom
  junction: 0.35, // trunk-hub disc
} as const;

// Clamp the per-step delta handed to the kit anims so a long stall (tab in the
// background) can't make a flame jump or a smoke puff teleport. Matches the
// source harness's `Math.min(0.05, dt)`.
const MAX_ANIM_DT = 0.05;

// ---------------------------------------------------------------------------
// DAY CYCLE (Polis L1) — a SLOW screen-space warmth that lerps noon → evening →
// noon on a multi-minute loop, so the city always feels alive even when nothing
// changes. Visual only: never persisted, no determinism requirement. Stays
// firmly ON-PALETTE — the "evening" pole is a warm terracotta/gold, NEVER a
// jarring blue night — and at a very low alpha so it tints, never paints over.
// ---------------------------------------------------------------------------
// Full loop period (ms). 4 minutes: noon at t=0, evening at the half, back to
// noon at the end — slow enough to be a mood, not an effect you "watch".
const DAY_CYCLE_MS = 240_000;
// The two warmth poles (both derived from PALETTE — no fresh hex). Noon is the
// cream/ivory daylight; evening pulls toward a warm terracotta-gold dusk.
const DAY_TINT_NOON = lighten(PALETTE.cream, 0.04); // bright, neutral-warm
const DAY_TINT_EVENING = saturate(blend(PALETTE.terracotta, PALETTE.goldAccent, 0.5), 0.08);
// Alpha poles — both subtle (~0.05 noon, ~0.12 evening) so the tint is a whisper.
const DAY_ALPHA_NOON = 0.05;
const DAY_ALPHA_EVENING = 0.12;

export interface PolisRendererCallbacks {
  onSelectBuilding?: (building: Building | null) => void;
  onHoverBuilding?: (building: Building | null) => void;
  onSelectAgent?: (agent: Agent | null) => void;
  /** A merchant porter (or its road) was clicked — surface the REAL import edge
   *  `from` (importer/consumer) imports `to` (imported/supplier). */
  onSelectConnection?: (from: string, to: string) => void;
  /** A cloud outpost ("harbour" node) was clicked — surface the REAL external
   *  service (provider/type/name/status) in the inspect sidebar. Null clears. */
  onSelectExternalService?: (service: ExternalService | null) => void;
}

/** Progress of the chunked city build. `done`/`total` are building counts;
 *  `phase` is "building" while batches are still being added and "done" once the
 *  whole scene (buildings + agents + camera) is in place.
 *
 *  B2b (ADDITIVE): `visibleDone`/`visibleTotal` track the VIEWPORT-PRIORITY subset
 *  — the buildings in the viewport + preload-ring chunks that build FIRST. When
 *  `visibleTotal > 0` and `visibleDone >= visibleTotal`, the city the camera can
 *  see is placed and the map is effectively interactive even though the background
 *  fill of distant chunks continues. Optional so existing consumers that read only
 *  `done`/`total`/`phase` are unaffected. */
export interface BuildProgress {
  done: number;
  total: number;
  phase: "building" | "done";
  /** Count of priority (viewport+ring) buildings placed so far. */
  visibleDone?: number;
  /** Total priority (viewport+ring) buildings for this build. 0 when the build was
   *  not viewport-prioritized (empty viewport / headless). */
  visibleTotal?: number;
}

interface BuildingNode {
  building: Building;
  iso: IsoPoint;
  /** The node ROOT Container — added to a chunk; positioned at the building's iso
   *  (front-bottom) anchor exactly as before. Its FIRST child is `bodySprite` (the
   *  batched, shared-texture static body); the live overlays (anims, pennant,
   *  scaffold, disaster, investigation, label) are its other children, and the
   *  growth transitions mutate its transform. Kept as a Container (not the Sprite
   *  itself) so it idiomatically owns children in pixi v8 and every existing
   *  consumer (growthFx, LOD alpha, overlay parenting, eventMode) is unchanged. */
  container: Container;
  /** The batched static-body Sprite (shared per-variant texture from the atlas),
   *  child 0 of `container`. The TEXTURE is atlas-owned — destroyed with neither
   *  the sprite nor the container. Swapped in place on a tier/variant change. */
  bodySprite: Sprite;
  /** Drop-shadow SPRITE (shared per-variant texture; lives on the shadows layer,
   *  not in `container`). Tracked so a live-diff removal can destroy it. The
   *  TEXTURE is owned by the atlas and is NOT destroyed with the sprite. */
  shadow: Sprite;
  /** The kit's live animated instances (Flame/Beacon/Flag/Smoke/Water). Built
   *  ONCE per building; their update() mutates their own small Graphics. Driven
   *  by the step clock for VISIBLE chunks only. Empty for static buildings. */
  kitAnims: AnimInstance[];
  /** The filename label Text, or null when LOD-hidden (zoomed out). Now ATTACH-ON-
   *  DEMAND: the LOD pass creates it on zoom-in past LOD_LABELS and DESTROYS it on
   *  zoom-out (not just visibility-toggled), so a far view of a huge city retains
   *  zero label glyphs — the heap win for labels. */
  label: Text | null;
  /** Silhouette pixel height above the iso anchor — kept so the LOD pass can
   *  re-create the label (positioned at `-labelDepth - 6`) without re-measuring. */
  labelDepth: number;
  /** TECH LIVERY (F4): the provider pennant Graphics (child of `container`), or
   *  null for files with no provider. LOD-gated (hidden below LOD_LIVERY) and
   *  destroyed with `container` (children:true) — no separate disposal. */
  pennant: Graphics | null;
  /** On-map DISASTER overlay (kit Flame/Smoke composed by worst sin severity), or
   *  null when the building has no sins. A CHILD of `container` (torn down by the
   *  node's `destroy({children:true})` — no separate disposal) and a reference in
   *  `kitAnims` (driven by the step clock, no separate driver). Tracked here only
   *  so the LOD pass can toggle its visibility by zoom. */
  disaster: Disaster | null;
  /** On-map INVESTIGATION overlay (bug-investigation P3): tinted kit Smoke + a "?"
   *  marker when this building is an OPEN bug card's Oracle suspect
   *  (`suspectOfCardId` set), or null otherwise. A CHILD of `container` (torn down
   *  by the node's `destroy({children:true})` — no separate disposal) and a
   *  reference in `kitAnims` (driven by the step clock). COEXISTS with `disaster`:
   *  a suspect that is also a confirmed disaster shows BOTH. Tracked here only so
   *  the LOD pass can toggle its visibility by zoom. */
  investigation: Investigation | null;
  hitRadius: number;
  chunkKey: string;
}

/**
 * Remove the FIRST occurrence of `item` from `list` by REFERENCE identity, in
 * place. Returns true iff it was present. Extracted (and exported) so the
 * animated-node lifecycle invariant is testable headless: the renderer tracks
 * animated building nodes in `animatedNodes` and the per-step clock walks that
 * array, so a node DESTROYED via `destroyBuildingNode` MUST be spliced out or a
 * stale (destroyed-container) entry would be stepped next frame — a use-after-free
 * leak. `destroyBuildingNode` calls this; a focused test pins that a removed node
 * no longer appears, while siblings survive. (No-op when the item is absent —
 * destroy is safe to call on a static, never-tracked node.)
 */

/** P5.1 — deterministic seeded phase from fileId (same algo as fire.ts:seededPhase,
 *  inlined here to avoid an import cycle with fire.ts). */
function seededPhaseFromId(fileId: string): number {
  // Use the hashString from rng.ts (already imported)
  let h = 1779033703 ^ fileId.length;
  for (let i = 0; i < fileId.length; i++) {
    h = Math.imul(h ^ fileId.charCodeAt(i), 3432918353);
    h = (h << 13) | (h >>> 19);
  }
  h = Math.imul(h ^ (h >>> 16), 2246822507);
  h = Math.imul(h ^ (h >>> 13), 3266489909);
  h ^= h >>> 16;
  return ((h >>> 0) % 9973) / 9973 * 100;
}

export function removeFromArrayByIdentity<T>(list: T[], item: T): boolean {
  const idx = list.indexOf(item);
  if (idx >= 0) {
    list.splice(idx, 1);
    return true;
  }
  return false;
}

/** P3.2 — Pure per-node filter verdict. Exported so the filter test can import
 *  the PRODUCTION function (not a stub). The renderer uses this through
 *  applyFilterToNode and the filter-aware LOD helpers. */
export interface FilterVerdict {
  ghosted: boolean;
  hide: boolean;
  effectsHidden: boolean;
}

export function nodeFilterVerdict(
  fileId: string,
  sets: import("./filterModel").FilterSets | null,
): FilterVerdict {
  if (!sets) return { ghosted: false, hide: false, effectsHidden: false };
  const ghosted = sets.ghostedFileIds.has(fileId);
  return {
    ghosted,
    hide: ghosted && sets.mode === "hide",
    effectsHidden: sets.effectsHiddenFileIds.has(fileId),
  };
}

interface Chunk {
  container: Container;
  bounds: Rectangle;
  visible: boolean;
}

export class PolisRenderer {
  private app: Application;
  private viewport: Viewport;
  private callbacks: PolisRendererCallbacks;

  // z-ordered layer stack (terrain at the bottom, ui at the top). `shadows`
  // sits between roads and buildings so drop shadows ground the buildings.
  private layers = {
    terrain: new Container(),
    districts: new Container(),
    roads: new Container(),
    // Trade-route porters: their OWN layer, ABOVE the road cobble but BELOW the
    // buildings layer. Below-buildings is the click-discipline (a building body
    // always wins an overlapping hit-test, so a porter never steals a building's
    // click). A dedicated layer (NOT a child of `roads`) so the road redraw —
    // which clears the whole `roads` layer on every live diff — can't destroy
    // the porter pool out from under the layer.
    tradeRoutes: new Container(),
    shadows: new Container(),
    // Ambient roaming crowd: BELOW buildings (same occlusion discipline as the
    // tradeRoutes porters). Crowd walkers live on the road network; in iso a
    // building sprite only extends UP-screen from its base, so a walker passing
    // NORTH of (behind) a building is correctly hidden by it, while a walker on
    // the road south of it never overlaps the sprite. Real, arrow-marked agents
    // stay on the `agents` layer above buildings so a parked omino at a door is
    // never swallowed by the facade.
    crowd: new Container(),
    buildings: new Container(),
    // External cloud services ("harbour / cloud outposts") at the seaward margin.
    // Its OWN layer (above buildings, below agents) so the road/terrain redraws on
    // a live diff can't tear the pooled outpost nodes down, and so an outpost click
    // is independent of building picking. The nodes sit OUTSIDE the building grid
    // (placed by the Rust backend), so they never overlap a building in practice.
    external: new Container(),
    agents: new Container(),
    effects: new Container(),
    halos: new Container(),
    ui: new Container(),
  };

  private agentLayer: AgentLayer;
  // Decorative ambient crowd (NOT data — see AmbientLayer header). Wanders the
  // road network behind the real, arrow-marked agents.
  private ambientLayer: AmbientLayer;
  // Polis-P4 — PURE possession decision layer: makes an activating agent take
  // possession of an idle roaming omino (claim-from-crowd) and walk it to its
  // building, falls back to a fresh spawn, releases on vanish (adopting a claimed
  // omino back into the crowd — the claimedCount contract), and derives the small
  // per-subagent omini. The renderer applies its decisions against AgentLayer +
  // AmbientLayer via `reconcileAgents`.
  private possession = new PossessionController();
  // Polis-P5 — PURE Censor firefighter presence. Driven by the REAL
  // `censor://findings-updated` event (fed via {@link onCensorFindings}) + the
  // gemma availability ({@link setCensorGemmaStatus}), NOT by `city.agents` — Censor
  // is an ENGINE, never an agent, so it is never in the fleet roster / the agent
  // diff. It claims/releases an idle firefighter from the ambient crowd (the same
  // claimedCount contract as possession) and walks it between reviewed buildings.
  private censor = new CensorPresence();
  // The single Censor firefighter omino id (keyed by the watched projectId). Set
  // when the first findings event names a project; null while none is active. The
  // AgentLayer external map is keyed by this so the firefighter never collides with
  // a real agentId and never appears in the roster.
  private censorOminoId: string | null = null;
  // Stable id of the project the Censor firefighter is currently bound to. A
  // findings event for a DIFFERENT project releases the current firefighter first
  // (single-active, mirroring the backend's single-active watch model).
  private censorProjectId: string | null = null;
  // Polis-P6 — memoized CensorEnv. The env's closures dereference `this.*` (the
  // building lookups, the AmbientLayer/AgentLayer, the omino id) LAZILY on each
  // call, so a SINGLE instance stays valid for the renderer's whole lifetime even
  // as the scene rebuilds — there is no per-scene state captured by value. This
  // avoids re-allocating 4 closures + an object every frame in `tickCensor` while a
  // findings burst is pending (the P5-review nitpick). Built once on first use.
  private cachedCensorEnv: CensorEnv | null = null;
  // #10 — memoized findRoute closure for applyCensorDecisions. Reads `this.roadGraph`
  // lazily so it survives scene rebuilds (the graph swaps under it); built once on
  // first use to avoid re-allocating a closure on every Censor decision flush.
  private cachedCensorFindRoute:
    | ((from: string, to: string) => IsoPoint[] | null)
    | null = null;
  // filePath → fileId lookup for the Censor relPath→building resolution. Built
  // alongside `buildingNodes` (same lifecycle); a normalized project-relative path
  // maps to the building's stable fileId, then `buildingNodes.get(fileId)?.iso`
  // gives the iso — the SAME anchor agents resolve to. Rebuilt on every build/diff.
  private fileIdByPath = new Map<string, string>();
  // Polis 4a — DATA-BOUND trade-route porters (TradeRouteLayer). Merchant figures
  // that walk the REAL top-weight import roads supplier→consumer so the busiest
  // dependencies read as the busiest streets. ZOOM-IN ONLY (hidden below
  // TRADE_LOD_ZOOM) and visible-chunk-only. Rebuilt ONLY when roads change.
  private tradeRouteLayer: TradeRouteLayer;
  // Polis 5 — external cloud services ("the city meets the cloud"). Data-bound
  // procedural harbour/outpost nodes at the seaward margin, sourced ONLY from the
  // real synced provider inventory (`city.externalServices`). Pooled by serviceId,
  // LOD-gated, clean teardown. Never buildings/agents/files.
  private externalLayer: ExternalServiceLayer;
  // Polis L2 — DATA-DRIVEN growth visuals (scaffolding / tier-grow / pop-in /
  // rubble / golden-seal). Pooled one-shot bursts live on the effects layer;
  // scaffolding is parented into each building node (torn down with it). See
  // growthEffects.ts. Fired from the live diff on real agentPresent/tier/add/
  // remove deltas — never for the ambient crowd.
  private growthFx: GrowthFx;
  private clock = new StepClock();
  // Running animation time (seconds) handed to the kit anim instances'
  // update(t, dt). Advanced once per step in update(); only visible-chunk anims
  // are actually ticked.
  private animT = 0;

  // P3.2 — Filter pass state. Null means no filter applied.
  private filterSets: FilterSets | null = null;

  // Building-level road navigation graph for the AgentMover. Rebuilt once
  // whenever roads change (setCityState + applyCityDiff); agents query it for a
  // walkable route only when they actually change building. Null until the first
  // city is set or when there are no roads.
  private roadGraph: RoadGraph | null = null;
  // T2 — walk blocker: true on tiles walkers must never stand on (water/buildings).
  // Built once per city load (where RoadGraph is constructed) and passed to
  // AgentLayer and AmbientLayer. No per-frame construction.
  private blocked: (gx: number, gy: number) => boolean = () => false;
  // Stable signature of the roads the current `roadGraph` was built from. A live
  // diff compares the incoming roads' signature to this; only when it DIFFERS do
  // we rebuild `roadGraph` + the road-dependent ambient crowd. A pure
  // sin/status/provider change leaves roads identical, so the O(E) graph rebuild
  // + ambient weight-table rebuild are skipped. Null until the first build.
  private lastRoadSig: string | null = null;
  // Cheap stable signature of the last RENDERED terrain frame. A pure tier change
  // (no add/remove, no road change) can still grow a building's footprint → the
  // backend recomputes `sea_x` + the sea band, so the terrain frame moves with NO
  // add/remove/road signal. We redraw the (heavy) terrain only when this signature
  // actually changes, so a pure sin/agent/provider diff (terrain identical) stays
  // cheap. Null until the first build.
  private lastTerrainSig: string | null = null;

  private buildingNodes = new Map<string, BuildingNode>();
  // SPRITE-SHEET BUILDINGS — lazy per-variant texture cache. Each building on the
  // map is ONE batched Sprite referencing a shared texture keyed by (purpose,
  // level); the heavy static Graphics body is rendered ONCE per variant and
  // destroyed, killing the ~1MB/building retained-Graphics heap. The cache warms
  // naturally as the build loop places the first building of each variant. Owned
  // by the renderer; released in destroy(). dpr-aware, capped resolution.
  // B2c — the atlas resolution is the device pixel ratio CAPPED by the profile's
  // `atlasResolutionCap` (min(dpr, cap)). A lean/minimal tier caps at 1 (no HiDPI
  // super-sampling) to bound texture memory. Assigned in the constructor (after the
  // profile is chosen) — see below.
  private buildingAtlas!: BuildingTextureAtlas;
  private chunks = new Map<string, Chunk>();
  // Polis terrain frame (sea/rivers/shores/bridges) chunks. Parented into the
  // terrain layer (BELOW buildings, correct water-under-buildings draw order),
  // keyed by the SAME CHUNK_SIZE as building chunks so the cull pass toggles
  // them in lockstep with the buildings overhead. Built ONCE per city in
  // redrawTerrainProps; only VISIBLE chunks' water shimmer is ticked per frame.
  private terrainChunks: {
    chunk: TerrainChunk;
    bounds: Rectangle;
    visible: boolean;
  }[] = [];
  // Fields (farmland parcels) — one Graphics, LOD-gated at zoom >= 0.3.
  private fieldsGraphics: Graphics | null = null;
  // T6a — terrain grid Graphics tracked separately for zoom gating (sub-pixel
  // below zoom 0.5; hidden there to save ~557 draw calls).
  private terrainGridGraphics: Graphics | null = null;
  private animatedNodes: BuildingNode[] = []; // nodes with >=1 kit anim instance
  // The last CityState rendered (full build OR live diff). Diff input for the
  // next live update; null until the first setCityState. We snapshot the
  // building extent (terrain bounds) so a diff only redraws terrain/props when a
  // building lands outside it.
  private lastCity: CityState | null = null;
  private destroyed = false;
  private cullTick: (() => void) | null = null;
  private lastScale = -1;

  // B2c — HARDWARE-ADAPTIVE render profile. Chosen ONCE (constructor) from the
  // detected hardware; defaults to the safe MIDDLE tier when no profile is passed
  // (a null-hw renderer). The LOD bands below are SEEDED from it (and from the rich
  // module constants when absent), so the LOD pass + the build prioritization read
  // per-instance values that scale to the host. Never reassigned after construction
  // (a hardware change would require a remount).
  private profile: RenderProfile;
  // LOD zoom thresholds for THIS renderer instance (seeded from `profile`, falling
  // back to the rich-tier module constants). Replace the constants at every live
  // LOD decision so a lean/minimal box reveals labels/detail/agents later.
  private lodLabelsIn: number;
  private lodLabelsOut: number;
  private lodDetails: number;
  private lodAgents: number;

  // CHUNKED BUILD state. The heavy work in setCityState is constructing one
  // procedural kit per building; for a large city that single synchronous loop
  // froze the UI thread for minutes. We now build buildings in batches across
  // requestAnimationFrame so the browser can paint the "Generating the Polis…"
  // overlay and stay responsive. `buildRaf` is the pending frame handle (null
  // when idle); `buildToken` is bumped on every new build / destroy so an
  // in-flight batch loop self-cancels (latest build wins, no stale apply).
  private buildRaf: number | null = null;
  private buildToken = 0;
  // FIX 6: the last progress callback handed to setCityState/setCity, remembered
  // so the applyCityDiff FALLBACK rebuild (which calls setCityState internally on
  // a build-in-flight / first-frame / reentrancy case) can keep reporting
  // progress — otherwise a multi-second fallback rebuild shows no "Generating…"
  // overlay. Cleared on destroy.
  private lastOnProgress: ((p: BuildProgress) => void) | undefined = undefined;

  // B2b — ON-DEMAND (viewport-prioritized) BUILD state. The chunked build no longer
  // places buildings in pure depth order: the chunks the viewport (+ a profile
  // preload ring) can see build FIRST (depth-sorted within), the rest fill in by
  // distance. `buildState` holds the in-flight ordering so a camera move DURING the
  // background fill can re-sort the REMAINING (unplaced) tail without restarting the
  // build or touching placed chunks. Null when no build is in flight.
  //   - `sorted`: the depth-sorted buildings (source array; `order` indexes into it).
  //   - `chunkXY`: per-source-index chunk grid coords (parallel to `sorted`).
  //   - `order`: build order = indices into `sorted`. The tail `[cursor, total)` is
  //              the not-yet-placed remainder a reprioritization re-sorts.
  //   - `cursor`: how many of `order` have been placed (the head is immutable).
  //   - `preloadRing`: the profile's ring, captured for reprioritization.
  private buildState: {
    sorted: Building[];
    chunkXY: { cx: number; cy: number }[];
    order: number[];
    cursor: number;
    preloadRing: number;
    // FIX 2 — the size of the CURRENT priority (visible) set: the count of items
    // at the HEAD of `order` that are in the priority chunks. Stored here (not a
    // build-local) because reprioritizeRemaining() re-sorts the tail toward a NEW
    // viewport and so the head's visible count CHANGES; progress callbacks read
    // this so visibleDone/visibleTotal always describe the CURRENT visible set.
    visibleTotal: number;
  } | null = null;
  // B2b — debounced camera-move reprioritization handle (a setTimeout id). A pan/
  // zoom burst during the background fill coalesces to ONE re-sort of the remaining
  // queue. Null when none is pending. Cleared on cancel/destroy.
  private reprioritizeTimer: ReturnType<typeof setTimeout> | null = null;

  // MUTATION STATE MACHINE (BLOCKER C). The scene's building/road/agent
  // structures may be mutated by exactly ONE of two paths at a time:
  //   - "building": a chunked `setCityState` build is in flight across rAF
  //                 batches (createBuildingNode called incrementally).
  //   - "diffing":  an in-place `applyCityDiff` is mutating nodes synchronously.
  //   - "idle":     no mutation in progress; the scene is fully built + settled.
  // A live diff arriving DURING a build must NOT interleave with the batch loop
  // (it would re-add/relocate buildings the loop hasn't placed yet, or that the
  // diff already handled). So `applyCityDiff` falls back to a full `setCityState`
  // whenever a build is in flight, and the two paths are otherwise mutually
  // exclusive. `setCityState` always cancels first (latest wins), so it may
  // legitimately preempt either state.
  private mutationState: "idle" | "building" | "diffing" = "idle";

  // Road LOD: the minor-lane sub-container, toggled (visibility + alpha) by zoom
  // in the cull/LOD pass. Built ONCE in drawRoads; never rebuilt per frame. The
  // trunk cobble lives in its own sub-container that is always visible.
  private roadMinorLayer: Container | null = null;

  // Dirty-flag for the cull/LOD recompute: the ticker early-returns unless the
  // camera actually moved/zoomed (or the scene/size changed). Set by the
  // viewport `moved`/`zoomed` listeners, by setCityState, and on resize.
  private cullDirty = true;
  // Reused visible-bounds Rectangle — avoids the per-frame allocation that
  // viewport.getVisibleBounds() would incur (it `new Rectangle()`s each call).
  private viewBounds = new Rectangle();
  private onViewportChanged: (() => void) | null = null;
  private labelStyle: TextStyle;
  private selectedId: string | null = null;
  private selectionRing: Graphics;

  // Screen-space vignette (added to app.stage so it does NOT pan with world).
  private vignette: Graphics;
  // Screen-space DAY-CYCLE tint (added to app.stage, ABOVE the vignette so the
  // warmth reads over the whole view). A single WHITE rect whose `tint` + `alpha`
  // are lerped on a SLOW multi-minute loop (warm noon → warm evening → back).
  // Recolored allocation-free per step; geometry rebuilt only on resize. Visual
  // only — NOT persisted, determinism not required.
  private dayCycle: Graphics;
  // Unclamped elapsed wall-time (ms) for the day cycle ONLY. Distinct from the
  // step clock's `animT` (which is clamped per step to keep flames from jumping):
  // the day phase wants real elapsed time so the loop period stays honest even
  // after a stall. Advanced in update(); reset on a fresh city build.
  private dayElapsedMs = 0;
  // Whether the day-cycle rect geometry has been built. drawDayCycle() early
  // returns on a zero-size host (hidden/unmeasured at mount), so without this the
  // overlay would stay invisible until an explicit resize. The step tick draws it
  // once as soon as the host gains size (FIX 5). Alloc-free in steady state.
  private dayCycleDrawn = false;

  /**
   * P5.1 — exposed day-phase value (folded triangle wave, 0 noon → 1 dusk → 0).
   * Drives halos night-boost (both evening and morning twilight — symmetric reuse
   * of the existing 4-minute day-cycle warmth loop) and shadow skew angle.
   * Updated every step in applyDayCycle(), allocation-free.
   */
  public dayPhase = 0;
  // P5.1 — previous budget rung (for transition detection on ladder shift).
  private _prevBudgetRung: BudgetRung = 0;

  // P5.1 — fire atlas (baked once per session), hero fire pool, crowd fires map.
  private fireAtlas: FireAtlas | null = null;
  private heroFirePool: HeroFire[] = [];
  private crowdFires = new Map<string, CrowdFire>();
  private effectsBudget!: EffectsBudget;
  // P5.1 — debug overlay (dev-flag). One Text node, updated at 2Hz.
  private debugOverlay: import('pixi.js').Text | null = null;
  private debugOverlayTimer = 0;
  // P5.1 — hero-promotion-dirty flag (re-eval on moved/zoomed + sin changes).
  private heroPromoDirty = true;
  // P5.1 — shared halo radial texture (256px, rendered once).
  private haloTex: import('pixi.js').Texture | null = null;
  // P5.1 — pooled halo sprites keyed by fileId (mirrors crowdFires lifecycle).
  private haloSprites = new Map<string, import('pixi.js').Sprite>();
  private onResize: (() => void) | null = null;
  private onBackgroundTap: (() => void) | null = null;

  constructor(
    app: Application,
    viewport: Viewport,
    callbacks: PolisRendererCallbacks = {},
    profile?: RenderProfile,
    hardware?: HardwareInfo | null,
  ) {
    this.app = app;
    this.viewport = viewport;
    this.callbacks = callbacks;

    // B2c — adopt the chosen render profile (or the safe MIDDLE default when none
    // is supplied: `profileFor(null)` returns the lean tier). Seed the per-instance
    // LOD bands from it so the LOD pass + build prioritization scale to the host.
    this.profile = profile ?? profileFor(null);
    this.lodLabelsIn = this.profile.lodLabelsIn;
    this.lodLabelsOut = this.profile.lodLabelsOut;
    this.lodDetails = this.profile.lodDetails;
    this.lodAgents = this.profile.lodAgents;
    // Building atlas: device pixel ratio capped by the profile (min(dpr, cap)). A
    // lean/minimal tier caps at 1 — no HiDPI super-sampling, bounded texture heap.
    const dpr =
      typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1;
    this.buildingAtlas = new BuildingTextureAtlas(
      Math.min(dpr, this.profile.atlasResolutionCap),
    );
    // Record the chosen profile ONCE so live verification can read the tier from
    // the Phase-0 debug log (`PROFILE gpu=<name> kind=<kind> tier=<…>`).
    this.debugLog(
      `PROFILE gpu=${hardware?.gpuName ?? "unknown"} kind=${hardware?.gpuKind ?? "unknown"}` +
        ` tier=${this.profile.tier} labelsIn=${this.lodLabelsIn}` +
        ` details=${this.lodDetails} agents=${this.lodAgents}` +
        ` preloadRing=${this.profile.preloadRing} atlasCap=${this.profile.atlasResolutionCap}` +
        ` aa=${this.profile.antialias} maxWalkers=${this.profile.maxAmbientWalkers}`,
    );

    // Attach layers to the viewport in z-order.
    for (const layer of Object.values(this.layers)) {
      this.viewport.addChild(layer);
    }
    this.layers.shadows.eventMode = "none"; // shadows never intercept clicks

    // Background tap (on the world/viewport, not on a building or omino) clears
    // the selection. Building / agent handlers `stopPropagation()` so a real
    // selection never reaches here. The viewport is interactive for pan already.
    this.viewport.eventMode = "static";
    this.onBackgroundTap = () => {
      this.callbacks.onSelectBuilding?.(null);
      this.callbacks.onSelectAgent?.(null);
    };
    this.viewport.on("pointertap", this.onBackgroundTap);

    // F5 — shared slot allocator so agent + ambient walkers never collide at the same building door.
    const sharedSlotAllocator = new SlotAllocator();

    this.agentLayer = new AgentLayer(this.layers.agents, (agent) => {
      this.callbacks.onSelectAgent?.(agent);
    }, sharedSlotAllocator);

    // Decorative ambient crowd in its own sub-container on the dedicated
    // `crowd` layer (below buildings — see the layer declaration for the
    // occlusion rationale). PURE-DATA NOTE: scenery only — never part of
    // city.agents.
    const ambientContainer = new Container();
    this.layers.crowd.addChild(ambientContainer);
    this.ambientLayer = new AmbientLayer(ambientContainer, sharedSlotAllocator);

    // Trade-route porters: built directly on their dedicated `tradeRoutes`
    // layer (above road cobble, below buildings — see the layer declaration for
    // the click-discipline rationale). A porter click surfaces the real import
    // connection through the renderer callback.
    this.tradeRouteLayer = new TradeRouteLayer(
      this.layers.tradeRoutes,
      (from, to) => {
        this.callbacks.onSelectConnection?.(from, to);
      },
    );

    // External cloud-service outposts on their dedicated `external` layer. A click
    // surfaces the real service (provider/type/name/status) in the inspect sidebar.
    this.externalLayer = new ExternalServiceLayer(
      this.layers.external,
      (service) => {
        this.callbacks.onSelectExternalService?.(service);
      },
    );

    // L2 growth effects: pooled one-shot bursts (dust/rubble/seal) draw on the
    // effects layer; node transitions (tier-grow/pop-in) mutate node transforms.
    // The effects layer never intercepts picking.
    this.layers.effects.eventMode = "none";
    this.growthFx = new GrowthFx(this.layers.effects);

    // P5.1 — halos layer: z-grouped additive sprites (two blend switches per frame).
    this.layers.halos.eventMode = "none";
    // P5.1 — effects budget (pure, injectable clock).
    this.effectsBudget = new EffectsBudget(this.profile, () => performance.now() / 1000);
    // P5.1 — bake fire atlas (Flame/Smoke → RenderTexture flip-book frames).
    // The PIXI renderer is required; it is ready by the time the constructor runs.
    try {
      this.fireAtlas = bakeFireAtlas(this.app.renderer as unknown as import('./fire').FireTextureSource);
    } catch {
      this.debugLog("P5.1 fire atlas bake failed — crowd fires disabled");
    }
    // P5.1 — shared halo texture (256px radial gradient, additive blend).
    this.haloTex = this.makeHaloTexture();
    // P5.1 — pre-allocate hero fire pool (maxHeroFires ParticleContainers).
    if (this.profile.maxHeroFires > 0 && this.fireAtlas) {
      for (let i = 0; i < this.profile.maxHeroFires; i++) {
        // Parked hero fires — will be re-targeted on promotion.
        const hf = createHeroFire(
          this.app.renderer as unknown as import('./fire').FireTextureSource,
          `__pool_${i}`, 0, 0,
        );
        parkHeroFire(hf);
        this.layers.effects.addChild(hf.container);
        this.heroFirePool.push(hf);
      }
    }

    this.labelStyle = new TextStyle({
      fontFamily: "Inter, system-ui, sans-serif",
      fontSize: 12,
      fontWeight: "600",
      fill: PALETTE.shadow,
      stroke: { color: PALETTE.cream, width: 3 },
      align: "center",
    });

    this.selectionRing = new Graphics();
    this.selectionRing.visible = false;
    this.layers.ui.addChild(this.selectionRing);

    // Screen-space vignette overlay. Lives on app.stage (NOT the viewport) so it
    // stays pinned to the screen edges while the world pans/zooms beneath it.
    this.vignette = new Graphics();
    this.vignette.eventMode = "none";
    this.app.stage.addChild(this.vignette);
    this.drawVignette();
    // P5.1 — debug overlay (dev-flag toggle via localStorage).
    this.initDebugOverlay();

    // Day-cycle tint overlay — a screen-space WHITE rect on app.stage, ABOVE the
    // vignette so the warmth reads over the whole view (it does NOT pan/zoom with
    // the world). Non-interactive so it never intercepts picking/selection. Its
    // tint/alpha are lerped per step in update(); geometry is (re)built here and
    // on resize, exactly like the vignette.
    this.dayCycle = new Graphics();
    this.dayCycle.eventMode = "none";
    this.app.stage.addChild(this.dayCycle);
    this.drawDayCycle();
    this.applyDayCycle(); // set the initial (noon) tint before the first frame

    this.onResize = () => {
      this.drawVignette();
      this.drawDayCycle();
      // A resize changes worldScreenWidth/Height (the visible world rect), so
      // the cull/LOD must recompute on the next tick.
      this.cullDirty = true;
    };
    this.app.renderer.on("resize", this.onResize);

    // Dirty-flag the cull: only recompute when the camera actually moves/zooms.
    // pixi-viewport emits `moved` (pan/decelerate/clamp) and `zoomed` (wheel/
    // pinch/animate). Subscribing to both covers every camera change.
    this.onViewportChanged = () => {
      this.cullDirty = true;
      // P5.1 — hero promotion re-eval on camera move/zoom.
      this.heroPromoDirty = true;
      // B2b — if a chunked build is still filling in the background, a camera move
      // RE-PRIORITIZES the not-yet-placed remainder so the chunks the user just
      // panned/zoomed to build next. Debounced (a pan/zoom burst coalesces to one
      // re-sort); never restarts the build or re-places built chunks. Cheap no-op
      // when no build is in flight (`buildState` null).
      if (this.buildState) this.scheduleReprioritize();
    };
    this.viewport.on("moved", this.onViewportChanged);
    this.viewport.on("zoomed", this.onViewportChanged);

    // Culling + LOD ticker. Gated on cullDirty: runs the O(chunks+buildings)
    // sweep only when the camera/scene/size changed, not every frame.
    this.cullTick = () => this.updateCulling();
    this.app.ticker.add(this.cullTick);
  }

  /**
   * Replace the rendered city. NON-BLOCKING: the synchronous prelude (clear,
   * terrain, props, districts, roads, nav graph) runs at once — these are a
   * bounded number of batched Graphics — then the EXPENSIVE part, constructing
   * one procedural kit per building, is spread across requestAnimationFrame in
   * fixed-size batches so the UI thread never blocks (the old single loop froze
   * the app for minutes on a large city). Agents, the ambient crowd, and the
   * camera recenter run once, on the final batch, since agents resolve building
   * iso positions. `onProgress` (if given) is called after every batch with the
   * running building count and once more with phase "done".
   *
   * Idempotent + cancellable: a new setCityState / applyCityDiff / destroy bumps
   * `buildToken` so any in-flight batch loop bows out, and clearScene() tears
   * down whatever partial scene the cancelled build had created.
   */
  /** DIAGNOSTIC (temporary): fire-and-forget one line to
   *  `%TEMP%/aspis-polis-debug.log` via the `polis_debug_log` Tauri command,
   *  appending the current JS-heap reading when the engine exposes it. Lets us
   *  trace the chunked build from a file when DevTools are unavailable (release)
   *  or the webview OOM-crashes before the console is readable (dev). */
  private debugLog(msg: string): void {
    let heap = "";
    const mem = (
      performance as unknown as {
        memory?: { usedJSHeapSize: number; totalJSHeapSize: number; jsHeapSizeLimit: number };
      }
    ).memory;
    if (mem) {
      heap =
        ` heap=${Math.round(mem.usedJSHeapSize / 1048576)}/` +
        `${Math.round(mem.totalJSHeapSize / 1048576)}MB (limit ${Math.round(mem.jsHeapSizeLimit / 1048576)}MB)`;
    }
    try {
      // Dynamic import = the proven path (matches invokeBackendCommand internals);
      // the static `import { invoke }` form silently wrote nothing in the release
      // webview, so use the dynamic form here too.
      void import("@tauri-apps/api/core")
        .then(({ invoke }) => invoke("polis_debug_log", { line: `${msg}${heap}` }))
        .catch(() => {});
    } catch {
      /* no tauri context (tests) — ignore */
    }
  }

  setCityState(city: CityState, onProgress?: (p: BuildProgress) => void): void {
    if (this.destroyed) return;
    // FIX 6: remember the latest progress callback so a later applyCityDiff
    // FALLBACK rebuild can reuse it (it calls setCityState with no callback).
    this.lastOnProgress = onProgress;
    // B2b CAMERA-YANK GUARD: capture the first-ever-build witness BEFORE the
    // teardown below. `lastCity` is the only robust witness: it is set ONLY in
    // finalize() (a completed build) and is nulled by clearScene() — so it is
    // null on the very first build AND would *also* read null AFTER the
    // clearScene() on line below, which is exactly why we must read it HERE,
    // before cancelBuild()/clearScene() run. `buildingNodes.size` is NOT a valid
    // witness: clearScene() empties it too, so it is 0 on a fallback rebuild as
    // well as a first build. When this is a FALLBACK rebuild (a live watcher diff
    // that hit the in-flight-build / no-baseline path and re-entered here),
    // `lastCity` is non-null from the prior completed build, so we SKIP the
    // synchronous camera pre-fit and keep the user's current pan/zoom. The
    // priority chunks then derive from the CURRENT (user) viewport — correct.
    const isFirstBuild = this.lastCity === null;
    // Cancel any in-flight chunked build and start a fresh one (latest wins).
    this.cancelBuild();
    this.clearScene();

    const buildingById = new Map<string, Building>();
    for (const b of city.buildings) buildingById.set(b.fileId, b);

    this.debugLog(
      `BUILD START buildings=${city.buildings.length} roads=${city.roads.length} ` +
        `districts=${city.districts.length} agents=${city.agents.length}`,
    );

    // Synchronous prelude — bounded cost (a handful of batched Graphics each).
    this.redrawTerrainProps(city.buildings, city.gridSize, city.terrain, city.districts, city.roads);
    this.debugLog("prelude: terrain done");
    this.drawDistricts(city.districts);
    this.debugLog("prelude: districts done");
    this.drawRoads(city.roads, buildingById);
    this.debugLog("prelude: roads done");

    // Build the agent navigation graph from the same roads (once per city).
    // Record the road signature so the FIRST live diff after this build can tell
    // whether roads actually changed (FIX 3) instead of always rebuilding.
    this.roadGraph = new RoadGraph(city.roads, makeWaterBlocker(city.terrain));
    // T2 — build the combined walk blocker (water + building footprints) once.
    this.blocked = combineBlockers(
      makeWaterBlocker(city.terrain),
      makeBuildingBlocker(city.buildings),
    );
    // T2 — propagate the blocker to the walker layers.
    this.agentLayer.setBlocked(this.blocked);
    this.ambientLayer.setBlocked(this.blocked);
    this.debugLog("prelude: roadGraph built");
    this.lastRoadSig = PolisRenderer.roadSignature(city.roads);
    // Record the terrain signature (just drawn above) so the first live diff can
    // tell whether the terrain frame actually moved (footprint-growing tier change
    // → new sea_x/band) without re-walking the tiles.
    this.lastTerrainSig = PolisRenderer.terrainSignature(city.terrain);

    // z-sort buildings by depth ONCE so nearer buildings draw on top of farther
    // ones; the batched loop then adds them in this stable (depth) order — within
    // each priority bucket the B2b ordering below preserves exactly this order.
    const sorted = [...city.buildings].sort(
      (a, b) =>
        depthKey(a.coords.x, a.coords.y) - depthKey(b.coords.x, b.coords.y),
    );
    const total = sorted.length;

    // Capture this build's token; if it changes mid-flight (a newer build or
    // destroy) the loop self-cancels without touching the newer scene.
    const token = ++this.buildToken;

    // B2b — VIEWPORT-PRIORITIZED BUILD ORDER. On the FIRST-EVER build, pre-fit the
    // camera to the city's extent FIRST so the priority region reflects the FINAL
    // framing (the camera is still at its mount default — without this pre-fit the
    // "visible" chunks would be whatever the default camera happened to show, not
    // the city the user is about to see). On a FALLBACK rebuild (a live diff that
    // re-entered setCityState) we must NOT pre-fit: that would YANK the user's
    // current pan/zoom mid-session. We keep the user's camera and let the priority
    // chunks derive from the CURRENT viewport — exactly the right region to build
    // first. Then order the depth-sorted buildings so the viewport + preload-ring
    // chunks build first (depth-stable within), the rest by distance. Time-to-
    // visible now scales with the VIEWPORT, not the whole city.
    if (isFirstBuild) this.fitCameraToBuildings(sorted);
    const ring = this.profile.preloadRing;
    // Per-source-index chunk grid coords (parallel to `sorted`) + the candidate
    // chunk-key set the buildings occupy (input to the pure priority computation;
    // `this.chunks` is empty pre-build, so we derive candidates from the buildings).
    const chunkXY = sorted.map((b) => ({
      cx: Math.floor(b.coords.x / CHUNK_SIZE),
      cy: Math.floor(b.coords.y / CHUNK_SIZE),
    }));
    const candidateKeys = new Set(chunkXY.map((c) => `${c.cx},${c.cy}`));
    const { keys: priorityKeys, center } = this.viewportPriorityChunks(
      candidateKeys,
      ring,
    );
    const isPriority = priorityFromKeys(priorityKeys);
    const order = orderBuildQueue(chunkXY as BuildQueueItem[], isPriority, center);
    // The viewport-priority subset count: every priority item is the HEAD of
    // `order`, so `visibleDone = min(cursor, visibleTotal)` tracks the visible city.
    let visibleTotal = 0;
    for (const c of chunkXY) if (isPriority(c.cx, c.cy)) visibleTotal++;

    // Stash the in-flight ordering so a camera move mid-fill can reprioritize the
    // not-yet-placed tail (see scheduleReprioritize). cursor starts at 0. FIX 2 —
    // `visibleTotal` lives in buildState so a reprioritization can recompute it.
    this.buildState = {
      sorted,
      chunkXY,
      order,
      cursor: 0,
      preloadRing: ring,
      visibleTotal,
    };

    const batches = sliceBatches(total, DEFAULT_BUILD_BATCH);
    this.debugLog(
      `BUILD ORDER total=${total} visible=${visibleTotal} ring=${ring} ` +
        `priorityChunks=${priorityKeys.size}`,
    );

    // Finalize once all building batches are placed: agents reference buildings
    // by fileId (resolved to iso here), the ambient crowd follows the road
    // graph, then reset the anim clock and fit the camera on the full scene.
    const finalize = () => {
      // FIX 3: never leave the mutation state stuck on a thrown PIXI op. The
      // `finally` ALWAYS restores idle + clears the build handle, so an exception
      // anywhere below can't wedge the renderer (every future diff would otherwise
      // misbehave forever). lastCity is set inside the try so a half-finalized
      // build doesn't get claimed as the diff baseline.
      try {
        this.debugLog(`FINALIZE start placed=${this.buildingNodes.size}/${total}`);
        // Populate the ambient crowd FIRST so the possession reconcile below can
        // actually claim an idle roaming omino from it (claim-from-crowd needs a
        // crowd to exist; without this the first build would always spawn-fresh).
        this.syncAmbient();
        this.debugLog("FINALIZE syncAmbient done");
        this.reconcileAgents(city.agents);
        this.debugLog("FINALIZE reconcileAgents done");
        // Trade-route porters: built once per city from the same roads (data-
        // bound, top-weight edges only). Rebuilt on a live diff only when roads
        // change (gated below in the diff). The terrain frame drives the defensive
        // walkability guard (no porter ever walks onto water).
        this.syncTradeRoutes(city.roads, city.terrain);
        this.debugLog("FINALIZE syncTradeRoutes done");
        // External cloud outposts: built from the REAL synced inventory list. The
        // backend already placed them at the seaward margin (outside the grid).
        this.externalLayer.setServices(city.externalServices ?? []);
        this.externalLayer.setLodVisible(this.viewport.scale.x >= LOD_EXTERNAL);
        this.clock.reset();
        this.recenter();
        // Force a cull/LOD pass on the next tick for the freshly built scene.
        this.cullDirty = true;
        // P3.2 — apply any active filter to the freshly built scene
        if (this.filterSets) this.applyFilter();
        this.lastCity = city;
        // FIX 2 — report the CURRENT visible total (a mid-build reprioritization
        // may have changed it); fall back to the initial value if buildState was
        // already cleared. At "done" the whole build is placed, so visibleDone
        // equals the full visible set.
        const vTotal = this.buildState?.visibleTotal ?? visibleTotal;
        onProgress?.({
          done: total,
          total,
          phase: "done",
          visibleDone: vTotal,
          visibleTotal: vTotal,
        });
        this.debugLog(`BUILD DONE placed=${this.buildingNodes.size}/${total}`);
      } finally {
        this.buildRaf = null;
        // B2b — the build is over; drop the in-flight ordering + any pending
        // reprioritization so a stray timer can't re-sort a settled scene.
        this.buildState = null;
        if (this.reprioritizeTimer !== null) {
          clearTimeout(this.reprioritizeTimer);
          this.reprioritizeTimer = null;
        }
        // Build complete: the scene is fully placed and settled — back to idle so
        // a subsequent live diff can mutate in place (not fall back to a rebuild).
        this.mutationState = "idle";
      }
    };

    // A chunked build is now in flight (even the empty-city fast path runs the
    // synchronous finalize below, which flips us back to "idle").
    this.mutationState = "building";

    // Empty city (no buildings): nothing to batch — finalize immediately so the
    // empty/loading states resolve and agents/camera still settle.
    if (batches.length === 0) {
      finalize();
      return;
    }

    let i = 0;
    const runBatch = () => {
      // Bow out if a newer build started or the renderer was destroyed; the new
      // build's clearScene() owns teardown of this build's partial scene.
      if (this.destroyed || token !== this.buildToken) return;
      const state = this.buildState;
      if (!state) return; // defensive: cancelled out from under us
      const { start, end } = batches[i];
      // B2b — place buildings in the PRIORITIZED `order` (indices into `sorted`),
      // not in raw depth order. A mid-build reprioritization re-sorts the tail
      // `[cursor, total)` of `order` in place, so we always read the LATEST order;
      // `cursor` is advanced past every placed index so a re-sort never re-places.
      for (let k = start; k < end; k++) {
        const srcIdx = state.order[k];
        const b = state.sorted[srcIdx];
        // ROBUSTNESS: a single malformed building (e.g. an unusual file ingested
        // from a non-source dir) must NEVER kill the whole chunked build. Without
        // this guard a throw here escaped the rAF callback, the loop died, finalize()
        // never ran, and the ENTIRE map rendered grey (0 buildings placed) while the
        // React/DOM chrome kept working. Skip + log the offending node and carry on.
        try {
          this.createBuildingNode(b);
        } catch (err) {
          console.error(
            `[polis] createBuildingNode failed for fileId=${b?.fileId ?? "?"} ` +
              `purpose=${b?.purpose ?? "?"} — skipped:`,
            err,
          );
          this.debugLog(
            `createBuildingNode FAILED fileId=${b?.fileId ?? "?"} ` +
              `purpose=${b?.purpose ?? "?"} err=${String((err as Error)?.message ?? err)}`,
          );
        }
        // Advance the cursor as each index is placed so the immutable HEAD of
        // `order` (already built) is never re-sorted by a reprioritization. FIX 5 —
        // the cursor advances even when createBuildingNode threw and was caught
        // above, so `cursor` (and thus visibleDone) counts ATTEMPTED, not strictly
        // PLACED, buildings. This is accepted: progress = "attempted", and PolisView
        // ignores visibleDone/visibleTotal (they drive only the debugLog + an
        // optional overlay hint). Counting attempts keeps the cursor monotonic and
        // guarantees a skipped node can never wedge progress short of total.
        state.cursor = k + 1;
      }
      i += 1;
      // FIX 2 — read the CURRENT visible total from buildState (a mid-build
      // reprioritization re-sorts the tail toward a NEW viewport and recomputes
      // state.visibleTotal, so the build-local seed is stale after that). Priority
      // items are the HEAD of `order`, so the count placed so far is
      // min(cursor, visibleTotal): "the visible city" completes once cursor reaches
      // visibleTotal, BEFORE the whole build (cursor reaches total).
      const stateVisibleTotal = state.visibleTotal;
      const visibleDone = Math.min(state.cursor, stateVisibleTotal);
      this.debugLog(
        `batch ${i}/${batches.length} placed=${this.buildingNodes.size} ` +
          `visible=${visibleDone}/${stateVisibleTotal}`,
      );
      if (i < batches.length) {
        onProgress?.({
          done: batches[i - 1].end,
          total,
          phase: "building",
          visibleDone,
          visibleTotal: stateVisibleTotal,
        });
        this.buildRaf = requestAnimationFrame(runBatch);
      } else {
        finalize();
      }
    };
    // Kick off on the next frame so the loading overlay paints before the first
    // batch runs (the synchronous prelude above already used this frame).
    this.buildRaf = requestAnimationFrame(runBatch);
  }

  /** Cancel any in-flight chunked build. Bumps the build token so a queued batch
   *  callback self-cancels, and drops the pending rAF. Idempotent. The partial
   *  scene is left for the caller's clearScene()/destroy to tear down. */
  private cancelBuild(): void {
    this.buildToken += 1;
    if (this.buildRaf !== null) {
      cancelAnimationFrame(this.buildRaf);
      this.buildRaf = null;
    }
    // B2b — drop the in-flight build ordering + any pending reprioritization so a
    // queued re-sort can't touch the next build's (or a torn-down) state.
    this.buildState = null;
    if (this.reprioritizeTimer !== null) {
      clearTimeout(this.reprioritizeTimer);
      this.reprioritizeTimer = null;
    }
    // No build is in flight anymore. The caller either kicks off a fresh build
    // (which sets "building" again) or tears the scene down (destroy) — either
    // way the in-flight build no longer owns the mutation state.
    this.mutationState = "idle";
  }

  /**
   * LIVE UPDATE — apply a freshly-scanned `CityState` to the EXISTING scene IN
   * PLACE, without a full rebuild and without moving the camera or losing the
   * selection. This is the core of "edit a file → its building resizes" with no
   * city teardown.
   *
   * Algorithm (only changed buildings cost anything):
   *   - NEW (fileId absent)        → build + add via createBuildingNode.
   *   - CHANGED (tier/purpose/coords/status/agent/sins differ) → destroy the old
   *     node (body/shadow/anim/label, chunk + maps) and rebuild it at the new
   *     tier. coords come from the backend meta-store so a same file keeps its
   *     position; only the box size/silhouette changes.
   *   - UNCHANGED                  → left untouched (the efficiency).
   *   - REMOVED (fileId gone)      → destroy + remove.
   * Then: rebuild roads (cheap, batched), re-register agents (the animated/smoke
   * pool is kept correct incrementally by destroy/createBuildingNode — no batch
   * rebuild), redraw terrain/props ONLY if buildings were added/removed, and
   * preserve camera + selection.
   */
  applyCityDiff(next: CityState): void {
    if (this.destroyed) return;
    // FIX 4: a no-op when the input is the SAME object already rendered. The
    // PolisView full-build effect no longer pre-claims liveCity, so the diff
    // effect may re-apply the exact object that was just full-built; diffing an
    // identical object against itself is wasted work, so short-circuit. (Distinct
    // objects that deep-equal still take the diff path; per-node `buildingChanged`
    // makes each unchanged building a no-op, so the result is still correct.)
    if (next === this.lastCity) return;
    // BLOCKER C — never interleave a live diff with an in-flight chunked build,
    // and (FIX 3 reentrancy) never run a nested in-place diff while another diff
    // is mid-flight. If a `setCityState` build is still placing buildings across
    // rAF batches, the scene is PARTIAL (and `lastCity` isn't set until finalize);
    // if a diff is already running, re-entering would mutate half-applied state.
    // Either way fall back to a full rebuild: `setCityState` cancels the in-flight
    // build (latest wins) and rebuilds from `next` cleanly. This also covers the
    // "no prior frame yet" case (lastCity null during the very first build).
    // FIX 6: reuse the remembered progress callback so the fallback rebuild still
    // shows the "Generating…" overlay during a multi-second rebuild.
    if (
      this.mutationState === "building" ||
      this.mutationState === "diffing" ||
      !this.lastCity
    ) {
      this.setCityState(next, this.lastOnProgress);
      return;
    }

    // Take the diffing path: synchronous, mutually exclusive with a build (we
    // just proved none is in flight). FIX 3: the body is wrapped in try/finally
    // so an exception in any PIXI op below can never leave mutationState stuck on
    // "diffing" — the finally ALWAYS restores idle.
    this.mutationState = "diffing";
    try {
      this.applyCityDiffInner(next);
    } finally {
      // Diff complete (or threw): back to idle so the next diff/build can run.
      this.mutationState = "idle";
    }
  }

  // -------------------------------------------------------------------------
  // P3.2 — Filter pass
  // -------------------------------------------------------------------------

  /** Set the filter state and apply in one pass. Null/empty = clear filter.
   *  Idempotent — calling twice with the same sets is a no-op. */
  setFilter(sets: FilterSets | null): void {
    this.filterSets = sets;
    this.applyFilter();
  }

  /** T1b — set which external providers are visible. Delegates to the
   *  ExternalServiceLayer which composes it with LOD visibility. */
  setVisibleProviders(providers: ReadonlySet<string>): void {
    this.externalLayer.setVisibleProviders(providers);
  }

  /** Apply the current filter to all built nodes. ONE PASS over building nodes
   *  + road segments + agents — no coords, no rebuild, plain property writes.
   *  Idempotent — safe to call anytime, no matter the scene state. */
  private applyFilter(): void {
    const sets = this.filterSets;
    const isHide = sets?.mode === "hide";
    const ghosted = sets?.ghostedFileIds ?? new Set<string>();
    const effectsHidden = sets?.effectsHiddenFileIds ?? new Set<string>();

    // --- Buildings: one pass over all nodes ---
    for (const [fileId, node] of this.buildingNodes) {
      const g = ghosted.has(fileId);
      const eh = effectsHidden.has(fileId);

      if (g) {
        // Ghost or hide the whole building
        if (isHide) {
          node.container.visible = false;
        } else {
          node.container.visible = true;
          node.container.alpha = 0.15;
        }
        node.container.eventMode = "none";
        node.container.cursor = "none";
        // Hide label
        if (node.label) node.label.visible = false;
        // Hide disaster/investigation effects
        if (node.disaster) node.disaster.node.visible = false;
        if (node.investigation) node.investigation.node.visible = false;
      } else {
        // Restore full visibility (respecting LOD which will re-apply next cull)
        node.container.visible = true;
        node.container.alpha = 1;
        node.container.eventMode = "static";
        node.container.cursor = "pointer";
        // Labels: let LOD decide (set visible true, LOD will hide if zoomed out)
        if (node.label) node.label.visible = true;
        // Disaster/investigation: let LOD decide
        if (node.disaster) node.disaster.node.visible = true;
        if (node.investigation) node.investigation.node.visible = true;

        // Effects-hidden: only toggle sin-effect display objects
        if (eh && node.disaster) {
          node.disaster.node.visible = false;
        }
        // If not effects-hidden but LOD-hidden, LOD will re-hide on next cull
      }
    }

    // --- Roads: redraw with ghost-alpha multiplier ---
    // Roads are O(N) strokes, fully rebuilt on every diff. Redrawing them
    // here with filter-aware alpha is cheap (no building geometry involved).
    if (this.lastCity) {
      const byId = new Map<string, typeof this.lastCity.buildings[number]>();
      for (const b of this.lastCity.buildings) byId.set(b.fileId, b);
      this.redrawRoads(this.lastCity.roads, byId);
    }

    // --- Agents: set ghost filter on AgentLayer ---
    this.agentLayer.setGhostFilter(sets ? ghosted : new Set<string>());

    // Force a cull pass so LOD restores correct alpha/labels on unfiltered nodes
    this.cullDirty = true;

    // F7 — reconcile selection ring against active filter.
    // If the selected building is HIDDEN (mode hide), clear the renderer-side
    // selection AND notify the view so the sidebar closes.
    if (this.selectedId && sets) {
      const selVerdict = nodeFilterVerdict(this.selectedId, sets);
      if (selVerdict.hide) {
        this.selectedId = null;
        this.callbacks.onSelectBuilding?.(null);
      }
    }
    this.drawSelectionRing();
  }

  // -------------------------------------------------------------------------
  // P3.2 — Filter-aware LOD helpers. Single source of truth for building alpha
  // and effect visibility — used by BOTH applyFilterToNode and updateCulling's
  // LOD block so they can never disagree.
  // -------------------------------------------------------------------------

  /** Target alpha for a building node, respecting BOTH LOD and the active
   *  filter. Returns the alpha (0.15 ghosted, LOD value, or 1). */
  private targetBuildingAlpha(fileId: string, lodAlpha: number): number {
    const sets = this.filterSets;
    if (!sets) return lodAlpha;
    if (sets.ghostedFileIds.has(fileId)) {
      return sets.mode === "hide" ? lodAlpha : 0.15;
    }
    return lodAlpha;
  }

  /** Whether the sin-effect overlay should be visible, respecting BOTH LOD
   *  and the active filter. Ghosted buildings never show effects. */
  private effectVisible(_node: BuildingNode, fileId: string, lodVisible: boolean): boolean {
    if (!lodVisible) return false;
    const sets = this.filterSets;
    if (!sets) return true;
    if (sets.ghostedFileIds.has(fileId)) return false;
    if (sets.effectsHiddenFileIds.has(fileId)) return false;
    return true;
  }

  /** Whether the label should be visible, respecting BOTH filter and LOD. */
  private labelVisible(fileId: string, hasLabel: boolean): boolean {
    if (!hasLabel) return false;
    const sets = this.filterSets;
    if (!sets) return true;
    if (sets.ghostedFileIds.has(fileId)) return false;
    return true;
  }

  /** Apply the current filter to a SINGLE building node. Called from
   *  createBuildingNode so buildings born during incremental build are filtered
   *  from birth — satisfying the threaded-predicate requirement (§1.2). */
  private applyFilterToNode(node: BuildingNode, fileId: string): void {
    const sets = this.filterSets;
    if (!sets) return;
    const isHide = sets.mode === "hide";
    const ghosted = sets.ghostedFileIds.has(fileId);
    const effectsHidden = sets.effectsHiddenFileIds.has(fileId);

    if (ghosted) {
      if (isHide) {
        node.container.visible = false;
      } else {
        node.container.visible = true;
        node.container.alpha = 0.15;
      }
      node.container.eventMode = "none";
      node.container.cursor = "none";
      if (node.label) node.label.visible = false;
      if (node.disaster) node.disaster.node.visible = false;
      if (node.investigation) node.investigation.node.visible = false;
    } else if (effectsHidden && node.disaster) {
      node.disaster.node.visible = false;
    }
  }

  /** The actual in-place diff body. Extracted so `applyCityDiff` can wrap it in a
   *  single try/finally that guarantees `mutationState` is restored to idle even
   *  if a PIXI op throws (FIX 3). Assumes the caller set `mutationState =
   *  "diffing"` and verified no build/diff is already in flight and `lastCity` is
   *  set. */
  private applyCityDiffInner(next: CityState): void {
    const nextById = new Map<string, Building>();
    for (const b of next.buildings) nextById.set(b.fileId, b);

    let addedOrRemoved = false;

    // 1) ADDED / CHANGED. Walk the next buildings; compare to the current node.
    //    L2 growth visuals are keyed on deltas computed HERE (old vs new) and
    //    fired AFTER the node mutation, so they never perturb the diff/cull/build
    //    or the F0 idle|building|diffing state machine.
    for (const b of next.buildings) {
      const node = this.buildingNodes.get(b.fileId);
      if (!node) {
        // NEW → build it, then pop it in from the ground + a dust puff.
        const fresh = this.createBuildingNode(b);
        this.growthFx.popIn(fresh.container);
        this.growthFx.dust(fresh.iso.x, fresh.iso.y);
        addedOrRemoved = true;
        continue;
      }
      // CHANGED? Update only if a visual input differs.
      if (buildingChanged(node.building, b)) {
        const old = node.building;
        // Capture the growth deltas from the OLD node BEFORE it mutates.
        const oldTier = tierRank(old.visualTier);
        const newTier = tierRank(b.visualTier);
        const grew = newTier > oldTier; // file gained tiers → grow transition
        // Agent FINISHED here: agentPresent was SET on the old, UNSET on the new.
        const agentLeft = !!old.agentPresent && !b.agentPresent;
        const at = node.iso;
        // SPRITE-SHEET: a coords-unchanged change (tier/status/sins/provider/
        // suspect/agent — the common file-edit case) is a TEXTURE SWAP + overlay
        // update on the SAME Sprite, NOT a destroy+rebuild. A coords change must
        // re-chunk the node, so it still falls back to destroy+rebuild.
        const moved =
          old.coords.x !== b.coords.x || old.coords.y !== b.coords.y;
        let fresh;
        if (moved) {
          this.destroyBuildingNode(node);
          fresh = this.createBuildingNode(b);
          addedOrRemoved = true; // a reposition changes the footprint set exactly like an add+remove
        } else {
          fresh = this.updateBuildingNodeInPlace(node, b);
        }
        if (grew) {
          // Grow the larger silhouette into place + a small dust puff. A tier
          // DECREASE (code shrank) is left as a plain swap (no over-animation).
          this.growthFx.growTransition(fresh.container);
          this.growthFx.dust(fresh.iso.x, fresh.iso.y);
        }
        if (agentLeft) {
          // Golden-seal celebration at the building (reuses the Augur seal burst).
          this.growthFx.seal(at.x, at.y);
        }
      }
      // UNCHANGED → leave it.
    }

    // 2) REMOVED. Any current node whose fileId is gone from `next`.
    //    Collect first (don't mutate the map while iterating it).
    const toRemove: BuildingNode[] = [];
    for (const node of this.buildingNodes.values()) {
      if (!nextById.has(node.building.fileId)) toRemove.push(node);
    }
    for (const node of toRemove) {
      // L2: leave a brief rubble/dust puff where the building stood — fired
      // BEFORE the node is destroyed (the burst lives on the effects layer, so
      // it outlives the gone node and fades on its own).
      this.growthFx.rubble(node.iso.x, node.iso.y);
      this.destroyBuildingNode(node);
      addedOrRemoved = true;
    }

    // 3) Roads — rebuild the whole roads layer (small, batched). Imports may have
    //    changed when a file's deps changed. Keeps the trunk/minor LOD split.
    const buildingById = new Map<string, Building>();
    for (const b of next.buildings) buildingById.set(b.fileId, b);
    this.redrawRoads(next.roads, buildingById);

    // Rebuild the agent navigation graph ONLY when the road set actually changed
    // (FIX 3). Most live diffs are pure sin/status/provider updates that leave
    // roads identical — rebuilding the RoadGraph (O(E) Edge allocations) + the
    // ambient weight table on those would be wasted work. A cheap stable
    // signature detects a real road change; if roads are unchanged the existing
    // graph is still valid (agents keep navigating it) and the ambient crowd
    // keeps strolling. NOTE: the ambient civic-anchor set (forum lingerers) is
    // refreshed only alongside a road change — a building whose purpose changed
    // without any road change will re-anchor the crowd on the NEXT road-affecting
    // diff. Accepted: anchors are decorative scenery, not data.
    const roadSig = PolisRenderer.roadSignature(next.roads);
    const roadsChanged = roadSig !== this.lastRoadSig;
    // The terrain frame moved? (Computed once here and reused by the redraw gate
    // below.) The nav graph's defensive walkability guard is derived from this
    // frame (the blocked-water set), so rebuild the graph when EITHER the road set
    // OR the terrain moved — a tier change that shifts the sea/river (terrain-only)
    // must refresh the guard even though the roads are identical.
    const terrainSig = PolisRenderer.terrainSignature(next.terrain);
    const terrainChanged = terrainSig !== this.lastTerrainSig;
    const graphRebuilt = roadsChanged || terrainChanged;
    if (graphRebuilt) {
      const waterBlocked = makeWaterBlocker(next.terrain);
      this.roadGraph = new RoadGraph(next.roads, waterBlocked);
      // T2 — rebuild the combined walk blocker when the graph is rebuilt.
      this.blocked = combineBlockers(
        waterBlocked,
        makeBuildingBlocker(next.buildings),
      );
      this.agentLayer.setBlocked(this.blocked);
      this.ambientLayer.setBlocked(this.blocked);
      this.lastRoadSig = roadSig;
    } else if (addedOrRemoved) {
      // T2 FIX: buildings changed without road/terrain change — rebuild the
      // building blocker only (the water blocker is still valid from the last
      // graph rebuild). Without this, newly-added buildings wouldn't be blocked
      // and demolished buildings would remain blocked.
      this.blocked = combineBlockers(
        makeWaterBlocker(next.terrain),
        makeBuildingBlocker(next.buildings),
      );
      this.agentLayer.setBlocked(this.blocked);
      this.ambientLayer.setBlocked(this.blocked);
    }

    // 4) Districts — cheap; rebuild so renamed/moved districts stay correct.
    this.redrawDistricts(next.districts);

    // 5) Terrain / props — redraw when the building set CHANGED size (added or
    //    removed; the extent + sea margin move), OR roads changed (a routed road
    //    may now cross a river → a bridge moves), OR the TERRAIN ITSELF changed.
    //    The last case is the BLOCKER fix: a pure TIER CHANGE on an east-edge
    //    building grows its footprint → the backend recomputes `sea_x` + the sea
    //    band with NO add/remove/road signal, so without the terrain-signature
    //    gate the grown building would float over the stale sea boundary. The
    //    signature is O(rivers) (counts + seaX + band + river ranges), so a pure
    //    sin/agent/provider/status diff (terrain identical) still skips the redraw.
    //    (`terrainSig`/`terrainChanged` computed above for the nav-graph guard.)
    if (addedOrRemoved || roadsChanged || terrainChanged) {
      this.redrawTerrainProps(next.buildings, next.gridSize, next.terrain, next.districts, next.roads);
      this.lastTerrainSig = terrainSig;
    }

    // Re-point the decorative crowd only when the graph it walks actually
    // changed (FIX 3) — its weight table + node set are derived from the roads (and
    // now the terrain-driven walkability guard, so re-sync on either). Run BEFORE
    // the agent reconcile so a newly-active agent claims from the up-to-date crowd.
    if (graphRebuilt) this.syncAmbient();

    // 6) Agents — reconcile through the PURE possession layer: a newly-active agent
    //    claims an idle crowd omino (or spawns fresh), an agent whose currentFileId
    //    changed WALKS the road graph to the new building (or fade-teleports), a
    //    vanished agent releases its omino (adopting a claimed one back into the
    //    crowd). Subagent omini are derived + spawned/removed here too. Identity is
    //    tracked by agentId so live diffs don't reset positions.
    this.reconcileAgents(next.agents);
    // Rebuild the trade-route porters ONLY when the road set OR the terrain frame
    // changed — a pure sin/status diff leaves the flow walking untouched. Porters
    // are derived from the road set + weights + the terrain walkability guard.
    if (graphRebuilt) this.syncTradeRoutes(next.roads, next.terrain);

    // 6b) External cloud outposts — reconcile against the fresh inventory list
    //     (status may flip, a resource may appear/vanish). Cheap (few services,
    //     keyed by serviceId; unchanged nodes are a no-op), so run every diff —
    //     a service status change is independent of any road/building change.
    this.externalLayer.setServices(next.externalServices ?? []);

    // 7) Selection — keep the ring on the selected building if it still exists;
    //    otherwise clear it. (A removed/rebuilt selected building gets a fresh
    //    node; drawSelectionRing re-resolves by id, so a rebuild keeps the ring.)
    if (this.selectedId && !this.buildingNodes.has(this.selectedId)) {
      this.selectedId = null;
    }
    this.drawSelectionRing();

    // 8) Camera is DELIBERATELY untouched (no recenter) — preserve pan/zoom.
    //    Force one cull/LOD pass so new/changed chunks get correct visibility.
    this.cullDirty = true;
    this.lastCity = next;
    // P3.2 — re-apply any active filter to the freshly diffed scene
    if (this.filterSets) this.applyFilter();
    // mutationState restored to idle by the caller's finally (FIX 3).
  }

  /**
   * Re-point the DECORATIVE ambient crowd at the current road graph and size it
   * to the city. Called after the roadGraph is (re)built (setCityState /
   * applyCityDiff). Not called on a bare agent refresh, so the crowd keeps
   * strolling. The crowd is scenery — never `city.agents`.
   */
  /**
   * Cheap, stable signature of a road set, capturing every input the
   * `roadGraph` + ambient weight table depend on: per-road id, endpoints,
   * import weight, and the routed polyline's length + first/last waypoint (so a
   * same-length re-route with shifted geometry still registers as a change).
   * Style/type are NOT included — they only affect the VISUAL road draw, which
   * is rebuilt unconditionally and is not what this gate protects. O(E), no
   * per-waypoint scan. Used only to decide whether to rebuild the nav graph.
   */
  private static roadSignature(roads: readonly Road[]): string {
    let sig = `${roads.length}|`;
    for (const r of roads) {
      const p = r.path;
      const n = p ? p.length : 0;
      const a = n > 0 ? `${p![0].x},${p![0].y}` : "";
      const b = n > 0 ? `${p![n - 1].x},${p![n - 1].y}` : "";
      sig += `${r.roadId}:${r.from}>${r.to}:${r.weight}:${n}:${a}:${b};`;
    }
    return sig;
  }

  /**
   * Cheap, stable signature of the terrain frame, capturing every input the
   * water/sand/bridge draw depends on WITHOUT walking all (potentially tens of
   * thousands of) tiles: the sea edge `seaX`, the sea y-band `minY/maxY`, the
   * per-list tile COUNTS, and every river's column range. A footprint-growing
   * tier change moves `seaX`/the band/the counts; a river relocation changes the
   * range list; a bridge appearing/vanishing changes the bridge count. A pure
   * sin/agent/provider/status diff leaves ALL of these identical, so the terrain
   * redraw is correctly skipped. `undefined` terrain (pre-terrain city) → a
   * stable empty sentinel. O(rivers), not O(tiles). Used only to gate the redraw.
   */
  private static terrainSignature(terrain?: TerrainData): string {
    if (!terrain) return "none";
    let sig = `${terrain.seaX}|${terrain.minY}|${terrain.maxY}|w${terrain.water.length}|s${terrain.sand.length}|b${terrain.bridges.length}|r`;
    for (const r of terrain.rivers) sig += `${r.gxMin},${r.gxMax};`;
    return sig;
  }

  /**
   * Polis-P5 — normalize a project-relative path to the canonical key used by the
   * `fileIdByPath` index, MIRRORING the Rust `meta_store::normalize_rel_path` so a
   * Censor `findings-updated` relPath matches a building's `filePath`: backslashes
   * → forward slashes, COLLAPSE repeated `//` (parity with the Rust
   * `censor::ledger::normalize_rel_path`, which emits the `findings-updated`
   * `files`), then trim leading `./` and surrounding `/`. The backend already
   * emits normalized paths and `building.filePath` is normalized, so this is
   * defensive (it costs nothing and survives a stray separator/prefix). The
   * SAME normalizer must be used both when building the `fileIdByPath` index
   * (from `building.filePath`) and when resolving an incoming event relPath, so
   * both sides agree on the canonical key (e.g. `src//foo.ts` → `src/foo.ts`).
   */
  static normalizeRelPath(rel: string): string {
    return rel
      .replace(/\\/g, "/")
      .replace(/\/\/+/g, "/")
      .replace(/^(\.\/)+/, "")
      .replace(/^\/+|\/+$/g, "");
  }

  /**
   * (Re)build the DATA-BOUND trade-route porters for the given roads. Called
   * after a city build (setCityState.finalize) and on a live diff ONLY when the
   * road set actually changed (the renderer's roads-changed signature gate) —
   * never on a pure sin/status diff, so a settled flow keeps walking. Each porter
   * is tied to a REAL `Road` (its `from`/`to` fileIds); the layer resolves
   * endpoints to confirm both buildings are on the map and uses the routed
   * polyline to walk. Pooled merchant figures are torn down + rebuilt inside the
   * layer (clean removeFromParent + destroy).
   */
  private syncTradeRoutes(roads: readonly Road[], terrain?: TerrainData): void {
    this.tradeRouteLayer.setWorld(
      roads,
      (fileId) => this.buildingNodes.get(fileId)?.iso ?? null,
      makeWaterBlocker(terrain),
    );
    // Apply the current zoom gate immediately so porters don't flash at the
    // wrong LOD before the next cull pass (mirrors the ambient/road LOD seed).
    this.tradeRouteLayer.setLodVisible(this.viewport.scale.x >= TRADE_LOD_ZOOM);
  }

  /**
   * Polis-P4 — reconcile the live agents through the PURE PossessionController and
   * apply its decisions against AgentLayer (create/walk/release omini + spawn/
   * remove subagent omini). The controller drives the AmbientLayer release/adopt
   * accounting via the injected env, so claim-from-crowd / spawn-fresh / return-
   * to-crowd all stay balanced (the claimedCount contract). Replaces the old
   * direct `agentLayer.setAgents(...)` call on both the build and the diff path.
   */
  private reconcileAgents(agents: Agent[]): void {
    const env: PossessionEnv = {
      resolve: (fileId) => this.buildingNodes.get(fileId)?.iso ?? null,
      release: (figure, nearIso) => this.ambientLayer.release(figure, nearIso),
      adopt: (figure, pos) => this.ambientLayer.adopt(figure, pos),
      agentPos: (agentId) => this.agentLayer.agentPos(agentId),
    };
    const { decisions } = this.possession.reconcile(agents, env);
    this.agentLayer.applyDecisions(
      decisions,
      (from, to) => this.roadGraph?.findRoute(from, to) ?? null,
    );
  }

  // ---------------------------------------------------------------------------
  // Polis-P5 — Censor firefighter presence. The renderer OWNS the CensorPresence
  // (a separate non-agent input) so it shares the AgentLayer / AmbientLayer + the
  // building resolution. PolisView feeds it the real `censor://findings-updated`
  // events + the gemma status; the renderer applies its decisions and ticks its
  // debounce from the existing loop. Censor is NEVER injected into `city.agents`.
  // ---------------------------------------------------------------------------

  /** Build the CensorEnv bound to the CURRENT scene: the relPath→building
   *  resolution (via `fileIdByPath` + `buildingNodes`), the AmbientLayer
   *  release/adopt of a FIREFIGHTER (the same primitives possession uses, so the
   *  claimedCount stays balanced), and the firefighter's last position. */
  private censorEnv(): CensorEnv {
    // Polis-P6 — return the memoized instance. Its closures read `this.*` lazily,
    // so it never goes stale across scene rebuilds; building it once removes the
    // per-frame closure allocation in `tickCensor`.
    if (this.cachedCensorEnv) return this.cachedCensorEnv;
    this.cachedCensorEnv = {
      resolveRelPath: (relPath): ResolvedBuilding | null => {
        const fileId = this.fileIdByPath.get(
          PolisRenderer.normalizeRelPath(relPath),
        );
        if (!fileId) return null;
        const iso = this.buildingNodes.get(fileId)?.iso;
        if (!iso) return null;
        return { fileId, iso: { x: iso.x, y: iso.y } };
      },
      release: (nearIso) => this.ambientLayer.release(CENSOR_FIGURE, nearIso),
      adopt: (pos) => this.ambientLayer.adopt(CENSOR_FIGURE, pos),
      firefighterPos: () =>
        this.censorOminoId
          ? this.agentLayer.externalPos(this.censorOminoId)
          : null,
    };
    return this.cachedCensorEnv;
  }

  /** Apply a batch of CensorPresence decisions against the AgentLayer external
   *  firefighter omino. The id is the per-project `censor:<projectId>` key. */
  private applyCensorDecisions(decisions: readonly CensorDecision[]): void {
    if (decisions.length === 0) return;
    const id = this.censorOminoId;
    // #10 — reuse a single cached findRoute closure instead of allocating a fresh
    // one per flush (mirrors `cachedCensorEnv`). It reads `this.roadGraph` lazily,
    // so it stays valid across scene rebuilds (the graph swaps under it).
    const findRoute = (this.cachedCensorFindRoute ??= (from: string, to: string) =>
      this.roadGraph?.findRoute(from, to) ?? null);
    for (const d of decisions) {
      switch (d.kind) {
        // #4 — only the CREATE decisions need a bound omino id (they bind a fresh
        // `censor:<projectId>` external omino). If the id is transiently null there
        // is nothing to create against → skip just THIS decision, not the whole
        // batch.
        case "createClaimed":
          if (!id) break;
          this.agentLayer.createExternalClaimed(
            id,
            CENSOR_FIGURE,
            d.startPos,
            d.startNodeId,
            d.targetFileId,
            d.targetIso,
            findRoute,
          );
          break;
        case "createFresh":
          if (!id) break;
          this.agentLayer.createExternalFresh(
            id,
            CENSOR_FIGURE,
            d.targetFileId,
            d.targetIso,
          );
          break;
        // #4 — destroy / extinguishing / walk are IDEMPOTENT and target the existing
        // omino by its id. They are guarded PER DECISION (not by a single up-front
        // `if(!id) return` over the WHOLE batch): a batch that mixes a (skipped)
        // create with a destroy still applies the destroy whenever an omino id is
        // bound. The omino is keyed solely by `censorOminoId`, so when that is null
        // there is genuinely no firefighter to act on and the action is a true no-op
        // (the AgentLayer external methods also no-op on an absent id) — but a destroy
        // for a LIVE firefighter can no longer be swallowed by an unrelated create in
        // the same batch.
        case "walk":
          if (!id) break;
          this.agentLayer.walkExternal(id, d.targetFileId, d.targetIso, findRoute);
          break;
        case "extinguishing":
          if (!id) break;
          this.agentLayer.setExternalExtinguishing(id, d.on);
          break;
        case "destroy":
          if (!id) break;
          this.agentLayer.destroyExternal(id);
          break;
      }
    }
  }

  /**
   * Feed a `censor://findings-updated` event into the Censor presence. PolisView
   * calls this from its Tauri subscription. `nowMs` is the clock (real
   * performance.now in prod; an injected value in tests via the pure core). The
   * firefighter is bound to `payload.projectId`; a DIFFERENT project releases the
   * current firefighter first (single-active, mirroring the backend watch model).
   */
  onCensorFindings(payload: CensorFindingsPayload, nowMs: number): void {
    if (this.destroyed) return;
    // #1 (BLOCKER) — DROP the event if the scene isn't ready. A
    // `censor://findings-updated` can arrive while a chunked build (or a live diff)
    // is in flight — AFTER clearScene() tore the old scene down, BEFORE finalize()
    // re-populates buildings/roads/crowd. Driving the firefighter into a half-built /
    // just-cleared scene would spawn an orphaned omino (no crowd to claim from, no
    // building anchor to walk to) and leave the censor state pointing at a project
    // whose scene doesn't exist yet → a stuck firefighter. Mirror the diff path's
    // readiness gate (mutationState) and also require at least one placed building.
    // CRITICAL: bail BEFORE mutating censorProjectId / censorOminoId / the presence,
    // so a dropped event leaves the censor state exactly as it was — the NEXT event
    // (post-build, scene ready) reconciles cleanly. Dropping is safe: findings are
    // re-emitted by the backend watch, and tickCensor/the next event re-drive it.
    if (this.mutationState !== "idle" || this.buildingNodes.size === 0) return;
    const env = this.censorEnv();
    // Project switch: release the firefighter bound to the OLD project before
    // adopting the new one, so a stale omino never lingers on the wrong project.
    if (
      this.censorProjectId !== null &&
      this.censorProjectId !== payload.projectId
    ) {
      // Release the OLD project's firefighter WITHOUT perturbing the cached gemma
      // status (the dedicated switch path), then drop the stale omino id so the
      // new project's create binds to the fresh `censor:<newId>` key below.
      const releaseDecisions = this.censor.releaseForSwitch(env);
      this.applyCensorDecisions(releaseDecisions);
      this.censorOminoId = null;
    }
    this.censorProjectId = payload.projectId;
    this.censorOminoId = `censor:${payload.projectId}`;
    const decisions = this.censor.onFindings(payload, nowMs, env);
    this.applyCensorDecisions(decisions);
  }

  /** Update the cached gemma availability for the Censor presence. PolisView calls
   *  this from its status source. Offline releases the firefighter. */
  setCensorGemmaStatus(status: GemmaStatus): void {
    if (this.destroyed) return;
    const decisions = this.censor.setGemmaStatus(status, this.censorEnv());
    this.applyCensorDecisions(decisions);
  }

  /** Advance the Censor debounce clock to `nowMs` and flush a settled naming
   *  event (claim/walk + extinguishing). Driven from the per-frame update loop. */
  private tickCensor(nowMs: number): void {
    if (!this.censor.hasPending) return;
    const decisions = this.censor.tick(nowMs, this.censorEnv());
    this.applyCensorDecisions(decisions);
  }

  private syncAmbient(): void {
    const g = this.roadGraph;
    if (!g) {
      this.ambientLayer.setWorld([], () => null, () => null);
      return;
    }
    const nodeIds = g.nodeIds;
    // Per-node busy-ness weight (sum of incident road weights) so the decorative
    // crowd biases toward arterials. Aligned with `nodeIds` by the graph.
    const nodeWeights = g.nodeWeights;
    // Civic "forum" anchors the lingerers mill near: market / townhall buildings,
    // plus any file routed to the shared COMMONS feature. Restricted to nodes
    // that are actually in the road graph (walkable). Pure scenery selection —
    // these ids never become agents and the forum crowd never glows.
    const forumNodeIds: string[] = [];
    for (const node of this.buildingNodes.values()) {
      const b = node.building;
      const civic =
        b.purpose === "market" ||
        b.purpose === "townhall" ||
        b.featureSource === "commons";
      if (civic && g.has(b.fileId)) forumNodeIds.push(b.fileId);
    }
    this.ambientLayer.setWorld(
      nodeIds,
      (fileId) => this.buildingNodes.get(fileId)?.iso ?? null,
      (from, to) => g.findRoute(from, to) ?? null,
      nodeWeights,
      forumNodeIds,
    );
    // B2c — cap the decorative crowd by the hardware profile (a lean/minimal tier
    // renders fewer walkers). The city-size derivation still applies under the cap.
    this.ambientLayer.setCount(
      desiredAmbientCount(nodeIds.length, this.profile.maxAmbientWalkers),
    );
  }

  /** Center + fit the viewport on the current content. */
  recenter(): void {
    if (this.buildingNodes.size === 0) {
      this.viewport.moveCenter(0, 0);
      this.viewport.setZoom(0.6, true);
      return;
    }
    let minX = Infinity,
      minY = Infinity,
      maxX = -Infinity,
      maxY = -Infinity;
    for (const node of this.buildingNodes.values()) {
      minX = Math.min(minX, node.iso.x);
      minY = Math.min(minY, node.iso.y);
      maxX = Math.max(maxX, node.iso.x);
      maxY = Math.max(maxY, node.iso.y);
    }
    this.fitCameraToIsoBounds(minX, minY, maxX, maxY);
  }


  /**
   * Fly the camera to a specific building by fileId. Animates 600ms with
   * easeInOutSine. If the current scale is below 0.6, also scales to 1.0.
   * No-op with a debug log when fileId is unknown.
   */
  flyTo(fileId: string): void {
    const node = this.buildingNodes.get(fileId);
    if (!node) {
      console.debug(`[Polis] flyTo: unknown fileId "${fileId}"`);
      return;
    }
    const opts: { position: { x: number; y: number }; time: number; ease: string; scale?: number } = {
      position: { x: node.iso.x, y: node.iso.y },
      time: 600,
      ease: "easeInOutSine",
    };
    if (this.viewport.scale.x < 0.6) {
      opts.scale = 1.0;
    }
    this.viewport.animate(opts);
  }

  /**
   * B2b — pre-fit the camera to a building set's ISO extent BEFORE the chunked
   * build runs, so the viewport already sits at its final framing and the build
   * prioritization can compute the truly-visible chunks. Computes the iso anchor of
   * each building directly from its grid coords (the SAME `cartToIso(coords)` the
   * node uses) — `buildingNodes` is empty pre-build. No-op for an empty set (the
   * empty-city path keeps the default camera). The final `recenter()` in finalize
   * re-fits from the placed nodes for exactness; this match means the pre-fit and
   * the final fit agree, so the camera does not jump when the build completes.
   */
  private fitCameraToBuildings(buildings: readonly Building[]): void {
    if (buildings.length === 0) return;
    let minX = Infinity,
      minY = Infinity,
      maxX = -Infinity,
      maxY = -Infinity;
    for (const b of buildings) {
      const iso = cartToIso(b.coords.x, b.coords.y);
      minX = Math.min(minX, iso.x);
      minY = Math.min(minY, iso.y);
      maxX = Math.max(maxX, iso.x);
      maxY = Math.max(maxY, iso.y);
    }
    this.fitCameraToIsoBounds(minX, minY, maxX, maxY);
  }

  /** Shared camera fit from an ISO-space bounding box (used by both `recenter` and
   *  the B2b pre-fit). Same framing policy: 120px margin, zoom clamped to [0.85,
   *  1.6] for a legible first impression. */
  private fitCameraToIsoBounds(
    minX: number,
    minY: number,
    maxX: number,
    maxY: number,
  ): void {
    const cx = (minX + maxX) / 2;
    const cy = (minY + maxY) / 2;
    // Tighter framing margin (was 240) so the fit doesn't pad the town into a
    // distant speck on load.
    const w = Math.max(maxX - minX, 1) + 120;
    const h = Math.max(maxY - minY, 1) + 120;
    // Open CLOSE: cap the fit at 1.6 and floor it at 0.85 so the first
    // impression is a legible town, never a tiny washed-out stamp. The user can
    // still freely zoom out afterwards (clampZoom range is unchanged).
    const fit = Math.min(this.viewport.findFit(w, h), 1.6);
    this.viewport.setZoom(Math.max(0.85, fit), true);
    this.viewport.moveCenter(cx, cy);
  }

  /**
   * B2b — the priority (first-pass) chunk KEYS + the center chunk for the CURRENT
   * viewport, over a CANDIDATE set of chunk keys. PURE w.r.t. the scene: it does
   * NOT read `this.chunks` (which is EMPTY at the start of a build, before any
   * building is placed) — instead the caller passes every chunk key the build's
   * buildings occupy, and we test each candidate chunk's iso bounds (via the pure
   * `computeChunkBounds`) against the visible world rectangle. So the same helper
   * works both for the INITIAL ordering (pre-build) and a mid-build
   * reprioritization. The base intersecting set is expanded by `ring` chunks (the
   * profile preload ring). The center chunk (distance origin for the remainder) is
   * whichever candidate chunk's bounds contain the viewport-center world point,
   * falling back to the chunk nearest that point.
   */
  private viewportPriorityChunks(
    candidateKeys: Iterable<string>,
    ring: number,
  ): { keys: Set<string>; center: { cx: number; cy: number } } {
    const left = this.viewport.left;
    const top = this.viewport.top;
    const w = Math.max(0, this.viewport.worldScreenWidth);
    const h = Math.max(0, this.viewport.worldScreenHeight);
    const viewRect = new Rectangle(left, top, w, h);
    const ccx = left + w / 2;
    const ccy = top + h / 2;

    const base = new Set<string>();
    // FIX 3 — track the two center candidates INDEPENDENTLY so the nearest-fallback
    // is robust: `nearestCenter` is the true nearest across ALL candidate chunks
    // (tracked UNCONDITIONALLY — the old code stopped nearest tracking after the
    // first containing chunk was found, so the distance-sort center could land on a
    // far chunk with big padded bounds); `containCenter` is the FIRST chunk whose
    // bounds contain the view center. The containing chunk wins when one exists,
    // otherwise the true nearest does.
    let nearestCenter: { cx: number; cy: number } = { cx: 0, cy: 0 };
    let containCenter: { cx: number; cy: number } | null = null;
    let nearestDist = Infinity;

    for (const key of candidateKeys) {
      const bounds = this.computeChunkBounds(key);
      if (viewRect.intersects(bounds)) base.add(key);
      const comma = key.indexOf(",");
      const cx = Number(key.slice(0, comma));
      const cy = Number(key.slice(comma + 1));
      // Nearest tracking is UNCONDITIONAL — every candidate updates it, so the
      // fallback is the genuine nearest chunk-center, not whatever happened to be
      // seen before the first containing chunk.
      const bcx = bounds.x + bounds.width / 2;
      const bcy = bounds.y + bounds.height / 2;
      const d = (bcx - ccx) ** 2 + (bcy - ccy) ** 2;
      if (d < nearestDist) {
        nearestDist = d;
        nearestCenter = { cx, cy };
      }
      // The FIRST chunk whose bounds contain the view center wins the contains race
      // (a robust fallback when the center falls in a gap, e.g. over water, uses
      // nearestCenter instead).
      if (containCenter === null && bounds.contains(ccx, ccy)) {
        containCenter = { cx, cy };
      }
    }

    return {
      keys: expandChunkRing(base, ring),
      center: containCenter ?? nearestCenter,
    };
  }

  /** B2b — debounce window (ms) for a camera-move reprioritization during the
   *  background fill. A pan/zoom burst coalesces to ONE re-sort of the remaining
   *  queue, so we never re-order on every intermediate `moved` event. */
  private static readonly REPRIORITIZE_DEBOUNCE_MS = 80;

  /** B2b — schedule (debounced) a reprioritization of the in-flight build's
   *  remaining queue against the CURRENT viewport. Coalesces a burst of camera
   *  events into one re-sort. Safe to call when no build is in flight (it still
   *  arms a timer that no-ops on fire because `buildState` is null by then). */
  private scheduleReprioritize(): void {
    if (this.reprioritizeTimer !== null) clearTimeout(this.reprioritizeTimer);
    this.reprioritizeTimer = setTimeout(() => {
      this.reprioritizeTimer = null;
      this.reprioritizeRemaining();
    }, PolisRenderer.REPRIORITIZE_DEBOUNCE_MS);
  }

  /**
   * B2b — re-sort the NOT-YET-PLACED tail of the in-flight build order against the
   * current viewport, so a pan/zoom mid-fill makes the newly-visible chunks build
   * next. PRESERVES the immutable head `[0, cursor)` (already-placed buildings are
   * never re-placed); only the remainder `[cursor, total)` is re-ordered, by the
   * SAME pure `orderBuildQueue` over the tail's chunk coords. No PIXI mutation, no
   * rebuild — just an array re-sort the next batch reads. No-op when no build is in
   * flight or the tail is empty.
   */
  private reprioritizeRemaining(): void {
    const state = this.buildState;
    if (!state) return;
    const { order, cursor, chunkXY, preloadRing } = state;
    if (cursor >= order.length) return; // nothing left to reorder

    // The tail's source indices (the buildings still to place).
    const tail = order.slice(cursor);
    // Candidate chunk keys = the chunks the REMAINING buildings occupy (the placed
    // head is irrelevant to where we go next).
    const candidateKeys = new Set(
      tail.map((idx) => `${chunkXY[idx].cx},${chunkXY[idx].cy}`),
    );
    const { keys: priorityKeys, center } = this.viewportPriorityChunks(
      candidateKeys,
      preloadRing,
    );
    const isPriority = priorityFromKeys(priorityKeys);
    // Order the tail's chunk coords (keep tail's own ordering stable within a
    // bucket so depth order is preserved), then map the LOCAL ordering back to the
    // tail's source indices and splice them in after the immutable head.
    const localOrder = orderBuildQueue(
      tail.map((idx) => chunkXY[idx]) as BuildQueueItem[],
      isPriority,
      center,
    );
    for (let k = 0; k < localOrder.length; k++) {
      order[cursor + k] = tail[localOrder[k]];
    }
    // FIX 2 — the head is now a DIFFERENT priority set (the new viewport's), so
    // the old visibleTotal lies. Recompute: the already-placed head [0, cursor)
    // stays (visibleDone is clamped to cursor anyway), plus the count of tail
    // items that fall in the NEW priority chunks — those are exactly the items
    // orderBuildQueue floated to the head of the re-sorted tail. Progress
    // callbacks read state.visibleTotal, so they now report the CURRENT visible
    // set instead of the stale initial one.
    let tailVisible = 0;
    for (const idx of tail) {
      const c = chunkXY[idx];
      if (isPriority(c.cx, c.cy)) tailVisible++;
    }
    state.visibleTotal = cursor + tailVisible;
    this.debugLog(
      `REPRIORITIZE cursor=${cursor}/${order.length} ` +
        `priorityChunks=${priorityKeys.size} visibleTotal=${state.visibleTotal}`,
    );
  }

  setSelected(fileId: string | null): void {
    this.selectedId = fileId;
    this.drawSelectionRing();
  }

  /**
   * Per-frame entry. Advances the 30fps STEPPED clock; only when the integer
   * frame changes do we flip visibility / nudge transforms on pre-built handles
   * and recycle smoke particles. Animates VISIBLE chunks only.
   */
  update(deltaMs: number): void {
    if (this.destroyed) return;

    // AgentMover: smooth per-FRAME travel along road routes (and fade-teleport /
    // appear fades). Integrated from real elapsed ms BEFORE the step-clock gate
    // so movement reads smoothly even though poses/bob stay on the 30fps step
    // cadence below. Allocation-light: only mutates omino/glow transform+alpha.
    this.agentLayer.update(deltaMs);
    this.ambientLayer.update(deltaMs);
    // Polis-P5 — flush a settled Censor findings debounce (the firefighter claims/
    // walks + lights its water arc once the engine's event burst quiesces). Gated
    // on a pending event so it's a single boolean test in steady state. The pure
    // core's clock is injected; in prod we use the monotonic performance.now().
    this.tickCensor(performance.now());
    // Trade-route porters: smooth per-FRAME travel along their routed polylines
    // (LOD-gated internally — a no-op while zoomed out, ZOOM-IN ONLY).
    this.tradeRouteLayer.update(deltaMs);

    // Day cycle: accumulate REAL elapsed time (a single frame's delta is capped
    // at 1s so a long background stall doesn't snap the tint forward). Recolored
    // on the step cadence below — once per ~33ms is plenty for a 4-minute loop.
    this.dayElapsedMs += Math.min(1000, deltaMs);

    if (!this.clock.advance(deltaMs)) return; // not a new step yet
    const frame = this.clock.frame;

    // FIX 5: if the host was zero-size at mount the day-cycle geometry never got
    // built (drawDayCycle early-returned). Draw it once the moment the host has a
    // real size, so the overlay appears without waiting for an explicit resize.
    // Steady-state cost is a single boolean test (no alloc, no redraw).
    if (!this.dayCycleDrawn && this.app.screen.width > 0 && this.app.screen.height > 0) {
      this.drawDayCycle();
    }

    // Recolor the screen-space day-cycle tint for the new elapsed time.
    this.applyDayCycle();

    // P5.1 — shadow skew: container-level skew.x driven by dayPhase.
    // One transform write per tick on the shadows layer (not per shadow).
    this.layers.shadows.skew.x = -0.12 + this.dayPhase * 0.24;

    // P5.1 — effects budget: bracket the effects pass.
    const effectsStart = performance.now();

    // Buildings: drive the ported kit anim instances (Flame/Beacon/Flag/Smoke/
    // Water) — but ONLY for nodes whose chunk is currently visible. Each anim's
    // update(t, dt) clears+redraws its own small Graphics (inherent to the
    // source art); culled buildings are skipped, so the per-frame redraw cost is
    // bounded to the handful of animated, on-screen buildings.
    //
    // dt is the elapsed step in seconds, clamped (long stalls can't make a flame
    // jump). t is the running total; matches the source harness's update(T, dt).
    const dt = Math.min(MAX_ANIM_DT, deltaMs / 1000);
    this.animT += dt;
    const t = this.animT;
    for (const node of this.animatedNodes) {
      const chunk = this.chunks.get(node.chunkKey);
      if (!(chunk?.visible ?? false)) continue; // off-screen: don't animate
      const anims = node.kitAnims;
      for (let i = 0; i < anims.length; i++) anims[i].update(t, dt);
    }

    // P5.1 — step crowd fires + hero fires (gated on budget rung).
    const rung = this.effectsBudget.rung;
    const prevRung = this._prevBudgetRung;
    this._prevBudgetRung = rung;
    const halfRate = rung >= 3; // rung 3+ → crowd at 15fps

    // Tier F2 promotion/demotion (reconcile only when rung < 1).
    // F8 — re-arm heroPromoDirty when budget recovers from >=1 back to <1,
    // so reconcileHeroFires doesn't bail out with a stale false flag.
    if (rung < 1 && this.fireAtlas) {
      if (prevRung >= 1) this.heroPromoDirty = true;
      this.reconcileHeroFires();
    }
    // Transition INTO rung >= 1: mass-demote all active heroes.
    if (rung >= 1 && prevRung === 0 && this.fireAtlas) {
      for (const hf of this.heroFirePool) {
        if (hf.targetFileId !== null) beginDemotionCrossfade(hf);
      }
    }
    // Always step heroes with active crossfade or target (regardless of rung).
    // Demotion fades play to completion; promotion fades only when rung<1.
    if (this.fireAtlas) {
      for (const hf of this.heroFirePool) {
        if (hf.targetFileId !== null || hf.crossfading) {
          stepHeroFire(hf, dt);
        }
        // F6 — gate hero fire container visibility via filter/LOD
        if (hf.targetFileId !== null) {
          const hnode = this.buildingNodes.get(hf.targetFileId);
          if (hnode) {
            hf.container.visible = this.effectVisible(hnode, hf.targetFileId, true);
          }
        }
      }
    }
    // Tier F1: crowd fires (always active unless rung 5 pauses everything).
    // Sync crowd fires from current burning buildings (no alloc in steady state).
    if (this.fireAtlas) {
      this.syncCrowdFires();
      for (const [fileId, cf] of this.crowdFires) {
        stepCrowdFire(cf, this.fireAtlas, frame, halfRate || rung >= 3);
        // F6 — gate crowd fire visibility via filter/LOD (no pool teardown)
        const vis = this.effectVisible(
          this.buildingNodes.get(fileId)!, fileId, true,
        );
        cf.fireSprite.visible = vis;
        cf.smokeSprite.visible = vis;
      }
    }

    // P5.1 — halos: additive sprites per on-screen burning building.
    this.updateHalos(frame);

    // Water shimmer: redraw the wave lines for VISIBLE terrain chunks only (each
    // is a bounded handful of strokes; off-screen water is skipped entirely so
    // the big-map water never costs per frame what it doesn't show).
    for (const tc of this.terrainChunks) {
      if (!tc.visible || !tc.chunk.anim) continue;
      tc.chunk.anim.update(t);
    }

    // L2 growth effects: advance the pooled one-shot bursts (visible-only
    // redraw, gated on `viewBounds`) + the tier-grow/pop-in node transitions.
    // Bounded, alloc-free in steady state. `viewBounds` is refreshed by the cull
    // pass (cullDirty is forced true after every diff, so it tracks the camera).
    this.growthFx.update(dt, this.viewBounds);

    // Agents: stepped bob + state poses + glow pulse. The SUBAGENT crowd redraw is
    // viewport-culled against the same `viewBounds` the trade porters use (#9) —
    // real agents are always stepped (few, and they carry the live marker/glow).
    this.agentLayer.step(frame, this.viewBounds);
    // Ambient crowd: stepped bob + figure redraw (LOD-gated + viewport-culled #9).
    // P5.1 budget rung 4 → half anim rate (every other step).
    // P5.1 budget rung 5 → ambient walkers pause.
    if (rung < 5) {
      if (rung < 4 || frame % 2 === 0) {
        this.ambientLayer.step(frame, this.viewBounds);
      }
    }
    // Trade-route porters: stepped bob + merchant figure redraw, LOD-gated and
    // VISIBLE-CHUNK-only (each porter is skipped unless its position is inside
    // `viewBounds` — the same cull rectangle the growth FX use, refreshed by the
    // cull pass which is forced after every diff so it tracks the camera).
    this.tradeRouteLayer.step(frame, this.viewBounds);
    // External cloud outposts: drive the kit harbour anims (Water ripple) via
    // update(t, dt) — same t/dt this loop uses for building anims — plus the
    // stepped status-lamp pulse for "spawning" nodes (steady states are set once
    // at build). LOD-gated inside the layer.
    this.externalLayer.step(frame, t, dt);

    // P5.1 — effects budget: record elapsed ms.
    const effectsElapsed = performance.now() - effectsStart;
    this.effectsBudget.record(effectsElapsed);

    // P5.1 — debug overlay (2Hz update).
    this.updateDebugOverlay(deltaMs, effectsElapsed);
  }

  // ---------------------------------------------------------------------------
  // Terrain + props
  // ---------------------------------------------------------------------------

  /**
   * (Re)draw the terrain + props layer for the given building set. Clears any
   * existing terrain children first, so it is safe to call on both the initial
   * build and a live diff (when buildings were added/removed and the extent may
   * have grown/shrunk). Both terrain and props share the terrain layer.
   */
  private redrawTerrainProps(
    buildings: Building[],
    gridSize: GridSize,
    terrain?: TerrainData,
    districts?: District[],
    roads?: Road[],
  ): void {
    // `removeChildren().destroy({children:true})` tears down BOTH the ground/props
    // Graphics and any previous terrain-frame chunk containers (whose own child
    // Graphics — sand/water/shimmer/bridges — must be freed too), so resetting the
    // tracking array after is leak-free.
    this.layers.terrain
      .removeChildren()
      .forEach((c) => c.destroy({ children: true }));
    this.terrainChunks = [];
    this.terrainGridGraphics = null;
    if (this.fieldsGraphics) {
      this.fieldsGraphics.destroy();
      this.fieldsGraphics = null;
    }
    const ext = computeExtent(
      buildings.map((b) => b.coords),
      gridSize.w,
      gridSize.h,
      4,
    );
    this.drawTerrainLayer(ext);

    // Fields (farmland parcels) — drawn between terrain and props.
    let fieldTileSet: Set<string> | null = null;
    if (districts && roads) {
      const blocked = buildFieldBlockedSet(
        buildings.map((b) => ({ coords: [b.coords] })),
        roads,
        terrain,
      );
      const centre = {
        x: (ext.minX + ext.maxX) / 2,
        y: (ext.minY + ext.maxY) / 2,
      };
      const parcels = planFields({
        ext,
        districts: districts.map((d) => d.bounds),
        blocked,
        centre,
      });
      if (parcels.length > 0) {
        const { graphics } = drawFields(ext, parcels);
        this.fieldsGraphics = graphics;
        // Apply LOD immediately so a live-diff rebuild at low zoom doesn't
        // leave the fresh Graphics visible=true (PixiJS default) until the
        // next zoom change re-enters the LOD block in updateCulling().
        graphics.visible = this.viewport.scale.x >= LOD_FIELDS;
        this.layers.terrain.addChild(graphics);
        fieldTileSet = parcelTiles(parcels);
      }
    }

    this.drawPropsLayer(ext, buildings, fieldTileSet);
    // Water frame on top of the grass ground but BELOW everything else (it lives
    // in the terrain layer). Chunked so the cull pass can hide off-screen water.
    this.drawWaterFrame(terrain);
  }

  /** Build the sparse sea/river/sand/bridge frame into CHUNK-keyed containers and
   *  track their iso bounds so the cull pass can toggle them with the buildings. */
  private drawWaterFrame(terrain?: TerrainData): void {
    const frame = buildTerrainFrame(terrain, CHUNK_SIZE);
    for (const chunk of frame) {
      this.layers.terrain.addChild(chunk.container);
      this.terrainChunks.push({
        chunk,
        bounds: this.computeChunkBounds(chunk.key),
        visible: true,
      });
    }
  }

  private drawTerrainLayer(ext: ReturnType<typeof computeExtent>): void {
    const { graphics, gridGraphics } = drawTerrain(ext);
    for (const g of graphics) this.layers.terrain.addChild(g);
    // T6a — track grid Graphics separately for zoom gating (sub-pixel below 0.5).
    this.terrainGridGraphics = gridGraphics;
    if (gridGraphics) {
      gridGraphics.visible = this.viewport.scale.x >= 0.5;
      this.layers.terrain.addChild(gridGraphics);
    }
  }

  private drawPropsLayer(
    ext: ReturnType<typeof computeExtent>,
    buildings: Building[],
    fieldTiles?: Set<string> | null,
  ): void {
    const occupied = occupiedTiles(buildings.map((b) => b.coords));
    // Union field parcel tiles into occupied so props never spawn inside parcels.
    if (fieldTiles) {
      for (const key of fieldTiles) occupied.add(key);
    }
    const { graphics } = drawProps(ext, occupied);
    for (const g of graphics) this.layers.terrain.addChild(g);
  }

  // ---------------------------------------------------------------------------
  // Districts
  // ---------------------------------------------------------------------------

  /** Clear + redraw the districts layer (used by the live diff). */
  private redrawDistricts(districts: District[]): void {
    this.layers.districts
      .removeChildren()
      .forEach((c) => c.destroy({ children: true }));
    this.drawDistricts(districts);
  }

  private drawDistricts(districts: District[]): void {
    for (const d of districts) {
      const g = new Graphics();
      const { x, y, w, h } = d.bounds;
      const c0 = cartToIso(x, y);
      const c1 = cartToIso(x + w, y);
      const c2 = cartToIso(x + w, y + h);
      const c3 = cartToIso(x, y + h);
      let tint: number = PALETTE.sandDark;
      if (/^#?[0-9a-fA-F]{6}$/.test(d.colorAccent)) {
        tint = parseInt(d.colorAccent.replace("#", ""), 16);
      }
      g.poly([c0.x, c0.y, c1.x, c1.y, c2.x, c2.y, c3.x, c3.y]).fill({
        color: tint,
        alpha: ALPHA.districtFill,
      });
      g.poly([c0.x, c0.y, c1.x, c1.y, c2.x, c2.y, c3.x, c3.y], true).stroke({
        color: tint,
        alpha: ALPHA.districtStroke,
        width: 2,
      });

      const label = new Text({
        text: d.name,
        style: new TextStyle({
          fontFamily: "Inter, system-ui, sans-serif",
          fontSize: 13,
          fontWeight: "600",
          fill: PALETTE.stoneDark,
        }),
      });
      label.anchor.set(0.5, 1);
      label.position.set(c0.x, c0.y - 6);

      const group = new Container();
      group.addChild(g);
      group.addChild(label);
      this.layers.districts.addChild(group);
    }
  }

  // ---------------------------------------------------------------------------
  // Roads ("lastricata" simplified)
  // ---------------------------------------------------------------------------

  /**
   * Clear + redraw the whole roads layer (used by the live diff). Roads are a
   * small batched Graphics (trunk + minor sub-layers); clearing and redrawing
   * once is cheap, and imports may have changed when a file changed. The
   * trunk/minor LOD split is re-established by drawRoads (it sets
   * `roadMinorLayer` + applies the current zoom LOD).
   */
  private redrawRoads(roads: Road[], byId: Map<string, Building>): void {
    this.layers.roads
      .removeChildren()
      .forEach((c) => c.destroy({ children: true }));
    this.roadMinorLayer = null;
    this.drawRoads(roads, byId);
  }

  private drawRoads(roads: Road[], byId: Map<string, Building>): void {
    const ghosted = this.filterSets?.ghostedFileIds;
    const isHideMode = this.filterSets?.mode === "hide";
    const roadAlphaMult = (from: string, to: string): number => {
      if (!ghosted || ghosted.size === 0) return 1;
      const endpointGhosted = ghosted.has(from) || ghosted.has(to);
      if (!endpointGhosted) return 1;
      if (isHideMode) return 0; // vanished building → no roads
      return 0.3;
    };
    // VISUAL HIERARCHY. The backend over-draws: ~35% of roads are weight-1
    // single-import lanes and ~half lack a routed `path` (straight fallbacks).
    // Drawn uniformly they bury the town in a red mesh. We split the network in
    // two so the eye reads avenues, not a web:
    //   - TRUNKS  → cobbled, opaque, wide. A segment is a trunk when it is
    //               shared by several routed roads OR the import is heavy.
    //   - MINOR   → a single faint earth line, low alpha, hidden when zoomed
    //               out. Built into its own sub-container so the LOD pass can
    //               toggle it allocation-free (no geometry rebuild).
    //
    // Each tier draws into ONE batched Graphics at a CONSISTENT alpha, so the
    // many crossings DON'T multiply into dark blobs — overlaps stay flat.

    // --- Pass 1: per-segment usage (how SHARED a segment is). Quantize each
    // routed segment's endpoints (order-independent) and count incident roads.
    // Trunk-merging in the backend means shared trunks light up here. O(points).
    const segUsage = new Map<string, number>();
    const segKey = (a: { x: number; y: number }, b: { x: number; y: number }) => {
      const k1 = `${a.x},${a.y}`;
      const k2 = `${b.x},${b.y}`;
      return k1 < k2 ? `${k1}|${k2}` : `${k2}|${k1}`;
    };
    for (const road of roads) {
      const p = road.path;
      if (!p || p.length < 2) continue;
      for (let i = 0; i < p.length - 1; i++) {
        const k = segKey(p[i], p[i + 1]);
        segUsage.set(k, (segUsage.get(k) ?? 0) + 1);
      }
    }

    // Two batched layers + their Graphics. minorG is faded by zoom in the LOD
    // pass; trunkG is always visible. Minor draws BELOW the trunk so cobble
    // always sits on top of the faint lanes at a crossing.
    //
    // T6a — Pixi v8 marks Graphics with ≥400 vertices as non-batchable (each
    // shape primitive → a separate GL draw call). We rotate Graphics every
    // ROAD_CHUNK_OPS (80) fill/stroke operations so each stays under the
    // batchability threshold → the batcher merges them into O(10) draw calls.
    const ROAD_CHUNK_OPS = 80;
    const minorLayer = new Container();
    const trunkLayer = new Container();
    let minorG = new Graphics();
    let minorOps = 0;
    let trunkG = new Graphics();
    let trunkOps = 0;
    minorLayer.addChild(minorG);
    trunkLayer.addChild(trunkG);

    // Rotate to a fresh Graphics when the current chunk is full.
    const rotateMinor = (ops: number): void => {
      minorOps += ops;
      if (minorOps >= ROAD_CHUNK_OPS) {
        minorG = new Graphics();
        minorLayer.addChild(minorG);
        minorOps = 0;
      }
    };
    const rotateTrunk = (ops: number): void => {
      trunkOps += ops;
      if (trunkOps >= ROAD_CHUNK_OPS) {
        trunkG = new Graphics();
        trunkLayer.addChild(trunkG);
        trunkOps = 0;
      }
    };

    // Junction nodes: only TRUE trunk hubs (>= ROAD_JUNCTION_MIN routed roads
    // through a waypoint). Drawn last on the trunk layer. Keyed by quantized
    // iso so coincident waypoints across roads collapse to one count.
    const junctionCount = new Map<string, { x: number; y: number; n: number }>();
    const bumpJunction = (p: IsoPoint): void => {
      const key = `${Math.round(p.x)},${Math.round(p.y)}`;
      const entry = junctionCount.get(key);
      if (entry) entry.n += 1;
      else junctionCount.set(key, { x: p.x, y: p.y, n: 1 });
    };

    // Deterministic draw order: roads arrive stably ordered from the backend.
    for (const road of roads) {
      const heavyImport = road.weight >= ROAD_WEIGHT_TRUNK;

      // Prefer the WORLD-GRID street polyline (>=2 points): draw segment by
      // segment so each segment is classified by ITS OWN shared-ness — a road
      // can run as a faint lane until it merges onto a busy avenue, then read
      // as a trunk for the shared stretch (true street-network feel).
      if (road.path && road.path.length >= 2) {
        const raw = road.path;
        const pts = raw.map((p) => cartToIso(p.x, p.y));
        let anyTrunk = false;
        for (let i = 0; i < pts.length - 1; i++) {
          const shared = segUsage.get(segKey(raw[i], raw[i + 1])) ?? 1;
          const isTrunk = heavyImport || shared >= ROAD_SHARED_TRUNK;
          if (isTrunk) {
            rotateTrunk(this.drawTrunk(trunkG, pts[i], pts[i + 1], road.weight, shared, roadAlphaMult(road.from, road.to)));
            anyTrunk = true;
          } else {
            rotateMinor(this.drawMinorLane(minorG, pts[i], pts[i + 1], roadAlphaMult(road.from, road.to)));
          }
        }
        // Only count junctions for trunk-bearing routes — minor kinks are not
        // intersections worth a disc.
        if (anyTrunk) {
          for (let i = 1; i < pts.length - 1; i++) bumpJunction(pts[i]);
          bumpJunction(pts[0]);
          bumpJunction(pts[pts.length - 1]);
        }
        continue;
      }

      // Fallback: no routed path. These straight `from`->`to` lines cut
      // corner-to-corner across the map and ARE the spiderweb — they don't
      // follow streets and can't share segments. So a straight fallback is
      // ALWAYS a faint minor lane (never a cobbled trunk), regardless of
      // weight: the cobbled-avenue read is reserved for the routed network
      // that actually forms streets. This drops the long diagonal slashes out
      // of the trunk layer and into the LOD-hidden minor layer.
      const from = byId.get(road.from);
      const to = byId.get(road.to);
      if (!from || !to) continue; // only draw roads whose endpoints exist
      const a = cartToIso(from.coords.x, from.coords.y);
      const b = cartToIso(to.coords.x, to.coords.y);
      rotateMinor(this.drawMinorLane(minorG, a, b, roadAlphaMult(road.from, road.to)));
    }

    // True trunk-hub discs. A single neutral disc tidies the overlap where many
    // avenues genuinely meet. Static (one pass, no per-frame work).
    for (const j of junctionCount.values()) {
      if (j.n >= ROAD_JUNCTION_MIN) {
        trunkG
          .circle(j.x, j.y, 4)
          .fill({ color: ROAD.junction, alpha: ROAD_ALPHA.junction });
      }
    }

    this.layers.roads.addChild(minorLayer);
    this.layers.roads.addChild(trunkLayer);
    // Hand the minor layer to the LOD pass; set its initial visibility/alpha now
    // (cull runs next tick, but this avoids a one-frame flash at the wrong LOD).
    this.roadMinorLayer = minorLayer;
    this.applyRoadLod(this.viewport.scale.x);
  }

  // TRUNK: cobbled lastricata — alternating muted stones + a soft kerb. Width
  // and alpha scale with BOTH import weight AND shared-ness so the busiest
  // avenues read widest/most solid. Drawn into the shared trunk Graphics at a
  // flat alpha so crossings stay clean.
  // T6a: returns fill+stroke count so drawRoads can rotate Graphics chunks.
  private drawTrunk(
    g: Graphics,
    from: IsoPoint,
    to: IsoPoint,
    weight: number,
    shared: number,
    alphaMult = 1,
  ): number {
    // weight 1..5 and shared 1..~13 both push the trunk wider. Clamp so the
    // fattest avenue stays sane. Base 6, +~1.4/weight, +~0.8/extra-share.
    const w = Math.max(1, Math.min(weight, 5));
    const s = Math.max(1, Math.min(shared, 8));
    const roadWidth = Math.min(6 + w * 1.4 + (s - 1) * 0.8, 22);
    const total = dist(from, to);
    const stoneLen = 14;
    const steps = Math.max(1, Math.floor(total / stoneLen));
    const dx = to.x - from.x;
    const dy = to.y - from.y;
    const len = total || 1;
    const px = (-dy / len) * (roadWidth / 2);
    const py = (dx / len) * (roadWidth / 2);

    for (let i = 0; i < steps; i++) {
      const t0 = i / steps;
      const t1 = (i + 0.82) / steps;
      const p0 = lerp(from, to, t0);
      const p1 = lerp(from, to, t1);
      const color = i % 2 === 0 ? ROAD.trunkStone : ROAD.trunkStoneAlt;
      g.poly([
        p0.x + px,
        p0.y + py,
        p1.x + px,
        p1.y + py,
        p1.x - px,
        p1.y - py,
        p0.x - px,
        p0.y - py,
      ]).fill({ color, alpha: ROAD_ALPHA.trunkFill * alphaMult });
    }
    g.moveTo(from.x + px, from.y + py).lineTo(to.x + px, to.y + py);
    g.moveTo(from.x - px, from.y - py).lineTo(to.x - px, to.y - py);
    g.stroke({ color: ROAD.trunkKerb, alpha: ROAD_ALPHA.trunkKerb * alphaMult, width: 1 });
    return steps + 1; // fills + 1 stroke
  }

  // MINOR: a single faint desaturated earth line — a hint of a footpath, not a
  // cobbled street. One stroke per segment into the shared minor Graphics at a
  // flat low alpha; LOD fades/hides the whole layer when zoomed out.
  // T6a: returns stroke count so drawRoads can rotate Graphics chunks.
  private drawMinorLane(g: Graphics, from: IsoPoint, to: IsoPoint, alphaMult = 1): number {
    g.moveTo(from.x, from.y).lineTo(to.x, to.y);
    g.stroke({ color: ROAD.minorPath, alpha: ROAD_ALPHA.minor * alphaMult, width: 2 });
    return 1; // 1 stroke
  }

  // LOD for roads: fade/hide the minor-lane layer by zoom. Allocation-free —
  // only flips visibility + alpha on the prebuilt sub-container. Below
  // LOD_ROAD_MINOR the far view shows trunks only; above it the minor lanes
  // ramp from invisible to their drawn alpha so the network reveals as you
  // zoom in.
  private applyRoadLod(scale: number): void {
    const layer = this.roadMinorLayer;
    if (!layer) return;
    if (scale < LOD_ROAD_MINOR) {
      layer.visible = false;
      return;
    }
    layer.visible = true;
    // Ramp 0..1 across LOD_ROAD_MINOR .. (LOD_ROAD_MINOR + 0.35) so the reveal
    // is a smooth fade-in rather than a pop.
    const t = Math.min(1, (scale - LOD_ROAD_MINOR) / 0.35);
    layer.alpha = t;
  }

  // ---------------------------------------------------------------------------
  // Buildings
  // ---------------------------------------------------------------------------

  /**
   * Destroy the freshly-built kit parts (static body, shadow, pennant) that the
   * atlas would normally consume — used on the atlas-throw error path where the
   * atlas ran our `build` closure but never reached its own destroy, leaving these
   * orphaned. Each destroy is `.destroyed`-guarded so it is safe to call even when
   * the atlas (or a HIT path) already disposed some of them. The pennant is only
   * ever an unparented Graphics at this point (attachBuildingDynamics adds it).
   */
  private disposeBuiltParts(built: BuiltParts): void {
    if (!built.staticBody.destroyed) built.staticBody.destroy({ children: true });
    if (!built.shadow.destroyed) built.shadow.destroy();
    if (built.pennant && !built.pennant.destroyed) built.pennant.destroy();
  }

  /**
   * Build ONE building and register it into all the renderer's structures
   * (chunk, node map, animated/smoke node lists). The SINGLE source of truth for
   * "how a building is made" — used by both the chunked `setCityState` build
   * loop and the live `applyCityDiff` so a diff-rebuilt building is byte-for-byte
   * the same as a freshly-scanned one.
   */
  private createBuildingNode(b: Building): BuildingNode {
    const iso = cartToIso(b.coords.x, b.coords.y);
    const profile = getProfile(b.purpose);
    const scale = tierScale(b.visualTier);
    const level = tierRank(b.visualTier); // atlas variant axis (0..4)

    // Build the kit split into STATIC body (textured + cached) vs the cheap live
    // parts (anims + pennant) we keep per building. The kit build is unavoidable
    // per building because the animated parts are positioned from its geometry,
    // but the HEAVY static body Graphics is captured to a SHARED texture and
    // destroyed (by the atlas on a miss, by us on a hit) so it is never retained.
    const built = buildBuildingParts(b, profile, scale);

    // Was the (purpose, level) variant already cached? If so the atlas will NOT
    // consume our freshly-built static body/shadow — we own their disposal.
    const wasCached = this.buildingAtlas.has(b.purpose, level);
    let variant: import("./buildingAtlas").BuildingVariant;
    try {
      variant = this.buildingAtlas.get(
        this.app.renderer as unknown as import("./buildingAtlas").TextureSource,
        b.purpose,
        level,
        () => ({ body: built.staticBody, shadow: built.shadow }),
      );
    } catch (err) {
      // generateTexture (or any atlas step) threw on a MISS: the atlas ran our
      // `build` closure but never reached its own destroy, so OUR staticBody/shadow
      // leak; the pennant was never parented anywhere either. Destroy all three
      // (guarded — the atlas may already have destroyed some) before re-throwing.
      this.disposeBuiltParts(built);
      throw err;
    }
    if (wasCached) {
      // HIT: the atlas returned the shared texture without touching our copies —
      // destroy them so the heavy static Graphics is not retained or orphaned.
      built.staticBody.destroy({ children: true });
      built.shadow.destroy();
    }

    // Drop-shadow SPRITE (shared texture). Anchored so the shadow ORIGIN sits at
    // iso (where the per-building shadow Graphics was drawn from local (0,0)).
    const shadowSprite = this.makeShadowSprite(variant);
    shadowSprite.position.set(iso.x, iso.y);
    shadowSprite.zIndex = depthKey(b.coords.x, b.coords.y);
    shadowSprite.eventMode = "none"; // shadows never intercept clicks
    this.layers.shadows.addChild(shadowSprite);

    // SAFETY: everything after this point runs inside a try/catch so that if any
    // mid-build step throws, we remove and destroy the shadow sprite we just
    // parented (the only child added to a shared layer so far) before re-throwing.
    // Without this, the shadow Sprite is an untracked orphan on layers.shadows —
    // invisible to culling and never destroyed until the next clearScene. (The
    // shared TEXTURE is owned by the atlas; the sprite destroy never frees it.)
    try {

    // Node root: a plain Container at the iso anchor (origin at iso, EXACTLY as the
    // old per-building container) holding the batched body Sprite as child 0. The
    // Container — not the Sprite — owns the children (idiomatic pixi v8) so the
    // label/anims/pennant/overlays attach to it with their identical local coords.
    const container = new Container();
    container.position.set(iso.x, iso.y);
    container.zIndex = depthKey(b.coords.x, b.coords.y);
    container.eventMode = "static";
    container.cursor = "pointer";
    (container as Container & { __fileId?: string }).__fileId = b.fileId;
    // Body Sprite (child 0) — its anchor places the shared static texture so the
    // body pixels land exactly where the old kit Graphics did.
    const bodySprite = this.makeBodySprite(variant);
    container.addChild(bodySprite);

    // NOTE: the provider pennant is parented EXCLUSIVELY by attachBuildingDynamics
    // (below), which is the SOLE add point shared with the in-place diff path. Do
    // NOT addChild(built.pennant) here too — a second addChild re-parents in pixi,
    // appending the pennant ABOVE the scaffold/disaster/investigation overlays and
    // violating the body < pennant < anims < disaster < investigation < label order.

    const chunkKey = this.chunkKey(b.coords.x, b.coords.y);

    // Attach the live, on-demand dynamic parts (anims, scaffold, disaster,
    // investigation, pennant, label) onto the Sprite. Shared with the in-place
    // diff update path so a CHANGED building reuses identical overlay logic.
    const dyn = this.attachBuildingDynamics(container, b, built);

    const node: BuildingNode = {
      building: b,
      iso,
      container,
      bodySprite,
      shadow: shadowSprite,
      kitAnims: dyn.anims,
      label: dyn.label,
      labelDepth: built.depth,
      pennant: dyn.pennant,
      disaster: dyn.disaster,
      investigation: dyn.investigation,
      hitRadius: built.hw,
      chunkKey,
    };

    // Pointer handlers read `node.building` at FIRE TIME (not the closure `b`):
    // `updateBuildingNodeInPlace` preserves this Container + its listeners and only
    // re-points `node.building`, so the in-place diff path (the common case) would
    // otherwise deliver a STALE Building (old tier/sins/provider) to the inspector.
    // The node object exists before these are wired, so the field is always set.
    container.on("pointertap", (e) => {
      // Consume the tap so the viewport background handler doesn't also fire
      // and immediately deselect what we just selected.
      e.stopPropagation();
      this.callbacks.onSelectBuilding?.(node.building);
    });
    container.on("pointerover", () =>
      this.callbacks.onHoverBuilding?.(node.building),
    );
    container.on("pointerout", () => this.callbacks.onHoverBuilding?.(null));

    this.buildingNodes.set(b.fileId, node);
    // P3.2 — apply current filter to this newly-created node so buildings born
    // during incremental build are filtered from birth.
    this.applyFilterToNode(node, b.fileId);
    // Polis-P5 — index the normalized project-relative path → fileId so the Censor
    // relPath→building resolution mirrors the agents' fileId-based resolution.
    this.fileIdByPath.set(PolisRenderer.normalizeRelPath(b.filePath), b.fileId);
    this.addToChunk(b.coords.x, b.coords.y, container);

    // Track nodes with any animated part for the per-step driver (a scaffold
    // counts, so an agent-present static building still animates its rig).
    if (dyn.anims.length > 0) {
      this.animatedNodes.push(node);
    }
    return node;
    } catch (err) {
      // Mid-build throw: remove the shadow SPRITE we already parented to the shared
      // layer so it does not orphan there until the next clearScene. The container
      // sprite was not yet added to any chunk/layer, so no container cleanup is
      // needed. Nothing was written to buildingNodes or fileIdByPath yet (those
      // lines are below, inside the try), so those indices remain clean. The shared
      // TEXTURE is owned by the atlas — the sprite destroy never frees it. Re-throw
      // so the outer per-node handler (runBatch) can log+skip and the diff propagate.
      shadowSprite.removeFromParent();
      shadowSprite.destroy();
      throw err;
    }
  }

  /**
   * Build the batched body Sprite for a texture variant. The anchor is set to
   * -frame/size so the Sprite's LOCAL ORIGIN coincides with the building's iso
   * anchor (front-bottom): placing the Sprite at local (0,0) inside the node
   * Container (whose origin is iso) renders the texture's pixel that was at local
   * (frame.x, frame.y) at world iso+(frame.x, frame.y) — i.e. EXACTLY where the
   * old kit Graphics drew it. Guarded against a zero-size frame.
   */
  private makeBodySprite(
    variant: import("./buildingAtlas").BuildingVariant,
  ): Sprite {
    const s = new Sprite(variant.texture);
    const fr = variant.frame;
    s.anchor.set(
      fr.width > 0 ? -fr.x / fr.width : 0,
      fr.height > 0 ? -fr.y / fr.height : 0,
    );
    return s;
  }

  /** Build the batched shadow Sprite for a variant — same origin-at-iso anchor
   *  math as {@link makeBodySprite}, applied to the shadow frame. */
  private makeShadowSprite(
    variant: import("./buildingAtlas").BuildingVariant,
  ): Sprite {
    const s = new Sprite(variant.shadowTexture);
    const sf = variant.shadowFrame;
    s.anchor.set(
      sf.width > 0 ? -sf.x / sf.width : 0,
      sf.height > 0 ? -sf.y / sf.height : 0,
    );
    return s;
  }

  /**
   * Build the LOD-gated filename label Text for a building. Extracted so the LOD
   * pass can attach it lazily (and `createBuildingNode` re-use it): below
   * LOD_LABELS no Text object exists at all, so a far view of a large city keeps
   * zero label glyphs in memory. Anchored above the silhouette (`depthPx` = the
   * building's pixel height above the iso anchor).
   */
  private makeLabel(b: Building, depthPx: number): Text {
    const label = new Text({ text: b.label, style: this.labelStyle });
    label.anchor.set(0.5, 1);
    label.position.set(0, -depthPx - 6);
    return label;
  }

  /**
   * Attach the on-demand DYNAMIC parts of a building onto its Sprite `container`:
   * the kit's live anim part nodes (re-parented off the static body), the L2
   * scaffold (agentPresent), the disaster (sins) + investigation (suspect)
   * overlays, the provider pennant, and the LOD-gated filename label. Child
   * z-order (bottom→top): body texture < pennant < anims/scaffold < disaster <
   * investigation < label — the label is added LAST so it stays topmost. Shared by
   * `createBuildingNode` and the in-place diff update so both produce an identical
   * overlay set. Returns the dynamic refs for the BuildingNode record. The caller
   * owns `animatedNodes` membership (anims length drives it).
   */
  private attachBuildingDynamics(
    container: Container,
    b: Building,
    built: BuiltParts,
  ): {
    anims: AnimInstance[];
    label: Text | null;
    pennant: Graphics | null;
    disaster: Disaster | null;
    investigation: Investigation | null;
  } {
    // Provider pennant FIRST (above the body texture, below overlays + label). It
    // varies by provider so it is an OVERLAY, not baked into the shared texture.
    let pennant: Graphics | null = built.pennant;
    if (pennant) {
      pennant.visible = this.viewport.scale.x >= LOD_LIVERY;
      container.addChild(pennant);
    }

    // Kit anim part nodes (Flame/Beacon/Flag/Smoke/Water) — detached from the
    // static body in buildBuildingParts, re-parented here so they animate live.
    const anims = built.anims;
    for (const a of anims) container.addChild(a.node);

    // L2 SCAFFOLDING — agentPresent: an AnimInstance over the body. Pushed into
    // kitAnims so the visible-chunk-only step driver animates it for free.
    if (b.agentPresent) {
      const scaffold = new Scaffold(built.hw, built.depth);
      container.addChild(scaffold.node);
      anims.push(scaffold);
    }

    // ON-MAP DISASTER — worst sin severity drives a kit fire/smoke overlay over the
    // body + any scaffold. LOD-seeded so it doesn't flash at the wrong zoom.
    let disaster: Disaster | null = null;
    const worst = worstSinSeverity(b);
    if (worst) {
      disaster = new Disaster(worst, built.hw, built.depth);
      container.addChild(disaster.node);
      disaster.node.visible = this.viewport.scale.x >= LOD_DISASTER;
      anims.push(disaster);
    }

    // ON-MAP INVESTIGATION (P3) — suspectOfCardId: tinted-smoke + "?" overlay,
    // COEXISTS with the disaster (a file can be both a suspect and a disaster).
    let investigation: Investigation | null = null;
    if (b.suspectOfCardId) {
      investigation = new Investigation(built.hw, built.depth);
      container.addChild(investigation.node);
      investigation.node.visible = this.viewport.scale.x >= LOD_DISASTER;
      anims.push(investigation);
    }

    // Filename label LAST (topmost). LAZY: below the profile's label-IN threshold no
    // Text is created, so a far view of a huge city retains zero label glyphs (the
    // LOD pass attaches it on zoom-in). Seeded from the per-instance `lodLabelsIn`
    // (B2c) so a node built while zoomed in agrees with the cull pass's create gate.
    let label: Text | null = null;
    if (this.viewport.scale.x >= this.lodLabelsIn) {
      label = this.makeLabel(b, built.depth);
      container.addChild(label);
    }

    return { anims, label, pennant, disaster, investigation };
  }

  /**
   * Tear down ONLY the dynamic overlays of a node (anims, scaffold, disaster,
   * investigation, pennant, label) — detaching + destroying their child nodes —
   * WITHOUT touching the Sprite, its shared texture, its chunk membership, or the
   * shadow. Used by the in-place diff update before re-attaching fresh dynamics.
   * Also splices the node out of `animatedNodes` (re-added by the caller iff the
   * new dynamics have anims), mirroring destroyBuildingNode's pool discipline.
   */
  private detachBuildingDynamics(node: BuildingNode): void {
    // The anim part nodes, pennant, label, disaster/investigation nodes are all
    // children of node.container — remove + destroy each so no orphan/leak remains.
    for (const a of node.kitAnims) {
      a.node.removeFromParent();
      a.node.destroy({ children: true });
    }
    node.kitAnims = [];
    if (node.label) {
      node.label.removeFromParent();
      node.label.destroy();
      node.label = null;
    }
    if (node.pennant) {
      node.pennant.removeFromParent();
      node.pennant.destroy();
      node.pennant = null;
    }
    // disaster/investigation are AnimInstances already destroyed via kitAnims above
    // (their .node was in kitAnims); just drop the references.
    node.disaster = null;
    node.investigation = null;
    // Drop from the animated pool; re-added by the caller iff new anims exist.
    removeFromArrayByIdentity(this.animatedNodes, node);
  }

  /**
   * LIVE DIFF — update a CHANGED building IN PLACE on its existing Sprite, without
   * destroying + rebuilding the node, when its grid COORDS are unchanged (the
   * common case: a file edit changes tier/status/sins/provider/suspect/agent, not
   * its position). Swaps the building + shadow textures to the new variant
   * (re-anchoring), tears down + re-attaches the dynamic overlays, and updates the
   * node's metrics + `building` snapshot. The Sprite object, its chunk membership,
   * its event handlers, and the node record identity are PRESERVED — so the
   * selection ring, growth transitions, and any consumer holding the node keep
   * working. A coords change still falls back to destroy+rebuild (re-chunk) in the
   * caller. Returns the same (mutated) node.
   */
  private updateBuildingNodeInPlace(node: BuildingNode, b: Building): BuildingNode {
    // Re-point the node's Building snapshot FIRST: the preserved pointer handlers
    // read `node.building` at fire time, so the new Building must be live before
    // anything else in this method can run (or a tap mid-update would still resolve
    // the stale snapshot). The remaining node fields are refreshed below.
    // Keep the OLD snapshot so the atlas-miss catch can restore it: if we re-throw
    // with the visuals still the OLD building, `node.building` must point at the OLD
    // building too — otherwise the inspector would open with NEW data over OLD art.
    const prev = node.building;
    node.building = b;

    const profile = getProfile(b.purpose);
    const scale = tierScale(b.visualTier);
    const level = tierRank(b.visualTier);

    // Build the new kit parts (for fresh anims/pennant/metrics) + the new variant
    // texture. Same atlas hit/miss disposal contract as createBuildingNode.
    const built = buildBuildingParts(b, profile, scale);
    const wasCached = this.buildingAtlas.has(b.purpose, level);
    let variant: import("./buildingAtlas").BuildingVariant;
    try {
      variant = this.buildingAtlas.get(
        this.app.renderer as unknown as import("./buildingAtlas").TextureSource,
        b.purpose,
        level,
        () => ({ body: built.staticBody, shadow: built.shadow }),
      );
    } catch (err) {
      // Atlas threw on a MISS: destroy our freshly-built parts (the atlas never
      // reached its own destroy) so they don't leak. The EXISTING node is left
      // untouched — its old dynamics/textures are still valid — and we re-throw so
      // the caller's per-node handler logs+skips this diff entry. Restore the OLD
      // building snapshot we re-pointed at the top: the visuals are still the OLD
      // building, so node.building must match (else the inspector shows NEW data).
      node.building = prev;
      this.disposeBuiltParts(built);
      throw err;
    }
    if (wasCached) {
      built.staticBody.destroy({ children: true });
      built.shadow.destroy();
    }

    // Tear down the OLD dynamics first (so re-attach lands a clean child set).
    this.detachBuildingDynamics(node);

    // Swap the BODY sprite texture + re-anchor (the node Container origin stays at
    // iso, so overlay local coords are unchanged). The OLD shared texture is
    // atlas-owned — never freed here; another building of the old variant may use it.
    const fr = variant.frame;
    node.bodySprite.texture = variant.texture;
    node.bodySprite.anchor.set(
      fr.width > 0 ? -fr.x / fr.width : 0,
      fr.height > 0 ? -fr.y / fr.height : 0,
    );

    // Swap the shadow texture + re-anchor likewise.
    const sf = variant.shadowFrame;
    node.shadow.texture = variant.shadowTexture;
    node.shadow.anchor.set(
      sf.width > 0 ? -sf.x / sf.width : 0,
      sf.height > 0 ? -sf.y / sf.height : 0,
    );

    // Re-attach fresh dynamics on the SAME Sprite.
    const dyn = this.attachBuildingDynamics(node.container, b, built);

    // Update the node record in place (identity preserved). `node.building` was
    // already re-pointed at the top of this method (handlers read it at fire time).
    node.kitAnims = dyn.anims;
    node.label = dyn.label;
    node.labelDepth = built.depth;
    node.pennant = dyn.pennant;
    node.disaster = dyn.disaster;
    node.investigation = dyn.investigation;
    // Sin/disaster state may have changed → re-evaluate hero promotions.
    this.heroPromoDirty = true;
    node.hitRadius = built.hw;

    // Keep the path→fileId index correct (filePath could change with a rename,
    // though coords-unchanged usually means same path). Re-point to this fileId.
    this.fileIdByPath.set(PolisRenderer.normalizeRelPath(b.filePath), b.fileId);

    // Re-add to the animated pool iff the new dynamics animate.
    if (dyn.anims.length > 0) this.animatedNodes.push(node);
    return node;
  }

  /**
   * Destroy ONE building node and remove it from every structure (chunk, node
   * map, animated/smoke lists). The inverse of `createBuildingNode`. Used by the
   * live diff for CHANGED (then rebuilt) and REMOVED buildings.
   *
   * The animated/smoke pool (`animatedNodes`, the list the per-step ambient clock
   * walks to recycle smoke/flame/flag particles) is maintained INCREMENTALLY, not
   * via any batch rebuild: this method splices the destroyed node out of
   * `animatedNodes`, and `createBuildingNode` pushes a rebuilt node back in iff it
   * has >=1 kit anim instance. So after a diff the pool is already correct with no
   * separate pass.
   */
  private destroyBuildingNode(node: BuildingNode): void {
    // Remember the chunk so we can drop it if it becomes empty.
    const chunk = this.chunks.get(node.chunkKey);
    // L2: if this node is mid grow/pop-in transition, drop it from the GrowthFx
    // queue first so it can't mutate (or read) a destroyed container next tick.
    this.growthFx.cancelTransition(node.container);
    try {
      // Remove the building SPRITE from its chunk and destroy it. children:true
      // tears down its overlays (label, scaffold rig, anim nodes, pennant, disaster/
      // investigation). texture defaults to FALSE so the SHARED per-variant texture
      // is NOT freed — it is owned by the atlas (destroying it would pull the rug
      // out from under every sibling building of the same variant).
      node.container.removeFromParent();
      node.container.destroy({ children: true });
      // The drop-shadow sprite lives on its own layer — destroy it explicitly. Its
      // texture is the SHARED shadow texture (atlas-owned), so again texture:false.
      node.shadow.removeFromParent();
      node.shadow.destroy();
    } finally {
      // ALWAYS untrack the node, even if a Pixi destroy threw above: otherwise a
      // destroyed (or half-destroyed) node would stay in the animated pool and the
      // per-step ambient clock would step a dead container next frame. The kit anim
      // instances (and their Graphics) are children of the display container, so
      // they are torn down with it (no separate disposal needed).
      this.buildingNodes.delete(node.building.fileId);
      // Polis-P5 — drop the path→fileId index entry too (re-added by
      // createBuildingNode on a CHANGED rebuild). Only delete it if it still points
      // at THIS node's fileId, so a rebuild that already re-set it isn't clobbered.
      const relKey = PolisRenderer.normalizeRelPath(node.building.filePath);
      if (this.fileIdByPath.get(relKey) === node.building.fileId) {
        this.fileIdByPath.delete(relKey);
      }
      // Splice the node out of the animated pool so the per-step clock can never
      // step a destroyed container next frame (see `removeFromArrayByIdentity`).
      removeFromArrayByIdentity(this.animatedNodes, node);
      // Drop the chunk container if it is now empty (a rebuilt building re-creates
      // its chunk via addToChunk, so this never orphans a still-needed chunk).
      if (chunk && chunk.container.children.length === 0) {
        chunk.container.removeFromParent();
        chunk.container.destroy();
        this.chunks.delete(node.chunkKey);
      }
    }
  }

  // ---------------------------------------------------------------------------
  // Chunked culling + LOD
  // ---------------------------------------------------------------------------

  private chunkKey(tileX: number, tileY: number): string {
    return `${Math.floor(tileX / CHUNK_SIZE)},${Math.floor(tileY / CHUNK_SIZE)}`;
  }

  private addToChunk(tileX: number, tileY: number, child: Container): void {
    const key = this.chunkKey(tileX, tileY);
    let chunk = this.chunks.get(key);
    if (!chunk) {
      const container = new Container();
      container.sortableChildren = true;
      this.layers.buildings.addChild(container);
      chunk = {
        container,
        bounds: this.computeChunkBounds(key),
        visible: true,
      };
      this.chunks.set(key, chunk);
    }
    chunk.container.addChild(child);
  }

  private computeChunkBounds(key: string): Rectangle {
    const [cx, cy] = key.split(",").map(Number);
    const a = cartToIso(cx * CHUNK_SIZE, cy * CHUNK_SIZE);
    const b = cartToIso((cx + 1) * CHUNK_SIZE, (cy + 1) * CHUNK_SIZE);
    const c = cartToIso((cx + 1) * CHUNK_SIZE, cy * CHUNK_SIZE);
    const d = cartToIso(cx * CHUNK_SIZE, (cy + 1) * CHUNK_SIZE);
    const minX = Math.min(a.x, b.x, c.x, d.x) - 96;
    const maxX = Math.max(a.x, b.x, c.x, d.x) + 96;
    const minY = Math.min(a.y, b.y, c.y, d.y) - 380; // headroom: tallest kit building (mnemeion lighthouse ≈ 340px) — avoids top-edge pop-cull
    const maxY = Math.max(a.y, b.y, c.y, d.y) + 96;
    return new Rectangle(minX, minY, maxX - minX, maxY - minY);
  }

  private updateCulling(): void {
    if (this.destroyed) return;
    // Gate the O(chunks+buildings) sweep on actual camera/scene/size changes.
    // The ambient STEP animation (smoke/flames/flags) is driven separately in
    // update() off the step clock, so it keeps running while this is parked.
    if (!this.cullDirty) return;
    this.cullDirty = false;

    // Reuse a single Rectangle for the visible world bounds. This mirrors
    // pixi-viewport's getVisibleBounds() (= new Rectangle(left, top,
    // worldScreenWidth, worldScreenHeight)) but allocation-free.
    const view = this.viewBounds;
    view.x = this.viewport.left;
    view.y = this.viewport.top;
    view.width = this.viewport.worldScreenWidth;
    view.height = this.viewport.worldScreenHeight;

    for (const chunk of this.chunks.values()) {
      const vis = view.intersects(chunk.bounds);
      chunk.container.visible = vis;
      chunk.visible = vis;
    }

    // Terrain water chunks: same cull, so off-screen sea/river geometry is hidden
    // AND its shimmer is skipped per frame (the update loop gates on `visible`).
    for (const tc of this.terrainChunks) {
      const vis = view.intersects(tc.bounds);
      tc.chunk.container.visible = vis;
      tc.visible = vis;
    }

    // LOD: only react when the zoom level changes meaningfully.
    const scale = this.viewport.scale.x;
    if (Math.abs(scale - this.lastScale) > 0.02) {
      this.lastScale = scale;
      // LABEL LOD with HYSTERESIS (dead-band). `createLabels` only at/above the IN
      // threshold, `destroyLabels` only below the OUT threshold; between OUT and IN
      // neither fires, so the existing label state is HELD and zoom oscillating
      // around the band stops thrashing ~879 Text allocs/frees per crossing.
      const createLabels = scale >= this.lodLabelsIn;
      const destroyLabels = scale < this.lodLabelsOut;
      const showDetails = scale >= this.lodDetails;
      const showLivery = scale >= LOD_LIVERY;
      const showDisaster = scale >= LOD_DISASTER;
      for (const node of this.buildingNodes.values()) {
        // LABEL — ATTACH-ON-DEMAND with hysteresis. At/above LOD_LABELS_IN: create
        // the Text + parent it (topmost child) if absent. Below LOD_LABELS_OUT:
        // DESTROY + detach it so a far view of a large city retains zero label
        // glyphs. In the dead-band between OUT and IN, do nothing (hold state).
        if (createLabels) {
          if (!node.label) {
            node.label = this.makeLabel(node.building, node.labelDepth);
            node.container.addChild(node.label);
          }
          // P3.2 — hide label when ghosted (filter-aware), even after creation
          if (node.label) node.label.visible = this.labelVisible(node.building.fileId, true);
        } else if (destroyLabels && node.label) {
          node.label.removeFromParent();
          node.label.destroy();
          node.label = null;
        } else if (node.label) {
          // P3.2 — in the dead-band, still respect filter (ghosted → hide)
          node.label.visible = this.labelVisible(node.building.fileId, true);
        }
        // TECH LIVERY: hide provider pennants in the far view (a static toggle,
        // allocation-free). Never touched for buildings with no provider (null).
        if (node.pennant) node.pennant.visible = showLivery;
        // ON-MAP DISASTER: hide burning overlays in the far view so the overview
        // isn't a field of tiny flames; when hidden, `Disaster.update` also
        // early-returns so the kit fire/smoke redraw is skipped per step. Never
        // touched for buildings with no sins (null). Allocation-free toggle.
        // P3.2 — filter-aware: ghosted/effects-hidden buildings suppress disaster.
        if (node.disaster) {
          node.disaster.node.visible = this.effectVisible(node, node.building.fileId, showDisaster);
        }
        // ON-MAP INVESTIGATION (P3): same LOD band as the disaster — hide the
        // suspect smoke + "?" in the far view (when hidden `Investigation.update`
        // early-returns, skipping the kit smoke redraw). Never touched for buildings
        // that are not a bug suspect (null). Allocation-free toggle.
        if (node.investigation) {
          node.investigation.node.visible = this.effectVisible(node, node.building.fileId, showDisaster);
        }
        // Below LOD_DETAILS, fade the fine detail (the kit's textured faces /
        // props read as one mass) by dropping the whole display alpha slightly.
        // P3.2 — filter-aware: ghosted stays at 0.15 regardless of LOD.
        const lodAlpha = showDetails ? 1 : 0.92;
        node.container.alpha = this.targetBuildingAlpha(node.building.fileId, lodAlpha);
      }
      // Road LOD: reveal/fade the faint minor-lane layer by zoom (trunks always
      // shown). Allocation-free — toggles the prebuilt sub-container only.
      this.applyRoadLod(scale);
      this.agentLayer.setLodVisible(scale >= this.lodAgents);
      this.ambientLayer.setLodVisible(scale >= this.lodAgents);
      // Trade-route porters are ZOOM-IN ONLY: shown/animated only at/above
      // TRADE_LOD_ZOOM (~0.45, above the agent/ambient band). Below it the whole
      // layer is hidden and roads render exactly as they do today (no zoom-out
      // flow). The per-step visible-chunk cull still gates which porters draw.
      this.tradeRouteLayer.setLodVisible(scale >= TRADE_LOD_ZOOM);
      // External cloud outposts: hidden in the far view (same band as agents) so
      // the seaward margin doesn't speckle the zoomed-out overview.
      this.externalLayer.setLodVisible(scale >= LOD_EXTERNAL);
      // Fields (farmland): hidden below LOD_FIELDS so the far view is clean.
      if (this.fieldsGraphics) this.fieldsGraphics.visible = scale >= LOD_FIELDS;
      // T6a — terrain grid: sub-pixel below zoom 0.5, hide to save ~557 draw calls.
      if (this.terrainGridGraphics) this.terrainGridGraphics.visible = scale >= 0.5;
    }
  }

  // ---------------------------------------------------------------------------
  // Selection ring
  // ---------------------------------------------------------------------------

  private drawSelectionRing(): void {
    this.selectionRing.clear();
    if (!this.selectedId) {
      this.selectionRing.visible = false;
      return;
    }
    const node = this.buildingNodes.get(this.selectedId);
    if (!node) {
      this.selectionRing.visible = false;
      return;
    }
    // F7 — ghosted buildings get a faded ring (0.15 alpha) instead of full.
    const verdict = nodeFilterVerdict(this.selectedId, this.filterSets);
    const ringAlpha = verdict.ghosted ? 0.15 : 0.95;
    const goldAlpha = verdict.ghosted ? 0.09 : 0.6;
    const r = node.hitRadius * 0.85;
    this.selectionRing
      .ellipse(node.iso.x, node.iso.y + 4, r, r * 0.5)
      .stroke({ color: PALETTE.terracotta, alpha: ringAlpha, width: 3 });
    // Inner gold accent ring for a richer selection read.
    this.selectionRing
      .ellipse(node.iso.x, node.iso.y + 4, r * 0.78, r * 0.39)
      .stroke({ color: PALETTE.goldAccent, alpha: goldAlpha, width: 1.5 });
    this.selectionRing.visible = true;
  }

  // ---------------------------------------------------------------------------
  // Vignette (screen-space)
  // ---------------------------------------------------------------------------

  private drawVignette(): void {
    const w = this.app.screen.width;
    const h = this.app.screen.height;
    if (w <= 0 || h <= 0) return;
    this.vignette.clear();
    const cx = w / 2;
    const cy = h / 2;
    const outer = Math.hypot(cx, cy);
    // Coordinates are in screen pixels, so the gradient must be 'global' space.
    // Inner circle (transparent core) -> outer circle (warm dark edge). The edge
    // color is PALETTE.shadow so the vignette stays in the warm theme.
    const grad = new FillGradient({
      type: "radial",
      center: { x: cx, y: cy },
      innerRadius: outer * 0.62,
      outerCenter: { x: cx, y: cy },
      outerRadius: outer,
      textureSpace: "global",
      colorStops: [
        { offset: 0, color: { r: 0, g: 0, b: 0, a: 0 } },
        {
          offset: 1,
          color: {
            r: (PALETTE.shadow >> 16) & 0xff,
            g: (PALETTE.shadow >> 8) & 0xff,
            b: PALETTE.shadow & 0xff,
            a: ALPHA.vignette,
          },
        },
      ],
    });
    this.vignette.rect(0, 0, w, h).fill(grad);
  }

  // ---------------------------------------------------------------------------
  // Day cycle (screen-space warmth)
  // ---------------------------------------------------------------------------

  /** (Re)build the day-cycle rect geometry to cover the whole screen. A solid
   *  WHITE fill so the per-step `tint` multiplies to the target warm hue and
   *  `alpha` controls intensity — both set in applyDayCycle(), allocation-free.
   *  Called once at construction and on every resize (mirrors drawVignette). */
  private drawDayCycle(): void {
    const w = this.app.screen.width;
    const h = this.app.screen.height;
    if (w <= 0 || h <= 0) return;
    this.dayCycle.clear();
    this.dayCycle.rect(0, 0, w, h).fill({ color: 0xffffff });
    this.dayCycleDrawn = true;
  }

  /** Recolor the day-cycle overlay for the current elapsed time. Lerps tint +
   *  alpha between the noon and evening poles on a triangle wave over
   *  DAY_CYCLE_MS (noon → evening at the half, back to noon at the end). Pure
   *  mutation of `tint`/`alpha` on the prebuilt rect — NO allocation, NO clear.
   *  Visual only; not persisted and determinism is not required. */
  private applyDayCycle(): void {
    // Triangle wave 0→1→0 across the loop: phase in [0,1), folded at the half.
    const phase = (this.dayElapsedMs % DAY_CYCLE_MS) / DAY_CYCLE_MS;
    const k = phase < 0.5 ? phase * 2 : (1 - phase) * 2; // 0 at noon, 1 at dusk
    this.dayCycle.tint = blend(DAY_TINT_NOON, DAY_TINT_EVENING, k);
    this.dayCycle.alpha = DAY_ALPHA_NOON + (DAY_ALPHA_EVENING - DAY_ALPHA_NOON) * k;
    this.dayPhase = k; // P5.1 — exposed for halos + shadow skew
  }

  // ---------------------------------------------------------------------------
  // Lifecycle
  // ---------------------------------------------------------------------------

  // ---------------------------------------------------------------------------
  // P5.1 — HALOS
  // ---------------------------------------------------------------------------

  /** Create the shared 256px radial-gradient halo texture (additive blend). */
  private makeHaloTexture(): import('pixi.js').Texture | null {
    try {
      const size = 256;
      const g = new Graphics();
      // Radial gradient: white core fading to transparent edge.
      // We approximate with concentric circles of decreasing alpha.
      for (let i = size / 2; i > 0; i -= 4) {
        const t = i / (size / 2);
        const a = (1 - t) * 0.5; // alpha from 0.5 at center to 0 at edge
        g.circle(size / 2, size / 2, i).fill({ color: 0xffffff, alpha: a * a });
      }
      const container = new Container();
      container.addChild(g);
      const tex = this.app.renderer.generateTexture({ target: container, resolution: 1, antialias: false });
      g.destroy();
      container.destroy({ children: false });
      return tex;
    } catch {
      return null;
    }
  }

  /**
   * P5.1 — Pooled halo update. Sprites are created/destroyed alongside crowd
   * fires (in syncCrowdFires); per-tick we ONLY mutate alpha/width/height/position
   * of existing pool entries. No per-tick allocation, no removeChildren storm.
   */
  private updateHalos(_frame: number): void {
    if (!this.haloTex) return;
    const rung = this.effectsBudget.rung;
    const flickerFreeze = rung >= 2;
    const tileSize = 64;
    const scale = this.viewport.scale.x;

    for (const [fileId, sprite] of this.haloSprites) {
      const node = this.buildingNodes.get(fileId);
      if (!node) continue;
      const chunk = this.chunks.get(node.chunkKey);
      if (!(chunk?.visible ?? false)) {
        sprite.visible = false;
        continue;
      }
      sprite.visible = true;

      const sev = (worstSinSeverity(node.building) ?? "smoke") as FireSeverity;
      const baseRadius = sev === "inferno" ? 6 : sev === "fire" ? 4 : 2.5;
      const baseAlpha = sev === "inferno" ? 0.22 : sev === "fire" ? 0.16 : 0.10;

      const cf = this.crowdFires.get(fileId);
      const phase = cf ? cf.phase : seededPhaseFromId(fileId);
      let flicker = 0;
      if (!flickerFreeze) {
        flicker = ((phase % 1) * 2 - 1) * 0.04;
      }
      let alpha = baseAlpha + flicker;
      let radius = baseRadius;

      const k = this.dayPhase;
      if (k > 0.3) {
        const night = Math.min(1, (k - 0.3) / 0.7);
        alpha *= 1 + night * 0.8;
        radius *= 1 + night * 0.2;
      }

      const r = radius * tileSize * scale;
      sprite.position.set(node.iso.x, node.iso.y - 12);
      sprite.width = r * 2;
      sprite.height = r * 2;
      sprite.alpha = Math.max(0, Math.min(1, alpha));
    }
  }

  // ---------------------------------------------------------------------------
  // P5.1 — FIRE PROMOTION
  // ---------------------------------------------------------------------------

  /** Sync crowd fires with current building disaster state. No allocation in
   *  steady state (creates/destroys only when sins change, which is rare). */
  private syncCrowdFires(): void {
    if (!this.fireAtlas) return;
    // F6 — Track which buildings have a sin (STATE, not visibility).
    // Decoupled from node.disaster.node.visible so filters never tear down pools.
    const currentBurning = new Set<string>();
    for (const [fileId, node] of this.buildingNodes) {
      if (node.disaster && worstSinSeverity(node.building) != null) {
        currentBurning.add(fileId);
      }
    }
    // Remove crowd fires + halo sprites for buildings no longer burning.
    for (const [fileId] of this.crowdFires) {
      if (!currentBurning.has(fileId)) {
        const cf = this.crowdFires.get(fileId)!;
        cf.fireSprite.removeFromParent();
        cf.smokeSprite.removeFromParent();
        cf.fireSprite.destroy();
        cf.smokeSprite.destroy();
        this.crowdFires.delete(fileId);
        // Destroy pooled halo sprite
        const hs = this.haloSprites.get(fileId);
        if (hs) {
          hs.removeFromParent();
          hs.destroy();
          this.haloSprites.delete(fileId);
        }
        // Re-enable legacy Flame/Smoke for this building
        const node = this.buildingNodes.get(fileId);
        if (node?.disaster) node.disaster.setLegacyVisible(true);
        // Extinguished fire → re-evaluate hero promotions
        this.heroPromoDirty = true;
      }
    }
    // Create crowd fires for newly burning buildings.
    for (const fileId of currentBurning) {
      if (!this.crowdFires.has(fileId)) {
        const node = this.buildingNodes.get(fileId);
        if (!node) continue;
        const sev = (worstSinSeverity(node.building) ?? "smoke") as FireSeverity;
        const cf = createCrowdFire(
          this.fireAtlas, fileId, sev,
          node.iso.x, node.iso.y - 20,
        );
        cf.fireSprite.eventMode = "none";
        cf.smokeSprite.eventMode = "none";
        this.layers.effects.addChild(cf.fireSprite);
        this.layers.effects.addChild(cf.smokeSprite);
        this.crowdFires.set(fileId, cf);
        // Pooled halo sprite for this building (created once, mutated per tick).
        if (this.haloTex && !this.haloSprites.has(fileId)) {
          const hs = new Sprite(this.haloTex);
          hs.anchor.set(0.5);
          hs.blendMode = "add";
          hs.eventMode = "none";
          this.layers.halos.addChild(hs);
          this.haloSprites.set(fileId, hs);
        }
        // Suppress legacy Flame/Smoke — crowd fire sprites replace them.
        if (node.disaster) node.disaster.setLegacyVisible(false);
        // Mark hero promo dirty so new fire gets considered
        this.heroPromoDirty = true;
      }
    }
  }

  /** Re-evaluate hero fire promotion set.
  /** Re-evaluate hero fire promotion set. Called on StepClock ticks (not per frame). */
  private reconcileHeroFires(): void {
    if (!this.fireAtlas || this.heroFirePool.length === 0) return;
    if (!this.heroPromoDirty) return;
    this.heroPromoDirty = false;

    // F6 — Defensive sweep: park heroes whose target building is no longer
    // burning (sin STATE, not visibility — filters don't demote heroes).
    for (let i = 0; i < this.heroFirePool.length; i++) {
      const hf = this.heroFirePool[i];
      if (!hf.targetFileId) continue;
      const node = this.buildingNodes.get(hf.targetFileId);
      if (!node || !node.disaster || worstSinSeverity(node.building) == null) {
        beginDemotionCrossfade(hf);
      }
    }

    // Collect on-screen burning buildings
    const candidates: PromotableBuilding[] = [];
    const cx = this.viewport.center.x;
    const cy = this.viewport.center.y;

    // F6 — Collect on-screen burning buildings by sin STATE (not visibility).
    for (const [fileId, node] of this.buildingNodes) {
      const disaster = node.disaster;
      if (!disaster || worstSinSeverity(node.building) == null) continue;
      const chunk = this.chunks.get(node.chunkKey);
      if (!(chunk?.visible ?? false)) continue;

      const sev = (worstSinSeverity(node.building) ?? "smoke") as FireSeverity;
      const dist = Math.hypot(node.iso.x - cx, node.iso.y - cy);
      candidates.push({ fileId, severity: sev, distToCenter: dist });
    }

    const ranked = rankForPromotion(candidates);
    const maxHeroes = this.profile.maxHeroFires;
    const promoteSet = new Set(ranked.slice(0, maxHeroes).map(b => b.fileId));

    // Assign hero fires to promoted buildings, demote the rest
    const currentAssignments = new Map<string, number>(); // fileId → pool index
    for (let i = 0; i < this.heroFirePool.length; i++) {
      const hf = this.heroFirePool[i];
      if (hf.targetFileId) currentAssignments.set(hf.targetFileId, i);
    }

    // Promote new entries
    let nextPoolIdx = 0;
    const usedPoolIndices = new Set<number>();

    for (const b of ranked) {
      if (!promoteSet.has(b.fileId)) break; // no more promotion slots

      // Already has a hero fire?
      const existingIdx = currentAssignments.get(b.fileId);
      if (existingIdx !== undefined) {
        usedPoolIndices.add(existingIdx);
        continue;
      }

      // Find a free pool slot
      while (usedPoolIndices.has(nextPoolIdx) && nextPoolIdx < this.heroFirePool.length) {
        nextPoolIdx++;
      }
      if (nextPoolIdx >= this.heroFirePool.length) break;

      const node = this.buildingNodes.get(b.fileId);
      if (!node) continue;

      const hf = this.heroFirePool[nextPoolIdx];
      retargetHeroFire(hf, b.fileId, b.severity, node.iso.x, node.iso.y - 20);
      usedPoolIndices.add(nextPoolIdx);
      currentAssignments.set(b.fileId, nextPoolIdx);
    }

    // Demote un-promoted hero fires
    for (let i = 0; i < this.heroFirePool.length; i++) {
      const hf = this.heroFirePool[i];
      if (hf.targetFileId && !promoteSet.has(hf.targetFileId)) {
        beginDemotionCrossfade(hf);
      }
    }

    // Demoted heroes auto-park inside stepHeroFire when crossfade reaches 0.
    // No separate completion check needed — demotionComplete is dead.
  }

  // NOTE: seededPhaseFromId — duplicate-safe, same as fire.ts seededPhase.
  // We avoid the import cycle by inlining the hash.

  // ---------------------------------------------------------------------------
  // P5.1 — DEBUG OVERLAY
  // ---------------------------------------------------------------------------

  /** Initialize the debug overlay Text node (hidden by default). */
  private initDebugOverlay(): void {
    // Dev flag: localStorage key 'polisDebugOverlay'
    try {
      if (typeof localStorage !== "undefined" && localStorage.getItem("polisDebugOverlay") !== "1") {
        return;
      }
    } catch { return; }
    
    this.debugOverlay = new Text({
      text: "",
      style: new TextStyle({
        fontFamily: "monospace",
        fontSize: 11,
        fill: 0x00ff88,
        stroke: { color: 0x000000, width: 2 },
        align: "left",
      }),
    });
    this.debugOverlay.eventMode = "none";
    this.debugOverlay.position.set(8, 8);
    this.app.stage.addChild(this.debugOverlay);
  }

  /** Update debug overlay text at ~2Hz. */
  private updateDebugOverlay(deltaMs: number, _effectsMs: number): void {
    if (!this.debugOverlay) return;
    this.debugOverlayTimer += deltaMs;
    if (this.debugOverlayTimer < 500) return;
    this.debugOverlayTimer = 0;

    const fps = Math.round(1000 / Math.max(1, deltaMs));
    const smoothed = Math.round(this.effectsBudget.smoothedCostMs * 100) / 100;
    const rung = this.effectsBudget.rung;
    const heroCount = this.heroFirePool.filter(h => h.targetFileId !== null).length;
    const crowdCount = this.crowdFires.size;

    // TODO(P5.2): walker count — not yet exposed via AmbientLayer
    const walkerCount = 0;

    const culledChunks = [...this.chunks.values()].filter(c => !c.visible).length;
    const totalChunks = this.chunks.size;
    const builtBld = this.buildingNodes.size;
    // Total buildings from lastCity
    const totalBld = this.lastCity?.buildings.length ?? builtBld;

    const rungLabels = ["full", "h→c", "halo", "15fps", "½anim", "pause"];
    this.debugOverlay.text =
      `fps:${fps} fx:${smoothed}ms parts:${heroCount * 50} hero:${heroCount} crowd:${crowdCount}
` +
      `rung:${rung}(${rungLabels[rung] ?? "?"}) cull:${culledChunks}/${totalChunks} bld:${builtBld}/${totalBld} walk:${walkerCount}`;
  }


  private clearScene(): void {
    this.agentLayer.clear();
    this.ambientLayer.clear();
    // Polis-P4 — reset the possession bookkeeping so a city reload doesn't carry
    // stale agent/subagent records (no adopt needed: ambientLayer.clear() above
    // already zeroed the claimedCount — the full-teardown branch of the contract).
    this.possession.clear();
    // Polis-P5 — reset the Censor firefighter presence so a city reload doesn't
    // carry a stale firefighter record (no adopt needed: ambientLayer.clear() below
    // zeroed claimedCount — the full-teardown branch of the contract; agentLayer.
    // clear() already destroyed the external omino). The path→fileId index is
    // rebuilt by createBuildingNode on the next build. The bound project id is kept
    // (a reload of the SAME project re-resolves correctly); the omino id is dropped
    // so a stale id can't drive a freshly-built scene before the next event.
    this.censor.clear();
    this.fileIdByPath.clear();
    this.censorOminoId = null;
    // Trade-route porters: tear down the pool (removeFromParent + destroy inside
    // the layer). Rebuilt by syncTradeRoutes on the next build/road-changed diff.
    this.tradeRouteLayer.clear();
    // External cloud outposts: tear down the pooled nodes (removeFromParent +
    // destroy inside the layer). Rebuilt by setServices on the next build/diff.
    this.externalLayer.clear();
    // L2: park any in-flight bursts + drop node transitions. The pool Graphics
    // are KEPT (reused across rebuilds) — they live on the effects layer, which
    // clearScene does not tear down; dispose() (in destroy) frees them.
    // P5.1 — clear crowd fires + park hero fires.
    for (const [, cf] of this.crowdFires) {
      cf.fireSprite.removeFromParent();
      cf.smokeSprite.removeFromParent();
      cf.fireSprite.destroy();
      cf.smokeSprite.destroy();
    }
    this.crowdFires.clear();
    for (const hf of this.heroFirePool) {
      parkHeroFire(hf);
    }
    this.heroPromoDirty = true;
    this.effectsBudget.reset();
    this._prevBudgetRung = 0;
    // P5.1 — clear halo sprite pool.
    for (const [, hs] of this.haloSprites) {
      hs.removeFromParent();
      hs.destroy();
    }
    this.haloSprites.clear();

    this.growthFx.clear();
    this.roadGraph = null;
    // FIX: reset the live-diff baselines. Leaving these set would let the next
    // setCityState diff against a destroyed/stale city (lastCity) or wrongly skip
    // a roads rebuild (lastRoadSig). Treat the scene as fresh after a clear.
    // setCityState recomputes lastRoadSig on the next build, so this only matters
    // on the stale-after-clear path.
    this.lastCity = null;
    this.lastRoadSig = null;
    this.lastTerrainSig = null;
    this.animatedNodes = [];
    this.animT = 0;
    // Restart the day cycle at "noon" for a freshly built scene (visual only).
    this.dayElapsedMs = 0;
    // Building containers (and their kit anim part nodes) are destroyed via
    // their chunk below; the node map is cleared after, dropping all frame refs.
    for (const chunk of this.chunks.values()) {
      chunk.container.destroy({ children: true });
    }
    this.chunks.clear();
    this.buildingNodes.clear();
    // Terrain layer holds the ground/props Graphics AND the water-frame chunk
    // containers (destroyed with their shimmer children here). Reset the tracking
    // array so the per-frame shimmer tick never touches a destroyed container.
    this.layers.terrain
      .removeChildren()
      .forEach((c) => c.destroy({ children: true }));
    this.terrainChunks = [];
    this.fieldsGraphics = null;
    this.terrainGridGraphics = null;
    this.layers.districts.removeChildren().forEach((c) => c.destroy());
    // Roads are now sub-containers (minor/trunk) each wrapping a Graphics, so
    // destroy children too. Drop the LOD handle.
    this.layers.roads
      .removeChildren()
      .forEach((c) => c.destroy({ children: true }));
    this.roadMinorLayer = null;
    this.layers.shadows
      .removeChildren()
      .forEach((c) => c.destroy({ children: true }));
    this.selectionRing.clear();
    this.selectionRing.visible = false;
    this.lastScale = -1;
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    // Abort any in-flight chunked build so a queued batch can't run against a
    // torn-down scene (it also self-guards on `this.destroyed`).
    this.cancelBuild();
    // FIX 6: drop the remembered progress callback so we never retain a stale
    // closure over the (now unmounting) React tree after destroy.
    this.lastOnProgress = undefined;
    if (this.cullTick) {
      this.app.ticker.remove(this.cullTick);
      this.cullTick = null;
    }
    if (this.onResize) {
      this.app.renderer.off("resize", this.onResize);
      this.onResize = null;
    }
    if (this.onViewportChanged) {
      this.viewport.off("moved", this.onViewportChanged);
      this.viewport.off("zoomed", this.onViewportChanged);
      this.onViewportChanged = null;
    }
    if (this.onBackgroundTap) {
      this.viewport.off("pointertap", this.onBackgroundTap);
      this.onBackgroundTap = null;
    }
    // clearScene() already clears agentLayer + ambientLayer (and the rest of the
    // scene) — do NOT clear them again here or PIXI would double-destroy the same
    // already-destroyed Graphics/Containers.
    this.clearScene();
    // SPRITE-SHEET BUILDINGS — release every shared per-variant building/shadow
    // texture. The atlas OWNS these (a building sprite destroy never frees them),
    // so they must be freed exactly once, here, after clearScene has torn down all
    // the sprites that referenced them.
    this.buildingAtlas.destroy();
    // L2: detach + destroy the growth-effect pool Graphics BEFORE the effects
    // layer is destroyed below (no removeFromParent leak — the L1 audit caught
    // this exact pattern).
    // P5.1 — destroy fire atlas textures + halos.
    if (this.fireAtlas) {
      destroyFireAtlas(this.fireAtlas);
      this.fireAtlas = null;
    }
    if (this.haloTex) {
      this.haloTex.destroy(true);
      this.haloTex = null;
    }
    // P5.1 — remove debug overlay from stage.
    if (this.debugOverlay) {
      this.debugOverlay.removeFromParent();
      this.debugOverlay.destroy();
      this.debugOverlay = null;
    }
    this.growthFx.dispose();
    // Both overlays live directly on app.stage. PIXI v8 `destroy()` does NOT
    // detach a child from its parent, and PolisRenderer.destroy() does NOT stop
    // the PIXI Application — so the ticker keeps rendering and would dereference
    // a destroyed Graphics on the next frame. Remove from the stage FIRST, then
    // destroy.
    this.vignette.removeFromParent();
    this.vignette.destroy();
    // Day-cycle overlay lives on app.stage like the vignette — detach + destroy.
    this.dayCycle.removeFromParent();
    this.dayCycle.destroy();
    for (const layer of Object.values(this.layers)) {
      layer.destroy({ children: true });
    }
  }
}
