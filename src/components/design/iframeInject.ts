// DOM-bound canvas injection helpers (Path-B). THIN: it translates manifest +
// component markup into positioned host elements inside the iframe's
// `contentDocument`. All correctness-bearing logic lives in the PURE engine; this
// file only (a) builds style strings, (b) wraps DOMParser, and (c) idempotently
// reconciles host `<div>`s. Every markup string is routed through the single
// sanitize chokepoint before it ever reaches `innerHTML`.

import type {
  DesignNodePlacement,
  DesignProject,
} from "../../types/design";
import { sanitizeNodeMarkup } from "./sanitize";
import type { ParsedNode } from "./engine/keyedDiff";

/** The id of the canvas root element inside the iframe document. */
export const CANVAS_ROOT_ID = "canvas-root";

/** The attribute the engine uses to mark + locate a top-level node host. */
export const NODE_ID_ATTR = "data-node-id";

/**
 * The minimal iframe shell document. NO scripts, NO external resources — the
 * iframe is sandboxed WITHOUT `allow-scripts` (defense in depth), so the parent
 * reaches into `contentDocument` to populate it. `position:relative` on the root
 * makes the absolutely-positioned hosts lay out in canvas coordinates.
 */
export function buildShellHtml(): string {
  return [
    "<!DOCTYPE html>",
    '<html><head><meta charset="utf-8">',
    "<style>",
    "html,body{margin:0;padding:0;background:transparent;}",
    `#${CANVAS_ROOT_ID}{position:relative;width:100%;height:100%;}`,
    "</style></head>",
    `<body><div id="${CANVAS_ROOT_ID}"></div></body></html>`,
  ].join("");
}

/**
 * Build the inline CSS for a host `<div>` from its placement. Always absolute
 * with left/top/z-index/width. `height` is emitted ONLY for a numeric `h`; an
 * `"auto"` height is left unset so the host hugs its content (LOCKED 1.4).
 * Pure (string -> string), unit-testable.
 */
export function placementStyle(p: DesignNodePlacement): string {
  const parts = [
    "position:absolute",
    `left:${p.x}px`,
    `top:${p.y}px`,
    `z-index:${p.z}`,
    `width:${p.w}px`,
  ];
  if (typeof p.h === "number") parts.push(`height:${p.h}px`);
  return parts.join(";") + ";";
}

/**
 * Parse a sanitized markup string into the PURE `ParsedNode[]` of its TOP-LEVEL
 * elements, using the given document's DOMParser. Thin DOM wrapper: it produces
 * the plain tree the engine (`keyedDiff`) operates on without a DOM. Non-element
 * top-level nodes (stray text) are ignored.
 */
export function parseTopLevelNodes(
  markup: string,
  parser: DOMParser = new DOMParser(),
): ParsedNode[] {
  const doc = parser.parseFromString(markup, "text/html");
  const roots = Array.from(doc.body.children);
  return roots.map(toParsedNode);
}

function toParsedNode(el: Element): ParsedNode {
  const attrs: Record<string, string> = {};
  for (const a of Array.from(el.attributes)) attrs[a.name] = a.value;
  const dataNodeId = el.getAttribute(NODE_ID_ATTR) ?? undefined;
  const children = Array.from(el.children).map(toParsedNode);
  return {
    tag: el.tagName.toLowerCase(),
    dataNodeId,
    attrs,
    children,
    text: el.textContent ?? "",
  };
}

/**
 * A top-level node paired with its ORIGINAL outer markup. The pipeline needs both
 * the PURE shape (for `reanchorIds`) AND the faithful raw markup (to sanitize +
 * store without lossy re-serialization). Keeping the raw `outerHTML` preserves
 * mixed text/element content that a tree round-trip would drop.
 */
export interface ParsedTopLevelNode {
  node: ParsedNode;
  /** The element's verbatim `outerHTML` (data-node-id still as the model wrote it). */
  markup: string;
}

/**
 * Like {@link parseTopLevelNodes} but also returns each element's verbatim outer
 * markup. Used by the generation pipeline so the content stored per node is the
 * model's faithful markup (only the resolved id is rewritten downstream), not a
 * lossy re-serialization of the parsed tree.
 */
export function parseTopLevelNodesWithMarkup(
  markup: string,
  parser: DOMParser = new DOMParser(),
): ParsedTopLevelNode[] {
  const doc = parser.parseFromString(markup, "text/html");
  return Array.from(doc.body.children).map((el) => ({
    node: toParsedNode(el),
    markup: el.outerHTML,
  }));
}

/**
 * Idempotently inject/refresh every node of `project` into `doc`'s canvas root:
 *  - for each manifest id, ensure a host `<div data-node-id position:absolute>`,
 *    set its (sanitized) inner markup, and apply its placement style;
 *  - remove any host whose id is no longer in the manifest (stale).
 * Safe to call repeatedly (re-render on srcdoc reload). Returns silently if the
 * canvas root is not present yet (contentDocument not loaded).
 */
export function injectNodes(doc: Document, project: DesignProject): void {
  const root = doc.getElementById(CANVAS_ROOT_ID);
  if (!root) return; // not loaded yet — caller re-runs on `load`

  const manifest = project.manifest;
  const wanted = new Set(Object.keys(manifest.nodes));

  // Remove stale hosts first.
  for (const host of Array.from(
    root.querySelectorAll(`:scope > [${NODE_ID_ATTR}]`),
  )) {
    const id = host.getAttribute(NODE_ID_ATTR);
    if (!id || !wanted.has(id)) host.remove();
  }

  // Upsert each wanted node.
  for (const id of Object.keys(manifest.nodes)) {
    applyNode(doc, root, id, manifest.nodes[id], project.components[id] ?? "");
  }
}

/** Upsert ONE host element + its content + placement. Exposed for focused tests. */
export function applyNode(
  doc: Document,
  root: Element,
  id: string,
  placement: DesignNodePlacement,
  markup: string,
): HTMLElement {
  let host = root.querySelector<HTMLElement>(
    `:scope > [${NODE_ID_ATTR}="${cssEscapeId(id)}"]`,
  );
  if (!host) {
    host = doc.createElement("div");
    host.setAttribute(NODE_ID_ATTR, id);
    root.appendChild(host);
  }
  // Content is ALWAYS routed through the single sanitize chokepoint. Finding 12:
  // markup arriving here may already have been sanitized by the pipeline, so this
  // is a double-sanitize — kept INTENTIONALLY as defense-in-depth for markup that
  // reaches the canvas straight off disk (a load path the pipeline never touched).
  host.innerHTML = sanitizeNodeMarkup(markup);
  host.setAttribute("style", placementStyle(placement));
  return host;
}

/**
 * Apply ONLY the placement style of an existing host (the cheap drag-loop path —
 * no innerHTML, no sanitize). No-op if the host is absent. Pure DOM mutation.
 */
export function applyPlacement(
  root: Element,
  id: string,
  placement: DesignNodePlacement,
): void {
  const host = root.querySelector<HTMLElement>(
    `:scope > [${NODE_ID_ATTR}="${cssEscapeId(id)}"]`,
  );
  if (host) host.setAttribute("style", placementStyle(placement));
}

/**
 * Escape an id for safe use inside a CSS attribute selector. Node ids are already
 * charset-validated (`^[a-z0-9][a-z0-9_-]{0,63}$`) so this is belt-and-suspenders;
 * we still drop any character outside that set to guarantee a valid selector.
 */
function cssEscapeId(id: string): string {
  return id.replace(/[^a-z0-9_-]/gi, "");
}
