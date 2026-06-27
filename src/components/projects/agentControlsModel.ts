// agentControlsModel — pure model for the per-project Agent controls card (Slice 5c).
// Kept separate from the component for vitest-friendly logic (same split as netConsentModel.ts).

import type { AgentControls } from "../../types/backend";

/** Effort options offered in the UI. "" == leave the CLI default (no flag emitted). */
export const AGENT_EFFORT_OPTIONS = ["", "low", "medium", "high", "xhigh", "max"] as const;

/** Normalize undefined metadata → an empty controls object. */
export function effectiveAgentControls(c: AgentControls | undefined): AgentControls {
  return c ?? {};
}

/**
 * Build the args for `set_project_agent_controls_cmd`. Strips empty string / non-positive /
 * non-finite values to `undefined` so the backend stores `None` (NO-CHURN). Returns
 * `{ projectId, controls }` (camelCase over the Tauri bridge).
 */
export function setAgentControlsArgs(
  projectId: string,
  controls: AgentControls,
): Record<string, unknown> {
  const clean: AgentControls = {};
  const effort = (controls.effort ?? "").trim();
  if (effort) clean.effort = effort;
  const sp = (controls.systemPrompt ?? "").trim();
  if (sp) clean.systemPrompt = sp;
  if (
    typeof controls.maxTurns === "number" &&
    Number.isFinite(controls.maxTurns) &&
    controls.maxTurns > 0
  ) {
    clean.maxTurns = Math.floor(controls.maxTurns);
  }
  if (
    typeof controls.maxBudgetUsd === "number" &&
    Number.isFinite(controls.maxBudgetUsd) &&
    controls.maxBudgetUsd > 0
  ) {
    clean.maxBudgetUsd = controls.maxBudgetUsd;
  }
  // Verifier work-ethic toggles (opt-in): only persist when ON (NO-CHURN, mirrors the Rust
  // skip_serializing_if).
  if (controls.verifierPerTask) clean.verifierPerTask = true;
  if (controls.maxRecallPerProject) clean.maxRecallPerProject = true;
  return { projectId, controls: clean };
}

/** The normalized (stripped) controls — what actually gets persisted. */
export function cleanControls(controls: AgentControls): AgentControls {
  return setAgentControlsArgs("", controls).controls as AgentControls;
}

/** True when two controls are equal AFTER normalization (used to skip no-op saves and to
 *  detect unsaved local edits). The cleaned object has a stable key order, so a stringify
 *  comparison is deterministic. */
export function controlsEqual(a: AgentControls, b: AgentControls): boolean {
  return JSON.stringify(cleanControls(a)) === JSON.stringify(cleanControls(b));
}
