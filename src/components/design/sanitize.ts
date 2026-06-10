// The SINGLE sanitization chokepoint for all LLM/untrusted node markup. Every
// `innerHTML` assignment in the design module MUST route a string through
// `sanitizeNodeMarkup` first — this is the security boundary (LOCKED decision:
// Path-B iframe + DOMPurify). Defense in depth: the canvas iframe also runs
// WITHOUT `allow-scripts`, so even a sanitizer miss cannot execute script there.
//
// PURE-ish: depends only on DOMPurify (which needs a DOM). No app state, no
// network, no clock. The DOMPurify default export auto-binds to the ambient
// `window` (the browser at runtime, jsdom under test).

import DOMPurify from "dompurify";

// Tags we forbid outright regardless of profile: script/style (exfil + CSS-based
// attacks), framing/embedding (iframe/object/embed), and document-head injectors
// (link/base/meta — base-tag hijack, meta-refresh redirect, external CSS).
const FORBID_TAGS = [
  "script",
  "style",
  "iframe",
  "object",
  "embed",
  "link",
  "base",
  "meta",
  // SVG script vector + foreignObject (can host arbitrary HTML/script context).
  "foreignobject",
];

// We additionally strip every `on*` event-handler attribute. DOMPurify removes
// known handlers by default; this belt-and-suspenders forbid list covers the
// common ones explicitly so intent is clear and a profile change can't silently
// re-allow them.
const FORBID_ATTR = [
  "onerror",
  "onload",
  "onclick",
  "onmouseover",
  "onmouseenter",
  "onmouseleave",
  "onfocus",
  "onblur",
  "onanimationstart",
  "onanimationend",
  "ontransitionend",
  "onbegin",
  "onend",
  "onrepeat",
  "srcdoc",
  "formaction",
  // SVG-namespaced href: forbidden outright (a `javascript:` xlink:href on
  // <use>/<a>/<image> is never legitimate node markup). The PLAIN `href` form is
  // NOT forbidden here — that would also strip legitimate `<a href="https://…">`
  // links; instead the `uponSanitizeAttribute` hook clamps its scheme (B3) so a
  // `javascript:`/`data:`-scheme plain href on any element is neutralized while
  // safe http(s)/mailto/tel links survive.
  "xlink:href",
];

// Charset a `data-node-id` value must satisfy to be preserved — identical to the
// Rust path-confinement charset (`validate_node_id`). A non-conforming value is
// dropped (it would never be a real engine-owned id anyway).
const NODE_ID_CHARSET = /^[a-z0-9][a-z0-9_-]{0,63}$/;
const DATA_NODE_ID_ATTR = "data-node-id";

// --- class / id stripping (direct-DOM canvas isolation) --------------------
// The canvas renders sanitized node markup DIRECTLY in the parent (app) DOM, not
// inside an iframe document. A node's `class`/`id` must therefore never collide
// with — or be targeted by — the app's own selectors (Tailwind utilities, global
// ids, component styles). We strip BOTH attributes from EVERY element of the
// sanitized markup:
//   - `class`: a node's visual styling is inline `style` only; class names would
//     leak app/Tailwind selectors onto nodes (and let a node match app rules).
//   - `id`: a global duplicate id breaks `getElementById`/`label[for]` in the app
//     and an attacker-chosen id enables DOM-clobbering.
//
// SVG INTERNAL ID REFS ARE NOT SUPPORTED (deliberate). One might want to keep `id`
// inside an SVG subtree so `url(#grad)`/`clip-path`/`xlink:href="#sym"` still
// resolve — BUT the BASE_CONFIG already sets `SANITIZE_NAMED_PROPS: true`, which
// rewrites every `id` to `user-content-<id>` (anti-DOM-clobbering) WITHOUT
// rewriting the matching `url(#…)`/`href="#…"` reference. That mismatch already
// severs SVG internal refs regardless of this hook, so preserving the SVG `id`
// would buy nothing. We therefore strip `id` everywhere — simplest correct option.
// Node markup that needs gradients/clip-paths should use inline values, not
// id-referenced defs. (`data-node-id` is a data-* attr, NOT `id`, and survives.)
const CLASS_ATTR = "class";
const ID_ATTR = "id";

// Plain `href` scheme clamp (B3 — defense in depth). DOMPurify's
// `ALLOWED_URI_REGEXP` governs href in HTML, but a bare `href` on SVG
// <a>/<image>/<use> (SVG2) is not always clamped the same way across versions.
// The hook below drops a plain `href` whose value is not an allowed safe scheme
// (http(s)/mailto/tel) or a same-document fragment. Mirrors BASE_CONFIG's
// ALLOWED_URI_REGEXP so HTML and SVG hrefs share one policy.
const HREF_ATTR = "href";
const SAFE_HREF_RE = /^(?:(?:https?|mailto|tel):|#|\/(?![/\\])|\.{0,2}\/)/i;

// M2: an SVG `<image href="…">` is a RESOURCE load, not a navigation link, so the
// link policy (which permits http(s)) is wrong for it — a remote
// `href="https://evil/x.png"` is an exfil/beacon vector (it pings the attacker's
// host the moment the SVG renders) we cannot distinguish from a legit asset at this
// layer. Mirror the inline-style `url()` policy instead: only an in-document
// `data:image/` payload or a same-document `#fragment` ref is allowed on SVG
// <image> href; everything else (incl. http(s)) is dropped.
const SVG_IMAGE_TAG = "image";
const SAFE_SVG_IMAGE_HREF_RE = /^(?:data:image\/|#)/i;

// --- Inline-style neutralization -------------------------------------------
// DOMPurify's `ALLOWED_URI_REGEXP` clamps href/src-style URI attributes, but it
// does NOT inspect URIs embedded inside an inline `style` attribute. That leaves
// `style="background:url(javascript:...)"`, external-beacon `url(https://evil/x)`,
// CSS `expression(...)`, and overlay CSS (`position:fixed`) intact on a node.
// Phase 2 will feed untrusted LLM markup through this chokepoint, so we
// neutralize dangerous CSS here as the first layer.
//
// NOTE: full positional-CSS neutralization of node markup (e.g. transform-based
// escape, negative-margin overlays, viewport units) is completed in Phase 2
// (plan risk #8). This hook is the first layer, not the whole defense.
const STYLE_ATTR = "style";

// `url(...)` occurrence (single/double/unquoted), capturing the inner target.
const CSS_URL_RE = /url\(\s*(['"]?)([^)'"]*)\1\s*\)/gi;
// Schemes we permit inside an inline-style `url(...)`: ONLY in-document data
// images and same-document fragment refs (e.g. `url(#clip)` for SVG). An external
// `url(https://host/...)` is an exfil/beacon vector we cannot distinguish from a
// legit asset at this layer, so it is NOT allowed (W1). Everything else (http:,
// https:, javascript:, vbscript:, data:text/html, protocol-relative //host, bare
// external paths) is replaced with `url(about:blank)`.
const SAFE_CSS_URL_RE = /^(?:data:image\/|#)/i;
// `position:fixed` / `position:sticky` — a node must not escape its host div to
// overlay the canvas. The host owns placement; strip these declarations. W7: also
// tolerate an `!important` qualifier (with arbitrary surrounding whitespace) so
// `position: fixed !important` cannot slip past the strip and override the host's
// containment.
const DANGEROUS_POSITION_RE =
  /position\s*:\s*(?:fixed|sticky)(?:\s*!\s*important)?\s*;?/gi;
// CSS `expression(...)` (legacy IE dynamic-property XSS); kill it wholesale.
const CSS_EXPRESSION_RE = /expression\s*\(/gi;

/**
 * Neutralize dangerous declarations inside an inline `style` value while keeping
 * legitimate layout CSS (flex/gap/color/etc.). Pure (string -> string).
 */
function sanitizeStyleValue(value: string): string {
  let out = value.replace(CSS_URL_RE, (_match, _quote, target: string) => {
    const t = target.trim();
    return SAFE_CSS_URL_RE.test(t) ? `url(${t})` : "url(about:blank)";
  });
  out = out.replace(DANGEROUS_POSITION_RE, "");
  // After url() rewriting any residual `expression(` is dead, but strip it too so
  // the literal token never survives into the output.
  out = out.replace(CSS_EXPRESSION_RE, "(");
  return out;
}

// `ALLOW_DATA_ATTR: false` strips ALL data-* attributes (a common exfil/marker
// vector). We re-allow EXACTLY `data-node-id` — and only when its value matches
// the strict id charset — via a single `uponSanitizeAttribute` hook below. A bare
// `ADD_ATTR` is insufficient because the `ALLOW_DATA_ATTR` flag governs the whole
// `data-` prefix; the hook is the documented way to keep one specific data-attr.
//
// NOTE: `DOMPurify` is a process-wide SINGLETON (the default export binds to the
// ambient window). `addHook` MUTATES that singleton, so this must be idempotent:
// the `hookInstalled` module flag guarantees the hook is registered exactly once
// per isolate, no matter how many times `sanitizeNodeMarkup` is called.
let hookInstalled = false;
function ensureNodeIdHook(): void {
  if (hookInstalled) return;
  // Mark installed BEFORE registering so a re-entrant call (hook bodies never call
  // back here, but defense in depth) cannot double-register the same hook.
  hookInstalled = true;
  DOMPurify.addHook("uponSanitizeAttribute", (node, data) => {
    if (data.attrName === DATA_NODE_ID_ATTR) {
      // Keep only a well-formed id; drop anything else (and never force-keep an
      // attacker-controlled arbitrary data-node-id value).
      data.forceKeepAttr = NODE_ID_CHARSET.test(data.attrValue ?? "");
      return;
    }
    if (data.attrName === HREF_ATTR && typeof data.attrValue === "string") {
      const value = data.attrValue.trim();
      // M2: an SVG <image> href is a RESOURCE fetch — apply the stricter
      // image-url policy (data:image/ or #fragment only) so a remote beacon
      // `href="https://evil/x.png"` is dropped while a link's http(s) href is not.
      // `nodeName` for an SVG element is its lowercase local name in jsdom; lowercase
      // defensively for the browser path.
      const tag = (node as Element)?.nodeName?.toLowerCase?.() ?? "";
      if (tag === SVG_IMAGE_TAG) {
        if (!SAFE_SVG_IMAGE_HREF_RE.test(value)) data.keepAttr = false;
        return;
      }
      // Clamp the scheme of a PLAIN `href` (HTML <a> and SVG <a>/<use>). Anything
      // that is not an allowed safe scheme/fragment/relative path is dropped so
      // `javascript:`/`data:`/`vbscript:` can never survive on href.
      if (!SAFE_HREF_RE.test(value)) {
        data.keepAttr = false;
      }
      return;
    }
    if (data.attrName === STYLE_ATTR && typeof data.attrValue === "string") {
      // DOMPurify does not URI-check inline style; neutralize dangerous CSS in
      // place (url() schemes, position:fixed/sticky overlays, expression()).
      data.attrValue = sanitizeStyleValue(data.attrValue);
    }
  });
  // Strip `class` and `id` from every element (see the CLASS_ATTR/ID_ATTR
  // rationale above). `afterSanitizeAttributes` runs once per element with the
  // real node in hand, AFTER SANITIZE_NAMED_PROPS may have rewritten `id` to
  // `user-content-…`; removing it here drops both the original and any rewritten
  // form. `data-node-id` is a data-* attribute, not `id`, so it is untouched.
  DOMPurify.addHook("afterSanitizeAttributes", (node) => {
    // Only elements carry attributes; guard for the DOM type.
    const el = node as Element;
    if (typeof el.removeAttribute !== "function" || el.attributes == null) return;
    // M3: SVG (and foreign-content) attribute names are CASE-SENSITIVE in the DOM,
    // so `CLASS`/`ID`/`Class` on an SVG element are DISTINCT attributes that a
    // case-sensitive `hasAttribute("class")` misses entirely — they would survive.
    // Compare by lowercased name instead, and remove EVERY attribute whose name is
    // `class` or `id` in any case. Collect names first: removeAttribute mutates the
    // live `attributes` NamedNodeMap, so iterating + removing in one pass skips
    // entries. `data-node-id` is a data-* attr (not `class`/`id`) and is untouched.
    const toRemove: string[] = [];
    for (let i = 0; i < el.attributes.length; i += 1) {
      const name = el.attributes[i]?.name ?? "";
      const lower = name.toLowerCase();
      if (lower === CLASS_ATTR || lower === ID_ATTR) toRemove.push(name);
    }
    for (const name of toRemove) el.removeAttribute(name);
  });
}

// Shared base config. `data-node-id` survival is handled by the hook above.
const BASE_CONFIG = {
  FORBID_TAGS,
  FORBID_ATTR,
  ALLOW_DATA_ATTR: false,
  // Only http(s), mailto, tel are safe link/resource schemes. This clamp blocks
  // `javascript:`, `data:` (data:text/html mutation-XSS), `vbscript:`, etc.
  ALLOWED_URI_REGEXP: /^(?:https?|mailto|tel):/i,
  // Keep markup only (no full documents); never return a Node.
  RETURN_DOM: false,
  RETURN_DOM_FRAGMENT: false,
  // Block DOM-clobbering of id/name handles (mutation-XSS hardening).
  SANITIZE_DOM: true,
  SANITIZE_NAMED_PROPS: true,
} as const;

/**
 * Sanitize one node's inner markup (HTML or SVG). The same strict policy applies
 * to both kinds: SVG is allowed (USE_PROFILES.svg) but `<script>`/`foreignObject`
 * inside SVG are forbidden by FORBID_TAGS above. Returns a safe HTML string ready
 * for `innerHTML`. Total: a non-string or empty input returns "".
 */
export function sanitizeNodeMarkup(markup: unknown): string {
  if (typeof markup !== "string" || markup.length === 0) return "";
  ensureNodeIdHook();
  const clean = DOMPurify.sanitize(markup, {
    ...BASE_CONFIG,
    // Allow HTML + SVG (+ svgFilters) profiles; MathML is intentionally excluded.
    USE_PROFILES: { html: true, svg: true, svgFilters: true },
  });
  // DOMPurify returns a string when RETURN_DOM is false; coerce defensively.
  return typeof clean === "string" ? clean : String(clean);
}
