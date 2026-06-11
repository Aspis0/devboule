// Design tokens — W3C DTCG model (Phase 2 STEP 4). Replaces the Phase-1b stub.
//
// We adopt the W3C Design Tokens Community Group (DTCG) JSON format: a tree of
// GROUPS whose leaves are TOKENS, each `{ "$value": ..., "$type": ... }`. The
// document is portable (re-importable into the target) and is the single source
// for token enforcement (in the generate prompt) + export.
//
//   {
//     "color":   { "brand": { "$value": "#c2410c", "$type": "color" } },
//     "spacing": { "md":    { "$value": "8px",     "$type": "dimension" } }
//   }
//
// This module is PURE (no DOM, no network, no clock, no random). Oracle seeding
// (which DOES hit the backend) lives in the impure helper `seedTokensFromOracle`
// in the design surface; here we only provide the model, a validator, and the
// prompt-name extractor.
//
// Future (DEFERRED — the "Token Coherence Loop"): Censor enforcing tokens IN the
// target's code, and DTCG -> target codegen. NOT built here; this step only feeds
// token NAMES into the generate prompt as a soft preference.

/** The DTCG `$type` values we recognize. Unknown types are tolerated (kept as-is)
 * so a richer target document round-trips, but only these are first-class. */
export type DtcgType =
  | "color"
  | "dimension"
  | "fontFamily"
  | "fontWeight"
  | "duration"
  | "number"
  | "string";

/** A single DTCG token leaf: a `$value` plus an optional `$type` (+ `$description`).
 * `$value` is intentionally `unknown` — DTCG values range over strings, numbers,
 * and composite objects; the validator narrows what we accept. */
export interface DtcgToken {
  $value: unknown;
  $type?: string;
  $description?: string;
}

/** A DTCG node is either a token leaf or a nested group of more nodes. We model the
 * group recursively; `$`-prefixed keys on a group are DTCG metadata, not children. */
export interface DtcgGroup {
  [key: string]: DtcgGroup | DtcgToken | string | undefined;
  $type?: string;
  $description?: string;
}

/** The top-level DTCG document is a group. */
export type DtcgDocument = DtcgGroup;

/** An empty DTCG document — the default when no tokens are configured / seeded. */
export const EMPTY_TOKENS: DtcgDocument = {};

/** True if a value is a non-null plain object (DTCG group or token candidate). */
function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** True if an object node is a DTCG token LEAF (has an own `$value` key). DTCG
 * distinguishes a token from a group solely by the presence of `$value`. */
export function isDtcgToken(node: unknown): node is DtcgToken {
  return isObject(node) && Object.prototype.hasOwnProperty.call(node, "$value");
}

/**
 * Validate that `doc` is a well-formed DTCG document. PURE. Returns the list of
 * problems found (empty list === valid). We deliberately keep this LIGHT (this
 * step's scope): structural well-formedness, not exhaustive per-type value
 * checking. Rules:
 *  - the root must be a plain object (group);
 *  - every non-`$` key maps to either a group (object without `$value`) or a token
 *    leaf (object WITH `$value`);
 *  - a token's `$type`, when present, must be a string;
 *  - groups nest recursively under the same rules;
 *  - a key whose value is neither an object nor a `$`-metadata string is invalid.
 * Bounded by a max depth so a pathological/cyclic-by-construction document cannot
 * recurse without limit.
 */
export function validateTokensDoc(doc: unknown): string[] {
  const problems: string[] = [];
  if (!isObject(doc)) {
    return ["tokens document must be a JSON object"];
  }
  const MAX_DEPTH = 16;
  const walk = (node: Record<string, unknown>, path: string, depth: number) => {
    if (depth > MAX_DEPTH) {
      problems.push(`tokens document nests too deeply at "${path}"`);
      return;
    }
    for (const [key, value] of Object.entries(node)) {
      // `$`-prefixed keys are DTCG metadata ($type/$description/$value/$extensions).
      if (key.startsWith("$")) {
        if (
          (key === "$type" || key === "$description") &&
          value !== undefined &&
          typeof value !== "string"
        ) {
          problems.push(`"${path}${key}" must be a string`);
        }
        continue;
      }
      const childPath = path ? `${path}.${key}` : key;
      if (!isObject(value)) {
        problems.push(`"${childPath}" must be a group or a token object`);
        continue;
      }
      if (isDtcgToken(value)) {
        const t = (value as DtcgToken).$type;
        if (t !== undefined && typeof t !== "string") {
          problems.push(`"${childPath}.$type" must be a string`);
        }
        // $value may be string/number/object — accepted as-is in this step.
      } else {
        walk(value, childPath, depth + 1);
      }
    }
  };
  walk(doc as Record<string, unknown>, "", 0);
  return problems;
}

/** Convenience boolean form of {@link validateTokensDoc}. */
export function isValidTokensDoc(doc: unknown): boolean {
  return validateTokensDoc(doc).length === 0;
}

/** Max token names we surface to the prompt (a soft preference list, not a corpus).
 * Bounds the prompt block a huge target document could otherwise produce. */
const MAX_PROMPT_TOKEN_NAMES = 80;

/**
 * Extract the dotted NAMES of every token leaf in a DTCG document, in stable
 * (depth-first, key-sorted) order. PURE. Used to seed the generate prompt with a
 * soft "prefer these tokens" preference. Returns at most {@link MAX_PROMPT_TOKEN_NAMES}
 * names; an invalid document yields an empty list (caller degrades to no tokens).
 *
 * Example: `{ color: { brand: { $value:"#c2410c", $type:"color" } } }` -> `["color.brand"]`.
 */
export function tokenNamesForPrompt(doc: unknown): string[] {
  if (!isObject(doc)) return [];
  const names: string[] = [];
  const MAX_DEPTH = 16;
  const walk = (node: Record<string, unknown>, path: string, depth: number) => {
    if (depth > MAX_DEPTH || names.length >= MAX_PROMPT_TOKEN_NAMES) return;
    // Sort keys for a deterministic, stable name list (no clock/random).
    const keys = Object.keys(node)
      .filter((k) => !k.startsWith("$"))
      .sort();
    for (const key of keys) {
      if (names.length >= MAX_PROMPT_TOKEN_NAMES) return;
      const value = node[key];
      if (!isObject(value)) continue;
      const childPath = path ? `${path}.${key}` : key;
      if (isDtcgToken(value)) {
        names.push(childPath);
      } else {
        walk(value, childPath, depth + 1);
      }
    }
  };
  walk(doc as Record<string, unknown>, "", 0);
  return names;
}

/** A color token surfaced to the UI: its dotted name + its resolved CSS color
 *  string. Produced by {@link colorTokens}. */
export interface ColorTokenSwatch {
  name: string;
  value: string;
}

/** Max color swatches surfaced to the content-edit toolbar (a compact palette, not
 *  the whole token corpus). Bounds the toolbar width. */
const MAX_COLOR_SWATCHES = 6;

/**
 * A CONSERVATIVE CSS color shape. A color token's `$value` is applied as an inline
 * style (`el.style.color = value`) by the CE swatch, so a value like
 * `"red; position: fixed"` or `"url(javascript:x)"` could inject extra declarations or
 * a URL. The browser's CSS parser would reject most of that, but we never want to feed
 * such a value to the style API at all. We accept ONLY: hex (#RGB/#RGBA/#RRGGBB/#RRGGBBAA),
 * the functional forms rgb/rgba/hsl/hsla/oklch/oklab/lab/lch/color(...) with no `;` or
 * `{}` inside, or a bare CSS named color ([a-z]{2,20}). Anything else is filtered out.
 */
const CSS_COLOR_RE =
  /^(?:#(?:[0-9a-f]{3}|[0-9a-f]{4}|[0-9a-f]{6}|[0-9a-f]{8})|(?:rgb|rgba|hsl|hsla|hwb|lab|lch|oklab|oklch|color)\([^;{}]*\)|[a-z]{2,20})$/i;

/** True when `value` is a safe, conservative CSS color string (see {@link CSS_COLOR_RE}). */
export function isSafeCssColor(value: string): boolean {
  const v = value.trim();
  // Reject control chars / declaration or block separators outright (defense in depth —
  // the functional-form branch already forbids `;{}` inside the parens).
  if (/[;{}<>]/.test(v)) return false;
  return CSS_COLOR_RE.test(v);
}

/**
 * Extract the COLOR token leaves from a DTCG document as `{ name, value }` pairs in
 * stable (depth-first, key-sorted) order, capped at {@link MAX_COLOR_SWATCHES}. A
 * token counts as a color when its `$type === "color"` AND its `$value` is a
 * non-empty string (a composite/object color value is skipped — we only surface
 * scalar CSS colors a swatch can apply). PURE. An invalid/empty document yields `[]`
 * so the caller can fall back to a neutral default palette.
 */
export function colorTokens(doc: unknown): ColorTokenSwatch[] {
  if (!isObject(doc)) return [];
  const out: ColorTokenSwatch[] = [];
  const MAX_DEPTH = 16;
  const walk = (node: Record<string, unknown>, path: string, depth: number) => {
    if (depth > MAX_DEPTH || out.length >= MAX_COLOR_SWATCHES) return;
    const keys = Object.keys(node)
      .filter((k) => !k.startsWith("$"))
      .sort();
    for (const key of keys) {
      if (out.length >= MAX_COLOR_SWATCHES) return;
      const value = node[key];
      if (!isObject(value)) continue;
      const childPath = path ? `${path}.${key}` : key;
      if (isDtcgToken(value)) {
        const tok = value as DtcgToken;
        if (
          tok.$type === "color" &&
          typeof tok.$value === "string" &&
          tok.$value.trim().length > 0 &&
          // The value is applied as an inline style by the CE swatch — only surface a
          // conservative, well-formed CSS color (filters injection like
          // "red; position: fixed" or "url(javascript:x)").
          isSafeCssColor(tok.$value)
        ) {
          out.push({ name: childPath, value: tok.$value.trim() });
        }
      } else {
        walk(value, childPath, depth + 1);
      }
    }
  };
  walk(doc as Record<string, unknown>, "", 0);
  return out;
}

/**
 * Resolve a DTCG token reference of the form `{group.token}` to its `$value`
 * within `doc`. Returns the input unchanged if it is not a reference or the path is
 * absent / not a token. PURE. (Aliases-of-aliases are NOT chased in this step —
 * single-hop resolution only.)
 *
 * WARNING 6: a `$value` may be a COMPOSITE object (DTCG shadow/gradient/typography)
 * or null. We must NEVER `String(...)`-coerce those into the prompt — that injects
 * `"[object Object]"` / `"null"`. We resolve ONLY scalar values: a string passes
 * through, a finite number is stringified; for anything else (object/array/null/
 * boolean/NaN) we return the ORIGINAL reference string unchanged so the caller sees
 * an unresolved `{ref}` rather than garbage.
 */
export function resolveToken(value: string, doc: DtcgDocument): string {
  const ref = value.match(/^\{([^}]+)\}$/);
  if (!ref) return value;
  const segments = ref[1].split(".");
  let node: unknown = doc;
  for (const seg of segments) {
    if (!isObject(node)) return value;
    node = (node as Record<string, unknown>)[seg];
  }
  if (isDtcgToken(node)) {
    const v = (node as DtcgToken).$value;
    if (typeof v === "string") return v;
    if (typeof v === "number" && Number.isFinite(v)) return String(v);
    // Object/array/null/boolean/NaN: do NOT coerce — return the original ref.
    return value;
  }
  return value;
}
