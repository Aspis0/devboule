// Macro KANBAN board for the Projects page: one COLUMN per project stage
// (Planned -> Launching -> Active -> Review -> Blocked -> Verified), each column
// holding the ProjectCards assigned to that stage. Extracted from ProjectsView
// purely for presentation — the stage-derivation logic lives unchanged in
// projectStage.ts and the per-project agentActive/agentLabel signals are
// computed here from the SAME grouping maps the view already builds.
//
// Visual language mirrors the detail Board task columns (bordered cream column
// with an uppercase header + count chip, dashed placeholder when empty,
// horizontal scroll when the six columns overflow).

import { memo } from "react";
import type { AgentClaim, AgentSession, ProjectSummary } from "../../types/backend";
import { isWorkingClaim } from "../../utils/agentClaims";
import { freshestSession } from "./agentLiveStatus";
import { ProjectCard } from "./ProjectCard";
import {
  type ProjectStageId,
  projectStageTitles,
  projectStages,
} from "./projectStage";

function ProjectsBoardComponent({
  projectsByStage,
  claimsByProject,
  sessionsByProject,
  censorCountByProject,
  selectedId,
  isLoading,
  onSelect,
}: {
  /** Projects already grouped by stage (most-important first per stage). */
  projectsByStage: Record<ProjectStageId, ProjectSummary[]>;
  /** Open claims grouped by project id — drives the agentActive/agentLabel WHO. */
  claimsByProject: Record<string, AgentClaim[]>;
  /** Recent sessions grouped by project id — drives the agentActive/agentLabel WHO. */
  sessionsByProject: Record<string, AgentSession[]>;
  /** Open Censor finding count per project id — drives the ⚠ card chip. Missing
   *  entries (not yet fetched / no root) render no chip. Event-driven, no poll. */
  censorCountByProject: Record<string, number>;
  selectedId: string | null;
  isLoading: boolean;
  onSelect: (projectId: string) => void;
}) {
  return (
    <section className="overflow-x-auto pb-2">
      <div className="grid min-w-[1180px] grid-cols-6 gap-3">
        {projectStages.map((stage) => {
          const Icon = stage.icon;
          const items = projectsByStage[stage.id];
          return (
            <div
              key={stage.id}
              title={projectStageTitles[stage.id]}
              data-help-title={`${stage.label} is a project stage column.`}
              data-help-lines="Project stages are computed from project lifecycle, task counts, agent claims, and recent sessions.|For Devboule, this is the high-level portfolio board: see what is planned, active, blocked, in review, or verified.|Stages move when agents update project state through MCP or when humans change tasks/status.|Use this board before spawning more agents so work does not duplicate."
              className="flex flex-col rounded-lg border border-cream-200 bg-cream-50 p-3"
            >
              <div className="mb-3 flex items-center justify-between gap-2">
                <div className="flex items-center gap-2">
                  <Icon className="h-4 w-4 text-cream-500" />
                  <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
                    {stage.label}
                  </h3>
                </div>
                <span className="rounded-md bg-white px-2 py-1 text-[10px] font-semibold text-cream-500">
                  {items.length}
                </span>
              </div>
              <div className="space-y-2">
                {isLoading ? (
                  Array.from({ length: 2 }).map((_, index) => (
                    <div
                      key={`${stage.id}-loading-${index}`}
                      className="h-20 animate-pulse rounded-lg border border-cream-200 bg-white/80"
                    />
                  ))
                ) : items.length === 0 ? (
                  <div className="rounded-lg border border-dashed border-cream-200 bg-white/70 p-3 text-center text-[11px] text-cream-400">
                    —
                  </div>
                ) : (
                  items.map((item) => {
                    // "Agent actively working" reuses the same signal the old
                    // card showed: at least one working claim or recent
                    // project session for this project. Same helpers
                    // (isWorkingClaim + sessionsByProject), just collapsed to
                    // a single boolean for the calm preview indicator.
                    const projectClaims = claimsByProject[item.id] ?? [];
                    const workingClaim = projectClaims.find((claim) =>
                      isWorkingClaim(claim),
                    );
                    // F43: sessionsByProject is already live-only; agentActive must
                    // not light on a claim alone while no live session exists.
                    const sessions = sessionsByProject[item.id] ?? [];
                    const agentActive = sessions.length > 0;
                    // WHO line: prefer a live session that matches a working claim,
                    // else the freshest live session. Same freshestSession helper
                    // as the header/panel (#4).
                    const workingSession = freshestSession(sessions);
                    const claimMatched = workingClaim
                      ? sessions.find((s) => s.agentId === workingClaim.agentId)
                      : undefined;
                    const agentLabel = claimMatched
                      ? `${claimMatched.agentId} · ${claimMatched.role}`
                      : workingSession
                        ? `${workingSession.agentId} · ${workingSession.role}`
                        : null;
                    return (
                      <ProjectCard
                        key={item.id}
                        project={item}
                        stageLabel={stage.label}
                        selected={selectedId === item.id}
                        agentActive={agentActive}
                        agentLabel={agentLabel}
                        // git data is already on every summary (no extra poll);
                        // the censor count is fed from the event-driven map.
                        gitStatus={item.gitStatus}
                        censorCount={censorCountByProject[item.id]}
                        // Stable callback (no per-card inline closure) so the
                        // memoized ProjectCard's onSelect prop is referentially
                        // stable; the card supplies its own id on click.
                        onSelect={onSelect}
                      />
                    );
                  })
                )}
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}

// Memoized so an unrelated ProjectsView state update (e.g. a 5s agent poll that
// does not change any board input) does not re-render the whole six-column board.
// The view feeds it referentially-stable props: the grouping maps are useMemo'd,
// censorCountByProject only changes identity when a count actually changes, and
// onSelect is the stable setSelectedId setter.
export const ProjectsBoard = memo(ProjectsBoardComponent);
