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

/** The contract revision. Bump on ANY wording change to the system prompt OR to how
 * the injected blocks (context / design contract) are framed.
 * v2 (Phase C): adds the optional `DESIGN CONTRACT` block built from the project's
 * design.md (injected for ALL providers, including CLI — see the trust note below). */
export const DESIGN_PROMPT_VERSION = 2;

/** The fence sentinels around the injected contract DATA block. The opening sentinel
 * is `<<<` + the token; the closing sentinel is the bare token alone on its own line. */
const CONTRACT_FENCE_TOKEN = "DESIGN_CONTRACT";
const CONTRACT_FENCE_OPEN = `<<<${CONTRACT_FENCE_TOKEN}`;

/**
 * Neutralize any line of the contract body that could be read by a downstream parser
 * (or a steerable CLI model) as the fence boundary, so untrusted-but-user-curated
 * content can never BREAK OUT of the `<<<DESIGN_CONTRACT … DESIGN_CONTRACT` fence.
 *
 * A line is dangerous when, after trimming leading whitespace, it STARTS WITH the bare
 * `DESIGN_CONTRACT` token (this also covers a line equal to it, and the opening
 * `<<<DESIGN_CONTRACT` sentinel via the explicit `<<<` check). We defang it by prefixing
 * a visible, deterministic guard marker `· ` (U+00B7 MIDDLE DOT + space) so the line no
 * longer begins with the sentinel and is obviously an inert, escaped data line. We do
 * NOT mutate the token mid-line (only the boundary-significant leading position matters).
 */
function neutralizeFence(body: string): string {
  return body
    .split("\n")
    .map((line) => {
      const lead = line.replace(/^\s+/, "");
      if (lead.startsWith(CONTRACT_FENCE_OPEN) || lead.startsWith(CONTRACT_FENCE_TOKEN)) {
        return `· ${line}`;
      }
      return line;
    })
    .join("\n");
}

/**
 * Frame the project's design.md as a clearly-fenced DATA block placed BEFORE the user
 * instruction. PURE. Returns [] when there is no contract.
 *
 * TRUST: design.md is injected into EVERY prompt, including CLI providers (claude/
 * codex) which otherwise receive NO pre-fetched Oracle grounding (B4). This is safe
 * ONLY because design.md is USER-CURATED: the seed draft (which may quote target
 * source) is always shown in the contract editor and written to disk ONLY on an
 * explicit Save, so by the time it reaches here it is the same trust class as the
 * user's own instruction. The caller clamps it to 16 KiB before passing it in. We
 * fence it as data and label it a project file the model should FOLLOW (rules), not
 * obey as new system instructions.
 *
 * FENCE SAFETY: the body is run through {@link neutralizeFence} so a bare
 * `DESIGN_CONTRACT` (or `<<<DESIGN_CONTRACT`) line in the content cannot close/reopen
 * the fence early — exactly ONE opening and ONE closing sentinel ever bound the block.
 */
function designContractBlock(contract: string | undefined): string[] {
  const c = (contract ?? "").trim();
  if (c.length === 0) return [];
  return [
    "DESIGN CONTRACT (project file, follow its rules):",
    CONTRACT_FENCE_OPEN,
    neutralizeFence(c),
    CONTRACT_FENCE_TOKEN,
    "",
  ];
}

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
  /**
   * The project's design.md contract (already clamped to 16 KiB by the caller).
   * Injected as a fenced DATA block before the instruction for ALL providers — see
   * {@link designContractBlock} for the trust rationale.
   */
  designContract?: string;
}

/** Options accepted by the single-node edit prompt builder. */
export interface EditPromptOptions {
  /** The project's design.md contract (clamped). Same injection as generate. */
  designContract?: string;
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
  parts.push(...designContractBlock(opts.designContract));
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
 *
 * B4 EXEMPTION: `nodeMarkup` is embedded verbatim for ALL providers, including the
 * CLI/agentic ones the B4 gate otherwise denies pre-fetched grounding. This is intentional
 * and NOT a gate violation: the B4 gate bars only UN-REVIEWED TARGET TEXT (raw Oracle chunk
 * text a CLI provider could be prompt-injected by). The node markup is the user's own canvas
 * working material — visible/editable on the canvas, the same trust class as the approved
 * design.md contract — not retrieved untrusted source. The caller (DesignView.onEditNode)
 * documents the same exemption at the call site.
 */
export function buildEditPrompt(
  nodeMarkup: string,
  userInstruction: string,
  opts: EditPromptOptions = {},
): string {
  return [
    DESIGN_SYSTEM_PROMPT_V1,
    "",
    ...designContractBlock(opts.designContract),
    "EDIT — here is ONE existing component. Return the SAME single element with",
    "its data-node-id PRESERVED, applying this change:",
    userInstruction.trim(),
    "",
    "CURRENT MARKUP:",
    nodeMarkup.trim(),
  ].join("\n");
}
