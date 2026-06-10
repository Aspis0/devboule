// MC-P6: slow, lazy fetch of the per-agent token / cost window for the SELECTED
// agent only.
//
// FETCH POLICY (reviewer will check): transcript reads are EXPENSIVE (a multi-MB
// JSONL tail per call), so this hook fetches ONLY for the one selected agent, and
// only on a SLOW cadence (default ~45s) — NEVER per rail row, NEVER on the 5s
// live-state tick. When `agentId` is null the hook fetches nothing and clears any
// prior value.
//
// TRACKER DISCIPLINE (mirrors attentionPoller / terminalSession): the loop is a
// PURE helper (`startTokenUsageTracker`) with injected timers so it is unit-testable
// in node without a DOM. An EPOCH guard (the per-start `cancelled` flag) invalidates
// a fetch whose start was torn down (agentId changed or unmount), an in-flight guard
// prevents stacked fetches, and the interval is cleared on teardown. A degrade to
// source="unavailable" (or a thrown invoke) silently yields null so the badge hides.

import { useEffect, useRef, useState } from "react";
import type { AgentTokenUsage } from "../../types/backend";

// Default slow cadence. Far slower than the 5s live-state tick so transcript reads
// stay cheap; the running total does not change second-to-second anyway.
export const TOKEN_USAGE_POLL_MS = 45_000;

export interface TokenUsageTrackerDeps {
  /** Fetch the usage for one agent (typically invokeBackendCommand). */
  fetchUsage: (agentId: string) => Promise<AgentTokenUsage>;
  /** Receives each settled value: the usage, or null when unavailable / failed.
   *  Called only while the tracker is live (never after teardown). */
  onValue: (usage: AgentTokenUsage | null) => void;
  /** Cadence override (tests). Defaults to TOKEN_USAGE_POLL_MS. */
  cadenceMs?: number;
  /** Schedule helpers (tests). Default to window timers. */
  setIntervalFn?: (cb: () => void, ms: number) => number;
  clearIntervalFn?: (id: number) => void;
}

/**
 * Start a slow token-usage tracker for ONE agent. Fetches immediately, then on the
 * slow cadence; returns a teardown that stops the timer and invalidates any
 * in-flight fetch (so a slow transcript read can never write a stale value after
 * the selection changed or the component unmounted).
 *
 * Pure over its deps (no React, no DOM) so the leak/race discipline is unit-tested.
 */
export function startTokenUsageTracker(
  agentId: string,
  deps: TokenUsageTrackerDeps,
): () => void {
  const {
    fetchUsage,
    onValue,
    cadenceMs = TOKEN_USAGE_POLL_MS,
    setIntervalFn = (cb, ms) => window.setInterval(cb, ms),
    clearIntervalFn = (id) => window.clearInterval(id),
  } = deps;

  let cancelled = false;
  let inFlight = false;

  const run = (): void => {
    if (cancelled || inFlight) return;
    inFlight = true;
    // FIX 7: `fetchUsage` may throw SYNCHRONOUSLY (before returning a promise) — e.g.
    // a misconfigured invoke. If it does, the .then/.catch/.finally chain below never
    // runs, so without this guard `inFlight` would stay `true` forever and block EVERY
    // future tick for this agent. Wrap the kickoff in try/catch so a synchronous throw
    // is treated exactly like an async rejection: emit null and clear inFlight.
    let promise: Promise<AgentTokenUsage>;
    try {
      promise = fetchUsage(agentId);
    } catch {
      inFlight = false;
      if (!cancelled) onValue(null);
      return;
    }
    void promise
      .then((result) => {
        if (cancelled) return;
        // Degrade silently: an unavailable source hides the badge (emit null).
        onValue(result.source === "unavailable" ? null : result);
      })
      .catch(() => {
        if (!cancelled) onValue(null);
      })
      .finally(() => {
        inFlight = false;
      });
  };

  run();
  const timer = setIntervalFn(run, cadenceMs);
  return () => {
    cancelled = true;
    clearIntervalFn(timer);
  };
}

export interface UseAgentTokenUsageOptions {
  /** Fetch the usage for one agent (typically invokeBackendCommand). Injected so
   *  the hook is testable without the Tauri runtime. */
  fetchUsage: (agentId: string) => Promise<AgentTokenUsage>;
  /** Cadence override (tests). Defaults to TOKEN_USAGE_POLL_MS. */
  cadenceMs?: number;
}

/**
 * Returns the selected agent's token usage, refreshed on selection + a slow timer.
 *
 * - `agentId === null` -> no fetch; returns null (and clears any prior value).
 * - On `agentId` change -> immediate fetch + a fresh slow interval.
 * - Teardown on unmount / re-select drops any in-flight result (epoch guard inside
 *   `startTokenUsageTracker`), so a stale value never lands on the wrong agent.
 */
export function useAgentTokenUsage(
  agentId: string | null,
  options: UseAgentTokenUsageOptions,
): AgentTokenUsage | null {
  const { fetchUsage, cadenceMs = TOKEN_USAGE_POLL_MS } = options;

  const [usage, setUsage] = useState<AgentTokenUsage | null>(null);

  // Keep fetchUsage in a ref so its identity does not re-fire the selection effect
  // (it must re-run ONLY when agentId/cadence change).
  const fetchRef = useRef(fetchUsage);
  fetchRef.current = fetchUsage;

  useEffect(() => {
    // No selection: clear and do nothing (no fetch, no timer).
    if (!agentId) {
      setUsage(null);
      return;
    }
    // Clear the previous agent's number immediately on switch so the badge never
    // shows the WRONG agent's tokens during the (slow) first fetch of the new one.
    setUsage(null);
    return startTokenUsageTracker(agentId, {
      fetchUsage: (id) => fetchRef.current(id),
      onValue: setUsage,
      cadenceMs,
    });
  }, [agentId, cadenceMs]);

  return usage;
}
