// Plan approval gate UI. Mirrors the PushApprovalCard pattern: compact,
// attention-styled card surfacing pending plan-approval requests for the
// current project, letting the human Approve or Reject with an optional note.
//
// PRIVACY: renders only agentId, title, and age. Plan markdown is fetched
// on expand and rendered via MarkdownRenderer (text nodes only — no HTML).

import { useCallback, useEffect, useRef, useState } from "react";
import { AlertTriangle, ChevronDown, ChevronUp } from "lucide-react";

import { invokeBackendCommand } from "../../context/AppContext";
import type { PlanApprovalRequest } from "../../types/backend";
import { stripSpoofChars } from "../agents/attentionNotifier";
import { parseMarkdown } from "../../utils/planMarkdown";
import { MarkdownRenderer } from "./MarkdownRenderer";
import { pendingPlanRequestsForProject } from "./projectWorkspaceModel";

const POLL_INTERVAL_MS = 5000;
const RESOLVED_LINGER_MS = 8000;

export interface PlanApprovalCardProps {
  projectId: string;
  /** Optional: if provided, the card is controlled (no internal polling).
   *  The parent passes the full requests list; the card filters by projectId.
   *  When omitted the card polls independently (self-contained mode). */
  requests?: PlanApprovalRequest[];
  /** Optional: fires whenever the pending request count changes. Used by the
   *  parent to feed the Plans tab badge count without polling itself. */
  onPendingCountChange?: (count: number) => void;
  /** Optional: when true, the component still runs its full poll/derive/callback
   *  logic (so the count keeps updating) but renders nothing. Default false. */
  hidden?: boolean;
}

interface ExpandedPlan {
  requestId: string;
  markdown: string | null;
  loading: boolean;
  error: string | null;
}

interface ResolvedEntry {
  id: string;
  status: string;
}

export function PlanApprovalCard({ projectId, requests: externalRequests, onPendingCountChange, hidden = false }: PlanApprovalCardProps) {
  const [polledRequests, setPolledRequests] = useState<PlanApprovalRequest[]>([]);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [noteById, setNoteById] = useState<Record<string, string>>({});
  const [expanded, setExpanded] = useState<ExpandedPlan | null>(null);
  const [lastResolved, setLastResolved] = useState<ResolvedEntry | null>(null);

  const mountedRef = useRef(true);
  const inFlightRef = useRef(false);
  const busyRef = useRef<string | null>(null);
  const resolvedTimerRef = useRef<number | null>(null);
  // Monotonic generation: only the latest poll/expand response may write state
  // (mirrors ChangesDockTab requestToken — older responses are dropped).
  const pollGenRef = useRef(0);
  const mdGenRef = useRef(0);

  const clearResolvedTimer = useCallback(() => {
    if (resolvedTimerRef.current !== null) {
      window.clearTimeout(resolvedTimerRef.current);
      resolvedTimerRef.current = null;
    }
  }, []);

  // When requests come from the parent (controlled mode) we skip the poller.
  const isControlled = externalRequests !== undefined;

  const load = useCallback(async () => {
    if (isControlled) return;
    if (inFlightRef.current) return;
    inFlightRef.current = true;
    const gen = ++pollGenRef.current;
    try {
      const all = await invokeBackendCommand<PlanApprovalRequest[]>(
        "plan_approval_requests_list",
      );
      if (!mountedRef.current || gen !== pollGenRef.current) return;
      setPolledRequests(pendingPlanRequestsForProject(all, projectId));
    } catch {
      // Keep prior list; only the latest generation may touch state.
      if (mountedRef.current && gen === pollGenRef.current) {
        setPolledRequests((prev) => prev);
      }
    } finally {
      if (gen === pollGenRef.current) inFlightRef.current = false;
    }
  }, [projectId, isControlled]);

  useEffect(() => {
    mountedRef.current = true;
    void load();
    if (isControlled) return;
    const id = window.setInterval(() => {
      if (document.visibilityState === "visible") void load();
    }, POLL_INTERVAL_MS);
    return () => {
      mountedRef.current = false;
      pollGenRef.current++;
      mdGenRef.current++;
      window.clearInterval(id);
      clearResolvedTimer();
    };
  }, [load, clearResolvedTimer, isControlled]);

  // Derive the visible pending requests: controlled vs. self-polled.
  const requests = isControlled
    ? pendingPlanRequestsForProject(externalRequests, projectId)
    : polledRequests;

  // Notify the parent of the pending count whenever it changes.
  // MUST be before the early return below (Rules of Hooks).
  useEffect(() => {
    onPendingCountChange?.(requests.length);
  }, [requests.length, onPendingCountChange]);

  const fetchMarkdown = useCallback(
    async (request: PlanApprovalRequest) => {
      if (expanded?.requestId === request.id) {
        // Toggle off if already expanded; invalidate any in-flight expand fetch.
        mdGenRef.current++;
        setExpanded(null);
        return;
      }
      const gen = ++mdGenRef.current;
      setExpanded({ requestId: request.id, markdown: null, loading: true, error: null });
      try {
        const md = await invokeBackendCommand<string>("get_plan_markdown", {
          projectId: request.projectId,
          planId: request.id,
        });
        if (!mountedRef.current || gen !== mdGenRef.current) return;
        // F04: empty/failed load must not render MarkdownRenderer(null) → blank
        // expand with no error. Surface an explicit message instead.
        const text = (md ?? "").trim();
        // Only apply if this request is still the expanded one (user may have
        // collapsed or opened another plan while the fetch was in flight).
        setExpanded((prev) => {
          if (!prev || prev.requestId !== request.id) return prev;
          if (!text) {
            return {
              requestId: request.id,
              markdown: null,
              loading: false,
              error: "Plan markdown is empty or missing on disk.",
            };
          }
          return {
            requestId: request.id,
            markdown: text,
            loading: false,
            error: null,
          };
        });
      } catch (e) {
        if (!mountedRef.current || gen !== mdGenRef.current) return;
        setExpanded((prev) => {
          if (!prev || prev.requestId !== request.id) return prev;
          return {
            requestId: request.id,
            markdown: null,
            loading: false,
            error: e instanceof Error ? e.message : "Failed to load plan.",
          };
        });
      }
    },
    [expanded],
  );

  const resolve = useCallback(
    async (request: PlanApprovalRequest, command: string) => {
      if (busyRef.current) return;
      busyRef.current = request.id;
      setBusyId(request.id);
      setActionError(null);
      clearResolvedTimer();
      const note = noteById[request.id]?.trim() || undefined;
      try {
        const updated = await invokeBackendCommand<PlanApprovalRequest>(command, {
          requestId: request.id,
          ...(note ? { note } : {}),
        });
        if (!mountedRef.current) return;
        setLastResolved({ id: updated.id, status: updated.status });
        resolvedTimerRef.current = window.setTimeout(() => {
          resolvedTimerRef.current = null;
          if (mountedRef.current) setLastResolved(null);
        }, RESOLVED_LINGER_MS);
        if (expanded?.requestId === request.id) {
          mdGenRef.current++;
          setExpanded(null);
        }
        // F05: notify PlansDockTab to refetch approval history immediately.
        try {
          window.dispatchEvent(new CustomEvent("devboule:plans-refresh"));
        } catch {
          /* ignore */
        }
        // Force a post-action refresh (same as PushApprovalCard): an interval poll
        // mid-flight would make load() a no-op under inFlightRef; generation tokens
        // then drop that poll's stale list once this refresh completes.
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
    [load, clearResolvedTimer, noteById, expanded],
  );

  if (hidden || (requests.length === 0 && !lastResolved && !actionError)) {
    return null;
  }

  return (
    <div className="flex flex-col gap-2 rounded-2xl border border-amber/30 bg-amber/[0.05] p-3">
      <div className="flex items-center gap-2">
        <AlertTriangle className="h-4 w-4 text-amber-dark" aria-hidden />
        <h3 className="text-[12px] font-semibold text-amber-dark">
          Plan approval needed
        </h3>
      </div>

      {requests.map((request) => {
        const isExpanded = expanded?.requestId === request.id;
        const agentId = stripSpoofChars(request.agentId);
        const title = stripSpoofChars(request.title);
        const note = noteById[request.id] ?? "";

        return (
          <div
            key={request.id}
            className="flex flex-col gap-2 rounded-lg border border-cream-200 bg-white p-2.5"
          >
            <div className="flex items-start justify-between gap-2">
              <div className="min-w-0 flex-1">
                <p className="truncate text-[12px] font-semibold text-cream-800">
                  <span className="text-cream-500">{agentId}</span> wants approval
                </p>
                <p className="mt-0.5 text-[12px] text-cream-700">{title}</p>
                <p className="mt-0.5 text-[11px] text-cream-400">
                  {formatAge(request.createdAt)}
                </p>
              </div>
              <button
                type="button"
                onClick={() => void fetchMarkdown(request)}
                className="shrink-0 rounded-lg p-1 text-cream-400 hover:bg-cream-50 hover:text-cream-700"
                aria-label={isExpanded ? "Collapse plan" : "Expand plan"}
              >
                {isExpanded ? (
                  <ChevronUp className="h-4 w-4" aria-hidden />
                ) : (
                  <ChevronDown className="h-4 w-4" aria-hidden />
                )}
              </button>
            </div>

            {isExpanded && (
              <div className="rounded-lg border border-cream-100 bg-cream-50 p-3">
                {expanded.loading && (
                  <p className="text-[11px] text-cream-400">Loading plan…</p>
                )}
                {expanded.error && (
                  <p className="text-[11px] text-coral-dark">{expanded.error}</p>
                )}
                {!expanded.loading && !expanded.error && expanded.markdown !== null && (
                  <MarkdownRenderer blocks={parseMarkdown(expanded.markdown)} />
                )}
              </div>
            )}

            <textarea
              value={note}
              onChange={(e) =>
                setNoteById((prev) => ({ ...prev, [request.id]: e.target.value }))
              }
              placeholder="Optional note (shown to the agent)"
              rows={2}
              className="w-full resize-none rounded-lg border border-cream-200 bg-white px-2.5 py-2 text-[12px] text-cream-800 placeholder:text-cream-400 focus:outline-none focus:ring-1 focus:ring-teal/40"
            />

            <div className="flex justify-end gap-2">
              <button
                type="button"
                onClick={() => void resolve(request, "deny_plan_request")}
                disabled={busyId !== null}
                className="rounded-lg border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-700 hover:bg-cream-50 disabled:opacity-60"
              >
                Reject
              </button>
              <button
                type="button"
                onClick={() => void resolve(request, "approve_plan_request")}
                disabled={busyId !== null}
                className="rounded-lg bg-teal px-3 py-1.5 text-[12px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60"
              >
                {busyId === request.id ? "Working…" : "Approve"}
              </button>
            </div>
          </div>
        );
      })}

      {actionError && (
        <p className="rounded-lg bg-coral/[0.06] px-3 py-2 text-[11px] font-semibold text-coral-dark">
          {actionError}
        </p>
      )}

      {lastResolved && (
        <p
          className={`rounded-lg px-3 py-2 text-[11px] font-semibold ${
            lastResolved.status === "approved"
              ? "bg-sage/10 text-sage-dark"
              : "bg-cream-50 text-cream-600"
          }`}
        >
          {lastResolved.status === "approved"
            ? "Plan approved."
            : lastResolved.status === "rejected"
              ? "Plan rejected."
              : `Plan ${lastResolved.status}.`}
        </p>
      )}
    </div>
  );
}

function formatAge(iso: string): string {
  const ms = Date.now() - Date.parse(iso);
  if (!Number.isFinite(ms) || ms < 0) return "";
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

export default PlanApprovalCard;
