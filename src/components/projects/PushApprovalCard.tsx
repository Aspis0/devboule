// GH-P4: the agent push-approval gate UI. Agents COMMIT freely, but every PUSH
// must be approved by the human. The agent's MCP `request_git_push` tool appends a
// `pending_approval` request; this card surfaces those for the CURRENT project and
// lets the human Approve (the backend then performs the push) or Deny.
//
// It mirrors the mini-coder needs-you card idiom: a compact, attention-styled card
// in the Work-mode shell, reusing the existing design tokens (cream/terracotta/teal/
// amber/coral), no new visual style. It polls `git_push_requests_list` on the same
// ~5s cadence the rail uses, visibility-gated and in-flight-guarded, and cleans up
// its interval on unmount (no stale closures, no setState-after-unmount).
//
// PRIVACY: it renders only the agent id, the (display-only) branch, the remote name,
// a FORCE marker, and — for a just-resolved request — the ALREADY-SANITIZED push
// result/error the backend redacted (the token can never reach here). No raw URL.

import { useCallback, useEffect, useRef, useState } from "react";
import { AlertTriangle, GitBranch } from "lucide-react";

import { invokeBackendCommand } from "../../context/AppContext";
import type { GitPushRequest } from "../../types/backend";
import {
  pendingPushRequestsForProject,
  pushRequestSummary,
} from "./projectWorkspaceModel";

const POLL_INTERVAL_MS = 5000;
// FIX F11: how long the just-resolved result line lingers before auto-hiding, so the
// card disappears once there is nothing pending instead of staying up forever.
const RESOLVED_LINGER_MS = 8000;

export interface PushApprovalCardProps {
  projectId: string;
}

export function PushApprovalCard({ projectId }: PushApprovalCardProps) {
  const [requests, setRequests] = useState<GitPushRequest[]>([]);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  // The last resolved request to flash a short result line (sanitized).
  const [lastResolved, setLastResolved] = useState<GitPushRequest | null>(null);

  const mountedRef = useRef(true);
  const inFlightRef = useRef(false);
  // FIX F3: a synchronous in-flight guard for resolve. `busyId` is React state, so a
  // fast double-click fires TWO IPC calls before the first setState commits. The ref
  // is updated synchronously, mirroring `inFlightRef` in `load`, so the second click
  // is rejected immediately; `busyId` state is kept purely for rendering.
  const busyRef = useRef<string | null>(null);
  // FIX F11: the auto-dismiss timer for the last-resolved result line, so we can
  // clear it on unmount / on the next action (no stale timer, no setState-after-unmount).
  const resolvedTimerRef = useRef<number | null>(null);

  const clearResolvedTimer = useCallback(() => {
    if (resolvedTimerRef.current !== null) {
      window.clearTimeout(resolvedTimerRef.current);
      resolvedTimerRef.current = null;
    }
  }, []);

  const load = useCallback(async () => {
    // In-flight guard: never overlap two list fetches (a slow tick must not stack).
    if (inFlightRef.current) return;
    inFlightRef.current = true;
    try {
      const all = await invokeBackendCommand<GitPushRequest[]>(
        "git_push_requests_list",
      );
      if (!mountedRef.current) return;
      setRequests(pendingPushRequestsForProject(all, projectId));
    } catch {
      // A degraded fetch must not crash the card; keep the prior pending list.
      if (mountedRef.current) setRequests((prev) => prev);
    } finally {
      inFlightRef.current = false;
    }
  }, [projectId]);

  useEffect(() => {
    mountedRef.current = true;
    void load();
    const id = window.setInterval(() => {
      // Skip the tick when the tab is hidden (mirrors the app attention poller).
      if (document.visibilityState === "visible") void load();
    }, POLL_INTERVAL_MS);
    return () => {
      mountedRef.current = false;
      window.clearInterval(id);
      clearResolvedTimer();
    };
  }, [load, clearResolvedTimer]);

  const resolve = useCallback(
    async (request: GitPushRequest, command: string) => {
      // FIX F3: synchronous guard — reject a second click before the first setState
      // commits (one action at a time, no double IPC round-trip / flicker).
      if (busyRef.current) return;
      busyRef.current = request.id;
      setBusyId(request.id);
      setActionError(null);
      // A new action supersedes any lingering resolved line; cancel its timer.
      clearResolvedTimer();
      try {
        const updated = await invokeBackendCommand<GitPushRequest>(command, {
          requestId: request.id,
        });
        if (!mountedRef.current) return;
        setLastResolved(updated);
        // FIX F11: auto-hide the resolved line after a short linger, so the card
        // disappears once there is nothing pending instead of staying up forever.
        resolvedTimerRef.current = window.setTimeout(() => {
          resolvedTimerRef.current = null;
          if (mountedRef.current) setLastResolved(null);
        }, RESOLVED_LINGER_MS);
        // Refresh the pending list immediately (the resolved one drops out).
        // FIX 9: force-clear the in-flight guard first. If an interval-driven `load()`
        // is mid-flight when we get here, the guard would make THIS refresh a no-op,
        // leaving the just-resolved request shown as still pending for up to one poll
        // interval. Resetting the ref bypasses the guard so the post-action refresh
        // always runs (the concurrent tick's own `finally` resetting it to false is
        // harmless — it only re-clears an already-false flag).
        inFlightRef.current = false;
        await load();
      } catch (e) {
        if (mountedRef.current) {
          setActionError(e instanceof Error ? e.message : "Action failed.");
        }
      } finally {
        busyRef.current = null;
        if (mountedRef.current) setBusyId(null);
      }
    },
    [load, clearResolvedTimer],
  );

  if (requests.length === 0 && !lastResolved && !actionError) {
    return null; // nothing pending and nothing to report — stay invisible.
  }

  return (
    <div className="flex flex-col gap-2 rounded-2xl border border-amber/30 bg-amber/[0.05] p-3">
      <div className="flex items-center gap-2">
        <AlertTriangle className="h-4 w-4 text-amber-dark" aria-hidden />
        <h3 className="text-[12px] font-semibold text-amber-dark">
          Push approval needed
        </h3>
      </div>

      {requests.map((request) => (
        <div
          key={request.id}
          className="flex flex-col gap-2 rounded-lg border border-cream-200 bg-white p-2.5 sm:flex-row sm:items-center sm:justify-between"
        >
          <div className="min-w-0">
            <p className="truncate text-[12px] font-semibold text-cream-800">
              <span className="text-cream-500">{request.agentId}</span> wants to
              push
            </p>
            <p className="mt-0.5 flex items-center gap-1.5 text-[11px] text-cream-600">
              <GitBranch className="h-3 w-3 text-cream-400" aria-hidden />
              {pushRequestSummary(request)}
            </p>
            {request.force && (
              <p className="mt-0.5 text-[11px] font-semibold text-coral-dark">
                Force push — this can overwrite remote history.
              </p>
            )}
          </div>
          <div className="flex shrink-0 gap-2">
            <button
              type="button"
              onClick={() => void resolve(request, "deny_git_push_request")}
              disabled={busyId !== null}
              className="rounded-lg border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-700 hover:bg-cream-50 disabled:opacity-60"
            >
              Deny
            </button>
            <button
              type="button"
              onClick={() => void resolve(request, "approve_git_push_request")}
              disabled={busyId !== null}
              className={`rounded-lg px-3 py-1.5 text-[12px] font-semibold text-white disabled:opacity-60 ${
                request.force
                  ? "bg-coral hover:bg-coral/90"
                  : "bg-teal hover:bg-teal/90"
              }`}
            >
              {busyId === request.id ? "Working…" : "Approve"}
            </button>
          </div>
        </div>
      ))}

      {actionError && (
        <p className="rounded-lg bg-coral/[0.06] px-3 py-2 text-[11px] font-semibold text-coral-dark">
          {actionError}
        </p>
      )}

      {/* Short, sanitized result of the most recent approve/deny (token-redacted
          upstream by git_run_authenticated). */}
      {lastResolved && (
        <p
          className={`rounded-lg px-3 py-2 text-[11px] font-semibold ${
            lastResolved.status === "pushed"
              ? "bg-sage/10 text-sage-dark"
              : "bg-cream-50 text-cream-600"
          }`}
        >
          {pushResolvedLine(lastResolved)}
        </p>
      )}
    </div>
  );
}

/** A short human line for a just-resolved request. Renders only the sanitized
 *  result fields the backend stored (never a token/URL). */
function pushResolvedLine(request: GitPushRequest): string {
  const result = request.result;
  switch (request.status) {
    case "pushed":
      return `Pushed. ${result?.output ?? ""}`.trim();
    case "push_failed":
      return `Push failed: ${result?.error ?? "unknown error"}`;
    case "denied":
      return "Push denied.";
    default:
      return `Push request is ${request.status}.`;
  }
}

export default PushApprovalCard;
