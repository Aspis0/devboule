// Pure classifier for the Oracle local-retrieval-runtime setup state.
//
// The runtime "not ready" banner must NOT cry "No Python 3.9+ found" while the
// backend probe is merely slow/busy on first startup. The backend MAY ship an
// additive `checking` flag (tri-state Python probe: Found / NotPython /
// Inconclusive). Older builds omit it, so we ALSO sniff the soft message the
// backend emits while a probe is inconclusive. This module is the single source
// of truth for that decision and is unit-tested (no React, no DOM).

import type { OracleRuntimeSetup } from "../types/backend";

export type OracleRuntimeStage =
  // The probe is still running / inconclusive — show "Checking the local
  // runtime…", NOT an install prompt.
  | "checking"
  // The probe genuinely ran and found no usable Python — show the install
  // prompt with the python.org guidance.
  | "missingPython"
  // Python is present; the runtime just needs venv/deps/embedder installed.
  | "needsInstall";

// Phrases the backend uses (case-insensitively) in `messages` while a probe is
// inconclusive — busy machine, slow first spawn, or a timed-out probe. Kept
// permissive so a minor wording change on the Rust side still degrades safely
// to "checking" rather than the scary false "missing Python".
const INCONCLUSIVE_HINTS = [
  "checking",
  "still checking",
  "inconclusive",
  "timed out",
  "timeout",
  "taking longer",
  "busy",
  "in progress",
  "verifying",
];

function messagesSuggestChecking(messages: readonly string[]): boolean {
  return messages.some((raw) => {
    const m = raw.toLowerCase();
    return INCONCLUSIVE_HINTS.some((hint) => m.includes(hint));
  });
}

/**
 * Decide what the runtime banner should show. Only call this once the runtime
 * is known NOT ready (`setup.ready === false`); when it is ready the banner is
 * hidden entirely by the caller.
 *
 * Precedence:
 *   1. If Python is reported found, it is never "missing" — go straight to
 *      `needsInstall` regardless of any stale checking hint.
 *   2. Else if the additive `checking` flag is true, OR (the flag is absent and
 *      a soft message hints the probe is inconclusive) -> `checking`.
 *   3. Else -> `missingPython` (the probe genuinely ran and found nothing).
 */
export function classifyRuntimeStage(
  setup: OracleRuntimeSetup,
): OracleRuntimeStage {
  if (setup.pythonFound) return "needsInstall";

  // Field may be absent on older backends; `=== true` guards undefined.
  if (setup.checking === true) return "checking";

  // No explicit flag: fall back to sniffing the soft progress message. Only
  // treat as checking when the flag was NOT explicitly provided as false (a
  // backend that sets `checking: false` has authoritatively finished probing).
  if (setup.checking === undefined && messagesSuggestChecking(setup.messages)) {
    return "checking";
  }

  return "missingPython";
}
