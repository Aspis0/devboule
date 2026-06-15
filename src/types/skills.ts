// P10(b) Step 3 — wire shapes for the unified per-project Skills panel.
//
// These MUST mirror the backend (`src-tauri/src/backend/project_skill.rs`)
// camelCase IPC shapes EXACTLY: `SkillEntry` / `CatalogEntry` are
// `#[tauri::command]` return types serialized with `serde(rename_all =
// "camelCase")`, and the cap is the Rust `MAX_SKILL_BYTES` (8 * 1024).
//
// IMPORTANT: the backend caps on BYTE length, not char count. When checking a
// draft against the cap, measure `new TextEncoder().encode(content).length` —
// never `content.length` (multi-byte chars would under-count and the save would
// be rejected by the backend with a confusing error).

/** The three skill roles the panel renders, in display order. */
export type SkillRole = "mini" | "coder" | "design";

/** Byte ceiling for a single SKILL.md. Mirrors the Rust `MAX_SKILL_BYTES`. */
export const MAX_SKILL_BYTES = 8192;

/**
 * One role's editor + toggle state. `content` is the RAW file capped at
 * `MAX_SKILL_BYTES` on a char boundary; `bytes` is its BYTE length; `truncated`
 * means the on-disk file was larger than the cap and `content` is only its head
 * (saving would PERMANENTLY discard the tail — the panel guards this).
 */
export interface SkillEntry {
  role: SkillRole;
  exists: boolean;
  enabled: boolean;
  content: string;
  bytes: number;
  truncated: boolean;
}

/**
 * One bundled, self-authored starter template the owner can install into a role.
 * `body` ships in the binary (never fetched); `sourceUrl` is null for bundled
 * templates (reserved for a future owner-vetted external catalog).
 */
export interface CatalogEntry {
  id: string;
  name: string;
  role: string;
  description: string;
  sourceUrl: string | null;
  body: string;
}
