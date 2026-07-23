// Pure, DOM-free badge model for the Censor findings list (Phase E2).
//
// REUSES the `RiskFlags` severity vocab + palette shape
// (`high | medium | low`: cream/terracotta/teal tokens) so the
// findings list reads visually identical to the rest of the app — there is
// intentionally NO critical/info bucket. Category and source get their own small
// neutral badges. Everything here is total and null-safe (an unknown severity /
// category falls back to a conservative style) and renders LABELS ONLY — never a
// path, a raw value, or anything that could carry a secret.

import type {
  CensorCategory,
  CensorSeverity,
  CensorGemmaStatus,
} from "../../types/backend";

/** The Tailwind class bundle for one severity, mirroring RiskFlags `severityConfig`. */
export interface SeverityStyle {
  /** Row tint background. */
  bg: string;
  /** Row border. */
  border: string;
  /** Strong text color. */
  text: string;
  /** Pill background+text for the severity badge. */
  badge: string;
}

// Verbatim palette from RiskFlags.severityConfig (cream/terracotta/teal tokens).
const SEVERITY_STYLES: Record<CensorSeverity, SeverityStyle> = {
  high: {
    bg: "bg-coral/10",
    border: "border-coral/20",
    text: "text-coral-dark",
    badge: "bg-coral/10 text-coral-dark",
  },
  medium: {
    bg: "bg-amber/10",
    border: "border-amber/20",
    text: "text-amber-dark",
    badge: "bg-amber/10 text-amber-dark",
  },
  low: {
    bg: "bg-teal/10",
    border: "border-teal/20",
    text: "text-teal-dark",
    badge: "bg-teal/10 text-teal-dark",
  },
};

/** Style bundle for a severity; falls back to `low` (the calmest) for an unknown
 *  value so a hand-edited / future shard never throws or renders unstyled. */
export function severityStyle(severity: CensorSeverity | string | undefined): SeverityStyle {
  return SEVERITY_STYLES[severity as CensorSeverity] ?? SEVERITY_STYLES.low;
}

/** Rank used to sort findings high → medium → low (and to compare buckets). */
export function severityRank(severity: CensorSeverity | string | undefined): number {
  switch (severity) {
    case "high":
      return 0;
    case "medium":
      return 1;
    case "low":
      return 2;
    default:
      return 3; // unknown sorts last
  }
}

/** Human label for a category badge (kebab → Title Case-ish, short). */
export function categoryLabel(category: CensorCategory | string | undefined): string {
  switch (category) {
    case "security":
      return "Security";
    case "correctness":
      return "Correctness";
    case "complexity":
      return "Complexity";
    case "duplication":
      return "Duplication";
    case "dead-code":
      return "Dead code";
    case "style":
      return "Style";
    default:
      return "Finding";
  }
}

/** Neutral pill classes for a category badge. `security` gets a coral accent (it
 *  is the most important class); the rest share a cream/teal neutral. */
export function categoryBadgeClass(
  category: CensorCategory | string | undefined,
): string {
  if (category === "security") return "bg-coral/10 text-coral-dark";
  return "bg-cream-100 text-cream-600";
}

/** Pill classes for a source badge. `gemma` (the optional AI-review tier) gets a
 *  teal accent; deterministic linters get a neutral cream pill so the two layers
 *  read as visually distinct in the findings list. */
export function sourceBadgeClass(source: string | undefined): string {
  if ((source ?? "").toLowerCase() === "gemma") return "bg-teal/10 text-teal-dark";
  return "bg-cream-100 text-cream-500";
}

/** Human display label for a finding source.
 *  - `"gemma"` (any case) → "AI review" (Layer 2; legacy wire name kept on the backend)
 *  - any other runner (clippy / eslint / ruff / …) → "Linter · {tool}" (Layer 1)
 *  Never a path or value; tool names are capped so a malformed shard cannot blow
 *  out the row. */
export function sourceLabel(source: string | undefined): string {
  const s = (source ?? "").trim();
  if (!s) return "Linter";
  if (s.toLowerCase() === "gemma") return "AI review";
  const tool = s.length > 20 ? `${s.slice(0, 20)}…` : s;
  return `Linter · ${tool}`;
}

/** The compact, user-facing `file:line` reference for a finding. A null line
 *  (file-level finding) renders just the file. Returns the path verbatim — the
 *  file path is the project-relative path the user chose to track, not a secret. */
export function fileLineLabel(
  file: string | undefined,
  line: number | null | undefined,
): string {
  const f = (file ?? "").trim() || "(unknown file)";
  if (typeof line === "number" && Number.isFinite(line) && line > 0) {
    return `${f}:${Math.floor(line)}`;
  }
  return f;
}

/** Calm one-line note when the optional AI-review tier is offline. Driven by
 *  `censor_status.gemmaStatus` (legacy wire name). Render this in the panel so
 *  users see that Layer 1 linters still run without a model. */
export function gemmaStatusNote(status: CensorGemmaStatus | string | undefined): string | null {
  switch (status) {
    case "offline":
      return "AI review offline — deterministic linters still active";
    case "unknown":
      // Not yet probed this session (no watch started). No banner — deterministic
      // linters still run; we just don't claim a state we don't know.
      return null;
    case "available":
    default:
      return null;
  }
}
