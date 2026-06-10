// Top-level markup parsing for the generation pipeline. (Formerly also hosted the
// Path-B iframe shell + DOM injection; that half was retired when the canvas moved
// to direct-DOM rendering — see `canvas/DesignCanvas.tsx`. The filename is kept to
// minimize import churn in the generation pipeline, which depends on the survivors
// below.)
//
// THIN DOM wrapper: it parses a sanitized markup string into the PURE
// `ParsedNode[]` of its top-level elements (and, optionally, each element's
// verbatim outer markup). All correctness-bearing reconciliation lives in the pure
// engine (`keyedDiff`); this file only wraps `DOMParser`.

import type { ParsedNode } from "./engine/keyedDiff";

/** The attribute the engine uses to mark + locate a top-level node host. */
export const NODE_ID_ATTR = "data-node-id";

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
