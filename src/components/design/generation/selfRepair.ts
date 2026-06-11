// Bounded self-repair — PURE decision + prompt helpers (Phase 2.5 STEP A, Tier 1).
//
// DesignView owns the actual loop (it drives the stream + React state); these pure
// functions decide WHETHER to retry and BUILD the corrected prompt, so the policy
// is independently unit-testable. NO clock, NO random, NO DOM.
//
// Policy (the loop in DesignView enforces the cap + cancel-awareness):
//   - Attempt 0 is the user's original generation.
//   - A generation RESULT triggers a repair attempt when it produced ZERO usable
//     nodes OR dropped >=1 node for an UNFIXABLE contract violation (foster/empty).
//   - Repairs are capped (default 1 retry, hard cap 2). After the cap, give up and
//     surface a clear status — never loop, never corrupt the canvas.

import { buildGeneratePrompt } from "./prompt";
import { buildRepairInstruction } from "./repairInstruction";
import type { Violation } from "./contractValidator";

/** Hard upper bound on repair attempts, regardless of the configured retry count. */
export const MAX_REPAIR_ATTEMPTS = 2;
/** Default repair retries when the caller does not specify one. */
export const DEFAULT_REPAIR_RETRIES = 1;

/** The minimal generation outcome the repair policy inspects. */
export interface RepairableOutcome {
  /** Count of nodes COMMITTED to the canvas (manifest size after the pipeline). */
  committedNodeCount: number;
  /** Unfixable violations from dropped nodes (pipeline `remainingViolations`). */
  remainingViolations: Violation[];
}

/**
 * Decide whether a generation outcome warrants ANOTHER repair attempt, given how
 * many repair attempts have ALREADY run. PURE.
 *
 * @param outcome        the just-finished generation's result summary
 * @param attemptsSoFar  repair attempts already performed (0 on the first result)
 * @param maxRetries     configured retries (clamped to [0, MAX_REPAIR_ATTEMPTS])
 */
export function shouldSelfRepair(
  outcome: RepairableOutcome,
  attemptsSoFar: number,
  maxRetries: number = DEFAULT_REPAIR_RETRIES,
): boolean {
  const cap = Math.max(0, Math.min(maxRetries, MAX_REPAIR_ATTEMPTS));
  if (attemptsSoFar >= cap) return false;
  const producedNothing = outcome.committedNodeCount === 0;
  const droppedSomething = outcome.remainingViolations.length > 0;
  return producedNothing || droppedSomething;
}

/**
 * Build the corrected generation prompt for a repair attempt: the original user
 * instruction, the same grounding context, the SAME design contract, PLUS a targeted
 * correction block built from the observed violation codes. Returns null when there is
 * nothing actionable (no recognized violations AND the canvas wasn't empty — the caller
 * shouldn't even have asked). PURE.
 */
export function buildRepairPrompt(
  userInstruction: string,
  outcome: RepairableOutcome,
  context: string,
  designContract?: string,
): string | null {
  // When nothing was produced but no specific violation was captured (e.g. the
  // model returned only prose), fall back to an EMPTY-style correction so the retry
  // still carries an actionable instruction.
  const violations =
    outcome.remainingViolations.length > 0
      ? outcome.remainingViolations
      : outcome.committedNodeCount === 0
        ? [{ code: "EMPTY" as const, message: "" }]
        : [];

  const repair = buildRepairInstruction(violations);
  if (repair.length === 0) return null;

  const mergedContext = context.trim().length > 0
    ? `${context.trim()}\n\n${repair}`
    : repair;
  // The design contract is carried into the repair prompt too, so a corrected pass
  // is still bound by the project's rules (same injection as the original generate).
  return buildGeneratePrompt(userInstruction, {
    context: mergedContext,
    designContract,
  });
}
