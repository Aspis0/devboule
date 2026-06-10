// Tier-1 generation guard — DETERMINISTIC contract validator + auto-fixer for a
// SINGLE node's markup (Phase 2.5 STEP A). Runs on LLM-generated/edited markup
// AFTER parse + re-anchor and BEFORE the sanitize chokepoint, so the deterministic
// layer (not a model) owns structural correctness per LOCKED architecture 1.5.
//
// Responsibilities (LOCKED contract 1.1 / 1.3):
//   - Finding 8: the HOST owns top-level placement, so positional CSS on the ROOT
//     element is neutralized — `position` ONLY when it TAKES the root OUT of normal
//     flow (`absolute`/`fixed`/`sticky`), plus `top`/`left`/`right`/`bottom`/`inset*`,
//     `float`/`clear`, `z-index`, and OUTER `margin*`. `position:relative`/`static`
//     are KEPT: they establish a positioning CONTEXT for `position:absolute`
//     DESCENDANTS without taking the root itself out of flow, so stripping them
//     would make those children escape to the host/viewport. Descendants are left
//     untouched (intra-component layout is the model's, 1.3).
//   - Finding 9: a foster-parented root tag (`<tr>`, `<td>`, `<option>`, ...) can
//     never be a free-standing canvas node — the HTML parser hoists/relocates it,
//     so a node id would be stamped on the wrong element (or nothing). These are
//     flagged as UNFIXABLE so the caller drops the node with a warning.
//   - MULTIPLE_TOP_LEVEL / EMPTY / SCRIPT_OR_HANDLER are reported (the last is
//     defense-in-depth — sanitize.ts is still the security boundary downstream).
//
// PURE-ish: the only impurity is a DOMParser, kept INJECTABLE so the validator is
// table-testable in a plain node environment. NO clock, NO random.

/** Machine-readable violation codes. Stable — referenced by the repair builder. */
export type ViolationCode =
  | "EMPTY"
  | "FOSTER_PARENTED_ROOT"
  | "MULTIPLE_TOP_LEVEL"
  | "POSITIONAL_CSS_ON_ROOT"
  | "SCRIPT_OR_HANDLER";

/** One contract violation: a machine code + a human-readable message. */
export interface Violation {
  code: ViolationCode;
  message: string;
}

/** Result of {@link validateNodeMarkup}. */
export interface ValidationResult {
  violations: Violation[];
  /** Lowercased tag of the resolved root element, or null if none/foster-parented. */
  rootTag: string | null;
  /** True when the markup's leading element is a foster-parented (relocatable) tag. */
  fosterParented: boolean;
}

/** Result of {@link autoFixNodeMarkup}. */
export interface AutoFixResult {
  /** The fixed markup (root positional CSS stripped, single top-level kept). */
  markup: string;
  /** Violations that were deterministically resolved. */
  fixed: Violation[];
  /** Violations that could NOT be auto-fixed — caller drops the node + warns. */
  remaining: Violation[];
  /**
   * True when the node yields a USABLE element to commit (i.e. NOT a `remaining`
   * unfixable case). False signals the caller MUST NOT persist a "no-op" edit as
   * if it succeeded (WARNING 5). Note: `usable === true` even when nothing was
   * mutated (clean markup) — it only goes false for empty/foster/no-root.
   */
  usable: boolean;
  /**
   * True when MULTIPLE_TOP_LEVEL was auto-fixed (trailing siblings dropped). The
   * caller surfaces this upstream so the operator knows extra content was collapsed
   * (WARNING 9) and the self-repair can hint "one element per component".
   */
  collapsedSiblings: boolean;
}

/** An injectable DOMParser factory (the runtime/jsdom default binds to `window`). */
export type ParseHtml = (markup: string) => Document;

const defaultParseHtml: ParseHtml = (markup) =>
  new DOMParser().parseFromString(markup, "text/html");

// Tags the HTML parser foster-parents / relocates (table internals) or only
// permits inside a specific parent (`<option>`/`<optgroup>` inside <select>). None
// can stand alone as a top-level canvas node: the parser either drops them
// (body.children === 0) or would relocate them, so a stamped id lands wrong.
const FOSTER_PARENT_TAGS = new Set([
  "tr",
  "td",
  "th",
  "thead",
  "tbody",
  "tfoot",
  "col",
  "colgroup",
  "caption",
  "option",
  "optgroup",
]);

// Root-level CSS properties the HOST owns (Finding 8). Stripped from the root
// element's inline style ONLY. `margin` (+ logical/long-hand variants) is the
// OUTER margin; inner padding/gap is kept. NOTE: `position` is NOT in this set —
// it is handled separately by {@link isDangerousRootPosition} because only the
// out-of-flow values (`absolute`/`fixed`/`sticky`) are dangerous; `relative`/
// `static` are a legit positioning context for absolutely-positioned descendants
// and MUST be kept.
const ROOT_POSITIONAL_PROPS = new Set([
  "top",
  "left",
  "right",
  "bottom",
  "float",
  "clear",
  "z-index",
  "inset",
  "inset-block",
  "inset-block-start",
  "inset-block-end",
  "inset-inline",
  "inset-inline-start",
  "inset-inline-end",
]);

// `position` values that take the root OUT of normal flow and let it escape the
// host placement box. `relative`/`static` (and any non-listed value such as
// `inherit`) are KEPT: they only establish a containing block for absolutely-
// positioned DESCENDANTS, which is legit intra-component layout (1.3).
const DANGEROUS_POSITION_VALUES = new Set(["absolute", "fixed", "sticky"]);

// Normalize a `position` value for the dangerous-value test: lowercase, trim, and
// tolerate a trailing `!important`. e.g. "Fixed !important" -> "fixed".
function isDangerousPositionValue(value: string): boolean {
  const normalized = value
    .replace(/!\s*important\s*$/i, "")
    .trim()
    .toLowerCase();
  return DANGEROUS_POSITION_VALUES.has(normalized);
}

// True when a declaration is a host-owned positional prop that must be stripped
// from the ROOT: the offset/float/inset/z-index set, an OUTER margin, OR a
// `position` with an out-of-flow value. `position:relative`/`static` are NOT
// dangerous and return false (kept).
function isDangerousRootDeclaration(d: Declaration): boolean {
  if (d.prop === "position") return isDangerousPositionValue(d.value);
  return ROOT_POSITIONAL_PROPS.has(d.prop) || isMarginProp(d.prop);
}

// Margin family: the outer box edge. Matches `margin`, `margin-top`, `margin-block`,
// `margin-inline-start`, etc. — but NOT `padding` (kept) and NOT a property merely
// containing the substring (we match a whole CSS property name).
function isMarginProp(prop: string): boolean {
  return prop === "margin" || prop.startsWith("margin-");
}

/** A leading tag-name matcher: the first `<tagname` at the very start (after ws). */
const LEADING_TAG_RE = /^\s*<\s*([a-zA-Z][a-zA-Z0-9-]*)/;

/** Find the first element tag name in raw markup (lowercased) or null. */
function leadingTagName(markup: string): string | null {
  const m = LEADING_TAG_RE.exec(markup);
  return m ? m[1].toLowerCase() : null;
}

// Cheap defense-in-depth scan for script/handler tokens in RAW markup (sanitize.ts
// is still the real boundary; this only SURFACES the violation as a contract
// signal so the repair loop can ask the model to stop emitting them).
const SCRIPT_TAG_RE = /<\s*script[\s>/]/i;
const ON_HANDLER_RE = /\son[a-z]+\s*=/i;
const JS_URI_RE = /(?:href|src)\s*=\s*["']?\s*javascript:/i;

function hasScriptOrHandler(markup: string): boolean {
  return (
    SCRIPT_TAG_RE.test(markup) ||
    ON_HANDLER_RE.test(markup) ||
    JS_URI_RE.test(markup)
  );
}

/**
 * Internal validation that ALSO returns the parsed top-level roots (or null when
 * the markup short-circuited before a parse: empty / foster-parented). This lets
 * {@link autoFixNodeMarkup} reuse the SAME parse instead of re-parsing (WARNING 8:
 * no redundant DOMParser pass on the generation hot path).
 */
function validateInternal(
  markup: string,
  parseHtml: ParseHtml,
): { result: ValidationResult; roots: Element[] | null } {
  const violations: Violation[] = [];

  if (typeof markup !== "string" || markup.trim().length === 0) {
    violations.push({ code: "EMPTY", message: "Markup is empty." });
    return {
      result: { violations, rootTag: null, fosterParented: false },
      roots: null,
    };
  }

  // Foster-parent detection is driven by the RAW leading tag, NOT the parsed tree:
  // the parser DROPS most foster-parented tags entirely (body.children === 0), so
  // by the time we'd inspect the tree the evidence is gone. The leading tag name is
  // the reliable signal.
  const leadTag = leadingTagName(markup);
  const fosterParented = leadTag !== null && FOSTER_PARENT_TAGS.has(leadTag);

  if (hasScriptOrHandler(markup)) {
    violations.push({
      code: "SCRIPT_OR_HANDLER",
      message: "Markup contains a <script>, on* handler, or javascript: URL.",
    });
  }

  if (fosterParented) {
    violations.push({
      code: "FOSTER_PARENTED_ROOT",
      message: `Top-level element <${leadTag}> cannot be a free-standing node (it is relocated by the HTML parser).`,
    });
    // A foster-parented root yields no usable parsed element; report and bail.
    return {
      result: { violations, rootTag: null, fosterParented: true },
      roots: null,
    };
  }

  const doc = parseHtml(markup);
  const roots = Array.from(doc.body.children);

  if (roots.length === 0) {
    // Non-empty markup that parses to zero elements (stray prose, or a relocatable
    // tag not in our set). Treat as EMPTY for the caller (nothing to place).
    violations.push({
      code: "EMPTY",
      message: "Markup contains no top-level element.",
    });
    return {
      result: { violations, rootTag: null, fosterParented },
      roots,
    };
  }

  const root = roots[0];
  const rootTag = root.tagName.toLowerCase();

  if (roots.length > 1) {
    violations.push({
      code: "MULTIPLE_TOP_LEVEL",
      message: `Expected exactly one top-level element, found ${roots.length}.`,
    });
  }

  if (rootHasPositionalCss(root)) {
    violations.push({
      code: "POSITIONAL_CSS_ON_ROOT",
      message:
        "Top-level element sets host-owned positional CSS (position:absolute/fixed/sticky, top/left/right/bottom/inset, float, z-index, or outer margin).",
    });
  }

  return { result: { violations, rootTag, fosterParented }, roots };
}

/**
 * Validate one node's markup against the LOCKED contract. PURE aside from the
 * injectable parser. Reports every violation it finds (does NOT mutate markup —
 * see {@link autoFixNodeMarkup} for the deterministic fixes).
 */
export function validateNodeMarkup(
  markup: string,
  parseHtml: ParseHtml = defaultParseHtml,
): ValidationResult {
  return validateInternal(markup, parseHtml).result;
}

/** True if the ROOT element's inline style declares any host-owned positional prop. */
function rootHasPositionalCss(root: Element): boolean {
  const style = root.getAttribute("style");
  if (!style) return false;
  const decls = parseDeclarations(style);
  // If the style is un-parseable (null = conservative fallback), we do NOT claim a
  // violation: autoFix would be unable to safely strip it anyway, and a false flag
  // would spin the self-repair loop. Treat as clean.
  if (decls === null) return false;
  return decls.some(isDangerousRootDeclaration);
}

/** One parsed `prop: value` declaration (prop lowercased, value trimmed). */
interface Declaration {
  prop: string;
  value: string;
}

/**
 * Parse an inline-style attribute into ordered declarations with a URL- and
 * quote-AWARE tokenizer. The naive `split(";")` corrupts values that legitimately
 * contain `;` — e.g. `background:url(data:image/png;base64,AAA)` (the `;base64`
 * is eaten) or `content:";"` — and `indexOf(":")` mishandles multi-colon values
 * (`url(data:...)`). So we walk the string tracking depth inside `url(...)` and
 * inside `'`/`"` quotes, and only treat a `;` as a separator and the FIRST `:`
 * as the prop/value split when OUTSIDE both.
 *
 * The property name is lowercased; the value (including the FULL url, `!important`,
 * and any inner `;`/`:`) is preserved VERBATIM so re-serialization is byte-exact.
 *
 * CONSERVATIVE FALLBACK: if the style cannot be safely parsed (unterminated quote
 * or unbalanced `url(`), returns `null`. Callers then leave the style attribute
 * UNTOUCHED rather than risk corrupting it. PURE string parsing — no CSSOM.
 */
function parseDeclarations(style: string): Declaration[] | null {
  const segments = splitTopLevel(style, ";");
  if (segments === null) return null; // unbalanced -> conservative fallback
  const out: Declaration[] = [];
  for (const segment of segments) {
    if (segment.trim().length === 0) continue;
    const idx = colonOutsideUrlOrQuote(segment);
    if (idx === -1) continue; // no prop/value colon -> skip this segment
    const prop = segment.slice(0, idx).trim().toLowerCase();
    const value = segment.slice(idx + 1).trim();
    if (prop.length === 0 || value.length === 0) continue;
    out.push({ prop, value });
  }
  return out;
}

/**
 * Split `s` on every top-level occurrence of `sep` (a single char), ignoring
 * separators that fall inside `url(...)` or inside single/double quotes. Returns
 * `null` if the string ends with an unterminated quote or an unbalanced `url(`,
 * which signals the caller to bail (conservative fallback).
 */
function splitTopLevel(s: string, sep: string): string[] | null {
  const out: string[] = [];
  let start = 0;
  let urlDepth = 0; // open `url(` parens (we only track url() — other parens are rare in inline style and not separators)
  let quote: '"' | "'" | null = null;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (quote) {
      if (c === quote) quote = null;
      continue;
    }
    if (c === '"' || c === "'") {
      quote = c;
      continue;
    }
    if (urlDepth > 0) {
      if (c === "(") urlDepth++;
      else if (c === ")") urlDepth--;
      continue;
    }
    if ((c === "u" || c === "U") && matchesUrlOpen(s, i)) {
      // consume `url(` and enter url-depth (the `(` is at the matched offset).
      const open = s.indexOf("(", i);
      urlDepth = 1;
      i = open; // loop ++ moves past `(`
      continue;
    }
    if (c === sep) {
      out.push(s.slice(start, i));
      start = i + 1;
    }
  }
  if (quote !== null || urlDepth > 0) return null; // unterminated -> bail
  out.push(s.slice(start));
  return out;
}

/**
 * True if a `url(` token (case-insensitive, optional whitespace before the `(`)
 * begins at index `i`. Requires the chars before `url` to NOT be an identifier
 * char so we don't match the tail of e.g. `myurl(`. The 32-char window bounds the
 * regex cost while tolerating any realistic amount of whitespace before `(`.
 */
function matchesUrlOpen(s: string, i: number): boolean {
  if (i > 0 && /[a-z0-9_-]/i.test(s[i - 1])) return false;
  return /^url\s*\(/i.test(s.slice(i, i + 32));
}

/**
 * Index of the FIRST `:` in `segment` that is OUTSIDE `url(...)` and quotes (the
 * prop/value boundary), or -1 if none. Mirrors {@link splitTopLevel}'s scanning so
 * `background:url(data:image/png;base64,...)` splits on the colon after
 * `background`, not the one inside the data-URL.
 */
function colonOutsideUrlOrQuote(segment: string): number {
  let urlDepth = 0;
  let quote: '"' | "'" | null = null;
  for (let i = 0; i < segment.length; i++) {
    const c = segment[i];
    if (quote) {
      if (c === quote) quote = null;
      continue;
    }
    if (c === '"' || c === "'") {
      quote = c;
      continue;
    }
    if (urlDepth > 0) {
      if (c === "(") urlDepth++;
      else if (c === ")") urlDepth--;
      continue;
    }
    if ((c === "u" || c === "U") && matchesUrlOpen(segment, i)) {
      const open = segment.indexOf("(", i);
      urlDepth = 1;
      i = open;
      continue;
    }
    if (c === ":") return i;
  }
  return -1;
}

/** Serialize declarations back to an inline-style string (`a:b;c:d;`), or "" if none. */
function serializeDeclarations(decls: Declaration[]): string {
  if (decls.length === 0) return "";
  return decls.map((d) => `${d.prop}:${d.value}`).join(";") + ";";
}

/**
 * DETERMINISTICALLY fix what can be fixed; report what cannot:
 *   - Finding 8: strip host-owned positional CSS from the ROOT element ONLY (its
 *     descendants are untouched). Reported as `fixed: POSITIONAL_CSS_ON_ROOT`.
 *   - MULTIPLE_TOP_LEVEL: keep the FIRST top-level element, drop the rest. Reported
 *     as `fixed: MULTIPLE_TOP_LEVEL`.
 *   - Finding 9 (FOSTER_PARENTED_ROOT) and EMPTY are UNFIXABLE — there is no valid
 *     free-standing node to salvage — so they go to `remaining` and the caller
 *     drops the node with a warning.
 *   - SCRIPT_OR_HANDLER is reported but NOT relied on for security: the downstream
 *     sanitize chokepoint removes it regardless, so it is recorded as `fixed`
 *     (sanitize will neutralize it) to inform the repair prompt without blocking.
 *
 * PURE aside from the injectable parser. Returns the original markup unchanged for
 * any unfixable case.
 */
export function autoFixNodeMarkup(
  markup: string,
  parseHtml: ParseHtml = defaultParseHtml,
): AutoFixResult {
  // WARNING 8: parse ONCE. validateInternal returns the parsed roots, reused below.
  const { result, roots } = validateInternal(markup, parseHtml);
  const { violations, fosterParented } = result;

  // Unfixable short-circuits: empty / foster-parented yield nothing to place.
  const empty = violations.find((v) => v.code === "EMPTY");
  if (empty) {
    return unusable(markup, empty);
  }
  if (fosterParented) {
    const foster = violations.find((v) => v.code === "FOSTER_PARENTED_ROOT");
    // Always present here, but keep the type honest.
    return foster
      ? unusable(markup, foster)
      : unusable(markup, { code: "EMPTY", message: "No usable root element." });
  }

  // validateInternal already guaranteed >= 1 element for the non-empty/non-foster
  // path; defensive guard keeps this total.
  if (roots === null || roots.length === 0) {
    return unusable(markup, {
      code: "EMPTY",
      message: "Markup contains no top-level element.",
    });
  }

  const fixed: Violation[] = [];
  const root = roots[0];

  // MULTIPLE_TOP_LEVEL: keep the first element only (the edit/contract expects one).
  const collapsedSiblings = roots.length > 1;
  if (collapsedSiblings) {
    fixed.push({
      code: "MULTIPLE_TOP_LEVEL",
      message: `Kept the first of ${roots.length} top-level elements.`,
    });
  }

  // Finding 8: strip host-owned positional CSS from the ROOT element only.
  if (stripRootPositionalCss(root)) {
    fixed.push({
      code: "POSITIONAL_CSS_ON_ROOT",
      message: "Stripped host-owned positional CSS from the top-level element.",
    });
  }

  // SCRIPT_OR_HANDLER: surfaced for the repair prompt; sanitize neutralizes it.
  const scriptViolation = violations.find((v) => v.code === "SCRIPT_OR_HANDLER");
  if (scriptViolation) {
    fixed.push({
      code: "SCRIPT_OR_HANDLER",
      message:
        "Script/handler will be removed by sanitization (kept node, neutralized markup).",
    });
  }

  return {
    markup: root.outerHTML,
    fixed,
    remaining: [],
    usable: true,
    collapsedSiblings,
  };
}

/** Build an unusable (no-op) AutoFixResult: original markup, one `remaining`. */
function unusable(markup: string, remaining: Violation): AutoFixResult {
  return {
    markup,
    fixed: [],
    remaining: [remaining],
    usable: false,
    collapsedSiblings: false,
  };
}

/**
 * Remove host-owned positional declarations from the ROOT element's inline style
 * IN PLACE. Returns true if anything was removed. Descendants are NOT touched.
 * CONSERVATIVE: if the inline style is un-parseable (a tokenizer bail, e.g. an
 * unterminated quote/url), the attribute is left UNTOUCHED rather than corrupted
 * (BLOCKER 2) — and nothing is reported as fixed.
 */
function stripRootPositionalCss(root: Element): boolean {
  const style = root.getAttribute("style");
  if (!style) return false;
  const decls = parseDeclarations(style);
  if (decls === null) return false; // un-parseable -> leave untouched
  const kept = decls.filter((d) => !isDangerousRootDeclaration(d));
  if (kept.length === decls.length) return false; // nothing stripped

  const next = serializeDeclarations(kept);
  if (next.length === 0) root.removeAttribute("style");
  else root.setAttribute("style", next);
  return true;
}
