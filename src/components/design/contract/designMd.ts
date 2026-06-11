// designMd — PURE helpers for the design.md contract: build a REVIEW-FIRST draft from
// extracted tokens + Oracle chunks, and a byte-aware clamp for prompt injection.
//
// TRUST MODEL: the draft may QUOTE Oracle chunk text (the target's own source). That
// is only safe because the draft is ALWAYS shown to the user in the contract editor
// and written to disk ONLY on an explicit Save — nothing here writes anything. After
// Save it is user-curated content. This module just shapes strings.
//
// PURE: no DOM, no network, no clock, no random — deterministic for a given input.

import type { DesignTokensDoc } from "./extractTokens";
import type { DesignContextChunk } from "../generation/grounding";
import { isDtcgToken } from "../engine/tokens";

/** Hard cap on the generated draft length (chars). The draft is a starting point the
 * user edits, not a corpus — keep it readable and small. */
const MAX_DRAFT_CHARS = 12 * 1024;
/** Byte cap applied to design.md before it is injected into a prompt (W6 / B4). */
const MAX_INJECT_BYTES = 16 * 1024;
/** Number of highest-score chunks we quote as illustrative snippets in the draft. */
const MAX_QUOTED_SNIPPETS = 3;
/** Per-quoted-snippet char budget. */
const MAX_SNIPPET_CHARS = 360;
/** Marker appended when content is truncated by the byte clamp. */
const TRUNCATION_MARKER = "\n\n<!-- …truncated to fit the 16 KiB injection cap -->";

/** Flatten an untrusted label (chunk fileSource) to one safe line. Mirrors the
 * grounding sanitizer: collapse control chars, squeeze whitespace, bound length. */
function sanitizeLabel(raw: string): string {
  const flattened = raw
    // eslint-disable-next-line no-control-regex
    .replace(/[\x00-\x1F\x7F-\x9F]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  const safe = flattened.length > 0 ? flattened : "unknown";
  return safe.length <= 160 ? safe : safe.slice(0, 160) + "…";
}

/** Trim a snippet to a budget; collapse a fenced code fence inside it so a target
 * snippet can never break OUT of the markdown code fence we wrap it in (we strip
 * triple backticks from the quoted text). */
function safeSnippet(text: string, max: number): string {
  const stripped = text.replace(/```/g, "ʼʼʼ").trim();
  if (stripped.length <= max) return stripped;
  return stripped.slice(0, max).trimEnd() + "\n… (truncated)";
}

/** Walk the token doc collecting `prefix.name → $value` rows for tokens of a $type. */
function tokenRows(
  doc: DesignTokensDoc,
  groupKey: string,
): Array<{ name: string; value: string }> {
  const group = (doc as Record<string, unknown>)[groupKey];
  if (group == null || typeof group !== "object" || Array.isArray(group)) return [];
  const rows: Array<{ name: string; value: string }> = [];
  for (const [name, node] of Object.entries(group as Record<string, unknown>)) {
    if (name.startsWith("$")) continue;
    if (isDtcgToken(node)) {
      const v = (node as { $value: unknown }).$value;
      if (typeof v === "string" && v.trim().length > 0) {
        rows.push({ name, value: v.trim() });
      }
    }
  }
  return rows;
}

/**
 * Build a REVIEW-FIRST design.md draft from extracted tokens + the highest-score
 * Oracle chunks. The draft is structured markdown: a provenance header + a REVIEW
 * warning line, a palette/typography/spacing/radii summary of the extracted tokens,
 * then up to 3 short QUOTED snippets (fenced, with their fileSource as caption) so the
 * user can see what the extraction was grounded on. Hard-capped at 12 KiB. PURE.
 */
export function buildDesignMdDraft(
  chunks: DesignContextChunk[],
  tokens: DesignTokensDoc,
): string {
  const parts: string[] = [];
  parts.push("# Design contract (draft)");
  parts.push("");
  parts.push(
    "> Drafted automatically from your target codebase. REVIEW BEFORE SAVE — edit or",
  );
  parts.push(
    "> replace anything below; nothing is written until you click Save.",
  );
  parts.push("");

  const colors = tokenRows(tokens, "color");
  const fonts = tokenRows(tokens, "typography");
  const spacing = tokenRows(tokens, "spacing");
  const radii = tokenRows(tokens, "radius");

  if (colors.length > 0) {
    parts.push("## Palette");
    for (const c of colors) parts.push(`- \`${c.value}\` — color.${c.name}`);
    parts.push("");
  }
  if (fonts.length > 0) {
    parts.push("## Typography");
    for (const f of fonts) parts.push(`- ${f.value} — typography.${f.name}`);
    parts.push("");
  }
  if (spacing.length > 0) {
    parts.push("## Spacing");
    parts.push(
      "- Scale: " + spacing.map((s) => `\`${s.value}\``).join(" · "),
    );
    parts.push("");
  }
  if (radii.length > 0) {
    parts.push("## Radii");
    parts.push("- " + radii.map((r) => `\`${r.value}\``).join(" · "));
    parts.push("");
  }

  if (
    colors.length === 0 &&
    fonts.length === 0 &&
    spacing.length === 0 &&
    radii.length === 0
  ) {
    parts.push(
      "_No design tokens were confidently extracted. Describe the palette, type",
    );
    parts.push("scale, spacing and component conventions you want here._");
    parts.push("");
  }

  // Highest-score chunks, quoted as illustrative grounding (fenced, captioned). We do
  // NOT trust the server ordering blindly — sort by score desc, stable.
  const quotable = chunks
    .filter((c) => c && typeof c.text === "string" && c.text.trim().length > 0)
    .slice()
    .sort((a, b) => (b.score ?? 0) - (a.score ?? 0))
    .slice(0, MAX_QUOTED_SNIPPETS);

  if (quotable.length > 0) {
    parts.push("## Reference snippets (from the target — for context only)");
    parts.push("");
    for (const c of quotable) {
      parts.push(`**${sanitizeLabel(c.fileSource || "unknown")}**`);
      parts.push("```");
      parts.push(safeSnippet(c.text, MAX_SNIPPET_CHARS));
      parts.push("```");
      parts.push("");
    }
  }

  const draft = parts.join("\n").trimEnd() + "\n";
  if (draft.length <= MAX_DRAFT_CHARS) return draft;
  // Char-cap with a clear marker (the draft is editable so a hard cut is acceptable).
  return draft.slice(0, MAX_DRAFT_CHARS).trimEnd() + "\n… (draft truncated)\n";
}

/**
 * Clamp design.md content to {@link MAX_INJECT_BYTES} UTF-8 bytes for prompt injection,
 * cutting on a CHARACTER boundary (never mid-codepoint) and appending a truncation
 * marker when cut. Content already under the cap is returned UNCHANGED. PURE.
 *
 * Multibyte safety: we measure bytes with TextEncoder and walk the string by code
 * UNIT but verify the encoded byte length, so a 4-byte emoji is never split.
 */
export function clampDesignMd(content: string): string {
  const enc = new TextEncoder();
  if (enc.encode(content).length <= MAX_INJECT_BYTES) return content;

  // Reserve room for the marker so the FINAL string still fits the cap.
  const budget = MAX_INJECT_BYTES - enc.encode(TRUNCATION_MARKER).length;

  // Binary search the longest char-prefix whose UTF-8 length <= budget. Slicing by
  // code unit can land between a surrogate pair, so snap back off a lone high surrogate.
  let lo = 0;
  let hi = content.length;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (enc.encode(content.slice(0, mid)).length <= budget) lo = mid;
    else hi = mid - 1;
  }
  let cut = lo;
  // If we cut right after a lone high surrogate, drop it (avoid a split pair).
  if (cut > 0 && cut < content.length) {
    const code = content.charCodeAt(cut - 1);
    if (code >= 0xd800 && code <= 0xdbff) cut -= 1;
  }
  return content.slice(0, cut).trimEnd() + TRUNCATION_MARKER;
}

/** The injection byte cap, exported for tests + callers that want to surface it. */
export const DESIGN_MD_INJECT_BYTES = MAX_INJECT_BYTES;
