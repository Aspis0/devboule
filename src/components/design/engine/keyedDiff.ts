// Deterministic id re-anchoring — THE correctness crux (LOCKED architecture 1.6).
//
// When the LLM regenerates / edits node markup it may drop, rename, duplicate, or
// reorder the `data-node-id`s we own. The deterministic layer NEVER trusts the
// model to report ids; instead it re-assigns stable ids by a KEYED STRUCTURAL
// DIFF (React-reconciliation style) so manifest placements survive content edits.
//
// PURE: operates on a plain `ParsedNode` tree (no DOM). A thin DOMParser wrapper
// lives in the DOM layer and produces this structure; the matching logic here is
// fully unit-testable without a DOM.
//
// Algorithm (deterministic, document order):
//   1. EXACT-ID CLAIM. A new top-level node carrying a prev id keeps it — but
//      only the FIRST node (document order) claiming a given prev id wins; later
//      duplicates fall through (they become inserts).
//   2. STRUCTURAL RECOVERY (when previous shapes are supplied). Each still-
//      unmatched new node is matched to the first still-unclaimed prev id whose
//      stored shape has the same structural signature (tag + recursive child
//      structure, ignoring data-node-id). This recovers renames/reorders where
//      the model dropped or changed the id.
//   3. MINT FRESH. Remaining new nodes get fresh, unique, charset-valid ids.
//
// Determinism: no clock, no random — minted ids derive from a counter; ties
// resolve by document/prev order.
//
// KNOWN LIMITATION (Finding 10): two structurally-IDENTICAL siblings reordered by
// the model can swap placements — structural matching alone can't tell them apart
// without content hashing, which is intentionally out of scope here.

/** A minimal parsed top-level element (and its descendants for signatures). */
export interface ParsedNode {
  /** Lowercased tag name, e.g. "section", "button", "svg". */
  tag: string;
  /** The `data-node-id` the markup carried, if any (untrusted; may be dropped). */
  dataNodeId?: string;
  /** Remaining attributes (excluding/ including data-node-id; ignored for sig). */
  attrs: Record<string, string>;
  /** Child elements (used for the structural signature). */
  children: ParsedNode[];
  /** Concatenated text content (not used for matching; carried for completeness). */
  text: string;
}

/**
 * A stable structural fingerprint: tag + recursive child tags. Deliberately
 * IGNORES `data-node-id` and all attributes/text so a rename or content tweak
 * still matches the previous shape. Pure + deterministic.
 */
export function structuralSignature(node: ParsedNode): string {
  if (node.children.length === 0) return node.tag;
  const kids = node.children.map(structuralSignature).join(",");
  return `${node.tag}(${kids})`;
}

/** Charset for minted/validated ids: ^[a-z0-9][a-z0-9_-]{0,63}$ (mirrors Rust). */
const ID_CHARSET = /^[a-z0-9][a-z0-9_-]{0,63}$/;

/**
 * Mint a fresh id not present in `taken`, derived deterministically from a
 * counter. Format `n<counter>` always satisfies the id charset. Increments past
 * any collision so the result is unique within the produced set.
 */
function mintId(taken: Set<string>, counterRef: { n: number }): string {
  let candidate = `n${counterRef.n}`;
  counterRef.n += 1;
  while (taken.has(candidate)) {
    candidate = `n${counterRef.n}`;
    counterRef.n += 1;
  }
  taken.add(candidate);
  return candidate;
}

/**
 * Re-anchor stable ids onto a freshly produced list of top-level nodes.
 *
 * @param prevIds   The previous top-level node ids (placement authority owns these).
 * @param nextNodes The freshly parsed top-level nodes (ids untrusted).
 * @param prevShapes Optional map prevId -> its previous parsed shape, enabling
 *                   structural recovery of dropped/renamed ids.
 * @returns NEW node objects (inputs never mutated) each with a resolved,
 *          unique, charset-valid `dataNodeId`.
 */
export function reanchorIds(
  prevIds: string[],
  nextNodes: ParsedNode[],
  prevShapes?: Record<string, ParsedNode>,
): ParsedNode[] {
  const prevSet = new Set(prevIds);
  const claimed = new Set<string>(); // prev ids already taken by a survivor
  // Seed `taken` with EVERY prev id (not just survivors): a minted id must never
  // equal a DROPPED prev id, or applyGeneration's `prevNodes[id]` lookup would
  // make a genuinely-new node inherit the dropped node's placement (WARNING 5).
  const taken = new Set<string>(prevIds); // every id we must not re-mint
  const counterRef = { n: 1 };

  // Resolved id per new-node index; undefined until assigned.
  const assigned: (string | undefined)[] = new Array(nextNodes.length).fill(
    undefined,
  );

  // --- Pass 1: exact-id claim (first claimant of each prev id wins) ----------
  for (let i = 0; i < nextNodes.length; i++) {
    const carried = nextNodes[i].dataNodeId;
    if (
      carried !== undefined &&
      prevSet.has(carried) &&
      !claimed.has(carried) &&
      ID_CHARSET.test(carried)
    ) {
      assigned[i] = carried;
      claimed.add(carried);
      taken.add(carried);
    }
  }

  // --- Pass 2: structural recovery for the unmatched (needs prevShapes) ------
  if (prevShapes) {
    for (let i = 0; i < nextNodes.length; i++) {
      if (assigned[i] !== undefined) continue;
      const sig = structuralSignature(nextNodes[i]);
      // First unclaimed prev id (in prev order) whose shape signature matches.
      for (const pid of prevIds) {
        if (claimed.has(pid)) continue;
        const shape = prevShapes[pid];
        if (shape && structuralSignature(shape) === sig) {
          assigned[i] = pid;
          claimed.add(pid);
          taken.add(pid);
          break;
        }
      }
    }
  }

  // --- Pass 3: mint fresh ids for everything still unmatched -----------------
  for (let i = 0; i < nextNodes.length; i++) {
    if (assigned[i] === undefined) {
      assigned[i] = mintId(taken, counterRef);
    }
  }

  // Build NEW node objects (never mutate inputs).
  return nextNodes.map((n, i) => ({ ...n, dataNodeId: assigned[i] }));
}
