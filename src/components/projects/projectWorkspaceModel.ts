// Pure, DOM-free model for the Work-mode shell (Phase D of the Projects/Agents
// IA redesign). All routing/selection/format logic lives here so ProjectWorkspace
// .tsx stays a thin JSX shell and the logic is unit-testable in node (this repo's
// vitest runs in the node env, no jsdom).
//
// NONE of these render a secret or raw value: the git top-bar line shows the
// branch name + integer counts already on ProjectGitStatus; the rail model echoes
// agent ids / roles / models the agent self-reported.

import type {
	AgentSession,
	AgentSubagent,
	GitPushRequest,
	ProjectGitStatus,
} from "../../types/backend";
import { freshestSession } from "./agentLiveStatus";
import { displayRole } from "../agents/roleDisplay";

// ---- work-mode routing (sub-state of activeView==="projects") ----------------

/** The two render branches of ProjectsView's projects sub-state. */
export type ProjectsViewMode = "board" | "work";

export interface ProjectsRouteState {
	selectedId: string | null;
	workMode: boolean;
}

/** Entering Work mode from a card click: select the project AND flip workMode on.
 *  Pure so the click handler logic is testable without React. */
export function enterWorkMode(
	_state: ProjectsRouteState,
	projectId: string,
): ProjectsRouteState {
	return { selectedId: projectId, workMode: true };
}

/** `← Board`: leave Work mode but KEEP the selection (the card stays selected on
 *  the board the user returns to). */
export function exitWorkMode(state: ProjectsRouteState): ProjectsRouteState {
	return { selectedId: state.selectedId, workMode: false };
}

/** Which branch to render. Work mode requires BOTH the flag AND a loaded current
 *  project; otherwise the board renders (so a flipped flag with no project never
 *  shows an empty full-bleed shell). */
export function projectsViewMode(
	workMode: boolean,
	hasCurrentProject: boolean,
): ProjectsViewMode {
	return workMode && hasCurrentProject ? "work" : "board";
}

// ---- top-bar git status -----------------------------------------------------

/** Sanitize a git counter to a non-negative integer (0 for missing/negative/NaN). */
function safeCount(value: number | undefined | null): number {
	if (typeof value !== "number" || !Number.isFinite(value)) return 0;
	const floored = Math.floor(value);
	return floored > 0 ? floored : 0;
}

export interface WorkspaceGitLine {
	/** "main", or "—" when no branch is known. */
	branch: string;
	dirtyCount: number;
	aheadCount: number;
	behindCount: number;
	/** dirtyCount === 0 — the working tree is fully committed. */
	committed: boolean;
	/** aheadCount === 0 — the local branch has nothing unpushed. */
	pushed: boolean;
	/** Whether the project root is a git repo at all (drives whether to show the
	 *  commit/push controls + the line). */
	isGitRepo: boolean;
	/** The compact human segments, in a stable order, e.g.
	 *  ["main", "2 modified", "↑1", "committed?: no", "pushed?: no"]. */
	segments: string[];
}

/** Build the Work-mode top-bar git line from a (possibly null/partial)
 *  ProjectGitStatus. `committed?` is derived from dirtyCount===0 and `pushed?`
 *  from aheadCount===0, exactly as the plan specifies. Renders only the branch
 *  name + integer counts — never a commit hash, upstream URL, or any raw value. */
export function workspaceGitLine(
	gitStatus: ProjectGitStatus | null | undefined,
): WorkspaceGitLine {
	const isGitRepo = Boolean(gitStatus?.isGitRepo);
	const branch =
		gitStatus?.branch && gitStatus.branch.trim().length > 0
			? gitStatus.branch.trim()
			: "—";
	const dirtyCount = safeCount(gitStatus?.dirtyCount);
	const aheadCount = safeCount(gitStatus?.aheadCount);
	const behindCount = safeCount(gitStatus?.behindCount);
	const committed = dirtyCount === 0;
	const pushed = aheadCount === 0;

	// Only show "N modified" when there is something modified — a "0 modified"
	// segment is noise (the `committed?: yes` segment already conveys a clean tree),
	// and is dropped here just like the ahead/behind segments are when zero.
	const segments: string[] = [branch];
	if (dirtyCount > 0) segments.push(`${dirtyCount} modified`);
	if (aheadCount > 0) segments.push(`↑${aheadCount}`);
	if (behindCount > 0) segments.push(`↓${behindCount}`);
	segments.push(`committed?: ${committed ? "yes" : "no"}`);
	segments.push(`pushed?: ${pushed ? "yes" : "no"}`);

	return {
		branch,
		dirtyCount,
		aheadCount,
		behindCount,
		committed,
		pushed,
		isGitRepo,
		segments,
	};
}

// ---- agent rail model -------------------------------------------------------

/** One subagent line under an agent in the rail: "label · model · ×count". */
export function subagentRailLabel(sub: AgentSubagent): string {
	const count = safeCount(sub.count);
	const model = (sub.model ?? "").trim();
	const parts = [sub.label.trim() || "subagent"];
	if (model) parts.push(model);
	if (count > 0) parts.push(`×${count}`);
	return parts.join(" · ");
}

export interface RailAgentRow {
	agentId: string;
	/** Canonical display role — pass-through of the stored role (role untangle:
	 *  orchestrator is first-class, no fold, no derived badge). */
	role: "coder" | "verifier" | "orchestrator";
	/** True exactly when the stored role is "orchestrator" (ledger truth). */
	orchestratorBadge: boolean;
	selected: boolean;
	/** The agent's reported subagents, formatted small with the "- " prefix in the
	 *  view. Empty when none reported. These are LABEL-ONLY heartbeat lines (no
	 *  PTY) — distinct from `miniChildren` below. */
	subagents: AgentSubagent[];
	/** Whether this row is itself a mini-coder SESSION (`parentAgentId` set). A mini
	 *  is a real `host="app"` PTY session, hence selectable. Top-level rows that are
	 *  minis are ORPHANS (their parent is absent — see `orphanedMini`). */
	isMini: boolean;
	/** True only for a mini whose parent session is NOT present in this project's
	 *  session list: it is surfaced at top level (never hidden) with a hint. */
	orphanedMini: boolean;
	/** Mini-coder SESSIONS nested under this (parent) row: real selectable live-PTY
	 *  child rows. Empty for a row with no mini children. Distinct from `subagents`
	 *  (label-only info). Order preserved from the input session list. */
	miniChildren: RailAgentRow[];
}

/** Whether a session is a mini-coder: an app-hosted PTY a Main coder delegated
 *  to. Non-empty `parentAgentId` is the usual signal, BUT `spawn_main_coder`
 *  also stamps parent=orch on the Main coder session (message "Main coder
 *  running") — that is NOT a mini. Label-only heartbeat `AgentSubagent`s are a
 *  different thing (no PTY) and are not minis. */
export function isMiniSession(session: AgentSession): boolean {
	const msg = (session.message ?? "").toLowerCase();
	if (msg.includes("main coder")) return false;
	if (session.role === "mini") return true;
	return (
		typeof session.parentAgentId === "string" &&
		session.parentAgentId.trim().length > 0
	);
}

/** True when the agent is driven by the mini_coder directive layer (a local mini OR a
 *  local agentic coder) and must therefore be steered/answered via `mini_coder_steer`
 *  (the directive queue), NOT by raw-writing its PTY. A mini is always mini-managed; a
 *  top-level coder is mini-managed unless its client is a cloud CLI (claude/codex), which
 *  runs as a raw PTY worker. An unknown/absent client defaults to mini-managed (the safer
 *  route: mini_coder_steer no-ops if there is no directive, whereas a raw PTY write could
 *  corrupt a process that isn't a cloud worker). */
export function isMiniManagedSession(session: AgentSession): boolean {
	if (isMiniSession(session)) return true;
	const client = (session.client ?? "").trim().toLowerCase();
	return client !== "claude" && client !== "codex" && client !== "openai";
}

/** Build a single row. `miniChildren` are assigned AT CONSTRUCTION (default empty),
 *  so a returned row object is never mutated after it is built — important for any
 *  future memo-by-identity and to avoid an aliased-mutation hazard. */
function buildRow(
	session: AgentSession,
	selectedAgentId: string | null,
	options: {
		isMini: boolean;
		orphanedMini: boolean;
		miniChildren?: RailAgentRow[];
	},
): RailAgentRow {
	const { role, orchestratorBadge } = displayRole(session);
	return {
		agentId: session.agentId,
		role,
		orchestratorBadge,
		selected: session.agentId === selectedAgentId,
		subagents: session.subagents ?? [],
		isMini: options.isMini,
		orphanedMini: options.orphanedMini,
		miniChildren: options.miniChildren ?? [],
	};
}

/** Build the rail's pure row model from the project's live sessions and the
 *  current selection. Sessions are assumed already filtered to this project by
 *  the caller (it reuses ProjectsView's sessionsByProject — NO new poller).
 *
 *  Mini-coder SESSIONS (those with a `parentAgentId` matching a present session)
 *  are NESTED under their parent's `miniChildren` instead of appearing at top
 *  level. A mini whose parent is absent (orphan) is surfaced at top level with
 *  `orphanedMini` set — never hidden. Top-level order is preserved from the input;
 *  each parent's mini children preserve their input order too. */
export function railRows(
	sessions: AgentSession[],
	selectedAgentId: string | null,
): RailAgentRow[] {
	// A mini nests only under a present, NON-mini session — its real coder parent.
	// Anything else (parent absent, or parent itself a mini — minis don't spawn
	// minis in this design) is treated as an orphan and surfaced at top level so a
	// row is NEVER silently dropped. One level of nesting only.
	const nonMiniIds = new Set(
		sessions.filter((s) => !isMiniSession(s)).map((s) => s.agentId),
	);
	const nestsUnderParent = (session: AgentSession): boolean =>
		isMiniSession(session) && nonMiniIds.has(session.parentAgentId as string);

	// WARNING 1: two-pass build so a parent row's `miniChildren` is FULLY assembled
	// BEFORE the row object is constructed — the returned rows are never mutated after
	// construction (no aliased-mutation hazard, memo-by-identity safe).
	//
	// First pass: collect each nesting mini's child row under its parent id, preserving
	// input order. A mini that does NOT nest (parent absent, or parent itself a mini)
	// is left for the second pass to surface at top level as an orphan.
	const childrenByParent = new Map<string, RailAgentRow[]>();
	for (const session of sessions) {
		if (!nestsUnderParent(session)) continue;
		const parentId = session.parentAgentId as string;
		const child = buildRow(session, selectedAgentId, {
			isMini: true,
			orphanedMini: false,
		});
		const existing = childrenByParent.get(parentId);
		if (existing) existing.push(child);
		else childrenByParent.set(parentId, [child]);
	}

	// Second pass: a top-level row for every session that does NOT nest, with its
	// children assigned at construction. Preserves input order. A top-level mini is,
	// by definition here, an orphan.
	const topLevel: RailAgentRow[] = [];
	for (const session of sessions) {
		if (nestsUnderParent(session)) continue; // emitted as a child above
		const mini = isMiniSession(session);
		topLevel.push(
			buildRow(session, selectedAgentId, {
				isMini: mini,
				orphanedMini: mini,
				miniChildren: childrenByParent.get(session.agentId),
			}),
		);
	}

	return topLevel;
}

/** The default agent to select when entering Work mode (or after the selection
 *  was pruned): the freshest project session by heartbeat, or null when none.
 *  Reuses the shared freshestSession helper so the rail agrees with the board
 *  card + status header about WHICH agent represents the project. */
export function defaultSelectedAgentId(
	sessions: AgentSession[],
): string | null {
	return freshestSession(sessions)?.agentId ?? null;
}

/** Reconcile the current selection against the live session list: keep it if it
 *  still exists, otherwise fall back. This prunes a dangling selection when the
 *  selected agent exits — mirroring the open-terminal pruning ProjectsView does.
 *  Returns the SAME string when unchanged so the caller can skip a setState.
 *
 *  Parent-aware fallback: when the disappeared selection was a MINI (looked up in
 *  `previousSessions`, the snapshot from before this reconcile), prefer selecting
 *  its PARENT if the parent is still present — a reaped mini should hand focus back
 *  to the coder that spawned it, not to an unrelated freshest agent. Only when the
 *  parent is also gone (or the selection was never a mini) does it fall back to the
 *  freshest survivor. `previousSessions` is OPTIONAL: when omitted (undefined) the
 *  parent-aware branch is SKIPPED entirely and this behaves like the pre-P3 plain
 *  freshest fallback. It must NOT default to `sessions` — doing so would look up the
 *  vanished selection in the CURRENT list (where it no longer exists, or where a
 *  recycled id could misfire the parent fallback). Only the React caller, which
 *  captures the genuinely-prior snapshot, passes a real `previousSessions`. */
export function reconcileSelectedAgentId(
	selectedAgentId: string | null,
	sessions: AgentSession[],
	previousSessions?: AgentSession[],
): string | null {
	if (
		selectedAgentId !== null &&
		sessions.some((session) => session.agentId === selectedAgentId)
	) {
		return selectedAgentId;
	}
	// The selection disappeared. If it was a mini AND we have a genuine prior snapshot,
	// try to fall back to its parent. With no prior snapshot, skip straight to freshest.
	if (selectedAgentId !== null && previousSessions !== undefined) {
		const prior = previousSessions.find((s) => s.agentId === selectedAgentId);
		if (prior && isMiniSession(prior)) {
			const parentId = prior.parentAgentId as string;
			if (sessions.some((s) => s.agentId === parentId)) {
				return parentId;
			}
		}
	}
	return defaultSelectedAgentId(sessions);
}

// ---- commit / push IPC contract --------------------------------------------
//
// Pure builders for the exact (command, args) pair ProjectsView sends over IPC,
// so a test can assert the right backend command + camelCase args WITHOUT a live
// Tauri. The backend enforces the real safety (current-branch only, never force,
// stderr surfaced, ensure_unlocked) — these only pin the call shape.

export interface IpcCall {
	command: string;
	args: Record<string, unknown>;
}

/** A minimal mutable busy flag, matching the `{ current: boolean }` shape of a
 *  React `useRef`. Lets `runGitActionGuarded` be unit-tested without React. */
export interface BusyFlag {
	current: boolean;
}

/** Reentrancy guard for the Work-mode git actions (commit/push). If `flag.current`
 *  is already set, the action is a NO-OP (returns false without running `action`),
 *  preventing a double-click or Commit-then-Push from firing two concurrent git
 *  ops on the same repo (double commit / non-fast-forward push). Otherwise it sets
 *  the flag, awaits `action`, and ALWAYS clears the flag in a finally — so a thrown
 *  action never wedges the guard. Returns true when the action ran.
 *
 *  Mirrors the `busyRef`/`agentStateInFlightRef` set-in-try / clear-in-finally
 *  pattern used throughout ProjectsView. */
export async function runGitActionGuarded(
	flag: BusyFlag,
	action: () => Promise<void>,
): Promise<boolean> {
	if (flag.current) return false;
	flag.current = true;
	try {
		await action();
	} finally {
		flag.current = false;
	}
	return true;
}

/** The IPC call for committing a project's tracked changes with `message`. */
export function commitProjectCall(projectId: string, message: string): IpcCall {
	return { command: "project_git_commit", args: { projectId, message } };
}

/** The IPC call for pushing a project's current branch (no force, backend-side). */
export function pushProjectCall(projectId: string): IpcCall {
	return { command: "project_git_push", args: { projectId } };
}

/** The IPC call for pulling a project's current branch (fast-forward only,
 *  backend-side). On a divergence the backend leaves the tree clean and returns
 *  Err(git message) telling the user to resolve manually. */
export function pullProjectCall(projectId: string): IpcCall {
	return { command: "project_git_pull", args: { projectId } };
}

/** CLIENT-SIDE shape check for a pasted GitHub clone URL. This is ONLY a fast UX
 *  gate so the Clone button can disable / show inline feedback before a round-trip;
 *  the BACKEND (`parse_github_repo` in project_git_clone) is the real authority and
 *  re-validates with the canonical, stricter parser. We accept the common remote
 *  shapes the backend accepts (https / http github.com, `git@github.com:` SSH,
 *  `ssh://git@github.com/`) with a non-empty `owner/repo` path. Returns false for
 *  empty input, a non-github host, or a missing owner/repo. */
export function isLikelyGithubRepoUrl(value: string): boolean {
	const raw = value.trim().replace(/\.git$/, "");
	if (raw === "") return false;
	// Normalize the SSH / scp-like shapes the backend also rewrites to https.
	let normalized = raw;
	if (raw.startsWith("git@github.com:")) {
		normalized = `https://github.com/${raw.slice("git@github.com:".length)}`;
	} else if (raw.startsWith("ssh://git@github.com/")) {
		normalized = `https://github.com/${raw.slice("ssh://git@github.com/".length)}`;
	} else if (raw.startsWith("http://github.com/")) {
		normalized = `https://github.com/${raw.slice("http://github.com/".length)}`;
	}
	let url: URL;
	try {
		url = new URL(normalized);
	} catch {
		return false;
	}
	if (url.protocol !== "https:" || url.hostname !== "github.com") return false;
	// The filter already drops empty segments, so length >= 2 alone guarantees a
	// non-empty owner + repo (the prior per-segment `!== ""` checks were dead code).
	const segments = url.pathname.split("/").filter((s) => s.length > 0);
	return segments.length >= 2;
}

/** The IPC call for cloning a GitHub repository and registering it as a project.
 *  The token is NEVER sent here — the backend injects it via GIT_ASKPASS. We pass
 *  the raw pasted URL (backend re-validates + rebuilds a credential-free URL) and
 *  an optional destination parent folder. camelCase over IPC. */
export function cloneProjectCall(url: string, destParent?: string): IpcCall {
	const trimmed = url.trim();
	const dest = destParent?.trim();
	return {
		command: "project_git_clone",
		args: dest ? { url: trimmed, destParent: dest } : { url: trimmed },
	};
}

/** MC-P7 + Slice 5a: whether the work-mode Compact button should show for this session.
 *  GATED on the RESOLVED built-in client being EXACTLY "claude" OR "codex"
 *  (case-insensitive after trim). Claude -> its PTY `/compact` slash command;
 *  codex -> the app-server `thread/compact/start` JSON-RPC. Meaningless to a
 *  powershell/ollama-mini/custom CLI, so those stay false. This is an EXACT match,
 *  never a substring: a custom client whose id merely CONTAINS "claude"/"codex"
 *  (e.g. "claudex") must NOT trip it — the reserved built-in ids can never be
 *  shadowed by a custom client (validateCustomClient rejects the reserved ids), so
 *  exact equality is the right and only safe test. Empty/absent client -> false. */
export function shouldShowCompact(session: AgentSession): boolean {
	// EXACT match (never substring): "claude" → its PTY `/compact` slash command;
	// "codex" → the Codex app-server `thread/compact/start` JSON-RPC (Slice 5a), which
	// only takes effect for a live duplex session (the backend command no-ops with a
	// clear error for a non-duplex Codex session, so showing the button is safe).
	const client = (session.client ?? "").trim().toLowerCase();
	return client === "claude" || client === "codex" || client === "openai";
}

/** MC-P7 + Slice 5a: the IPC call that compacts the selected agent's context.
 *  GATED: returns null unless `shouldShowCompact(session)`.
 *  - Claude: reuses the EXISTING `agent_pty_write` path — writes `/compact\n` (the
 *    slash command + carriage return) to the agent's terminal.
 *  - Codex: invokes `project_cloud_compact` which sends `thread/compact/start` over the
 *    live app-server JSON-RPC stream (only meaningful for a duplex session; the backend
 *    returns a clear error otherwise).
 *  camelCase over IPC. No token/secret — fixed literals only. */
export function compactWriteCall(session: AgentSession): IpcCall | null {
	if (!shouldShowCompact(session)) return null;
	const client = (session.client ?? "").trim().toLowerCase();
	if (client === "codex") {
		return {
			command: "project_cloud_compact",
			args: { agentId: session.agentId },
		};
	}
	return {
		command: "agent_pty_write",
		args: { agentId: session.agentId, data: "/compact\n" },
	};
}

/** MC-P5: the IPC call for the human Stop (kill) safety brake on a mini-coder.
 *  GATED: returns null for a non-mini session (a session WITHOUT a parentAgentId) —
 *  the 1-click kill is only ever wired for a mini, so the caller renders no Stop
 *  button and fires no invoke for a normal agent. For a mini it returns the
 *  `mini_coder_kill` call with the mini's agentId (camelCase over IPC). The backend
 *  records killRequested THEN kills the PTY so the executor finalizes it as
 *  aborted_by_human and the parent coder is told to escalate. */
export function miniKillCall(session: AgentSession): IpcCall | null {
	if (!isMiniSession(session)) return null;
	return { command: "mini_coder_kill", args: { agentId: session.agentId } };
}

// ---- bottom dock ------------------------------------------------------------

export type DockTab = "tasks" | "censor" | "git" | "plans" | "mcp" | "changes" | "notes" | "project";

/** The dock's default-selected tab. Tasks is the star of the Work-mode dock —
 *  the project's Kanban board is the first thing the user sees. */
export const DEFAULT_DOCK_TAB: DockTab = "tasks";

export const DOCK_TABS: { id: DockTab; label: string }[] = [
	{ id: "tasks", label: "Tasks" },
	{ id: "censor", label: "Censor" },
	{ id: "git", label: "Git" },
	{ id: "changes", label: "Changes" },
	{ id: "plans", label: "Plans" },
	{ id: "notes", label: "Notes" },
	// NOTE: the standalone "Console" dock tab was merged into the unified Work Console
	// (FocusStage Activity view) and removed — no duplicate structured-console surface.
	// Project-scoped user MCP servers (Phase A.3).
	{ id: "mcp", label: "MCP" },
	// Project detail (status header + root editor + saved workflows).
	{ id: "project", label: "Project" },
];

// ---- plan approval model ---------------------------------------------------

/** True for a plan request the human still has to decide. */
export function isPendingPlanRequest(
	request: import("../../types/backend").PlanApprovalRequest,
): boolean {
	return request.status === "pending_approval";
}

/** Pending plan approval requests for ONE project, oldest-first.
 *  Pure / node-safe so the card and its tests stay a thin shell. */
export function pendingPlanRequestsForProject(
	requests:
		| import("../../types/backend").PlanApprovalRequest[]
		| null
		| undefined,
	projectId: string,
): import("../../types/backend").PlanApprovalRequest[] {
	if (!requests || !projectId) return [];
	return requests
		.filter((r) => r.projectId === projectId && isPendingPlanRequest(r))
		.slice()
		.sort((a, b) => {
			const byTime = (a.createdAt ?? "").localeCompare(b.createdAt ?? "");
			return byTime !== 0 ? byTime : (a.id ?? "").localeCompare(b.id ?? "");
		});
}

// ---- GH-P4: agent push-approval gate -----------------------------------------

/** True for a push request the human still has to answer (pending_approval). The
 *  PushApprovalCard shows ONLY these. An `approved`/`pushing` request is in flight
 *  (no buttons), and a terminal one is done. */
export function isPendingPushRequest(request: GitPushRequest): boolean {
	return request.status === "pending_approval";
}

/** The pending push requests for ONE project, oldest first (stable createdAt sort).
 *  Filters out other projects and any non-pending request. Pure so the card stays a
 *  thin shell and the selection is unit-testable in node. */
export function pendingPushRequestsForProject(
	requests: GitPushRequest[] | null | undefined,
	projectId: string,
): GitPushRequest[] {
	if (!requests || !projectId) return [];
	return requests
		.filter((r) => r.projectId === projectId && isPendingPushRequest(r))
		.slice()
		.sort((a, b) => {
			const byTime = (a.createdAt ?? "").localeCompare(b.createdAt ?? "");
			return byTime !== 0 ? byTime : (a.id ?? "").localeCompare(b.id ?? "");
		});
}

/** A human-readable one-line summary of WHAT a push request will do, for the card.
 *  NEVER renders a token/URL — only the agent id, the (display-only) branch, the
 *  remote name, and a FORCE marker. */
export function pushRequestSummary(request: GitPushRequest): string {
	const target = request.branch?.trim() || "current branch";
	const remote = request.remote?.trim() || "origin";
	const force = request.force ? " (FORCE)" : "";
	return `${target} → ${remote}${force}`;
}
