// Markup extraction from raw model text (Phase 2 STEP 3).
//
// The model SHOULD return bare markup per the prompt contract, but real models
// wrap output in ```html fences and/or surround it with prose ("Here is the
// section:"). `extractMarkup` is the tolerant, PURE pre-parse step that recovers
// the raw markup fragment:
//   - fenced blocks (```html … ``` or bare ``` … ```): concatenate the INSIDE of
//     every fenced block, dropping the fences and any prose between/around them;
//   - no fences: return the text trimmed (the parser tolerates surrounding prose
//     because only top-level *elements* become nodes — stray text is ignored).
//
// PURE (string -> string): no DOM, no clock, no random. DOM parsing of the
// returned fragment is delegated to `parseTopLevelNodes` (iframeInject.ts).

// A fenced code block: ```lang? \n … \n``` . We capture the inner body and ignore
// the optional info string (html / svg / xml / etc.) on the opening fence.
//
// - `^|\n` anchors the opening fence to a line start (so a literal "```" inside
//   content doesn't false-trigger mid-line).
// - The info string is the rest of that opening line (non-greedy, no backticks).
// - The body is everything up to the next closing fence line.
const FENCE_RE = /(?:^|\n)```[^\n`]*\n([\s\S]*?)(?:\n```|$)/g;

/**
 * Strip code fences and surrounding prose from raw model text, returning the raw
 * markup fragment. Tolerant of: no fences (returns the trimmed text), a single
 * fence, multiple fences (their bodies are concatenated in order), and leading/
 * trailing prose. PURE.
 */
export function extractMarkup(modelText: unknown): string {
  if (typeof modelText !== "string") return "";
  const text = modelText;

  const bodies: string[] = [];
  let match: RegExpExecArray | null;
  // Reset lastIndex defensively (module-level regex with the `g` flag is stateful).
  FENCE_RE.lastIndex = 0;
  while ((match = FENCE_RE.exec(text)) !== null) {
    const body = match[1];
    if (body !== undefined) bodies.push(body.trim());
    // Guard against a zero-width match advancing the cursor (cannot happen with
    // this pattern, but keeps the loop provably terminating).
    if (match.index === FENCE_RE.lastIndex) FENCE_RE.lastIndex++;
  }

  if (bodies.length > 0) {
    // Concatenate fenced bodies with a separating newline so adjacent fragments
    // stay distinct top-level siblings when parsed.
    return bodies.filter((b) => b.length > 0).join("\n");
  }

  // No fences — return the trimmed text. The DOM parser ignores stray top-level
  // text, so leading/trailing prose around real elements is harmless.
  return text.trim();
}
