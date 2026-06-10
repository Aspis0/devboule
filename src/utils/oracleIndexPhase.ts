// Pure helpers for the Oracle dense-index live "phase" sub-state.
//
// While the index job pauses on GPU heat / low RAM it stays `status: "running"`
// but does not progress, so the progress bar looks frozen. The backend surfaces
// a live `phase` ("running" | "cooling_gpu" | "waiting_memory") plus a short,
// PATH-FREE `phaseMessage` on the job object; the UI turns that into a calm
// "working, not stuck" hint. Kept dependency-free so it is trivially unit-tested.

// The live index sub-states that warrant a visible hint. Anything else (incl.
// undefined / "running") means "normal progress" and no hint is shown.
export type OracleIndexPhase = "cooling_gpu" | "waiting_memory";

export interface OracleIndexPhaseHint {
  phase: OracleIndexPhase;
  label: string;
}

// Turn a job's live `phase` + optional server `phaseMessage` into the hint, or
// null when the job is just progressing normally. Prefers the server message (it
// carries the live temp / free-GB numbers) and falls back to a static per-phase
// label. Path-free by contract.
export function oracleIndexPhaseHint(
  phase: unknown,
  phaseMessage: unknown,
): OracleIndexPhaseHint | null {
  if (phase !== "cooling_gpu" && phase !== "waiting_memory") return null;
  const fromServer =
    typeof phaseMessage === "string" && phaseMessage.trim().length > 0
      ? phaseMessage.trim()
      : null;
  const fallback =
    phase === "cooling_gpu"
      ? "GPU cooling — resuming…"
      : "Waiting for memory — resuming…";
  return { phase, label: fromServer ?? fallback };
}
