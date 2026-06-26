// The dock's Censor tab content (Phase E2): the roborev-style findings list.
//
// Data flow (NO new poller): a CensorFindingsTracker fetches censor_get_findings
// once on mount and REFETCHES only on `censor://findings-updated` for this
// project (a listener, cleaned up on unmount). A one-shot censor_status read drives
// the Gemma-offline / tool-absent hints. Findings are grouped by file and rendered
// with CensorFindingRow.
//
// Strip actions:
//   - Review now → censor_review_now (whole-project sweep).
//   - Run final review → launches a `verifier` in-app via the existing project
//     launch path (finalReviewLaunchInput). TODO(Phase H): thread the verifier's
//     Censor residual-adjudication addendum + the `censorReview` flag through here;
//     for now we just launch a verifier scoped to the project.
//
// SECRET SAFETY: only the safe finding fields are rendered (title/body are the
// engine's already-redacted summaries); nothing here logs shard contents.

import { useEffect, useMemo, useRef, useState } from "react";
import { Play, ShieldCheck, RefreshCw, ShieldAlert, ShieldOff, XCircle } from "lucide-react";
import { installHintFor } from "./censorInstallHints";
import { invokeBackendCommand, isTauriRuntime } from "../../context/AppContext";
import type {
  CensorDisposition,
  CensorFinding,
  CensorStatus,
} from "../../types/backend";
import type { SpawnLaunchInput } from "../agents/agentRowModel";
import { CensorFindingRow } from "./CensorFindingRow";
import { gemmaStatusNote } from "./censorSeverity";
import {
  CensorFindingsTracker,
  censorPanelViewState,
  disposeArgs,
  finalReviewLaunchInput,
  groupFindingsByFile,
  openInEditorArgs,
  reviewNowArgs,
  setCensorTrustedArgs,
} from "./censorPanelModel";

export interface CensorPanelProps {
  projectId: string;
  /** The project's working root (ProjectDetail.metadata.rootPath). */
  root: string | null;
  /** When provided, the parent owns the findings feed (a single shared
   *  CensorFindingsTracker); this panel renders them and skips its own tracker
   *  to avoid a duplicate subscription + doubled IPC. */
  findings?: CensorFinding[];
  /** Launches an agent via the same path ProjectsView uses ("Run final review"). */
  onLaunch: (input: SpawnLaunchInput) => void;
  /** True while a launch/git op is in flight (disables Run final review). */
  isBusy?: boolean;
  /** Whether the project may launch agents (RBAC / config gate). */
  canLaunch?: boolean;
  /** Preferred editor for opening file:line (default vscode). */
  editor?: string;
}

export function CensorPanel({
  projectId,
  root,
  findings: findingsProp,
  onLaunch,
  isBusy = false,
  canLaunch = true,
  editor = "vscode",
}: CensorPanelProps) {
  const ownsFeed = findingsProp === undefined;
  const [localFindings, setFindings] = useState<CensorFinding[]>([]);
  const findings = findingsProp ?? localFindings;
  const [status, setStatus] = useState<CensorStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionBusy, setActionBusy] = useState(false);
  // Synchronous reentrancy guard for the dispose/open/review commands (a state
  // flag cannot gate a same-tick double-click).
  const actionBusyRef = useRef(false);

  const trimmedRoot = (root ?? "").trim();

  // Event-driven findings: initial fetch + refetch on censor://findings-updated.
  // ONE tracker per (project, root); cleaned up on unmount / project change.
  useEffect(() => {
    // When the parent owns the findings feed, this panel does not run its own tracker.
    if (!ownsFeed) return;
    if (!isTauriRuntime()) return;
    if (!trimmedRoot) {
      setFindings([]);
      return;
    }
    const tracker = new CensorFindingsTracker({
      projectId,
      root: trimmedRoot,
      invoke: invokeBackendCommand,
      listen: async (channel, handler) => {
        const { listen } = await import("@tauri-apps/api/event");
        return listen(channel, (event) => handler({ payload: event.payload }));
      },
      onChange: (next) => {
        setFindings(next);
        setLoadError(null);
      },
      onError: (message) => setLoadError(message),
    });
    void tracker.start();
    return () => tracker.stop();
  }, [projectId, trimmedRoot, ownsFeed]);

  // One-shot status read (Gemma availability + detected linters). Re-read when the
  // project root changes; refreshed alongside a manual Review now.
  const refreshStatus = useMemo(
    () => async () => {
      if (!isTauriRuntime() || !trimmedRoot) return;
      try {
        const next = await invokeBackendCommand<CensorStatus>("censor_status", {
          root: trimmedRoot,
          projectId,
        });
        setStatus(next);
      } catch {
        // Status is advisory; a failure just hides the hints.
      }
    },
    [trimmedRoot, projectId],
  );

  useEffect(() => {
    void refreshStatus();
  }, [refreshStatus]);

  const runGuarded = async (work: () => Promise<void>) => {
    if (actionBusyRef.current) return;
    actionBusyRef.current = true;
    setActionBusy(true);
    try {
      await work();
    } finally {
      actionBusyRef.current = false;
      setActionBusy(false);
    }
  };

  const handleOpen = (finding: CensorFinding) =>
    void runGuarded(async () => {
      if (!trimmedRoot) return;
      try {
        await invokeBackendCommand<void>(
          "censor_open_in_editor",
          openInEditorArgs({ projectId, root: trimmedRoot, file: finding.file, editor }),
        );
      } catch (e) {
        setLoadError(
          e instanceof Error ? e.message : "Could not open the file in the editor.",
        );
      }
    });

  const handleDispose = (finding: CensorFinding, disposition: CensorDisposition) =>
    void runGuarded(async () => {
      if (!trimmedRoot) return;
      try {
        await invokeBackendCommand<void>(
          "censor_dispose_finding",
          disposeArgs({
            projectId,
            root: trimmedRoot,
            file: finding.file,
            id: finding.id,
            disposition,
          }),
        );
        // Optimistic local removal; the findings-updated event will reconcile.
        setFindings((current) => current.filter((f) => f.id !== finding.id));
      } catch (e) {
        setLoadError(
          e instanceof Error ? e.message : "Could not update the finding.",
        );
      }
    });

  const handleReviewNow = () =>
    void runGuarded(async () => {
      if (!trimmedRoot) return;
      try {
        await invokeBackendCommand<void>(
          "censor_review_now",
          reviewNowArgs({ projectId, root: trimmedRoot }),
        );
        await refreshStatus();
      } catch (e) {
        setLoadError(
          e instanceof Error ? e.message : "Could not start a Censor review.",
        );
      }
    });

  const handleRunFinalReview = () => {
    if (isBusy || !canLaunch) return;
    onLaunch(finalReviewLaunchInput(projectId));
  };

  // Trust gate (BLOCKER B): an UNTRUSTED project runs no linters/Gemma. The
  // "Trust & enable Censor" button opts it in, then re-reads status (now trusted →
  // the findings UI) and kicks an immediate sweep so the ledger populates without
  // waiting for the next file save. Reuses the same reentrancy guard.
  const handleTrust = () =>
    void runGuarded(async () => {
      if (!trimmedRoot) return;
      try {
        await invokeBackendCommand<void>(
          "set_censor_trusted",
          setCensorTrustedArgs({ projectId, trusted: true }),
        );
        await refreshStatus();
        // Best-effort first sweep; failure is surfaced but does not un-trust.
        await invokeBackendCommand<void>(
          "censor_review_now",
          reviewNowArgs({ projectId, root: trimmedRoot }),
        );
      } catch (e) {
        setLoadError(
          e instanceof Error ? e.message : "Could not enable Censor for this project.",
        );
      }
    });

  // Reverse of handleTrust: turn Censor back OFF for this project (BLOCKER B is per-project
  // and reversible). No review sweep — disabling stops the engine.
  const handleDisable = () =>
    void runGuarded(async () => {
      if (!trimmedRoot) return;
      try {
        await invokeBackendCommand<void>(
          "set_censor_trusted",
          setCensorTrustedArgs({ projectId, trusted: false }),
        );
        await refreshStatus();
      } catch (e) {
        setLoadError(
          e instanceof Error ? e.message : "Could not disable Censor for this project.",
        );
      }
    });

  return (
    <CensorPanelView
      findings={findings}
      status={status}
      loadError={loadError}
      hasRoot={Boolean(trimmedRoot)}
      actionBusy={actionBusy}
      launchBusy={isBusy}
      canLaunch={canLaunch}
      onOpen={handleOpen}
      onDispose={handleDispose}
      onReviewNow={handleReviewNow}
      onRunFinalReview={handleRunFinalReview}
      onTrust={handleTrust}
      onDisable={handleDisable}
    />
  );
}

export interface CensorPanelViewProps {
  findings: CensorFinding[];
  status: CensorStatus | null;
  loadError: string | null;
  hasRoot: boolean;
  actionBusy: boolean;
  launchBusy: boolean;
  canLaunch: boolean;
  onOpen: (finding: CensorFinding) => void;
  onDispose: (finding: CensorFinding, disposition: CensorDisposition) => void;
  onReviewNow: () => void;
  onRunFinalReview: () => void;
  /** "Trust & enable Censor" for an untrusted project (the trust gate). */
  onTrust: () => void;
  /** Turn Censor back OFF for this project (reversible). */
  onDisable: () => void;
}

/**
 * Pure, props-driven presentation of the Censor panel — no effects, no IO. Split
 * out so all visual states (empty / no-root / Gemma-offline / tool-absent / the
 * grouped findings list) are statically renderable in tests (this repo has no
 * jsdom). The stateful `CensorPanel` above owns the tracker + status reads and
 * feeds this view.
 */
export function CensorPanelView({
  findings,
  status,
  loadError,
  hasRoot,
  actionBusy,
  launchBusy,
  canLaunch,
  onOpen,
  onDispose,
  onReviewNow,
  onRunFinalReview,
  onTrust,
  onDisable,
}: CensorPanelViewProps) {
  const groups = groupFindingsByFile(findings);
  const gemmaNote = gemmaStatusNote(status?.gemmaStatus);
  const absentTools = (status?.tools ?? []).filter((t) => !t.available);
  const viewState = censorPanelViewState({ hasRoot, status });

  // ---- trust gate (BLOCKER B): untrusted projects run NO linters/Gemma ----
  if (viewState === "untrusted") {
    return (
      <div
        className="space-y-3"
        data-help-title="Censor is disabled for this project until you trust it."
        data-help-lines="Censor runs this repository's OWN linter/build configs (eslint plugins, cargo build scripts, semgrep rules), which can execute arbitrary code. Only enable it for repositories you trust.|Trusting is per-project and reversible from the project settings."
      >
        <div className="space-y-3 rounded-2xl border border-amber/25 bg-amber/8 p-5 text-[12px] text-cream-600">
          <div className="flex items-center gap-2 text-[13px] font-semibold text-amber-dark">
            <ShieldAlert className="h-4 w-4 shrink-0" />
            Censor is off for this project
          </div>
          <p className="leading-relaxed text-cream-500">
            Censor runs this project&apos;s own linter configs (eslint plugins,
            cargo build scripts, semgrep rules). Only enable for repos you trust.
          </p>
          <button
            type="button"
            onClick={onTrust}
            disabled={actionBusy}
            className="inline-flex items-center gap-1.5 rounded-2xl bg-teal px-4 py-2 text-[12px] font-semibold text-white transition-colors hover:bg-teal/90 disabled:cursor-not-allowed disabled:opacity-50"
          >
            <ShieldCheck className="h-3.5 w-3.5" />
            Trust &amp; enable Censor
          </button>
          {loadError && (
            <p className="rounded-lg bg-coral/8 px-3 py-2 text-[11px] text-coral-dark">
              {loadError}
            </p>
          )}
        </div>
      </div>
    );
  }

  return (
    <div
      className="space-y-3"
      data-help-title="Censor is a continuous, local-first per-file code review."
      data-help-lines="Deterministic linters run on every change; an optional on-device model (Gemma via Ollama) adds file-local smells. Nothing leaves your machine.|Open findings are pulled by the coder each step and by the verifier on a final review, via MCP.|Click a file:line to jump to the code; mark false positives to dispose them.|Run now triggers an immediate sweep; Run final review launches a verifier to adjudicate the residual ledger."
    >
      {/* ---- action strip ---- */}
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={onReviewNow}
          disabled={actionBusy || !hasRoot}
          className="inline-flex items-center gap-1.5 rounded-lg border border-cream-200 px-3 py-1.5 text-[12px] font-semibold text-cream-600 transition-colors hover:bg-cream-50 disabled:cursor-not-allowed disabled:opacity-50"
        >
          <RefreshCw className="h-3.5 w-3.5" />
          Review now
        </button>
        <button
          type="button"
          onClick={onRunFinalReview}
          disabled={launchBusy || !canLaunch || !hasRoot}
          className="inline-flex items-center gap-1.5 rounded-lg bg-terracotta px-3 py-1.5 text-[12px] font-semibold text-white transition-colors hover:bg-terracotta/90 disabled:cursor-not-allowed disabled:opacity-50"
        >
          <ShieldCheck className="h-3.5 w-3.5" />
          Run final review
        </button>
        <span className="ml-auto text-[11px] text-cream-400">
          {findings.length === 0
            ? "0 open findings"
            : `${findings.length} open finding${findings.length === 1 ? "" : "s"}`}
        </span>
        <button
          type="button"
          onClick={onDisable}
          disabled={actionBusy || !hasRoot}
          title="Turn Censor off for this project (reversible)"
          className="inline-flex items-center gap-1.5 rounded-lg border border-cream-200 px-3 py-1.5 text-[12px] font-semibold text-cream-500 transition-colors hover:bg-cream-50 disabled:cursor-not-allowed disabled:opacity-50"
        >
          <ShieldOff className="h-3.5 w-3.5" />
          Disable
        </button>
      </div>

      {/* ---- Gemma-offline banner ---- */}
      {gemmaNote && (
        <div className="flex items-center gap-2 rounded-xl border border-amber/20 bg-amber/8 px-3 py-2 text-[11px] text-amber-dark">
          <Play className="h-3.5 w-3.5 shrink-0" />
          {gemmaNote}
        </div>
      )}

      {/* ---- tool-absent hint (optional) ---- */}
      {absentTools.length > 0 && (
        <div className="space-y-1">
          <p className="text-[10px] text-cream-400">
            not installed — those Censor layers are skipped. Click a tool to copy its install command.
          </p>
          <div className="flex flex-wrap gap-1.5">
            {absentTools.map((t) => {
              const hint = installHintFor(t.name);
              return (
                <button
                  key={t.name}
                  type="button"
                  data-censor-missing-tool={t.name}
                  title={hint ? `Click to copy: ${hint}` : `${t.name} is not installed`}
                  onClick={() => {
                    if (hint) void navigator.clipboard?.writeText(hint);
                  }}
                  className="inline-flex items-center gap-1 rounded-md border border-coral/30 bg-coral/8 px-2 py-0.5 text-[10px] text-coral-dark hover:bg-coral/15"
                >
                  <XCircle className="h-3 w-3" />
                  {t.name}
                </button>
              );
            })}
          </div>
        </div>
      )}

      {/* ---- soft error ---- */}
      {loadError && (
        <p className="rounded-lg bg-coral/8 px-3 py-2 text-[11px] text-coral-dark">
          {loadError}
        </p>
      )}

      {/* ---- findings list grouped by file, or empty state ---- */}
      {!hasRoot ? (
        <div className="rounded-xl border border-dashed border-cream-200 bg-cream-50 p-6 text-center text-[12px] text-cream-400">
          This project has no working root configured, so Censor cannot review it.
        </div>
      ) : groups.length === 0 ? (
        <div className="rounded-xl border border-dashed border-cream-200 bg-cream-50 p-6 text-center text-[12px] text-cream-400">
          No open findings. Edit a file or run a review to populate the ledger.
        </div>
      ) : (
        <div className="space-y-4">
          {groups.map((group) => (
            <div key={group.file} className="space-y-2">
              <p className="text-[11px] font-semibold text-cream-500">
                {group.file}
                <span className="ml-2 font-normal text-cream-400">
                  {group.findings.length} finding
                  {group.findings.length === 1 ? "" : "s"}
                </span>
              </p>
              <div className="space-y-2">
                {group.findings.map((finding) => (
                  <CensorFindingRow
                    key={finding.id}
                    finding={finding}
                    onOpen={onOpen}
                    onDispose={onDispose}
                    busy={actionBusy}
                  />
                ))}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
