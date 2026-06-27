// useStageRotation — the auto-rotating stage controller for the Planner panel.
//
// Ports the DCLogic from `Planner Plan Mode.dc.html`: the stage drifts
// exa -> plan -> design -> exa on a timer; selecting a view manually pauses the
// rotation; a toggle resumes/pauses it. Written by the local model (gemma MoE),
// verified by hand.
import { useState, useEffect, useCallback } from "react";
import type { StageView } from "./plannerModel";

export type { StageView };

const STAGES: StageView[] = ["exa", "plan", "design"];

export interface StageRotation {
  view: StageView;
  auto: boolean;
  /** Select a view AND pause auto-rotation (manual revisit). */
  pick: (v: StageView) => void;
  /** Resume / pause the rotation. */
  toggleAuto: () => void;
}

export function useStageRotation(
  intervalMs: number = 3800,
  // Only auto-rotate while the orchestrator is actually working. When idle the
  // stage holds still (no fake cycling through views with nothing happening).
  enabled: boolean = true,
  // Phase 5: suspend rotation while an interactive artifact is actively shown.
  // DOES NOT mutate `auto` — when hold releases, rotation resumes iff
  // `auto && enabled` are still true (the user's toggle is untouched).
  hold: boolean = false,
): StageRotation {
  const [view, setView] = useState<StageView>("exa");
  const [auto, setAuto] = useState<boolean>(true);

  const pick = useCallback((v: StageView) => {
    setView(v);
    setAuto(false);
  }, []);

  const toggleAuto = useCallback(() => {
    setAuto((prev) => !prev);
  }, []);

  useEffect(() => {
    if (!auto || !enabled || hold) return;
    const timer = setInterval(() => {
      setView((current) => {
        const next = (STAGES.indexOf(current) + 1) % STAGES.length;
        return STAGES[next];
      });
    }, intervalMs);
    return () => clearInterval(timer);
  }, [auto, enabled, hold, intervalMs]);

  return { view, auto, pick, toggleAuto };
}
