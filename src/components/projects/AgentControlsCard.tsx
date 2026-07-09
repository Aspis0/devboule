// AgentControlsCard — per-project agent capability/cost controls (Slice 5c). Effort,
// system-prompt, and (Claude-only) turn/budget caps, bound to ProjectMetadata.agentControls.
// Saves optimistically via set_project_agent_controls_cmd (mirrors SandboxModeSelector's
// busy/error/revert pattern). The PERMISSION axis lives in SandboxModeSelector, not here.

import { useEffect, useRef, useState } from "react";
import { Sliders } from "lucide-react";
import { invokeBackendCommand, isTauriRuntime } from "../../context/AppContext";
import {
  AGENT_EFFORT_OPTIONS,
  controlsEqual,
  effectiveAgentControls,
  setAgentControlsArgs,
} from "./agentControlsModel";
import type { AgentControls } from "../../types/backend";

export interface AgentControlsCardProps {
  projectId: string;
  controls: AgentControls | undefined;
  onControlsChange?: (c: AgentControls) => void;
}

export function AgentControlsCard({
  projectId,
  controls,
  onControlsChange,
}: AgentControlsCardProps) {
  const [local, setLocal] = useState<AgentControls>(() => effectiveAgentControls(controls));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Synchronous reentrancy guard so a rapid change never fires two IPC calls; `busy` state is
  // kept purely for rendering (busyRef is updated synchronously, before the setState commits).
  const busyRef = useRef(false);
  // The last value confirmed on disk (used to skip no-op saves — W1 — and to detect unsaved
  // local edits in the prop-sync below).
  const savedRef = useRef<AgentControls>(effectiveAgentControls(controls));
  // The latest value requested while a save was in flight — retried on completion so a fast
  // edit during a save is never silently dropped (B2). `localRef` mirrors `local` so the
  // prop-sync effect can read the current value without making `local` a dependency.
  const pendingRef = useRef<AgentControls | null>(null);
  const localRef = useRef(local);
  localRef.current = local;

  // Prop-sync: adopt the incoming prop (e.g. the 10s project refetch) ONLY when no save is in
  // flight AND there are no unsaved local edits — otherwise a refetch would clobber what the
  // user is typing (the textarea only commits on blur).
  useEffect(() => {
    if (busyRef.current) return;
    if (!controlsEqual(localRef.current, savedRef.current)) return; // unsaved edits → keep them
    const incoming = effectiveAgentControls(controls);
    savedRef.current = incoming;
    setLocal(incoming);
  }, [controls]);

  const save = async (next: AgentControls) => {
    if (!isTauriRuntime()) return;
    // W1: nothing actually changed vs the last persisted value — reflect locally, skip the write.
    if (controlsEqual(next, savedRef.current)) {
      setLocal(next);
      return;
    }
    // B2: a save is in flight — remember the latest and let the running save retry it.
    if (busyRef.current) {
      pendingRef.current = next;
      setLocal(next);
      return;
    }
    busyRef.current = true;
    setBusy(true);
    setError(null);
    setLocal(next);
    try {
      await invokeBackendCommand<void>(
        "set_project_agent_controls_cmd",
        setAgentControlsArgs(projectId, next),
      );
      savedRef.current = next;
      onControlsChange?.(next);
    } catch (e) {
      setLocal(savedRef.current); // revert to the last value confirmed on disk
      setError(
        typeof e === "string"
          ? e
          : e instanceof Error
            ? e.message
            : "Could not update agent controls.",
      );
    } finally {
      busyRef.current = false;
      setBusy(false);
      // Flush any edit queued while we were busy (B2).
      const queued = pendingRef.current;
      pendingRef.current = null;
      if (queued) void save(queued);
    }
  };

  const parseNum = (val: string): number | undefined => {
    if (val.trim() === "") return undefined;
    const n = Number(val);
    return Number.isFinite(n) ? n : undefined;
  };

  // Effort "" (Default) is selected when no effort is set (undefined).
  const currentEffort = local.effort ?? "";

  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2">
        <Sliders className="h-4 w-4 text-teal" />
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Agent controls
        </h3>
      </div>

      {error && (
        <p className="rounded-lg bg-coral/10 px-3 py-2 text-[11px] text-coral-dark">{error}</p>
      )}

      {/* Effort */}
      <div>
        <label className="mb-1 block text-[11px] font-semibold text-cream-600">Effort</label>
        <div className="flex flex-wrap gap-1.5">
          {AGENT_EFFORT_OPTIONS.map((opt) => {
            const selected = currentEffort === opt;
            return (
              <button
                key={opt || "default"}
                type="button"
                disabled={busy}
                aria-pressed={selected}
                onClick={() => void save({ ...local, effort: opt })}
                className={`rounded-md border px-2.5 py-1 text-[11px] font-medium transition-colors disabled:opacity-50 ${
                  selected
                    ? "border-teal/40 bg-teal/[0.06] text-teal"
                    : "border-cream-200 bg-white text-cream-700 hover:border-cream-300"
                }`}
              >
                {opt === "" ? "Default" : opt.charAt(0).toUpperCase() + opt.slice(1)}
              </button>
            );
          })}
        </div>
      </div>

      {/* Verifier work-ethic (recommended, opt-in) */}
      <div>
        <label className="mb-1 block text-[11px] font-semibold text-cream-600">
          Verifier (recommended)
        </label>
        <div className="space-y-1.5">
          {([
            ["verifierPerTask", "Verifier per task", "Auto-review each task when it reaches review."],
            ["maxRecallPerProject", "Max-recall at project end", "A 3-verifier final pass when all tasks are done."],
          ] as const).map(([key, title, caption]) => {
            const on = local[key] ?? false;
            return (
              <button
                key={key}
                type="button"
                disabled={busy}
                aria-pressed={on}
                onClick={() => void save({ ...local, [key]: !on })}
                className={`flex w-full items-start gap-2 rounded-md border px-2.5 py-1.5 text-left transition-colors disabled:opacity-50 ${
                  on ? "border-teal/40 bg-teal/[0.06]" : "border-cream-200 bg-white hover:border-cream-300"
                }`}
              >
                <span
                  className={`mt-0.5 h-3.5 w-3.5 shrink-0 rounded border ${
                    on ? "border-teal bg-teal" : "border-cream-300 bg-white"
                  }`}
                />
                <span className="text-[11px]">
                  <span className="font-semibold text-cream-700">{title}</span>
                  <span className="block text-cream-500">{caption}</span>
                </span>
              </button>
            );
          })}
        </div>
      </div>

      {/* System prompt */}
      <div>
        <label className="mb-1 block text-[11px] font-semibold text-cream-600">
          System prompt
        </label>
        <textarea
          rows={2}
          disabled={busy}
          className="w-full rounded-lg border border-cream-200 px-2 py-1.5 text-[12px] focus:border-teal focus:outline-none focus:ring-1 focus:ring-teal/20 disabled:opacity-60"
          value={local.systemPrompt ?? ""}
          onChange={(e) => setLocal({ ...local, systemPrompt: e.target.value })}
          onBlur={() => void save({ ...local })}
          placeholder="Extra system prompt (optional)"
        />
      </div>

      {/* Turn & budget caps (Claude only) */}
      <div>
        <label className="mb-1 block text-[11px] font-semibold text-cream-600">
          Turn &amp; budget caps
        </label>
        <div className="flex gap-2">
          <input
            type="number"
            min="0"
            disabled={busy}
            className="w-1/2 rounded-lg border border-cream-200 px-2 py-1.5 text-[12px] focus:border-teal focus:outline-none focus:ring-1 focus:ring-teal/20 disabled:opacity-60"
            value={local.maxTurns ?? ""}
            onChange={(e) => setLocal({ ...local, maxTurns: parseNum(e.target.value) })}
            onBlur={() => void save({ ...local })}
            placeholder="Max turns"
          />
          <input
            type="number"
            min="0"
            disabled={busy}
            className="w-1/2 rounded-lg border border-cream-200 px-2 py-1.5 text-[12px] focus:border-teal focus:outline-none focus:ring-1 focus:ring-teal/20 disabled:opacity-60"
            value={local.maxBudgetUsd ?? ""}
            onChange={(e) => setLocal({ ...local, maxBudgetUsd: parseNum(e.target.value) })}
            onBlur={() => void save({ ...local })}
            placeholder="Max budget (USD)"
          />
        </div>
        <p className="mt-1 text-[10px] text-cream-400">
          Turn/budget caps apply to Claude only.
        </p>
      </div>
    </div>
  );
}

export default AgentControlsCard;
