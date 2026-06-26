import {
  AlertCircle,
  Archive,
  CheckCircle2,
  Circle,
  Clock3,
  FolderKanban,
  FolderOpen,
  GitBranch,
  Play,
  Plus,
  ShieldCheck,
} from "lucide-react";
import {
  type ProjectStageId,
  projectStage,
  stageLabel,
  stageTone,
} from "../projects/projectStage";
import { ProjectsBoard } from "../projects/ProjectsBoard";
import { PlannerPlanMode } from "../projects/planner/PlannerPlanMode";
import {
  derivePlanCards,
  latestWeb,
  pickProjectDesign,
  chatMessages,
} from "../projects/planner/plannerModel";
import { useAgentConsole } from "../agents/useAgentConsole";
import { useDesignRequestWatcher } from "../projects/planner/useDesignRequestWatcher";
import type { PlannerMessage } from "../projects/planner/plannerModel";
import type { DesignProjectEntry } from "../../types/design";
import { ProjectCalendar } from "../projects/ProjectCalendar";
import {
  clearPersistedProjectRootDraft,
  persistProjectRootDraft,
  readPersistedProjectRootDraft,
} from "./projectRootDraft";
import {
  CensorCountsTracker,
  censorTrackedSignature,
  type CensorCountByProject,
} from "../projects/censorCounts";
import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  invokeBackendCommand,
  isTauriRuntime,
  useAppActions,
  useAppContext,
} from "../../context/AppContext";
import { useAgentAttentionStore } from "../../store/agentAttentionStore";
import {
  useWorkSelectionStore,
  primaryAgentForTask,
} from "../../store/workSelectionStore";
import {
  parseWorkTab,
  shouldClearWorkEntryBridge,
  shouldExitWorkMode,
} from "../../utils/deepLink";
import type {
  AgentClaim,
  AgentEvent,
  AgentLiveState,
  AgentSession,
  ProjectAgentLaunchResult,
  ProjectDetail,
  ProjectGitCommandResult,
  ProjectLiveStatus,
  ProjectStatus,
  ProjectSummary,
  ProjectTask,
  ProjectTaskCategory,
  SandboxMode,
  SavedWorkflow,
} from "../../types/backend";
import { isOpenClaim, isRecentProjectSession } from "../../utils/agentClaims";
import { CollapsibleSection } from "../projects/CollapsibleSection";
import { formatDateTime } from "../projects/projectFormat";
import type { SpawnRole } from "../agents/roleDisplay";

// Work-mode shell, lazy so its (and the terminal's) chunk loads only when a card
// is opened into the full-screen IDE view.
const ProjectWorkspace = lazy(() =>
  import("../projects/ProjectWorkspace").then((m) => ({
    default: m.ProjectWorkspace,
  })),
);
import {
  normalizeModelHint,
  type SpawnLaunchInput,
  type SpawnSelection,
} from "../agents/agentRowModel";
import { buildWorkflowLaunchInput } from "../agents/savedWorkflowModel";
import { ProjectStatusHeader } from "../projects/ProjectStatusHeader";
import { ProjectNotes } from "../projects/ProjectNotes";
import { TaskCard } from "../projects/TaskCard";
import { TaskDependencyArrows } from "../projects/TaskDependencyArrows";
import { TASK_CATEGORIES, categoryLabel } from "../projects/taskCategory";
import type { ColumnId } from "../projects/taskBoard";
import { deriveWorkers, agentColorHex } from "../projects/agentBadge";
import { freshestSession } from "../projects/agentLiveStatus";
import {
  commitProjectCall,
  pushProjectCall,
  pullProjectCall,
  cloneProjectCall,
  isLikelyGithubRepoUrl,
  runGitActionGuarded,
} from "../projects/projectWorkspaceModel";

// Single dedupe signature for a polled agent live state. Combines updatedAt with
// the collection sizes so a nested mutation that fails to bump updatedAt is
// still picked up. Used for BOTH the setState-skip AND the project-refresh
// dedupe so the two can never drift apart (#7/#8). Null-safe: a null/undefined
// live state (backend returned nothing) yields a stable sentinel rather than
// throwing on `.updatedAt`.
export function agentStateSignature(
  state: AgentLiveState | null | undefined,
): string {
  if (!state) return "∅";
  return `${state.updatedAt ?? ""}|${state.sessions?.length ?? 0}|${state.claims?.length ?? 0}|${state.events?.length ?? 0}`;
}

// A trailing click fired within this window after a card drag-end is treated as drag noise,
// not a real selection (see dragEndAtRef).
const DRAG_CLICK_SUPPRESS_MS = 300;

const columns: { id: ColumnId; label: string; icon: typeof Circle }[] = [
  { id: "todo", label: "To do", icon: Circle },
  { id: "wip", label: "In progress", icon: Clock3 },
  { id: "review", label: "In review", icon: ShieldCheck },
  { id: "blocked", label: "Blocked", icon: AlertCircle },
  { id: "done", label: "Done", icon: CheckCircle2 },
];

function canLaunchProjectAgents(project: ProjectDetail | null) {
  return project?.metadata.status === "active";
}

function projectLaunchTitle(project: ProjectDetail | null) {
  return canLaunchProjectAgents(project)
    ? "Launch agent"
    : "Only active projects can launch agents.";
}

function canCoderClaimTask(task: ProjectTask) {
  return (
    task.status === "todo" || task.status === "wip" || task.status === "blocked"
  );
}

function canVerifierClaimTask(task: ProjectTask) {
  return task.status === "review" || task.status === "blocked";
}

function recommendedTaskRole(task: ProjectTask): "coder" | "verifier" {
  return canVerifierClaimTask(task) && !canCoderClaimTask(task)
    ? "verifier"
    : "coder";
}

function taskMoveTargets(task: ProjectTask) {
  if (task.status === "done") return [];
  return columns.filter(
    (target) => target.id !== task.status && target.id !== "done",
  );
}

// B5: key under which the create-flow working-folder draft is persisted, so it
// survives the app's idle auto-lock (which unmounts ProjectsView and wipes all
// volatile useState). Read defensively — localStorage can throw (private mode,
// disabled storage) and must never break the view.
const NEW_PROJECT_ROOT_DRAFT_KEY = "devboule.newProjectRootDraft";

function readPersistedRootDraft(): string {
  try {
    return localStorage.getItem(NEW_PROJECT_ROOT_DRAFT_KEY) ?? "";
  } catch {
    return "";
  }
}

// B15b: remember the orchestrator agentId per project, so the planner chat can keep
// reading the durable transcript after the orchestrator session ends (the in-memory
// activity store still has it) — and, via read_activity_chat, across an app restart.
// Without this the chat was bound to the LIVE session id; when it went null the whole
// conversation vanished (the reported "steer kills the orchestrator + chat resets" bug).
const orchAgentKey = (projectId: string) => `devboule.orch.${projectId}`;

function readPersistedOrchestratorId(projectId: string | null | undefined): string | null {
  if (!projectId) return null;
  try {
    return localStorage.getItem(orchAgentKey(projectId));
  } catch {
    return null;
  }
}

export function ProjectsView() {
  const { pendingTab, config, requestView } = useAppContext();
  const { consumePendingTab } = useAppActions();
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  // Open Censor finding count per project id, fed by an event-driven tracker (NO
  // new poller): one count_open per project on (re)bind + a refetch on each
  // censor://findings-updated event. Drives the board card ⚠ chip.
  const [censorCountByProject, setCensorCountByProject] =
    useState<CensorCountByProject>({});
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // Phase B twinning: the board card selection is the SAME shared selection the Work
  // Console (LivingPlan/FocusStage) drives. Reading selectedTaskId here highlights the
  // active card; clicking a card sets task + its primary agent so the console focuses it.
  const selectedTaskId = useWorkSelectionStore((s) => s.selectedTaskId);
  const selectBothSelection = useWorkSelectionStore((s) => s.selectBoth);
  const clearSelection = useWorkSelectionStore((s) => s.clear);
  // A card drag emits a trailing click on some platforms; suppress the spurious select that
  // would otherwise hijack the Work Console focus right after a board move. Timestamp-based
  // (not a boolean + rAF) so there is no leaked frame callback and no rAF-vs-click ordering race:
  // a click within DRAG_CLICK_SUPPRESS_MS of a drag-end is ignored; a genuine later click passes.
  const dragEndAtRef = useRef(0);
  // Work mode is a SUB-STATE of the board (not a new nav view): clicking a card
  // flips this on and renders ProjectWorkspace full-bleed; `← Board` flips it off
  // while KEEPING selectedId so the card stays selected on the board.
  const [workMode, setWorkMode] = useState(false);
  // Overview (board-mode) segmented toggle: the active stage board + calendar
  // (default) vs. a simple read-only list of archived projects. Purely a view
  // sub-state of board mode; it never affects Work mode or the selection.
  const [overviewTab, setOverviewTab] = useState<"board" | "archived">("board");
  // Planner panel (Plan Mode) controls — lifted from the old OrchestratorHeroCard so
  // the choices survive (coder hand-off, auto-create, websearch auto/manual).
  const [plannerCoderId, setPlannerCoderId] = useState<string>("claude");
  // Which backend runs as the ORCHESTRATOR (who you talk to). "orchestrator" = local Devboule
  // (our Stage/TUI); "claude"/"codex" run their own CLI (we show their terminal). Default local.
  const [plannerOrchestratorClient, setPlannerOrchestratorClient] =
    useState<string>("orchestrator");
  // B10: discuss-first by default. OFF = the orchestrator converses on turn 1 and
  // only plans/creates when the user clicks "Create plan"; ON = the old eager mode
  // (plan-first + auto-create on launch). The choice is preserved (a toggle), just
  // defaulted to the non-eager behavior the owner wants.
  const [plannerAutoCreate, setPlannerAutoCreate] = useState<boolean>(false);
  const [plannerWebMode, setPlannerWebMode] = useState<"auto" | "manual">("auto");
  // The chat transcript + active goal echo shown in the planner panel. P0 seeds them
  // optimistically on send; P2 replaces them with the orchestrator's live transcript.
  const [plannerMessages, setPlannerMessages] = useState<PlannerMessage[]>([]);
  const [plannerGoal, setPlannerGoal] = useState<string | null>(null);
  // True from the moment a Planner launch starts until its session registers (the agent
  // poll lags the launch by up to ~5s). Guards the composer so a second send in that window
  // can't create a duplicate project or launch a second Planner. A timeout safety clears it
  // if the session never registers (e.g. the orchestrator crashes on start).
  const [plannerLaunching, setPlannerLaunching] = useState(false);
  // B2 F2: the EXACT agentId of the cloud orchestrator (claude/codex) captured at
  // launch, so its terminal binds to the right session instead of `.find(by client)`
  // — which after a hand-off can match a CODER of the same CLI.
  const [cloudOrchestratorLaunchedId, setCloudOrchestratorLaunchedId] = useState<
    string | null
  >(null);
  const plannerLaunchTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // The project's most-recent design (read-only preview in the planner's Design tab).
  const [plannerDesign, setPlannerDesign] = useState<{
    name: string;
    version: string | null;
    ago: string | null;
    thumbnailUri: string | null;
  } | null>(null);
  // Bumped when a design-request the watcher fulfilled completes, to re-run the design
  // load effect so the freshly-generated design surfaces in the Stage Design view (P-B L5).
  const [plannerDesignNonce, setPlannerDesignNonce] = useState(0);
  // Frecce (Phase 17): show/hide the dependency-arrow overlay on the task board.
  const [showArrows, setShowArrows] = useState(true);
  const [project, setProject] = useState<ProjectDetail | null>(null);
  // Inline status for the Work-mode Commit/Push controls (success or git stderr).
  const [gitActionMessage, setGitActionMessage] = useState<string | null>(null);
  const [gitActionError, setGitActionError] = useState(false);
  const [gitActionBusy, setGitActionBusy] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");
  // R1: the project's working folder, chosen at creation (the agent reads/writes here).
  // B5: persisted to localStorage so a folder picked for a NEW project survives the
  // app's idle auto-lock — which unmounts this whole view (App renders <LockedScreen>
  // when isLocked), wiping all volatile useState. Without this the user picks a
  // folder, the app self-locks mid-planning, and on unlock the draft is gone, so the
  // project gets created with rootPath=null. Restored on mount, cleared after create.
  const [newProjectRootDraft, setNewProjectRootDraft] = useState(
    () => readPersistedRootDraft(),
  );
  // S5: the configured default external main-coder CLI for task-card launches (claude|codex).
  const [mainCoderClient, setMainCoderClient] = useState<"claude" | "codex">(
    "codex",
  );
  // Clone-from-GitHub dialog (board): open flag, pasted URL draft, busy + error.
  const [cloneOpen, setCloneOpen] = useState(false);
  const [cloneUrlDraft, setCloneUrlDraft] = useState("");
  const [cloneBusy, setCloneBusy] = useState(false);
  const [cloneError, setCloneError] = useState<string | null>(null);
  const [taskDraft, setTaskDraft] = useState("");
  // Mandatory category for a new Todo card; null until the user picks one so the
  // create button stays disabled (forces an explicit choice for clarity).
  const [taskCategory, setTaskCategory] = useState<ProjectTaskCategory | null>(
    null,
  );
  // Bug description, revealed only when category === "bug". Persisted on the
  // card; P2 will use it as the Oracle localization query.
  const [taskBugDescription, setTaskBugDescription] = useState("");
  const [noteDraft, setNoteDraft] = useState("");
  const [rootDraft, setRootDraft] = useState("");
  const [launchMessage, setLaunchMessage] = useState<string | null>(null);
  const [savedWorkflows, setSavedWorkflows] = useState<SavedWorkflow[]>([]);
  const [workflowArgs, setWorkflowArgs] = useState<Record<string, string>>({});
  const [workflowError, setWorkflowError] = useState<string | null>(null);
  const [workflowBusyName, setWorkflowBusyName] = useState<string | null>(null);
  const [agentState, setAgentState] = useState<AgentLiveState | null>(null);
  // agent_ids with a live app-hosted PTY (from agent_pty_list, fetched on the
  // board poll). Threaded into Work mode (ProjectWorkspace) to gate the shared
  // AgentRow's Terminal toggle exactly like the global Agents room.
  const [ptyAgents, setPtyAgents] = useState<Set<string>>(() => new Set());
  const [isBusy, setIsBusy] = useState(false);
  const [isLoadingProjects, setIsLoadingProjects] = useState(true);
  const [loadingProjectId, setLoadingProjectId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [agentSyncError, setAgentSyncError] = useState<string | null>(null);
  const busyRef = useRef(false);
  // Guards the whole orchestrator "Plan it" sequence (note write → launch). `busyRef` alone has a
  // gap between the note mutation finishing and the launch acquiring it, where a fast second click
  // could double-fire (two notes + two orchestrators); this ref spans the entire sequence.
  const orchestratorPlanRef = useRef(false);
  // Reentrancy guard for the Work-mode git actions (commit/push). A fast
  // double-click or Commit-then-Push must not fire two concurrent git ops on the
  // same repo (double commit / non-fast-forward push). Mirrors busyRef: set in a
  // try, cleared in finally. The `gitActionBusy` STATE drives the disabled UI; the
  // REF is the synchronous guard (state updates are async and can't gate a burst).
  const gitActionBusyRef = useRef(false);
  // FIX 1 (clone double-submit): the SYNCHRONOUS reentrancy gate for cloneProject.
  // `cloneBusy` is React state (drives the disabled UI) but updates asynchronously,
  // so two fast clicks both observe the stale `false` and fire two concurrent
  // project_git_clone calls. The ref is flipped synchronously, so the second click
  // is rejected immediately — mirroring gitActionBusyRef for commit/push/pull.
  const cloneBusyRef = useRef(false);
  const selectedIdRef = useRef<string | null>(null);
  const loadProjectSeqRef = useRef(0);
  // Signature of the agent state we last reloaded projects from (#7/#8): keyed
  // on agentStateSignature, not bare updatedAt, so a nested mutation that does
  // not bump updatedAt still triggers a project reload.
  const lastAgentRefreshRef = useRef<string | null>(null);
  // Tracks the signature of the agent state currently held in React state, so
  // we can skip setAgentState (and the ~6 dependent useMemo grouping maps) when
  // the polled live state has not actually changed.
  const appliedAgentUpdatedAtRef = useRef<string | null>(null);
  const agentStateInFlightRef = useRef(false);
  // True while ProjectsView is mounted (its board poller is the active feeder).
  // loadAgentState checks this AFTER its await so a tick whose navigation/unmount
  // raced the in-flight fetch does NOT feed the attention store — guaranteeing the
  // single-feeder invariant has no double window when switching INTO/OUT of
  // Projects (the global App poller becomes the feeder the moment we leave).
  const viewMountedRef = useRef(true);
  const reloadProjectInFlightRef = useRef(false);
  const pendingAgentRefreshRef = useRef<{
    signature: string;
    state: AgentLiveState;
  } | null>(null);
  const loadProjectsSeqRef = useRef(0);
  const loadAgentStateSeqRef = useRef(0);
  // Phase G BLOCKER bridge: enterWorkMode sets this to the just-selected id
  // SYNCHRONOUSLY so the work-mode-coherence effect holds Work mode on the very
  // first render after a bell deep-link — before the detail-load effect has run
  // setLoadingProjectId(id) (that state lands a render later). Once loadingProjectId
  // catches up (or the detail resolves), the effect clears this bridge so the
  // genuine missing/archived fallback still works. A ref (not state) because it
  // only needs to be read synchronously inside the effect, never to trigger render.
  const pendingWorkEntryIdRef = useRef<string | null>(null);

  useEffect(() => {
    selectedIdRef.current = selectedId;
  }, [selectedId]);

  // B5: mirror the create-flow folder draft to localStorage on every change, so a
  // folder picked for a new project outlives the idle auto-lock unmount. Clearing
  // the draft after create (setNewProjectRootDraft("")) writes "" here too, which
  // we treat as "no draft" and remove from storage.
  useEffect(() => {
    try {
      if (newProjectRootDraft.trim()) {
        localStorage.setItem(NEW_PROJECT_ROOT_DRAFT_KEY, newProjectRootDraft);
      } else {
        localStorage.removeItem(NEW_PROJECT_ROOT_DRAFT_KEY);
      }
    } catch {
      /* storage unavailable — non-fatal, the draft just won't persist */
    }
  }, [newProjectRootDraft]);

  // Memoized so the six downstream useMemos keyed on currentProject don't all
  // recompute on every unrelated state update (e.g. the 5s agent poll).
  const currentProject = useMemo(
    () => (project?.metadata.id === selectedId ? project : null),
    [project, selectedId],
  );

  // READ-ONLY gate: the selected project is archived. Every mutation handler
  // early-returns on this, and the Work-mode page renders in read-only mode with
  // an [Unarchive] banner. Derived from the loaded detail's metadata status.
  const isArchived = currentProject?.metadata.status === "archived";

  const loadSavedWorkflows = useCallback(async (projectId: string) => {
    setWorkflowError(null);
    try {
      const workflows = await invokeBackendCommand<SavedWorkflow[]>(
        "list_saved_workflows",
        { projectId },
      );
      if (selectedIdRef.current === projectId) {
        setSavedWorkflows(workflows);
      }
    } catch (e) {
      const message =
        e instanceof Error ? e.message : "Saved workflows could not be loaded.";
      if (selectedIdRef.current === projectId) {
        setSavedWorkflows([]);
        setWorkflowError(message);
      }
    }
  }, []);

  const tasksByColumn = useMemo(() => {
    const grouped: Record<ColumnId, ProjectTask[]> = {
      todo: [],
      wip: [],
      review: [],
      blocked: [],
      done: [],
    };
    for (const task of currentProject?.state.tasks ?? []) {
      const key = columns.some((column) => column.id === task.status)
        ? (task.status as ColumnId)
        : "todo";
      grouped[key].push(task);
    }
    return grouped;
  }, [currentProject]);

  const loadProjects = useCallback(async () => {
    const requestSeq = ++loadProjectsSeqRef.current;
    setError(null);
    const list = await invokeBackendCommand<ProjectSummary[]>("list_projects");
    if (requestSeq !== loadProjectsSeqRef.current) return null;
    setProjects(list);
    setSelectedId((current) => {
      // Open the landing EMPTY: a project is a plan you CREATE here, so we never
      // auto-select the most-recent one. Keep a still-valid current selection (e.g. after a
      // mutation / entering work mode), otherwise clear it.
      const next =
        current && list.some((item) => item.id === current) ? current : null;
      selectedIdRef.current = next;
      if (!next) setProject(null);
      return next;
    });
    return list;
  }, []);

  const loadProject = useCallback(async (projectId: string) => {
    const requestSeq = ++loadProjectSeqRef.current;
    setError(null);
    setLoadingProjectId(projectId);
    try {
      const detail = await invokeBackendCommand<ProjectDetail>("get_project", {
        projectId,
      });
      if (
        requestSeq === loadProjectSeqRef.current &&
        selectedIdRef.current === projectId
      ) {
        setProject(detail);
      }
    } finally {
      if (requestSeq === loadProjectSeqRef.current) {
        setLoadingProjectId(null);
      }
    }
  }, []);

  useEffect(() => {
    if (!currentProject) {
      setSavedWorkflows([]);
      setWorkflowArgs({});
      setWorkflowError(null);
      return;
    }
    void loadSavedWorkflows(currentProject.metadata.id);
  }, [currentProject?.metadata.id, loadSavedWorkflows]);

  // Single source of truth for writing agent state into React state: it sets the
  // state AND records the applied signature so the next poll can correctly skip
  // an unchanged snapshot. Crucially it must be used by BOTH the poll path and
  // stopAgent (#1) — stopAgent that wrote setAgentState without updating the
  // signature ref would let the very next poll re-apply an OLDER on-disk
  // snapshot, making the stopped agent reappear.
  const applyAgentState = useCallback((state: AgentLiveState) => {
    appliedAgentUpdatedAtRef.current = agentStateSignature(state);
    setAgentState(state);
  }, []);

  const loadAgentState = useCallback(async () => {
    const requestSeq = ++loadAgentStateSeqRef.current;
    // Fetch live agent state AND the live PTY list together. The PTY list gates
    // the Terminal toggle on app-hosted rows; it is cheap backend-side and
    // tolerant of failure (defaults to empty so the toggle simply hides).
    const [liveState, ptyList] = await Promise.all([
      invokeBackendCommand<AgentLiveState>("get_agent_live_state"),
      invokeBackendCommand<string[]>("agent_pty_list").catch(
        () => [] as string[],
      ),
    ]);
    if (requestSeq !== loadAgentStateSeqRef.current) return null;
    // Navigation/unmount raced this fetch: the global App poller is (or is about
    // to be) the feeder now, so dropping the result here keeps a single feeder and
    // avoids a setState-after-unmount. The seq guard above only covers a NEWER
    // loadAgentState call, not leaving the view — this covers that gap.
    if (!viewMountedRef.current) return null;
    setPtyAgents((prev) => {
      if (prev.size === ptyList.length && ptyList.every((id) => prev.has(id))) {
        return prev;
      }
      return new Set(ptyList);
    });
    // Feed the app-wide attention store from THIS existing board poll (no new
    // poller): the Header bell pill and the OS-notification watcher read it.
    // Runs BEFORE the change-detection early-return below so every tick
    // re-evaluates needsUser transitions even when the board state is unchanged.
    // setFromLiveState is null-tolerant (an empty live state clears attention).
    useAgentAttentionStore.getState().setFromLiveState(liveState);
    setAgentSyncError(null);
    // WARNING: the backend can return null/undefined for get_agent_live_state
    // (e.g. nothing loaded yet). Treat it as an empty live state: the store was
    // already fed above (null clears the bell); skip the apply/refresh paths,
    // which assume a populated shape, so we never call agentStateSignature(null)
    // → TypeError or dereference null collections.
    if (!liveState) return liveState ?? null;
    // Skip the state write (and the dependent grouping useMemos / board
    // re-render) when nothing changed since the last applied snapshot.
    if (appliedAgentUpdatedAtRef.current === agentStateSignature(liveState)) {
      return liveState;
    }
    applyAgentState(liveState);
    return liveState;
  }, [applyAgentState]);

  const agentStateTouchesProject = (
    liveState: AgentLiveState,
    projectId: string,
  ) =>
    (liveState.events ?? []).some((event) => event.projectId === projectId) ||
    (liveState.claims ?? []).some((claim) => claim.projectId === projectId) ||
    (liveState.sessions ?? []).some(
      (session) => session.currentProjectId === projectId,
    );

  const refreshProjectsFromAgentState = useCallback(
    async (signature: string, liveState: AgentLiveState) => {
      if (lastAgentRefreshRef.current === signature) return;
      const list = await loadProjects();
      if (!list) return;
      const selectedProjectId = selectedIdRef.current;
      if (
        selectedProjectId &&
        agentStateTouchesProject(liveState, selectedProjectId)
      ) {
        await loadProject(selectedProjectId);
      }
      lastAgentRefreshRef.current = signature;
    },
    [loadProject, loadProjects],
  );

  useEffect(() => {
    setIsLoadingProjects(true);
    void loadProjects()
      .catch((e) =>
        setError(
          e instanceof Error ? e.message : "Projects could not be loaded.",
        ),
      )
      .finally(() => setIsLoadingProjects(false));
  }, [loadProjects]);

  useEffect(() => {
    if (!selectedId) {
      setProject(null);
      return;
    }
    void loadProject(selectedId).catch((e) => {
      if (selectedIdRef.current !== selectedId) return;
      setProject(null);
      setError(e instanceof Error ? e.message : "Project could not be loaded.");
    });
  }, [loadProject, selectedId]);

  // Per-project reset (keyed on project id). Tracks the PREVIOUS id so we only wipe the
  // planner chat/drafts when LEAVING a project (→ empty landing) or SWITCHING to a different
  // one — NEVER on the null→new transition (creating a project from the empty landing), where
  // onSend/startNewProject just populated the chat (that was the "my message vanished right
  // after the project was created" bug). Also must NOT depend on rootPath (separate effect):
  // setting a project's working folder must not wipe the conversation either.
  const prevProjectIdRef = useRef<string | null>(null);
  useEffect(() => {
    const prev = prevProjectIdRef.current;
    const next = currentProject?.metadata.id ?? null;
    prevProjectIdRef.current = next;
    // null→new (creation) or no real change: keep the freshly-populated chat.
    if (prev === null || prev === next) return;
    setTaskDraft("");
    setTaskCategory(null);
    setTaskBugDescription("");
    setNoteDraft("");
    setLaunchMessage(null);
    // Reset the planner chat/goal/web-mode so a different project never shows the previous
    // project's conversation (and a steer can't fire with stale context). The design +
    // websearch feeds are already per-project (their own effect / the agent console).
    setPlannerMessages([]);
    setPlannerGoal(null);
    setPlannerWebMode("auto");
    // B15b (reviewer F1): clear the durable transcript SYNCHRONOUSLY on switch, so
    // the new project never briefly renders the previous project's conversation
    // while its own read_activity_chat round-trips.
    setDurableChat([]);
    // B2 F2: a captured cloud-orchestrator id belongs to the project we're leaving.
    setCloudOrchestratorLaunchedId(null);
    // Phase B twinning (reviewer F1): the work-selection store is a global singleton, so a
    // stale agent/task selection from the project we're leaving would otherwise highlight a
    // card / focus an agent in the new project. Wipe it on a real project switch.
    clearSelection();
  }, [currentProject?.metadata.id, clearSelection]);

  // Prefill the root editor with the project's current root (#6) so "Set root" edits the
  // existing value instead of starting blank. Tracks rootPath too, so after you set a
  // folder the editor shows the saved value — WITHOUT touching the chat (separate effect).
  // Finding 5: a draft persisted before the idle auto-lock unmount wins over the
  // saved rootPath, so an unsaved edit survives the lock. With no persisted draft
  // this restores the saved value exactly as before. Persistence is driven by the
  // input's onChange (user edits only), NOT a mirror effect — so this restore-only
  // effect cannot clobber the stored draft via effect-ordering.
  useEffect(() => {
    const persisted = readPersistedProjectRootDraft(currentProject?.metadata.id);
    setRootDraft(persisted ?? (currentProject?.metadata.rootPath ?? ""));
  }, [currentProject?.metadata.id, currentProject?.metadata.rootPath]);

  useEffect(() => {
    const reportAgentSyncError = (e: unknown) => {
      setAgentSyncError(e instanceof Error ? e.message : "Agent sync failed.");
    };
    let timer: number | null = null;
    let firstTick: number | null = null;
    let cancelled = false;
    viewMountedRef.current = true;
    const tick = async () => {
      // Self-scheduling: re-arm only after the previous poll settles, so a slow
      // invoke can never stack multiple in-flight requests.
      if (cancelled) return;
      if (
        document.visibilityState === "visible" &&
        !agentStateInFlightRef.current
      ) {
        // Phase G: the standalone Agents page was dissolved, so this is now the
        // app's SINGLE agent-live-state poller. It runs whenever the Projects view
        // is active (Board OR Work mode) — both surfaces need live agent state, and
        // it is the sole feeder of the app-wide attention store (Header bell + OS
        // notifications) via loadAgentState -> setFromLiveState. It must NOT be
        // gated on any removed tab, or the bell would go dark.
        agentStateInFlightRef.current = true;
        try {
          await loadAgentState();
        } catch (e) {
          reportAgentSyncError(e);
        } finally {
          agentStateInFlightRef.current = false;
        }
      }
      if (!cancelled) timer = window.setTimeout(() => void tick(), 5_000);
    };
    // Defer the FIRST tick to a macrotask so App's activeViewRef has committed to
    // "projects" before the next global attention tick fires. Without this defer,
    // switching INTO Projects could briefly have BOTH this board poll's immediate
    // first tick AND the global poller (still reading the lagging activeViewRef) in
    // flight — a double get_agent_live_state. setTimeout(0) guarantees the React
    // commit (and its activeViewRef write) has run, so the global poller's gate
    // (`activeView === "projects"`) stands down first. (MAJOR: single feeder.)
    firstTick = window.setTimeout(() => void tick(), 0);
    return () => {
      cancelled = true;
      viewMountedRef.current = false;
      if (firstTick !== null) window.clearTimeout(firstTick);
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [loadAgentState]);

  // The (id, rootPath) pairs the Censor tracker watches, derived from the loaded
  // summaries. Keyed by a stable signature so the tracker only RE-BINDS when the
  // project set or a root actually changes — NOT on every 5s agent poll (which
  // replaces `projects` with a fresh array of the same shape). This is what keeps
  // the event-driven counts from triggering a re-render / re-fetch storm.
  const censorSignature = useMemo(
    () => censorTrackedSignature(projects),
    [projects],
  );
  // Latest summaries for the tracker to read at (re)bind time without making the
  // bind effect depend on the array identity (which churns every poll).
  const projectsRef = useRef<ProjectSummary[]>(projects);
  useEffect(() => {
    projectsRef.current = projects;
  }, [projects]);

  // Event-driven Censor counts. ONE tracker for the board's lifetime: it does an
  // initial count_open sweep and refetches only on censor://findings-updated
  // (NOT a poll). We re-bind it whenever the watched project set/roots change so
  // a new/removed project (or a changed root) is reflected. The tracker owns its
  // single event listener; cleanup on unmount calls stop() -> unlisten, so no
  // listener leaks. Failures degrade to 0 inside the tracker (never throw here).
  const censorTrackerRef = useRef<CensorCountsTracker | null>(null);
  useEffect(() => {
    if (!isTauriRuntime()) return;
    const tracker = new CensorCountsTracker({
      invoke: invokeBackendCommand,
      listen: async (channel, handler) => {
        const { listen } = await import("@tauri-apps/api/event");
        return listen(channel, (event) => handler({ payload: event.payload }));
      },
      onChange: (counts) => setCensorCountByProject(counts),
    });
    censorTrackerRef.current = tracker;
    void tracker.start(
      projectsRef.current.map((p) => ({ id: p.id, rootPath: p.rootPath })),
    );
    return () => {
      tracker.stop();
      if (censorTrackerRef.current === tracker) censorTrackerRef.current = null;
    };
    // censorSignature changes only when the watched set/roots change.
  }, [censorSignature]);

  const projectClaims = useMemo<AgentClaim[]>(() => {
    if (!currentProject) return [];
    return (agentState?.claims ?? []).filter(
      (claim) =>
        claim.projectId === currentProject.metadata.id && isOpenClaim(claim),
    );
  }, [agentState?.claims, currentProject]);

  const claimsByTask = useMemo(() => {
    const grouped: Record<string, AgentClaim[]> = {};
    for (const claim of projectClaims) {
      grouped[claim.taskId] = [...(grouped[claim.taskId] ?? []), claim];
    }
    return grouped;
  }, [projectClaims]);

  const projectAgentEvents = useMemo<AgentEvent[]>(() => {
    if (!currentProject) return [];
    return (agentState?.events ?? [])
      .filter((event) => event.projectId === currentProject.metadata.id)
      .slice(-8)
      .reverse();
  }, [agentState?.events, currentProject]);

  const claimsByProject = useMemo(() => {
    const grouped: Record<string, AgentClaim[]> = {};
    for (const claim of agentState?.claims ?? []) {
      grouped[claim.projectId] = [...(grouped[claim.projectId] ?? []), claim];
    }
    return grouped;
  }, [agentState?.claims]);

  const sessionsByProject = useMemo(() => {
    const grouped: Record<string, AgentSession[]> = {};
    for (const session of agentState?.sessions ?? []) {
      if (!session.currentProjectId || !isRecentProjectSession(session))
        continue;
      grouped[session.currentProjectId] = [
        ...(grouped[session.currentProjectId] ?? []),
        session,
      ];
    }
    return grouped;
  }, [agentState?.sessions]);

  const sessionsByTask = useMemo(() => {
    const grouped: Record<string, AgentSession[]> = {};
    for (const session of agentState?.sessions ?? []) {
      if (
        !currentProject ||
        session.currentProjectId !== currentProject.metadata.id ||
        !session.currentTaskId ||
        !isRecentProjectSession(session)
      ) {
        continue;
      }
      grouped[session.currentTaskId] = [
        ...(grouped[session.currentTaskId] ?? []),
        session,
      ];
    }
    return grouped;
  }, [agentState?.sessions, currentProject]);

  // R7 vision: per-task arrow hue (taskId → the hex of the agent working it) so the dependency
  // arrows of parallel tracks read as distinct colors (same hue as the card's worker badge).
  const arrowColorByTask = useMemo(() => {
    const map: Record<string, number> = {};
    for (const task of currentProject?.state.tasks ?? []) {
      const workers = deriveWorkers(
        claimsByTask[task.id] ?? [],
        sessionsByTask[task.id] ?? [],
      );
      if (workers.length > 0) map[task.id] = agentColorHex(workers[0].agentId);
    }
    return map;
  }, [currentProject, claimsByTask, sessionsByTask]);

  // Active board feed: archived projects LEAVE the active stage board + macro
  // calendar (an archived project is read-only and out of the active workflow).
  // Computed once and fed to BOTH projectsByStage and ProjectCalendar so the two
  // stay consistent. The full `projects` array is kept untouched for the archived
  // list/count below. Paused/done stay on the board — only `archived` is excluded.
  const activeProjects = useMemo(
    () => projects.filter((p) => p.status !== "archived"),
    [projects],
  );

  // The archived projects, surfaced via the overview "Archived (N)" toggle. Most
  // recently updated first so the freshest archive is at the top of the list.
  const archivedProjects = useMemo(
    () =>
      projects
        .filter((p) => p.status === "archived")
        .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt)),
    [projects],
  );

  const projectsByStage = useMemo(() => {
    const grouped: Record<ProjectStageId, ProjectSummary[]> = {
      planned: [],
      launching: [],
      active: [],
      review: [],
      blocked: [],
      verified: [],
    };
    for (const item of activeProjects) {
      grouped[
        projectStage(
          item,
          claimsByProject[item.id] ?? [],
          sessionsByProject[item.id] ?? [],
        )
      ].push(item);
    }
    return grouped;
  }, [activeProjects, claimsByProject, sessionsByProject]);

  // Keep the overview toggle coherent: if the last archived project is
  // unarchived (or none exist), snap back to the board so the user is never left
  // staring at an empty "Archived" pane with no archived projects.
  useEffect(() => {
    if (archivedProjects.length === 0 && overviewTab === "archived") {
      setOverviewTab("board");
    }
  }, [archivedProjects.length, overviewTab]);

  const currentSummary = useMemo(() => {
    if (!currentProject) return null;
    return (
      projects.find((item) => item.id === currentProject.metadata.id) ?? null
    );
  }, [currentProject, projects]);

  const currentStage = useMemo(() => {
    if (!currentSummary) return null;
    return projectStage(
      currentSummary,
      claimsByProject[currentSummary.id] ?? [],
      sessionsByProject[currentSummary.id] ?? [],
    );
  }, [claimsByProject, currentSummary, sessionsByProject]);

  const currentProjectSessions = useMemo(() => {
    if (!currentProject) return [];
    return sessionsByProject[currentProject.metadata.id] ?? [];
  }, [currentProject, sessionsByProject]);

  // B12: the orchestrator is the CREATE-TIME conversation (the planner), not a
  // permanent project agent. Keep it OFF the single-project Work page by default —
  // it shouldn't auto-appear in the agent rail or get auto-selected there (it
  // couldn't have existed "at the beginning"). Re-invoking it is opt-in (the
  // planner). The landing's planner binding still uses currentProjectSessions, so
  // only the Work-mode rail is scoped here.
  const workModeSessions = useMemo(
    () => currentProjectSessions.filter((s) => s.client !== "orchestrator"),
    [currentProjectSessions],
  );

  // The single agent to surface in the header's working-agent line: the
  // freshest session by last-seen heartbeat, if any. Uses the shared
  // freshestSession helper so the header, board card, and panel all agree on
  // WHICH agent represents the project (#4).
  const workingAgent = useMemo<AgentSession | null>(
    () => freshestSession(currentProjectSessions),
    [currentProjectSessions],
  );

  // The live orchestrator session for THIS project (its in-app PTY agent), if any.
  // Drives the planner panel's "live" state + feeds its REAL websearch pages/findings
  // from the agent's activity console (the file-bridge `websearch` events).
  // Bind to the current project's LOCAL orchestrator session. Match on client only — the
  // orchestrator registers itself via MCP with host=null (NOT "app"), so requiring host==="app"
  // here meant the landing never bound to it and the live conversation only showed up in the
  // single-project work view (recognized as a coder). currentProjectSessions is already scoped
  // to this project + recent (active) sessions, so client==="orchestrator" is the right one.
  const orchestratorAgentId = useMemo(
    () =>
      currentProjectSessions.find((s) => s.client === "orchestrator")?.agentId ??
      null,
    [currentProjectSessions],
  );
  // When the chosen orchestrator is a CLOUD CLI (claude/codex), the running session for this
  // project — its terminal is shown in the planner instead of the local Stage. null when the
  // orchestrator is local, or when no cloud session is running yet (pre-launch).
  const cloudOrchestratorAgentId = useMemo(() => {
    if (plannerOrchestratorClient === "orchestrator") return null;
    // B2 F2: prefer the EXACT id captured at launch, validated against this
    // project's live sessions (so it can't point at a stale/other-project agent).
    // Only if that's unavailable fall back to the first session of this CLI — which
    // can collide with a hand-off coder, hence the captured id takes precedence.
    if (
      cloudOrchestratorLaunchedId &&
      currentProjectSessions.some(
        (s) => s.agentId === cloudOrchestratorLaunchedId,
      )
    ) {
      return cloudOrchestratorLaunchedId;
    }
    return (
      currentProjectSessions.find((s) => s.client === plannerOrchestratorClient)
        ?.agentId ?? null
    );
  }, [
    plannerOrchestratorClient,
    currentProjectSessions,
    cloudOrchestratorLaunchedId,
  ]);
  // Phase D: the agent whose activity bridge feeds the planner Stage — the LOCAL orchestrator, or
  // (when the orchestrator is a CLOUD CLI in duplex mode) the cloud agent, whose reader thread
  // writes the SAME bridge file. So cloud + local drive the identical Stage (chat/websearch).
  const plannerActivityAgentId = useMemo(
    () => orchestratorAgentId ?? cloudOrchestratorAgentId,
    [orchestratorAgentId, cloudOrchestratorAgentId],
  );
  // Mark a Planner launch in flight (plannerLaunching) until its session registers — guards
  // the composer against a duplicate launch during the agent-poll lag. A safety timeout
  // clears it if the session never appears (e.g. the orchestrator crashes on start).
  const beginPlannerLaunch = useCallback(() => {
    setPlannerLaunching(true);
    if (plannerLaunchTimerRef.current) clearTimeout(plannerLaunchTimerRef.current);
    plannerLaunchTimerRef.current = setTimeout(() => {
      setPlannerLaunching(false);
      plannerLaunchTimerRef.current = null;
    }, 30000);
  }, []);
  useEffect(() => {
    // Clear the launching flag when EITHER a local or a cloud orchestrator session binds
    // (cloud sessions never set orchestratorAgentId, so watching only it left the composer
    // locked for the full 30s timeout on a cloud launch — review F3).
    if (!orchestratorAgentId && !cloudOrchestratorAgentId) return;
    setPlannerLaunching(false);
    if (plannerLaunchTimerRef.current) {
      clearTimeout(plannerLaunchTimerRef.current);
      plannerLaunchTimerRef.current = null;
    }
  }, [orchestratorAgentId, cloudOrchestratorAgentId]);
  useEffect(
    () => () => {
      if (plannerLaunchTimerRef.current) clearTimeout(plannerLaunchTimerRef.current);
    },
    [],
  );
  // B15b: persist the live orchestrator id per project whenever one is bound — local OR cloud
  // duplex (both write the same durable activity .jsonl), so the chat survives an app restart for
  // either backend.
  useEffect(() => {
    const pid = currentProject?.metadata.id;
    if (!pid || !plannerActivityAgentId) return;
    try {
      localStorage.setItem(orchAgentKey(pid), plannerActivityAgentId);
    } catch {
      /* storage unavailable — non-fatal */
    }
  }, [plannerActivityAgentId, currentProject?.metadata.id]);

  // The id the chat reads from: the LIVE session when present, else the last known
  // orchestrator for this project. The in-memory activity store keeps that agent's
  // transcript after its process ends, so the conversation survives a session death
  // within the app; read_activity_chat (below) covers an app restart.
  // Used ONLY for the durable chat READ (read_activity_chat below) — NOT for the live
  // console. Binding the live console to a persisted (possibly dead / previous-session)
  // id would feed plannerWeb (websearch) + the "real" plannerConvo from a stale agent
  // and surface another project's activity (reviewer max-recall finding). The live
  // console stays on orchestratorAgentId; the disk transcript covers a dead session.
  const effectiveOrchestratorId = useMemo(() => {
    if (plannerActivityAgentId) return plannerActivityAgentId;
    return readPersistedOrchestratorId(currentProject?.metadata.id);
  }, [plannerActivityAgentId, currentProject?.metadata.id]);

  const orchestratorConsole = useAgentConsole(plannerActivityAgentId);
  const plannerWeb = useMemo(
    () => latestWeb(orchestratorConsole.entries),
    [orchestratorConsole.entries],
  );

  // B15b durable fallback: when the in-memory console has NO chat (e.g. after an app
  // restart, the store is empty) but we know this project's orchestrator, read the
  // transcript straight from the on-disk .jsonl so the conversation is reconstructed.
  const [durableChat, setDurableChat] = useState<PlannerMessage[]>([]);
  useEffect(() => {
    let cancelled = false;
    const live = chatMessages(orchestratorConsole.entries);
    if (!effectiveOrchestratorId || live.length > 0) {
      setDurableChat([]);
      return;
    }
    void invokeBackendCommand<{ role: string; text: string }[]>(
      "read_activity_chat",
      { agentId: effectiveOrchestratorId },
    )
      .then((turns) => {
        if (cancelled) return;
        setDurableChat(
          (turns ?? []).map((t) => ({
            role: t.role === "assistant" ? "assistant" : "user",
            text: t.text,
          })),
        );
      })
      .catch(() => {
        if (!cancelled) setDurableChat([]);
      });
    return () => {
      cancelled = true;
    };
  }, [effectiveOrchestratorId, orchestratorConsole.entries]);

  // The planner chat = the REAL conversation from the bridge (the orchestrator's
  // assistant turns + its echoed user steers) PLUS any just-sent user message not yet
  // echoed back (optimistic, deduped by text once the bridge catches up).
  const plannerConvo = useMemo(() => {
    const real = chatMessages(orchestratorConsole.entries);
    // Before the orchestrator's session binds, the bridge has nothing. B15b: if a
    // DURABLE transcript was reconstructed from disk (session ended / app restarted),
    // show it (+ any just-typed optimistic user messages after it); otherwise show the
    // optimistic echo + frontend notices (pick-a-folder / starting…) in send order.
    if (real.length === 0) {
      return durableChat.length > 0
        ? [...durableChat, ...plannerMessages]
        : plannerMessages;
    }
    // Once the bridge has the conversation it is AUTHORITATIVE and chronological (user turns
    // + assistant replies in order). Append ONLY the user messages sent but not yet echoed
    // back — a position watermark on USER turns, so repeated "yes"/"continue" are never
    // dropped, and frontend-only notices can't slip in and push a reply ABOVE the message it
    // answers (that was the "the bubble is above my message" bug). Stale notices are dropped
    // here on purpose: the real conversation supersedes them.
    const echoedUserCount = real.filter((m) => m.role === "user").length;
    const pendingUsers = plannerMessages
      .filter((m) => m.role === "user")
      .slice(echoedUserCount);
    return [...real, ...pendingUsers];
  }, [orchestratorConsole.entries, plannerMessages, durableChat]);

  // B14b: append the live, in-progress assistant reply (token streaming) as the LAST bubble —
  // a SEPARATE slot from the finalized timeline, so it grows token-by-token and is replaced by
  // the real entry when the turn finalizes. Empty tail (no reply streaming) leaves chat as-is.
  const plannerConvoLive = useMemo(() => {
    const tail = orchestratorConsole.streamingChat;
    // Suppress the live tail if the finalized assistant entry already landed (the `chat` entry can
    // arrive one event before `streamingChat` is cleared) — otherwise the bubble shows twice for a
    // frame. Also skip an empty tail.
    const lastIsAssistant =
      plannerConvo.length > 0 &&
      plannerConvo[plannerConvo.length - 1].role === "assistant";
    if (!tail || tail.text.length === 0 || lastIsAssistant) return plannerConvo;
    return [...plannerConvo, { role: "assistant" as const, text: tail.text, streaming: true }];
  }, [plannerConvo, orchestratorConsole.streamingChat]);

  // "Awaiting your reply": the orchestrator spoke last and is live → your turn. Drives the
  // PlannerChat pill so the user knows to respond (esp. after an AskUser question).
  const plannerAwaitingReply =
    !!orchestratorAgentId &&
    plannerConvo.length > 0 &&
    plannerConvo[plannerConvo.length - 1].role === "assistant";

  // Phase B L4/L5: fulfill the orchestrator's design_request directives (run the reused
  // design pipeline) and refresh the Stage Design view when one completes.
  useDesignRequestWatcher(
    orchestratorAgentId,
    currentProject?.metadata.rootPath ?? null,
    () => setPlannerDesignNonce((n) => n + 1),
  );

  // Load the project's most-recent design for the planner's read-only Design tab:
  // registry -> pick the entry at/under the project root -> its thumbnail (a data: URI).
  // Re-runs when the selected project's root changes; the cancel flag drops a stale
  // in-flight load when the selection changes again.
  useEffect(() => {
    const rootPath = currentProject?.metadata.rootPath ?? null;
    if (!rootPath) {
      setPlannerDesign(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const entries = await invokeBackendCommand<DesignProjectEntry[]>(
          "design_registry_list",
        );
        const entry = pickProjectDesign(entries ?? [], rootPath);
        if (!entry) {
          if (!cancelled) setPlannerDesign(null);
          return;
        }
        // The selection changed while the registry was loading — drop this stale load
        // before firing a second (thumbnail) IPC for a project we no longer show.
        if (cancelled) return;
        let thumbnailUri: string | null = null;
        try {
          thumbnailUri = await invokeBackendCommand<string | null>(
            "design_read_thumbnail",
            { workingFolderPath: entry.workingFolderPath },
          );
        } catch {
          thumbnailUri = null;
        }
        if (!cancelled) {
          setPlannerDesign({
            name: entry.name,
            version: null,
            ago: entry.updatedAt ? entry.updatedAt.slice(0, 10) : null,
            thumbnailUri,
          });
        }
      } catch {
        if (!cancelled) setPlannerDesign(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [currentProject?.metadata.rootPath, plannerDesignNonce]);

  useEffect(() => {
    if (!agentState) return;
    const signature = agentStateSignature(agentState);
    if (lastAgentRefreshRef.current === signature) return;
    const events = agentState.events ?? [];
    const hasAgentActivity =
      events.length > 0 ||
      (agentState.claims ?? []).length > 0 ||
      (agentState.sessions ?? []).length > 0;
    if (!hasAgentActivity) return;
    if (busyRef.current) {
      pendingAgentRefreshRef.current = {
        signature,
        state: agentState,
      };
      return;
    }
    void refreshProjectsFromAgentState(signature, agentState).catch((e) =>
      setAgentSyncError(
        e instanceof Error ? e.message : "Agent refresh failed.",
      ),
    );
  }, [agentState, refreshProjectsFromAgentState]);

  const drainPendingAgentRefresh = useCallback(() => {
    const pending = pendingAgentRefreshRef.current;
    if (!pending) return;
    pendingAgentRefreshRef.current = null;
    void refreshProjectsFromAgentState(pending.signature, pending.state).catch(
      (e) =>
        setAgentSyncError(
          e instanceof Error ? e.message : "Agent refresh failed.",
        ),
    );
  }, [refreshProjectsFromAgentState]);

  // useCallback so the 10s polling effect captures the current implementation
  // (loadProjects/loadProject are themselves stable) instead of a stale copy
  // closed over by the first render.
  const reloadSelectedProject = useCallback(async () => {
    const projectId = selectedIdRef.current;
    if (!projectId) return;
    await loadProjects();
    await loadProject(projectId);
  }, [loadProjects, loadProject]);

  const reloadSelectedProjectSafe = useCallback(async () => {
    try {
      await reloadSelectedProject();
    } catch (e) {
      setError(
        e instanceof Error ? e.message : "Project could not be reloaded.",
      );
    }
  }, [reloadSelectedProject]);

  useEffect(() => {
    let timer: number | null = null;
    let cancelled = false;
    const tick = async () => {
      if (cancelled) return;
      // Only poll the project file when visible, not busy with a mutation, and
      // no previous reload is still running. Self-scheduling prevents stacking.
      if (
        document.visibilityState === "visible" &&
        // Phase G: sole project-file poll (AgentsView removed). Runs whenever the
        // Projects view is active so the board/Work-mode detail stays fresh.
        !busyRef.current &&
        !reloadProjectInFlightRef.current
      ) {
        reloadProjectInFlightRef.current = true;
        try {
          await reloadSelectedProject();
        } catch (e) {
          setAgentSyncError(
            e instanceof Error ? e.message : "Project file refresh failed.",
          );
        } finally {
          reloadProjectInFlightRef.current = false;
        }
      }
      if (!cancelled) timer = window.setTimeout(() => void tick(), 10_000);
    };
    timer = window.setTimeout(() => void tick(), 10_000);
    return () => {
      cancelled = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [reloadSelectedProject]);

  // Shared "changed on disk" recovery used by every mutation path (#9): when a
  // backend write conflicts because the project file moved underneath us, reload
  // the latest state instead of leaving the user with a raw conflict message.
  const recoverFromConflict = useCallback(
    async (message: string) => {
      if (!message.toLowerCase().includes("changed on disk")) return message;
      const projectId = selectedIdRef.current;
      if (!projectId) return message;
      try {
        await loadProjects();
        await loadProject(projectId);
        return `${message} Latest project state reloaded.`;
      } catch {
        return `${message} Reload failed; use Reload.`;
      }
    },
    [loadProject, loadProjects],
  );

  const runMutation = async (mutation: () => Promise<ProjectDetail>) => {
    if (busyRef.current) return null;
    busyRef.current = true;
    setIsBusy(true);
    setError(null);
    try {
      const detail = await mutation();
      setProject(detail);
      selectedIdRef.current = detail.metadata.id;
      setSelectedId(detail.metadata.id);
      await loadProjects();
      return detail;
    } catch (e) {
      const message = e instanceof Error ? e.message : "Project update failed.";
      setError(await recoverFromConflict(message));
      return null;
    } finally {
      busyRef.current = false;
      setIsBusy(false);
      drainPendingAgentRefresh();
    }
  };

  // R1: native folder picker for the project working root (mirrors SkillsView/DesignView).
  const pickProjectFolder = async () => {
    let picked: string | null = null;
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const result = await open({
        directory: true,
        multiple: false,
        title: "Choose the project working folder",
      });
      if (typeof result === "string" && result.trim()) picked = result.trim();
    } catch {
      // Dialog unavailable or dismissed — no-op (no feedback needed for a cancel).
    }
    if (picked !== null) setNewProjectRootDraft(picked);
  };

  const createProject = async () => {
    const title = titleDraft.trim();
    if (!title || busyRef.current) return;
    const detail = await runMutation(() =>
      invokeBackendCommand<ProjectDetail>("create_project", {
        // R1: pass the chosen working folder so tools/agents key off it (not the app dir).
        input: {
          title,
          status: "active",
          rootPath: newProjectRootDraft.trim() || null,
        },
      }),
    );
    if (detail) {
      setTitleDraft("");
      setNewProjectRootDraft("");
    }
  };

  // S5: load the configured default main-coder CLI once (the task-card launches use it).
  useEffect(() => {
    void invokeBackendCommand<"claude" | "codex">("get_main_coder_client")
      .then((c) => {
        if (c === "claude" || c === "codex") setMainCoderClient(c);
      })
      .catch(() => {
        /* leave the "codex" default on any error */
      });
  }, []);

  // Board: clone a GitHub repo and register it as a project, then dive into Work
  // mode on it. The token is NEVER passed here — the backend injects it via
  // GIT_ASKPASS and rebuilds a credential-free clone URL.
  //
  // FIX 1: the reentrancy gate is the SYNCHRONOUS `cloneBusyRef` (via
  // runGitActionGuarded), NOT the async `cloneBusy` state — two fast clicks both
  // saw the stale `false` and fired two concurrent project_git_clone calls.
  // `cloneBusy` is still set for the disabled-button / "Cloning…" UI feedback.
  // FIX 8: the clone awaits a backend op bounded at 600s; if the view unmounts
  // mid-clone we must NOT setState on an unmounted component, so every post-await
  // state write is gated on `viewMountedRef.current`.
  const cloneProject = async () => {
    const url = cloneUrlDraft.trim();
    if (!url) return;
    if (!isLikelyGithubRepoUrl(url)) {
      setCloneError(
        "Enter a valid GitHub repository URL (https://github.com/owner/repo).",
      );
      return;
    }
    await runGitActionGuarded(cloneBusyRef, async () => {
      setCloneBusy(true);
      setCloneError(null);
      try {
        const call = cloneProjectCall(url);
        const detail = await invokeBackendCommand<ProjectDetail>(
          call.command,
          call.args,
        );
        await loadProjects();
        if (!viewMountedRef.current) return;
        setCloneUrlDraft("");
        setCloneOpen(false);
        // Navigate to the freshly cloned project's Work mode.
        enterWorkMode(detail.metadata.id);
      } catch (e) {
        if (!viewMountedRef.current) return;
        setCloneError(e instanceof Error ? e.message : "git clone failed.");
      } finally {
        if (viewMountedRef.current) setCloneBusy(false);
      }
    });
  };

  const createTask = async () => {
    if (isArchived) return; // archived project is read-only.
    // Category is mandatory on create (the button is disabled until picked, but
    // guard here too). A bug card carries its description as the P2 Oracle query.
    if (
      !currentProject ||
      !taskDraft.trim() ||
      !taskCategory ||
      busyRef.current
    )
      return;
    const title = taskDraft.trim();
    const description = taskCategory === "bug" ? taskBugDescription.trim() : "";
    const projectId = currentProject.metadata.id;
    // Task ids present BEFORE the create, so we can identify the new card in the
    // returned detail without relying on duplicate-prone title matching.
    const priorTaskIds = new Set(currentProject.state.tasks.map((t) => t.id));
    const detail = await runMutation(() =>
      invokeBackendCommand<ProjectDetail>("create_project_task", {
        projectId,
        task: {
          title,
          status: "todo",
          category: taskCategory,
          ...(description ? { description } : {}),
          expectedRevision: currentProject.revision,
        },
      }),
    );
    if (detail) {
      setTaskDraft("");
      setTaskCategory(null);
      setTaskBugDescription("");
      // P2: seed the new card's Oracle suspect files (top-K relevant files) as a
      // SEPARATE best-effort step. The card already shows immediately; the suspect
      // list/note lands on the next reload when localization returns. Runs for
      // EVERY category. Tauri-only (the browser harness has no backend); failure is
      // silent — the card is fine without suspects (fail-closed in the backend).
      if (isTauriRuntime()) {
        const newTask = detail.state.tasks.find((t) => !priorTaskIds.has(t.id));
        if (newTask) {
          // Build the query from the PERSISTED card (title + description), not the
          // raw draft: the backend trims/caps both (description at 4000 chars via
          // clean_description), so the retrieval query matches exactly what was
          // stored on the card.
          const persistedDescription = newTask.description ?? "";
          const query = persistedDescription
            ? `${newTask.title}\n${persistedDescription}`
            : newTask.title;
          void invokeBackendCommand<ProjectDetail>("localize_card_suspects", {
            projectId,
            taskId: newTask.id,
            query,
          })
            .catch(() => {
              // Best-effort: a failed localization must never disrupt the card,
              // and must not log the query text.
            })
            // Refresh the board in EVERY outcome (success OR failure) so the seeded
            // suspects / honest failure note appear and the board never stays stale.
            .finally(() => {
              void reloadSelectedProjectSafe();
            });
        }
      }
    }
  };

  const moveTask = async (task: ProjectTask, status: ColumnId) => {
    if (isArchived) return; // archived project is read-only.
    if (!currentProject || task.status === status || busyRef.current) return;
    await runMutation(() =>
      invokeBackendCommand<ProjectDetail>("move_project_task", {
        projectId: currentProject.metadata.id,
        taskId: task.id,
        status,
        expectedRevision: currentProject.revision,
      }),
    );
  };

  // B17: drag-and-drop across columns (logic generated by the local Qwen model). A card is
  // only draggable when it's movable (see `moveDisabled` on the card); dropping on a column
  // routes through the SAME `moveTask` (which re-guards archived/busy/same-status).
  const dragTaskRef = useRef<ProjectTask | null>(null);
  const handleCardDragStart = (task: ProjectTask) => {
    dragTaskRef.current = task;
  };
  const handleColumnDrop = (status: ColumnId) => {
    const task = dragTaskRef.current;
    dragTaskRef.current = null;
    // B-F2: "done" is verifier-gated — a drag must never move a task INTO done (mirrors the
    // Move menu, which omits done as a target). Done-SOURCE cards are non-draggable (see
    // `moveDisabled` below), so guarding the target here closes both directions.
    if (task && status !== "done" && task.status !== status) {
      void moveTask(task, status);
    }
  };

  const appendNote = async () => {
    if (isArchived) return; // archived project is read-only.
    if (!currentProject || !noteDraft.trim() || busyRef.current) return;
    const text = noteDraft.trim();
    const detail = await runMutation(() =>
      invokeBackendCommand<ProjectDetail>("append_project_note", {
        projectId: currentProject.metadata.id,
        note: {
          text,
          source: "user",
          expectedRevision: currentProject.revision,
        },
      }),
    );
    if (detail) setNoteDraft("");
  };

  const updateProjectStatus = async (
    status: Exclude<ProjectStatus, "done">,
  ) => {
    if (
      !currentProject ||
      busyRef.current ||
      currentProject.metadata.status === status
    )
      return;
    await runMutation(() =>
      invokeBackendCommand<ProjectDetail>("update_project_metadata", {
        projectId: currentProject.metadata.id,
        patch: {
          status,
          expectedRevision: currentProject.revision,
        },
      }),
    );
  };

  // B11: permanently delete the current project. Unlike runMutation (which
  // expects a ProjectDetail back and re-selects it), delete returns nothing and
  // the project no longer exists — so we clear the selection and reload the list.
  // Idempotent backend: a missing project is a success.
  const deleteProject = async () => {
    if (!currentProject || busyRef.current) return;
    const title = currentProject.metadata.title || currentProject.metadata.id;
    // Warn (do not hard-block — the user's explicit choice wins) if agents are
    // still live on this project: deleting the record orphans them and their MCP
    // writes will start failing with "Project not found." Surface that so the user
    // can stop them first if they meant to.
    const liveCount = currentProjectSessions.length;
    const liveWarning =
      liveCount > 0
        ? `\n\n⚠️ ${liveCount} agent session${liveCount > 1 ? "s are" : " is"} still live on this project. Deleting it will orphan ${liveCount > 1 ? "them" : "it"} — stop ${liveCount > 1 ? "them" : "it"} first if you didn't mean to.`
        : "";
    if (
      !window.confirm(
        `Delete project “${title}” permanently?\n\nThis removes the project record from disk and cannot be undone. The working folder, git history, and any cloud resources are left untouched.${liveWarning}`,
      )
    )
      return;
    busyRef.current = true;
    setIsBusy(true);
    setError(null);
    try {
      await invokeBackendCommand<void>("delete_project", {
        projectId: currentProject.metadata.id,
      });
      // B15b hygiene: drop the remembered orchestrator id for the gone project.
      try {
        localStorage.removeItem(orchAgentKey(currentProject.metadata.id));
      } catch {
        /* non-fatal */
      }
      selectedIdRef.current = null;
      setSelectedId(null);
      setProject(null);
      // Reviewer max-recall: if we deleted the project we were working on, leave Work
      // mode in the same tick — don't rely on the coherence effect re-firing a render
      // later (closes the workMode=true / currentProject=null window).
      setWorkMode(false);
      await loadProjects();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not delete project.");
    } finally {
      busyRef.current = false;
      setIsBusy(false);
      // Flush any agent-state updates parked while busyRef was held (matches every
      // other mutation path — without this the parked update is stuck until the
      // next poll, bleeding stale liveness into sessionsByProject).
      drainPendingAgentRefresh();
    }
  };

  // Set (or clear) the project root from the UI (#6). This is the only remaining
  // editor for the agent root after the GitHub panel was removed: without it,
  // agents fall back to a default root with no recovery path. Routes through
  // runMutation so it shares the same conflict-recovery + reload behaviour.
  const setProjectRoot = async () => {
    if (isArchived) return; // archived project is read-only.
    if (!currentProject || busyRef.current) return;
    const trimmed = rootDraft.trim();
    const nextRoot = trimmed === "" ? null : trimmed;
    if (nextRoot === (currentProject.metadata.rootPath ?? null)) return;
    const savedProjectId = currentProject.metadata.id;
    const saved = await runMutation(() =>
      invokeBackendCommand<ProjectDetail>("update_project_metadata", {
        projectId: savedProjectId,
        patch: {
          rootPath: nextRoot,
          expectedRevision: currentProject.revision,
        },
      }),
    );
    // Finding 5: drop the persisted draft ONLY on a successful save (runMutation
    // returns null on failure) — otherwise a failed save would silently discard
    // the user's unsaved edit, which is the very thing we're trying to protect.
    if (saved !== null) {
      clearPersistedProjectRootDraft(savedProjectId);
    }
  };

  const copyAgentPrompt = async (role: SpawnRole, taskId?: string) => {
    if (isArchived) return; // archived project is read-only.
    if (!currentProject || busyRef.current) return;
    if (!canLaunchProjectAgents(currentProject)) {
      setError(projectLaunchTitle(currentProject));
      return;
    }
    const agentId = `${role}-${Date.now()}`;
    busyRef.current = true;
    setIsBusy(true);
    setError(null);
    try {
      const result = await invokeBackendCommand<ProjectAgentLaunchResult>(
        "prepare_project_agent_prompt",
        {
          input: {
            projectId: currentProject.metadata.id,
            role,
            // R4/B20: the manual copy-prompt was hardcoded to "powershell" (Windows-only
            // shell), which surfaced "PowerShell" on macOS. Use a platform-neutral CLI
            // client — "claude" works on mac + Windows; the user pastes the prompt into
            // their own terminal regardless of OS.
            client: "claude",
            agentId,
            taskId: taskId ?? null,
          },
        },
      );
      await navigator.clipboard.writeText(result.prompt);
      setLaunchMessage(
        `${role} prompt copied with app-issued MCP launch token.`,
      );
      await loadAgentState();
      await loadProjects();
    } catch (e) {
      const message =
        e instanceof Error
          ? e.message
          : "Clipboard is unavailable. Launch a terminal from the app or copy the MCP command manually.";
      // Route through the same disk-conflict recovery as runMutation (#9) so a
      // stale-revision write reloads the latest state instead of dead-ending on
      // a raw error.
      setError(await recoverFromConflict(message));
    } finally {
      busyRef.current = false;
      setIsBusy(false);
      drainPendingAgentRefresh();
    }
  };

  const launchAgent = async (
    role: SpawnRole,
    client: "codex" | "claude",
    taskId?: string,
  ) => {
    if (isArchived) return; // archived project is read-only.
    if (!currentProject || busyRef.current) return;
    if (!canLaunchProjectAgents(currentProject)) {
      setError(projectLaunchTitle(currentProject));
      return;
    }
    busyRef.current = true;
    setIsBusy(true);
    setError(null);
    setLaunchMessage(null);
    try {
      const result = await invokeBackendCommand<ProjectAgentLaunchResult>(
        "launch_project_agent_terminal",
        {
          input: {
            projectId: currentProject.metadata.id,
            role,
            client,
            agentId: `${role}-${Date.now()}`,
            taskId: taskId ?? null,
            // R4/B19: the task-card quick-launch uses the IN-APP PTY terminal — reliable
            // on macOS AND Windows (the external osascript→Terminal.app path silently
            // fails on mac → no CLI appears). External CLIs remain available via the
            // Spawn panel, which passes its own host choice.
            host: "app",
          },
        },
      );
      setLaunchMessage(
        `${result.client} launched in the in-app terminal at ${result.rootPath}. MCP config and prompt attached.`,
      );
      // B-F3: host:"app" runs the agent in the IN-APP PTY, whose viewer only renders in the
      // workspace — enter work mode so the user actually SEES the terminal (otherwise the
      // launch is invisible from the board, the very symptom R4 set out to fix).
      enterWorkMode(currentProject.metadata.id);
      await loadAgentState();
      await loadProjects();
    } catch (e) {
      const message =
        e instanceof Error
          ? e.message
          : "Agent terminal could not be launched.";
      // Same disk-conflict recovery as runMutation (#9).
      setError(await recoverFromConflict(message));
    } finally {
      busyRef.current = false;
      setIsBusy(false);
      drainPendingAgentRefresh();
    }
  };

  const runSavedWorkflow = async (workflow: SavedWorkflow) => {
    if (isArchived) return; // archived project is read-only.
    if (!currentProject || busyRef.current) return;
    if (!canLaunchProjectAgents(currentProject)) {
      setWorkflowError(projectLaunchTitle(currentProject));
      return;
    }
    let input;
    try {
      input = buildWorkflowLaunchInput(
        currentProject.metadata.id,
        workflow.name,
        workflowArgs[workflow.name] ?? "",
        savedWorkflows,
      );
    } catch (e) {
      setWorkflowError(
        e instanceof Error ? e.message : "Workflow launch failed.",
      );
      return;
    }
    busyRef.current = true;
    setIsBusy(true);
    setWorkflowBusyName(workflow.name);
    setWorkflowError(null);
    setLaunchMessage(null);
    try {
      const result = await invokeBackendCommand<ProjectAgentLaunchResult>(
        "launch_project_agent_terminal",
        { input },
      );
      setLaunchMessage(
        `Claude workflow /${workflow.name} launched at ${result.rootPath}.`,
      );
      await loadAgentState();
      await loadProjects();
    } catch (e) {
      const message =
        e instanceof Error
          ? e.message
          : "Saved workflow could not be launched.";
      setWorkflowError(await recoverFromConflict(message));
    } finally {
      setWorkflowBusyName(null);
      busyRef.current = false;
      setIsBusy(false);
      drainPendingAgentRefresh();
    }
  };

  // Copy a reconnect prompt for a stalled agent. Pure client-side text (project
  // + session data already in React state) — it invents no backend call and
  // reveals no hidden token, mirroring the Agents control-room recovery copy.
  const copyAgentRecovery = async (session: AgentSession) => {
    const projectId =
      session.currentProjectId ?? currentProject?.metadata.id ?? "unknown";
    const rootPath =
      currentProject?.metadata.rootPath ??
      "use the project root shown in Aspis Management";
    const recovery = [
      "ASPIS AGENT RECOVERY",
      "",
      `Agent id: ${session.agentId}`,
      `Role: ${session.role}`,
      `Status: ${session.status}`,
      `CLI: ${session.client ?? "unknown"}`,
      `Project id: ${projectId}`,
      `Task id: ${session.currentTaskId ?? "project-level"}`,
      `Expected root: ${rootPath}`,
      "",
      "Do this in the existing CLI only if it is still alive:",
      "1. Check that the terminal cwd is the expected project root.",
      "2. If you still have a session_token, call agent_heartbeat with the current project/task.",
      "3. Call project_get, then oracle_context for context, then update the claim or status.",
      "4. If the session_token is gone, do not invent one. Relaunch the agent from Aspis Management.",
    ].join("\n");
    try {
      await navigator.clipboard.writeText(recovery);
      setLaunchMessage(`Recovery steps copied for ${session.agentId}.`);
    } catch {
      setError(
        "Clipboard is unavailable. Copy the recovery steps from the agent panel manually.",
      );
    }
  };

  // Focus the dedicated console window for a live agent. The backend finds the
  // window by its unique title marker and brings it to the foreground.
  const openAgentCli = async (agentId: string) => {
    setError(null);
    try {
      await invokeBackendCommand("focus_agent_terminal", { agentId });
      setLaunchMessage(`Focused terminal for ${agentId}.`);
    } catch (e) {
      setError(
        e instanceof Error
          ? e.message
          : "Agent terminal window not found — it may have been closed.",
      );
    }
  };

  // Stop a live agent: kill its process tree and mark the session closed. The
  // backend returns the refreshed live state so the panel updates immediately.
  const stopAgent = async (agentId: string) => {
    setError(null);
    try {
      const liveState = await invokeBackendCommand<AgentLiveState>(
        "stop_agent",
        { agentId },
      );
      // Apply through the shared helper so the applied signature is updated too
      // (#1): otherwise the next 5s poll sees a different signature and
      // re-applies an OLDER on-disk snapshot, resurrecting the stopped agent.
      applyAgentState(liveState);
      setLaunchMessage(`Stopped ${agentId}.`);
      await loadProjects();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Agent could not be stopped.");
    }
  };

  const refreshLiveStatus = async () => {
    if (!currentProject || busyRef.current) return;
    const requestedId = currentProject.metadata.id;
    busyRef.current = true;
    setIsBusy(true);
    setError(null);
    try {
      const liveStatus = await invokeBackendCommand<ProjectLiveStatus>(
        "refresh_project_live_status",
        {
          projectId: requestedId,
        },
      );
      setProject((current) =>
        current?.metadata.id === requestedId
          ? { ...current, liveStatus }
          : current,
      );
    } catch (e) {
      setError(
        e instanceof Error ? e.message : "Live resource refresh failed.",
      );
    } finally {
      busyRef.current = false;
      setIsBusy(false);
      drainPendingAgentRefresh();
    }
  };

  // Card click → enter Work mode: select the project AND flip workMode on. Stable
  // (useCallback) so the memoized ProjectsBoard's onSelect prop stays referentially
  // stable across the 5s poll re-renders.
  const enterWorkMode = useCallback((projectId: string) => {
    setSelectedId(projectId);
    selectedIdRef.current = projectId;
    // Bridge the work-mode-coherence guard across the first render (see ref decl):
    // the detail load hasn't set loadingProjectId yet, so without this the guard
    // would see currentProject===null + loadingProjectId!==id and bounce a bell
    // deep-link straight back to the Board.
    pendingWorkEntryIdRef.current = projectId;
    setGitActionMessage(null);
    setGitActionError(false);
    setWorkMode(true);
  }, []);

  // Calendar milestone click → SELECT/highlight the project on the board WITHOUT
  // diving into Work mode (the plan said "select is fine"; forcing full Work mode
  // on a calendar click is a spec deviation). Mirrors enterWorkMode's selection
  // bookkeeping (state + ref) minus setWorkMode(true). Selecting an archived /
  // since-deleted project id can never crash: setSelectedId just stores the id and
  // the detail-load + work-mode-coherence effects already handle a missing detail
  // (the board renders the highlight only when the id is present in a stage).
  const selectProjectOnly = useCallback((projectId: string) => {
    setSelectedId(projectId);
    selectedIdRef.current = projectId;
  }, []);

  // `← Board`: leave Work mode but KEEP the selection.
  const exitWorkMode = useCallback(() => {
    setWorkMode(false);
    setGitActionMessage(null);
    setGitActionError(false);
  }, []);

  // Keep work mode coherent with the selection: if the selected project's detail
  // is genuinely gone (deleted/archived/selection cleared) while work mode is on,
  // the shell would silently fall back to the board with workMode still true.
  // Exit work mode so the two stay consistent. exitWorkMode flips workMode→false,
  // so this effect re-runs once and then no-ops (no loop).
  //
  // BLOCKER (Phase G): we must NOT exit while the selected project is still
  // LOADING. A bell deep-link from another view runs enterWorkMode(id) before the
  // `get_project` fetch resolves, so currentProject is briefly null. The old guard
  // `workMode && !currentProject` fired immediately and bounced every deep-link
  // back to the Board. shouldExitWorkMode holds Work mode while
  // loadingProjectId === selectedId (load in flight) and exits only once the load
  // settles empty (missing/archived id) or stays empty with no load pending.
  useEffect(() => {
    // Clear the synchronous bridge once the real loading state has caught up
    // (loadingProjectId now reflects this id) or the project has resolved, or the
    // selection genuinely moved on — from then on loadingProjectId /
    // currentProject are the source of truth and the genuine missing/archived
    // fallback must work. CRUCIAL: the "selection moved" check compares against
    // selectedIdRef.current (the SYNCHRONOUS selection), NOT the stale `selectedId`
    // state — on a bell deep-link from project A to B, enterWorkMode(B) sets the
    // ref to B this same tick but `selectedId` state is still A, and comparing the
    // bridge (B) to the stale state (A) would clear the bridge immediately and
    // bounce the deep-link back to the Board (see shouldClearWorkEntryBridge).
    if (
      shouldClearWorkEntryBridge({
        pendingWorkEntryId: pendingWorkEntryIdRef.current,
        currentProjectId: currentProject?.metadata.id ?? null,
        loadingProjectId,
        currentSelectedId: selectedIdRef.current,
      })
    ) {
      pendingWorkEntryIdRef.current = null;
    }
    if (
      shouldExitWorkMode({
        workMode,
        hasCurrentProject: currentProject !== null,
        selectedId,
        loadingProjectId,
        pendingWorkEntryId: pendingWorkEntryIdRef.current,
      })
    ) {
      exitWorkMode();
    }
  }, [workMode, currentProject, selectedId, loadingProjectId, exitWorkMode]);

  // DEEP-LINK consume (Phase G): the Header attention bell deep-links a needs-you
  // agent straight into its project's Work mode via `projects#work:<projectId>`.
  // parseViewTarget splits that into the `work:<id>` pending tab; parseWorkTab maps
  // it to {selectedId, workMode}. Consume here exactly like ProvidersView /
  // SettingsView do, depending on `pendingTab` (not just the stable callback) so a
  // request that arrives while Projects is ALREADY the active view still re-runs
  // (otherwise the click is dead — the M1 pattern). A bell click with no resolvable
  // project never reaches here (the Header falls back to `projects` with no tab), so
  // an empty/non-work token is simply ignored. enterWorkMode does the same selection
  // bookkeeping (state + ref) used by a card click, and the work-mode-coherence
  // effect above safely exits if the deep-linked project id no longer loads.
  useEffect(() => {
    const requested = consumePendingTab();
    const selection = parseWorkTab(requested);
    if (selection) {
      enterWorkMode(selection.selectedId);
    }
  }, [consumePendingTab, pendingTab, enterWorkMode]);

  // Rail launch (app/external): thread the SpawnPanel-built input into the same
  // launch_project_agent_terminal command used everywhere (host + advisory model).
  const launchFromSpawnPanel = async (
    input: SpawnLaunchInput,
  ): Promise<string | null> => {
    if (isArchived) return null; // archived project is read-only.
    if (busyRef.current) return null;
    busyRef.current = true;
    setIsBusy(true);
    setError(null);
    setLaunchMessage(null);
    // B2 F2: generate the launched agentId HERE (once) so the caller can capture
    // the EXACT id — binding the cloud orchestrator's terminal by a captured id is
    // robust, vs `.find(by client)` which collides with a hand-off coder of the
    // same CLI.
    const launchedAgentId = `${input.role}-${Date.now()}`;
    try {
      const result = await invokeBackendCommand<ProjectAgentLaunchResult>(
        "launch_project_agent_terminal",
        {
          input: {
            projectId: input.projectId,
            role: input.role,
            client: input.client,
            agentId: launchedAgentId,
            taskId: input.taskId,
            host: input.host,
            model: input.model,
            // Phase H: only the Censor "Run final review" launch sets this; a
            // normal SpawnPanel launch leaves it undefined, so the backend's
            // lenient default keeps the verifier prompt unchanged.
            censorReview: input.censorReview,
            // 3b: "Plan first" rides through ONLY for the orchestrator client
            // (buildLaunchInput already gates it). Undefined for every other
            // launch, so the backend omits DEVBOULE_PLAN_FIRST and the env is
            // byte-identical to a pre-3b launch.
            planFirst: input.planFirst,
            // Phase 6: the per-launch language-persona override (absent ⇒ backend auto-detects).
            // buildLaunchInput already emits undefined when the user picked no override.
            languageOverride: input.languageOverride,
            // Orchestrator composer: the typed goal + auto-create toggle (absent for every other
            // launch, so the backend omits DEVBOULE_GOAL/DEVBOULE_AUTO_CREATE — byte-identical).
            initialGoal: input.initialGoal,
            autoCreate: input.autoCreate,
            // Phase D: run a cloud orchestrator as a piped duplex child (Stage, not terminal).
            cloudDuplex: input.cloudDuplex,
          },
        },
      );
      setLaunchMessage(
        input.host === "app"
          ? `${result.role} launched in app at ${result.rootPath}.`
          : `${result.client} launched at ${result.rootPath}. MCP config and prompt attached.`,
      );
      await loadAgentState();
      await loadProjects();
      return launchedAgentId;
    } catch (e) {
      setError(
        await recoverFromConflict(
          e instanceof Error
            ? e.message
            : "Agent terminal could not be launched.",
        ),
      );
      return null;
    } finally {
      busyRef.current = false;
      setIsBusy(false);
      drainPendingAgentRefresh();
    }
  };

  // NEW-PROJECT flow: the landing opens EMPTY; the user's first message CREATES the project
  // (with the chosen Folder) and launches the Planner on it (plan-first). A project IS a plan:
  // creating one === starting the planning conversation. Chains the EXISTING create_project +
  // orchestrator launch — no new backend. The typed goal reaches the Planner via initialGoal
  // (DEVBOULE_GOAL); the drafted plan flows through the existing plan-approval surface.
  const startNewProject = async (
    goal: string,
    coderId: string,
    autoCreate: boolean,
  ) => {
    if (busyRef.current || orchestratorPlanRef.current) return;
    const folder = newProjectRootDraft.trim();
    if (!folder) {
      // The Folder is required (the project depends on a working tree). Say so IN the
      // conversation — the folder picker is right here in the create bar.
      setPlannerMessages((prev) => [
        ...prev,
        {
          role: "assistant",
          text: "Pick a folder for this project first (use an existing one, or create a new one), then send your goal again.",
        },
      ]);
      return;
    }
    // Clean slate: the conversation starts with exactly this goal (drops any stale notices
    // like "pick a folder first" from earlier attempts, so the bridge echo lines up cleanly).
    setPlannerMessages([{ role: "user", text: goal }]);
    orchestratorPlanRef.current = true;
    try {
      // The NAME is decided DURING the conversation (this is a vibe-coding app: you talk to
      // the Planner to shape the project/plan/tasks). Start with a neutral placeholder — the
      // first message is a goal, NOT a title — and let the conversation rename it later.
      const detail = await runMutation(() =>
        invokeBackendCommand<ProjectDetail>("create_project", {
          input: { title: "Untitled plan", status: "active", rootPath: folder },
        }),
      );
      if (!detail) return;
      // runMutation already selected the new project; clear the folder draft and launch the
      // Planner ON the freshly-created project (use the returned detail — currentProject
      // updates asynchronously, so we must not read it here).
      setNewProjectRootDraft("");
      setPlannerGoal(goal);
      // Audit note: record the goal + the chosen build coder (mirrors planWithOrchestrator).
      // The goal itself reaches the Planner via initialGoal (DEVBOULE_GOAL).
      await runMutation(() =>
        invokeBackendCommand<ProjectDetail>("append_project_note", {
          projectId: detail.metadata.id,
          note: {
            text: `Planner goal: ${goal}\n(build with: ${coderId} · auto-create tasks: ${autoCreate ? "on" : "off"})`,
            source: "user",
            expectedRevision: detail.revision,
          },
        }),
      );
      beginPlannerLaunch();
      const launchedId = await launchFromSpawnPanel({
        projectId: detail.metadata.id,
        role: "coder",
        // The chosen orchestrator backend: local Devboule ("orchestrator") or a cloud CLI
        // ("claude"/"codex"). plan-first is a local-orchestrator concept; cloud CLIs drive
        // their own loop, so only gate it on local.
        client: plannerOrchestratorClient,
        taskId: null,
        host: "app",
        model: null,
        // B10: discuss-first. Only bias the local orchestrator plan-first when the
        // user opted into eager mode (autoCreate ON); otherwise it converses until
        // the user clicks "Create plan".
        planFirst: plannerOrchestratorClient === "orchestrator" && autoCreate,
        initialGoal: goal,
        autoCreate,
        // Phase D: a CLOUD orchestrator runs as a duplex child (drives the Stage).
        cloudDuplex: plannerOrchestratorClient !== "orchestrator",
      });
      // B2 F2: capture the cloud orchestrator's exact agentId for binding (Stage in duplex mode).
      if (plannerOrchestratorClient !== "orchestrator") {
        setCloudOrchestratorLaunchedId(launchedId);
      }
    } finally {
      orchestratorPlanRef.current = false;
    }
  };

  // Orchestrator composer "Plan it": record the typed goal as a project note (the orchestrator
  // reads project state), then launch the LOCAL orchestrator in plan-first mode — the SAME path as
  // the SpawnPanel "Plan first" toggle (role "coder" + client "orchestrator" + planFirst). The
  // drafted plan flows through the EXISTING plan-approval surface (bell / PlanApprovalCard); on
  // approval its tasks land on the board. No new backend, no new approval UI.
  const planWithOrchestrator = async (
    goal: string,
    coderId: string,
    autoCreate: boolean,
  ) => {
    if (isArchived) return;
    if (
      !currentProject?.metadata.rootPath ||
      busyRef.current ||
      orchestratorPlanRef.current
    )
      return;
    orchestratorPlanRef.current = true;
    try {
      const note = `Orchestrator goal: ${goal}\n(hand off to: ${coderId} · auto-create tasks: ${autoCreate ? "on" : "off"})`;
      const detail = await runMutation(() =>
        invokeBackendCommand<ProjectDetail>("append_project_note", {
          projectId: currentProject.metadata.id,
          note: {
            text: note,
            source: "user",
            expectedRevision: currentProject.revision,
          },
        }),
      );
      if (!detail) return;
      beginPlannerLaunch();
      const launchedId = await launchFromSpawnPanel({
        projectId: currentProject.metadata.id,
        role: "coder",
        client: plannerOrchestratorClient,
        taskId: null,
        host: "app",
        model: null,
        // B10: discuss-first. Only bias the local orchestrator plan-first when the
        // user opted into eager mode (autoCreate ON); otherwise it converses until
        // the user clicks "Create plan".
        planFirst: plannerOrchestratorClient === "orchestrator" && autoCreate,
        // The typed goal now reaches the planner directly (DEVBOULE_GOAL — the orchestrator runs it
        // headless, plan-first); the note above is kept only as an audit trail. auto-create rides too.
        initialGoal: goal,
        autoCreate,
        // Phase D: a CLOUD orchestrator runs as a duplex child (drives the Stage).
        cloudDuplex: plannerOrchestratorClient !== "orchestrator",
      });
      // B2 F2: capture the cloud orchestrator's exact agentId for binding (Stage in duplex mode).
      if (plannerOrchestratorClient !== "orchestrator") {
        setCloudOrchestratorLaunchedId(launchedId);
      }
    } finally {
      orchestratorPlanRef.current = false;
    }
  };

  // Rail copy-prompt: SpawnPanel selection → prepare_project_agent_prompt
  // (normalizes the advisory model hint identically to the launch path).
  const copyFromSpawnPanel = async (selection: SpawnSelection) => {
    if (isArchived) return; // archived project is read-only.
    if (busyRef.current) return;
    busyRef.current = true;
    setIsBusy(true);
    setError(null);
    try {
      const result = await invokeBackendCommand<ProjectAgentLaunchResult>(
        "prepare_project_agent_prompt",
        {
          input: {
            projectId: selection.projectId,
            role: selection.role,
            client: selection.client,
            agentId: `${selection.role}-${Date.now()}`,
            taskId: selection.taskId || null,
            model: normalizeModelHint(selection.model),
            // Phase 6: forward the language-persona override so the COPIED prompt (preview) matches
            // what a launch would compose. Empty/absent ⇒ null ⇒ backend auto-detects.
            languageOverride: selection.languageOverride || null,
          },
        },
      );
      await navigator.clipboard.writeText(result.prompt);
      setLaunchMessage(
        `${selection.role} prompt copied with app-issued MCP launch token.`,
      );
      await loadAgentState();
      await loadProjects();
    } catch (e) {
      setError(
        await recoverFromConflict(
          e instanceof Error
            ? e.message
            : "Clipboard is unavailable. Use the MCP command and project id manually.",
        ),
      );
    } finally {
      busyRef.current = false;
      setIsBusy(false);
      drainPendingAgentRefresh();
    }
  };

  // Work-mode Commit: stage + commit the current branch's tracked changes. The
  // backend never force-anything and surfaces git stderr on failure; we show the
  // result inline (no modal). The refreshed gitStatus updates the top bar in place.
  const commitProject = async (message: string) => {
    if (isArchived) return; // archived project is read-only.
    if (!currentProject) return;
    const projectId = currentProject.metadata.id;
    let succeeded = false;
    // Synchronous reentrancy guard: a second invocation while a git op is in
    // flight is a no-op (the gitActionBusy state can't gate a same-tick burst).
    await runGitActionGuarded(gitActionBusyRef, async () => {
      setGitActionBusy(true);
      setGitActionMessage(null);
      setGitActionError(false);
      // Suppress the 10s project reload for the duration so it cannot setProject
      // with a pre-commit gitStatus on top of our fresh post-commit one. The poll
      // tick already skips when busyRef.current is set (same gate the mutations use).
      busyRef.current = true;
      try {
        const call = commitProjectCall(projectId, message);
        const result = await invokeBackendCommand<ProjectGitCommandResult>(
          call.command,
          call.args,
        );
        setProject((current) =>
          current?.metadata.id === projectId
            ? { ...current, gitStatus: result.gitStatus }
            : current,
        );
        setGitActionMessage(result.message);
        setGitActionError(false);
        succeeded = true;
      } catch (e) {
        setGitActionMessage(
          e instanceof Error ? e.message : "git commit failed.",
        );
        setGitActionError(true);
      } finally {
        busyRef.current = false;
        setGitActionBusy(false);
      }
    });
    // MAJOR: the optimistic inline gitStatus merge above only patches the loaded
    // detail; the board summaries (and any field the commit changed beyond
    // gitStatus) stay stale because the 10s reload was suppressed for the op. After
    // a SUCCESSFUL commit, reload the selected project ONCE so board + Work mode
    // reflect real post-commit disk state. Runs after the guard released busyRef so
    // the reload's own loadProject is not self-suppressed. Skipped on failure (the
    // tree is unchanged) to avoid a needless fetch.
    if (succeeded) await reloadSelectedProjectSafe();
  };

  // Work-mode Push: push the current branch to origin (never force).
  const pushProject = async () => {
    if (isArchived) return; // archived project is read-only.
    if (!currentProject) return;
    const projectId = currentProject.metadata.id;
    let succeeded = false;
    // Same synchronous guard as commit: blocks a double-click OR a Commit-then-Push
    // burst from running two concurrent git ops on the same repo.
    await runGitActionGuarded(gitActionBusyRef, async () => {
      setGitActionBusy(true);
      setGitActionMessage(null);
      setGitActionError(false);
      busyRef.current = true;
      try {
        const call = pushProjectCall(projectId);
        const result = await invokeBackendCommand<ProjectGitCommandResult>(
          call.command,
          call.args,
        );
        setProject((current) =>
          current?.metadata.id === projectId
            ? { ...current, gitStatus: result.gitStatus }
            : current,
        );
        setGitActionMessage(result.message);
        setGitActionError(false);
        succeeded = true;
      } catch (e) {
        setGitActionMessage(
          e instanceof Error ? e.message : "git push failed.",
        );
        setGitActionError(true);
      } finally {
        busyRef.current = false;
        setGitActionBusy(false);
      }
    });
    // MAJOR: refresh board + Work-mode git status from real disk state after a
    // SUCCESSFUL push (ahead count clears, upstream tracking updates) — the inline
    // merge only patches the loaded detail and the 10s reload was suppressed during
    // the op. One reload, after the guard released busyRef; skipped on failure.
    if (succeeded) await reloadSelectedProjectSafe();
  };

  // Work-mode Pull: fast-forward the current branch from origin. On a divergence
  // the backend leaves the tree clean and surfaces git's "resolve manually" message
  // (no auto-merge in v1). Mirrors pushProject's guard/busy/error handling exactly.
  const pullProject = async () => {
    if (isArchived) return; // archived project is read-only.
    if (!currentProject) return;
    const projectId = currentProject.metadata.id;
    let succeeded = false;
    await runGitActionGuarded(gitActionBusyRef, async () => {
      setGitActionBusy(true);
      setGitActionMessage(null);
      setGitActionError(false);
      busyRef.current = true;
      try {
        const call = pullProjectCall(projectId);
        const result = await invokeBackendCommand<ProjectGitCommandResult>(
          call.command,
          call.args,
        );
        setProject((current) =>
          current?.metadata.id === projectId
            ? { ...current, gitStatus: result.gitStatus }
            : current,
        );
        setGitActionMessage(result.message);
        setGitActionError(false);
        succeeded = true;
      } catch (e) {
        setGitActionMessage(
          e instanceof Error ? e.message : "git pull failed.",
        );
        setGitActionError(true);
      } finally {
        busyRef.current = false;
        setGitActionBusy(false);
      }
    });
    // A pull that fast-forwards changes the working tree: reload once so board +
    // Work-mode reflect the real post-pull disk state. Skipped on failure (the
    // tree is unchanged by an --ff-only pull that was rejected).
    if (succeeded) await reloadSelectedProjectSafe();
  };

  // Work-mode slots (Fase 1 UI reorg): the per-project task board and the Notes
  // section, relocated out of the board-mode detail panel and into ProjectWorkspace
  // via its taskBoardSlot / notesSlot. Built here so they keep ProjectsView's own
  // handlers/state (moveTask, createTask, appendNote, noteDraft, …). The `?`-guard
  // narrows currentProject to non-null inside each branch, so the moved JSX's
  // existing currentProject uses still type-check; absent → null renders nothing.
  const notesNode =
    workMode && currentProject ? (
      <ProjectNotes
        notes={currentProject.state.notes}
        noteDraft={noteDraft}
        onNoteDraftChange={setNoteDraft}
        onAppend={appendNote}
        isBusy={isBusy}
        readOnly={isArchived}
        revision={currentProject.revision}
        // ProjectDetail.modifiedAt is `string | null`; ProjectNotes expects a
        // string and renders it via formatDate, whose falsy-guard maps "" → "no
        // date" — identical to the prior inline block that passed null straight in.
        modifiedAt={currentProject.modifiedAt ?? ""}
        updatedAt={currentProject.metadata.updatedAt}
      />
    ) : null;

  // B8: the per-project detail (status header + lifecycle actions + agent-root
  // editor + saved workflows), relocated OFF the create landing and INTO the
  // single-project Work page via ProjectWorkspace's detailSlot. The landing is now
  // create + Kanban(history) only; a project's detail lives on its own page.
  const projectDetailNode =
    workMode && currentProject ? (
      <div className="space-y-4">
        <ProjectStatusHeader
          project={currentProject}
          compact
          stageLabel={currentStage ? stageLabel(currentStage) : null}
          stageToneClass={currentStage ? stageTone[currentStage] : null}
          taskCounts={currentSummary?.taskCounts ?? null}
          isBusy={isBusy}
          workingAgent={
            currentProject.metadata.status === "archived"
              ? undefined
              : workingAgent
          }
          onReload={() => void reloadSelectedProjectSafe()}
          onRefreshLiveStatus={() => void refreshLiveStatus()}
          onPause={() => void updateProjectStatus("paused")}
          onResume={() => void updateProjectStatus("active")}
          onArchive={() => void updateProjectStatus("archived")}
          onDelete={() => void deleteProject()}
        />

        {/* Agent-root editor — unobtrusive single input + button, prefilled with the
        current root. Hidden for an archived (read-only) project. */}
        {currentProject.metadata.status !== "archived" && (
          <div className="flex flex-col gap-2 rounded-lg border border-cream-200 bg-white p-3 sm:flex-row sm:items-center">
            <label
              htmlFor="project-root-input"
              className="text-[11px] font-semibold uppercase tracking-widest text-cream-500"
            >
              Agent root
            </label>
            <input
              id="project-root-input"
              value={rootDraft}
              onChange={(event) => {
                const next = event.target.value;
                setRootDraft(next);
                // Finding 5: persist the unsaved edit on each keystroke so it
                // outlives an idle auto-lock unmount (removed when it equals the
                // saved rootPath or is empty).
                persistProjectRootDraft(
                  currentProject?.metadata.id,
                  next,
                  currentProject?.metadata.rootPath,
                );
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter") void setProjectRoot();
              }}
              placeholder="Absolute path agents launch in (blank = default)"
              data-help-title="The agent root is the folder CLI agents launch in."
              data-help-lines="Set this to the exact repository or working folder for this project.|Coders and verifiers open their terminal here, so a wrong root makes them edit the wrong files.|Leave it blank to fall back to the app's default root.|It only updates project metadata; it does not move any files."
              className="min-w-0 flex-1 rounded-lg border border-cream-200 bg-cream-50 px-3 py-2 font-mono text-[11px] text-cream-700 outline-none focus:border-terracotta-200"
            />
            <button
              type="button"
              onClick={() => void setProjectRoot()}
              disabled={
                isBusy ||
                rootDraft.trim() ===
                  (currentProject.metadata.rootPath ?? "").trim()
              }
              className="shrink-0 rounded-lg bg-teal px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
            >
              Set root
            </button>
          </div>
        )}

        {currentProject.metadata.status !== "archived" && (
          <section className="rounded-lg border border-cream-200 bg-white p-3">
            <div className="mb-3 flex items-center justify-between gap-3">
              <div>
                <h4 className="text-[12px] font-semibold text-cream-800">
                  Saved workflows
                </h4>
                <p className="text-[11px] text-cream-500">
                  Claude Code workflows discovered from this project and your user
                  profile.
                </p>
              </div>
              <button
                type="button"
                onClick={() => void loadSavedWorkflows(currentProject.metadata.id)}
                disabled={isBusy}
                className="rounded-md border border-cream-200 px-2 py-1 text-[11px] font-semibold text-cream-600 hover:bg-cream-50 disabled:opacity-60"
              >
                Refresh
              </button>
            </div>
            {workflowError && (
              <p className="mb-2 text-[11px] font-medium text-red-600">
                {workflowError}
              </p>
            )}
            {savedWorkflows.length === 0 ? (
              <p className="text-[11px] text-cream-500">
                No saved workflows found.
              </p>
            ) : (
              <div className="space-y-2">
                {savedWorkflows.map((workflow) => {
                  const args = workflowArgs[workflow.name] ?? "";
                  const running = workflowBusyName === workflow.name;
                  return (
                    <div
                      key={`${workflow.scope}-${workflow.name}`}
                      className="grid gap-2 rounded-md border border-cream-100 bg-cream-50/50 p-2 md:grid-cols-[minmax(0,1fr)_minmax(12rem,18rem)_auto]"
                    >
                      <div className="min-w-0">
                        <div className="flex min-w-0 items-center gap-2">
                          <span className="truncate font-mono text-[12px] font-semibold text-cream-800">
                            /{workflow.name}
                          </span>
                          <span className="rounded bg-white px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-cream-500">
                            {workflow.scope}
                          </span>
                        </div>
                        {workflow.description && (
                          <p className="mt-1 line-clamp-2 text-[11px] text-cream-500">
                            {workflow.description}
                          </p>
                        )}
                      </div>
                      <input
                        value={args}
                        onChange={(event) =>
                          setWorkflowArgs((prev) => ({
                            ...prev,
                            [workflow.name]: event.target.value,
                          }))
                        }
                        placeholder="Args"
                        spellCheck={false}
                        className="min-w-0 rounded-md border border-cream-200 bg-white px-2 py-1.5 font-mono text-[11px] text-cream-700 outline-none focus:border-terracotta-200"
                      />
                      <button
                        type="button"
                        onClick={() => void runSavedWorkflow(workflow)}
                        disabled={isBusy || running}
                        className="inline-flex items-center justify-center gap-1.5 rounded-md bg-terracotta px-3 py-1.5 text-[11px] font-semibold text-white disabled:opacity-60"
                      >
                        <Play className="h-3.5 w-3.5" />
                        {running ? "Running..." : "Run"}
                      </button>
                    </div>
                  );
                })}
              </div>
            )}
          </section>
        )}
      </div>
    ) : null;

  const taskBoardNode =
    workMode && currentProject ? (
      <CollapsibleSection
        icon={FolderKanban}
        title="Tasks"
        purpose="Tasks for this project"
        summary={`${tasksByColumn.done.length} done · ${tasksByColumn.wip.length} in progress · ${tasksByColumn.review.length} in review`}
        defaultOpen
        helpTitle="Board is for tasks and quick coder/verifier launches."
        helpLines="Task columns are the project-level Kanban workflow.|For Aspis Bio, coders should move tasks toward Review and verifiers decide when Done is justified.|Manual moves are blocked when an agent has an open claim to avoid conflicting writes.|The Markdown project file is the durable state behind this UI."
      >
        {/* Create-task form: hidden when the project is archived (read-only). The
          task COLUMNS below stay visible so archived tasks remain inspectable. */}
        {!isArchived && (
          <div className="mb-4 space-y-2">
            <div className="flex gap-2">
              <input
                value={taskDraft}
                onChange={(event) => setTaskDraft(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") void createTask();
                }}
                placeholder="Add task"
                data-help-title="A task is one concrete piece of project work."
                data-help-lines="Tasks appear in Todo, WIP, Review, Blocked, or Done.|Coders should move work toward Review; verifiers decide whether it can close.|Keep tasks small enough for one agent session.|Agents can update task state through MCP instead of editing the UI."
                className="min-w-0 flex-1 rounded-lg border border-cream-200 bg-cream-50 px-3 py-2 text-[12px] text-cream-700 outline-none focus:border-terracotta-200"
              />
              <button
                onClick={() => void createTask()}
                disabled={isBusy || !taskDraft.trim() || !taskCategory}
                data-help-title="This adds a Todo task to the project."
                data-help-lines="The task is written to the project Markdown file.|A category is required so the orchestrator and Oracle know how to treat it.|It starts in Todo and can later be claimed by a coder or orchestrator.|If an agent is already working, reload before adding overlapping tasks."
                className="inline-flex items-center gap-2 rounded-lg bg-teal px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
              >
                <Plus className="h-3.5 w-3.5" />
                Task
              </button>
            </div>
            <div
              className="flex flex-wrap items-center gap-1.5"
              data-help-title="A category is required for every new Todo card."
              data-help-lines="feature, hardening, bug, or other.|It tells the orchestrator how to treat the card and seeds Oracle's suspect files.|A bug card asks for a short description used to localize the suspect files.|Pick one before the Task button enables."
            >
              <span className="text-[10px] font-semibold uppercase tracking-widest text-cream-500">
                Category
              </span>
              {TASK_CATEGORIES.map((category) => {
                const active = taskCategory === category;
                return (
                  <button
                    key={category}
                    type="button"
                    onClick={() => setTaskCategory(category)}
                    aria-pressed={active}
                    className={`rounded-md px-2 py-1 text-[10px] font-semibold transition-colors ${
                      active
                        ? "bg-teal text-white"
                        : "bg-cream-100 text-cream-600 hover:bg-cream-200"
                    }`}
                  >
                    {categoryLabel(category)}
                  </button>
                );
              })}
            </div>
            {taskCategory === "bug" && (
              <textarea
                value={taskBugDescription}
                onChange={(event) => setTaskBugDescription(event.target.value)}
                rows={3}
                placeholder="Describe the bug — what is wrong, where you see it. This seeds Oracle's suspect files."
                data-help-title="The bug description is used to localize suspect files."
                data-help-lines="Oracle (P2) retrieves the most relevant files from the codebase index using this text.|Be specific: symptoms, error messages, the area of the app.|It is stored on the card and visible to the agent that claims it.|Optional, but a good description makes the suspect list far more useful."
                className="w-full resize-y rounded-lg border border-cream-200 bg-cream-50 px-3 py-2 text-[12px] text-cream-700 outline-none focus:border-terracotta-200"
              />
            )}
          </div>
        )}
        {!isArchived && (
          <div className="mb-2 flex items-center justify-end">
            <button
              type="button"
              onClick={() => setShowArrows((v) => !v)}
              aria-pressed={showArrows}
              title="Toggle the dependency arrows on the board"
              className={`rounded-md border px-2.5 py-1 text-[11px] font-semibold transition-colors ${
                showArrows
                  ? "border-terracotta bg-terracotta text-white"
                  : "border-cream-200 text-cream-600 hover:bg-cream-50"
              }`}
            >
              {showArrows ? "Arrows: on" : "Arrows: off"}
            </button>
          </div>
        )}
        <div className="overflow-x-auto pb-2">
          <div className="relative grid min-w-[1080px] grid-cols-5 gap-3">
            <TaskDependencyArrows
              tasks={currentProject?.state.tasks ?? []}
              visible={showArrows && !isArchived}
              colorByTask={arrowColorByTask}
            />
            {columns.map((column) => {
              const Icon = column.icon;
              const items = tasksByColumn[column.id];
              return (
                <div
                  key={column.id}
                  className="rounded-lg border border-cream-200 bg-cream-50 p-3"
                  onDragOver={(event) => event.preventDefault()}
                  onDrop={(event) => {
                    event.preventDefault();
                    handleColumnDrop(column.id);
                  }}
                  data-help-title={`${column.label} is a task status column.`}
                  data-help-lines="Task columns are the project-level Kanban workflow.|For Aspis Bio, coders should move tasks toward Review and verifiers decide when Done is justified.|Manual moves are blocked when an agent has an open claim to avoid conflicting writes.|The Markdown project file is the durable state behind this UI."
                >
                  <div className="mb-3 flex items-center justify-between">
                    <div className="flex items-center gap-2">
                      <Icon className="h-4 w-4 text-cream-500" />
                      <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
                        {column.label}
                      </h3>
                    </div>
                    <span className="rounded-md bg-white px-2 py-1 text-[10px] font-semibold text-cream-500">
                      {items.length}
                    </span>
                  </div>
                  {column.id === "done" && (
                    <p className="mb-2 rounded-md bg-white/70 px-2 py-1 text-[10px] font-semibold text-cream-400">
                      Verifier gated
                    </p>
                  )}

                  <div className="space-y-2">
                    {items.length === 0 ? (
                      <div className="rounded-lg border border-dashed border-cream-200 bg-white/70 p-3 text-[11px] text-cream-400">
                        Empty
                      </div>
                    ) : (
                      items.map((task) => {
                        // All gating is computed here (unchanged) and
                        // passed to TaskCard as plain booleans/titles. The
                        // card invokes the SAME moveTask / launchAgent /
                        // copyAgentPrompt handlers with identical args; it
                        // only changes presentation (button rows -> menus).
                        const taskClaims = claimsByTask[task.id] ?? [];
                        const taskSessions = sessionsByTask[task.id] ?? [];
                        const taskAgentControlled =
                          taskClaims.length > 0 || taskSessions.length > 0;
                        // R7: the agent(s) working this card — internal mini OR external
                        // Claude/codex — each a color-coded badge (the parallel-track viz).
                        const workers = deriveWorkers(taskClaims, taskSessions);
                        const manualMoveTitle = isArchived
                          ? "Archived project is read-only."
                          : taskAgentControlled
                            ? "An open agent claim or session controls this task; let MCP update status or wait for expiry."
                            : "Move task";
                        const launchable =
                          canLaunchProjectAgents(currentProject);
                        const moveDisabled =
                          isBusy ||
                          taskAgentControlled ||
                          isArchived ||
                          // B-F2: a "done" task is verifier-terminal — not draggable (the Move
                          // menu already offers it no targets).
                          task.status === "done";
                        return (
                          // B17: the card is draggable only when movable; the column's
                          // onDrop routes the dropped card through moveTask.
                          <div
                            key={task.id}
                            draggable={!moveDisabled}
                            onDragStart={() => handleCardDragStart(task)}
                            onDragEnd={() => {
                              dragTaskRef.current = null;
                              dragEndAtRef.current = Date.now();
                            }}
                          >
                            <TaskCard
                              task={task}
                              selected={selectedTaskId === task.id}
                              onSelect={(taskId) => {
                                // A drag emits a trailing click on some platforms — ignore a
                                // click that lands right after a drag-end.
                                if (
                                  Date.now() - dragEndAtRef.current <
                                  DRAG_CLICK_SUPPRESS_MS
                                ) {
                                  return;
                                }
                                // Twin the board with the console: select the task AND its
                                // primary worker (the first badge on this card) in ONE write,
                                // so the FocusStage focuses exactly the agent shown here.
                                selectBothSelection(
                                  primaryAgentForTask(taskId, taskClaims, taskSessions),
                                  taskId,
                                );
                              }}
                              agentControlled={taskAgentControlled}
                              workers={workers}
                              moveTargets={taskMoveTargets(task)}
                              moveDisabled={moveDisabled}
                              manualMoveTitle={manualMoveTitle}
                              showLaunch={task.status !== "done"}
                              launchTitle={projectLaunchTitle(currentProject)}
                              coderDisabled={
                                isBusy ||
                                !launchable ||
                                !canCoderClaimTask(task)
                              }
                              coderTitle={
                                !launchable
                                  ? projectLaunchTitle(currentProject)
                                  : canCoderClaimTask(task)
                                    ? "Launch coder"
                                    : "Coder cannot claim a review task"
                              }
                              verifierDisabled={
                                isBusy ||
                                !launchable ||
                                !canVerifierClaimTask(task)
                              }
                              verifierTitle={
                                !launchable
                                  ? projectLaunchTitle(currentProject)
                                  : canVerifierClaimTask(task)
                                    ? "Launch verifier"
                                    : "Verifier can claim Review or Blocked tasks"
                              }
                              manualDisabled={!launchable}
                              launchDisabled={isBusy || !launchable}
                              onMove={(status) => void moveTask(task, status)}
                              onLaunchCoder={() =>
                                void launchAgent(
                                  "coder",
                                  mainCoderClient,
                                  task.id,
                                )
                              }
                              onLaunchVerifier={() =>
                                void launchAgent(
                                  "verifier",
                                  mainCoderClient,
                                  task.id,
                                )
                              }
                              onCopyManualPrompt={() =>
                                void copyAgentPrompt(
                                  recommendedTaskRole(task),
                                  task.id,
                                )
                              }
                            />
                          </div>
                        );
                      })
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      </CollapsibleSection>
    ) : null;

  return (
    <div className="w-full space-y-6">
      {/* WORK MODE (sub-state of the board): full-bleed IDE shell. The kanban /
          calendar / project detail are skipped entirely; `← Board` returns.
          Rendered only when the flag is on AND the selected project's detail is
          loaded. This is the only alternate render — there is no Agents tab. */}
      {workMode && currentProject ? (
        <Suspense
          fallback={
            <div className="rounded-2xl border border-cream-200 bg-white p-8 text-center text-[12px] text-cream-400">
              Loading workspace…
            </div>
          }
        >
          <ProjectWorkspace
            // Remount on project switch so ALL per-project local state (splitAgentId, open
            // drawer, dock tab, commit/launcher) resets — none of it is valid across projects.
            key={currentProject.metadata.id}
            project={currentProject}
            sessions={workModeSessions}
            claims={projectClaims}
            events={projectAgentEvents}
            ptyAgents={ptyAgents}
            isBusy={isBusy}
            canLaunch={canLaunchProjectAgents(currentProject)}
            readOnly={isArchived}
            onUnarchive={() => void updateProjectStatus("active")}
            launchMessage={launchMessage}
            rules={agentState?.rules ?? []}
            customClients={config.customAgentClients ?? []}
            localCoderModel={config.localCoderBackend?.model ?? null}
            onBack={exitWorkMode}
            onRecallOrchestrator={() => {
              // Recall the orchestrator to REVISE the current plan, reusing the SAME
              // planner console 1:1 (the main-page orchestrator). It relaunches on the
              // existing project — so it sees the current tasks — and on re-approval it
              // sleeps. Surface the planner by returning to the board view.
              const replanGoal =
                "Revise the current plan for this project — the user wants to change it. Read the existing tasks and propose the changes.";
              setPlannerGoal(replanGoal);
              void planWithOrchestrator(replanGoal, plannerCoderId, plannerAutoCreate);
              exitWorkMode();
            }}
            onLaunch={(input) => void launchFromSpawnPanel(input)}
            onCopyPrompt={(selection) => void copyFromSpawnPanel(selection)}
            onSandboxModeChange={(mode: SandboxMode) => {
              setProject((current) =>
                current?.metadata.id === currentProject.metadata.id
                  ? {
                      ...current,
                      metadata: { ...current.metadata, sandboxMode: mode },
                    }
                  : current,
              );
            }}
            onWorkingSetChange={(next: string[]) => {
              setProject((current) =>
                current?.metadata.id === currentProject.metadata.id
                  ? {
                      ...current,
                      metadata: { ...current.metadata, workingSet: next },
                    }
                  : current,
              );
            }}
            onReloadProject={() => void loadProject(currentProject.metadata.id)}
            onCommit={(message) => void commitProject(message)}
            onPush={() => void pushProject()}
            onPull={() => void pullProject()}
            onStopAgent={(agentId) => void stopAgent(agentId)}
            onFocusCli={(agentId) => void openAgentCli(agentId)}
            onCopyRecovery={(agentId) => {
              // The Work-mode controls call by agentId; resolve the live session
              // (recovery text reads its role/status/task) from this project's
              // sessions. A miss (e.g. the agent just exited) is a no-op.
              const session = currentProjectSessions.find(
                (s) => s.agentId === agentId,
              );
              if (session) void copyAgentRecovery(session);
            }}
            gitActionMessage={gitActionMessage}
            gitActionError={gitActionError}
            gitActionBusy={gitActionBusy}
            taskBoardSlot={taskBoardNode}
            notesSlot={notesNode}
            detailSlot={projectDetailNode}
          />
        </Suspense>
      ) : (
        // BOARD mode (Mode-1, unconditional now that the standalone Agents page is
        // dissolved): the kanban board + calendar + selected-project detail. Work
        // mode is the only alternate render, gated solely by `workMode` above.
        <div className="space-y-6">
          {error && (
            <div className="rounded-lg border border-coral/20 bg-coral/[0.04] px-4 py-3 text-[12px] font-medium text-coral-dark">
              {error}
            </div>
          )}
          {agentSyncError && (
            <div className="rounded-lg border border-amber/20 bg-amber/[0.06] px-4 py-3 text-[12px] font-medium text-amber-dark">
              Agent sync degraded: {agentSyncError}
            </div>
          )}

          <section className="flex flex-wrap items-center gap-2 rounded-2xl border border-cream-200 bg-white px-3 py-2">
            <FolderKanban className="h-5 w-5 shrink-0 text-terracotta" />
            <input
              value={titleDraft}
              onChange={(event) => setTitleDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") void createProject();
              }}
              placeholder="New project title — or describe it to the Orchestrator below…"
              data-help-title="A project is the local notebook for one work stream."
              data-help-lines="Projects are Markdown files that the UI, agents, and Oracle can read.|Use one project for one goal, for example scrna-seq backend and frontend.|Agents update tasks and notes through MCP so the board stays current.|The project file is indexed by Oracle after it changes."
              className="min-w-[12rem] flex-1 rounded-xl border border-transparent bg-transparent px-2 py-2 text-[13px] text-cream-800 outline-none focus:border-cream-200 focus:bg-cream-50"
            />
            <button
              onClick={() => void pickProjectFolder()}
              // B-F7: the native folder dialog only exists in the Tauri runtime — disable in the
              // browser harness (where the dynamic import would silently no-op).
              disabled={isBusy || !isTauriRuntime()}
              title={
                newProjectRootDraft ||
                "Choose the working folder the agent reads/writes in"
              }
              data-help-title="Choose the project's working folder."
              data-help-lines="The agent reads + writes code in THIS folder.|Without a working folder the project can't run tools — project_structure, visual_check, and Censor all need a root.|Pick the repo/folder the coder should work in.|This is the one thing to set here; the rest is configured in Settings."
              className="inline-flex min-w-0 shrink-0 items-center gap-1.5 rounded-xl border border-cream-200 bg-cream-50 px-3 py-1.5 text-[12px] font-semibold text-cream-600 hover:bg-cream-100 disabled:opacity-60"
            >
              <FolderOpen className="h-3.5 w-3.5 shrink-0" />
              <span className="max-w-[10rem] truncate">
                {newProjectRootDraft
                  ? (newProjectRootDraft.split(/[\\/]/).filter(Boolean).pop() ??
                    newProjectRootDraft)
                  : "Folder"}
              </span>
            </button>
            <button
              onClick={() => {
                setCloneError(null);
                setCloneOpen((open) => !open);
              }}
              disabled={cloneBusy}
              data-help-title="This clones a GitHub repository and adds it as a project."
              data-help-lines="Paste an https://github.com/owner/repo URL to clone it locally.|The clone uses your GitHub token from Settings; the token never appears in the URL or logs.|It refuses to overwrite a non-empty folder.|The cloned repo becomes a project rooted at the new folder."
              className="inline-flex shrink-0 items-center gap-1.5 rounded-xl border border-cream-200 bg-cream-50 px-3 py-1.5 text-[12px] font-semibold text-cream-600 hover:bg-cream-100 disabled:opacity-60"
            >
              <GitBranch className="h-3.5 w-3.5" />
              GitHub
            </button>
            <button
              onClick={() => void createProject()}
              disabled={isBusy || !titleDraft.trim()}
              data-help-title="This creates a new project Markdown file."
              data-help-lines="The new project starts active and appears on the stage board.|It does not launch agents by itself.|Choose the working folder (button at left) before creating — agents cannot run without one; there is no app-folder fallback.|Oracle can index the project file after the watcher sees it."
              className="inline-flex shrink-0 items-center gap-1.5 rounded-xl bg-terracotta px-4 py-1.5 text-[12px] font-semibold text-white hover:bg-terracotta/90 disabled:opacity-60"
            >
              <Plus className="h-3.5 w-3.5" />
              Create
            </button>
          </section>

          {/* Clone-from-GitHub dialog (inline, board-mode only). Mirrors the small
          inline-input idiom used elsewhere; reuses the same input/button tokens. */}
          {cloneOpen && (
            <section className="flex flex-col gap-2 rounded-lg border border-cream-200 bg-white p-4">
              <label className="text-[12px] font-semibold text-cream-700">
                Clone a GitHub repository
              </label>
              <div className="flex min-w-0 flex-wrap gap-2 sm:flex-nowrap sm:items-center">
                <input
                  value={cloneUrlDraft}
                  onChange={(event) => {
                    setCloneUrlDraft(event.target.value);
                    if (cloneError) setCloneError(null);
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") void cloneProject();
                  }}
                  placeholder="https://github.com/owner/repo"
                  autoFocus
                  spellCheck={false}
                  className="min-w-0 flex-1 rounded-lg border border-cream-200 bg-cream-50 px-3 py-2 text-[12px] text-cream-700 outline-none focus:border-terracotta-200"
                />
                <button
                  onClick={() => void cloneProject()}
                  disabled={cloneBusy || !cloneUrlDraft.trim()}
                  className="shrink-0 inline-flex items-center gap-2 rounded-lg bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
                >
                  <GitBranch className="h-3.5 w-3.5" />
                  {cloneBusy ? "Cloning…" : "Clone"}
                </button>
              </div>
              {cloneError && (
                <p className="text-[11px] font-medium text-red-600">
                  {cloneError}
                </p>
              )}
            </section>
          )}

          {/* The Orchestrator composer — the centerpiece "describe a goal → plan → board" surface.
          Always visible; "Plan it" stays guarded until a project with a working folder is selected
          (the wiring to the existing plan-first flow lands in a follow-up). */}
          <PlannerPlanMode
            goal={plannerGoal}
            contextLabel={
              currentProject?.metadata.rootPath
                ?.split("/")
                .filter(Boolean)
                .pop() ?? ""
            }
            plannerModelLabel={config.localCoderBackend?.model ?? mainCoderClient}
            live={!!orchestratorAgentId || !!cloudOrchestratorAgentId}
            planCards={derivePlanCards(currentProject?.state.tasks ?? [])}
            pages={plannerWeb.pages}
            findings={plannerWeb.findings}
            webMode={plannerWebMode}
            onWebModeChange={setPlannerWebMode}
            onManualSearch={(q) => {
              const query = q.trim();
              if (!query) return;
              if (!orchestratorAgentId) {
                setError(
                  "Start the Orchestrator (describe a goal) before running a manual web search.",
                );
                return;
              }
              // Manual web search = steer the running orchestrator to search for it
              // (reuses the P2 steer channel). The Websearch view then shows the results.
              invokeBackendCommand("orchestrator_steer", {
                agentId: orchestratorAgentId,
                message: `Web-search for "${query}" and use the findings in the plan.`,
              }).catch((e) =>
                setError(
                  e instanceof Error ? e.message : "Manual search failed.",
                ),
              );
            }}
            design={plannerDesign}
            linkedTask={null}
            onOpenInDesign={() => requestView("design")}
            messages={plannerConvoLive}
            awaitingReply={plannerAwaitingReply}
            onSend={(text) => {
              const msg = text.trim();
              if (!msg) return;
              // Always echo the user's message in the chat.
              setPlannerMessages((prev) => [...prev, { role: "user", text: msg }]);
              if (plannerOrchestratorClient === "orchestrator" && orchestratorAgentId) {
                // LIVE: steer the running LOCAL orchestrator (mid-plan course-correction).
                // It drains DEVBOULE_STEER_FILE between rounds and injects this as a
                // human turn (P2). Routed by the selected client (not just "any non-null id")
                // so a stale-but-recent local session can't steal a cloud-mode message.
                invokeBackendCommand("orchestrator_steer", {
                  agentId: orchestratorAgentId,
                  message: msg,
                }).catch((e) =>
                  setError(
                    e instanceof Error
                      ? e.message
                      : "Steer failed — message not delivered.",
                  ),
                );
                return;
              }
              if (cloudOrchestratorAgentId) {
                // Phase D: a CLOUD duplex orchestrator — write the turn straight to its piped
                // stdin (the cloud counterpart of orchestrator_steer). Its event stream surfaces
                // the reply in the Stage just like the local path.
                invokeBackendCommand("project_cloud_orchestrator_send", {
                  agentId: cloudOrchestratorAgentId,
                  message: msg,
                }).catch((e) =>
                  setError(
                    e instanceof Error
                      ? e.message
                      : "Send failed — message not delivered.",
                  ),
                );
                return;
              }
              // A Planner launch is in flight but its session hasn't registered yet — don't
              // create a duplicate project or launch a second Planner; it'll pick up from here.
              if (plannerLaunching) {
                setPlannerMessages((prev) => [
                  ...prev,
                  {
                    role: "assistant",
                    text: "The Planner is starting — one moment, then it'll pick up your messages.",
                  },
                ]);
                return;
              }
              // FRESH landing (no project yet): the first message CREATES the project
              // (with the chosen Folder) and launches the Planner on it.
              if (!currentProject) {
                void startNewProject(msg, plannerCoderId, plannerAutoCreate);
                return;
              }
              // A project is selected but no Planner is running: (re)plan it. Needs a folder.
              if (!currentProject.metadata.rootPath) {
                setPlannerMessages((prev) => [
                  ...prev,
                  {
                    role: "assistant",
                    text: "This project has no folder yet, so I can't plan or write code in it. Pick a folder, then send your goal again.",
                  },
                ]);
                setError("Select a folder for this project before planning.");
                return;
              }
              if (busyRef.current || orchestratorPlanRef.current) return;
              setPlannerGoal(msg);
              void planWithOrchestrator(msg, plannerCoderId, plannerAutoCreate);
            }}
            orchestrators={[
              { id: "orchestrator", label: "Local" },
              { id: "claude", label: "Claude" },
              { id: "codex", label: "Codex" },
            ]}
            orchestratorId={plannerOrchestratorClient}
            onOrchestratorChange={setPlannerOrchestratorClient}
            cloudTerminalAgentId={null}
            coders={[
              { id: "claude", label: "Claude" },
              { id: "codex", label: "Codex" },
              ...(config.customAgentClients ?? []).map((c) => ({
                id: c.id,
                label: c.label,
              })),
            ]}
            coderId={plannerCoderId}
            onCoderChange={setPlannerCoderId}
            autoCreate={plannerAutoCreate}
            onAutoCreateToggle={() => setPlannerAutoCreate((v) => !v)}
            canCreatePlan={!!orchestratorAgentId || !!cloudOrchestratorAgentId}
            onCreatePlan={() => {
              // B10: discuss-first — the plan is created on demand. Steer the live orchestrator
              // (local OR cloud duplex) to draft + submit the plan and create the Kanban tasks.
              const planMessage =
                "We've discussed enough — now draft the plan: submit it with plan_submit, and once it's approved create the Kanban tasks with project_create_plan_tasks (one per phase).";
              const target =
                plannerOrchestratorClient === "orchestrator" && orchestratorAgentId
                  ? { command: "orchestrator_steer", agentId: orchestratorAgentId }
                  : cloudOrchestratorAgentId
                    ? {
                        command: "project_cloud_orchestrator_send",
                        agentId: cloudOrchestratorAgentId,
                      }
                    : null;
              if (!target) {
                setError(
                  "Start the Orchestrator (describe a goal) before creating the plan.",
                );
                return;
              }
              setPlannerMessages((prev) => [
                ...prev,
                { role: "user", text: "Create the plan and the tasks now." },
              ]);
              invokeBackendCommand(target.command, {
                agentId: target.agentId,
                message: planMessage,
              }).catch((e) =>
                setError(
                  e instanceof Error ? e.message : "Could not start planning.",
                ),
              );
            }}
          />

          {/* Overview segmented toggle: the active stage board (default) vs. a simple
          read-only list of archived projects. The "Archived (N)" segment is shown
          disabled when there is nothing archived, so the control stays predictable
          without cluttering the header when N === 0. */}
          <div className="flex items-center gap-1 rounded-lg border border-cream-200 bg-white p-1 w-fit">
            <button
              type="button"
              onClick={() => setOverviewTab("board")}
              aria-pressed={overviewTab === "board"}
              data-help-title="Show the active stage board and calendar."
              data-help-lines="The board is the active project workflow.|Archived projects leave the board and the calendar.|Use the Archived tab to find and reopen them.|This toggle only changes the overview, not the selection."
              className={`rounded-md px-3 py-1.5 text-[12px] font-semibold transition-colors ${
                overviewTab === "board"
                  ? "bg-terracotta text-white"
                  : "text-cream-600 hover:bg-cream-50"
              }`}
            >
              Board
            </button>
            <button
              type="button"
              onClick={() => setOverviewTab("archived")}
              aria-pressed={overviewTab === "archived"}
              disabled={archivedProjects.length === 0}
              data-help-title="Show archived projects."
              data-help-lines="Archived projects are read-only and out of the active workflow.|They do not appear on the board or the calendar.|Open one to view it, then Unarchive from its page to make it active again.|Archiving is reversible."
              className={`rounded-md px-3 py-1.5 text-[12px] font-semibold transition-colors disabled:opacity-40 ${
                overviewTab === "archived"
                  ? "bg-terracotta text-white"
                  : "text-cream-600 hover:bg-cream-50"
              }`}
            >
              Archived ({archivedProjects.length})
            </button>
          </div>

          {overviewTab === "archived" ? (
            // Archived list: a calm read-only list of archived projects, each with an
            // [Open] button that enters the same single-project Work mode a board card
            // does (enterWorkMode). Unarchive lives in the page banner (Part 2).
            <section className="rounded-lg border border-cream-200 bg-white p-4">
              <div className="mb-3 flex items-center gap-2">
                <Archive className="h-4 w-4 text-cream-500" aria-hidden />
                <h3 className="text-sm font-semibold text-cream-800">
                  Archived projects
                </h3>
              </div>
              {archivedProjects.length === 0 ? (
                <p className="text-[12px] text-cream-400">
                  No archived projects.
                </p>
              ) : (
                <ul className="space-y-2">
                  {archivedProjects.map((item) => (
                    <li
                      key={item.id}
                      className="flex flex-col gap-2 rounded-lg border border-cream-200 bg-cream-50 p-3 sm:flex-row sm:items-center sm:justify-between"
                    >
                      <div className="min-w-0">
                        <p className="truncate text-[12px] font-semibold text-cream-800">
                          {item.title}
                        </p>
                        <p className="mt-0.5 text-[11px] text-cream-400">
                          Updated {formatDateTime(item.updatedAt)}
                        </p>
                      </div>
                      <button
                        type="button"
                        onClick={() => enterWorkMode(item.id)}
                        className="shrink-0 self-start rounded-lg border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-700 hover:bg-cream-50 sm:self-auto"
                      >
                        Open
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </section>
          ) : (
            <>
              <ProjectsBoard
                projectsByStage={projectsByStage}
                claimsByProject={claimsByProject}
                sessionsByProject={sessionsByProject}
                censorCountByProject={censorCountByProject}
                selectedId={selectedId}
                isLoading={isLoadingProjects}
                onSelect={enterWorkMode}
              />

              {/* Calendar / organizer BELOW the board (Board mode only: this whole
              block is skipped in Work mode). Fed from activeProjects so archived
              projects leave the calendar exactly as they leave the stage board. */}
              <ProjectCalendar
                projects={activeProjects}
                onSelectProject={selectProjectOnly}
                onChanged={() => void loadProjects()}
              />
            </>
          )}

          <div className="grid grid-cols-1 gap-5">
            {selectedId &&
            loadingProjectId === selectedId &&
            !currentProject ? (
              <main className="rounded-lg border border-cream-200 bg-white p-8">
                <div className="mb-4 h-6 w-52 animate-pulse rounded-md bg-cream-100" />
                <div className="grid grid-cols-1 gap-3 xl:grid-cols-5">
                  {columns.map((column) => (
                    <div
                      key={column.id}
                      className="h-44 animate-pulse rounded-lg bg-cream-50"
                    />
                  ))}
                </div>
              </main>
            ) : currentProject ? (
              // B8: the per-project detail moved to the single-project Work page
              // (ProjectWorkspace detailSlot ← projectDetailNode). The landing is now
              // create + Kanban(history); a selected project opens its own page.
              null
            ) : (
              <main className="rounded-lg border border-dashed border-cream-200 bg-white p-8 text-center">
                <FolderKanban className="mx-auto mb-3 h-8 w-8 text-cream-300" />
                <p className="text-sm font-semibold text-cream-700">
                  {error && projects.length === 0
                    ? "Project list unavailable."
                    : "Create a project to start."}
                </p>
                <p className="mt-1 text-[12px] text-cream-400">
                  {error && projects.length === 0
                    ? "Fix the load error above or reload the project folder."
                    : "Files are stored as local Markdown with a structured Aspis project block."}
                </p>
              </main>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
