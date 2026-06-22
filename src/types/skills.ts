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

/** The skill roles the panel renders, in display order. Mirrors the Rust `KNOWN_ROLES`. */
export type SkillRole = "mini" | "coder" | "design" | "orchestrator";

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

/**
 * Where a skill's content comes from. `bundled` = the app's built-in default; `project` = a
 * `.claude/skills/...` file forked into the repo (overrides the bundled default). The open
 * `string` arm reserves `marketplace-<id>` for when external skill sources land (deferred).
 */
export type SkillSource = "bundled" | "project" | (string & {});

/**
 * One (role × language) persona row. `source` is "project" when a
 * `.claude/skills/<role>/lang-<lang>.md` override exists, else "bundled" (the embedded default).
 * Same cap / `truncated` semantics as `SkillEntry`. Mirrors the Rust `LangEntry`.
 */
export interface LangEntry {
  role: SkillRole;
  lang: string;
  source: SkillSource;
  content: string;
  bytes: number;
  truncated: boolean;
}

/**
 * One installable bundled language persona (Discover tab) — role-agnostic, installed into a chosen
 * role via `skills_save_lang`. Mirrors the Rust `LangCatalogEntry`. The set is DATA-DRIVEN (the
 * backend derives it from the persona bundle), so the UI must render whatever languages it returns
 * — NEVER hardcode the language list (it grows as personas are added to the bundle).
 */
export interface LangCatalogEntry {
  lang: string;
  name: string;
  description: string;
  source: SkillSource;
  body: string;
}

// --- Phase 4: external skill MARKETPLACE (fetch → vet-preview → install) ---

export type RiskSeverity = "Info" | "Warn" | "Danger";

/** One risk the static scanner found in a fetched SKILL.md (mirrors backend skill_vet::RiskFinding). */
export interface RiskFinding {
  code: string;
  severity: RiskSeverity;
  title: string;
  evidence: string;
}

/** What `skills_marketplace_preview` returns: the parsed metadata + a body excerpt + the risk
 *  findings the owner reviews BEFORE confirming an install (mirrors backend MarketplacePreview). */
export interface MarketplacePreview {
  name: string | null;
  description: string | null;
  allowed_tools: string | null;
  body_excerpt: string;
  findings: RiskFinding[];
  worst: RiskSeverity | null;
  source_url: string;
  sha256: string;
}
