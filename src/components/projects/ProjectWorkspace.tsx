// Work-mode full-screen IDE shell (Phase D). Rendered full-bleed by ProjectsView
// when `workMode && currentProject`; the kanban / calendar chrome is skipped
// (the per-project task board + Notes now live HERE, via slots). Layout:
//   - Top bar: ← Board, project name, git status (from project.gitStatus) +
//     [Commit]/[Push] wired to the new backend commands.
//   - Left: the Living Plan navigator (work/LivingPlan) — agents inhabit the file
//     they edit; selecting a node drives the Focus stage. (The old rail + its spawn
//     launcher are replaced: launch is now a top-bar "+ Launch" → SpawnPanel.)
//   - Center: the FocusStage — Activity (structured) / Raw (the lazy AgentTerminalViewer,
//     KEYED by agentId) + a two-way composer + inline question card.
//   - ONE consolidated tab bar below the console: Tasks (default) / Censor / Git /
//     Changes / Plans / Notes / MCP / Project. The task board, notes, project detail,
//     censor strip + panel, git detail, changes, plans (history + approval), and MCP
//     servers all live in their respective tabs.
//
// CRITICAL: this component adds NO agent-state poller. It consumes the sessions /
// claims / events ProjectsView already polls (passed as props). Each focus pane mounts the
// SAME lazy AgentTerminalViewer the Agents room uses; in split view UP TO TWO are mounted at
// once — safe, since each is a self-contained TerminalSession on its own per-agent channel
// (agent-terminal://<agentId>), no shared module-level state.

import {
  ArrowLeft,
  Columns2,
  GitBranch,
  LifeBuoy,
  Minimize2,
  OctagonX,
  PanelRightOpen,
  Plus,
  Sparkles,
  Square,
  Terminal,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { invokeBackendCommand, isTauriRuntime } from "../../context/AppContext";
import { useNow } from "../../hooks/useNow";
import type {
  AgentClaim,
  AgentEvent,
  AgentRoleRule,
  AgentSession,
  CensorFinding,
  CensorStatus,
  ProjectDetail,
  SandboxMode,
} from "../../types/backend";
import type { CustomAgentClient } from "../../types/config";
import type { SpawnLaunchInput, SpawnSelection } from "../agents/agentRowModel";
import { AgentDetailDrawer } from "../agents/AgentDetailDrawer";
import { CensorPanel } from "./CensorPanel";
import { AgentControlsCard } from "./AgentControlsCard";
import { SandboxModeSelector } from "./SandboxModeSelector";
import { WorkingSetCard } from "./WorkingSetCard";
import { FocusStagePane } from "../work/FocusStagePane";
import {
  Panel,
  PanelGroup,
  PanelResizeHandle,
  type ImperativePanelGroupHandle,
} from "react-resizable-panels";
import { LivingPlan } from "../work/LivingPlan";
import { BUILTIN_CLIENTS, SpawnPanel } from "../agents/SpawnPanel";
import { SkillsToolsModal } from "../work/SkillsToolsModal";
import { CensorStrip } from "../work/CensorStrip";
import { buildCensorStrip } from "../work/censorStripModel";
import { useMiniStuckReports } from "../work/useMiniStuckReports";
import { MiniStuckBanner } from "../work/MiniStuckBanner";
import { buildWorkConsoleModel, type WorkNode } from "../work/workConsoleModel";
import { CensorFindingsTracker } from "./censorPanelModel";
import { AgentConsentModal } from "./AgentConsentModal";
import { FolderConsentModal } from "./FolderConsentModal";
import { NetConsentModal } from "./NetConsentModal";
import {
  enqueueConsent,
  grantFolderConsentArgs,
  grantNetConsentArgs,
  isConsentRequestForProject,
  respondCloudConsentArgs,
  sameConsentRequest,
  type ConsentKind,
  type ConsentRequest,
} from "./netConsentModel";
import { ConsentBridgePoller } from "./ConsentBridgePoller";
import { PlanApprovalCard } from "./PlanApprovalCard";
import { PlansDockTab } from "./PlansPanel";
import { PushApprovalCard } from "./PushApprovalCard";
import { ProjectMcpServersCard } from "./ProjectMcpServersCard";
import { ChangesDockTab } from "./ChangesDockTab";
import {
  DOCK_TABS,
  type DockTab,
  compactWriteCall,
  isMiniSession,
  miniKillCall,
  reconcileSelectedAgentId,
  shouldShowCompact,
  workspaceGitLine,
} from "./projectWorkspaceModel";
import { readActiveTabPref, writeActiveTabPref } from "./activeTabPref";
import {
  useWorkSelectionStore,
  taskIdForAgent,
} from "../../store/workSelectionStore";
import { TokenUsageBadge } from "../agents/TokenUsageBadge";
import { useAgentTokenUsage } from "../agents/useAgentTokenUsage";
import type { AgentTokenUsage } from "../../types/backend";

// Consent kinds the workspace can render (modal switch) AND route a decision for
// (handleConsentDecision). Any kind outside this set is dropped at the listener so it
// can never fall through to the NetConsentModal default and misroute to grant_net_consent.
const HANDLED_CONSENT_KINDS = new Set<ConsentKind>([
  "net",
  "folderWrite",
  "exec",
  "patch",
]);

// Stable empty set so the Living Plan's dirty highlight doesn't churn identity (and force
// a re-render) on every poll when the Censor is clean.
const EMPTY_AGENT_IDS: Set<string> = new Set<string>();

export interface ProjectWorkspaceProps {
  project: ProjectDetail;
  /** Sessions for THIS project (sessionsByProject) — consumed, not re-polled. */
  sessions: AgentSession[];
  /** Open claims for this project (from the same board state). */
  claims: AgentClaim[];
  /** Recent events for this project (from the same board state). */
  events: AgentEvent[];
  /** agent_ids with a live app-hosted PTY (from the board's agent_pty_list).
   *  Gates the center terminal: an agent without an app PTY (external/legacy) gets
   *  a tidy explanatory state instead of the viewer's error banner. */
  ptyAgents: Set<string>;
  isBusy: boolean;
  canLaunch: boolean;
  /** READ-ONLY mode: the project is archived. The page stays fully VIEWABLE
   *  (tasks, notes, console, git diff, terminal) but EVERY mutating control is
   *  disabled + guarded. A prominent banner with an [Unarchive] action is shown
   *  at the top. Defaults to false so a non-archived project behaves identically
   *  to today (byte-identical render). */
  readOnly?: boolean;
  /** Restore an archived project to "active" (only meaningful when readOnly).
   *  Wired by ProjectsView to the same updateProjectStatus("active") path the
   *  Resume button uses. Optional so existing call sites without it still type. */
  onUnarchive?: () => void;
  launchMessage: string | null;
  rules?: AgentRoleRule[];
  customClients?: CustomAgentClient[];
  /** The configured local Devboule main-coder model (config.localCoderBackend.model),
   *  threaded to SpawnPanel via the agent rail. Optional. */
  localCoderModel?: string | null;
  onBack: () => void;
  /** Recall the orchestrator to revise the current plan (reuses the main-page planner
   *  console 1:1). Optional: omitted in contexts without an orchestrator. */
  onRecallOrchestrator?: () => void;
  onLaunch: (input: SpawnLaunchInput) => void;
  onCopyPrompt: (selection: SpawnSelection) => void;
  onCommit: (message: string) => void;
  onPush: () => void;
  onPull: () => void;
  /** Stop a NORMAL (non-mini) agent: kills its process tree via the backend's
   *  `stop_agent` flow (the same one the removed board-mode panel used). The mini
   *  safety brake (`mini_coder_kill`) is a SEPARATE, in-component path — the two
   *  never apply to the same selected agent. */
  onStopAgent: (agentId: string) => void;
  /** Focus the dedicated external-console window of an agent that runs OUTSIDE the
   *  app (no in-app PTY). Backend `focus_agent_terminal`. */
  onFocusCli: (agentId: string) => void;
  /** Copy a client-side recovery prompt for the selected agent (no backend call,
   *  no secret). The parent resolves the live session from this agent id. */
  onCopyRecovery: (agentId: string) => void;
  /** Inline status message for the last commit/push/pull (success or git stderr). */
  gitActionMessage: string | null;
  gitActionError: boolean;
  gitActionBusy: boolean;
  /** The per-project task board, built by ProjectsView (reusing its handlers) and
   *  rendered here ABOVE the dock tabs. Optional → absent renders nothing. */
  taskBoardSlot?: ReactNode;
  /** The per-project Notes section, built by ProjectsView, rendered BELOW the dock. */
  notesSlot?: ReactNode;
  /** B8: the per-project detail (status header + lifecycle actions + agent-root
   *  editor + saved workflows), built by ProjectsView and relocated here from the
   *  create landing — a project's detail belongs on its OWN page, not the create
   *  page (the landing = create + Kanban-as-history). Optional → absent renders nothing. */
  detailSlot?: ReactNode;
  /** Called after a successful sandbox-mode write so the parent can patch the
   *  in-memory project metadata immediately (avoids a ~10s wait for the poll). */
  onSandboxModeChange?: (mode: SandboxMode) => void;
  /** Called after a successful working-set add or remove so the parent can patch
   *  the in-memory project metadata immediately (avoids a ~10s wait for the poll). */
  onWorkingSetChange?: (next: string[]) => void;
  /**
   * Called after a successful `grant_folder_consent` with decision=allowRemember so
   * the parent can reload the project detail and surface the backend-canonicalized
   * folder in WorkingSetCard without waiting for the 10s poll.
   * Optional: if absent the card just waits for the next poll.
   */
  onReloadProject?: () => void;
}

export function ProjectWorkspace({
  project,
  sessions,
  claims,
  events,
  ptyAgents,
  isBusy,
  canLaunch,
  readOnly = false,
  onUnarchive,
  launchMessage,
  rules = [],
  customClients = [],
  localCoderModel = null,
  onBack,
  onRecallOrchestrator,
  onLaunch,
  onCopyPrompt,
  onCommit,
  onPush,
  onPull,
  onStopAgent,
  onFocusCli,
  onCopyRecovery,
  gitActionMessage,
  gitActionError,
  gitActionBusy,
  taskBoardSlot,
  notesSlot,
  detailSlot,
  onSandboxModeChange,
  onWorkingSetChange,
  onReloadProject,
}: ProjectWorkspaceProps) {
  const now = useNow();
  // Phase B twinning: the selection lives in the shared store so the bottom DAG board
  // (TaskCard) and the Work Console drive ONE selection. All the reads below are unchanged;
  // writes go through selectAgentWithTask, which sets the agent AND its task in tandem so the
  // two surfaces never half-desync. The effective per-render selection is reconciled
  // synchronously just below (so the default is right on the first render, effects-free).
  const storeSelectedAgentId = useWorkSelectionStore((s) => s.selectedAgentId);
  const selectBoth = useWorkSelectionStore((s) => s.selectBoth);
  // Select an agent AND resolve its task together (agent -> task direction), in ONE atomic
  // store write so the board twin never sees a half-updated snapshot.
  const selectAgentWithTask = useCallback(
    (agentId: string | null) =>
      selectBoth(agentId, taskIdForAgent(agentId, sessions, claims)),
    [selectBoth, sessions, claims],
  );
  const [drawerOpen, setDrawerOpen] = useState(false);
  // Split view: when set, pins a SECOND agent's focus pane beside the primary selection.
  // Local to the console (not shared with the board) — split is a pure view concern.
  const [splitAgentId, setSplitAgentId] = useState<string | null>(null);
  // Work Console "Skills & Tools" modal (per-role skills/tools for this project).
  const [skillsOpen, setSkillsOpen] = useState(false);
  // Stable so the modal's Escape-key effect isn't re-registered on every parent render.
  const closeSkills = useCallback(() => setSkillsOpen(false), []);
  // defaultSize is initial-mount-only in react-resizable-panels, so drive the proportions
  // imperatively: toggling split rebalances to 50/50, unsplit restores the primary to 100%.
  const panelGroupRef = useRef<ImperativePanelGroupHandle>(null);
  useEffect(() => {
    panelGroupRef.current?.setLayout(splitAgentId ? [50, 50] : [100]);
  }, [splitAgentId]);
  const [dockTab, setDockTabState] = useState<DockTab>(() =>
    readActiveTabPref(project.metadata.id),
  );
  const setDockTab = useCallback(
    (next: DockTab) => {
      setDockTabState(next);
      writeActiveTabPref(project.metadata.id, next);
    },
    [project.metadata.id],
  );
  const [launcherOpen, setLauncherOpen] = useState(false);
  // Plan pending count: fed by PlanApprovalCard's onPendingCountChange callback.
  const [planPendingCount, setPlanPendingCount] = useState(0);

  // Client labels for the Launch button tooltip.
  const clientLabels = useMemo(() => {
    const builtins = BUILTIN_CLIENTS.map((c) => c.label);
    const customs = customClients.map((c) => c.label);
    return [...builtins, ...customs];
  }, [customClients]);
  const clientLabelStr = useMemo(
    () => clientLabels.join(" \u{00b7} "),
    [clientLabels],
  );

  // Tasks badge: count of wip + review tasks.
  const tasksBadgeCount = useMemo(
    () =>
      project.state.tasks.filter(
        (t) => t.status === "wip" || t.status === "review",
      ).length,
    [project.state.tasks],
  );
  const [commitOpen, setCommitOpen] = useState(false);
  const [commitMessage, setCommitMessage] = useState("");

  // The previous sessions snapshot, so reconcile can tell whether a now-gone
  // selection WAS a mini (and thus fall back to its parent, not the freshest).
  const prevSessionsRef = useRef<AgentSession[]>(sessions);

  // Effective selection for THIS render: reconcile the STORED selection against the live
  // sessions synchronously, so the default is correct on the very first render (SSR /
  // renderToStaticMarkup run no effects, so a store seeded only by an effect would render
  // null). The effect below persists the reconciled value back to the store so the board
  // twin + a reaped-mini→parent fallback stay in sync across the live (DOM) poll.
  const selectedAgentId = useMemo(
    () =>
      reconcileSelectedAgentId(
        storeSelectedAgentId,
        sessions,
        prevSessionsRef.current,
      ),
    [storeSelectedAgentId, sessions],
  );

  // Reconcile the selection against the live session list every render-relevant
  // change: keep a still-live selection, otherwise fall back — to the gone mini's
  // PARENT when it was a mini and the parent survives, else the freshest survivor
  // (or null). This prunes a dangling selection when the selected agent exits — no
  // second poller, just a derive off the props ProjectsView feeds.
  useEffect(() => {
    // BLOCKER 2: capture the genuinely-prior snapshot, then update the ref to the new
    // sessions BEFORE calling setSelectedAgentId. The setState updater runs LATER
    // (async), so if we updated the ref after it, the updater would read the
    // already-overwritten ref (prev === current) and a reaped mini would never fall
    // back to its parent. Closing over the captured `prev` makes reconcile see the
    // real prior list.
    const prev = prevSessionsRef.current;
    prevSessionsRef.current = sessions;
    // Read the live store value (not a stale closure) so reconcile sees the real current
    // selection — which may have just been set by the board card the user clicked to enter
    // Work mode. Only write when it actually changes; reconcile is idempotent for a still-
    // live selection. When the agent changes (e.g. a reaped mini falls back to its parent),
    // the task follows the agent so the board highlight stays twinned.
    const current = useWorkSelectionStore.getState().selectedAgentId;
    const next = reconcileSelectedAgentId(current, sessions, prev);
    if (next !== current) {
      selectBoth(next, taskIdForAgent(next, sessions, claims));
    }
  }, [sessions, claims, selectBoth]);

  // Stuck-report filter: match against BOTH current and previous sessions so a
  // parent-gone stuck report (emitted after the parent left `sessions`) is still
  // visible for one render cycle — prevSessionsRef holds the prior snapshot until
  // the next effect run advances it.
  const { reports: stuckReports, dismiss: dismissStuck } = useMiniStuckReports();
  const stuckAgentIds = useMemo(() => {
    const ids = new Set<string>();
    for (const s of sessions) ids.add(s.agentId);
    for (const s of prevSessionsRef.current) ids.add(s.agentId);
    return ids;
  }, [sessions]);
  const filteredStuckReports = useMemo(
    () => stuckReports.filter((r) => stuckAgentIds.has(r.agentId)),
    [stuckReports, stuckAgentIds],
  );

  // Auto-unpin the split's second pane when its agent's session disappears (reaped/exited),
  // so a dead agent can't leave a stale "unsplit" state or a wasted activity subscription.
  useEffect(() => {
    if (splitAgentId && !sessions.some((s) => s.agentId === splitAgentId)) {
      setSplitAgentId(null);
    }
  }, [splitAgentId, sessions]);

  const gitLine = useMemo(
    () => workspaceGitLine(project.gitStatus),
    [project.gitStatus],
  );

  const selectedSession = useMemo<AgentSession | null>(
    () => sessions.find((s) => s.agentId === selectedAgentId) ?? null,
    [sessions, selectedAgentId],
  );

  // Clear a stale Stop/Compact failure when the selected agent changes, so the
  // error never lingers attached to a different agent.
  useEffect(() => {
    setMiniActionError(null);
  }, [selectedSession]);
  // MC-P5: the Stop (kill) safety brake is gated to a SELECTED mini session only —
  // a mini is a session with a parentAgentId. A normal agent's stop is the
  // separate `stop_agent` flow wired through onStopAgent below, NOT this 1-click
  // brake.
  const selectedIsMini = useMemo(
    () => (selectedSession ? isMiniSession(selectedSession) : false),
    [selectedSession],
  );

  // The Stop control for a NORMAL agent (the restored board-mode `stop_agent`
  // capability): shown ONLY for a selected NON-mini agent with a live session.
  // Mutually exclusive with the mini brake above — a mini shows the mini brake,
  // a normal agent shows this. Either, but never both, on the same selection.
  const selectedShowsNormalStop = Boolean(selectedSession) && !selectedIsMini;

  // Open-CLI is meaningful ONLY for an agent that runs in an EXTERNAL console
  // (no in-app PTY). This mirrors the same condition that renders the
  // "runs in an external console" placeholder below: a non-app host with no live
  // in-app PTY. An app-hosted agent whose PTY exited has no external window to
  // focus, so it stays hidden (matching agentRowModel.rowActions.showOpenCli).
  const selectedShowsOpenCli =
    selectedSession != null &&
    selectedSession.host !== "app" &&
    !ptyAgents.has(selectedSession.agentId);

  // MC-P7: the Compact action is gated to a SELECTED session whose RESOLVED
  // built-in client is exactly "claude" — `/compact` is a Claude Code slash
  // command, meaningless to codex/powershell/ollama-mini/custom CLIs. It is an
  // independent control from the mini Stop brake (Stop = mini-only safety kill;
  // Compact = claude-only context hygiene); either, both, or neither may show
  // depending on the selected session.
  const selectedCanCompact = useMemo(
    () => (selectedSession ? shouldShowCompact(selectedSession) : false),
    [selectedSession],
  );

  // MC-P6: token/cost window for the SELECTED agent ONLY, on a slow lazy cadence
  // (transcript reads are expensive — never per rail row, never on the 5s
  // live-state tick). Degrades silently to a hidden badge on unavailable.
  const tokenUsage = useAgentTokenUsage(selectedAgentId, {
    fetchUsage: (agentId) =>
      invokeBackendCommand<AgentTokenUsage>("get_agent_token_usage", {
        agentId,
      }),
  });

  // Archiving (readOnly) closes the launcher so a stale-open SpawnPanel can't re-mount on unarchive.
  useEffect(() => {
    if (readOnly) {
      setLauncherOpen(false);
      setSkillsOpen(false);
    }
  }, [readOnly]);
  // Split: the model rebuilds only when sessions/tasks change (the 5s poll), and the node
  // lookup re-runs when the SELECTION changes — so switching agents doesn't rebuild the model.
  const workConsoleModel = useMemo(
    () =>
      buildWorkConsoleModel({
        sessions,
        tasks: project.state.tasks,
        projectId: project.metadata.id,
      }),
    [sessions, project.state.tasks, project.metadata.id],
  );
  // Censor strip: the project-wide inspection summary (clean/dirty per file). Reuses the
  // SAME event-driven findings feed as the Censor dock tab — NO new poller.
  const [censorFindings, setCensorFindings] = useState<CensorFinding[]>([]);
  const [censorScanning, setCensorScanning] = useState(false);
  const [censorScannedFiles, setCensorScannedFiles] = useState(0);
  const [censorMissingTools, setCensorMissingTools] = useState<string[]>([]);
  const censorRoot = (project.metadata.rootPath ?? "").trim();
  useEffect(() => {
    if (!isTauriRuntime() || !censorRoot) {
      setCensorFindings([]);
      return;
    }
    const tracker = new CensorFindingsTracker({
      projectId: project.metadata.id,
      root: censorRoot,
      invoke: invokeBackendCommand,
      listen: async (channel, handler) => {
        const { listen } = await import("@tauri-apps/api/event");
        return listen(channel, (event) => handler({ payload: event.payload }));
      },
      onChange: (next) => setCensorFindings(next),
      onError: () => {},
    });
    void tracker.start();
    return () => tracker.stop();
  }, [project.metadata.id, censorRoot]);
  // Live scan state (the "linters running…" indicator) + tool-health for the strip. Listens to
  // the same Censor events as the tracker (no new poller); censor_status feeds missing tools.
  useEffect(() => {
    const pid = project.metadata.id;
    if (!isTauriRuntime() || !censorRoot) {
      setCensorScanning(false);
      setCensorMissingTools([]);
      return;
    }
    let cancelled = false;
    const unlistens: Array<() => void> = [];
    // Safety timer: scan-started fires BEFORE the slow runner work; if the project is stopped
    // mid-pass the matching findings-updated never arrives (emit_if_running skips it), which
    // would otherwise leave the "running…" indicator stuck on. Auto-clear it after a grace
    // window longer than the slowest linter.
    let scanTimer: ReturnType<typeof setTimeout> | null = null;
    const clearScanTimer = () => {
      if (scanTimer !== null) {
        clearTimeout(scanTimer);
        scanTimer = null;
      }
    };

    const refreshMissingTools = async () => {
      try {
        const status = await invokeBackendCommand<CensorStatus>(
          "censor_status",
          {
            root: censorRoot,
            projectId: pid,
          },
        );
        if (!cancelled) {
          setCensorMissingTools(
            (status.tools ?? []).filter((t) => !t.available).map((t) => t.name),
          );
        }
      } catch {
        // Status is advisory; a failure just hides the missing-tool hints.
      }
    };

    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      if (cancelled) return; // torn down before the dynamic import resolved — register nothing.
      const un1 = await listen("censor://scan-started", (event) => {
        if (cancelled) return;
        const payload = event.payload as {
          projectId: string;
          fileCount?: number;
        };
        if (payload.projectId !== pid) return;
        setCensorScanning(true);
        setCensorScannedFiles(payload.fileCount ?? 0);
        clearScanTimer();
        scanTimer = setTimeout(() => {
          if (!cancelled) setCensorScanning(false);
        }, 45000);
      });
      if (cancelled) {
        un1();
        return;
      }
      unlistens.push(un1);
      const un2 = await listen("censor://findings-updated", (event) => {
        if (cancelled) return;
        const payload = event.payload as { projectId: string };
        if (payload.projectId !== pid) return;
        clearScanTimer();
        setCensorScanning(false);
        void refreshMissingTools();
      });
      if (cancelled) {
        un2();
        return;
      }
      unlistens.push(un2);
      await refreshMissingTools();
    })();

    return () => {
      cancelled = true;
      clearScanTimer();
      setCensorScanning(false);
      unlistens.forEach((u) => u());
    };
  }, [project.metadata.id, censorRoot]);
  const censorStrip = useMemo(
    () =>
      buildCensorStrip(censorFindings, {
        scanning: censorScanning,
        scannedFiles: censorScannedFiles,
        missingTools: censorMissingTools,
      }),
    [censorFindings, censorScanning, censorScannedFiles, censorMissingTools],
  );

  // Map the Censor's DIRTY files onto the agents inhabiting them, so the Living Plan can
  // highlight a node coral when the file it edits has open findings.
  const dirtyAgentIds = useMemo(() => {
    const dirtyFiles = new Set(
      censorStrip.items.filter((i) => i.status === "dirty").map((i) => i.file),
    );
    if (dirtyFiles.size === 0) return EMPTY_AGENT_IDS;
    const ids = new Set<string>();
    const walk = (n: WorkNode) => {
      if (n.file && dirtyFiles.has(n.file)) ids.add(n.agentId);
      n.children.forEach(walk);
    };
    if (workConsoleModel.orchestrator) walk(workConsoleModel.orchestrator);
    workConsoleModel.districts.forEach((d) => d.nodes.forEach(walk));
    workConsoleModel.unplaced.forEach(walk);
    return ids;
  }, [censorStrip, workConsoleModel]);

  // MC-P5: 1-click kill of the selected mini-coder. It is a TRUE safety brake (no
  // two-step confirm): `mini_coder_kill` records killRequested THEN kills the PTY so
  // the executor finalizes the mini as aborted_by_human and the parent coder is told
  // to escalate. Best-effort; a failed invoke is swallowed (the executor's own
  // timeout/parent-gone backstops still reap a runaway mini).
  const stopMini = useCallback(() => {
    if (!selectedSession) return;
    const call = miniKillCall(selectedSession);
    if (!call) return; // not a mini — no 1-click kill for a normal agent.
    setMiniActionError(null);
    // Surface a failed kill instead of swallowing it — a silently-dropped Stop
    // leaves the user thinking the brake did nothing while the mini keeps running.
    void invokeBackendCommand(call.command, call.args).catch((e) =>
      setMiniActionError(
        e instanceof Error ? e.message : "Mini-coder could not be stopped.",
      ),
    );
  }, [selectedSession]);

  // MC-P7: run `/compact` in the selected Claude agent's terminal. Reuses the
  // EXISTING `agent_pty_write` path (the same the reply bar uses) to write the
  // fixed `/compact\n` literal — no new write path, no secret. Gated by
  // `compactWriteCall` returning null for any non-claude session, so a stray call
  // on a wrong client can never fire. Best-effort; a failed invoke is swallowed.
  const compactSelected = useCallback(() => {
    if (readOnly) return; // archived project: no mutations.
    if (!selectedSession) return;
    const call = compactWriteCall(selectedSession);
    if (!call) return; // not a claude client — no Compact for it.
    setMiniActionError(null);
    // Surface a failed compact instead of swallowing it.
    void invokeBackendCommand(call.command, call.args).catch((e) =>
      setMiniActionError(
        e instanceof Error ? e.message : "Compact could not be run.",
      ),
    );
  }, [readOnly, selectedSession]);

  // Stop a NORMAL (non-mini) selected agent via the parent's restored
  // `stop_agent` flow. Gated to a non-mini selection (the mini brake owns minis),
  // so a mistaken call on a mini can never fire.
  const stopSelected = useCallback(() => {
    if (!selectedSession || isMiniSession(selectedSession)) return;
    onStopAgent(selectedSession.agentId);
  }, [selectedSession, onStopAgent]);

  // Focus the selected agent's external console window (parent's
  // `focus_agent_terminal`). The button is only shown for an external-console
  // agent, but re-derive the condition here so it can never fire otherwise.
  const focusCliSelected = useCallback(() => {
    if (
      !selectedSession ||
      selectedSession.host === "app" ||
      ptyAgents.has(selectedSession.agentId)
    ) {
      return;
    }
    onFocusCli(selectedSession.agentId);
  }, [selectedSession, ptyAgents, onFocusCli]);

  // Copy a recovery prompt for the selected agent (parent builds the text from
  // the live session — pure, no backend call, no secret).
  const copyRecoverySelected = useCallback(() => {
    if (!selectedSession) return;
    onCopyRecovery(selectedSession.agentId);
  }, [selectedSession, onCopyRecovery]);

  const submitCommit = () => {
    if (readOnly) return; // archived project: no mutations.
    const trimmed = commitMessage.trim();
    if (!trimmed || gitActionBusy) return;
    onCommit(trimmed);
    setCommitMessage("");
    setCommitOpen(false);
  };

  // ── Permission broker — net-consent listener ──────────────────────────────
  // Subscribes to `sandbox://consent-request` on mount (subscribe-before-invoke
  // discipline: the listener is registered before any invoke that could trigger
  // a backend emit). Filters to kind=Net and the current projectId so requests
  // from other projects are silently ignored. Cleared after the user acts.
  //
  // Seatbelt constraint: the grant activates at the NEXT spawn; the copy inside
  // NetConsentModal tells the user to re-launch.
  // FIX 1: FIFO queue — concurrent requests are appended, not overwritten.
  const [pendingConsents, setPendingConsents] = useState<ConsentRequest[]>([]);
  const [consentBusy, setConsentBusy] = useState(false);
  const [consentError, setConsentError] = useState<string | null>(null);
  // Inline error for the fire-and-forget mini/compact actions — they used to
  // swallow backend rejections silently, so a failed Stop/Compact looked dead.
  const [miniActionError, setMiniActionError] = useState<string | null>(null);
  const consentMountedRef = useRef(true);
  // max-recall: a SYNCHRONOUS busy guard. `consentBusy` is React state and commits a tick
  // late, so a rapid double-tap can fire two decisions before it flips — delivering the same
  // decision twice to respond_cloud_consent. The ref flips synchronously, closing that race.
  const consentBusyRef = useRef(false);
  // 5b reviewer F3: ids of cloud consent requests we have already answered. The 4s
  // file-bridge poll can carry a STALE `pending_approval` snapshot (captured before the
  // backend stamped the verdict) and would otherwise RE-ENQUEUE an already-decided request,
  // flashing a dead modal. We skip any id recorded here. Reset on project change.
  const decidedConsentIdsRef = useRef<Set<string>>(new Set());
  // Mark unmounted so the handleConsentDecision async callback doesn't write state
  // after the component has been torn down. Runs only once on component unmount.
  // FIX 4: The current safety depends on the parent's key={projectId} triggering
  // a full remount on project switch; self-heal by resetting to true on each mount.
  useEffect(() => {
    consentMountedRef.current = true;
    // FIX 4: reset consentBusy on (re)mount so a re-used instance (one whose project
    // prop changed without a full remount) can never start with all consent buttons
    // permanently disabled. The parent uses key={projectId} to force full remounts on
    // project switch, but the self-heal here is a defensive belt-and-suspenders guard.
    setConsentBusy(false);
    return () => {
      consentMountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    // Use a local cancelled flag per effect run so re-mounting (project id change)
    // doesn't poison the shared ref: the async unlisten registration must be
    // guarded per-invocation, not per-component-lifetime.
    let cancelled = false;
    let unlisten: (() => void) | null = null;

    if (!isTauriRuntime()) return;

    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      if (cancelled) return; // effect tore down before the dynamic import resolved
      unlisten = await listen<ConsentRequest>(
        "sandbox://consent-request",
        (event) => {
          if (cancelled) return;
          const req = event.payload;
          // Accept the kinds the UI can actually render+route for the current project:
          // Net/FolderWrite (local seatbelt, Slices 0-2) and Exec/Patch (live cloud agents,
          // Slice 5). Requests for other projects — or any FUTURE/unknown kind the modal
          // switch and handleConsentDecision don't handle — are dropped, so an unhandled
          // kind can never fall through and misroute to grant_net_consent.
          if (!isConsentRequestForProject(req, project.metadata.id)) return;
          if (!HANDLED_CONSENT_KINDS.has(req.kind)) return;
          // FIX 1: append to FIFO queue, deduping by identity (see sameConsentRequest;
          // cloud requests dedupe on approvalId) so a duplicate event doesn't double-enqueue.
          setPendingConsents((prev) => enqueueConsent(prev, req));
          setConsentError(null);
        },
      );
      if (cancelled) {
        unlisten?.();
        unlisten = null;
      }
    })();

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [project.metadata.id]);

  // Slice 5b: the Claude consent file-bridge has no push event — the `claude_consent_hook`
  // process writes a `pending_approval` entry into `.aspis-agents.json` and bounded-polls it.
  // Poll `consent_requests_list` and enqueue any pending entry into the SAME FIFO the event
  // listener feeds (dedupe by approvalId is handled in enqueueConsent), so Claude's Exec/Patch
  // approvals render in the identical AgentConsentModal and answer via respond_cloud_consent.
  // New project context → forget the previously-decided ids (the queue is per-project).
  useEffect(() => {
    decidedConsentIdsRef.current.clear();
  }, [project.metadata.id]);

  // Receives the pending Claude file-bridge requests from <ConsentBridgePoller> and enqueues
  // them into the SAME FIFO the event listener feeds. F3: drop anything we already answered —
  // a stale in-flight poll snapshot must not resurrect a decided request after the pop.
  const handleBridgePending = useCallback((pending: ConsentRequest[]) => {
    const decided = decidedConsentIdsRef.current;
    const fresh = pending.filter(
      (req) => !(req.approvalId && decided.has(req.approvalId)),
    );
    if (fresh.length > 0) {
      setPendingConsents((prev) =>
        fresh.reduce((acc, req) => enqueueConsent(acc, req), prev),
      );
    }
  }, []);

  const handleConsentDecision = useCallback(
    async (decision: "allowRemember" | "allowOnce" | "deny") => {
      // FIX 1: operate on the HEAD of the FIFO queue
      const head = pendingConsents[0];
      if (!head || consentBusyRef.current) return;
      consentBusyRef.current = true;
      setConsentBusy(true);
      setConsentError(null);
      try {
        // Slice 5: LIVE cloud-agent requests carry an approvalId — the decision must
        // round-trip back to the blocked Claude/Codex agent via respond_cloud_consent,
        // NOT the fire-and-forget grant_* commands (which only persist for the next spawn).
        // Truthy check (not `!== undefined`) so a malformed empty-string id doesn't route.
        if (head.approvalId) {
          // F3: record the id as decided BEFORE the await so a poll that fires mid-flight
          // (and reads a stale pending snapshot) cannot re-enqueue this request afterwards.
          decidedConsentIdsRef.current.add(head.approvalId);
          await invokeBackendCommand<void>(
            "respond_cloud_consent",
            respondCloudConsentArgs({ approvalId: head.approvalId, decision }),
          );
        } else if (head.kind === "folderWrite") {
          // Branch by kind: Net → grant_net_consent, FolderWrite → grant_folder_consent.
          // Both share the same ConsentDecision enum and the same mounted-ref / FIFO logic.
          // BLOCKER 1 fix: use head.path (the machine-readable canonical folder), NOT
          // head.detail (human-readable prose). head.detail is rejected by the backend's
          // normalize_working_set_folder (!is_absolute) → AllowOnce/AllowRemember fail.
          // Surface an error if path is somehow missing rather than silently sending prose.
          const folder = head.path;
          if (!folder) {
            throw new Error(
              "FolderWrite consent request is missing the machine-readable path field. " +
                "This is a backend bug — please report it.",
            );
          }
          await invokeBackendCommand<void>(
            "grant_folder_consent",
            grantFolderConsentArgs({
              projectId: head.projectId,
              folder,
              decision,
            }),
          );
        } else if (head.kind === "net") {
          await invokeBackendCommand<void>(
            "grant_net_consent",
            grantNetConsentArgs({ projectId: head.projectId, decision }),
          );
        } else {
          // exec/patch without an approvalId, or any other kind, is malformed: it must
          // NOT silently fall through to grant_net_consent (which would grant network for
          // an unrelated request). Surface it instead of misrouting.
          throw new Error(
            `Cannot route a "${head.kind}" consent request without an approvalId. ` +
              "This is a backend bug — please report it.",
          );
        }
        if (consentMountedRef.current) {
          // FIX 1: remove head by identity so a concurrently enqueued request survives
          setPendingConsents((prev) =>
            prev.filter((r) => !sameConsentRequest(r, head)),
          );
          // FIX 3b: after an allowRemember folder grant the backend canonicalizes the
          // path and persists it to working_set. Trigger an immediate reload so
          // WorkingSetCard reflects the canonical folder without waiting for the 10s
          // poll. Net grants do not change working_set, so we only reload for folderWrite.
          if (head.kind === "folderWrite" && decision === "allowRemember") {
            onReloadProject?.();
          }
        }
      } catch (e) {
        if (consentMountedRef.current) {
          // FIX 2: Tauri rejects with a string, not an Error — use String(e)
          setConsentError(e instanceof Error ? e.message : String(e));
          // Slice 5: a cloud request that errors means the live waiter is GONE (the agent
          // timed out / the session ended) — retrying always fails with the same error and
          // there is no on-disk grant to fall back on. Pop the head so the modal can't get
          // permanently stuck. Local net/folder errors are left queued (retry is valid there).
          if (head.approvalId) {
            setPendingConsents((prev) =>
              prev.filter((r) => !sameConsentRequest(r, head)),
            );
          }
        }
      } finally {
        // Always clear the synchronous guard (even when unmounted) so a remount isn't wedged.
        consentBusyRef.current = false;
        // FIX 3: reset busy in finally so it clears even when unmounted mid-invoke
        if (consentMountedRef.current) {
          setConsentBusy(false);
        }
      }
    },
    // CHEAP FIX B: onReloadProject is used inside the callback (called after
    // allowRemember folder grant) but was missing from the deps array — stale
    // closure if the parent re-renders with a new onReloadProject reference.
    [pendingConsents, consentBusy, onReloadProject],
  );

  return (
    <div className="flex w-full flex-col gap-4">
      {/* ---- Read-only (archived) banner ---- */}
      {readOnly && (
        <div className="flex flex-col gap-2 rounded-2xl border border-amber/30 bg-amber/[0.06] p-3 sm:flex-row sm:items-center sm:justify-between">
          <p className="text-[12px] font-semibold text-amber-dark">
            📦 Project archived — read only
          </p>
          <button
            type="button"
            onClick={() => onUnarchive?.()}
            disabled={isBusy || onUnarchive === undefined}
            data-help-title="This restores the project to active."
            data-help-lines="Unarchiving sets the project back to active and editable.|It returns to the stage board and the calendar.|Agents can be launched again once it is active.|Archiving is fully reversible."
            className="shrink-0 self-start rounded-lg bg-terracotta px-3 py-1.5 text-[12px] font-semibold text-white hover:bg-terracotta/90 disabled:opacity-60 sm:self-auto"
          >
            Unarchive
          </button>
        </div>
      )}

      {/* ---- Mini-stuck banner ---- */}
      <MiniStuckBanner reports={filteredStuckReports} onDismiss={dismissStuck} />

      {/* ---- Top bar ---- */}
      <div className="flex flex-col gap-2 rounded-2xl border border-cream-200 bg-white p-3 lg:flex-row lg:items-center lg:justify-between">
        <div className="flex min-w-0 items-center gap-3">
          <button
            type="button"
            onClick={onBack}
            data-help-title="This returns to the project board."
            data-help-lines="Work mode is a full-screen view of one project.|Going back keeps this project selected on the board.|The agents keep running; you are only changing the view.|Use the board to switch between projects."
            className="inline-flex shrink-0 items-center gap-1.5 rounded-lg border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-600 hover:text-terracotta"
          >
            <ArrowLeft className="h-3.5 w-3.5" aria-hidden />
            Board
          </button>
          <h2 className="truncate text-sm font-semibold text-cream-800">
            {project.metadata.title}
          </h2>
          {onRecallOrchestrator && !readOnly && (
            <button
              type="button"
              onClick={onRecallOrchestrator}
              data-help-title="This recalls the Orchestrator to revise the current plan."
              data-help-lines="Brings the Orchestrator back to change the plan for this project.|It is the SAME planner console you used to create the project, seeded with the current work.|Once you re-approve the plan, the Orchestrator sleeps and you return to the Work Console.|The running coders keep their work; only the plan is revised."
              className="inline-flex shrink-0 items-center gap-1.5 rounded-lg border border-teal/30 bg-teal/[0.06] px-3 py-1.5 text-[12px] font-semibold text-teal-dark hover:bg-teal/[0.12]"
            >
              <Sparkles className="h-3.5 w-3.5" aria-hidden />
              Change plan
            </button>
          )}
          {!readOnly && (
            <button
              type="button"
              onClick={() => setLauncherOpen((open) => !open)}
              disabled={!canLaunch}
              title={clientLabelStr}
              data-help-title="Launch a coder or verifier for this project."
              data-help-lines="Opens the spawn panel to start a new agent on this project.|Pick a coder (writes code) or a verifier (reviews) and a task.|Only active projects can launch agents.|The new agent appears in the Living Plan on the file it claims."
              className={`inline-flex shrink-0 items-center gap-1.5 rounded-lg border px-3 py-1.5 text-[12px] font-semibold disabled:opacity-60 ${
                launcherOpen
                  ? "border-terracotta bg-terracotta/10 text-terracotta"
                  : "border-cream-200 bg-white text-cream-600 hover:text-terracotta"
              }`}
            >
              <Plus className="h-3.5 w-3.5" aria-hidden />
              Launch
            </button>
          )}
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {gitLine.isGitRepo && (
            <span
              className="inline-flex items-center gap-1.5 rounded-lg bg-cream-50 px-2.5 py-1 text-[11px] font-semibold text-cream-600"
              title={gitLine.segments.join(" · ")}
            >
              <GitBranch className="h-3.5 w-3.5 text-cream-400" aria-hidden />
              {gitLine.segments.join(" · ")}
            </span>
          )}
          <button
            type="button"
            onClick={onPull}
            disabled={!gitLine.isGitRepo || gitActionBusy || readOnly}
            data-help-title="This pulls the latest changes from origin (fast-forward only)."
            data-help-lines="Pull downloads the current branch's new commits from origin and fast-forwards.|It never merges or rebases: if your branch has diverged, it stops and shows the git error.|Resolve a divergence yourself (commit/stash, then merge or rebase) before pulling again.|The working tree is left untouched when a fast-forward is not possible."
            className="inline-flex items-center gap-1.5 rounded-lg border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-700 hover:bg-cream-50 disabled:opacity-60"
          >
            Pull
          </button>
          <button
            type="button"
            onClick={() => setCommitOpen((open) => !open)}
            disabled={!gitLine.isGitRepo || gitActionBusy || readOnly}
            data-help-title="This commits the tracked changes on the current branch."
            data-help-lines="A commit records the modified, tracked files on the current branch only.|Untracked files are not swept in; stage them in your editor if needed.|Enter a short message describing the change.|The app never force-anything; on failure the git error is shown."
            className="inline-flex items-center gap-1.5 rounded-lg bg-terracotta px-3 py-1.5 text-[12px] font-semibold text-white hover:bg-terracotta/90 disabled:opacity-60"
          >
            Commit
          </button>
          <button
            type="button"
            onClick={onPush}
            disabled={!gitLine.isGitRepo || gitActionBusy || readOnly}
            data-help-title="This pushes the current branch to origin."
            data-help-lines="Push uploads the current branch's commits to the origin remote.|It never force-pushes, so it can only fast-forward the remote.|If there is no upstream or the push is rejected, the git error is shown.|Commit first if you have local changes you want to push."
            className="inline-flex items-center gap-1.5 rounded-lg bg-teal px-3 py-1.5 text-[12px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60"
          >
            Push
          </button>
        </div>
      </div>

      {/* Commit message input (small, inline — no modal). */}
      {commitOpen && !readOnly && (
        <div className="flex flex-col gap-2 rounded-2xl border border-cream-200 bg-white p-3 sm:flex-row sm:items-center">
          <input
            value={commitMessage}
            onChange={(e) => setCommitMessage(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") submitCommit();
            }}
            placeholder="Commit message"
            maxLength={2000}
            autoFocus
            className="min-w-0 flex-1 rounded-lg border border-cream-200 bg-cream-50 px-3 py-2 text-[12px] text-cream-700 outline-none focus:border-terracotta-200"
          />
          <button
            type="button"
            onClick={submitCommit}
            disabled={!commitMessage.trim() || gitActionBusy || readOnly}
            className="shrink-0 rounded-lg bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
          >
            Commit
          </button>
        </div>
      )}

      {/* Inline git action status (success or git stderr). */}
      {gitActionMessage && (
        <p
          className={`rounded-lg px-3 py-2 text-[11px] font-semibold ${
            gitActionError
              ? "bg-coral/[0.06] text-coral-dark"
              : "bg-sage/10 text-sage-dark"
          }`}
        >
          {gitActionMessage}
        </p>
      )}

      {/* GH-P4: agent push-approval gate — agents commit freely, the human approves
          every push. Surfaces this project's pending request(s) with Approve/Deny.
          Hidden when archived (read-only): an archived project has no live agents to
          push, and approving a push is a mutation — gate it defensively by hiding. */}
      {!readOnly && <PushApprovalCard projectId={project.metadata.id} />}
      {/* Slice 5b: renderless poller that surfaces Claude file-bridge consent requests into
          the shared consent modal (kept out of this component to preserve the no-self-poller
          invariant). */}
      {!readOnly && (
        <ConsentBridgePoller
          projectId={project.metadata.id}
          onPending={handleBridgePending}
        />
      )}

      {/* Permission-broker consent gate — surfaces the head of the FIFO queue when
          a mini-coder is blocked. Handles Net (Slice 0) and FolderWrite (Slice 2).
          Hidden when archived (no live agents to prompt). Grant applies on NEXT spawn. */}
      {/* FIX 1: render the HEAD of the queue; subsequent requests queue up and
          become the new head once the user acts on the current one. */}
      {!readOnly &&
        pendingConsents.length > 0 &&
        (() => {
          const head = pendingConsents[0];
          const decisionHandler = (d: "allowRemember" | "allowOnce" | "deny") =>
            void handleConsentDecision(d);
          // Slice 5: Exec/Patch from a live cloud agent (Claude/Codex) → generic card.
          if (head.kind === "exec" || head.kind === "patch") {
            return (
              <AgentConsentModal
                request={head}
                busy={consentBusy}
                error={consentError}
                onDecision={decisionHandler}
              />
            );
          }
          if (head.kind === "folderWrite") {
            return (
              <FolderConsentModal
                request={head}
                busy={consentBusy}
                error={consentError}
                onDecision={decisionHandler}
              />
            );
          }
          return (
            <NetConsentModal
              request={head}
              busy={consentBusy}
              error={consentError}
              onDecision={decisionHandler}
            />
          );
        })()}

      {/* ---- Skills & Tools modal ---- */}
      {skillsOpen && (
        <SkillsToolsModal projectRoot={censorRoot} onClose={closeSkills} />
      )}

      {/* ---- Launcher (anchored above the console grid) ---- */}
      {launcherOpen && !readOnly && (
        <SpawnPanel
          projects={[
            { id: project.metadata.id, title: project.metadata.title },
          ]}
          lockedProjectId={project.metadata.id}
          selectedProjectId={project.metadata.id}
          tasks={project.state.tasks}
          projectActive={canLaunch}
          isBusy={isBusy}
          message={launchMessage}
          rules={rules}
          customClients={customClients}
          localCoderModel={localCoderModel}
          onLaunch={onLaunch}
          onCopyPrompt={onCopyPrompt}
        />
      )}

      {/* ---- Main: Living Plan (left nav, replaces the rail) + Focus stage (center) ---- */}
      <div className="grid grid-cols-1 gap-4 min-h-[60vh] lg:grid-cols-[300px_minmax(0,1fr)]">
        <div className="overflow-hidden rounded-2xl border border-cream-200 bg-white">
          <LivingPlan
            model={workConsoleModel}
            selectedAgentId={selectedAgentId}
            onSelect={selectAgentWithTask}
            dirtyAgentIds={dirtyAgentIds}
          />
        </div>

        <section className="min-w-0">
          {selectedSession ? (
            <div className="space-y-3">
              <div className="flex items-center justify-between gap-2">
                <div className="flex min-w-0 items-center gap-2">
                  <span className="truncate text-[12px] font-semibold text-cream-700">
                    {selectedSession.agentId}
                  </span>
                  <TokenUsageBadge usage={tokenUsage} />
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  {/* MC-P7: the Compact action — for a selected Claude OR Codex agent
                      (resolved client === "claude" || client === "codex"). Claude runs
                      `/compact` in its terminal; Codex calls the app-server thread compact
                      JSON-RPC. Both shrink the agent's context window. Independent of the
                      mini Stop brake: a coder shows Compact (not Stop); a mini shows Stop
                      (not Compact, unless it is itself a claude/codex mini). */}
                  {selectedCanCompact && (
                    <button
                      type="button"
                      onClick={compactSelected}
                      disabled={readOnly}
                      className="inline-flex items-center gap-1 rounded-2xl border border-teal bg-teal px-2.5 py-0.5 text-[10px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60"
                      data-help-title="Compacts this agent's context to free up its context window."
                      data-help-lines="Sends a compact command to this agent.|The agent summarizes the conversation so far, freeing context window so it can keep working longer.|Shown for Claude and Codex agents — both support context compaction.|A one-click convenience."
                    >
                      <Minimize2 className="h-3 w-3" aria-hidden />
                      Compact
                    </button>
                  )}
                  {/* MC-P5: the Stop (kill) safety brake — ONLY for a selected mini.
                      1-click (no two-step confirm): a human Stop is an immediate
                      override. A non-mini agent never shows THIS; it shows the
                      normal-agent Stop button below (the restored stop_agent flow). */}
                  {selectedIsMini && (
                    <button
                      type="button"
                      onClick={stopMini}
                      className="inline-flex items-center gap-1 rounded-2xl border border-coral bg-coral px-2.5 py-0.5 text-[10px] font-semibold text-white hover:bg-coral-dark disabled:opacity-60"
                      data-help-title="Stop this mini-coder now."
                      data-help-lines="Immediately kills this mini-coder; the parent coder will be told it was aborted and must escalate to you.|This is a one-click safety brake — there is no confirm step.|Only mini-coders show this button; a normal agent is stopped from its own controls."
                    >
                      <OctagonX className="h-3 w-3" aria-hidden />
                      Stop
                    </button>
                  )}
                  {/* Open CLI — ONLY for a selected agent that runs in an EXTERNAL
                      console (no in-app PTY). Focuses its dedicated OS terminal
                      window via the parent's focus_agent_terminal. Hidden for an
                      app-hosted agent (its terminal is the in-app viewer). */}
                  {selectedShowsOpenCli && (
                    <button
                      type="button"
                      onClick={focusCliSelected}
                      className="inline-flex items-center gap-1 rounded-md border border-cream-200 bg-white px-2 py-0.5 text-[10px] font-semibold text-cream-600 hover:text-teal"
                      data-help-title="This focuses this agent's external terminal window."
                      data-help-lines="Open CLI restores and brings this agent's dedicated console window to the front.|It applies to externally launched agents (their own OS console).|If the window was closed, you get a friendly error instead.|Relaunch from the Spawn panel if the terminal is gone."
                    >
                      <Terminal className="h-3 w-3" aria-hidden />
                      Open CLI
                    </button>
                  )}
                  {/* Recovery — copy a reconnect prompt for the selected agent.
                      Pure client-side text (no backend call, no secret). Always
                      available for a selected agent; most useful when its heartbeat
                      has gone stale but its terminal may still be alive. */}
                  {selectedSession && (
                    <button
                      type="button"
                      onClick={copyRecoverySelected}
                      className="inline-flex items-center gap-1 rounded-md border border-amber/30 bg-amber/[0.08] px-2 py-0.5 text-[10px] font-semibold text-amber-dark hover:bg-amber/[0.14]"
                      data-help-title="This copies a recovery prompt for this agent."
                      data-help-lines="Recovery is for an agent whose heartbeat went stale but whose terminal may still be open.|It copies exact reconnect steps without exposing hidden tokens.|If the agent lost its session token, relaunch instead.|Always verify the terminal root before trusting status updates."
                    >
                      <LifeBuoy className="h-3 w-3" aria-hidden />
                      Recovery
                    </button>
                  )}
                  {/* Normal-agent Stop — the restored board-mode stop_agent
                      capability, the ONLY UI surface to kill a stalled/runaway
                      NORMAL (non-mini) agent. Mutually exclusive with the mini
                      brake above: a mini shows that brake, a normal agent shows
                      this. Kills the agent's process tree via the parent. */}
                  {selectedShowsNormalStop && (
                    <button
                      type="button"
                      onClick={stopSelected}
                      className="inline-flex items-center gap-1 rounded-md border border-cream-200 bg-white px-2 py-0.5 text-[10px] font-semibold text-cream-600 hover:text-coral-dark disabled:opacity-60"
                      data-help-title="This stops the agent session."
                      data-help-lines="Stop ends the launched agent.|For an app-hosted agent it kills the PTY child; for an external one it closes its console.|It does not delete the task; it only ends the agent.|Relaunch from the Spawn panel if you still need the work done."
                    >
                      <Square className="h-3 w-3" aria-hidden />
                      Stop
                    </button>
                  )}
                  <button
                    type="button"
                    onClick={() =>
                      setSplitAgentId((cur) =>
                        cur ? null : selectedSession.agentId,
                      )
                    }
                    // Splitting needs a SECOND agent to compare against — disable when there's
                    // only one session (unsplit is always allowed).
                    disabled={!splitAgentId && sessions.length < 2}
                    aria-pressed={splitAgentId != null}
                    title={
                      splitAgentId
                        ? "Close the split view"
                        : sessions.length < 2
                          ? "Need a second agent to split the view"
                          : "Pin this agent and open a second focus pane"
                    }
                    className={`inline-flex items-center gap-1 rounded-md border px-2 py-0.5 text-[10px] font-semibold disabled:cursor-not-allowed disabled:opacity-50 ${
                      splitAgentId
                        ? "border-terracotta bg-terracotta/10 text-terracotta"
                        : "border-cream-200 bg-white text-cream-600 hover:text-terracotta"
                    }`}
                  >
                    <Columns2 className="h-3 w-3" aria-hidden />
                    {splitAgentId ? "unsplit" : "split"}
                  </button>
                  {/* Skills & Tools: per-role skills/tools for this project's agents.
                      Opens the modal; needs a resolved project root. */}
                  <button
                    type="button"
                    onClick={() => setSkillsOpen(true)}
                    disabled={!censorRoot || readOnly}
                    title={
                      readOnly
                        ? "Archived project — skills & tools are read-only"
                        : censorRoot
                          ? "Manage skills & tools for this project's agents"
                          : "Open a project folder first"
                    }
                    className="inline-flex items-center gap-1 rounded-md border border-cream-200 bg-white px-2 py-0.5 text-[10px] font-semibold text-cream-600 hover:text-terracotta disabled:cursor-not-allowed disabled:opacity-50"
                  >
                    <Sparkles className="h-3 w-3" aria-hidden />
                    skills &amp; tools
                  </button>
                  <button
                    type="button"
                    onClick={() => setDrawerOpen((open) => !open)}
                    aria-expanded={drawerOpen}
                    className={`inline-flex items-center gap-1 rounded-md border px-2 py-0.5 text-[10px] font-semibold ${
                      drawerOpen
                        ? "border-terracotta bg-terracotta/10 text-terracotta"
                        : "border-cream-200 bg-white text-cream-600 hover:text-terracotta"
                    }`}
                  >
                    <PanelRightOpen className="h-3 w-3" aria-hidden />
                    drawer ▸
                  </button>
                </div>
                {miniActionError ? (
                  <p className="mt-1 text-[10px] leading-4 text-coral-dark">
                    {miniActionError}
                  </p>
                ) : null}
              </div>

              {/* Unified Work Console: the FocusStage merges the raw PTY terminal (Raw)
                  and the structured activity console (Activity) into ONE surface, with a
                  two-way composer + inline question card for the selected agent. The raw
                  terminal is mounted only for an app-hosted PTY agent; an external/legacy
                  agent shows a tidy note in the Raw slot instead. */}
              {/* The PanelGroup is ALWAYS the focus container — the primary pane lives in a
                  stable Panel whether split or not, so toggling split never remounts the
                  primary (no terminal flash / re-subscribe). The second Panel + handle are
                  added only when an agent is pinned. id/order keep the lib's layout stable
                  across the conditional second panel. */}
              <PanelGroup
                ref={panelGroupRef}
                direction="horizontal"
                className="h-[clamp(560px,calc(100vh-260px),1400px)] overflow-hidden rounded-2xl"
              >
                <Panel
                  id="focus-primary"
                  order={1}
                  defaultSize={splitAgentId ? 50 : 100}
                  minSize={28}
                  className="min-w-0"
                >
                  <FocusStagePane
                    agentId={selectedSession.agentId}
                    model={workConsoleModel}
                    sessions={sessions}
                    ptyAgents={ptyAgents}
                    readOnly={readOnly}
                  />
                </Panel>
                {splitAgentId ? (
                  <>
                    <PanelResizeHandle className="mx-1 w-1.5 rounded-full bg-cream-200 transition-colors hover:bg-terracotta/60 data-[resize-handle-active]:bg-terracotta" />
                    <Panel
                      id="focus-secondary"
                      order={2}
                      defaultSize={50}
                      minSize={28}
                      className="min-w-0"
                    >
                      <FocusStagePane
                        agentId={splitAgentId}
                        model={workConsoleModel}
                        sessions={sessions}
                        ptyAgents={ptyAgents}
                        readOnly={readOnly}
                        onClose={() => setSplitAgentId(null)}
                      />
                    </Panel>
                  </>
                ) : null}
              </PanelGroup>

              {drawerOpen && (
                <AgentDetailDrawer
                  session={selectedSession}
                  claims={claims}
                  events={events}
                  now={now}
                />
              )}

              {/* The agent's question card is now inline in the FocusStage (Direction B:
                  the agent asks, the composer becomes an answer box). */}
            </div>
          ) : (
            <div className="flex h-72 items-center justify-center rounded-2xl border border-dashed border-cream-200 bg-cream-50 text-center text-[12px] text-cream-400">
              {sessions.length === 0
                ? "No agent working this project. Use + Launch to spawn a coder or verifier."
                : "Select an agent from the left to view its terminal."}
            </div>
          )}
        </section>
      </div>

      {/* ---- Consolidated tab bar: 8 tabs in one row ---- */}
      <div className="rounded-2xl border border-cream-200 bg-white">
        <div className="flex w-fit gap-1 border-b border-cream-200 p-1" role="tablist" aria-label="Work console tabs">
          {DOCK_TABS.map((tab) => {
            const active = dockTab === tab.id;
            // Badge counts for specific tabs.
            let badge: number | null = null;
            if (tab.id === "tasks") badge = tasksBadgeCount || null;
            else if (tab.id === "censor") badge = censorStrip.openFindings || null;
            else if (tab.id === "plans" && !readOnly) badge = planPendingCount || null;
            return (
              <button
                key={tab.id}
                type="button"
                role="tab"
                id={`worktab-${tab.id}`}
                aria-controls={`workpanel-${tab.id}`}
                aria-selected={active}
                onClick={() => setDockTab(tab.id)}
                className={`inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-[12px] font-semibold transition-colors ${
                  active
                    ? "bg-terracotta text-white"
                    : "text-cream-500 hover:bg-cream-50 hover:text-cream-700"
                }`}
              >
                {tab.label}
                {badge != null && (
                  <span
                    className={`rounded-full px-1.5 py-0.5 text-[10px] font-semibold leading-none ${
                      active
                        ? "bg-white/20 text-white"
                        : "bg-cream-100 text-cream-600"
                    }`}
                  >
                    {badge}
                  </span>
                )}
              </button>
            );
          })}
        </div>

        <div className="p-4" role="tabpanel" id={`workpanel-${dockTab}`} aria-labelledby={`worktab-${dockTab}`}>
          {/* Plan approval card: mounted ALWAYS (hidden when not on Plans tab) so its
              poll keeps the badge count fed. Renders at the TOP of the Plans tab. */}
          {!readOnly && (
            <PlanApprovalCard
              projectId={project.metadata.id}
              onPendingCountChange={setPlanPendingCount}
              hidden={dockTab !== "plans"}
            />
          )}

          {/* ---- Tasks tab ---- */}
          {dockTab === "tasks" && taskBoardSlot}

          {/* ---- Censor tab ---- */}
          {dockTab === "censor" && (
            <div className="space-y-5">
              <div className="overflow-hidden rounded-2xl">
                <CensorStrip model={censorStrip} />
              </div>
              {!readOnly && (
                <SandboxModeSelector
                  projectId={project.metadata.id}
                  sandboxMode={project.metadata.sandboxMode}
                  onModeChange={onSandboxModeChange}
                />
              )}
              {!readOnly && (
                <WorkingSetCard
                  projectId={project.metadata.id}
                  workingSet={project.metadata.workingSet}
                  onWorkingSetChange={onWorkingSetChange}
                />
              )}
              {!readOnly && (
                <AgentControlsCard
                  projectId={project.metadata.id}
                  controls={project.metadata.agentControls}
                  onControlsChange={() => onReloadProject?.()}
                />
              )}
              <CensorPanel
                projectId={project.metadata.id}
                root={project.metadata.rootPath}
                findings={censorFindings}
                onLaunch={onLaunch}
                isBusy={isBusy}
                canLaunch={canLaunch}
              />
            </div>
          )}

          {/* ---- Git tab ---- */}
          {dockTab === "git" && <DockGit project={project} />}

          {/* ---- Changes tab ---- */}
          {dockTab === "changes" && <ChangesDockTab project={project} />}

          {/* ---- Plans tab ---- */}
          {dockTab === "plans" && (
            <PlansDockTab projectId={project.metadata.id} />
          )}

          {/* ---- Notes tab ---- */}
          {dockTab === "notes" && notesSlot}

          {/* ---- MCP tab ---- */}
          {dockTab === "mcp" &&
            (project.metadata.rootPath ? (
              <ProjectMcpServersCard projectRoot={project.metadata.rootPath} />
            ) : (
              <p className="text-[11px] text-cream-400">
                No project root path — cannot load MCP servers.
              </p>
            ))}

          {/* ---- Project tab ---- */}
          {dockTab === "project" && detailSlot}
        </div>
      </div>
    </div>
  );
}

// ---- Git tab: the gitStatus detail (branch, ahead/behind, counts, upstream) --

function DockGit({ project }: { project: ProjectDetail }) {
  const git = project.gitStatus;
  // Defensive: gitStatus is typed non-null but the backend can omit it (other call
  // sites — projectWorkspaceModel/censorCounts — treat it as nullable), so guard so
  // the Git dock tab renders a message instead of crashing on a missing status.
  if (!git || !git.isGitRepo) {
    return (
      <p className="text-[11px] text-cream-400">
        The project root is not inside a Git repository.
      </p>
    );
  }
  const rows: { label: string; value: string }[] = [
    { label: "Branch", value: git.branch ?? "—" },
    { label: "Upstream", value: git.upstream ?? "no upstream" },
    {
      label: "Ahead / Behind",
      value: `↑${git.aheadCount} / ↓${git.behindCount}`,
    },
    { label: "Staged", value: String(git.stagedCount) },
    { label: "Unstaged", value: String(git.unstagedCount) },
    { label: "Untracked", value: String(git.untrackedCount) },
    { label: "Dirty total", value: String(git.dirtyCount) },
    { label: "Last commit", value: git.commit ?? "—" },
  ];
  return (
    <dl className="grid grid-cols-1 gap-x-6 gap-y-2 sm:grid-cols-2">
      {rows.map((row) => (
        <div
          key={row.label}
          className="flex items-center justify-between gap-3"
        >
          <dt className="text-[10px] font-semibold uppercase tracking-widest text-cream-500">
            {row.label}
          </dt>
          <dd className="truncate font-mono text-[11px] text-cream-700">
            {row.value}
          </dd>
        </div>
      ))}
    </dl>
  );
}

export default ProjectWorkspace;
