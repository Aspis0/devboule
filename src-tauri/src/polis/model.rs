//! Polis Map — data model.
//!
//! All structures mirror the TypeScript `CityState` contract from the design
//! doc (`aspis-bio-polis-map.md`). They are serialized with
//! `#[serde(rename_all = "camelCase")]` so the JSON shape matches the
//! TypeScript interfaces in `src/types/city.ts` exactly
//! (e.g. `file_id` -> `fileId`, `lines_of_code` -> `linesOfCode`).
//!
//! `BuildingPurpose`, `VisualTier`, road/agent kinds etc. are kept as plain
//! `String`s where the doc explicitly says the set must stay extensible (the
//! Oracle may invent new purposes at runtime). Helper constants below document
//! the known vocabulary without locking it down.

use serde::{Deserialize, Serialize};

/// Current schema version. Bump when the wire shape changes.
pub const CITY_STATE_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Known BuildingPurpose vocabulary (extensible — see doc).
// Stored as `String` on the wire so Oracle can introduce new types.
// ---------------------------------------------------------------------------

/// Known `BuildingPurpose` values. The field type stays `String` so the Oracle
/// can classify files into new purposes not listed here (per the doc).
///
/// The slug is a STABLE ENGLISH machine key (the source of truth on the wire).
/// The human-facing "English (Greek)" display label is a presentation helper —
/// see [`purpose_label`]; it is NOT serialized onto the `Building`.
pub mod purpose {
    pub const TOWNHALL: &str = "townhall"; // Cloudflare worker entry, critical config
    pub const TEMPLE: &str = "temple"; // Oracle queries, LanceDB, prompt layer
    pub const FORTRESS: &str = "fortress"; // Agent core, orchestrator, dispatcher
    pub const MARKET: &str = "market"; // API clients, external integrations
    pub const TOWER: &str = "tower"; // Config: tauri.conf.json, tsconfig, *.toml
    pub const HOUSE: &str = "house"; // UI components, generic files (honest default)
    pub const WAREHOUSE: &str = "warehouse"; // Object store interface, storage layer
    pub const WORKSHOP: &str = "workshop"; // Scripts, utility, tools
    pub const CONDUIT: &str = "conduit"; // Middleware, proxy, routing layer
    pub const BATHS: &str = "baths"; // Auth, session, token management
    pub const THEATER: &str = "theater"; // Logging, telemetry, monitoring
    pub const LIGHTHOUSE: &str = "lighthouse"; // Entry point: main, index, lib.rs
    pub const HARBOR: &str = "harbor"; // Upload/download, file I/O, stream
    pub const LIBRARY: &str = "library"; // Constants, types, enums, shared interfaces
    /// Honest fallback for unclassified / Oracle-introduced unknown slugs.
    pub const UNKNOWN: &str = "unknown";
}

/// Human-facing display label for a `BuildingPurpose` slug, of the form
/// `"English (Greek)"`. The slug stays the machine source of truth; this is a
/// pure presentation helper (mirrors `PURPOSE_LABELS` in `src/types/city.ts`).
///
/// Unknown / Oracle-introduced slugs fall back to a Title-cased slug (no Greek)
/// so the registry stays extensible without ever panicking.
pub fn purpose_label(slug: &str) -> String {
    match slug {
        "temple" => "Temple (Naos)",
        "market" => "Market (Agora)",
        "fortress" => "Fortress (Phrourion)",
        "tower" => "Tower (Pyrgos)",
        "house" => "House (Oikos)",
        "warehouse" => "Warehouse (Apotheke)",
        "workshop" => "Workshop (Ergasterion)",
        "conduit" => "Conduit (Agogos)",
        "baths" => "Baths (Balaneion)",
        "theater" => "Theater (Theatron)",
        "lighthouse" => "Lighthouse (Pharos)",
        "harbor" => "Harbor (Limen)",
        "library" => "Library (Bibliotheke)",
        "townhall" => "Town Hall (Bouleuterion)",
        "unknown" => "Unclassified",
        other => return title_case_slug(other),
    }
    .to_string()
}

/// Human-facing display label for an agent `type` slug, of the form
/// `"English (Greek)"`. Mirrors `AGENT_TYPE_LABELS` in `src/types/city.ts`.
/// Unknown slugs fall back to a Title-cased slug (no Greek).
pub fn agent_type_label(slug: &str) -> String {
    match slug {
        "orchestrator" => "Orchestrator (Strategos)",
        "coder" => "Coder (Tekton)",
        "verifier" => "Verifier (Episkopos)",
        "augur" => "Augur (Mantis)",
        other => return title_case_slug(other),
    }
    .to_string()
}

/// Title-case an unknown slug as a graceful display fallback: split on `_`/`-`,
/// capitalize each word's first letter. Pure, never panics. e.g.
/// `"object_store"` -> `"Object Store"`.
///
/// `pub(crate)` so the Polis feature-assignment (F1) can humanize a feature key
/// (`"rnaseq"` -> `"Rnaseq"`, `"object-store"` -> `"Object Store"`) with the
/// SAME helper the purpose/agent display labels use — one title-casing rule for
/// the whole module, no drift.
pub(crate) fn title_case_slug(slug: &str) -> String {
    slug.split(['_', '-'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Provenance of a building's `purpose` — lets the UI render grounded verdicts
/// (oracle/entrypoint/extension/directory/graph) differently from *guesses*
/// (heuristic/default). This is the "PURE DATA" honesty marker: anything weaker
/// than a real structural/graph signal is flagged so it is never mistaken for a
/// verified classification. See `scanner::classify_purpose`.
pub mod purpose_source {
    /// Oracle / learned meta override (highest confidence). Supersedes all.
    pub const ORACLE: &str = "oracle";
    /// Detected as a real build entry point from project config (lighthouse).
    pub const ENTRYPOINT: &str = "entrypoint";
    /// Decided by a reliable file extension (e.g. `.toml` -> tower).
    pub const EXTENSION: &str = "extension";
    /// Decided by a directory role in the file's path (strong real signal).
    pub const DIRECTORY: &str = "directory";
    /// Decided by import-graph in/out-degree (computed, real).
    pub const GRAPH: &str = "graph";
    /// LOW-CONFIDENCE filename keyword guess. The UI should mark this as a guess.
    pub const HEURISTIC: &str = "heuristic";
    /// No structural/graph/name signal matched — honestly unclassified (house).
    pub const DEFAULT: &str = "default";
}

/// Known TECH-LIVERY provider slugs (Polis F4). The `Building::provider` field
/// stays `Option<String>` on the wire; these are the known machine keys the
/// scanner derives. `None`/absent = pure local code (the common case).
pub mod provider {
    pub const CLOUDFLARE: &str = "cloudflare";
    pub const SCALEWAY: &str = "scaleway";
}

/// Known `VisualTier` values. Stored as `String` on the wire.
pub mod visual_tier {
    pub const KALYBE: &str = "kalybe"; // 0–200 lines (hut)
    pub const OIKIA: &str = "oikia"; // 201–600 lines (house)
    pub const SYNOIKIA: &str = "synoikia"; // 601–1200 lines (tenement)
    pub const MEGARON: &str = "megaron"; // 1201–2500 lines (hall)
    pub const MNEMEION: &str = "mnemeion"; // > 2500 lines (monument)
}

/// Building lifecycle status.
pub mod building_status {
    pub const NORMAL: &str = "normal";
    pub const BURNING: &str = "burning";
    pub const ACTIVE: &str = "active";
    pub const OFFLINE: &str = "offline";
}

/// Urban sin severities (flame intensity).
pub mod severity {
    pub const SMOKE: &str = "smoke";
    pub const FIRE: &str = "fire";
    pub const INFERNO: &str = "inferno";
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// A 2D point on the cartesian (pre-isometric) tile grid.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Coords {
    pub x: f64,
    pub y: f64,
}

impl Coords {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

impl Eq for Coords {}

/// Grid dimensions (width/height in tiles).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridSize {
    pub w: u32,
    pub h: u32,
}

/// Rectangular bounds in tile space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

// ---------------------------------------------------------------------------
// CityState
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CityState {
    pub version: u32,
    pub project_name: String,
    /// "Alpha", "Beta", etc.
    pub era: String,
    /// ISO 8601 timestamp.
    pub generated_at: String,
    pub grid_size: GridSize,
    pub districts: Vec<District>,
    pub buildings: Vec<Building>,
    pub roads: Vec<Road>,
    pub agents: Vec<Agent>,
    pub external_services: Vec<ExternalService>,
    /// Deterministic feature/product-area registry (Polis F1). Every building's
    /// `feature_id` references one of these by `id`. Computed by the pure
    /// `scanner::assign_features` from the directory spine + import graph and
    /// persisted in `.aspis-meta.json`, so the assignment is STABLE across scans
    /// and survives the Oracle being unavailable. `#[serde(default)]` so a
    /// pre-F1 city (no `features` on the wire) still deserializes.
    ///
    /// A later phase (F3) lays out districts BY FEATURE using this registry; F1
    /// itself does NOT change layout/rendering — it only adds + persists the
    /// assignment. F2 (Oracle) fills `description` and may merge cross-tree
    /// features; F1 keeps each spine separate (see `scanner::assign_features`).
    #[serde(default)]
    pub features: Vec<Feature>,
    /// Free-form per-city notes (project-level log; per-building notes are
    /// appended to the matching building's `notes`).
    #[serde(default)]
    pub notes: Vec<String>,
    /// City-wide sins not attributable to a single building (e.g. a cyclic
    /// import involves several files). Per-building sins also live on
    /// `Building::sins`.
    #[serde(default)]
    pub sins: Vec<UrbanSin>,
    /// Diagnostic note recorded when the scan hit a cap (file count / size).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_note: Option<String>,
    /// Polis terrain frame (sea + rivers + shores + bridges) that surrounds the
    /// city. Computed deterministically by `terrain::build_terrain` from the
    /// laid-out building extent + routed roads, AFTER the layout — it is purely
    /// ADDITIVE decoration and never moves a building or reroutes a road. Sparse
    /// (only the non-grass water/sand/bridge tiles), so a big map stays compact.
    /// `#[serde(default)]` so a pre-terrain city (no `terrain` on the wire) still
    /// deserializes to an empty frame.
    #[serde(default = "crate::polis::terrain::TerrainData::empty")]
    pub terrain: crate::polis::terrain::TerrainData,
}

impl CityState {
    /// An empty city for a freshly-named project/era.
    pub fn empty(project_name: impl Into<String>, era: impl Into<String>) -> Self {
        Self {
            version: CITY_STATE_VERSION,
            project_name: project_name.into(),
            era: era.into(),
            generated_at: String::new(),
            grid_size: GridSize { w: 1, h: 1 },
            districts: Vec::new(),
            buildings: Vec::new(),
            roads: Vec::new(),
            agents: Vec::new(),
            external_services: Vec::new(),
            features: Vec::new(),
            notes: Vec::new(),
            sins: Vec::new(),
            scan_note: None,
            terrain: crate::polis::terrain::TerrainData::empty(),
        }
    }

    /// Mutable lookup of a building by `file_id`.
    pub fn building_mut(&mut self, file_id: &str) -> Option<&mut Building> {
        self.buildings.iter_mut().find(|b| b.file_id == file_id)
    }

    /// Mutable lookup of an agent by `agent_id`.
    pub fn agent_mut(&mut self, agent_id: &str) -> Option<&mut Agent> {
        self.agents.iter_mut().find(|a| a.agent_id == agent_id)
    }

    /// Mutable lookup of an external service by `service_id`.
    pub fn external_service_mut(&mut self, service_id: &str) -> Option<&mut ExternalService> {
        self.external_services
            .iter_mut()
            .find(|s| s.service_id == service_id)
    }
}

// ---------------------------------------------------------------------------
// District
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct District {
    pub district_id: String,
    pub name: String,
    /// "cloudflare_worker" | "scaleway_zone" | "scripts" | "core"
    #[serde(rename = "type")]
    pub district_type: String,
    pub bounds: Bounds,
    /// "roman_wall" | "aqueduct" | "palisade" | "none"
    pub wall_style: String,
    pub color_accent: String,
}

// ---------------------------------------------------------------------------
// Building
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Building {
    /// Stable UUID, persisted in `.aspis-meta.json`.
    pub file_id: String,
    pub file_path: String,
    pub district_id: String,
    /// `BuildingPurpose` — kept as `String` to stay extensible (see doc).
    pub purpose: String,
    /// Provenance of `purpose` (serde `purposeSource`): which rule decided it.
    /// One of `purpose_source::*` — see that module. Grounded sources
    /// ("oracle"/"entrypoint"/"extension"/"directory"/"graph") are verified;
    /// "heuristic"/"default" are guesses the UI should render differently.
    pub purpose_source: String,
    /// Polis F1: which `Feature` (product/domain area) this building belongs to —
    /// references a `CityState::features[].id`. Computed deterministically by
    /// `scanner::assign_features` from the file's directory spine / import graph
    /// and persisted in `.aspis-meta.json` (stable across scans). Independent of
    /// `purpose` (the tech/building TYPE): `purpose` drives the building's look,
    /// `feature_id` will drive its DISTRICT in F3. `#[serde(default)]` -> empty
    /// string on a pre-F1 city.
    #[serde(default)]
    pub feature_id: String,
    /// Provenance of `feature_id`: `"directory"` (resolved from the dir-spine),
    /// `"commons"` (routed to the shared/cross-cutting feature), or `"default"`
    /// (no resolvable spine — root file). Lets the UI/F2 tell a structural
    /// assignment from a fallback. `#[serde(default)]` -> empty on a pre-F1 city.
    #[serde(default)]
    pub feature_source: String,
    /// Polis F4 — TECH LIVERY channel (the 3rd orthogonal visual channel:
    /// district=feature, building-shape=purpose, livery=PROVIDER). Which cloud
    /// provider this file is tied to, DERIVED each scan from path + import/config
    /// signals (`scanner::derive_provider`): `Some("cloudflare")` / `Some("scaleway")`
    /// for files with a conservative provider signal, `None` for pure local code
    /// (the vast majority). The frontend renders a small roof pennant + tint per
    /// provider (LOD-gated). Independent of `purpose`/`feature_id`.
    ///
    /// NOT PERSISTED: unlike `feature_id`/coords this is recomputed every scan from
    /// current content (cheap, always fresh, never a layout input). `#[serde(default,
    /// skip_serializing_if = "Option::is_none")]` so old persisted state still loads
    /// and a `None` building omits the field entirely on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub lines_of_code: u32,
    /// `VisualTier` — kept as `String`; drives size, not quality.
    pub visual_tier: String,
    pub coords: Coords,
    /// "normal" | "burning" | "active" | "offline"
    pub status: String,
    /// Short display name.
    pub label: String,
    /// Generated by Oracle (heuristic placeholder for now).
    pub description: String,
    pub last_modified: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_present: Option<String>,
    /// Bug-investigation P3 — TRANSIENT "under investigation" marker: the id of an
    /// OPEN Kanban card with `category == "bug"` whose Oracle `suspect_file_ids`
    /// resolved to THIS building. Drives the investigative-smoke overlay (blue/violet
    /// kit `Smoke` + a "?" marker), VISUALLY DISTINCT from the confirmed-sin disaster
    /// (orange/red fire). HONESTY: a suspect is Oracle's GUESS, never a confirmed
    /// disaster. Recomputed each scan/refresh by `scanner::attach_suspect_cards` from
    /// the live open bug cards. PERSISTENCE: never written into the live
    /// `.aspis-meta.json`/city state we read back on the next launch — it is
    /// `skip_serializing_if = "Option::is_none"` so a `None` building omits it on the
    /// wire and pre-P3 city JSON still loads to `None`, and the live store is always
    /// repopulated from a fresh scan. The ONE place a SET value reaches disk is the
    /// era archive (`eras/<slug>_snapshot.json`, written by `reset_city_in_place`):
    /// that is a write-only, never-read-back snapshot of the transient state AT
    /// era-close, so it intentionally captures `suspect_of_card_id` exactly as it
    /// captures `agent_present`. No code ever loads an era snapshot into live state.
    /// WORKSPACE ASSUMPTION: the suspect file paths originate from the Oracle INDEX
    /// root and are resolved against buildings from the Polis SCAN root; the two are
    /// configured independently and assumed to be the SAME workspace (by design,
    /// `aspis bio`). If they diverge a suspect may not resolve (or resolve to a
    /// same-named file in another tree) — an accepted limitation with no runtime
    /// path gate (see `backend::projects::gather_open_bug_suspects`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suspect_of_card_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kanban_card_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub untracked_change: Option<bool>,
    /// Urban sins detected on this building by the Augure (see `sins.rs`).
    #[serde(default)]
    pub sins: Vec<UrbanSin>,
    /// Per-building log lines (appended via `append_city_note`).
    #[serde(default)]
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Road
// ---------------------------------------------------------------------------

// `Eq` is derivable because every field is `Eq` (`Coords` provides a manual
// `impl Eq` despite its `f64` fields, and `Option<Vec<Coords>>` follows).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Road {
    pub road_id: String,
    /// `file_id` of the source building.
    pub from: String,
    /// `file_id` of the target building.
    pub to: String,
    /// "import" | "semantic" | "infrastructure"
    #[serde(rename = "type")]
    pub road_type: String,
    /// "terra_battuta" | "lastricata" | "acquedotto"
    pub style: String,
    /// 1–5, visual thickness.
    pub weight: u32,
    /// Optional WORLD-GRID street polyline: the ordered sequence of world/tile
    /// cell-center coords the road follows, from the `from` building's cell to
    /// the `to` building's cell (corner waypoints only — colinear runs are
    /// collapsed). Computed deterministically by `grid::route_roads` so roads
    /// route AROUND building tiles and SHARE segments (an emergent street
    /// network), instead of cutting straight diagonals. `None` means no grid
    /// path was found within budget; consumers fall back to a straight
    /// `from`->`to` line. Additive: existing `from`/`to`/`weight`/etc. are
    /// untouched, so old consumers keep working.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<Coords>>,
}

/// Known road kinds.
pub mod road_type {
    pub const IMPORT: &str = "import";
    pub const SEMANTIC: &str = "semantic";
    pub const INFRASTRUCTURE: &str = "infrastructure";
}

/// Known road styles.
pub mod road_style {
    pub const TERRA_BATTUTA: &str = "terra_battuta";
    pub const LASTRICATA: &str = "lastricata";
    pub const ACQUEDOTTO: &str = "acquedotto";
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

/// DATA-PURITY CONTRACT — agents are REAL or absent, never invented.
///
/// `CityState::agents` is sourced from the real MCP agent live-state file
/// (`projects/.aspis-agents.json`, owned by `backend::agents` — see
/// `AGENTS_STATE_FILE`) by `scanner::attach_agents`, which the
/// `generate_city_state` command runs after the pure scan. An agent appears on
/// the map only if it exists in that state with a real, live session. The PURE
/// scanner core still emits an EMPTY agent list (an empty city is honest;
/// fabricated agents are not) — agents are folded in only at the command layer
/// from the real live state. `current_file_id` is ALWAYS either a real building
/// id from this map or `None` (off-map roster); a position is never fabricated.
///
// POLIS FOLLOW-UP: precise per-file agent location will come from MCP
// `file_path` events later (the current MCP tracks project/task, not file), so
// `attach_agents` resolves to a stable representative building for the agent's
// project subtree for now. Never synthesize agents that are not in the live state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub agent_id: String,
    /// "orchestrator" | "coder" | "verifier" | "augur" (stable English slug).
    /// Display label via `agent_type_label`.
    #[serde(rename = "type")]
    pub agent_type: String,
    /// "idle" | "walking" | "working" | "reviewing" | "surveying"
    pub status: String,
    pub current_file_id: Option<String>,
    pub current_task: Option<String>,
    /// Omino + background-glow color for the 3 visible agents; unused for augur.
    pub color: String,
    /// ISO timestamp of the last augur action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_intervention: Option<String>,
    /// Set when this session is a MINI-CODER spawned by a parent coder
    /// (`AgentSession.parent_agent_id`). The Polis walker layer uses its
    /// presence to pick the mini-coder figure (watercarrier) instead of the
    /// plain coder builder. Additive + skip-if-none so an ordinary agent (and
    /// archived/older city payloads) round-trip byte-identical — the key is
    /// simply absent for every non-mini agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    /// Per-role subagent breakdown this agent reported (projected from
    /// `AgentSession.subagents`). The Polis walker layer derives one scaled-down
    /// omino per `count` for each entry, keyed by `role`. Only `role` + `count`
    /// are carried — the renderer needs nothing else. Additive + skip-if-empty
    /// so an agent with no subagents (and older payloads) round-trip unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<AgentSubagentBrief>,
}

/// Slim per-role subagent projection carried on a Polis [`Agent`].
///
/// The fleet model's `backend::model::AgentSubagent` also carries `label` +
/// `model`; the Polis walker layer only needs `role` + `count` to derive the
/// scaled subagent ominos, so this is a deliberate slim projection (less wire,
/// no leak of model/label into the map payload). `role` is normalized to a
/// non-empty slug at projection time (a subagent with no declared role defaults
/// to "coder", mirroring the Python MCP "" -> "coder" normalization).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSubagentBrief {
    pub role: String,
    pub count: u32,
}

/// Known agent kinds (stable English slugs). Display labels via
/// [`agent_type_label`].
pub mod agent_type {
    pub const ORCHESTRATOR: &str = "orchestrator";
    pub const CODER: &str = "coder";
    pub const VERIFIER: &str = "verifier";
    pub const AUGUR: &str = "augur";
}

/// Known agent statuses.
pub mod agent_status {
    pub const IDLE: &str = "idle";
    pub const WALKING: &str = "walking";
    pub const WORKING: &str = "working";
    pub const REVIEWING: &str = "reviewing";
    pub const SURVEYING: &str = "surveying";
}

// ---------------------------------------------------------------------------
// ExternalService
// ---------------------------------------------------------------------------

/// DATA-PURITY CONTRACT — external services mirror REAL cloud inventory only.
///
/// When the provider seam is wired, the `provider == "scaleway"` /
/// `"cloudflare"` services in `CityState::external_services` MUST be sourced
/// from the real cached provider inventory (`backend::providers::ProviderInventory`,
/// populated from the live Scaleway/Cloudflare APIs). A service appears on the
/// map only if it is present in that inventory. The scanner currently emits an
/// EMPTY service list rather than placeholder infrastructure.
///
/// NOTE: era monuments (`provider == "monument"`) are the one legitimate
/// non-inventory entry — they are derived from real archived `CityState`
/// statistics, not invented (see `commands::reset_city_in_place`).
///
// POLIS FOLLOW-UP: populate scaleway/cloudflare services from the cached
// `ProviderInventory` (containers, VMs, object stores, workers). Never
// synthesize services that are not in the real inventory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalService {
    pub service_id: String,
    /// "scaleway" | "cloudflare"
    pub provider: String,
    /// "container" | "gpu_vm" | "cpu_vm" | "object_store" | "llm_api" | "worker"
    #[serde(rename = "type")]
    pub service_type: String,
    pub name: String,
    /// "running" | "stopped" | "spawning" | "error"
    pub status: String,
    pub coords: Coords,
    pub spawnable: bool,
}

// ---------------------------------------------------------------------------
// UrbanSin — the Augure's findings (see sins.rs)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UrbanSin {
    pub sin_id: String,
    /// "smoke" | "fire" | "inferno" — flame intensity.
    pub severity: String,
    /// Sidebar text. MUST NOT contain secret values (see `sins.rs` redaction).
    pub description: String,
    /// `true` if detectable by pure Rust (no Oracle).
    pub auto_detectable: bool,
    /// `file_id` of the offending building, if attributable to one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Feature — Polis F1 deterministic product/domain area
// ---------------------------------------------------------------------------

/// What KIND of feature this is — drives how F3 will treat its district.
///
/// Serialized camelCase like every other Polis enum-ish value, but as a CLOSED
/// enum (unlike `purpose`/`agent_type` which stay `String` for Oracle
/// extensibility): F1's feature KINDS are a fixed, deterministic vocabulary —
/// `Domain` (a real product/domain area derived from a directory spine),
/// `Commons` (the single cross-cutting shared-infrastructure feature), and
/// `External` (reserved for provider/service-backed areas, populated by a later
/// phase). The HUMAN naming/merging of features is F2 (Oracle); the KIND is a
/// structural fact F1 owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FeatureKind {
    /// A real product/domain area, keyed by a file's directory spine.
    Domain,
    /// The single cross-cutting "commons" area (shared infra: types/utils/lib/…
    /// and import-graph hubs). One per city.
    Commons,
    /// Reserved for provider/service-backed areas (Scaleway/Cloudflare). Not
    /// produced by F1's pure assignment yet — kept so the kind vocabulary is
    /// complete and F3/F5 can introduce them without a wire change.
    External,
}

/// A deterministic product/domain area (Polis F1). Buildings reference it by
/// `id` via `Building::feature_id`. Mirrors the other Polis structs' camelCase
/// serde contract.
///
/// F1 produces this PURELY from structure (directory spine + import graph), with
/// NO LLM:
///   - `id`            = the stable feature KEY (the dir-spine slug, or
///                       `"commons"`/`"root"`). Forward-slash/lowercase-stable.
///   - `label`         = a humanized key via `title_case_slug` (e.g. `"rnaseq"`
///                       -> `"Rnaseq"`). F2 (Oracle) may replace this later.
///   - `description`   = `""` in F1 — F2 fills it.
///   - `color_accent`  = a DETERMINISTIC on-palette color picked by a stable
///                       hash of `id` (see `scanner::feature_color_for_key`).
///   - `kind`          = `Domain` / `Commons` / `External`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Feature {
    pub id: String,
    pub label: String,
    pub description: String,
    pub color_accent: String,
    pub kind: FeatureKind,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn city_state_round_trips_with_camel_case_field_names() {
        let mut city = CityState::empty("Aspis Bio", "Alpha");
        city.generated_at = "2026-05-29T00:00:00Z".into();
        city.buildings.push(Building {
            file_id: "fid-1".into(),
            file_path: "src/main.tsx".into(),
            district_id: "core".into(),
            purpose: purpose::LIGHTHOUSE.into(),
            purpose_source: purpose_source::ENTRYPOINT.into(),
            feature_id: "core".into(),
            feature_source: "directory".into(),
            provider: None,
            lines_of_code: 42,
            visual_tier: visual_tier::KALYBE.into(),
            coords: Coords::new(1.0, 2.0),
            status: building_status::NORMAL.into(),
            label: "main.tsx".into(),
            description: "Entry point".into(),
            last_modified: "2026-05-29T00:00:00Z".into(),
            agent_present: None,
            suspect_of_card_id: None,
            kanban_card_id: None,
            untracked_change: None,
            sins: Vec::new(),
            notes: Vec::new(),
        });

        let json = serde_json::to_string(&city).unwrap();
        // camelCase contract verification (matches TS in src/types/city.ts).
        assert!(json.contains("\"projectName\":\"Aspis Bio\""));
        assert!(json.contains("\"generatedAt\""));
        assert!(json.contains("\"gridSize\""));
        assert!(json.contains("\"fileId\":\"fid-1\""));
        assert!(json.contains("\"filePath\":\"src/main.tsx\""));
        assert!(json.contains("\"linesOfCode\":42"));
        assert!(json.contains("\"visualTier\""));
        assert!(json.contains("\"districtId\":\"core\""));
        // New honesty marker: provenance of the purpose classification.
        assert!(json.contains("\"purposeSource\":\"entrypoint\""));
        // Polis F1: feature assignment fields ride on the building (camelCase).
        assert!(json.contains("\"featureId\":\"core\""));
        assert!(json.contains("\"featureSource\":\"directory\""));

        let back: CityState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, city);
    }

    #[test]
    fn feature_round_trips_with_camel_case_and_kind() {
        let mut city = CityState::empty("Aspis Bio", "Alpha");
        city.features.push(Feature {
            id: "rnaseq".into(),
            label: "Rnaseq".into(),
            description: String::new(),
            color_accent: "#C17A5A".into(),
            kind: FeatureKind::Domain,
        });
        city.features.push(Feature {
            id: "commons".into(),
            label: "Commons".into(),
            description: String::new(),
            color_accent: "#8AAABB".into(),
            kind: FeatureKind::Commons,
        });

        let json = serde_json::to_string(&city).unwrap();
        assert!(json.contains("\"features\":["));
        assert!(json.contains("\"colorAccent\":\"#C17A5A\""));
        // FeatureKind serializes camelCase (Domain -> "domain", Commons ->
        // "commons", External -> "external").
        assert!(json.contains("\"kind\":\"domain\""));
        assert!(json.contains("\"kind\":\"commons\""));

        let back: CityState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, city);
    }

    #[test]
    fn external_feature_kind_serializes_camel_case() {
        let f = Feature {
            id: "scaleway".into(),
            label: "Scaleway".into(),
            description: String::new(),
            color_accent: "#A89880".into(),
            kind: FeatureKind::External,
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("\"kind\":\"external\""));
        let back: Feature = serde_json::from_str(&json).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn pre_f1_city_without_features_still_deserializes() {
        // A city JSON written before F1 (no `features`, no `featureId`/
        // `featureSource` on buildings) must still load — the new fields default.
        let json = r#"{
            "version":1,"projectName":"Old","era":"Alpha","generatedAt":"",
            "gridSize":{"w":1,"h":1},"districts":[],
            "buildings":[{"fileId":"x","filePath":"a.ts","districtId":"core",
                "purpose":"house","purposeSource":"default","linesOfCode":1,
                "visualTier":"kalybe","coords":{"x":0.0,"y":0.0},"status":"normal",
                "label":"a.ts","description":"","lastModified":""}],
            "roads":[],"agents":[],"externalServices":[]
        }"#;
        let city: CityState = serde_json::from_str(json).expect("pre-F1 city must load");
        assert!(
            city.features.is_empty(),
            "missing features -> empty registry"
        );
        assert_eq!(
            city.buildings[0].feature_id, "",
            "missing featureId -> empty"
        );
        assert_eq!(city.buildings[0].feature_source, "");
    }

    #[test]
    fn road_and_agent_use_renamed_type_field() {
        let road = Road {
            road_id: "r1".into(),
            from: "a".into(),
            to: "b".into(),
            road_type: road_type::IMPORT.into(),
            style: road_style::LASTRICATA.into(),
            weight: 3,
            path: None,
        };
        let json = serde_json::to_string(&road).unwrap();
        assert!(json.contains("\"type\":\"import\""));
        assert!(json.contains("\"roadId\":\"r1\""));
        // `path` is omitted from the wire when None (skip_serializing_if).
        assert!(!json.contains("\"path\""));

        let agent = Agent {
            agent_id: "ag1".into(),
            agent_type: agent_type::CODER.into(),
            status: agent_status::IDLE.into(),
            current_file_id: None,
            current_task: None,
            color: "#FFB347".into(),
            last_intervention: None,
            parent_agent_id: None,
            subagents: Vec::new(),
        };
        let json = serde_json::to_string(&agent).unwrap();
        assert!(json.contains("\"type\":\"coder\""));
        assert!(json.contains("\"agentId\":\"ag1\""));
        assert!(json.contains("\"currentFileId\":null"));
    }

    #[test]
    fn road_path_round_trips_as_camel_case_path() {
        let road = Road {
            road_id: "r2".into(),
            from: "a".into(),
            to: "b".into(),
            road_type: road_type::IMPORT.into(),
            style: road_style::LASTRICATA.into(),
            weight: 1,
            path: Some(vec![Coords::new(1.0, 2.0), Coords::new(3.0, 2.0)]),
        };
        let json = serde_json::to_string(&road).unwrap();
        // serde camelCase: the field is `path`, with Coords as {x,y}.
        assert!(json.contains("\"path\":[{\"x\":1.0,\"y\":2.0},{\"x\":3.0,\"y\":2.0}]"));
        let back: Road = serde_json::from_str(&json).unwrap();
        assert_eq!(back, road);
    }

    #[test]
    fn urban_sin_serializes_camel_case() {
        let sin = UrbanSin {
            sin_id: "s1".into(),
            severity: severity::INFERNO.into(),
            description: "Hardcoded secret-like value at line 10".into(),
            auto_detectable: true,
            file_id: Some("fid-1".into()),
        };
        let json = serde_json::to_string(&sin).unwrap();
        assert!(json.contains("\"sinId\":\"s1\""));
        assert!(json.contains("\"autoDetectable\":true"));
        assert!(json.contains("\"fileId\":\"fid-1\""));
    }

    #[test]
    fn purpose_label_maps_known_slugs_and_falls_back() {
        // Known slugs -> "English (Greek)".
        assert_eq!(purpose_label(purpose::TEMPLE), "Temple (Naos)");
        assert_eq!(purpose_label(purpose::LIGHTHOUSE), "Lighthouse (Pharos)");
        assert_eq!(purpose_label(purpose::TOWNHALL), "Town Hall (Bouleuterion)");
        assert_eq!(purpose_label(purpose::HOUSE), "House (Oikos)");
        // The explicit unknown slug is "Unclassified".
        assert_eq!(purpose_label(purpose::UNKNOWN), "Unclassified");
        // Oracle-introduced slug -> Title-cased fallback, no Greek, no panic.
        assert_eq!(purpose_label("object_store"), "Object Store");
        assert_eq!(purpose_label("laboratory"), "Laboratory");
        assert_eq!(purpose_label(""), "");
    }

    #[test]
    fn agent_type_label_maps_known_slugs_and_falls_back() {
        assert_eq!(
            agent_type_label(agent_type::ORCHESTRATOR),
            "Orchestrator (Strategos)"
        );
        assert_eq!(agent_type_label(agent_type::CODER), "Coder (Tekton)");
        assert_eq!(
            agent_type_label(agent_type::VERIFIER),
            "Verifier (Episkopos)"
        );
        assert_eq!(agent_type_label(agent_type::AUGUR), "Augur (Mantis)");
        // Unknown -> Title-cased fallback.
        assert_eq!(agent_type_label("new-role"), "New Role");
    }
}
