// "Plan execution" view — UX piece 2 (read-only render) + piece 3 Part B (skip/retry).
//
// Rendered above the approval history in PlansDockTab. Polls get_project on the
// same PLANS_POLL_INTERVAL_MS cadence (12 s) used by the approval-history poller.
//
// Piece 3 Part B adds per-row ⏭ skip + ↻ retry buttons wired to the dedicated
// `plan_task_control` Tauri command (NOT move_project_task — that path verifier-gates
// `done` so it cannot skip, and rejects a blocked task that still holds its failed
// attempt's claim). retry: blocked → todo (runner re-picks); skip: → done (terminal,
// runner skips it, dependents unblock). Stopping a RUNNING mini is NOT here — that is
// mini-scoped and lives in the Console Stop control (MiniSteerBar).
//
// CSP-strict: no dangerouslySetInnerHTML, no inline HTML on*= handlers, no eval.

import { useCallback, useEffect, useRef, useState } from "react";

import { invokeBackendCommand } from "../../context/AppContext";
import type { ProjectDetail, ProjectTaskStatus } from "../../types/backend";
import {
  buildPlanExecutionModel,
  type PlanExecutionModel,
  type PlanTaskRow,
} from "./planExecutionModel";

// Mirror the Plans approval poller cadence exactly.
export const PLAN_EXEC_POLL_INTERVAL_MS = 12000;

export type PlanControlAction = "skip" | "retry";

/** Retry only re-arms a failed (blocked) plan step. */
export function canRetry(status: ProjectTaskStatus): boolean {
  return status === "blocked";
}

/** Skip applies to non-terminal, non-running steps. A `wip` task is a RUNNING mini —
 *  stopping it is the Console Stop (MiniSteerBar), not a task skip. The backend
 *  plan_task_control(skip) rejects wip, so we gate it here too. */
export function canSkip(status: ProjectTaskStatus): boolean {
  return status !== "done" && status !== "wip";
}

// ---- pure render helpers ----------------------------------------------------

interface TaskRowProps {
  row: PlanTaskRow;
  /** Invoked with the task id + action when a control button is clicked. When
   *  omitted (read-only contexts), the control buttons are not rendered. */
  onControl?: (taskId: string, action: PlanControlAction) => void;
  /** True while THIS row's command is in flight — disables both its buttons. */
  busy?: boolean;
}

function TaskRow({ row, onControl, busy = false }: TaskRowProps) {
  return (
    <div className="flex items-center gap-2 py-1 min-w-0">
      {/* Status glyph */}
      <span className="shrink-0 text-[13px]" aria-label={row.status}>
        {row.glyph}
      </span>
      {/* Task id */}
      <span className="shrink-0 text-[11px] font-mono text-cream-500">
        {row.id}
      </span>
      {/* Title */}
      <span className="flex-1 min-w-0 truncate text-[12px] text-cream-800">
        {row.title}
      </span>
      {/* Dep label */}
      {row.depLabel !== "" && (
        <span className="shrink-0 text-[10px] text-cream-400 italic">
          {row.depLabel}
        </span>
      )}
      {/* Control buttons (piece 3 Part B) */}
      {onControl && (
        <span className="flex shrink-0 items-center gap-1">
          <button
            type="button"
            onClick={() => onControl(row.id, "retry")}
            disabled={busy || !canRetry(row.status)}
            aria-label={`Retry ${row.id}`}
            title="Retry (blocked → todo)"
            className="rounded px-1 text-[12px] text-cream-500 transition-colors hover:text-teal-dark disabled:cursor-not-allowed disabled:opacity-30"
          >
            ↻
          </button>
          <button
            type="button"
            onClick={() => onControl(row.id, "skip")}
            disabled={busy || !canSkip(row.status)}
            aria-label={`Skip ${row.id}`}
            title="Skip (mark done so the runner skips it)"
            className="rounded px-1 text-[12px] text-cream-500 transition-colors hover:text-terracotta disabled:cursor-not-allowed disabled:opacity-30"
          >
            ⏭
          </button>
        </span>
      )}
    </div>
  );
}

export interface PlanExecutionBodyProps {
  model: PlanExecutionModel;
  /** Per-row control callback (piece 3 Part B). Omit for a read-only render. */
  onControl?: (taskId: string, action: PlanControlAction) => void;
  /** The task id whose control command is currently in flight (disables its row). */
  busyTaskId?: string | null;
  /** A control error to surface above the rows (e.g. revision conflict). */
  controlError?: string | null;
}

export function PlanExecutionBody({
  model,
  onControl,
  busyTaskId = null,
  controlError = null,
}: PlanExecutionBodyProps) {
  if (model.activePlanId === null) {
    return (
      <p className="py-2 text-[11px] text-cream-400 italic">
        No active plan tasks.
      </p>
    );
  }

  return (
    <div className="flex flex-col">
      {controlError !== null && (
        <p className="mb-2 rounded-lg bg-coral/[0.06] px-3 py-1.5 text-[10px] font-semibold text-coral-dark">
          {controlError}
        </p>
      )}
      <div className="flex flex-col divide-y divide-cream-100">
        {model.rows.map((row) => (
          <TaskRow
            key={row.id}
            row={row}
            onControl={onControl}
            busy={busyTaskId === row.id}
          />
        ))}
      </div>
      {/* Footer */}
      <p className="mt-2 text-[10px] text-cream-400">
        {model.doneCount}/{model.totalCount} done · executor: local runner
      </p>
    </div>
  );
}

// ---- self-fetching component ------------------------------------------------

export interface PlanExecutionViewProps {
  projectId: string;
}

export function PlanExecutionView({ projectId }: PlanExecutionViewProps) {
  const [model, setModel] = useState<PlanExecutionModel>({
    activePlanId: null,
    rows: [],
    doneCount: 0,
    totalCount: 0,
  });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [controlBusyId, setControlBusyId] = useState<string | null>(null);
  const [controlError, setControlError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  // The latest known project revision, used as `expectedRevision` for plan_task_control
  // (optimistic-concurrency contract). A ref so the control handler always reads the
  // freshest value without re-creating the callback on every poll.
  const revisionRef = useRef<string | null>(null);

  const applyDetail = useCallback((detail: ProjectDetail | null) => {
    if (!mountedRef.current || !detail) return;
    revisionRef.current = detail.revision ?? null;
    setModel(buildPlanExecutionModel(detail.state?.tasks ?? []));
  }, []);

  const fetchProject = useCallback(
    async (showSpinner: boolean) => {
      if (showSpinner) setLoading(true);
      setError(null);
      try {
        const detail = await invokeBackendCommand<ProjectDetail>("get_project", {
          projectId,
        });
        applyDetail(detail);
      } catch (e) {
        if (mountedRef.current) {
          setError(e instanceof Error ? e.message : "Failed to load project.");
        }
      } finally {
        if (showSpinner && mountedRef.current) setLoading(false);
      }
    },
    [projectId, applyDetail],
  );

  const handleControl = useCallback(
    async (taskId: string, action: PlanControlAction) => {
      const expectedRevision = revisionRef.current;
      if (controlBusyId !== null || expectedRevision === null) return;
      setControlBusyId(taskId);
      setControlError(null);
      try {
        const detail = await invokeBackendCommand<ProjectDetail>(
          "plan_task_control",
          { projectId, taskId, action, expectedRevision },
        );
        // The command returns the updated project — apply it directly (its revision
        // becomes the basis for the next control action; no extra round trip).
        applyDetail(detail);
      } catch (e) {
        if (mountedRef.current) {
          setControlError(
            e instanceof Error ? e.message : "Plan control failed.",
          );
          // On a revision conflict (or any failure) re-sync so the next action uses a
          // fresh revision and the row statuses reflect on-disk reality.
          void fetchProject(false);
        }
      } finally {
        if (mountedRef.current) setControlBusyId(null);
      }
    },
    [projectId, controlBusyId, applyDetail, fetchProject],
  );

  useEffect(() => {
    mountedRef.current = true;
    // Per-effect cancellation flag: if projectId changes (or the component unmounts)
    // while a fetch is in-flight, the old fetch must not apply its stale result.
    // `mountedRef` alone is insufficient because the new effect sets it back to true
    // before the old fetch resolves, so the old result would slip through.
    let cancelled = false;

    const fetchWithCancellation = async (showSpinner: boolean) => {
      if (showSpinner) setLoading(true);
      setError(null);
      try {
        const detail = await invokeBackendCommand<ProjectDetail>("get_project", {
          projectId,
        });
        // Discard if this effect has been superseded (projectId changed) or unmounted.
        if (cancelled || !mountedRef.current || !detail) return;
        revisionRef.current = detail.revision ?? null;
        setModel(buildPlanExecutionModel(detail.state?.tasks ?? []));
      } catch (e) {
        if (!cancelled && mountedRef.current) {
          setError(e instanceof Error ? e.message : "Failed to load project.");
        }
      } finally {
        if (!cancelled && mountedRef.current && showSpinner) setLoading(false);
      }
    };

    void fetchWithCancellation(true);
    // Poll on the same cadence as the plans approval history; skip hidden ticks.
    const id = window.setInterval(() => {
      if (document.visibilityState === "visible") void fetchWithCancellation(false);
    }, PLAN_EXEC_POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      mountedRef.current = false;
      window.clearInterval(id);
    };
  }, [projectId]);

  if (loading) {
    return (
      <p className="py-2 text-[11px] text-cream-400">Loading plan…</p>
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
    <PlanExecutionBody
      model={model}
      onControl={handleControl}
      busyTaskId={controlBusyId}
      controlError={controlError}
    />
  );
}

export default PlanExecutionView;
