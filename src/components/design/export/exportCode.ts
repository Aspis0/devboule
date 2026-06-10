// Export-to-code — PURE assembler (Phase 2 STEP 4).
//
// Turns a DesignProject (manifest placement + already-sanitized per-node markup)
// into a standalone HTML document. Two modes:
//   - "absolute": each node is an absolutely-positioned host div at its manifest
//     {x,y,z,w,h}, reproducing the canvas layout pixel-for-pixel.
//   - "flow": a vertical flex scaffold in `nodeOrder`, ignoring coordinates (a
//     responsive, source-friendly export).
//
// PURE: no DOM, no network, no clock, no random — a deterministic
// project+mode -> string function (snapshot-tested). The component markup is
// ALREADY sanitized (the pipeline's single DOMPurify chokepoint ran before it was
// stored), so this exporter NEVER introduces unsanitized content: it only escapes
// attribute-context values it itself emits (ids) and inlines the stored markup
// verbatim. It does NOT re-open the markup to a parser, so it cannot widen the
// attack surface.

import type {
  DesignNodeHeight,
  DesignNodePlacement,
  DesignProject,
} from "../../../types/design";

/** Export layout mode. */
export type ExportMode = "absolute" | "flow";

/** Escape a string for safe insertion into a double-quoted HTML attribute value.
 * Used only for values WE emit (the node id, which is already charset-validated by
 * the backend, but we escape defensively). */
function escapeAttr(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/"/g, "&quot;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

/** Resolve a manifest height to a CSS length (`"auto"` -> `auto`, number -> `Npx`). */
function cssHeight(h: DesignNodeHeight): string {
  return typeof h === "number" ? `${h}px` : "auto";
}

/** The order to emit nodes: explicit `nodeOrder` filtered to ids that actually have
 * a manifest entry, then any manifest ids missing from the order (deterministic,
 * key-sorted) so nothing is silently dropped. */
function emitOrder(project: DesignProject): string[] {
  const nodes = project.manifest.nodes;
  const seen = new Set<string>();
  const order: string[] = [];
  for (const id of project.meta.nodeOrder) {
    if (nodes[id] && !seen.has(id)) {
      order.push(id);
      seen.add(id);
    }
  }
  for (const id of Object.keys(nodes).sort()) {
    if (!seen.has(id)) {
      order.push(id);
      seen.add(id);
    }
  }
  return order;
}

/** Build the absolute-positioned host div for one node. */
function absoluteHost(
  id: string,
  p: DesignNodePlacement,
  markup: string,
): string {
  const style = [
    "position:absolute",
    `left:${p.x}px`,
    `top:${p.y}px`,
    `z-index:${p.z}`,
    `width:${p.w}px`,
    `height:${cssHeight(p.h)}`,
  ].join(";");
  return (
    `<div data-node-id="${escapeAttr(id)}" style="${style}">` +
    `${markup}` +
    `</div>`
  );
}

/** Build the flow (flex) host div for one node — width preserved, position dropped. */
function flowHost(id: string, p: DesignNodePlacement, markup: string): string {
  const style = [`width:${p.w}px`, "max-width:100%"].join(";");
  return (
    `<div data-node-id="${escapeAttr(id)}" style="${style}">` +
    `${markup}` +
    `</div>`
  );
}

/** Wrap the assembled body in a standalone HTML document. The title is escaped. */
function htmlDocument(title: string, containerStyle: string, body: string): string {
  return [
    "<!doctype html>",
    '<html lang="en">',
    "<head>",
    '<meta charset="utf-8">',
    '<meta name="viewport" content="width=device-width, initial-scale=1">',
    `<title>${escapeAttr(title)}</title>`,
    "</head>",
    "<body>",
    `<div style="${containerStyle}">`,
    body,
    "</div>",
    "</body>",
    "</html>",
  ].join("\n");
}

/**
 * Assemble a standalone HTML export of `project` in the given `mode`. PURE.
 *
 * - "absolute": a `position:relative` canvas sized to the project canvas, with each
 *   node absolutely positioned at its manifest rect.
 * - "flow": a column flex container, nodes stacked in `nodeOrder`.
 *
 * Nodes whose markup is missing are skipped (a manifest entry without stored markup
 * — tolerated, mirroring the load path). The stored markup is inlined verbatim
 * (already sanitized upstream).
 */
export function exportCode(project: DesignProject, mode: ExportMode): string {
  const order = emitOrder(project);
  const nodes = project.manifest.nodes;
  const components = project.components;
  const title = project.meta.name || "Design export";

  const hosts: string[] = [];
  for (const id of order) {
    const placement = nodes[id];
    const markup = components[id];
    // Skip ids with no stored markup (tolerated, like the loader).
    if (!placement || typeof markup !== "string" || markup.length === 0) continue;
    hosts.push(
      mode === "absolute"
        ? absoluteHost(id, placement, markup)
        : flowHost(id, placement, markup),
    );
  }
  const body = hosts.join("\n");

  if (mode === "absolute") {
    const canvas = project.meta.canvas;
    const containerStyle = [
      "position:relative",
      `width:${canvas.w}px`,
      `height:${canvas.h}px`,
      "margin:0 auto",
    ].join(";");
    return htmlDocument(title, containerStyle, body);
  }

  // flow
  const containerStyle = [
    "display:flex",
    "flex-direction:column",
    "gap:24px",
    "padding:24px",
    "max-width:1200px",
    "margin:0 auto",
  ].join(";");
  return htmlDocument(title, containerStyle, body);
}
