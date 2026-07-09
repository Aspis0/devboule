// Polis Map — TypeScript contract.
//
// These interfaces mirror the Rust serde structs in
// `src-tauri/src/polis/model.rs`, which serialize with
// `#[serde(rename_all = "camelCase")]`. Field names here MUST match the
// camelCase JSON exactly (e.g. Rust `file_id` -> `fileId`).
//
// `BuildingPurpose` and `VisualTier` are kept open (`| string`) because the
// design doc says the Oracle may introduce new purposes/tiers at runtime.

export interface Coords {
  x: number;
  y: number;
}

export interface GridSize {
  w: number;
  h: number;
}

export interface Bounds {
  x: number;
  y: number;
  w: number;
  h: number;
}

export type BuildingPurpose =
  | "townhall"
  | "temple"
  | "fortress"
  | "market"
  | "tower"
  | "house"
  | "warehouse"
  | "workshop"
  | "conduit"
  | "baths"
  | "theater"
  | "lighthouse"
  | "harbor"
  | "library"
  | "unknown"
  // Extensible: the Oracle may classify new purposes not listed above.
  | string;

/**
 * Human-facing display label for a `BuildingPurpose` slug, of the form
 * `"English (Greek)"`. The slug is the machine source of truth (the value
 * serialized on `Building.purpose`); this map is a pure presentation helper.
 * Mirrors `purpose_label` in `src-tauri/src/polis/model.rs`.
 */
export const PURPOSE_LABELS: Record<string, string> = {
  temple: "Temple (Naos)",
  market: "Market (Agora)",
  fortress: "Fortress (Phrourion)",
  tower: "Tower (Pyrgos)",
  house: "House (Oikos)",
  warehouse: "Warehouse (Apotheke)",
  workshop: "Workshop (Ergasterion)",
  conduit: "Conduit (Agogos)",
  baths: "Baths (Balaneion)",
  theater: "Theater (Theatron)",
  lighthouse: "Lighthouse (Pharos)",
  harbor: "Harbor (Limen)",
  library: "Library (Bibliotheke)",
  townhall: "Town Hall (Bouleuterion)",
  unknown: "Unclassified",
};

/**
 * Display label for an agent `type` slug, of the form `"English (Greek)"`.
 * Mirrors `agent_type_label` in `src-tauri/src/polis/model.rs`.
 */
export const AGENT_TYPE_LABELS: Record<string, string> = {
  orchestrator: "Orchestrator (Strategos)",
  coder: "Coder (Tekton)",
  verifier: "Verifier (Episkopos)",
  augur: "Augur (Mantis)",
};

/**
 * Title-case an unknown slug as a graceful display fallback: split on `_`/`-`,
 * capitalize each word's first letter. e.g. `"object_store"` -> `"Object Store"`.
 */
function titleCaseSlug(slug: string): string {
  return slug
    .split(/[_-]/)
    .filter((w) => w.length > 0)
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(" ");
}

/**
 * Display label for a `BuildingPurpose` slug. Known slugs map to their
 * `"English (Greek)"` label; unknown / Oracle-introduced slugs fall back to a
 * Title-cased slug (no Greek) so the registry stays extensible.
 */
export function purposeLabel(slug: string): string {
  return PURPOSE_LABELS[slug] ?? titleCaseSlug(slug);
}

/**
 * Display label for an agent `type` slug. Unknown slugs fall back to a
 * Title-cased slug (no Greek).
 */
export function agentTypeLabel(slug: string): string {
  return AGENT_TYPE_LABELS[slug] ?? titleCaseSlug(slug);
}

export type VisualTier =
  | "kalybe" // 0–200 lines (hut)
  | "oikia" // 201–600 lines (house)
  | "synoikia" // 601–1200 lines (tenement)
  | "megaron" // 1201–2500 lines (hall)
  | "mnemeion" // > 2500 lines (monument)
  | string;

/**
 * Provenance of a Building's `purpose` — lets the UI render GROUNDED verdicts
 * differently from GUESSES.
 *
 * Grounded (verified from a real signal): "oracle" | "entrypoint" |
 * "extension" | "directory" | "graph".
 * Guesses (mark visually as uncertain): "heuristic" (low-confidence filename
 * keyword) | "default" (nothing matched — honestly unclassified).
 */
export type PurposeSource =
  | "oracle"
  | "entrypoint"
  | "extension"
  | "directory"
  | "graph"
  | "heuristic"
  | "default"
  | string;

export type BuildingStatus = "normal" | "burning" | "active" | "offline";

export type DistrictType = "commons" | "feature" | "external" | string;

/**
 * Provenance of a Building's / Feature's `featureId` (Polis F1 + F2):
 *   - "directory" — resolved from the file's directory spine (F1).
 *   - "commons"   — routed to the shared cross-cutting feature (F1).
 *   - "default"   — no resolvable spine, the root feature (F1).
 *   - "oracle"    — the feature was NAMED/DESCRIBED or is a MERGE TARGET set by the
 *                   Oracle via the explicit re-classify action (F2). The label and
 *                   any description are the Oracle's; the structural identity is
 *                   still grounded in F1.
 */
export type FeatureSource =
  | "directory"
  | "commons"
  | "default"
  | "oracle"
  | string;

/**
 * What KIND of feature this is (Polis F1). Mirrors the Rust `FeatureKind` enum
 * (serialized camelCase): `domain` | `commons` | `external`.
 */
export type FeatureKind = "domain" | "commons" | "external" | string;

/**
 * A product/domain area (Polis F1 structural assignment, optionally NAMED +
 * DESCRIBED by the Oracle in F2). Buildings reference it by `id` via
 * `Building.featureId`. Mirrors the Rust `Feature` struct (camelCase serde).
 */
export interface Feature {
  id: string;
  /** Human label: the Oracle's name (F2) when re-classified, else the F1 label. */
  label: string;
  /** One-line description from the Oracle (F2). Empty until re-classified. */
  description: string;
  colorAccent: string;
  kind: FeatureKind;
}

export type WallStyle = "roman_wall" | "aqueduct" | "palisade" | "none" | string;

export type RoadType = "import" | "semantic" | "infrastructure";

export type RoadStyle = "terra_battuta" | "lastricata" | "acquedotto";

export type AgentType =
  | "orchestrator"
  | "coder"
  | "verifier"
  | "augur"
  | string;

export type AgentStatus =
  | "idle"
  | "walking"
  | "working"
  | "reviewing"
  | "surveying";

export type ExternalProvider = "scaleway" | "cloudflare" | string;

export type ExternalServiceType =
  | "container"
  | "gpu_vm"
  | "cpu_vm"
  | "object_store"
  | "llm_api"
  | "worker"
  | string;

export type ExternalServiceStatus =
  | "running"
  | "stopped"
  | "spawning"
  | "error"
  | string;

export type SinSeverity = "smoke" | "fire" | "inferno";

export interface UrbanSin {
  sinId: string;
  severity: SinSeverity;
  /** Sidebar text. Never contains secret values (redacted server-side). */
  description: string;
  autoDetectable: boolean;
  fileId?: string;
}

/**
 * Ledger record for a detected sin (Augure P1.1–P1.3). Mirrors the Rust
 * `SinRecordWire` struct (camelCase serde). The UI drives the parchment
 * anomaly section from these records — not from `Building.sins` (which is
 * the visual-layer subset, open sins only, no ruleId/evidence).
 */
export interface SinRecord {
  id: string;
  relPath: string;
  ruleId: string;
  line: number | null;
  severity: SinSeverity;
  description: string;
  evidence: string;
  disposition: "open" | "ignored" | "fixed";
  createdAt: string;
  updatedAt: string;
  fixDirectiveId: string | null;
}

export interface District {
  districtId: string;
  name: string;
  type: DistrictType;
  bounds: Bounds;
  wallStyle: WallStyle;
  colorAccent: string;
}

export interface Building {
  fileId: string;
  filePath: string;
  districtId: string;
  purpose: BuildingPurpose;
  /**
   * Which rule decided `purpose`. Grounded sources are verified; "heuristic"
   * and "default" are guesses the UI should render differently. See
   * `PurposeSource` and `src-tauri/src/polis/scanner.rs::classify_purpose_grounded`.
   */
  purposeSource: PurposeSource;
  /**
   * Polis F1 — which `Feature` (product/domain area) this building belongs to;
   * references a `CityState.features[].id`. After an F2 Oracle re-classify this is
   * the CANONICAL feature id (merged cross-tree features collapse to one). Empty
   * on a pre-F1 city.
   */
  featureId?: string;
  /**
   * Provenance of `featureId`: "directory" | "commons" | "default" (F1) or
   * "oracle" (F2 — the feature was Oracle-named or a merge target). Empty on a
   * pre-F1 city. See `FeatureSource`.
   */
  featureSource?: FeatureSource;
  /**
   * TECH LIVERY (Polis F4) — the 3rd orthogonal visual channel: which cloud
   * provider this file is tied to ("cloudflare" | "scaleway"), or absent for
   * pure local code (the common case). DERIVED each scan in Rust from path +
   * import/config signals (never persisted). The renderer draws a small roof
   * pennant + tint per provider; absent → no livery. Mirrors `ExternalProvider`.
   */
  provider?: ExternalProvider;
  linesOfCode: number;
  visualTier: VisualTier;
  coords: Coords;
  status: BuildingStatus;
  label: string;
  description: string;
  lastModified: string;
  agentPresent?: string;
  /**
   * Bug-investigation P3 — TRANSIENT "under investigation" marker: the id of an
   * OPEN Kanban card with `category === "bug"` whose Oracle suspect files resolved
   * to this building. Drives the investigative-smoke overlay (blue/violet kit Smoke
   * + a "?" marker), VISUALLY DISTINCT from the confirmed-sin disaster fire.
   * HONESTY: a suspect is Oracle's GUESS, never a confirmed disaster. Recomputed
   * each scan/refresh in Rust (`scanner::attach_suspect_cards`); never persisted —
   * absent on a building no open bug card suspects. Mirrors `agentPresent`.
   */
  suspectOfCardId?: string;
  kanbanCardId?: string;
  untrackedChange?: boolean;
  sins: UrbanSin[];
  notes: string[];
}

export interface Road {
  roadId: string;
  from: string;
  to: string;
  type: RoadType;
  style: RoadStyle;
  weight: number;
  /**
   * Optional WORLD-GRID street polyline (world/tile coords): the ordered
   * corner waypoints the road follows, from the `from` building's cell to the
   * `to` building's cell, routed around building tiles and sharing segments.
   * Computed deterministically in Rust (`grid::route_roads`). When present
   * (>=2 points) the renderer draws the cobbled road along the polyline;
   * when absent it falls back to a straight `from`->`to` line.
   */
  path?: { x: number; y: number }[];
  /**
   * Provenance of this road: "ast" (tree-sitter parse) | "regex" (regex
   * extract from the scanner) | "semantic" (Oracle embedding similarity).
   */
  provenance?: "ast" | "regex" | "semantic";
}

/**
 * Slim per-role subagent projection carried on an {@link Agent}. The Polis
 * walker layer derives one scaled-down omino per `count` for each entry, keyed
 * by `role`. Only `role` + `count` are carried (the renderer needs nothing
 * else). Mirrors the Rust `AgentSubagentBrief`.
 */
export interface AgentSubagentBrief {
  role: string;
  count: number;
}

export interface Agent {
  agentId: string;
  type: AgentType;
  status: AgentStatus;
  currentFileId: string | null;
  currentTask: string | null;
  color: string;
  lastIntervention?: string;
  /**
   * The driving model string (e.g. "MiMo-V2.5", "deepseek-r1", "claude-sonnet").
   * Used by the Polis walker layer to tint the agent tunic by provider family.
   * Absent (omitted) for agents without a model — no-churn serde.
   */
  model?: string | null;
  /**
   * Set when this session is a mini-coder spawned by a parent coder. Its
   * presence selects the mini-coder figure (watercarrier) instead of the plain
   * coder builder. Absent (omitted) for every ordinary agent — no-churn serde.
   */
  parentAgentId?: string;
  /**
   * Per-role subagent breakdown this agent reported. Absent (omitted) when the
   * agent has no subagents — no-churn serde.
   */
  subagents?: AgentSubagentBrief[];
}

export interface ExternalService {
  serviceId: string;
  provider: ExternalProvider;
  type: ExternalServiceType;
  name: string;
  status: ExternalServiceStatus;
  coords: Coords;
  spawnable: boolean;
}

/**
 * An internal river channel: an inclusive column range `[gxMin, gxMax]` running
 * through the land and flowing east into the sea, with sand shores on both
 * banks. Mirrors the Rust `terrain::River`.
 */
export interface River {
  gxMin: number;
  gxMax: number;
}

/**
 * A water tile (sea or river) for the pooled water layer. `deep` selects the
 * darker open-sea shade. Mirrors the Rust `terrain::WaterTile`.
 */
export interface WaterTile {
  gx: number;
  gy: number;
  deep: boolean;
}

/** A terrain tile coordinate (absolute cartesian tile space). */
export interface TerrainTile {
  gx: number;
  gy: number;
}

/**
 * Polis terrain frame (sea + rivers + shores + bridges) surrounding the city.
 * SPARSE — only the non-grass tiles are transmitted; everything else is grass
 * (drawn by the value-noise ground). Mirrors the Rust `terrain::TerrainData`
 * (camelCase serde). Computed AFTER the layout, purely additive.
 *
 *   - `seaX`    : `gx >= seaX` (within `[minY, maxY)`) is open sea — the EAST /
 *                 seaward margin, aligned with the harbour/cloud-outpost column.
 *   - `minY/maxY`: the y-band the sea spans (the land's vertical extent).
 *   - `water`   : every sea + river tile (pooled water layer).
 *   - `sand`    : shore tiles adjacent to sea/river.
 *   - `bridges` : tiles where a routed road crosses a river (walkable deck over water).
 */
export interface TerrainData {
  seaX: number;
  minY: number;
  maxY: number;
  rivers: River[];
  water: WaterTile[];
  sand: TerrainTile[];
  bridges: TerrainTile[];
}

export interface CityState {
  version: number;
  projectName: string;
  era: string;
  generatedAt: string;
  gridSize: GridSize;
  districts: District[];
  buildings: Building[];
  roads: Road[];
  agents: Agent[];
  externalServices: ExternalService[];
  /**
   * Polis F1 feature/product-area registry. Every building's `featureId`
   * references one of these by `id`. Labels/descriptions are the Oracle's after an
   * F2 re-classify, else the deterministic F1 labels. Absent on a pre-F1 city.
   */
  features?: Feature[];
  notes: string[];
  sins: UrbanSin[];
  /** Present only when a scan hit the file-count / size cap. */
  scanNote?: string;
  /**
   * Polis terrain frame (sea + rivers + shores + bridges) around the city.
   * Sparse + additive; absent on a pre-terrain city (treat as no water).
   */
  terrain?: TerrainData;
}

// ---------------------------------------------------------------------------
// P3.2 — Filters
// ---------------------------------------------------------------------------

/** Filter axis that hides sin effects only — never ghosts buildings. */
export interface FilterState {
  /** rule_ids whose EFFECTS are hidden; empty = none hidden */
  categories: string[];
  /** Null = show all severities; "fire" = fire + inferno; "inferno" = inferno only */
  minSeverity: "fire" | "inferno" | null;
  /** Feature/district ids to KEEP; empty = all */
  features: string[];
  /** Simple glob/substring; "" = all */
  pathGlob: string;
  /** "ghost" = alpha 0.15 translucent; "hide" = visible=false */
  mode: "ghost" | "hide";
}
