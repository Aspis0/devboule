// Dock-tab panel that shows the full history of plan approval requests for the
// current project. Click a row to expand the full rendered plan markdown.
//
// Status badges follow the existing color idiom:
//   pending_approval → amber
//   approved         → sage/green
//   rejected         → coral/red
//   timeout          → cream/gray

import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronUp } from "lucide-react";

import { invokeBackendCommand } from "../../context/AppContext";
import type { PlanApprovalRequest } from "../../types/backend";
import { PlanExecutionView } from "./PlanExecutionView";
import { stripSpoofChars } from "../agents/attentionNotifier";
import { parseMarkdown } from "../../utils/planMarkdown";
import { MarkdownRenderer } from "./MarkdownRenderer";

// Modest self-poll so the dock tab reflects an approval/rejection without a manual
// reopen (mirrors the PushApprovalCard self-poll idiom; ticks only when visible).
const PLANS_POLL_INTERVAL_MS = 12000;

export interface PlansPanelProps {
  plans: PlanApprovalRequest[];
}

interface ExpandedPlan {
  requestId: string;
  markdown: string | null;
  loading: boolean;
  error: string | null;
}

export function PlansPanel({ plans }: PlansPanelProps) {
  const [expanded, setExpanded] = useState<ExpandedPlan | null>(null);

  // Guard setState after unmount.
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Tracks the requestId of the last-initiated fetch so a slow resolve from a
  // previously-clicked row does not clobber the current row's loading state.
  const pendingRequestIdRef = useRef<string | null>(null);

  const toggle = useCallback(
    async (request: PlanApprovalRequest) => {
      if (expanded?.requestId === request.id) {
        setExpanded(null);
        pendingRequestIdRef.current = null;
        return;
      }
      pendingRequestIdRef.current = request.id;
      setExpanded({ requestId: request.id, markdown: null, loading: true, error: null });
      try {
        const md = await invokeBackendCommand<string>("get_plan_markdown", {
          projectId: request.projectId,
          planId: request.id,
        });
        // Discard result if a different row was clicked while we were in-flight.
        if (!mountedRef.current || pendingRequestIdRef.current !== request.id) return;
        setExpanded({ requestId: request.id, markdown: md ?? "", loading: false, error: null });
      } catch (e) {
        if (!mountedRef.current || pendingRequestIdRef.current !== request.id) return;
        // Tauri rejections are often plain strings, not Error.
        const message =
          typeof e === "string"
            ? e
            : e instanceof Error
              ? e.message
              : "Failed to load plan.";
        setExpanded({
          requestId: request.id,
          markdown: null,
          loading: false,
          error: message,
        });
      }
    },
    [expanded],
  );

  if (plans.length === 0) {
    return (
      <div className="py-8 text-center">
        <p className="text-[12px] text-cream-400">No plans submitted yet for this project.</p>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2">
      {plans.map((plan) => {
        const isExpanded = expanded?.requestId === plan.id;
        const agentId = stripSpoofChars(plan.agentId);
        const title = stripSpoofChars(plan.title);

        return (
          <div
            key={plan.id}
            className="rounded-lg border border-cream-200 bg-white"
          >
            <button
              type="button"
              onClick={() => void toggle(plan)}
              className="flex w-full items-start gap-3 p-3 text-left hover:bg-cream-50"
            >
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2 flex-wrap">
                  <span className="truncate text-[12px] font-semibold text-cream-800">
                    {title}
                  </span>
                  <StatusBadge status={plan.status} />
                </div>
                <p className="mt-0.5 text-[11px] text-cream-500">
                  {agentId} · {formatStamp(plan.createdAt)}
                  {plan.decidedAt ? ` · decided ${formatStamp(plan.decidedAt)}` : ""}
                </p>
                {plan.note && (
                  <p className="mt-0.5 truncate text-[11px] text-cream-600 italic">
                    {stripSpoofChars(plan.note)}
                  </p>
                )}
              </div>
              <span className="shrink-0 text-cream-400 mt-0.5">
                {isExpanded ? (
                  <ChevronUp className="h-4 w-4" aria-hidden />
                ) : (
                  <ChevronDown className="h-4 w-4" aria-hidden />
                )}
              </span>
            </button>

            {isExpanded && (
              <div className="border-t border-cream-100 rounded-b-lg bg-cream-50 p-3">
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
          </div>
        );
      })}
    </div>
  );
}

function StatusBadge({ status }: { status: PlanApprovalRequest["status"] }) {
  const cfg: Record<string, { cls: string; label: string }> = {
    pending_approval: {
      cls: "bg-amber/10 text-amber-dark border border-amber/30",
      label: "pending approval",
    },
    approved: {
      cls: "bg-sage/10 text-sage-dark border border-sage/30",
      label: "approved",
    },
    rejected: {
      cls: "bg-coral/10 text-coral-dark border border-coral/30",
      label: "rejected",
    },
    timeout: {
      cls: "bg-cream-100 text-cream-500 border border-cream-200",
      label: "timeout",
    },
  };
  const { cls, label } = cfg[status] ?? cfg.timeout;
  return (
    <span
      className={`inline-flex shrink-0 items-center rounded-full px-2 py-0.5 text-[10px] font-semibold ${cls}`}
    >
      {label}
    </span>
  );
}

function formatStamp(iso: string | null | undefined): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (isNaN(d.getTime())) return "—";
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

export default PlansPanel;

// ---- self-fetching dock-tab wrapper -----------------------------------------

/** Drop-in replacement for use in the ProjectWorkspace dock: fetches
 *  `list_project_plans` on mount and re-fetches on focus. */
export function PlansDockTab({ projectId }: { projectId: string }) {
  const [plans, setPlans] = useState<PlanApprovalRequest[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  // Monotonic generation: only the latest list response may write state
  // (a slow interval tick must not overwrite a fresher post-approve refresh).
  const fetchGenRef = useRef(0);
  const inFlightRef = useRef(false);

  // `showSpinner` is true only for the initial mount load; background poll refetches
  // run silently so the panel does not flash "Loading plans…" every interval.
  // `force` bypasses the in-flight guard so a post-approve refresh always runs.
  const fetch = useCallback(
    async (showSpinner: boolean, force = false) => {
      if (inFlightRef.current && !force) return;
      inFlightRef.current = true;
      const gen = ++fetchGenRef.current;
      if (showSpinner) setLoading(true);
      setError(null);
      try {
        const data = await invokeBackendCommand<PlanApprovalRequest[]>(
          "list_project_plans",
          { projectId },
        );
        if (!mountedRef.current || gen !== fetchGenRef.current) return;
        setPlans(data ?? []);
      } catch (e) {
        if (mountedRef.current && gen === fetchGenRef.current) {
          // Tauri rejections are often plain strings, not Error.
          const message =
            typeof e === "string"
              ? e
              : e instanceof Error
                ? e.message
                : "Failed to load plans.";
          setError(message);
        }
      } finally {
        if (gen === fetchGenRef.current) {
          inFlightRef.current = false;
          // Always clear loading for the latest gen (a force refresh can supersede
          // the mount spinner fetch; only the winner may leave loading stuck).
          if (mountedRef.current) setLoading(false);
        }
      }
    },
    [projectId],
  );

  useEffect(() => {
    mountedRef.current = true;
    void fetch(true);
    // Refetch on a modest interval so an approval/rejection shows up without a
    // manual reopen. Skip the tick while the tab is hidden (mirrors the app poller).
    const id = window.setInterval(() => {
      if (document.visibilityState === "visible") void fetch(false);
    }, PLANS_POLL_INTERVAL_MS);
    // F05: PlanApprovalCard fires this after approve/deny so history updates
    // immediately instead of waiting for the 12s poll. Force so a mid-interval
    // tick cannot make this a no-op; gen tokens drop any older response.
    const onRefresh = () => {
      if (document.visibilityState === "visible") void fetch(false, true);
    };
    window.addEventListener("devboule:plans-refresh", onRefresh);
    return () => {
      mountedRef.current = false;
      fetchGenRef.current++;
      window.clearInterval(id);
      window.removeEventListener("devboule:plans-refresh", onRefresh);
    };
  }, [fetch]);

  if (loading) {
    return (
      <p className="py-6 text-center text-[12px] text-cream-400">
        Loading plans…
      </p>
    );
  }
  if (error) {
    return (
      <p className="rounded-lg bg-coral/[0.06] px-3 py-2 text-[11px] text-coral-dark">
        {error}
      </p>
    );
  }
  return (
    <div className="flex flex-col gap-4">
      {/* Live plan execution — piece 2. Polls get_project independently on the
          same 12 s cadence so task state stays fresh without a second aggressive
          poller. Read-only (action buttons are piece 3). */}
      <section>
        <h3 className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-cream-400">
          Plan execution
        </h3>
        <PlanExecutionView projectId={projectId} />
      </section>

      {/* Approval history — unchanged. */}
      <section>
        <h3 className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-cream-400">
          Approval history
        </h3>
        <PlansPanel plans={plans} />
      </section>
    </div>
  );
}
