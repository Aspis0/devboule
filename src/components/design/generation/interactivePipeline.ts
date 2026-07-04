// Interactive-artifact generation pipeline (Phase 2) — turn streamed model TEXT into
// ONE clean, self-contained HTML document ready to STORE at `<workingFolder>/artifact/
// index.html`. This is the SECOND output mode, parallel to the static node pipeline
// (`pipeline.ts`): an interactive artifact is a standalone document rendered inside a
// sandboxed, opaque-origin iframe served from the `artifact:` scheme.
//
// HARD INVARIANT: this path is for SCRIPT-BEARING content and therefore must NEVER touch
// the static node pipeline (`NodeContent.tsx` / `DesignCanvas.tsx` / `sanitize.ts` — the
// DOMPurify boundary). The security boundary here is the iframe sandbox (no
// `allow-same-origin`) + the per-document CSP RESPONSE HEADER set by the Rust serve
// handler (`artifact_protocol.rs`). Consequently this module:
//   - does NOT run DOMPurify (that would strip the very scripts we want to keep);
//   - does NOT inject the resize/error bridge script (the serve handler injects it,
//     idempotently, so it works regardless of the generating model's quality);
//   - does NOT inject a CSP `<meta>` tag (the serve handler owns the CSP via a header,
//     which a `<meta>` could only tighten — never loosen).
// It stores CLEAN artifact HTML only.
//
// The remote-URL NEUTRALIZATION below is GRACEFUL DEGRADATION + defense-in-depth ONLY.
// The authoritative network/exfil boundary is the served CSP header (`connect-src 'none'`,
// `img-src data:`, `font-src data:`, `script-src`/`style-src` = `'unsafe-inline'` +
// CDN allowlist). A best-effort string pass is therefore the right tool here: it makes a
// weak model's broken remote references fail quietly instead of showing console errors,
// and it never has to be a security boundary. (A future malicious doc cannot exfiltrate
// regardless, because the CSP — not this regex — is what enforces it.)
//
// PURE: string -> result. No DOM, no clock, no random, no Tauri, no DOMPurify. Provider-
// agnostic: nothing here branches on which backend produced `modelText`.

import { extractMarkup } from "./parseNodes";

/**
 * CDN origins whose `<script src>` / `<link href>` / `url()` references are KEPT (not
 * neutralized). Mirrors the Rust `ARTIFACT_CDN_ALLOWLIST` in `artifact_protocol.rs` so the
 * pipeline and the served CSP agree on which library origins are permitted.
 *
 * KEEP IN SYNC with `src-tauri/src/backend/artifact_protocol.rs`'s `ARTIFACT_CDN_ALLOWLIST`
 * (Rust side builds the CSP header). The two lists MUST be identical or artifacts silently
 * break; each side is pinned to the exact three origins by a test (the vitest below and the
 * Rust `cdn_allowlist_is_exactly_the_three_expected_origins`), so any change trips that
 * language's test and forces a conscious mirror of the other.
 */
export const ARTIFACT_CDN_ALLOWLIST = [
  "https://cdnjs.cloudflare.com",
  "https://cdn.jsdelivr.net",
  "https://unpkg.com",
] as const;

/** Options for {@link applyInteractiveGeneration}. */
export interface InteractiveGenerationOptions {
  /** CDN origins whose URLs are KEPT (not neutralized). Defaults to {@link ARTIFACT_CDN_ALLOWLIST}. */
  cdnAllowlist?: readonly string[];
  /** `lang` for the wrapped HTML shell (bare fragments only). Default `"en"`. */
  lang?: string;
}

/** Result of {@link applyInteractiveGeneration}. */
export interface InteractiveGenerationResult {
  /** Clean, ready-to-store artifact HTML — NO bridge, NO CSP meta, NO DOMPurify. */
  html: string;
  /** Non-fatal, human-readable warnings (neutralized refs, empty input). The UI surfaces these. */
  warnings: string[];
  /** How many remote `src`/`href`/`url()` references were neutralized (graceful degradation). */
  neutralizedCount: number;
  /** Whether a bare fragment was wrapped in the responsive HTML shell (vs a full document passed through). */
  wrapped: boolean;
}

/** Inert sentinel a neutralized URL is rewritten to: an empty `data:` URL loads nothing. */
const NEUTRALIZED_URL = "data:,";

/** Detect a FULL document: it carries an `<html …>`/`<html>`/`<html/>` tag (case-insensitive).
 * Anything else (a bare fragment, or empty text) is wrapped in the responsive shell. */
function isFullDocument(html: string): boolean {
  return /<html[\s/>]/i.test(html);
}

/** HTML-escape a value destined for a double-quoted attribute (the shell `lang`). */
function escapeAttr(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/**
 * Wrap a bare fragment in a MINIMAL, RESPONSIVE HTML shell so it renders correctly inside
 * the phone/browser frames (viewport meta + media-query-friendly base styles). Carries NO
 * bridge and NO CSP meta — the serve handler adds those.
 */
function wrapFragment(fragment: string, lang: string): string {
  const safeLang = escapeAttr(lang);
  return [
    "<!DOCTYPE html>",
    `<html lang="${safeLang}">`,
    "<head>",
    '<meta charset="utf-8">',
    '<meta name="viewport" content="width=device-width, initial-scale=1">',
    "<style>*,*::before,*::after{box-sizing:border-box}html,body{margin:0}body{font:16px/1.5 system-ui,-apple-system,\"Segoe UI\",Roboto,sans-serif;padding:16px}img{max-width:100%;height:auto}</style>",
    "</head>",
    "<body>",
    fragment,
    "</body>",
    "</html>",
    "",
  ].join("\n");
}

/**
 * Decide whether a single URL token must be neutralized. KEEP (return false) when it is an
 * inline `data:` URL or an allowlisted CDN origin; NEUTRALIZE (return true) when it is a
 * "remote" reference — protocol-relative (`//host/…`) or carrying an explicit non-`data:`
 * scheme that is not an allowlisted CDN. Relative paths, `#fragments`, and `?query`-only
 * values (no scheme, not `//`) are KEPT (harmless: they resolve to the artifact origin,
 * which serves nothing else, and are not exfiltration vectors).
 *
 * CDN matching is origin-exact (the prefix must be followed by `/`, `?`, `#`, or end) so a
 * look-alike host like `https://cdnjs.cloudflare.com.evil.example/x.js` is NOT mistaken for
 * the allowlisted origin — matching the CSP's host-source semantics.
 */
function shouldNeutralize(rawValue: string, allowlistLower: readonly string[]): boolean {
  const value = rawValue.trim();
  if (value === "") return false; // empty value: nothing to load
  const lower = value.toLowerCase();
  if (lower.startsWith("data:")) return false; // inline — keep
  if (value.startsWith("//")) return true; // protocol-relative — remote
  // Explicit scheme? (`https:`, `http:`, `mailto:`, `javascript:`, …)
  if (/^[a-z][a-z0-9+.\-]*:/i.test(value)) {
    for (const cdn of allowlistLower) {
      if (
        lower === cdn ||
        lower.startsWith(`${cdn}/`) ||
        lower.startsWith(`${cdn}?`) ||
        lower.startsWith(`${cdn}#`)
      ) {
        return false; // allowlisted CDN origin — keep
      }
    }
    return true; // remote, non-CDN scheme — neutralize
  }
  return false; // no scheme, not protocol-relative => relative/#/? => keep
}

/**
 * Neutralize remote references in `html`. Walks `src=`/`href=` attribute values (double-,
 * single-, and unquoted) and CSS `url(…)` tokens, replacing only the URL of a "remote"
 * reference with {@link NEUTRALIZED_URL} while preserving the surrounding syntax/quoting.
 * Returns the rewritten html + the count neutralized. Best-effort (see file header).
 */
function neutralizeRemoteRefs(
  html: string,
  allowlistLower: readonly string[],
): { html: string; count: number } {
  let count = 0;

  // src= / href= attributes. `\b` requires the attr to begin at a boundary so unrelated
  // attributes (e.g. `srcset=`) are not matched (the `\s*=` after `src` fails for `srcset`).
  const attrRe = /\b(src|href)(\s*=\s*)("([^"]*)"|'([^']*)'|([^\s"'>=`]+))/gi;
  html = html.replace(
    attrRe,
    (match, name: string, eq: string, _token: string, dq?: string, sq?: string, uq?: string) => {
      const value = dq ?? sq ?? uq ?? "";
      if (!shouldNeutralize(value, allowlistLower)) return match;
      count++;
      if (dq !== undefined) return `${name}${eq}"${NEUTRALIZED_URL}"`;
      if (sq !== undefined) return `${name}${eq}'${NEUTRALIZED_URL}'`;
      return `${name}${eq}${NEUTRALIZED_URL}`;
    },
  );

  // CSS url(...) tokens (double-, single-, unquoted).
  const urlRe = /url\(\s*("([^"]*)"|'([^']*)'|([^)'"]*))\s*\)/gi;
  html = html.replace(
    urlRe,
    (match, _inner: string, dq?: string, sq?: string, uq?: string) => {
      const value = dq ?? sq ?? uq ?? "";
      if (!shouldNeutralize(value, allowlistLower)) return match;
      count++;
      if (dq !== undefined) return `url("${NEUTRALIZED_URL}")`;
      if (sq !== undefined) return `url('${NEUTRALIZED_URL}')`;
      return `url(${NEUTRALIZED_URL})`;
    },
  );

  return { html, count };
}

/**
 * Turn raw model text into a clean, self-contained interactive artifact document.
 *
 * 1. `extractMarkup` strips ```fences``` / surrounding prose (tolerates non-string input).
 * 2. A bare fragment (no `<html>`) — or empty text — is wrapped in a minimal responsive
 *    shell; a full document is passed through as-is.
 * 3. Remote `src`/`href`/`url()` references that are not `data:` and not on the CDN
 *    allowlist are neutralized to an inert `data:,` (graceful degradation; the served CSP
 *    is the real boundary).
 *
 * Returns the clean HTML + warnings + the neutralized count + whether it was wrapped. PURE.
 */
export function applyInteractiveGeneration(
  modelText: unknown,
  opts: InteractiveGenerationOptions = {},
): InteractiveGenerationResult {
  const allowlistLower = (opts.cdnAllowlist ?? ARTIFACT_CDN_ALLOWLIST).map((s) =>
    s.toLowerCase(),
  );
  const lang = (opts.lang ?? "en").trim() || "en";
  const warnings: string[] = [];

  const extracted = extractMarkup(modelText);

  let html: string;
  let wrapped: boolean;
  if (isFullDocument(extracted)) {
    html = extracted;
    wrapped = false;
  } else {
    html = wrapFragment(extracted, lang);
    wrapped = true;
    if (extracted.trim() === "") {
      warnings.push("The model returned no usable markup; produced an empty document.");
    }
  }

  const { html: neutralized, count } = neutralizeRemoteRefs(html, allowlistLower);
  if (count > 0) {
    warnings.push(
      `Neutralized ${count} remote resource reference(s); the sandbox blocks the network — use inline data: URIs or an allowlisted CDN library.`,
    );
  }

  return { html: neutralized, warnings, neutralizedCount: count, wrapped };
}
