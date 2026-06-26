// ConsentBridgePoller — Slice 5b. A RENDERLESS self-contained poller for the Claude consent
// file-bridge, mirroring the PushApprovalCard self-poll idiom (keeps ProjectWorkspace itself
// free of any `setInterval` / live-state poller — see workspaceNoSecondPoller invariant).
//
// The `claude_consent_hook` process writes a `pending_approval` entry into `.aspis-agents.json`
// and bounded-polls it; there is no push event. This component polls `consent_requests_list`
// every 4s and hands the pending entries (mapped to the shared ConsentRequest shape) up to the
// parent via `onPending`, which enqueues them into the SAME FIFO + modal the event listener and
// the Codex path use. It does NOT poll `get_agent_live_state` (the single-feeder invariant is
// about that command only).

import { useEffect, useRef } from "react";
import { invokeBackendCommand, isTauriRuntime } from "../../context/AppContext";
import {
  pendingConsentBridgeForProject,
  type ConsentBridgeRequest,
  type ConsentRequest,
} from "./netConsentModel";

const POLL_INTERVAL_MS = 4000;

export interface ConsentBridgePollerProps {
  projectId: string;
  /** Called with the pending bridge requests (mapped to ConsentRequest) on each poll. */
  onPending: (pending: ConsentRequest[]) => void;
}

export function ConsentBridgePoller({ projectId, onPending }: ConsentBridgePollerProps) {
  // Keep the latest callback in a ref so a changing parent closure doesn't restart the interval.
  const onPendingRef = useRef(onPending);
  onPendingRef.current = onPending;

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let cancelled = false;
    let inFlight = false;
    const poll = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const all = await invokeBackendCommand<ConsentBridgeRequest[]>(
          "consent_requests_list",
        );
        if (cancelled) return;
        const pending = pendingConsentBridgeForProject(all, projectId);
        if (pending.length > 0) onPendingRef.current(pending);
      } catch {
        // A degraded fetch must not crash the workspace; keep the prior queue.
      } finally {
        inFlight = false;
      }
    };
    void poll();
    const id = window.setInterval(() => {
      if (document.visibilityState === "visible") void poll();
    }, POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [projectId]);

  return null;
}

export default ConsentBridgePoller;
