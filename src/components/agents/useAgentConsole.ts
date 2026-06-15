// useAgentConsole — the frontend TRANSPORT for the Agent Activity Console (Step A).
//
// It hydrates ONE agent's console state via the backend `mini_activity_snapshot`
// command and then keeps it live by subscribing to the per-agent
// `mini-activity://<agentId>` Tauri event channel. The reactive value it returns is
// a `ConsoleActivity` (see agentConsoleModel.ts) that AgentConsole.tsx renders
// directly.
//
// STEP B CONTRACT — what the backend MUST provide for this hook to light up:
//   - command `mini_activity_snapshot({ agentId }) -> ConsoleActivity`
//     (camelCase JSON matching agentConsoleModel.ts; an absent/empty run returns
//     `{ empty: true }` or simply `{}`).
//   - event channel `mini-activity://<agentId>` emitting `MiniActivityEvent`
//     payloads (the incremental update shape defined + documented below).
//
// Neither exists YET. So this hook DEGRADES GRACEFULLY and is the correct no-op
// today: it tries the snapshot, swallows any error, defaults to `{ empty: true }`
// when there is no data, and sets up the listener DEFENSIVELY — guarding when the
// Tauri runtime / `listen` is unavailable (tests, web, pre-backend). It cleans up
// (unlisten + ignore late events) on unmount AND on every agentId change.
//
// Concurrency correctness (mirrors useDesignStream.ts, the repo's established
// pattern): SUBSCRIBE BEFORE the snapshot invoke AND BUFFER-AND-REPLAY so no early
// event is lost. Subscribing first is not enough on its own: a flat
// `setActivity(snapshot)` would CLOBBER any event that arrived during the snapshot
// await. So while the snapshot is still pending the channel handler does NOT apply
// to state — it pushes each event into a local buffer; when the snapshot resolves we
// apply it as the base and REPLAY the buffered events in order, in a single
// functional update (so ordering is preserved and no event in the window is lost).
// After the snapshot has resolved the handler applies events directly. A per-run
// `active` guard ignores late callbacks from a SUPERSEDED agentId; we ALWAYS
// unlisten; the same guard prevents setState after unmount.
//
// PRIVACY: this hook moves only already-redacted `ConsoleActivity` summaries the
// backend chose to surface — no raw transcript, token, or secret is read here.

import { useEffect, useRef, useState } from "react";
import {
  type Action,
  type Banner,
  type ConsoleActivity,
  type ConsoleEntry,
  type Round,
  type Verdict,
} from "./agentConsoleModel";

// ---- the incremental update contract (Step B implements this) ---------------
//
// The simplest shape the component can apply without re-deriving anything: a
// FULL-SNAPSHOT replace plus a few coarse append/set deltas. The backend may emit
// ONLY `snapshot` events and this hook is fully correct (each replaces the whole
// state); the deltas exist so a chatty backend can avoid re-sending the world on
// every tool action. `applyMiniActivityEvent` below is the single, pure reducer —
// both the hook and Step B's tests use it, so the apply semantics can never drift.
//
// Delta addressing is by POSITION, kept deliberately coarse:
//   - appendEntry: push a new top-level row (coder milestone / spawn) to the end.
//   - appendRound: push a round onto the LAST entry's mini run (must be a spawn).
//   - appendAction: push an action onto a given round (by `roundIndex`) of the
//     LAST entry's mini run.
//   - setVerdict: set the verdict of a given round of the LAST entry's mini run.
//   - setBanner / setWorking: set the banner / working line of the LAST entry's
//     mini run (setWorking with `working: undefined` clears the shimmer).
//   - setRunning: update the tab spinner/count without touching the timeline.
//
// "LAST entry" is the live mini run — the only one a delta ever mutates while a run
// streams. A backend that wants to mutate an older entry sends a full `snapshot`.
export type MiniActivityEvent =
  | { type: "snapshot"; activity: ConsoleActivity }
  | { type: "appendEntry"; entry: ConsoleEntry }
  | { type: "appendRound"; round: Round }
  | { type: "appendAction"; roundIndex: number; action: Action }
  | { type: "setVerdict"; roundIndex: number; verdict: Verdict }
  | { type: "setBanner"; banner: Banner }
  | { type: "setWorking"; working?: string }
  | { type: "setRunning"; running: boolean; runCount?: number };

/** The channel name for an agent's console stream — MUST match the Rust side. */
export function miniActivityChannel(agentId: string): string {
  return `mini-activity://${agentId}`;
}

/** The empty resting state, returned before any data and whenever a snapshot is
 *  absent/empty. A fresh object each call so it is never aliased/mutated. */
function emptyActivity(): ConsoleActivity {
  return { empty: true };
}

/** Index of the LAST entry that owns a mini run (a spawn entry), or -1. Deltas only
 *  ever mutate the live mini run, which is the last spawn entry. */
function lastMiniEntryIndex(entries: ConsoleEntry[]): number {
  for (let i = entries.length - 1; i >= 0; i--) {
    if (entries[i].type === "spawn") return i;
  }
  return -1;
}

/**
 * Apply ONE incremental event to a prior `ConsoleActivity`, returning a NEW
 * activity (pure — never mutates `prev` or its nested arrays/objects, so React
 * identity changes propagate and the prior value is safe to keep). Unknown or
 * unappliable deltas (e.g. a round delta with no live mini run) return `prev`
 * UNCHANGED so a malformed/early event is a harmless no-op.
 */
export function applyMiniActivityEvent(
  prev: ConsoleActivity,
  event: MiniActivityEvent,
): ConsoleActivity {
  switch (event.type) {
    case "snapshot":
      // Full replace. Normalize an empty/missing snapshot to the resting state.
      return event.activity && Object.keys(event.activity).length > 0
        ? event.activity
        : emptyActivity();

    case "appendEntry": {
      const entries = [...(prev.entries ?? []), event.entry];
      // The first real entry clears the explicit empty flag.
      return { ...prev, empty: false, entries };
    }

    case "appendRound": {
      const entries = prev.entries ?? [];
      const idx = lastMiniEntryIndex(entries);
      if (idx < 0) return prev;
      const entry = entries[idx];
      if (entry.type !== "spawn") return prev;
      const nextEntry = {
        ...entry,
        mini: { ...entry.mini, rounds: [...entry.mini.rounds, event.round] },
      };
      const nextEntries = entries.slice();
      nextEntries[idx] = nextEntry;
      return { ...prev, entries: nextEntries };
    }

    case "appendAction":
    case "setVerdict": {
      const entries = prev.entries ?? [];
      const idx = lastMiniEntryIndex(entries);
      if (idx < 0) return prev;
      const entry = entries[idx];
      if (entry.type !== "spawn") return prev;
      const rounds = entry.mini.rounds;
      if (event.roundIndex < 0 || event.roundIndex >= rounds.length) return prev;
      const round = rounds[event.roundIndex];
      const nextRound: Round =
        event.type === "appendAction"
          ? { ...round, actions: [...round.actions, event.action] }
          : { ...round, verdict: event.verdict };
      const nextRounds = rounds.slice();
      nextRounds[event.roundIndex] = nextRound;
      const nextEntries = entries.slice();
      nextEntries[idx] = {
        ...entry,
        mini: { ...entry.mini, rounds: nextRounds },
      };
      return { ...prev, entries: nextEntries };
    }

    case "setBanner":
    case "setWorking": {
      const entries = prev.entries ?? [];
      const idx = lastMiniEntryIndex(entries);
      if (idx < 0) return prev;
      const entry = entries[idx];
      if (entry.type !== "spawn") return prev;
      const nextMini =
        event.type === "setBanner"
          ? { ...entry.mini, banner: event.banner }
          : { ...entry.mini, working: event.working };
      const nextEntries = entries.slice();
      nextEntries[idx] = { ...entry, mini: nextMini };
      return { ...prev, entries: nextEntries };
    }

    case "setRunning":
      return { ...prev, running: event.running, runCount: event.runCount };

    default: {
      // Exhaustiveness: the switch above covers every MiniActivityEvent variant.
      // `event` narrows to `never` here at compile time. At RUNTIME a forward-
      // compatible backend could emit a NEW variant this build doesn't know — so
      // we return `prev` unchanged (a harmless no-op), never crashing the console.
      const _exhaustive: never = event;
      void _exhaustive;
      return prev;
    }
  }
}

// ---- injectable Tauri surface ----------------------------------------------

export type UnlistenFn = () => void;

/** The minimal Tauri surface the hook needs, injectable so vitest drives it
 *  without a real runtime. The defaults dynamically import the real APIs and
 *  guard their absence (web / pre-backend / tests). */
export interface AgentConsoleDeps {
  /** Subscribe to `channel`; resolves to an unlisten fn (or a no-op when listen is
   *  unavailable so the caller's teardown is always safe to call). */
  listen: (
    channel: string,
    handler: (event: { payload: MiniActivityEvent }) => void,
  ) => Promise<UnlistenFn>;
  /** Fetch the initial snapshot. Rejects/throws are swallowed by the hook. */
  fetchSnapshot: (agentId: string) => Promise<ConsoleActivity>;
}

const defaultDeps: AgentConsoleDeps = {
  listen: async (channel, handler) => {
    // Guard: in a non-Tauri runtime (web/tests) the import or `listen` may be
    // missing — return a no-op unlisten so the hook degrades silently.
    try {
      const mod = await import("@tauri-apps/api/event");
      if (typeof mod.listen !== "function") return () => {};
      return await mod.listen<MiniActivityEvent>(channel, (event) =>
        handler({ payload: event.payload }),
      );
    } catch {
      return () => {};
    }
  },
  fetchSnapshot: async (agentId) => {
    const { invokeBackendCommand } = await import("../../context/AppContext");
    return invokeBackendCommand<ConsoleActivity>("mini_activity_snapshot", {
      agentId,
    });
  },
};

/**
 * React hook: the live `ConsoleActivity` for `agentId` (or the empty resting state
 * when null / no data / pre-backend). Subscribes BEFORE fetching the snapshot and
 * BUFFERS-AND-REPLAYS events that arrive during the snapshot await so none is lost
 * or clobbered (see the file header), applies incremental `MiniActivityEvent`s via
 * the pure reducer, and cleans up (unlisten + ignore late events) on unmount and on
 * every agentId change. `deps` is injectable for tests.
 */
export function useAgentConsole(
  agentId: string | null,
  deps: AgentConsoleDeps = defaultDeps,
): ConsoleActivity {
  const [activity, setActivity] = useState<ConsoleActivity>(emptyActivity);

  // Keep deps in a ref so the effect closes over the latest without re-subscribing
  // when a caller passes a fresh-but-equivalent deps object each render.
  const depsRef = useRef(deps);
  depsRef.current = deps;

  useEffect(() => {
    // No agent selected: reset to the resting state and wire nothing.
    if (!agentId) {
      setActivity(emptyActivity());
      return;
    }

    let active = true; // ignore every async result once superseded/unmounted
    let unlisten: UnlistenFn | null = null;
    const { listen, fetchSnapshot } = depsRef.current;

    // BUFFER-AND-REPLAY state, closure-local to this effect run. Until the snapshot
    // has resolved, the channel handler appends events here instead of applying them
    // to state; once the snapshot lands we replay them in order ON TOP of the base.
    // `snapshotApplied` flips the handler from buffering to direct-apply.
    const buffered: MiniActivityEvent[] = [];
    let snapshotApplied = false;

    // Reset to empty while (re)hydrating so a stale agent's timeline never lingers
    // under the newly-selected agent during the snapshot await.
    setActivity(emptyActivity());

    void (async () => {
      // SUBSCRIBE FIRST so an event arriving during the snapshot await is captured,
      // not lost. The default listen never throws (it guards a missing runtime),
      // but a custom dep might — so we still guard.
      try {
        const handle = await listen(miniActivityChannel(agentId), (event) => {
          if (!active) return;
          if (!snapshotApplied) {
            // In the subscribe→snapshot window: buffer for replay, do NOT apply yet
            // (a flat snapshot apply below would otherwise clobber this event).
            buffered.push(event.payload);
            return;
          }
          setActivity((current) =>
            applyMiniActivityEvent(current, event.payload),
          );
        });
        if (!active) {
          // Superseded/unmounted during the listen await: tear down immediately.
          try {
            handle();
          } catch {
            /* unlisten on a torn-down runtime is non-fatal */
          }
          return;
        }
        unlisten = handle;
      } catch {
        unlisten = null; // no live channel; the snapshot still hydrates a static view
      }

      // THEN fetch the initial snapshot. Swallow any error (the command does not
      // exist yet) and fall back to the empty resting state. EITHER WAY, apply the
      // base (snapshot or emptyActivity) and REPLAY the buffered events on top in
      // order, in a SINGLE functional update — so an event that arrived in the
      // window is preserved, not overwritten, even on the throw path (a pre-backend
      // live channel without a snapshot command must not lose its events).
      let base: ConsoleActivity;
      try {
        const snapshot = await fetchSnapshot(agentId);
        if (!active) return;
        base =
          snapshot && Object.keys(snapshot).length > 0
            ? snapshot
            : emptyActivity();
      } catch {
        if (!active) return;
        base = emptyActivity();
      }
      setActivity(() => {
        let next = base;
        for (const ev of buffered) next = applyMiniActivityEvent(next, ev);
        buffered.length = 0;
        return next;
      });
      // From here on the handler applies events directly (the buffer is drained and
      // will never be read again for this run).
      snapshotApplied = true;
    })();

    return () => {
      active = false;
      if (unlisten) {
        try {
          unlisten();
        } catch {
          /* unlisten on a torn-down runtime is non-fatal */
        }
        unlisten = null;
      }
    };
  }, [agentId]);

  return activity;
}
