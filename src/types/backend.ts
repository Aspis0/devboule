export interface AuthState {
	locked: boolean;
	helloAvailable: boolean;
	lastUnlockedAt: string | null;
	lockReason:
		| "startup"
		| "manual"
		| "idle"
		| "windows_resume"
		| "unavailable"
		| null;
	/** True when DEVBOULE_DEV_UNLOCK was active at backend startup (stable for the process). */
	devUnlock: boolean;
}

/**
 * Generic vault credential health (websearch, cloud LLM keys, Claude OAuth, etc.).
 * Not tied to the removed Cloudflare/Scaleway ProviderId inventory.
 */
export interface AuxCredentialStatus {
	id: string;
	label: string;
	configured: boolean;
	status: "missing" | "configured" | "error";
	lastCheckedAt: string | null;
	message: string | null;
}

export interface OracleDuplicateLabel {
	label: string;
	nodeIds: string[];
}

export interface OracleSnapshot {
	status: string;
	source: string;
	phase: string;
	nodeCount: number;
	edgeCount: number;
	clusterCount: number;
	duplicateLabels: OracleDuplicateLabel[];
}

export interface OracleCoverage {
	totalNodes: number;
	oracleNodes: number;
	oraclePercent: number;
}

export interface OracleRuntimeVectorStore {
	backend: string;
	path: string;
	records: number;
	ready: boolean;
}

// The REAL dense-retrieval index (chunks.lancedb + the SQLite chunk table).
// Readiness and the file/chunk counts shown in the UI come from HERE — the
// legacy node-level vectorStore (vectors.lancedb) is no longer produced and is
// typically empty. Fields are optional-by-default on the wire (an older sidecar
// may omit the block); the Rust layer defaults them to zero/false.
export interface OracleRuntimeChunkStore {
	backend: string;
	path: string;
	files: number;
	records: number;
	vectorRecords: number;
	ready: boolean;
}

// VESTIGIAL: kept so the OracleRuntime wire payload shape stays stable. The
// local Ollama chat path has been removed (answers are API-only); this object
// is always populated empty/disabled and is no longer rendered in the UI.
export interface OracleRuntimeOllama {
	cli: string | null;
	server: string;
	model: string;
	modelAvailable: boolean;
	models: string[];
	message: string | null;
}

export interface OracleRuntime {
	vectorStore: OracleRuntimeVectorStore;
	// The REAL dense-retrieval index status — authoritative for readiness and for
	// the file/chunk counts the UI shows.
	chunkStore: OracleRuntimeChunkStore;
	// Top-level retrieval readiness, mirrored from chunkStore.ready by the sidecar.
	ready: boolean;
	// Vestigial: kept for wire-payload compatibility; not rendered.
	ollama: OracleRuntimeOllama;
	setupCommands: string[];
}

export interface OracleRuntimeSetup {
	pythonFound: boolean;
	pythonCommand: string | null;
	pythonVersion: string | null;
	venvReady: boolean;
	depsReady: boolean;
	embedderReady: boolean;
	/** Everything the retrieval layer needs is installed. */
	ready: boolean;
	embedModel: string;
	messages: string[];
	/**
	 * ADDITIVE / OPTIONAL: the backend Python probe is still inconclusive (timed
	 * out or lost the spawn race on a busy machine) — `pythonFound: false` here
	 * does NOT mean Python is genuinely missing. The Rust side sets this on a
	 * tri-state probe; older builds omit it, so the UI MUST treat an absent field
	 * as "not checking" and fall back to message-sniffing. Never assume present.
	 */
	checking?: boolean;
	/**
	 * ADDITIVE / OPTIONAL: this package ships the int8 ONNX embedder weights
	 * (full build). Lite packages omit weights and download on first install.
	 * Explicit `false` always wins. Absent → UI may fall back to `bundleKind`.
	 */
	embedderBundled?: boolean;
	/**
	 * ADDITIVE / OPTIONAL: `"full"` | `"lite"` | `"unknown"`. Older backends
	 * omit it. Used only when `embedderBundled` is absent (not when false).
	 */
	bundleKind?: string;
}

export interface OracleLlmSettings {
	provider: string;
	model: string;
	baseUrl: string | null;
	remoteEnabled: boolean;
}

export interface OracleLlmSettingsStatus {
	settings: OracleLlmSettings;
	apiKeyConfigured: boolean;
	status: string;
	message: string | null;
}

export interface OracleIndexPreferences {
	autoWatchOnUnlock: boolean;
	indexRoot: string | null;
	/** "watch" | "commit". Absent means watch (default). */
	indexMode?: "watch" | "commit";
}

export interface OracleIndexJob extends Record<string, unknown> {
	// Live sub-state while status === "running": "running" | "cooling_gpu" |
	// "waiting_memory". A short, path-free human label accompanies the non-normal
	// phases (e.g. "GPU cooling (85°C), resuming…"). Both are optional and only
	// present while a job is active; older servers omit them.
	phase?: string;
	phaseMessage?: string;
}

export interface OracleIndexStatus {
	job: OracleIndexJob | null;
	watcherRunning: boolean;
	index: {
		root: string;
		expectedFiles: number;
		indexedFiles: number;
		pendingFiles: number;
		staleFiles: number;
		sqliteChunkFiles: number;
		sqliteChunks: number;
		vectorRecords: number;
		firstPending: string[];
		firstStale: string[];
		freeRamGb: number;
	};
}

// "draft": planner-created, plan not yet approved — kept OFF the kanban board;
// plan approval (backend) promotes it to "active".
export type ProjectStatus = "draft" | "active" | "paused" | "done" | "archived";
export type ProjectTaskStatus = "todo" | "wip" | "review" | "blocked" | "done";
export type ProjectEditableTaskStatus = Exclude<ProjectTaskStatus, "done">;
// ROLE UNTANGLE (2026-07): FOUR first-class roles — orchestrator (plans+delegates,
// never writes), coder ("Main coder"), verifier, mini. The ledger stores them
// truthfully (a planner launch persists role:"orchestrator" whether local Devboule
// binary or cloud CLI in duplex mode); displayRole() is a pass-through, not a fold.
// Mirrors VALID_ROLES in aspis_mcp.py and backend/agent_role.rs (the ONE fold —
// only legacy aliases architect/code fold to coder). "mini" sessions are typed via
// their own surfaces, not this union.
export type AgentRole = "orchestrator" | "coder" | "verifier";
// The roles a NEW agent can be spawned with from the SpawnPanel role radio. The
// orchestrator is launched via the planner ("Plan it") or the Devboule client
// selection, not the radio — the Roles-table phase may widen this.
export type SpawnRole = "coder" | "verifier";
export type AgentClaimStatus =
	| ProjectTaskStatus
	| "claimed"
	| "provider_action_pending";

/**
 * Per-project sandbox autonomy level.
 *
 * Maps to the Rust enum `SandboxMode` (serde rename_all = "camelCase"):
 *   Ask                   → "ask"
 *   AutoAcceptInWorkspace → "autoAcceptInWorkspace"
 *   Unattended            → "unattended"
 *
 * The backend omits the field (skip_serializing_if) when the value is the
 * default "ask", so the frontend MUST treat `undefined` as "ask".
 */
export type SandboxMode = "ask" | "autoAcceptInWorkspace" | "unattended";

export interface ProjectMetadata {
	id: string;
	title: string;
	status: ProjectStatus;
	updatedAt: string;
	rootPath: string | null;
	/** Whether network access is enabled for agentic minis in this project. */
	netEnabled?: boolean;
	/** Whether the Censor reviewer is trusted (opted in) for this project. */
	censorTrusted?: boolean;
	/**
	 * Per-project sandbox autonomy level. Absent means "ask" (the default).
	 * The backend skips serializing this field when it equals the default, so
	 * `undefined` and `"ask"` are semantically identical on the frontend.
	 */
	sandboxMode?: SandboxMode;
	/**
	 * Extra writable folders outside the project root granted via the permission
	 * broker (Slice 2 FolderWrite consent). The backend omits this field when the
	 * list is empty (NO-CHURN); treat `undefined` as `[]`.
	 */
	workingSet?: string[];
	/**
	 * Slice 5c: per-project default agent capability/cost controls for the cloud coders.
	 * Omitted by the backend when every field is unset (NO-CHURN); treat `undefined` as `{}`.
	 */
	agentControls?: AgentControls;
	/**
	 * Role untangle (P6b): per-project OVERRIDE of the Main-coder engine (the hand-off
	 * dropdown). Absent means "use the global RolesConfig default". An opaque engine/client id
	 * ("claude" | "codex" | a custom id). Set via set_project_main_coder_override_cmd.
	 */
	mainCoder?: string;
}

/**
 * Slice 5c: per-project (or per-launch) capability/cost controls for the cloud coding CLIs.
 * All optional — absent means "leave the CLI default". The PERMISSION axis is `sandboxMode`,
 * not here. Mirrors `AgentControls` in src-tauri/src/backend/model.rs.
 */
export interface AgentControls {
	/** Reasoning effort. Claude low/medium/high/xhigh/max; Codex minimal…ultra. */
	effort?: string;
	/** Extra system-prompt text (Claude --append-system-prompt / Codex developerInstructions). */
	systemPrompt?: string;
	/** Max agent turns (Claude only). */
	maxTurns?: number;
	/** Max spend in USD (Claude only). */
	maxBudgetUsd?: number;
	/** Verifier work-ethic (opt-in): auto-spawn one verifier when a task enters review. */
	verifierPerTask?: boolean;
	/** Verifier work-ethic (opt-in): a 3-verifier max-recall fan-out when all tasks are done. */
	maxRecallPerProject?: boolean;
}

export interface ProjectTaskCounts {
	todo: number;
	wip: number;
	review: number;
	blocked: number;
	done: number;
	total: number;
}

export interface ProjectLinkedResource {
	/** Legacy field; cloud provider links are no longer resolved. */
	provider: string;
	resourceId: string;
	label: string | null;
}

export type ProjectTaskCategory = "feature" | "hardening" | "bug" | "other";

export interface ProjectTask {
	id: string;
	title: string;
	status: ProjectTaskStatus;
	priority: string | null;
	assignee: string | null;
	due: string | null;
	linkedResources: ProjectLinkedResource[];
	updatedAt: string;
	// P1: mandatory on create (Todo column); absent on legacy cards.
	category?: ProjectTaskCategory;
	// P1: free-form bug/work description; the Oracle query in P2.
	description?: string;
	// P1: Oracle-localized suspect files. Empty for now; P2 populates it.
	suspectFileIds: string[];
	// Plan-execution fields (camelCase from Rust model). Absent on legacy/non-plan tasks.
	planId?: string;
	dependsOn?: string[];
	scope?: string[];
	acceptance?: string;
}

export interface ProjectNote {
	id: string;
	text: string;
	source: string;
	createdAt: string;
}

export interface ProjectMilestone {
	id: string;
	title: string;
	/** ISO calendar date, `YYYY-MM-DD`. */
	date: string;
	note?: string | null;
}

export interface ProjectStateBlock {
	version: number;
	tasks: ProjectTask[];
	notes: ProjectNote[];
	/** Optional for forward-compat: older detail payloads omit it. */
	milestones?: ProjectMilestone[];
}

export interface ProjectSummary {
	id: string;
	title: string;
	status: ProjectStatus;
	updatedAt: string;
	rootPath: string | null;
	revision: string;
	path: string;
	taskCounts: ProjectTaskCounts;
	gitStatus: ProjectGitStatus;
	/** Calendar milestones, surfaced on the summary so the Board calendar can
	 *  aggregate across projects from the cheap list result. Optional for
	 *  forward-compat with any cached/older summary payload. */
	milestones?: ProjectMilestone[];
}

export interface ProjectLiveResourceStatus {
	/** Legacy field; cloud live status is no longer resolved. */
	provider: string;
	resourceId: string;
	label: string;
	status: string;
	resourceType: string;
	region: string | null;
}

export interface ProjectLiveStatus {
	resources: ProjectLiveResourceStatus[];
	checkedAt: string;
}

export interface ProjectGitRepoCandidate {
	name: string;
	path: string;
	branch: string | null;
	origin: string | null;
	dirtyCount: number;
	cloneCommand: string | null;
}

export interface ProjectGitStatus {
	rootPath: string | null;
	repoRoot: string | null;
	repoName: string | null;
	branch: string | null;
	upstream: string | null;
	origin: string | null;
	githubUrl: string | null;
	cloneCommand: string | null;
	pullRequestUrl: string | null;
	commit: string | null;
	dirtyCount: number;
	stagedCount: number;
	unstagedCount: number;
	untrackedCount: number;
	aheadCount: number;
	behindCount: number;
	isGitRepo: boolean;
	isGithub: boolean;
	policyStatus: "ready" | "warning" | "blocked" | string;
	warnings: string[];
	requiredActions: string[];
	suggestedRepos: ProjectGitRepoCandidate[];
}

export interface GithubConnectionStatus {
	configured: boolean;
	status: "missing" | "valid" | "error" | string;
	source: string;
	cliAvailable: boolean;
	login: string | null;
	name: string | null;
	avatarUrl: string | null;
	profileUrl: string | null;
	scopes: string[];
	rateLimitRemaining: number | null;
	lastCheckedAt: string | null;
	message: string | null;
}

export interface GithubRepoAccessStatus {
	url: string;
	owner: string | null;
	repo: string | null;
	description: string | null;
	accessible: boolean;
	private: boolean | null;
	defaultBranch: string | null;
	openIssuesCount: number | null;
	stargazersCount: number | null;
	forksCount: number | null;
	pushedAt: string | null;
	updatedAt: string | null;
	permissions: string[];
	status: string;
	checkedAt: string;
	message: string | null;
}

// Access role bound to a device identity. Extend by adding a value here and a
// row in the backend `roles::role_permissions` table.
export type Role = "admin" | "collaborator";

export interface DeviceVaultStatus {
	configured: boolean;
	deviceId: string | null;
	deviceName: string | null;
	platform: string;
	vaultBackend: string;
	biometricLabel: string;
	publicKey: string | null;
	publicKeyFingerprint: string | null;
	privateKeyConfigured: boolean;
	signingPublicKey?: string | null;
	signingFingerprint?: string | null;
	signingKeyConfigured?: boolean;
	createdAt: string | null;
	lastCheckedAt: string;
	securityLevel: string;
	joinRequest: string | null;
	message: string | null;
	// Verified local role (derived, never trusted from the wire). Null = unprovisioned.
	role?: Role | null;
}

export interface DeviceInviteRecord {
	id: string;
	collaboratorName: string;
	deviceName: string;
	platform: string;
	publicKey: string;
	publicKeyFingerprint: string;
	signingPublicKey?: string | null;
	signingFingerprint?: string | null;
	status: "approved" | "revoked" | string;
	createdAt: string;
	approvedAt: string | null;
	revokedAt: string | null;
	notes: string | null;
	// Role the admin assigned at approval; null defaults to collaborator.
	role?: Role | null;
}

export interface DeviceInviteInput {
	collaboratorName: string;
	joinRequest: string;
	notes?: string | null;
	role?: Role | null;
}

// A role assignment the admin signs for a collaborator's device identity.
export interface RoleGrant {
	role: Role;
	subjectPublicKey: string;
	subjectSigningPublicKey: string;
	subjectFingerprint: string;
	issuedAt: string;
	expiresAt?: string | null;
}

export interface SignedRoleGrant {
	grant: RoleGrant;
	scheme: string;
	issuerSigningPublicKey: string;
	issuerFingerprint: string;
	signature: string;
}

// The verified local role + onboarding signal returned by get_local_role.
export interface LocalRoleStatus {
	role: Role;
	// True when this install is the admin OR holds a valid grant; false means a
	// fresh collaborator that still needs onboarding.
	provisioned: boolean;
	isAdmin: boolean;
	// Whether the bundled trust anchor is set (false => this build must not ship).
	anchorConfigured: boolean;
}

export interface DevicesInvitesSnapshot {
	localDevice: DeviceVaultStatus;
	invites: DeviceInviteRecord[];
}

export interface ProjectDetail {
	metadata: ProjectMetadata;
	state: ProjectStateBlock;
	markdown: string;
	revision: string;
	path: string;
	modifiedAt: string | null;
	liveStatus: ProjectLiveStatus;
	gitStatus: ProjectGitStatus;
}

export interface ProjectCreateInput {
	id?: string | null;
	title: string;
	status?: Exclude<ProjectStatus, "done"> | null;
	rootPath?: string | null;
}

export interface ProjectTaskInput {
	title: string;
	status?: ProjectEditableTaskStatus | null;
	priority?: string | null;
	assignee?: string | null;
	due?: string | null;
	linkedResources?: ProjectLinkedResource[] | null;
	// P1: mandatory on create; the Rust validator rejects an unknown/empty value.
	category?: ProjectTaskCategory | null;
	// P1: only meaningful for bug cards in the UI, persisted for any category.
	description?: string | null;
	expectedRevision: string;
}

export interface ProjectMetadataPatch {
	title?: string | null;
	status?: Exclude<ProjectStatus, "done"> | null;
	rootPath?: string | null;
	expectedRevision: string;
}

export interface ProjectAgentLaunchInput {
	projectId: string;
	// Phase B role merge: only coder/verifier are spawnable. The Rust boundary
	// (normalize_agent_role) still tolerates a legacy "orchestrator" inbound and
	// folds it to coder, but the app never sends it.
	role: "coder" | "verifier";
	client: "codex" | "claude" | "openai" | "powershell";
	agentId?: string | null;
	taskId?: string | null;
	// Where the agent runs: "app" (in-app PTY viewer) or "external" (detached OS
	// console). Optional; the backend normalizes absent/unknown to "external".
	host?: "app" | "external" | null;
	// Advisory model hint (opus/sonnet/haiku/custom) threaded into the launch
	// prompt's agent_register model= placeholder. Optional; absent keeps the
	// agent self-report placeholder. Mirrors ProjectAgentLaunchInput in model.rs.
	model?: string | null;
	// Phase H: true only for a verifier launched as a Censor "final review" (the
	// "Run final review" button). The backend then appends the residual-
	// adjudication addendum to the launch prompt. Optional and lenient: absent
	// leaves the verifier prompt unchanged (back-compat). Mirrors the
	// #[serde(default)] censor_review on ProjectAgentLaunchInput in model.rs.
	censorReview?: boolean | null;
	// 3b: true only for a LOCAL orchestrator launch (client === "orchestrator") with
	// the "Plan first" toggle ON. The backend then adds DEVBOULE_PLAN_FIRST=1 to the
	// orchestrator's env so its system prompt biases toward planning before acting.
	// Optional and lenient: absent/false omits the env entirely (the default launch
	// is byte-identical). Ignored for codex/claude. Mirrors the #[serde(default)]
	// plan_first on ProjectAgentLaunchInput in model.rs.
	planFirst?: boolean | null;
	// Phase 6: per-launch LANGUAGE-persona override (rust/node/python/go/cpp/kotlin). Absent ⇒ the
	// backend auto-detects the project's primary language for the (role × language) persona; a value
	// forces that language's persona on whatever backend the role runs on. Optional + lenient:
	// mirrors the #[serde(default)] language_override on ProjectAgentLaunchInput in model.rs.
	languageOverride?: string | null;
	// Saved Claude Code workflow launch. The backend validates name against
	// list_saved_workflows(projectId) before building the fixed prompt addendum.
	workflowRun?: {
		name: string;
		args?: string | null;
	} | null;
}

export interface ProjectAgentLaunchResult {
	projectId: string;
	role: string;
	client: string;
	agentId: string;
	rootPath: string;
	prompt: string;
	launched: boolean;
	message: string;
}

export interface SavedWorkflow {
	name: string;
	description?: string | null;
	scope: "project" | "global" | string;
}

// Result of project_git_commit / project_git_push (Work mode top bar). The
// refreshed gitStatus lets the bar update in place. On FAILURE the backend
// command rejects with the git stderr string instead of returning this, so the
// UI surfaces the real git error. Mirrors ProjectGitCommandResult in
// src-tauri/src/backend/model.rs (camelCase over IPC).
export interface ProjectGitCommandResult {
	projectId: string;
	branch: string;
	message: string;
	gitStatus: ProjectGitStatus;
}

// GH-P4: an agent→human git push-approval request. The agent's MCP
// `request_git_push` tool appends a `pending_approval` entry; the human's
// approve/deny Tauri command drives the rest. Mirrors GitPushRequest in
// src-tauri/src/backend/git_push.rs (camelCase over IPC). Optional fields are
// omitted by the backend when unset (no churn), so they are `?` here.
export type GitPushStatus =
	| "pending_approval"
	| "approved"
	| "pushing"
	| "pushed"
	| "push_failed"
	| "denied"
	| "timeout";

export interface GitPushResult {
	status: GitPushStatus;
	exitCode?: number;
	// Already SANITIZED + token-redacted by git_run_authenticated before storage.
	output?: string;
	error?: string;
}

export interface GitPushRequest {
	id: string;
	// The requesting agent session id.
	agentId: string;
	projectId: string;
	status: GitPushStatus;
	// Informational only (the push targets the repo's current HEAD).
	branch?: string;
	remote?: string;
	// True when the agent requested a FORCE push (the card warns prominently).
	force?: boolean;
	createdAt: string;
	// RFC3339 time the human approved (set by the Rust approve command). Used by the
	// backend's list-time stuck-request reconciliation; display-only here.
	approvedAt?: string;
	result?: GitPushResult;
}

export interface ProjectTaskPatch {
	title?: string | null;
	status?: ProjectEditableTaskStatus | null;
	priority?: string | null;
	assignee?: string | null;
	due?: string | null;
	linkedResources?: ProjectLinkedResource[] | null;
	expectedRevision: string;
}

export interface ProjectNoteInput {
	text: string;
	source?: string | null;
	expectedRevision: string;
}

export interface AgentRoleRule {
	// A role RULE row from the role_rules.json SSoT: since the 2026-07 role
	// untangle the backend emits FOUR rules — {coder, orchestrator, verifier,
	// mini} (default_role_rules in agents.rs and ROLE_RULES in aspis_mcp.py both
	// parse oracle/server/role_rules.json). NOT SpawnRole: the spawn radio stays
	// narrower, but a consumer switching on rule.role must handle all four.
	role: AgentRole | "mini";
	summary: string;
	allowedTools: string[];
	forbidden: string[];
	// Practical "what every agent of this role must DO" mandate strings. Mirrors
	// ROLE_RULES[].contract in oracle/server/aspis_mcp.py and default_role_rules()
	// in src-tauri/src/backend/agents.rs. Optional: older payloads omit it.
	contract?: string[];
	// PHASE E: the role's Censor-consumption mandate (coder per-step, verifier
	// residual adjudication). Mirrors `censor: Vec<String>` on the Rust
	// `AgentRoleRule` (serde `censor`) + ROLE_RULES[].censor in aspis_mcp.py.
	// Optional + skip-if-empty on the Rust side, so older payloads omit it.
	censor?: string[];
	// GH-P5: the role's cooperative git-push mandate (coder only — commit freely,
	// NEVER raw `git push`, publish via the request_git_push MCP tool + human
	// approval, STOP on deny/timeout). Mirrors `push: Vec<String>` on the Rust
	// `AgentRoleRule` (serde `push`) + ROLE_RULES[].push in aspis_mcp.py. Optional
	// + skip-if-empty on the Rust side (verifier has no push field), so payloads
	// omit it for the verifier and for older versions.
	push?: string[];
}

// One subagent breakdown entry reported via MCP agent_heartbeat `subagents`.
// Mirrors AgentSubagent in src-tauri/src/backend/model.rs.
export interface AgentSubagent {
	label: string;
	model: string;
	count: number;
	role?: string | null;
}

// Set when an agent is blocked waiting on the human. Mirrors AgentNeedsUser in
// src-tauri/src/backend/model.rs.
export interface AgentNeedsUser {
	reason: string;
	message: string;
	since: string;
}

// A plan approval request the agent raised via the MCP request_plan_approval tool.
// Pending requests block the agent until the human approves or rejects.
// Mirrors PlanApprovalRequest in src-tauri/src/backend/model.rs.
export interface PlanApprovalRequest {
	id: string;
	agentId: string;
	projectId: string;
	title: string;
	status: "pending_approval" | "approved" | "rejected" | "timeout";
	createdAt: string;
	decidedAt?: string | null;
	note?: string | null;
	/** Set true after project_create_plan_tasks one-shot materialize. */
	tasksCreated?: boolean | null;
	/** RFC3339 when plan tasks were materialized. */
	tasksMaterializedAt?: string | null;
}

// A pending question the agent asked the user via the MCP ask_user tool.
export interface AgentPendingQuestion {
	id: string;
	question: string;
	createdAt: string;
}

// The human's reply to a pending question (echoed back on the session after reply).
export interface AgentUserReply {
	questionId: string;
	text: string;
	createdAt: string;
}

export interface AgentSession {
	agentId: string;
	role: string;
	model: string | null;
	status: string;
	message: string | null;
	// Launch CLI for this agent (codex/claude/powershell). Carried by the Rust
	// backend via a ledger + read-time stamp; may be null/absent for sessions not
	// launched by the app.
	client?: string | null;
	currentProjectId: string | null;
	currentTaskId: string | null;
	firstSeenAt: string | null;
	lastSeenAt: string | null;
	launchTokenHash?: string | null;
	launchTokenIssuedAt?: string | null;
	sessionTokenHash?: string | null;
	sessionTokenIssuedAt?: string | null;
	/** SEC#7: set when launch token consumed; scrubbed from UI paths. */
	launchConsumedAt?: string | null;
	// Subagent breakdown the agent reports (orchestrator fan-out, coder helpers).
	// Optional/absent for sessions that never reported any.
	subagents?: AgentSubagent[];
	// Present when the agent is blocked on the human (question/permission/block).
	needsUser?: AgentNeedsUser | null;
	// Terminal host: "app" (in-app PTY), "external" (detached OS console), or
	// null/absent when the session was not launched by the app. READ-TIME stamp
	// from the Rust ledger (get_agent_live_state); never persisted. Gates the
	// row's "Open CLI" (external only) vs. the in-app Terminal toggle.
	host?: string | null;
	// Parent agent id when this session is a mini-coder a coder delegated to (a
	// real host="app" PTY one-shot). Set by the Rust executor on launch; null/absent
	// for top-level agents. camelCase mirror of the Rust `parentAgentId` serde field.
	// "is a mini" === parentAgentId is a non-empty string.
	parentAgentId?: string | null;
	// Set when the agent is waiting for the human to answer a question it raised via
	// the MCP ask_user tool. Cleared once the human sends a reply.
	pendingQuestion?: AgentPendingQuestion | null;
	// Echoed back on the session after the human replied (present until next poll cycle).
	userReply?: AgentUserReply | null;
}

export interface AgentClaim {
	projectId: string;
	projectTitle: string | null;
	taskId: string;
	taskTitle: string | null;
	agentId: string;
	role: AgentRole;
	status: AgentClaimStatus;
	claimedAt: string;
	updatedAt: string;
	leaseUntil: string | null;
	evidence: string | null;
}

export interface AgentEvent {
	id: string;
	timestamp: string;
	agentId: string;
	role: AgentRole;
	eventType: string;
	projectId: string | null;
	taskId: string | null;
	status: AgentClaimStatus | null;
	message: string;
	evidence: string | null;
}

export interface AgentLiveState {
	version: number;
	updatedAt: string;
	sessions: AgentSession[];
	claims: AgentClaim[];
	events: AgentEvent[];
	rules: AgentRoleRule[];
	statePath: string;
	mcpCommand: string;
	mcpClientConfig: string;
	// Plan approval requests currently pending human review. Optional: absent in
	// older snapshots or backends that do not yet populate this field.
	planApprovalRequests?: PlanApprovalRequest[];
	// Mini-coder directive queue entries from .aspis-agents.json.
	miniCoderDirectives?: MiniCoderDirective[];
}

// ---- Mini-coder directive shapes (camelCase wire from .aspis-agents.json) -------

/** Terminal stuck report written when a mini-coder directive times out or fails
 *  terminally. Mirrors the Rust `StuckReport` serde shape. */
export interface StuckReport {
	taskId: string;
	agentId: string;
	reason: string;
	attempts: number;
	lastOutput: string;
	filesTouched: string[];
	projectId?: string;
}

/** Censor phase-a summary for a completed mini-coder directive, recording how
 *  many findings the coarse pass flagged on which files. */
export interface MiniCensorSummary {
	total: number;
	files: string[];
}

/** Result block for a completed/failed mini-coder directive. */
export interface MiniCoderResult {
	status: string;
	filesTouched?: string[];
	error?: string;
}

/** One entry in the mini-coder directive queue, persisted in .aspis-agents.json.
 *  Mirrors the Rust `MiniCoderDirective` serde shape (camelCase wire). */
export interface MiniCoderDirective {
	id: string;
	parentAgentId: string;
	status: string;
	task: string;
	files: string[];
	projectId?: string;
	resultPath: string;
	agentId?: string;
	createdAt: string;
	claimedAt?: string;
	startedAt?: string;
	attempt?: number;
	write?: boolean;
	result?: MiniCoderResult;
	stuckReport?: StuckReport;
	censorSummary?: MiniCensorSummary;
}

// MC-P6: per-agent token / cost window (best-effort, Claude-Code-coupled). Mirror
// of the Rust `AgentTokenUsage` (backend/token_usage.rs). Fetched ONLY for the
// selected agent on a slow cadence — never per rail row, never on the live-state
// tick. Degrades to source="unavailable" on any surprise.
export interface AgentTokenCounts {
	input: number;
	output: number;
	cacheCreation: number;
	cacheRead: number;
	total: number;
}

export type AgentTokenUsageSource =
	| "claude-transcript"
	| "subscription"
	| "unavailable";

export interface AgentTokenUsage {
	tokens: AgentTokenCounts;
	// Approximate USD cost for API-priced Claude, or null for unknown model /
	// subscription. PRICES ARE APPROXIMATE (manually maintained Rust table).
	costUsd: number | null;
	source: AgentTokenUsageSource;
}

export interface WorkspaceSizeEntry {
	name: string;
	entryType: "dir" | "file" | string;
	path: string;
	sizeGb: number;
	fileCount: number;
	lastWrite: string | null;
}

export interface WorkspaceLargeFile {
	relativePath: string;
	path: string;
	sizeGb: number;
	sizeMb: number;
	lastWrite: string | null;
	classLabel: string;
}

export interface WorkspaceGitRepoStatus {
	name: string;
	path: string;
	relativePath: string;
	branch: string;
	origin: string | null;
	dirtyCount: number;
	gitSize: string | null;
	cloneCommand: string | null;
	warnings: string[];
}

export interface WorkspaceClassificationEntry {
	path: string;
	classLabel: string;
	git: string;
	oracle: string;
	storage: string;
	notes: string;
}

export interface WorkspacePolicyFile {
	name: string;
	path: string;
	exists: boolean;
	lineCount: number;
	activeRules: number;
}

export interface WorkspaceHygieneSnapshot {
	root: string;
	workspaceDir: string;
	scannedAt: string;
	needsScan: boolean;
	totalSizeGb: number;
	totalFiles: number;
	oracleCandidateFiles: number;
	topLevel: WorkspaceSizeEntry[];
	largeFiles: WorkspaceLargeFile[];
	gitRepos: WorkspaceGitRepoStatus[];
	classifications: WorkspaceClassificationEntry[];
	policyFiles: WorkspacePolicyFile[];
	warnings: string[];
}

export interface WorkspacePackageRecipient {
	fingerprint: string;
	collaboratorName: string;
	deviceName: string;
	platform: string;
	source: string;
	publicKey: string;
}

export interface WorkspacePackageInfo {
	path: string;
	fileName: string;
	sizeMb: number;
	createdAt: string | null;
}

export interface WorkspacePackageSnapshot {
	root: string;
	packageDir: string;
	importDir: string;
	approvedRecipients: WorkspacePackageRecipient[];
	latestPackages: WorkspacePackageInfo[];
	maxPackageSizeMb: number;
	warnings: string[];
}

export interface WorkspacePackageResult {
	packageId: string;
	path: string;
	fileName: string;
	fileCount: number;
	totalBytes: number;
	packageBytes: number;
	recipientCount: number;
	skippedFiles: number;
	skippedBytes: number;
	readmePath: string;
	createdAt: string;
	warnings: string[];
}

export interface WorkspaceDecryptResult {
	packageId: string;
	outputDir: string;
	filesRestored: number;
	bytesRestored: number;
	recipientFingerprint: string;
	// Package signature provenance. `signatureValid` is always true here because
	// decrypt fails closed before extraction if the Ed25519 signature does not
	// verify; it is surfaced so the UI can render a positive "verified" state.
	signatureValid: boolean;
	signerPublicKey: string;
	// Fingerprint recomputed from the verified signer public key (not the
	// self-reported block field). Always shown so the user can compare it
	// out-of-band.
	signerFingerprint: string;
	// True when the signer's Ed25519 key matches the local device or an approved
	// device; false means the signature is valid but the signer is unrecognized.
	signerKnown: boolean;
	signerName: string | null;
	warnings: string[];
}

export interface OracleResult {
	id: string;
	label: string;
	type: string;
	cluster: number;
	score: number;
	fileSource: string;
	functionPrimary: string;
	dependencies: string[];
	chunkId?: string | null;
	chunkIndex?: number | null;
	startChar?: number | null;
	endChar?: number | null;
	chunkPreview?: string | null;
}

// ---------------------------------------------------------------------------
// Censor — continuous local-first per-file code review (engine: A1–A3).
//
// These mirror the Rust serde shapes in `src-tauri/src/backend/censor/schema.rs`
// (camelCase over IPC) + the A3 command/event surface in
// `src-tauri/src/backend/censor/commands.rs` and `orchestrator.rs`. They are
// CONTRACT types: the same shard JSON is also lock-read by the Python MCP server
// (Phase E), so the keys must match Rust EXACTLY. No UI is built here (Phase C/E);
// these are the types + command names the dock/board will consume.
// ---------------------------------------------------------------------------

/** Finding severity — `high | medium | low` vocab (no critical/info). */
export type CensorSeverity = "high" | "medium" | "low";

/** Finding category. `dead-code` is kebab-cased over the wire. */
export type CensorCategory =
	| "security"
	| "correctness"
	| "complexity"
	| "duplication"
	| "dead-code"
	| "style";

/** Confidence: deterministic linters emit `suspected`; the reviewer confirms. */
export type CensorVerdict = "suspected" | "confirmed";

/** Lifecycle disposition. `fp` = false positive. Set via censor_dispose_finding. */
export type CensorDisposition = "open" | "fixed" | "fp" | "wontfix";

/** One audit-trail entry on a finding (who did what, when). Append-only. */
export interface CensorProvenanceEntry {
	actor: string;
	action: string;
	/** The actor's role (coder/verifier) at the time, or "" for machine/legacy
	 *  entries. Mirrors `role: String` on the Rust `ProvenanceEntry` (always
	 *  serialized) + the `_safe_censor_finding` projection in aspis_mcp.py. */
	role: string;
	at: string;
}

/** A single code-review finding for one file. */
export interface CensorFinding {
	id: string;
	file: string;
	contentHash: string;
	/** 1-based line, or null for a file-level finding. */
	line: number | null;
	severity: CensorSeverity;
	category: CensorCategory;
	/** Tool name (e.g. "clippy", "gitleaks") or "gemma". */
	source: string;
	title: string;
	/** English summary. NEVER raw tool stdout that could carry a secret value. */
	body: string;
	verdict: CensorVerdict;
	disposition: CensorDisposition;
	provenance: CensorProvenanceEntry[];
	createdAt: string;
	commit?: string | null;
}

/** One per-file shard: the file's current content-hash plus its findings array. */
export interface CensorShard {
	fileRelPath: string;
	contentHash: string;
	updatedAt: string;
	findings: CensorFinding[];
}

/**
 * Payload of the `censor://findings-updated` Tauri event, emitted after one or
 * more shards change. Must match `FindingsUpdatedPayload` in
 * `src-tauri/src/backend/censor/orchestrator.rs`. `files` are project-relative,
 * forward-slash-normalized paths whose findings changed (may be empty).
 */
export interface CensorFindingsUpdatedPayload {
	projectId: string;
	files: string[];
}

/** Tauri event name the Censor orchestrator emits. Must match the Rust const. */
export const CENSOR_FINDINGS_UPDATED_EVENT = "censor://findings-updated";

/**
 * Argument shapes for the Censor Tauri commands (all `ensure_unlocked`-gated),
 * registered in `lib.rs`. The frontend invokes these via `invokeBackendCommand`;
 * Tauri lower-camelCases the Rust snake_case params, so these mirror the wire
 * names. Kept as types (not invoker fns) per this file's convention.
 */
export interface CensorStartWatchArgs {
	projectId: string;
	root: string;
}
export interface CensorStopWatchArgs {
	projectId: string;
}
export interface CensorReviewNowArgs {
	projectId: string;
	root: string;
	/** Recheck one file (its fine runners), or omit for the whole-project sweep. */
	file?: string | null;
}
export interface CensorGetFindingsArgs {
	root: string;
	/** One file's findings, or omit for every shard's open findings. */
	file?: string | null;
}
export interface CensorCountOpenArgs {
	root: string;
}
export interface CensorDisposeFindingArgs {
	projectId: string;
	root: string;
	file: string;
	id: string;
	disposition: CensorDisposition;
}
export interface CensorStatusArgs {
	root: string;
	/** When supplied, confines the status to THAT project's root and reports its
	 *  Censor `trusted` flag (BLOCKER B). Omit for a board-level read (trusted=false). */
	projectId?: string | null;
}
/** Args for `set_censor_trusted` (BLOCKER B): opt a project in/out of running Censor. */
export interface SetCensorTrustedArgs {
	projectId: string;
	trusted: boolean;
}
/** Args for `censor_open_in_editor`. WARNING D: `projectId` confines the open to
 *  THAT project's configured root (a valid root for project A can no longer be
 *  paired with project B's id). Mirrors the Rust command params. */
export interface CensorOpenInEditorArgs {
	projectId: string;
	root: string;
	file: string;
	editor: string;
}

/** Cached Gemma (Ollama) availability tri-state from `CensorState::gemma_status`. */
export type CensorGemmaStatus = "available" | "offline" | "unknown";

/** One detected/absent linter, mirrors Rust `CensorToolStatus`. */
export interface CensorToolStatus {
	/** The runner's executable name (e.g. "cargo", "eslint", "gitleaks"). */
	name: string;
	available: boolean;
}

/** `censor_status` payload, mirrors Rust `CensorStatus`. */
export interface CensorStatus {
	gemmaStatus: CensorGemmaStatus;
	/** Linters relevant to the project's kinds, deduped by executable. */
	tools: CensorToolStatus[];
	/** BLOCKER B: whether the user has trusted this project to RUN Censor. `false`
	 *  for a board-level read (no projectId) or an untrusted project. The panel shows
	 *  a "Trust this project to run Censor" prompt when false. */
	trusted: boolean;
	/** COARSE review policy: "off" | "manual" | "auto". */
	coarsePolicy: string;
	/** ISO timestamp of last COARSE pass, or null. */
	lastCoarseRun: string | null;
}

export interface OracleCitation {
	ref: string;
	fileSource: string;
	chunkId: string;
	chunkIndex: number | null;
	startChar: number | null;
	endChar: number | null;
	retrieval: string;
	score: number;
}

export interface OracleAnswer {
	mode: string;
	query: string;
	summary: string;
	answer: string;
	citations: OracleCitation[];
	notFound: boolean;
	suggestedPath: string | null;
	answerSource?: string | null;
	// The reason an answer degraded to the extractive (retrieval-only) fallback —
	// the ONLY fallback. There is no LLM-to-LLM fallback.
	fallbackReason?: string | null;
	llmProvider?: string | null;
	llmModel?: string | null;
	results: OracleResult[];
}

export interface OracleNodeCard {
	id: string;
	label: string;
	area: string;
	clusterSemantic: string;
	funzionePrimaria: string;
	esponeApi: string[];
	dipendeDa: string[];
	usedBy: string[];
	simileA: string[];
	tecnologie: string[];
	fileSorgente: string;
	ultimaModifica: string | null;
	source: string;
	embeddingDims: number;
}

// Typed failure surface for every Oracle command. Mirrors the Rust
// `OracleError` (serde camelCase). The backend stopped swallowing failures;
// these arrive as the rejection value of a Tauri command so the UI can branch
// on `kind` and render `remediation`.
export type OracleErrorKind =
	| "noWorkspaceRoot"
	| "serverUnavailable"
	| "pythonError"
	| "embedderUnavailable"
	| "indexEmpty"
	| "missingApiKey"
	| "internal";

export interface OracleError {
	kind: OracleErrorKind;
	message: string;
	remediation: string;
}

export interface OracleDoctorCheck {
	id: string;
	ok: boolean;
	detail: string;
	remediation: string;
}

export interface OracleDoctorReport {
	ok: boolean;
	checks: OracleDoctorCheck[];
}

export interface OracleIndexedFile {
	path: string;
	chunks: number;
	// ISO timestamp, OR "" when the manifest entry carries no date. The UI must
	// treat "" as "unknown date" and MUST NOT call new Date("") on it.
	updatedAt: string;
}

export interface OracleIndexedFiles {
	total: number;
	files: OracleIndexedFile[];
	limit: number;
	offset: number;
}

export interface CliAgentsStatus {
	claudeConfigured: boolean;
	claudeConfigPath: string | null;
	codexConfigured: boolean;
	codexNote: string | null;
	interpreter: string | null;
	root: string | null;
	projectsDir: string | null;
	runtimeReady: boolean;
	warning: string | null;
}

// TASK #13 — the Settings "Dependencies" tab: every external CLI Devboule can use,
// its resolved location + version (best-effort). Mirror of the Rust
// `backend::provider_detect::DetectedDependency` (camelCase over IPC). Unlike
// `DetectedProvider`, `path` IS exposed here — the Dependencies page is
// user-requested diagnostics that intentionally show WHERE each tool was found.
export interface DetectedDependency {
	/** The tool's binary name (monospace in the UI). */
	name: string;
	/** What Devboule uses the tool for. */
	purpose: string;
	/** Grouping bucket shown as a section header in the UI. */
	category: string;
	/** Whether the binary was found on the augmented PATH. */
	found: boolean;
	/** Resolved absolute path, or omitted when not found (serde skip_serializing_if). */
	path?: string;
	/** Best-effort `tool --version` output (first line), or omitted. */
	version?: string;
}

const ORACLE_ERROR_KINDS: ReadonlySet<string> = new Set<OracleErrorKind>([
	"noWorkspaceRoot",
	"serverUnavailable",
	"pythonError",
	"embedderUnavailable",
	"indexEmpty",
	"missingApiKey",
	"internal",
]);

// Structural guard so the UI can branch on a caught (unknown) rejection value.
// Validates the full `{ kind, message, remediation }` shape and that `kind` is
// a known variant.
export function isOracleError(e: unknown): e is OracleError {
	if (typeof e !== "object" || e === null) return false;
	const candidate = e as Record<string, unknown>;
	return (
		typeof candidate.kind === "string" &&
		ORACLE_ERROR_KINDS.has(candidate.kind) &&
		typeof candidate.message === "string" &&
		typeof candidate.remediation === "string"
	);
}

// Best-effort machine hardware snapshot from the `detect_hardware` command. A 1:1 MIRROR
// of the Rust `backend::hardware::HardwareInfo` (camelCase over IPC). Phase B2 will use this
// to scale Polis rendering detail to the host; B1 only makes it callable.
//
// Every field is best-effort: `gpuName`/`gpuKind` fall back to "unknown" and `vramGb` to
// null when the GPU cannot be probed, so consumers must treat it as advisory and never
// assume a discrete card.
export type GpuKind = "integrated" | "discrete" | "unknown";

export interface HardwareInfo {
	// Logical CPU core count (>= 1).
	cpuCores: number;
	// Total physical RAM, GiB.
	ramTotalGb: number;
	// Currently available RAM, GiB.
	ramAvailableGb: number;
	// Best-guess primary GPU model; "unknown" when unprobed.
	gpuName: string;
	// Dedicated VRAM in GiB when knowable; null for integrated/unified-memory or unknown.
	vramGb: number | null;
	gpuKind: GpuKind;
}
