// Generation pipeline: turn streamed model TEXT into placed, sanitized canvas
// nodes (full generation) + the single-node content-edit round-trip (Phase 2
// STEP 3). DETERMINISTIC and PURE on plain data — the DOM parser is injectable so
// the reconciliation matrix is unit-testable in a node environment without jsdom.
//
// Data flow (full generation):
//   modelText
//     -> extractMarkup            (strip fences/prose)
//     -> parse                    (top-level {node, markup}[]; parser injectable)
//     -> reanchorIds              (THE crux: survivors keep ids, new ids minted)
//     -> build next manifest      (survivors keep {x,y,z,w,h}; new -> default grid)
//     -> applyNodeId + sanitize   (stamp the RESOLVED id, single sanitize chokepoint)
//   => { project, newIds, shapes }
//
// `shapes` is the per-id structural shape to PERSIST in memory so the NEXT
// regeneration can structurally re-anchor dropped/renamed ids (the `prevShapes`
// arg of reanchorIds). It is NOT written to the on-disk DesignProject (which
// mirrors the Rust wire format); the caller keeps it alongside the project.
//
// Authority split (LOCKED 1.1): the model owns CONTENT, the manifest owns
// PLACEMENT. We NEVER trust a model-reported id (reanchorIds owns id assignment)
// and NEVER read coordinates from the markup (the host owns placement).

import type {
  DesignNodeKind,
  DesignNodePlacement,
  DesignProject,
} from "../../../types/design";
import { reanchorIds, type ParsedNode } from "../engine/keyedDiff";
import { snapToGrid } from "../engine/snap";
import {
  parseTopLevelNodesWithMarkup,
  type ParsedTopLevelNode,
} from "../iframeInject";
import { sanitizeNodeMarkup } from "../sanitize";
import { autoFixNodeMarkup, type Violation } from "./contractValidator";
import { extractMarkup } from "./parseNodes";

/** A parse fn producing top-level {node, markup} pairs from a markup fragment.
 * Injectable so pipeline reconciliation is testable without a DOM. */
export type ParseFn = (markup: string) => ParsedTopLevelNode[];

/**
 * Hard cap on top-level nodes processed per generation (WARNING 7). A pathological
 * model response (or a runaway repair) could emit thousands of siblings, growing
 * `warnings`/`remainingViolations` and the manifest unboundedly. We process the
 * first N and drop the rest with ONE aggregated warning. 50 is far above any real
 * page's top-level component count.
 */
const MAX_TOP_LEVEL_NODES = 50;

/** Default-placement tuning (deterministic — NO random/clock). */
const DEFAULT_W = 360;
/** Vertical step between auto-placed rows (visual gap; height is "auto"). */
const DEFAULT_ROW_STEP = 240;
/** Horizontal gap between auto-placed columns. */
const DEFAULT_COL_GAP = 40;
/** Columns in the auto-placement grid before wrapping to a new row. */
const DEFAULT_COLS = 3;
/** Canvas margin for the first auto-placed node. */
const DEFAULT_MARGIN = 40;

/** Map of node id -> its parsed structural shape, persisted across regens. */
export type ShapeMap = Record<string, ParsedNode>;

/** Result of a full generation. */
export interface GenerationResult {
  /** The new project (manifest + sanitized components + meta.nodeOrder). */
  project: DesignProject;
  /** Ids freshly minted this generation (no prior placement). */
  newIds: string[];
  /** id -> structural shape, to persist for the NEXT regeneration's re-anchor. */
  shapes: ShapeMap;
  /**
   * Human-readable warnings for nodes DROPPED by the Tier-1 contract guard
   * (foster-parented / empty roots that cannot be salvaged). One per dropped node;
   * empty when every parsed node passed or was auto-fixed. The UI surfaces these.
   */
  warnings: string[];
  /**
   * The UNFIXABLE (`remaining`) violations aggregated across all dropped nodes,
   * for the bounded self-repair loop's instruction builder. Empty when nothing was
   * dropped. (Auto-FIXED violations are silent — the markup was corrected.)
   */
  remainingViolations: Violation[];
}

/** Result of a single-node {@link applyEdit}. */
export interface EditResult {
  /** The project AFTER the edit (unchanged reference when `changed` is false). */
  project: DesignProject;
  /**
   * True when the edit was actually applied (markup swapped). False when the edit
   * was a NO-OP — unknown id, nothing parseable, or an unfixable/empty/foster root.
   * The caller MUST NOT persist or report "Updated" when this is false (WARNING 5).
   */
  changed: boolean;
  /**
   * Human-readable warnings about the edit (e.g. a collapsed multi-element edit, or
   * the reason a no-op occurred). The UI surfaces these. Empty on a clean swap.
   */
  warnings: string[];
}

/** Options for `applyGeneration` / `applyEdit`. */
export interface PipelineOptions {
  /** DOM parse fn (defaults to DOMParser-backed parseTopLevelNodesWithMarkup). */
  parse?: ParseFn;
  /**
   * Previous per-id structural shapes (from the last generation result), enabling
   * structural recovery of dropped/renamed ids. Optional; without it only exact-
   * id survivors are recognized.
   */
  prevShapes?: ShapeMap;
}

/** Infer a node's sanitizer profile from its tag (svg root -> "svg"). */
function nodeKind(node: ParsedNode): DesignNodeKind {
  return node.tag === "svg" ? "svg" : "html";
}

/**
 * Deterministic default placement for the `index`-th NEWLY-minted node. Lays new
 * nodes out in a grid starting at `baseY` (below existing content), snapped to the
 * canvas grid. NO Math.random / Date.now — purely a function of the index.
 */
function defaultPlacement(
  index: number,
  baseY: number,
  z: number,
  grid: number,
  kind: DesignNodeKind,
): DesignNodePlacement {
  const col = index % DEFAULT_COLS;
  const row = Math.floor(index / DEFAULT_COLS);
  const x = DEFAULT_MARGIN + col * (DEFAULT_W + DEFAULT_COL_GAP);
  const y = baseY + row * DEFAULT_ROW_STEP;
  return {
    x: snapToGrid(x, grid),
    y: snapToGrid(y, grid),
    z,
    w: DEFAULT_W,
    h: "auto",
    kind,
  };
}

/**
 * The baseline Y for new auto-placed nodes: just below the lowest existing node
 * (numeric heights add their height; "auto" heights contribute a nominal step so
 * stacking still descends), or the canvas margin when there are no survivors.
 */
function baselineY(
  survivors: Record<string, DesignNodePlacement>,
  grid: number,
): number {
  let maxBottom = -Infinity;
  for (const p of Object.values(survivors)) {
    const h = typeof p.h === "number" ? p.h : DEFAULT_ROW_STEP;
    const bottom = p.y + h;
    if (bottom > maxBottom) maxBottom = bottom;
  }
  if (maxBottom === -Infinity) return snapToGrid(DEFAULT_MARGIN, grid);
  return snapToGrid(maxBottom + DEFAULT_COL_GAP, grid);
}

/** The current top z across a manifest (0 when empty). New nodes stack above. */
function topZ(nodes: Record<string, DesignNodePlacement>): number {
  let maxZ = 0;
  for (const p of Object.values(nodes)) if (p.z > maxZ) maxZ = p.z;
  return maxZ;
}

/**
 * Stamp the RESOLVED data-node-id onto a single top-level element's markup,
 * replacing any model-written value (the deterministic layer owns ids, 1.6).
 *
 * DOM-based (via DOMParser) rather than string surgery: the pipeline already
 * needs a DOM for `sanitizeNodeMarkup` (DOMPurify), so parsing here is free and
 * AVOIDS the fragility of regex against attribute values that contain `>`/`<` or
 * quoting edge cases. Falls back to the original markup if no element is found.
 * The result is re-sanitized downstream, so this never has to be a security
 * boundary — only an id-stamp.
 */
export function applyNodeId(markup: string, id: string): string {
  const doc = new DOMParser().parseFromString(markup, "text/html");
  const el = doc.body.firstElementChild;
  // Foster-parented roots (a bare `<tr>`/`<td>`...) are hoisted out of <body> by
  // the HTML parser, so firstElementChild is null/wrong. Those are detected and
  // DROPPED upstream by the Tier-1 contract guard (Finding 9), so by the time we
  // stamp an id here the markup is a real free-standing element. Defensive: if no
  // element is found, return the markup unchanged (caller already validated it).
  if (!el) return markup;
  el.setAttribute("data-node-id", id);
  // Finding 8 (positional-CSS strip on the root) is now handled deterministically
  // by `autoFixNodeMarkup`, which runs BEFORE this stamp (in the parse loop) and
  // before sanitize. We do NOT strip here so id-stamping stays a single, focused
  // concern.
  return el.outerHTML;
}

/**
 * FULL GENERATION. Reconcile freshly generated markup against the previous
 * project: survivors keep their placement, genuinely new nodes get deterministic
 * default placement, removed nodes are dropped. Returns the new project, the set
 * of newly-minted ids, and the per-id structural shapes to persist.
 */
export function applyGeneration(
  prevProject: DesignProject,
  modelText: string,
  opts: PipelineOptions = {},
): GenerationResult {
  const parse = opts.parse ?? parseTopLevelNodesWithMarkup;
  const grid = prevProject.meta.canvas.grid;

  const fragment = extractMarkup(modelText);
  const parsedRaw = parse(fragment);

  // TIER-1 CONTRACT GUARD (Phase 2.5). Run the deterministic auto-fixer on EACH
  // parsed node's markup BEFORE re-anchor/placement/sanitize:
  //   - Finding 8: positional CSS on the root is stripped (always neutralized).
  //   - Finding 9 + EMPTY: foster-parented/empty roots are UNFIXABLE -> the node is
  //     DROPPED here (a wrong id-stamp is never even attempted) and a warning is
  //     recorded. Surviving nodes carry their FIXED markup downstream.
  // Index alignment with the survivors' ParsedNode list is preserved because both
  // are built in the same single pass.
  //
  // NOTE on Finding 9 coverage: the HTML parser foster-parents most table-internal
  // tags (<tr>/<td>/<thead>/<caption>/<col>...) OUT of the fragment entirely at
  // PARSE time, so they never appear in `parsedRaw` and cannot be warned on — but
  // the SAFETY invariant still holds: discarded content never reaches placement, so
  // an id is never stamped on a wrong element. Foster tags the parser KEEPS as real
  // elements (<option>/<optgroup>) DO reach this guard and are dropped + warned.
  const parsed: ParsedTopLevelNode[] = [];
  const warnings: string[] = [];
  const remainingViolations: Violation[] = [];
  let dropIndex = 0;
  let collapsedAnySiblings = false;

  // WARNING 7: cap the processed top-level node count. Anything beyond the cap is
  // discarded with ONE aggregated warning (never thousands of per-node warnings).
  const overflow = parsedRaw.length - MAX_TOP_LEVEL_NODES;
  const capped =
    overflow > 0 ? parsedRaw.slice(0, MAX_TOP_LEVEL_NODES) : parsedRaw;

  for (const p of capped) {
    const { markup, remaining, usable, collapsedSiblings } = autoFixNodeMarkup(
      p.markup,
    );
    if (!usable) {
      dropIndex++;
      remainingViolations.push(...remaining);
      warnings.push(
        `Dropped 1 node (#${dropIndex}): ${remaining
          .map((v) => v.message)
          .join("; ")}`,
      );
      continue;
    }
    // WARNING 9: a node whose markup had multiple top-level siblings is collapsed to
    // its first element. Surface that upstream so the operator knows extra content
    // was dropped (and the self-repair can hint "one element per component").
    if (collapsedSiblings) collapsedAnySiblings = true;
    // Keep the FIXED markup paired with the parsed shape (shape is unaffected by
    // the root positional-CSS strip — keyedDiff ignores attrs/style).
    parsed.push({ node: p.node, markup });
  }

  if (collapsedAnySiblings) {
    warnings.push(
      "Some nodes had multiple top-level elements; kept the first of each (one element per component).",
    );
  }
  if (overflow > 0) {
    warnings.push(
      `Response had ${parsedRaw.length} top-level nodes; processed the first ${MAX_TOP_LEVEL_NODES} and dropped ${overflow}.`,
    );
  }

  const prevIds = prevProject.meta.nodeOrder.length
    ? prevProject.meta.nodeOrder
    : Object.keys(prevProject.manifest.nodes);

  // THE CRUX: the deterministic layer assigns the ids; the model's are untrusted.
  const anchored = reanchorIds(
    prevIds,
    parsed.map((p) => p.node),
    opts.prevShapes,
  );

  const prevNodes = prevProject.manifest.nodes;

  // Baseline for new auto-placed nodes is derived from SURVIVORS ONLY (ids in the
  // reanchored set that still exist in prevNodes), NOT the whole old manifest:
  // nodes about to be DROPPED this regen must not push new nodes off-screen below
  // invisible content (WARNING 6). topZ still spans all prevNodes (z stacking is
  // harmless and keeps new nodes above any lingering high-z content).
  const survivors: Record<string, DesignNodePlacement> = {};
  for (const node of anchored) {
    const id = node.dataNodeId as string;
    const p = prevNodes[id];
    if (p) survivors[id] = p;
  }
  const baseY = baselineY(survivors, grid);
  const baseZ = topZ(prevNodes);

  const nextNodes: Record<string, DesignNodePlacement> = {};
  const components: Record<string, string> = {};
  const shapes: ShapeMap = {};
  const order: string[] = [];
  const newIds: string[] = [];

  let newIndex = 0;
  for (let i = 0; i < anchored.length; i++) {
    const node = anchored[i];
    const id = node.dataNodeId as string; // reanchorIds always assigns one
    const kind = nodeKind(node);

    const survivor = prevNodes[id];
    if (survivor) {
      // Survivor: keep its placement verbatim, only refresh the kind from markup.
      nextNodes[id] = { ...survivor, kind };
    } else {
      // New: deterministic default placement, stacked above existing content.
      nextNodes[id] = defaultPlacement(
        newIndex,
        baseY,
        baseZ + 1 + newIndex,
        grid,
        kind,
      );
      newIndex++;
      newIds.push(id);
    }

    // Stamp the resolved id onto the faithful markup, then sanitize (chokepoint).
    components[id] = sanitizeNodeMarkup(applyNodeId(parsed[i].markup, id));
    shapes[id] = node;
    order.push(id);
  }

  const project: DesignProject = {
    ...prevProject,
    meta: { ...prevProject.meta, nodeOrder: order },
    manifest: { ...prevProject.manifest, nodes: nextNodes },
    components,
  };

  // WARNING 7: defensively cap the surfaced warnings/violations length. With the
  // MAX_TOP_LEVEL_NODES cap these can't realistically exceed it, but bounding here
  // guarantees a fixed upper bound on what the UI/self-repair carries regardless of
  // future changes to the per-node warning shape.
  return {
    project,
    newIds,
    shapes,
    warnings: warnings.slice(0, MAX_TOP_LEVEL_NODES),
    remainingViolations: remainingViolations.slice(0, MAX_TOP_LEVEL_NODES),
  };
}

/**
 * CONTENT-EDIT ROUND-TRIP. Re-generate ONLY one node: parse the single returned
 * element, keyed-diff it against the SAME `nodeId` (so the id survives even if the
 * model dropped/renamed it), re-sanitize, and swap ONLY that node's markup. The
 * node's placement and every OTHER node are left byte-identical.
 */
export function applyEdit(
  project: DesignProject,
  nodeId: string,
  modelText: string,
  opts: PipelineOptions = {},
): EditResult {
  const parse = opts.parse ?? parseTopLevelNodesWithMarkup;
  const noop = (warning?: string): EditResult => ({
    project,
    changed: false,
    warnings: warning ? [warning] : [],
  });

  // The target must exist; an unknown id is a no-op (defensive).
  if (!project.manifest.nodes[nodeId]) return noop();

  const fragment = extractMarkup(modelText);
  const parsed = parse(fragment);
  if (parsed.length === 0) {
    // Nothing parseable -> keep as-is, but tell the operator (WARNING 5).
    return noop("Couldn't apply the edit: invalid markup.");
  }

  // TIER-1 CONTRACT GUARD (Phase 2.5) on the edited node's markup. If the model
  // returned a foster-parented/empty root for this edit, there is no valid markup
  // to swap in — keep the EXISTING node untouched (never corrupt the canvas) AND
  // signal the no-op so the caller does NOT persist / report "Updated" (WARNING 5).
  // Positional CSS on the root is deterministically stripped by the auto-fixer
  // (Finding 8); a multi-top-level edit is collapsed to its first element.
  const { markup: fixedMarkup, usable, collapsedSiblings } = autoFixNodeMarkup(
    parsed[0].markup,
  );
  if (!usable) return noop("Couldn't apply the edit: invalid markup.");

  // Take the FIRST top-level element (an edit returns exactly one). Keyed-diff of
  // a single element against a single prev id is trivial: it ALWAYS resolves to
  // `nodeId` whether the model preserved, dropped, or renamed the id (the
  // deterministic layer owns id assignment, 1.6). We stamp the target id onto the
  // FIXED markup directly — equivalent to reanchorIds([nodeId],[node]) for one element.
  const markup = sanitizeNodeMarkup(applyNodeId(fixedMarkup, nodeId));

  // Refresh the node's sanitizer kind from the edited root tag: an edit may swap
  // html<->svg, and a stale kind would route it through the wrong profile on the
  // next load/inject (WARNING 7). Placement coordinates are otherwise untouched.
  const kind = nodeKind(parsed[0].node);
  const prevPlacement = project.manifest.nodes[nodeId];

  const next: DesignProject = {
    ...project,
    manifest: {
      ...project.manifest,
      nodes: {
        ...project.manifest.nodes,
        [nodeId]: { ...prevPlacement, kind },
      },
    },
    components: { ...project.components, [nodeId]: markup },
  };

  // WARNING 9: an edit that returned multiple top-level elements is collapsed to
  // its first; surface that so the operator knows extra content was dropped.
  const warnings = collapsedSiblings
    ? ["Kept the first element; an edit should return one element per component."]
    : [];
  return { project: next, changed: true, warnings };
}
