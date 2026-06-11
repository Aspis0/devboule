// extractTokens — PURE extraction of REAL design tokens from Oracle chunk text into
// a DTCG document (Phase C). This is the gap the old `seedTokens` stub left open:
// instead of writing an empty `{}`, we parse the target's OWN source excerpts that
// Oracle retrieved (tokens.css, tailwind.config.js, styled-components, SCSS vars,
// etc.) into concrete color/typography/spacing/radius tokens with real `$value`s.
//
// PURE: no DOM, no network, no clock, no random — deterministic output for a given
// input so it is snapshot/unit testable. The chunk text is the TARGET's own source;
// nothing here logs or emits it, it only shapes a token document the USER will review
// in the contract editor before anything is written.
//
// HEURISTICS (deliberately conservative — we would rather miss a token than invent
// one or leak a non-design value):
//   - colors: hex (#RGB/#RRGGBB/#RRGGBBAA), rgb()/rgba()/hsl()/hsla()/oklch(), but
//     ONLY when they sit in a CSS-ish context (a `:` declaration, a CSS var, a quoted
//     value, or a tailwind color key) — a bare hex inside a hash/id/sha is ignored.
//   - names: from the CSS custom-prop name (`--brand`), the SCSS/Less var (`$brand`),
//     or the tailwind theme key when available; otherwise color-1..N by appearance.
//   - typography: `font-family` declarations + tailwind `fontFamily` entries.
//   - radii: `border-radius` / tailwind `borderRadius` dimension values.
//   - spacing: a small set from `--space-*` / `gap` / `padding` / `margin` / tailwind
//     `spacing` entries (dimension values only).
//   - dedupe by VALUE (case-insensitive for colors); deterministic ordering by
//     frequency desc then first-seen; counts capped per category.
//   - empty document when nothing extractable.

import type { DtcgDocument } from "../engine/tokens";
import type { DesignContextChunk } from "../generation/grounding";

/** Logical alias for a DTCG token document (the spec's `DesignTokensDoc`). */
export type DesignTokensDoc = DtcgDocument;

// --- caps (bound a pathological corpus; values are soft design budgets) -----------
const MAX_COLORS = 24;
const MAX_FONTS = 6;
const MAX_RADII = 8;
const MAX_SPACING = 8;
/** Hard cap on total chars scanned across all chunks (defense against a huge dump). */
const MAX_SCAN_CHARS = 200_000;
/** Per-LINE char cap applied before regex scanning. Minified CSS/JS arrives as one
 * enormous line; several of our value regexes (`[^;{}]+?`, `[^)]{1,80}`) can backtrack
 * super-linearly on such a line. Truncating each line to a generous-but-bounded width is
 * a cheap guard: any real design declaration fits well under it, and we still scan the
 * line's head where the meaningful tokens live. */
const MAX_LINE_CHARS = 4096;

/** Truncate any single line longer than {@link MAX_LINE_CHARS}. PURE. Cheap O(n) split. */
function capLineWidths(text: string): string {
  // Fast path: nothing to do when no line can exceed the cap.
  if (text.length <= MAX_LINE_CHARS) return text;
  let needsCap = false;
  let start = 0;
  for (let i = 0; i <= text.length; i++) {
    if (i === text.length || text.charCodeAt(i) === 10 /* \n */) {
      if (i - start > MAX_LINE_CHARS) {
        needsCap = true;
        break;
      }
      start = i + 1;
    }
  }
  if (!needsCap) return text;
  return text
    .split("\n")
    .map((line) => (line.length > MAX_LINE_CHARS ? line.slice(0, MAX_LINE_CHARS) : line))
    .join("\n");
}

/** A captured token candidate: its value + an OPTIONAL source name + first-seen index
 * + a frequency count. Ordering uses count desc then `seq` (first-seen) asc. */
interface Cand {
  value: string;
  name?: string;
  seq: number;
  count: number;
}

/** Accumulator keyed by a normalized de-dupe key (lowercased value). */
type Bag = Map<string, Cand>;

function bump(bag: Bag, key: string, value: string, seq: number, name?: string) {
  const existing = bag.get(key);
  if (existing) {
    existing.count += 1;
    // Keep the FIRST name we saw (deterministic); don't overwrite with a later one.
    if (!existing.name && name) existing.name = name;
    return;
  }
  bag.set(key, { value, name, seq, count: 1 });
}

/** Order candidates by frequency desc, then first-seen asc, then value for stability. */
function ordered(bag: Bag, cap: number): Cand[] {
  return Array.from(bag.values())
    .sort(
      (a, b) =>
        b.count - a.count || a.seq - b.seq || a.value.localeCompare(b.value),
    )
    .slice(0, cap);
}

// --- name sanitization (token names must be safe DTCG keys) ------------------------

/** Turn a raw CSS var / tailwind key into a safe, lowercase, kebab token name.
 * Strips leading `--`/`$`, quotes, and collapses non-alphanumerics to single dashes.
 * Returns "" when nothing usable remains (caller falls back to an ordinal name). */
/** Cap on a generated token NAME (chars). A pathological CSS var name (e.g. a 200-char
 * `--…` from a minified/obfuscated source) must not become a 200-char DTCG key. We slice
 * AFTER cleaning, then re-trim any trailing dash the cut may have exposed. */
const MAX_NAME_CHARS = 64;

function sanitizeName(raw: string | undefined): string {
  if (!raw) return "";
  const cleaned = raw
    .trim()
    .replace(/^["'`]|["'`]$/g, "")
    .replace(/^--/, "")
    .replace(/^\$/, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  if (cleaned.length <= MAX_NAME_CHARS) return cleaned;
  // Cap, then strip a trailing dash the slice may have exposed (keep the name tidy).
  return cleaned.slice(0, MAX_NAME_CHARS).replace(/-+$/g, "");
}

/** Assign final, UNIQUE token names: prefer the sanitized source name, else
 * `<prefix>-<n>`; disambiguate collisions with a numeric suffix. Deterministic. */
function nameTokens(cands: Cand[], prefix: string): Array<{ name: string; cand: Cand }> {
  const used = new Set<string>();
  const out: Array<{ name: string; cand: Cand }> = [];
  let ordinal = 0;
  for (const cand of cands) {
    ordinal += 1;
    let base = sanitizeName(cand.name);
    if (!base) base = `${prefix}-${ordinal}`;
    let name = base;
    let n = 2;
    while (used.has(name)) name = `${base}-${n++}`;
    used.add(name);
    out.push({ name, cand });
  }
  return out;
}

// --- color extraction --------------------------------------------------------------

// A hex color: 3/4/6/8 hex digits. We require a CSS-ish left boundary (see callers)
// so a 6-hex substring of a git sha / id is not mistaken for a color.
const HEX = /#([0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})\b/g;
// Functional color notations. `oklch`/`oklab`/`color()` included for modern targets.
const FUNC_COLOR =
  /\b(rgba?|hsla?|hwb|oklch|oklab|lab|lch|color)\(\s*[^)]{1,80}\)/gi;
// A CSS custom property: `--name: <value>;`
const CSS_VAR = /(--[A-Za-z0-9_-]+)\s*:\s*([^;{}]+?)\s*(?:;|}|$)/g;
// A SCSS/Less variable: `$name: <value>;` / `@name: <value>;`
const SCSS_VAR = /([$@][A-Za-z0-9_-]+)\s*:\s*([^;{}]+?)\s*(?:;|!|}|$)/g;
// A tailwind-style object key mapping to a quoted color string:  brand: '#4f46e5'
const TW_COLOR_ENTRY =
  /['"]?([A-Za-z0-9_-]+)['"]?\s*:\s*['"](#(?:[0-9a-fA-F]{3,8})|(?:rgba?|hsla?|oklch|oklab)\([^)]*\))['"]/g;

/** True when a string LOOKS like a CSS color value (so we don't capture a key/word). */
function looksLikeColorValue(v: string): boolean {
  const t = v.trim();
  if (/^#([0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/.test(t)) return true;
  if (/^(rgba?|hsla?|hwb|oklch|oklab|lab|lch|color)\(/i.test(t)) return true;
  return false;
}

/** Normalize a color for de-dupe: lowercase; expand #RGB(A) shorthand to #RRGGBB(AA)
 * so `#fff` and `#ffffff` collapse to one token. Functional notations are lowercased
 * and whitespace-squeezed. The STORED `$value` keeps a tidy normalized form. */
function normalizeColor(raw: string): string {
  let v = raw.trim().toLowerCase().replace(/\s+/g, " ");
  const hex = v.match(/^#([0-9a-f]{3,4})$/);
  if (hex) {
    v = "#" + hex[1].split("").map((c) => c + c).join("");
  }
  // Squeeze the space after commas in functional notations for a stable value.
  v = v.replace(/\(\s+/g, "(").replace(/\s+\)/g, ")").replace(/\s*,\s*/g, ", ");
  return v;
}

// --- typography / dimension extraction ---------------------------------------------

const FONT_FAMILY =
  /font-family\s*:\s*([^;{}]+?)\s*(?:;|}|$)/gi;
const TW_FONT_ENTRY =
  /['"]?([A-Za-z0-9_-]+)['"]?\s*:\s*\[?\s*['"]([^'"]{1,120})['"]/g; // inside fontFamily blocks
const BORDER_RADIUS =
  /border-radius\s*:\s*([^;{}]+?)\s*(?:;|}|$)/gi;
// A single CSS length token (px/rem/em/%/ch/vw/vh or unitless 0).
const LENGTH = /^(0|[0-9]*\.?[0-9]+(px|rem|em|%|ch|vw|vh|pt))$/;

/** A dimension value we accept for radius/spacing: a single length, or `9999px`,
 * or a multi-value shorthand we reduce to its FIRST length. Returns "" when not a
 * clean dimension (so we never store `calc(...)` or a color). */
function asDimension(raw: string): string {
  const first = raw.trim().split(/\s+/)[0]?.trim() ?? "";
  if (first === "0") return "0";
  if (LENGTH.test(first)) return first;
  return "";
}

/**
 * Extract a DTCG token document from Oracle chunks. PURE. Returns `{}` when nothing
 * design-like is found. The returned document validates against engine/tokens.ts and
 * its color tokens feed the Oracle popover swatches.
 */
export function extractTokensFromChunks(
  chunks: DesignContextChunk[],
): DesignTokensDoc {
  const colors: Bag = new Map();
  const fonts: Bag = new Map();
  const radii: Bag = new Map();
  const spacing: Bag = new Map();
  let seq = 0;
  let scanned = 0;

  const addColor = (rawValue: string, name?: string) => {
    if (!looksLikeColorValue(rawValue)) return;
    const norm = normalizeColor(rawValue);
    bump(colors, norm, norm, seq++, name);
  };

  for (const chunk of chunks) {
    if (!chunk || typeof chunk.text !== "string") continue;
    let text = chunk.text;
    if (scanned + text.length > MAX_SCAN_CHARS) {
      text = text.slice(0, Math.max(0, MAX_SCAN_CHARS - scanned));
    }
    scanned += text.length;
    if (text.length === 0) break;
    // Per-line cap BEFORE any regex runs (cheap guard vs. minified single-line input).
    text = capLineWidths(text);

    // 1) CSS custom properties: name + value. The value may itself be a color, a
    //    length, or a font stack — route by shape. This is our richest NAMED source.
    for (const m of text.matchAll(CSS_VAR)) {
      const name = m[1];
      const value = m[2].trim();
      if (looksLikeColorValue(value)) {
        addColor(value, name);
      } else {
        const lname = name.toLowerCase();
        const dim = asDimension(value);
        if (dim) {
          if (/(radius|round)/.test(lname)) bump(radii, dim, dim, seq++, name);
          else if (/(space|spacing|gap|size|gutter|pad|margin)/.test(lname))
            bump(spacing, dim, dim, seq++, name);
        } else if (/font/.test(lname) && /[A-Za-z]/.test(value)) {
          const fam = value.replace(/^["'`]|["'`]$/g, "").trim();
          if (fam) bump(fonts, fam.toLowerCase(), fam, seq++, name);
        }
      }
    }

    // 2) SCSS/Less variables: `$brand: #...;` (colors + dimensions only).
    for (const m of text.matchAll(SCSS_VAR)) {
      const name = m[1];
      const value = m[2].trim();
      if (looksLikeColorValue(value)) addColor(value, name);
    }

    // 3) Tailwind-style color entries inside a theme object: `brand: '#4f46e5'`.
    for (const m of text.matchAll(TW_COLOR_ENTRY)) {
      addColor(m[2], m[1]);
    }

    // 4) Bare CSS color DECLARATIONS: `color: #...;` / `background: rgb(...)`. We only
    //    take a color that follows a `:` (a declaration), never a loose hex elsewhere.
    for (const m of text.matchAll(/[A-Za-z-]+\s*:\s*([^;{}]+?)\s*(?:;|}|$)/g)) {
      const value = m[1].trim();
      // Pull any hex/functional color OUT of the (possibly compound) value, but only
      // because we are already inside a `prop: value` declaration context.
      for (const hx of value.matchAll(HEX)) addColor(hx[0]);
      for (const fn of value.matchAll(FUNC_COLOR)) addColor(fn[0]);
    }

    // 5) font-family declarations -> the first family name in the stack.
    for (const m of text.matchAll(FONT_FAMILY)) {
      const stack = m[1].trim();
      const first = stack
        .split(",")[0]
        ?.replace(/^["'`]|["'`]$/g, "")
        .trim();
      if (first && /[A-Za-z]/.test(first)) {
        bump(fonts, first.toLowerCase(), first, seq++);
      }
    }
    // tailwind fontFamily entries (quoted family names).
    if (/fontFamily/i.test(text)) {
      for (const m of text.matchAll(TW_FONT_ENTRY)) {
        const fam = m[2].trim();
        if (fam && /[A-Za-z]/.test(fam) && !looksLikeColorValue(fam)) {
          bump(fonts, fam.toLowerCase(), fam, seq++, m[1]);
        }
      }
    }

    // 6) border-radius declarations -> dimension.
    for (const m of text.matchAll(BORDER_RADIUS)) {
      const dim = asDimension(m[1]);
      if (dim) bump(radii, dim, dim, seq++);
    }

    // 7) tailwind dimension object blocks: `borderRadius: { card: '12px' }` and
    //    `spacing: { 4: '16px' }`. Scan the object body that follows the key for
    //    `name: 'dimension'` entries (named, so the token keeps the tailwind key).
    for (const m of text.matchAll(/\b(borderRadius|spacing)\s*:\s*\{([^}]*)\}/g)) {
      const into = m[1] === "borderRadius" ? radii : spacing;
      for (const e of m[2].matchAll(
        /['"]?([A-Za-z0-9_-]+)['"]?\s*:\s*['"]([^'"]+)['"]/g,
      )) {
        const dim = asDimension(e[2]);
        if (dim) bump(into, dim, dim, seq++, e[1]);
      }
    }
  }

  const doc: DtcgDocument = {};

  const colorList = ordered(colors, MAX_COLORS);
  if (colorList.length > 0) {
    const group: DtcgDocument = {};
    for (const { name, cand } of nameTokens(colorList, "color")) {
      group[name] = { $value: cand.value, $type: "color" };
    }
    doc.color = group;
  }

  const fontList = ordered(fonts, MAX_FONTS);
  if (fontList.length > 0) {
    const group: DtcgDocument = {};
    for (const { name, cand } of nameTokens(fontList, "font")) {
      group[name] = { $value: cand.value, $type: "fontFamily" };
    }
    doc.typography = group;
  }

  const spacingList = ordered(spacing, MAX_SPACING);
  if (spacingList.length > 0) {
    const group: DtcgDocument = {};
    for (const { name, cand } of nameTokens(spacingList, "space")) {
      group[name] = { $value: cand.value, $type: "dimension" };
    }
    doc.spacing = group;
  }

  const radiusList = ordered(radii, MAX_RADII);
  if (radiusList.length > 0) {
    const group: DtcgDocument = {};
    for (const { name, cand } of nameTokens(radiusList, "radius")) {
      group[name] = { $value: cand.value, $type: "dimension" };
    }
    doc.radius = group;
  }

  return doc;
}
