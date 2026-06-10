// Bounded self-repair — the PURE instruction builder (Phase 2.5 STEP A, Tier 1).
//
// When a FULL generation yields nodes with UNFIXABLE (`remaining`) contract
// violations, or yields ZERO usable nodes, DesignView may re-prompt the SAME
// provider ONCE with a targeted correction. This module turns the observed
// violation CODES into a deterministic, de-duplicated instruction string — no
// markup, no model text, no clock, no random.
//
// Keep it small + table-tested: the loop's behavior (cap, give-up, cancel-aware)
// lives in DesignView; the WORDING lives here so it is independently verifiable.

import type { ViolationCode } from "./contractValidator";

/** A targeted correction sentence per violation code (stable wording). */
const REPAIR_LINES: Record<ViolationCode, string> = {
  FOSTER_PARENTED_ROOT:
    "Do NOT use <tr>, <td>, <th>, <thead>, <tbody>, <tfoot>, <col>, <colgroup>, <caption>, <option>, or <optgroup> as a top-level element. Wrap tabular content in a <table> (or use a <div>) so each component's top-level element is a free-standing block.",
  MULTIPLE_TOP_LEVEL:
    "Return EXACTLY ONE top-level element per component. Do not wrap several components in a shared parent and do not emit sibling top-level elements for a single component.",
  POSITIONAL_CSS_ON_ROOT:
    "Do NOT set position, top, left, right, bottom, float, z-index, inset, or outer margin on the top-level element — the host owns placement. Use only internal layout (flex, grid, padding, gap).",
  EMPTY:
    "Return non-empty UI markup: at least one valid top-level HTML or SVG element. Do not return prose or an empty response.",
  SCRIPT_OR_HANDLER:
    "Do NOT include <script>, on* event-handler attributes, or javascript: URLs. Output plain HTML/SVG markup only.",
};

/**
 * Build a deterministic self-repair instruction from a set of violation codes.
 * De-duplicates codes, orders them by the fixed `REPAIR_LINES` key order (NOT by
 * input order, so the same code-set always yields byte-identical output), and
 * returns "" when there is nothing actionable. PURE.
 */
export function buildRepairInstruction(
  violations: ReadonlyArray<{ code: ViolationCode }>,
): string {
  const present = new Set<ViolationCode>();
  for (const v of violations) {
    if (v && v.code in REPAIR_LINES) present.add(v.code);
  }
  if (present.size === 0) return "";

  // Stable order: iterate REPAIR_LINES' declared key order, emit only present ones.
  const lines: string[] = [];
  for (const code of Object.keys(REPAIR_LINES) as ViolationCode[]) {
    if (present.has(code)) lines.push(`- ${REPAIR_LINES[code]}`);
  }

  return [
    "Your previous output did not satisfy the design contract. Fix EXACTLY these issues and return corrected markup only:",
    ...lines,
  ].join("\n");
}
