import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Layers,
  ShieldCheck,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { invokeBackendCommand } from "../../context/AppContext";
import type { MiniWriteBehavior } from "../../types/config";

// E1/E2/E3 — Settings → Providers & Models card for the mini WRITE-BEHAVIOR policy.
// This is the user-facing CEILING for how the coder is allowed to delegate writes to
// the local mini; within it the coder still decides per task (the A3 launch-prompt
// guidance reads this persisted policy). Mirrors MiniCoderBackendCard's shape
// (section shell, Save/persist-on-change via a backend command, mounted guard,
// inline error). PRODUCT-GENERAL: no model/product hardcoding; English UI copy.
//
// Persistence is its OWN get/set pair (get_mini_write_behavior /
// set_mini_write_behavior) — it does not live on miniCoderBackend, so it round-trips
// independently and an absent key reads back as the "auto" default (zero migration).

// The three policy options + their generic, layered descriptions (E1). The order is
// the ceiling progression: most restrictive → default → most permissive.
const POLICY_OPTIONS: ReadonlyArray<{
  value: MiniWriteBehavior;
  label: string;
  description: string;
}> = [
  {
    value: "safe",
    label: "Safe",
    description:
      "Emit-edits only: the mini makes one write and one fix per delegation. Agentic-iterative looping is disabled.",
  },
  {
    value: "auto",
    label: "Auto",
    description:
      "The coder decides per task by model and language — agentic-iterative where the language is gate-covered and the model is capable, emit-edits otherwise. (Default.)",
  },
  {
    value: "agenticAllowed",
    label: "Agentic allowed",
    description:
      "Agentic-iterative is encouraged for capable models on gate-covered languages; emit-edits remains the fallback for uncovered languages or a weak model.",
  },
];

export function MiniWriteBehaviorCard() {
  // The persisted policy (read on mount). null == not loaded yet; we default the
  // selection to "auto" so the control is never indeterminate.
  const [policy, setPolicy] = useState<MiniWriteBehavior>("auto");
  const [loaded, setLoaded] = useState(false);
  // E2 coverage: the project-agnostic potential set. null == not loaded / unavailable.
  const [coverage, setCoverage] = useState<string[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedTick, setSavedTick] = useState(false);
  const [explainerOpen, setExplainerOpen] = useState(false);
  const mountedRef = useRef(true);
  const savedTimer = useRef<number | null>(null);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      if (savedTimer.current !== null) {
        window.clearTimeout(savedTimer.current);
        savedTimer.current = null;
      }
    };
  }, []);

  // Load the persisted policy once on mount. A failure leaves the safe "auto" default
  // and surfaces no blocking error (the control still works; the first save fixes it).
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const current = await invokeBackendCommand<MiniWriteBehavior>(
          "get_mini_write_behavior",
        );
        if (!cancelled && mountedRef.current) {
          if (current === "safe" || current === "auto" || current === "agenticAllowed") {
            setPolicy(current);
          }
          setLoaded(true);
        }
      } catch {
        if (!cancelled && mountedRef.current) {
          // Degrade silently to the "auto" default; the user can still pick + save.
          setLoaded(true);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Load the read-only coverage list once on mount. Absent/failed -> null (the line is
  // hidden / shows a graceful note); never blocks the policy control.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const langs = await invokeBackendCommand<string[]>(
          "get_agentic_coverage_languages",
        );
        if (!cancelled && mountedRef.current) {
          setCoverage(Array.isArray(langs) ? langs : []);
        }
      } catch {
        if (!cancelled && mountedRef.current) {
          setCoverage(null);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Persist on change (no separate Save button — the segmented control IS the action).
  // Optimistically reflect the new selection, then revert it on a failed write.
  const onSelect = useCallback(
    async (next: MiniWriteBehavior) => {
      if (next === policy && loaded) return;
      const previous = policy;
      setPolicy(next);
      setBusy(true);
      setError(null);
      try {
        await invokeBackendCommand<MiniWriteBehavior>("set_mini_write_behavior", {
          behavior: next,
        });
        if (mountedRef.current) {
          setSavedTick(true);
          if (savedTimer.current !== null) window.clearTimeout(savedTimer.current);
          savedTimer.current = window.setTimeout(() => {
            if (mountedRef.current) setSavedTick(false);
          }, 2000);
        }
      } catch (e) {
        if (mountedRef.current) {
          // Revert the optimistic selection so the UI never claims an unsaved policy.
          setPolicy(previous);
          setError(
            e instanceof Error
              ? e.message
              : "Could not save the write-behavior policy.",
          );
        }
      } finally {
        if (mountedRef.current) setBusy(false);
      }
    },
    [policy, loaded],
  );

  const coverageText =
    coverage === null
      ? null
      : coverage.length > 0
        ? coverage.join(", ")
        : "none";

  return (
    <section
      className="rounded-2xl border border-cream-200 bg-white p-4"
      data-help-title="The write-behavior policy is the ceiling for how the coder delegates writes to the local mini."
      data-help-lines="Safe = emit-edits only (one write + one fix).|Auto = the coder decides by model and language (default).|Agentic allowed = agentic-iterative is encouraged for capable models on covered languages.|Within the ceiling, the coder still chooses per task.|Stored in your local config.json."
    >
      <div className="mb-3 flex items-center gap-2">
        <Layers className="h-4 w-4 text-teal" />
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Mini write behavior
        </h3>
      </div>
      <p className="mb-3 max-w-3xl text-[12px] leading-5 text-cream-500">
        Set the ceiling for how your coders delegate file WRITES to the local
        mini. Within this ceiling the coder still decides per task.
      </p>

      {/* E1 — the segmented policy control (radiogroup for accessibility). */}
      <div
        role="radiogroup"
        aria-label="Mini write-behavior policy"
        className="grid gap-2"
      >
        {POLICY_OPTIONS.map((option) => {
          const selected = policy === option.value;
          return (
            <button
              key={option.value}
              type="button"
              role="radio"
              aria-checked={selected}
              disabled={busy}
              onClick={() => void onSelect(option.value)}
              className={`flex items-start gap-3 rounded-2xl border px-3 py-2.5 text-left transition-colors disabled:opacity-60 ${
                selected
                  ? "border-teal/40 bg-teal/[0.06]"
                  : "border-cream-200 bg-white hover:border-teal/30"
              }`}
            >
              <span
                aria-hidden="true"
                className={`mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-full border ${
                  selected ? "border-teal bg-teal" : "border-cream-300 bg-white"
                }`}
              >
                {selected ? (
                  <span className="h-1.5 w-1.5 rounded-full bg-white" />
                ) : null}
              </span>
              <span className="min-w-0">
                <span className="flex items-center gap-2">
                  <span className="text-[12px] font-semibold text-cream-800">
                    {option.label}
                  </span>
                  {option.value === "auto" ? (
                    <span className="rounded-full bg-cream-100 px-1.5 py-0.5 text-[9px] font-semibold uppercase tracking-wider text-cream-500">
                      Default
                    </span>
                  ) : null}
                </span>
                <span className="mt-0.5 block text-[11px] leading-4 text-cream-500">
                  {option.description}
                </span>
              </span>
            </button>
          );
        })}
      </div>

      <div className="mt-2 flex h-4 items-center gap-2 text-[11px]">
        {savedTick ? (
          <span className="inline-flex items-center gap-1 text-sage-dark">
            <CheckCircle2 className="h-3.5 w-3.5" />
            Saved
          </span>
        ) : null}
      </div>

      {/* E2 — read-only coverage indicator. */}
      {coverageText !== null ? (
        <div className="mt-2 rounded-2xl border border-cream-200 bg-cream-50/60 px-3 py-2">
          <p className="text-[11px] leading-4 text-cream-700">
            <span className="font-semibold">Agentic-iterative coverage:</span>{" "}
            {coverageText}
          </p>
          <p className="mt-1 text-[10px] leading-4 text-cream-400">
            Actual coverage depends on the detected project (its manifests) and
            which language tools are installed on this machine.
          </p>
        </div>
      ) : null}

      {/* E3 — progressive-disclosure "How this works" explainer. */}
      <div className="mt-3">
        <button
          type="button"
          aria-expanded={explainerOpen}
          onClick={() => setExplainerOpen((open) => !open)}
          className="inline-flex items-center gap-1.5 text-[11px] font-semibold text-cream-600 hover:text-teal"
        >
          {explainerOpen ? (
            <ChevronDown className="h-3.5 w-3.5" />
          ) : (
            <ChevronRight className="h-3.5 w-3.5" />
          )}
          How this works
        </button>
        {explainerOpen ? (
          <div className="mt-2 space-y-2 rounded-2xl border border-cream-200 bg-cream-50/40 px-3 py-2.5 text-[11px] leading-4 text-cream-600">
            <p>
              Your coder delegates a write task to the local mini. The mini
              writes the change one of two ways:
            </p>
            <ul className="ml-3 list-disc space-y-1">
              <li>
                <span className="font-semibold">Emit-edits</span> — the mini
                returns one edit; the engine applies it and the mini fixes once.
              </li>
              <li>
                <span className="font-semibold">Agentic-iterative</span> — the
                mini loops, re-checking its work against a deterministic
                language-specific gate each round until it passes or the round
                budget is reached.
              </li>
            </ul>
            <p>
              The deterministic gate runs language-specific checks; the mini
              fixes what it flags, then your coder reviews the result. Final
              acceptance is always human-gated.
            </p>
            <p className="flex items-start gap-1.5 text-cream-500">
              <ShieldCheck className="mt-0.5 h-3.5 w-3.5 shrink-0 text-teal" />
              <span>
                Everything runs locally and sandboxed, and is opt-in. It
                degrades gracefully: with no local model the coder simply writes
                the change itself, and with no language tool the gate falls back
                to its baseline checks.
              </span>
            </p>
          </div>
        ) : null}
      </div>

      {error ? (
        <p className="mt-3 flex items-start gap-2 rounded-2xl border border-coral/30 bg-coral/[0.05] px-3 py-2 text-[11px] leading-4 text-coral-dark">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>{error}</span>
        </p>
      ) : null}
    </section>
  );
}

// Test-only alias kept for parity with the other extracted Settings cards.
export const __test_MiniWriteBehaviorCard = MiniWriteBehaviorCard;
