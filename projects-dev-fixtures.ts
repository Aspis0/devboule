// Mock fixtures for the Projects dev harness (projects-dev.tsx / projects-dev.html).
//
// Pure, dev-only data. Every object below is a valid instance of the REAL
// backend types — no `any`, no casts — so the presentational Projects
// components render exactly as they would against live Tauri data, but in a
// plain browser with no backend, no login, and no Tauri invoke.
//
// SECURITY: this module must never be imported by the shipping app. It is only
// pulled in by the root projects-dev.tsx entry, which is itself only served by
// the Vite dev server and is NOT a production rollup input (see vite.config.ts).

import type {
  AgentClaim,
  AgentEvent,
  AgentSession,
  ProjectDetail,
  ProjectGitStatus,
  ProjectSummary,
  ProjectTask,
} from "./src/types/backend";

// ---------------------------------------------------------------------------
// Git status fixtures (every ProjectSummary / ProjectDetail needs one)
// ---------------------------------------------------------------------------

function gitStatus(
  overrides: Partial<ProjectGitStatus> = {},
): ProjectGitStatus {
  return {
    rootPath: "C:/Users/gualt/Desktop/Devboule",
    repoRoot: "C:/Users/gualt/Desktop/Devboule",
    repoName: "devboule",
    branch: "main",
    upstream: "origin/main",
    origin: "git@github.com:aspis/devboule.git",
    githubUrl: "https://github.com/aspis/devboule",
    cloneCommand: "git clone git@github.com:aspis/devboule.git",
    pullRequestUrl: null,
    commit: "a1b2c3d",
    dirtyCount: 2,
    stagedCount: 1,
    unstagedCount: 1,
    untrackedCount: 0,
    aheadCount: 1,
    behindCount: 0,
    isGitRepo: true,
    isGithub: true,
    policyStatus: "ready",
    warnings: [],
    requiredActions: [],
    suggestedRepos: [],
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// ProjectSummary fixtures — feed the macro stage-board ProjectCard previews
// ---------------------------------------------------------------------------

export const summaryActiveNoAgent: ProjectSummary = {
  id: "proj-bio",
  title: "Devboule — sequencing pipeline",
  status: "active",
  updatedAt: "2026-05-29T14:12:00Z",
  rootPath: "C:/Users/gualt/Desktop/aspis-bio",
  revision: "rev-101",
  path: "C:/Users/gualt/Desktop/aspis-bio/PROJECT.md",
  taskCounts: { todo: 3, wip: 0, review: 1, blocked: 0, done: 3, total: 7 },
  gitStatus: gitStatus({ repoName: "aspis-bio" }),
};

export const summaryActiveWithAgent: ProjectSummary = {
  id: "proj-edge",
  title: "Edge worker rollout",
  status: "active",
  updatedAt: "2026-05-30T09:41:00Z",
  rootPath: "C:/Users/gualt/Desktop/edge-worker",
  revision: "rev-204",
  path: "C:/Users/gualt/Desktop/edge-worker/PROJECT.md",
  taskCounts: { todo: 1, wip: 2, review: 1, blocked: 0, done: 4, total: 8 },
  gitStatus: gitStatus({ repoName: "edge-worker", policyStatus: "warning" }),
};

export const summaryPaused: ProjectSummary = {
  id: "proj-billing",
  title: "Billing reconciliation",
  status: "paused",
  updatedAt: "2026-05-21T11:05:00Z",
  rootPath: "C:/Users/gualt/Desktop/billing",
  revision: "rev-58",
  path: "C:/Users/gualt/Desktop/billing/PROJECT.md",
  taskCounts: { todo: 4, wip: 0, review: 0, blocked: 1, done: 2, total: 7 },
  gitStatus: gitStatus({ repoName: "billing", policyStatus: "blocked" }),
};

export const summaryLaunching: ProjectSummary = {
  id: "proj-telemetry",
  title: "Telemetry ingest spike",
  status: "active",
  updatedAt: "2026-05-30T09:55:00Z",
  rootPath: "C:/Users/gualt/Desktop/telemetry",
  revision: "rev-12",
  path: "C:/Users/gualt/Desktop/telemetry/PROJECT.md",
  taskCounts: { todo: 2, wip: 0, review: 0, blocked: 0, done: 0, total: 2 },
  gitStatus: gitStatus({ repoName: "telemetry" }),
};

export const summaryReview: ProjectSummary = {
  id: "proj-portal",
  title: "Customer portal refresh",
  status: "active",
  updatedAt: "2026-05-30T08:30:00Z",
  rootPath: "C:/Users/gualt/Desktop/portal",
  revision: "rev-77",
  path: "C:/Users/gualt/Desktop/portal/PROJECT.md",
  taskCounts: { todo: 1, wip: 0, review: 2, blocked: 0, done: 4, total: 7 },
  gitStatus: gitStatus({ repoName: "portal", policyStatus: "warning" }),
};

export const summaryDone: ProjectSummary = {
  id: "proj-migrate",
  title: "DB migration v3",
  status: "done",
  updatedAt: "2026-05-18T16:30:00Z",
  rootPath: "C:/Users/gualt/Desktop/db-migrate",
  revision: "rev-90",
  path: "C:/Users/gualt/Desktop/db-migrate/PROJECT.md",
  taskCounts: { todo: 0, wip: 0, review: 0, blocked: 0, done: 6, total: 6 },
  gitStatus: gitStatus({ repoName: "db-migrate" }),
};

// ---------------------------------------------------------------------------
// Board task fixtures — one task per Kanban column, incl. an agent-controlled
// task whose open claim/session must disable the "Sposta" menu and show a chip.
// ---------------------------------------------------------------------------

export const taskTodo: ProjectTask = {
  id: "T-101",
  title: "Define ingestion schema for FASTQ batches",
  status: "todo",
  priority: "high",
  assignee: "user",
  due: "2026-06-05",
  linkedResources: [],
  updatedAt: "2026-05-29T10:00:00Z",
};

// Agent-controlled WIP task: an open claim + active session target this id, so
// the harness marks it agentControlled (chip shown, Sposta menu disabled).
export const taskWipAgent: ProjectTask = {
  id: "T-102",
  title: "Wire MCP claim updates into the board",
  status: "wip",
  priority: "medium",
  assignee: "coder-7f",
  due: null,
  linkedResources: [
    { provider: "cloudflare", resourceId: "wk-9921", label: "ingest-worker" },
  ],
  updatedAt: "2026-05-30T09:38:00Z",
};

export const taskReview: ProjectTask = {
  id: "T-103",
  title: "Audit Scaleway bucket retention policy",
  status: "review",
  priority: "medium",
  assignee: "verifier-2a",
  due: "2026-06-01",
  linkedResources: [
    { provider: "scaleway", resourceId: "bkt-441", label: "raw-reads" },
  ],
  updatedAt: "2026-05-30T08:10:00Z",
};

export const taskBlocked: ProjectTask = {
  id: "T-104",
  title: "Rotate provider token (waiting on key)",
  status: "blocked",
  priority: "high",
  assignee: null,
  due: null,
  linkedResources: [],
  updatedAt: "2026-05-28T19:22:00Z",
};

export const taskDone: ProjectTask = {
  id: "T-105",
  title: "Scaffold project Markdown writer",
  status: "done",
  priority: "low",
  assignee: "user",
  due: null,
  linkedResources: [],
  updatedAt: "2026-05-27T13:00:00Z",
};

export const boardTasks: ProjectTask[] = [
  taskTodo,
  taskWipAgent,
  taskReview,
  taskBlocked,
  taskDone,
];

// The set of task ids that an agent currently controls (open claim or active
// session). The harness uses this to drive the TaskCard `agentControlled` flag.
export const agentControlledTaskIds = new Set<string>([taskWipAgent.id]);

// ---------------------------------------------------------------------------
// Agent live state fixtures — sessions / claims / events for ProjectAgentPanel
// ---------------------------------------------------------------------------

// Relative timestamps so the live status (heartbeat age) renders correctly
// whenever the harness is opened, instead of going permanently "stalled"
// against a hardcoded past date.
const minutesAgo = (minutes: number) =>
  new Date(Date.now() - minutes * 60 * 1000).toISOString();

// Two concurrent Codex agents on DIFFERENT tasks (distinct agent ids, both
// fresh heartbeats -> "working") plus one Claude that went silent ~14 min ago
// (-> "reconnect", with a Recovery affordance). This exercises every new panel
// surface: CLI badges, multi-agent rows, live status, heartbeat age, recovery.
export const activeSession: AgentSession = {
  agentId: "coder-7f",
  role: "coder",
  model: "gpt-5-codex",
  status: "wip",
  client: "codex",
  message: "Editing board move handler",
  currentProjectId: "proj-edge",
  currentTaskId: "T-102",
  firstSeenAt: minutesAgo(21),
  lastSeenAt: minutesAgo(0),
};

export const secondCodexSession: AgentSession = {
  agentId: "coder-3c",
  role: "coder",
  model: "gpt-5-codex",
  status: "wip",
  client: "codex",
  message: "Defining FASTQ ingestion schema",
  currentProjectId: "proj-edge",
  currentTaskId: "T-101",
  firstSeenAt: minutesAgo(12),
  lastSeenAt: minutesAgo(1),
};

export const stalledClaudeSession: AgentSession = {
  agentId: "verifier-2a",
  role: "verifier",
  model: "claude-sonnet-4-6",
  status: "review",
  client: "claude",
  message: "Auditing Scaleway bucket retention",
  currentProjectId: "proj-edge",
  currentTaskId: "T-103",
  firstSeenAt: minutesAgo(40),
  lastSeenAt: minutesAgo(14),
};

// All concurrent sessions for the selected project, in the order the panel
// should list them.
export const projectSessions: AgentSession[] = [
  activeSession,
  secondCodexSession,
  stalledClaudeSession,
];

export const activeClaim: AgentClaim = {
  projectId: "proj-edge",
  projectTitle: "Edge worker rollout",
  taskId: "T-102",
  taskTitle: "Wire MCP claim updates into the board",
  agentId: "coder-7f",
  role: "coder",
  status: "wip",
  claimedAt: "2026-05-30T09:21:00Z",
  updatedAt: "2026-05-30T09:40:00Z",
  leaseUntil: "2026-05-30T10:40:00Z",
  evidence: null,
};

export const activeEvent: AgentEvent = {
  id: "ev-551",
  timestamp: "2026-05-30T09:39:00Z",
  agentId: "coder-7f",
  role: "coder",
  eventType: "task.progress",
  projectId: "proj-edge",
  taskId: "T-102",
  status: "wip",
  message: "Claimed T-102 and started wiring the MiniMenu move targets.",
  evidence: null,
};

// A handful of recent MCP events per live agent so the per-row "Activity" mini
// feed renders something for every agent. Timestamps are relative so the feed
// stays plausibly "live" every time the harness is opened. Includes the three
// live agents from projectSessions (coder-7f, coder-3c, verifier-2a).
export const projectAgentEvents: AgentEvent[] = [
  activeEvent,
  {
    id: "ev-552",
    timestamp: minutesAgo(1),
    agentId: "coder-7f",
    role: "coder",
    eventType: "task.progress",
    projectId: "proj-edge",
    taskId: "T-102",
    status: "wip",
    message: "Move targets wired; running the board reducer tests.",
    evidence: null,
  },
  {
    id: "ev-553",
    timestamp: minutesAgo(0),
    agentId: "coder-7f",
    role: "coder",
    eventType: "agent.heartbeat",
    projectId: "proj-edge",
    taskId: "T-102",
    status: "wip",
    message: "Heartbeat OK; still editing the board move handler.",
    evidence: null,
  },
  {
    id: "ev-560",
    timestamp: minutesAgo(11),
    agentId: "coder-3c",
    role: "coder",
    eventType: "task.claim",
    projectId: "proj-edge",
    taskId: "T-101",
    status: "wip",
    message: "Claimed T-101 (FASTQ ingestion schema).",
    evidence: null,
  },
  {
    id: "ev-561",
    timestamp: minutesAgo(2),
    agentId: "coder-3c",
    role: "coder",
    eventType: "task.progress",
    projectId: "proj-edge",
    taskId: "T-101",
    status: "wip",
    message: "Drafted the ingestion schema; validating against sample reads.",
    evidence: null,
  },
  {
    id: "ev-570",
    timestamp: minutesAgo(40),
    agentId: "verifier-2a",
    role: "verifier",
    eventType: "task.claim",
    projectId: "proj-edge",
    taskId: "T-103",
    status: "review",
    message: "Claimed T-103 to audit Scaleway bucket retention.",
    evidence: null,
  },
  {
    id: "ev-571",
    timestamp: minutesAgo(14),
    agentId: "verifier-2a",
    role: "verifier",
    eventType: "agent.heartbeat",
    projectId: "proj-edge",
    taskId: "T-103",
    status: "review",
    message: "Last heartbeat before the connection went silent.",
    evidence: null,
  },
];

// ---------------------------------------------------------------------------
// ProjectDetail fixture — drives ProjectStatusHeader + the detail layout.
// State carries the full board task list; progress reads 3/8 done.
// ---------------------------------------------------------------------------

export const projectDetail: ProjectDetail = {
  metadata: {
    id: "proj-edge",
    title: "Edge worker rollout",
    status: "active",
    updatedAt: "2026-05-30T09:41:00Z",
    rootPath: "C:/Users/gualt/Desktop/edge-worker",
  },
  state: {
    version: 4,
    tasks: boardTasks,
    notes: [
      {
        id: "note-1",
        text: "Coordinate token rotation with the secrets-rotator profile before resuming T-104.",
        source: "user",
        createdAt: "2026-05-29T18:00:00Z",
      },
    ],
  },
  markdown: "# Edge worker rollout\n\nMock project markdown for the dev harness.\n",
  revision: "rev-204",
  path: "C:/Users/gualt/Desktop/edge-worker/PROJECT.md",
  modifiedAt: "2026-05-30T09:41:00Z",
  liveStatus: {
    resources: [
      {
        provider: "cloudflare",
        resourceId: "wk-9921",
        label: "ingest-worker",
        status: "deployed",
        resourceType: "worker",
        region: null,
      },
    ],
    checkedAt: "2026-05-30T09:30:00Z",
  },
  gitStatus: gitStatus({ repoName: "edge-worker", policyStatus: "warning" }),
};

// Progress shown by ProjectStatusHeader: 3 done out of 8 total (per the prompt).
export const detailTaskCounts = {
  todo: 1,
  wip: 2,
  review: 1,
  blocked: 1,
  done: 3,
  total: 8,
} as const;
