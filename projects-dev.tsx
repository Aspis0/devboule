// Standalone dev entry for the redesigned Projects Kanban UI.
//
// Loaded by projects-dev.html. Mounts the NEW presentational Projects
// components (ProjectCard, ProjectStatusHeader, ProjectAgentPanel,
// CollapsibleSection, TaskCard, MiniMenu) against valid MOCK fixtures with
// no-op / console.log callbacks. No React app shell, no Tauri, no login, no
// backend invoke — purely visual layout verification in a plain browser.
//
// SECURITY: never a production rollup input (see vite.config.ts). Dev-only.

import React from "react";
import ReactDOM from "react-dom/client";
import { BookOpen, LayoutGrid } from "lucide-react";

import { ProjectsBoard } from "./src/components/projects/ProjectsBoard";
import type { ProjectStageId } from "./src/components/projects/projectStage";
import { ProjectStatusHeader } from "./src/components/projects/ProjectStatusHeader";
import { ProjectModePanel } from "./src/components/projects/ProjectModePanel";
import { CollapsibleSection } from "./src/components/projects/CollapsibleSection";
import { TaskCard } from "./src/components/projects/TaskCard";
import type { ColumnId, MoveTarget } from "./src/components/projects/taskBoard";
import {
  workflowModeForStage,
  type WorkflowModeId,
} from "./src/components/projects/workflowMode";
import {
  mockFindings,
  mockPlan,
} from "./src/components/projects/workflowMockData";
import type {
  AgentClaim,
  AgentSession,
  ProjectSummary,
  ProjectTask,
} from "./src/types/backend";

// Import the SAME global stylesheet the real app uses (src/main.tsx -> this),
// so Tailwind utility classes + the cream theme tokens actually apply here.
import "./src/styles/index.css";

import {
  activeClaim,
  activeSession,
  agentControlledTaskIds,
  boardTasks,
  detailTaskCounts,
  projectAgentEvents,
  projectDetail,
  projectSessions,
  summaryActiveNoAgent,
  summaryActiveWithAgent,
  summaryDone,
  summaryLaunching,
  summaryPaused,
  summaryReview,
} from "./projects-dev-fixtures";

// --- no-op handlers (logged so the menus/buttons are visibly interactive) ---

const log =
  (label: string) =>
  (...args: unknown[]) =>
    // eslint-disable-next-line no-console
    console.log(`[projects-dev] ${label}`, ...args);

// --- board gating mirrors ProjectsView (kept in sync, not imported) ---------

const columns: { id: ColumnId; label: string }[] = [
  { id: "todo", label: "Todo" },
  { id: "wip", label: "WIP" },
  { id: "review", label: "Review" },
  { id: "blocked", label: "Blocked" },
  { id: "done", label: "Done" },
];

function taskMoveTargets(task: ProjectTask): MoveTarget[] {
  if (task.status === "done") return [];
  return columns.filter(
    (target) => target.id !== task.status && target.id !== "done",
  );
}

function canCoderClaimTask(task: ProjectTask) {
  return (
    task.status === "todo" || task.status === "wip" || task.status === "blocked"
  );
}

function canVerifierClaimTask(task: ProjectTask) {
  return task.status === "review" || task.status === "blocked";
}

// --- detail "Board" column grid ---------------------------------------------

function BoardColumn({ column }: { column: { id: ColumnId; label: string } }) {
  const tasks = boardTasks.filter((task) => task.status === column.id);
  return (
    <div className="flex min-w-0 flex-col gap-2">
      <p className="px-1 text-[10px] font-semibold uppercase tracking-widest text-cream-500">
        {column.label}
      </p>
      {tasks.map((task) => {
        const agentControlled = agentControlledTaskIds.has(task.id);
        const targets = taskMoveTargets(task);
        const coderDisabled = !canCoderClaimTask(task);
        const verifierDisabled = !canVerifierClaimTask(task);
        return (
          <TaskCard
            key={task.id}
            task={task}
            agentControlled={agentControlled}
            moveTargets={targets}
            moveDisabled={agentControlled}
            manualMoveTitle={
              agentControlled
                ? "An agent controls this task; manual move is disabled."
                : "Move task"
            }
            showLaunch
            launchTitle="Launch agent for this task"
            coderDisabled={coderDisabled}
            coderTitle={
              coderDisabled
                ? "Coder cannot claim a task in this status."
                : "Launch coder"
            }
            verifierDisabled={verifierDisabled}
            verifierTitle={
              verifierDisabled
                ? "Verifier cannot claim a task in this status."
                : "Launch verifier"
            }
            manualDisabled={false}
            onMove={(status) => log(`move ${task.id}`)(status)}
            onLaunchCoder={log(`launch coder ${task.id}`)}
            onLaunchVerifier={log(`launch verifier ${task.id}`)}
            onCopyManualPrompt={log(`copy manual prompt ${task.id}`)}
          />
        );
      })}
    </div>
  );
}

// --- macro stage board fixtures (the REAL columnar ProjectsBoard) -----------

// Projects spread across several stages so every COLUMN is populated (and the
// empty ones show the dashed placeholder). The grouping is hand-built here the
// same shape ProjectsView's projectsByStage useMemo produces.
const projectsByStage: Record<ProjectStageId, ProjectSummary[]> = {
  planned: [summaryActiveNoAgent],
  launching: [summaryLaunching],
  active: [summaryActiveWithAgent],
  review: [summaryReview],
  blocked: [summaryPaused],
  verified: [summaryDone],
};

// Agent signals keyed by project id — the active project (proj-edge) has a
// working claim + recent session so its card shows the WHO agent line.
const claimsByProject: Record<string, AgentClaim[]> = {
  [summaryActiveWithAgent.id]: [activeClaim],
};
const sessionsByProject: Record<string, AgentSession[]> = {
  [summaryActiveWithAgent.id]: [activeSession],
};

// Dev-only stage switcher: maps each workflow mode to a representative kanban
// stage so the mode panel + header re-render for the chosen mode. Lets the user
// click through Plan / Build / Review / Done and SEE each mode.
const STAGE_FOR_MODE: Record<WorkflowModeId, ProjectStageId> = {
  architect: "planned",
  coder: "active",
  reviewer: "review",
  done: "verified",
};

function Harness() {
  // Dev-only: the mocked project's stage, driven by the segmented switcher.
  const [mode, setMode] = React.useState<WorkflowModeId>("coder");
  const stage = STAGE_FOR_MODE[mode];
  const workflowMode = workflowModeForStage(stage);
  // Surface the freshest session as the header's working-agent line (only when
  // some agent is actually working, i.e. in Build mode here).
  const workingAgent = mode === "coder" ? activeSession : null;

  return (
    <div className="min-h-screen bg-cream-50 px-6 pb-24 pt-6 text-cream-800">
      <div className="flex w-full flex-col gap-8">
        {/* 1. Macro KANBAN stage board: the REAL columnar ProjectsBoard ---- */}
        <section>
          <h3 className="mb-3 text-[11px] font-semibold uppercase tracking-widest text-cream-500">
            Project stage board
          </h3>
          <ProjectsBoard
            projectsByStage={projectsByStage}
            claimsByProject={claimsByProject}
            sessionsByProject={sessionsByProject}
            selectedId={summaryActiveWithAgent.id}
            isLoading={false}
            onSelect={(id) => log("select project")(id)}
          />
        </section>

        {/* 2. Selected project detail ------------------------------------- */}
        <section className="flex flex-col gap-4">
          <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
            Selected project detail
            <span className="ml-2 normal-case tracking-normal text-cream-400">
              — click the STAGE rail below to switch mode
            </span>
          </h3>

          <ProjectStatusHeader
            project={projectDetail}
            stageLabel={workflowMode.stageLabel}
            stageToneClass="bg-teal/10 text-teal"
            taskCounts={detailTaskCounts}
            isBusy={false}
            workflowMode={workflowMode.id}
            workingAgent={workingAgent}
            onSelectStage={setMode}
            onReload={log("reload")}
            onRefreshLiveStatus={log("refresh live status")}
            onPause={log("pause")}
            onResume={log("resume")}
            onArchive={log("archive")}
          />

          {/* Stage-aware workflow MODE panel: Architect (mock plan) / Coder
              (the live agent panel) / Reviewer (mock 3/10 findings) / Done. The
              switcher above drives which mode renders. Coder mode shows the
              three concurrent live agents; coderFinished is forced true so the
              "Send to review" handoff is visible in the skeleton. */}
          <ProjectModePanel
            stage={stage}
            plan={mockPlan}
            architectRunning={false}
            sessions={projectSessions}
            claims={[activeClaim]}
            events={projectAgentEvents}
            tasks={projectDetail.state.tasks}
            canLaunch
            launchTitle="Launch agent"
            isBusy={false}
            launchMessage="Coder launched in Codex at project root."
            coderFinished
            onLaunchAgent={(role, client, taskId) =>
              log("launch")(role, client, taskId)
            }
            onCopyPrompt={(role, taskId) => log("copy prompt")(role, taskId)}
            onOpenCli={(agentId) => log("open cli")(agentId)}
            onStop={(agentId) => log("stop")(agentId)}
            onRecovery={(session) => log("recovery")(session.agentId)}
            onOpenControl={log("open control (active)")}
            findings={mockFindings}
            reviewerRunning={false}
            onLaunchArchitect={log("launch architect (Opus)")}
            onApprovePlan={log("approve plan")}
            onStartCoder={log("start coder → Build")}
            onSendToReview={log("send to review → Review")}
            onLaunchReviewer={log("launch reviewer")}
            onSendFindingsToCoder={log("send findings to coder → Build")}
            onApproveRelease={log("approve → release (push)")}
          />

          {/* Board section (open) with the 5-column Kanban grid ----------- */}
          <CollapsibleSection
            icon={LayoutGrid}
            title="Board"
            purpose="Tasks for this project"
            summary={`${boardTasks.length} tasks`}
            defaultOpen
            onToggle={(open) => log("toggle board")(open)}
          >
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-5">
              {columns.map((column) => (
                <BoardColumn key={column.id} column={column} />
              ))}
            </div>
          </CollapsibleSection>

          {/* Notes: the cross-phase running log, kept alongside the mode. --- */}
          <CollapsibleSection
            icon={BookOpen}
            title="Notes"
            purpose="The project's running log — what agents did, plus reminders"
            summary="1 note"
            onToggle={(open) => log("toggle note")(open)}
          >
            <p className="mb-3 text-[12px] text-cream-500">
              Every important action or reminder an agent leaves lands here —
              it's what a verifier reads before marking work Done.
            </p>
            {projectDetail.state.notes[0]?.text ? (
              <p className="text-[12px] leading-5 text-cream-600">
                {projectDetail.state.notes[0]?.text}
              </p>
            ) : (
              <p className="text-[12px] text-cream-400">No notes yet.</p>
            )}
          </CollapsibleSection>
        </section>
      </div>
    </div>
  );
}

const host = document.getElementById("projects-dev-host");
if (host) {
  ReactDOM.createRoot(host).render(
    <React.StrictMode>
      <Harness />
    </React.StrictMode>,
  );
}
