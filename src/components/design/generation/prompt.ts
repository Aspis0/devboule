// Versioned system-prompt contract for the design LLM (Phase 2 STEP 3).
//
// This encodes the LOCKED architecture (1.1 / 1.3 / 1.6) as a single, versioned
// constant plus two PURE builders. The contract is the only thing standing
// between the operator's free-text instruction and the deterministic pipeline,
// so it must be explicit and stable: bump `DESIGN_PROMPT_VERSION` whenever the
// wording changes so audit lines / regressions can be tied to a contract rev.
//
// The deterministic layer NEVER trusts the model's ids or placement — but a
// well-behaved model that follows the contract produces markup that re-anchors
// cleanly and needs no positional neutralization, so we still spell out the
// rules. PURE (no DOM, no clock, no random): snapshot-tested.

/** The contract revision. Bump on ANY wording change to the system prompt. */
export const DESIGN_PROMPT_VERSION = 1;

/**
 * The versioned system prompt encoding the LOCKED contract. Output is UI markup
 * ONLY (HTML+CSS or inline SVG) — no prose, no JSX, no scripts, no event
 * handlers, and crucially NO positioning on top-level elements (placement is
 * owned by the host engine per 1.1 / 1.3).
 */
export const DESIGN_SYSTEM_PROMPT_V1 = [
  "You are a UI design generator for a Figma-like canvas.",
  "",
  "OUTPUT FORMAT — follow EXACTLY:",
  "- Output ONLY UI markup: HTML with inline CSS, or inline SVG. Nothing else.",
  "- NO prose, NO explanations, NO Markdown commentary around the markup.",
  "- NO JSX, NO framework code, NO template syntax — plain HTML/SVG only.",
  "- NEVER include <script>, <style>, <link>, <meta>, <base>, <iframe>, or any",
  "  on* event handler attribute (onclick, onerror, ...). They will be stripped.",
  "",
  "TOP-LEVEL COMPONENTS:",
  "- Each top-level component is EXACTLY ONE element (e.g. one <section>, one",
  "  <div>, one <svg>). Do not wrap multiple components in a shared parent.",
  "- Each top-level element carries a unique data-node-id attribute, e.g.",
  '  <section data-node-id="hero">…</section>.',
  "- On EDIT, you are given one element with its data-node-id — return the SAME",
  "  single element and PRESERVE its data-node-id exactly.",
  "",
  "PLACEMENT IS OWNED BY THE HOST — on TOP-LEVEL elements you MUST NOT set:",
  "- position (absolute/fixed/sticky/relative), top, left, right, bottom,",
  "- z-index, float, or any outer margin.",
  "- The host positions every component on the canvas. You only control INTERNAL",
  "  layout INSIDE a component: flex, grid, padding, gap, and the component's own",
  "  width/height of inner parts. Never the component's place on the page.",
  "",
  "STYLE:",
  "- Use clean, modern, accessible markup. Inline styles are fine.",
  "- Keep each component self-contained: its inner layout moves with it.",
].join("\n");

/** Options accepted by the generation prompt builder. */
export interface GeneratePromptOptions {
  /**
   * Optional grounding/context block (e.g. target design tokens or component
   * snippets) inserted verbatim before the instruction. Pure pass-through here;
   * Oracle grounding is wired in a later step.
   */
  context?: string;
}

/**
 * Build the FULL-GENERATION prompt: a fragment of sibling top-level elements is
 * expected back. Returns the system contract, the optional context, and the
 * user's instruction as one string. PURE.
 */
export function buildGeneratePrompt(
  userInstruction: string,
  opts: GeneratePromptOptions = {},
): string {
  const parts = [DESIGN_SYSTEM_PROMPT_V1, ""];
  const context = (opts.context ?? "").trim();
  if (context.length > 0) {
    parts.push("CONTEXT (use to stay coherent with the product):");
    parts.push(context);
    parts.push("");
  }
  parts.push("TASK — generate a fragment of sibling top-level components for:");
  parts.push(userInstruction.trim());
  return parts.join("\n");
}

/**
 * Build the SINGLE-NODE EDIT prompt: the model is handed ONLY the one node's
 * current markup and must return the ONE element back, preserving its
 * data-node-id. PURE.
 */
export function buildEditPrompt(
  nodeMarkup: string,
  userInstruction: string,
): string {
  return [
    DESIGN_SYSTEM_PROMPT_V1,
    "",
    "EDIT — here is ONE existing component. Return the SAME single element with",
    "its data-node-id PRESERVED, applying this change:",
    userInstruction.trim(),
    "",
    "CURRENT MARKUP:",
    nodeMarkup.trim(),
  ].join("\n");
}
