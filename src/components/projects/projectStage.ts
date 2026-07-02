// Shared project-stage derivation used by both ProjectsView (which owns the
// data fetching + polling) and the extracted ProjectsBoard (which renders the
// macro KANBAN columns). Kept in its own module so the presentational board can
// import the stage types/helpers without pulling the whole view back in.
//
// IMPORTANT: the stage-derivation logic below is relocated byte-for-byte from
// ProjectsView. Do not change how a project is assigned to a stage.

import {
  AlertCircle,
  CheckCircle2,
  Circle,
  Clock3,
  ShieldCheck,
  SquareTerminal,
} from "lucide-react";
import type { AgentClaim, AgentSession, ProjectSummary } from "../../types/backend";
import {
  isActiveProjectSession,
  isOpenClaim,
  isRecentProjectSession,
  isWorkingClaim,
} from "../../utils/agentClaims";
import { sessionHealth } from "./agentLiveStatus";

export type ProjectStageId =
  | "planned"
  | "launching"
  | "active"
  | "review"
  | "blocked"
  | "verified";

export const projectStages: {
  id: ProjectStageId;
  label: string;
  icon: typeof Circle;
}[] = [
  { id: "planned", label: "Planned", icon: Circle },
  { id: "launching", label: "Launching", icon: SquareTerminal },
  { id: "active", label: "Active", icon: Clock3 },
  { id: "review", label: "Review", icon: ShieldCheck },
  { id: "blocked", label: "Blocked", icon: AlertCircle },
  { id: "verified", label: "Verified", icon: CheckCircle2 },
];

export const projectStageTitles: Record<ProjectStageId, string> = {
  planned: "No recent agent launch, session, claim or work in progress.",
  launching: "A terminal was launched and the agent has not registered yet.",
  active: "A working session, task claim or WIP task is present.",
  review: "At least one task is waiting for verifier closure.",
  blocked:
    "Project is paused, archived, or blocked without active review/work.",
  verified: "Project is done or every task is done.",
};

export const stageTone: Record<ProjectStageId, string> = {
  planned: "bg-cream-100 text-cream-500",
  launching: "bg-amber/10 text-amber-dark",
  active: "bg-teal/10 text-teal",
  review: "bg-sage/10 text-sage-dark",
  blocked: "bg-coral/10 text-coral-dark",
  verified: "bg-sage/10 text-sage-dark",
};

export function stageLabel(stage: ProjectStageId) {
  return projectStages.find((item) => item.id === stage)?.label ?? stage;
}

function sessionStatus(session: AgentSession) {
  return session.status.toLowerCase();
}

function isReviewProjectSession(session: AgentSession, now = Date.now()) {
  return (
    isRecentProjectSession(session, now) && sessionStatus(session) === "review"
  );
}

function isBlockedProjectSession(session: AgentSession, now = Date.now()) {
  return (
    isRecentProjectSession(session, now) && sessionStatus(session) === "blocked"
  );
}

function isLaunchingProjectSession(session: AgentSession, now = Date.now()) {
  // BUG #19: agree with the agent dot (sessionHealth) — a launch_pending that
  // has gone stale (never registered within LAUNCH_PENDING_STALE_MS) no longer
  // counts as "launching", so the project reverts to "planned" instead of
  // hanging on "launching" for the full activity window.
  return (
    isRecentProjectSession(session, now) &&
    sessionHealth(session, now) === "pending"
  );
}

export function projectStage(
  project: ProjectSummary,
  claims: AgentClaim[],
  sessions: AgentSession[],
  now = Date.now(),
): ProjectStageId {
  if (
    project.status === "done" ||
    (project.taskCounts.total > 0 &&
      project.taskCounts.done === project.taskCounts.total)
  ) {
    return "verified";
  }
  if (project.status === "paused" || project.status === "archived")
    return "blocked";
  // The planner/orchestrator session is the create-time CONVERSATION, not
  // project work — it must never pull the card into Active/Launching (that was
  // the "plan goes active by itself" bug: chatting with the planner registered
  // a session and the stage recomputed on the next remount). Only worker-tier
  // sessions (coder/verifier/mini) drive the stage.
  const workSessions = sessions.filter(
    (session) => session.role !== "orchestrator",
  );
  const working = claims.filter((claim) => isWorkingClaim(claim, now));
  const reviewClaims = claims.filter(
    (claim) => isOpenClaim(claim, now) && claim.status === "review",
  );
  const blockedClaims = claims.filter(
    (claim) => isOpenClaim(claim, now) && claim.status === "blocked",
  );
  const activeSessions = workSessions.filter((session) =>
    isActiveProjectSession(session, now),
  );
  const reviewSessions = workSessions.filter((session) =>
    isReviewProjectSession(session, now),
  );
  const blockedSessions = workSessions.filter((session) =>
    isBlockedProjectSession(session, now),
  );
  const launchingSessions = workSessions.filter((session) =>
    isLaunchingProjectSession(session, now),
  );
  if (project.taskCounts.wip > 0 || activeSessions.length > 0) return "active";
  if (working.length > 0) return "active";
  if (launchingSessions.length > 0) return "launching";
  if (
    project.taskCounts.blocked > 0 ||
    blockedClaims.length > 0 ||
    blockedSessions.length > 0
  )
    return "blocked";
  if (
    project.taskCounts.review > 0 ||
    reviewClaims.length > 0 ||
    reviewSessions.length > 0
  )
    return "review";
  return "planned";
}
