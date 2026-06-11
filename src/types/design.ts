// Generative-design module — TypeScript shapes mirroring the Rust on-the-wire
// format from `src-tauri/src/backend/design.rs` (Phase 1a). camelCase EXACTLY
// matches the serde `rename_all = "camelCase"` on those structs, so these are
// the literal payloads crossing the Tauri IPC boundary.
//
// Authority split (LOCKED architecture 1.1): the LLM/markup owns node CONTENT;
// the manifest owns node PLACEMENT (`{x,y,z,w,h,kind}`). `h` is a fixed number
// OR the literal string `"auto"` (hug-contents, the default — see 1.4).

/** Sanitizer profile a node's markup is rendered under. `lowercase` over IPC. */
export type DesignNodeKind = "html" | "svg";

/**
 * Node height: a fixed numeric height (px) OR the literal `"auto"` (hug
 * contents). Mirrors the Rust untagged `NodeHeight` enum exactly — serialized as
 * a bare number or the bare string `"auto"`.
 */
export type DesignNodeHeight = number | "auto";

/** Canvas geometry stored in `project.json`. */
export interface DesignCanvas {
  w: number;
  h: number;
  grid: number;
}

/**
 * `project.json` metadata + the ordered list of top-level node ids. `nodeOrder`
 * is the paint/stacking companion to per-node `z`; both persist so a reload
 * restores stacking exactly.
 */
export interface DesignProjectMeta {
  schemaVersion: number;
  id: string;
  name: string;
  createdAt: string;
  updatedAt: string;
  canvas: DesignCanvas;
  nodeOrder: string[];
}

/** One manifest entry: top-level placement in global canvas coords + size + kind. */
export interface DesignNodePlacement {
  x: number;
  y: number;
  z: number;
  w: number;
  h: DesignNodeHeight;
  kind: DesignNodeKind;
  /**
   * Corner radius (px) applied to the node card. OPTIONAL: absent means the
   * default card radius from the stylesheet. Mirrors the Rust `radius` field
   * (omitted on the wire when `None`).
   */
  radius?: number;
  /** When true, render the card "flat" (no card chrome/shadow). OPTIONAL. */
  flat?: boolean;
  /** When true, the node is hidden on the canvas (layer visibility). OPTIONAL. */
  hidden?: boolean;
  /** Display label for the layers panel / node tag. OPTIONAL. */
  name?: string;
}

/** `manifest.json` — placement-only authority over top-level nodes, keyed by id. */
export interface DesignManifest {
  schemaVersion: number;
  nodes: Record<string, DesignNodePlacement>;
}

/**
 * Full in-memory project handed to / received from the Rust backend: metadata +
 * manifest + the opaque sanitized markup of every node, keyed by id. `warnings`
 * is omitted on the wire when empty (serde `skip_serializing_if`), so it is
 * optional here.
 */
export interface DesignProject {
  meta: DesignProjectMeta;
  manifest: DesignManifest;
  components: Record<string, string>;
  warnings?: string[];
}

/** A node id paired with its resolved on-screen rect, used by hit-test/guides. */
export interface NodeRect {
  id: string;
  x: number;
  y: number;
  w: number;
  /** Resolved numeric height; `"auto"` heights are resolved to a measured px value. */
  h: number;
  z: number;
}

/** A 2D point in canvas coordinates. */
export interface Point {
  x: number;
  y: number;
}

/**
 * One design-project registry entry (Phase 3, management plane). METADATA ONLY —
 * mirrors the Rust `DesignProjectEntry` (camelCase). Lives in config.json under
 * `designProjects`; NEVER holds the authoritative manifest/markup/prompt (the
 * working folder is the only source of truth). `workingFolderPath` is the dedupe
 * key. `lastOpenedAt` drives the recent-first sort.
 */
export interface DesignProjectEntry {
  id: string;
  name: string;
  workingFolderPath: string;
  createdAt: string;
  updatedAt: string;
  lastOpenedAt: string;
  thumbnailPath?: string;
  /**
   * SHA-256 (lowercase hex) of the design.md content the user last APPROVED in the
   * contract editor. Provenance gate (Fix 3): on load the on-disk design.md is re-hashed
   * and only INJECTED into prompts when it matches this value — an out-of-band (agent)
   * edit produces a mismatch and forces a review. Absent on legacy entries / projects
   * with no approved contract. Mirrors the Rust `contract_sha` (omitted on the wire when
   * `None`).
   */
  contractSha?: string;
}

/**
 * Compact Oracle grounding status for a design project's target, returned by the Rust
 * `design_oracle_status` command. Mirrors the Rust `DesignOracleStatus` (camelCase). The
 * command NEVER fails: when the target's index is not ready/empty (or any error occurs)
 * it returns `{ grounded: false }` with the optional fields absent.
 *
 * PRIVACY: `rootLabel` is the LEAF folder name of the resolved grounding root ONLY — never
 * the absolute path (the backend keeps the user's filesystem layout off the IPC boundary).
 */
export interface DesignOracleStatus {
  /** Whether the target has a usable Oracle index (any indexed file/chunk present). */
  grounded: boolean;
  /** Leaf folder name of the grounding root (never an absolute path). OPTIONAL. */
  rootLabel?: string;
  /** Indexed chunk count, when known. OPTIONAL. */
  chunks?: number;
  /** Indexed file count, when known. OPTIONAL. */
  files?: number;
  /** ISO-8601 time of the last completed index job, when known. OPTIONAL. */
  lastSyncIso?: string;
}
