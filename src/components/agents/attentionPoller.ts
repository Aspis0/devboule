// GLOBAL attention poller (Phase G).
//
// After the standalone Agents page was dissolved, the Header attention bell + OS
// notifications are the PRIMARY "an agent needs you" signal. The bell is fed by
// the agentAttentionStore, which is a passive sink. ProjectsView's existing
// agent-live-state poll feeds it — but ONLY while the Projects view is open. So a
// needsUser raised while the user sits on the Projects Board / Polis / Oracle
// would never light the bell (nor fire a toast) until they wandered into Projects.
//
// This poller closes that gap with full, everywhere coverage while preserving the
// HARD "single feeder" invariant: at most ONE get_agent_live_state is ever in
// flight, and the global poller and ProjectsView never both fetch at once. It does
// that by SKIPPING its tick whenever the Projects view is active (ProjectsView is
// the feeder there). Net: exactly one attention feeder active at any moment.
//
// Two layers, same split as attentionNotifier so the decision is pure + testable:
//   1. shouldAttentionTick(...) — pure predicate: fetch iff unlocked && visible &&
//      not already in flight && the Projects view is NOT active. Unit-tested.
//   2. startAttentionPoller(...) — thin glue: a visibility-gated, in-flight-guarded
//      setInterval that, when the predicate passes, fetches the live state and
//      feeds ONLY the attention store (never board/project state). Returns an
//      unsubscribe for symmetric teardown.

import type { AgentLiveState } from "../../types/backend";

/** Poll cadence: matches ProjectsView's agent-live-state poll (~5s) so the bell's
 *  freshness is identical no matter which view is open. */
export const ATTENTION_POLL_MS = 5_000;

export interface AttentionTickInputs {
  /** App is unlocked (poller must be silent while locked). */
  unlocked: boolean;
  /** Document is currently visible (skip background ticks — no stacked polls). */
  visible: boolean;
  /** A get_agent_live_state issued by THIS poller is still on the wire. */
  inFlight: boolean;
  /** The currently active top-level view. When "projects", ProjectsView is the
   *  feeder, so the global poller stands down to keep a single feeder. */
  activeView: string;
}

/**
 * Pure decision: should this tick fetch get_agent_live_state?
 *
 * True iff the app is unlocked, the document is visible, no fetch from this poller
 * is already in flight, AND the Projects view is not the active view (it owns the
 * feed there). Any false branch makes the tick a no-op, guaranteeing the single
 * feeder invariant + no background/stacked polls.
 */
export function shouldAttentionTick(inputs: AttentionTickInputs): boolean {
  const { unlocked, visible, inFlight, activeView } = inputs;
  if (!unlocked) return false;
  if (!visible) return false;
  if (inFlight) return false;
  if (activeView === "projects") return false;
  return true;
}

export interface AttentionPollerDeps {
  /** Reads the live active view each tick (the poller is long-lived; the view
   *  changes under it, so this must be a getter, not a captured value). */
  getActiveView: () => string;
  /** Reads the LIVE unlocked state each tick. The poller is long-lived and a
   *  strict-mode/race re-mount could outlive a lock transition, so the gate must
   *  read the current lock state rather than infer it from the teardown flag — a
   *  tick must NEVER fetch/feed while the app is locked. Optional (defaults to
   *  always-unlocked) for callers/tests that gate mounting on lock themselves. */
  isUnlocked?: () => boolean;
  /** Fetch the live agent state (typically invokeBackendCommand<...>). */
  fetchLiveState: () => Promise<AgentLiveState | null>;
  /** Feed the fetched state into the attention store (setFromLiveState ONLY). */
  feed: (state: AgentLiveState | null) => void;
  /** Document-visibility reader (injectable for tests). Defaults to the real DOM. */
  isVisible?: () => boolean;
  /** Poll cadence override (tests). Defaults to ATTENTION_POLL_MS. */
  cadenceMs?: number;
  /** Schedule helpers (injectable for tests). Default to window timers. */
  setIntervalFn?: (cb: () => void, ms: number) => number;
  clearIntervalFn?: (id: number) => void;
}

/**
 * Start the global attention poller. Mounted ONCE in AppShell while unlocked
 * (alongside startAttentionWatcher). Returns an unsubscribe that stops the timer
 * and ignores any in-flight result (so a fetch that resolves after teardown never
 * feeds the store).
 *
 * In-flight guard: a single boolean. The tick sets it before fetching and clears
 * it in `finally`, so at most one fetch from this poller is ever outstanding — and
 * combined with the activeView gate, exactly one attention feeder is active app-wide.
 */
export function startAttentionPoller(deps: AttentionPollerDeps): () => void {
  const {
    getActiveView,
    isUnlocked = () => true,
    fetchLiveState,
    feed,
    isVisible = () => document.visibilityState === "visible",
    cadenceMs = ATTENTION_POLL_MS,
    setIntervalFn = (cb, ms) => window.setInterval(cb, ms),
    clearIntervalFn = (id) => window.clearInterval(id),
  } = deps;

  let inFlight = false;
  let cancelled = false;

  const tick = (): void => {
    if (
      !shouldAttentionTick({
        // BOTH gates: the live lock state (a re-mount could outlive a lock) AND
        // the teardown flag. Either being false makes the tick a no-op so we never
        // fetch/feed while locked or after teardown.
        unlocked: isUnlocked() && !cancelled,
        visible: isVisible(),
        inFlight,
        activeView: getActiveView(),
      })
    ) {
      return;
    }
    inFlight = true;
    void fetchLiveState()
      .then((state) => {
        // Drop the result if the poller was torn down OR the app locked meanwhile
        // so we never feed a stale snapshot after teardown / during a lock.
        if (!cancelled && isUnlocked()) feed(state);
      })
      .catch(() => {
        // Best-effort: a failed fetch (unlock lapsed, backend hiccup) just skips
        // this tick — the in-app bell keeps its last snapshot until the next one.
      })
      .finally(() => {
        inFlight = false;
      });
  };

  const timer = setIntervalFn(tick, cadenceMs);
  return () => {
    cancelled = true;
    clearIntervalFn(timer);
  };
}
