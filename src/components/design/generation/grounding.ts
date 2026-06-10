// Oracle grounding — PURE formatter (Phase 2 STEP 4).
//
// The differentiator: the design LLM is grounded in the REAL TARGET codebase via
// Oracle. The Rust `design_oracle_context` command retrieves top-K chunks WITH
// TEXT over the target index; this module turns those chunks (plus the target's
// DTCG token names) into a COMPACT, bounded grounding block that is passed as the
// `context` arg of `buildGeneratePrompt`.
//
// PURE: no DOM, no network, no clock, no random. The chunk text is the TARGET's own
// source — it goes ONLY into the prompt sent to the (loopback) provider; it is never
// logged or emitted. This module just shapes the string.

/** One grounding chunk, mirroring the Rust `DesignContextChunk` (camelCase wire). */
export interface DesignContextChunk {
  fileSource: string;
  score: number;
  text: string;
}

/** Tuning for the grounding block size. Kept compact: grounding is a hint, not a
 * dump, and the prompt has its own backend-side byte cap. */
const MAX_CHUNKS = 8;
/** Max characters of source we keep PER chunk (snippet, not the whole file). */
const MAX_SNIPPET_CHARS = 800;
/** Overall character cap for the assembled block (defense against a huge corpus). */
const MAX_BLOCK_CHARS = 6000;

/** Max characters kept of a (sanitized) file-source label. */
const MAX_LABEL_CHARS = 200;

/**
 * Flatten an untrusted `fileSource` label into a single safe line for prompt
 * interpolation (W6). The label comes from Oracle (ultimately a path/string in
 * the indexed corpus) and is placed verbatim inside `--- <label> ---`. Newlines
 * and control chars would let it forge extra grounding sections or break the
 * prompt structure (prompt-injection), so collapse all C0/C1 control characters
 * (incl. CR/LF/TAB) to single spaces, squeeze runs, trim, and bound the length.
 * PURE.
 */
function sanitizeLabel(raw: string): string {
  const flattened = raw
    // Collapse C0 (\x00-\x1F) + DEL (\x7F) + C1 (\x80-\x9F) control chars
    // (incl. CR/LF/TAB) then squeeze remaining whitespace runs to single spaces.
    // eslint-disable-next-line no-control-regex
    .replace(/[\x00-\x1F\x7F-\x9F]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  const safe = flattened.length > 0 ? flattened : "unknown";
  return safe.length <= MAX_LABEL_CHARS
    ? safe
    : safe.slice(0, MAX_LABEL_CHARS) + "…";
}

/** Truncate a snippet to a char budget, appending an ellipsis marker when cut. */
function snippet(text: string, max: number): string {
  const t = text.trim();
  if (t.length <= max) return t;
  return t.slice(0, max) + "\n… (truncated)";
}

/**
 * Build the compact grounding block from retrieved chunks + the target's DTCG token
 * NAMES. Returns "" when there is nothing to ground on (no chunks AND no tokens) —
 * the caller then generates without a context block. PURE.
 *
 * Layout (passed verbatim as the prompt `context`):
 *   prefer these design tokens: color.brand, spacing.md, …      (when tokens present)
 *
 *   relevant excerpts from the target codebase (stay coherent with these):
 *   --- <file label> ---
 *   <bounded snippet>
 *   --- <file label> ---
 *   …
 */
export function buildGroundingBlock(
  chunks: DesignContextChunk[],
  tokenNames: string[] = [],
): string {
  const parts: string[] = [];

  const names = tokenNames.filter((n) => n.trim().length > 0);
  if (names.length > 0) {
    parts.push(`Prefer these design tokens: ${names.join(", ")}.`);
  }

  // Take the top-N chunks with non-empty text (already score-ordered by Oracle, but
  // we don't trust that — keep input order, which is the server's ranking).
  const usable = chunks
    .filter((c) => c && typeof c.text === "string" && c.text.trim().length > 0)
    .slice(0, MAX_CHUNKS);

  if (usable.length > 0) {
    const lines: string[] = [];
    lines.push(
      "Relevant excerpts from the target codebase (stay coherent with these):",
    );
    for (const c of usable) {
      // The label is UNTRUSTED (Oracle corpus). Flatten control chars/newlines
      // so it cannot forge extra `--- … ---` sections or break prompt structure.
      const label = sanitizeLabel(c.fileSource || "unknown");
      lines.push(`--- ${label} ---`);
      lines.push(snippet(c.text, MAX_SNIPPET_CHARS));
    }
    parts.push(lines.join("\n"));
  }

  const block = parts.join("\n\n").trim();
  if (block.length <= MAX_BLOCK_CHARS) return block;
  return block.slice(0, MAX_BLOCK_CHARS) + "\n… (grounding truncated)";
}
