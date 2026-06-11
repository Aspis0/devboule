// Pure formatting helpers for the design shell chrome (TopBar / popovers).
// NO DOM, NO clock of their own — the "now" is injected so they stay testable and
// deterministic. Used by OraclePopover (relative sync time + thousands separators)
// and elsewhere in the shell.

import { isDtcgToken, type DtcgDocument } from "../engine/tokens";

/** Format an integer with thousands separators (locale-stable grouping by 3).
 * Non-finite / negative-safe: NaN/Infinity yield "0". Used for the Oracle
 * chunk/file stats so "1284" reads as "1,284". */
export function formatThousands(n: number | undefined | null): string {
  if (n == null || !Number.isFinite(n)) return "0";
  const sign = n < 0 ? "-" : "";
  const digits = Math.abs(Math.trunc(n)).toString();
  // Insert a comma every 3 digits from the right (no Intl dependency so the output
  // is identical across environments / test runners).
  const grouped = digits.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return sign + grouped;
}

/**
 * Render an ISO-8601 instant as a short relative label vs `nowMs` ("just now",
 * "2m ago", "3h ago", "5d ago", or a date for older). PURE: the reference time is
 * passed in. An empty / unparseable timestamp yields "never" (no "Invalid Date"
 * leak). Future instants (clock skew) collapse to "just now".
 */
export function relativeTime(iso: string | undefined | null, nowMs: number): string {
  if (!iso) return "never";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "never";
  const diffMs = nowMs - t;
  if (diffMs < 0) return "just now";
  const sec = Math.floor(diffMs / 1000);
  if (sec < 45) return "just now";
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(hr / 24);
  if (day < 7) return `${day}d ago`;
  // Older than a week: a stable short date (no time-of-day to avoid TZ churn).
  return new Date(t).toLocaleDateString();
}

/**
 * Extract up to `max` color token `$value` strings from a DTCG document, in stable
 * (depth-first, key-sorted) order, for the Oracle popover swatch strip. PURE. Only
 * tokens whose `$type === "color"` AND whose `$value` is a plain non-empty string
 * (a CSS color literal) are taken — composite/object values and references are
 * skipped (we never coerce `[object Object]` or an unresolved `{ref}` into a swatch
 * background). Returns `[]` for an invalid/empty document.
 */
export function colorSwatches(doc: unknown, max = 4): string[] {
  if (doc == null || typeof doc !== "object" || Array.isArray(doc)) return [];
  const out: string[] = [];
  const MAX_DEPTH = 16;
  const walk = (node: Record<string, unknown>, depth: number) => {
    if (depth > MAX_DEPTH || out.length >= max) return;
    const keys = Object.keys(node)
      .filter((k) => !k.startsWith("$"))
      .sort();
    for (const key of keys) {
      if (out.length >= max) return;
      const value = node[key];
      if (value == null || typeof value !== "object" || Array.isArray(value)) continue;
      if (isDtcgToken(value)) {
        const tok = value as { $value: unknown; $type?: string };
        if (
          tok.$type === "color" &&
          typeof tok.$value === "string" &&
          tok.$value.trim().length > 0
        ) {
          out.push(tok.$value.trim());
        }
      } else {
        walk(value as Record<string, unknown>, depth + 1);
      }
    }
  };
  walk(doc as DtcgDocument as Record<string, unknown>, 0);
  return out;
}

/**
 * A deterministic pastel color block derived from an id, used as the project
 * thumbnail fallback when no `thumbnailPath` image exists (mirrors the prototype's
 * per-project `color`). PURE: same id -> same color. HSL with a fixed S/L in the
 * muted-olive/cream family so it sits in the Devboule palette.
 */
export function thumbColorFromId(id: string): string {
  let hash = 0;
  for (let i = 0; i < id.length; i++) {
    hash = (hash * 31 + id.charCodeAt(i)) | 0;
  }
  const hue = Math.abs(hash) % 360;
  return `hsl(${hue} 32% 84%)`;
}
