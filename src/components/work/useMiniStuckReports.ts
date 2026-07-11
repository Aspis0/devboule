/**
 * useMiniStuckReports — subscribes to the global `mini://stuck` Tauri event
 * channel and surfaces a capped, newest-first list of `MiniStuckReport`s that
 * the `MiniStuckBanner` component renders.  Mirrors the guarded dynamic-import
 * pattern from `useAgentConsole.ts`: the listen call is wrapped in try/catch
 * and degrades to a no-op unlisten when the Tauri runtime is absent (web, tests,
 * pre-backend).
 */

import { useCallback, useEffect, useRef, useState } from "react";
import type { MiniStuckReport } from "./miniStuckModel";

const MAX_REPORTS = 5;

export type UnlistenFn = () => void;

/** Injectable deps so tests can drive the hook without a real Tauri runtime. */
export interface MiniStuckDeps {
  listen: (
    channel: string,
    handler: (event: { payload: MiniStuckReport }) => void,
  ) => Promise<UnlistenFn>;
}

const STUCK_CHANNEL = "mini://stuck";

const defaultDeps: MiniStuckDeps = {
  listen: async (channel, handler) => {
    try {
      const mod = await import("@tauri-apps/api/event");
      if (typeof mod.listen !== "function") return () => {};
      return await mod.listen<MiniStuckReport>(channel, (event) =>
        handler({ payload: event.payload }),
      );
    } catch {
      return () => {};
    }
  },
};

/**
 * Returns the current list of stuck reports (newest first, max 5) and a
 * `dismiss(taskId)` callback that removes a single report by its `taskId`.
 */
export function useMiniStuckReports(
  deps: MiniStuckDeps = defaultDeps,
): {
  reports: MiniStuckReport[];
  dismiss: (taskId: string) => void;
} {
  const [reports, setReports] = useState<MiniStuckReport[]>([]);
  const depsRef = useRef(deps);
  depsRef.current = deps;

  useEffect(() => {
    let active = true;
    let unlisten: UnlistenFn | null = null;

    void (async () => {
      try {
        const handle = await depsRef.current.listen(STUCK_CHANNEL, (event) => {
          if (!active) return;
          setReports((prev) => {
            const next = [event.payload, ...prev];
            return next.length > MAX_REPORTS ? next.slice(0, MAX_REPORTS) : next;
          });
        });
        if (!active) {
          try { handle(); } catch { /* ok */ }
          return;
        }
        unlisten = handle;
      } catch {
        unlisten = null;
      }
    })();

    return () => {
      active = false;
      if (unlisten) {
        try { unlisten(); } catch { /* ok */ }
        unlisten = null;
      }
    };
  }, []);

  const dismiss = useCallback((taskId: string) => {
    setReports((prev) => prev.filter((r) => r.taskId !== taskId));
  }, []);

  return { reports, dismiss };
}
