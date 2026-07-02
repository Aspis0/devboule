use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Cloudflare,
    Scaleway,
}

impl ProviderId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cloudflare => "cloudflare",
            Self::Scaleway => "scaleway",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Cloudflare => "Cloudflare",
            Self::Scaleway => "Scaleway",
        }
    }

    pub fn credential_account(self) -> &'static str {
        match self {
            Self::Cloudflare => "provider:cloudflare",
            Self::Scaleway => "provider:scaleway",
        }
    }

    pub fn scope_credential_account(self) -> &'static str {
        match self {
            Self::Cloudflare => "scope:cloudflare_account_id",
            Self::Scaleway => "scope:scaleway_project_id",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthState {
    pub locked: bool,
    pub hello_available: bool,
    pub last_unlocked_at: Option<String>,
    pub lock_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretStatus {
    pub provider: ProviderId,
    pub configured: bool,
    pub status: String,
    pub last_checked_at: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderScopeStatus {
    pub provider: ProviderId,
    pub configured: bool,
    pub pinned_id: Option<String>,
    pub label: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuxCredentialStatus {
    pub id: String,
    pub label: String,
    pub configured: bool,
    pub status: String,
    pub last_checked_at: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareAgentTokenProfileStatus {
    pub id: String,
    pub label: String,
    pub role: String,
    pub configured: bool,
    pub status: String,
    pub env_var: String,
    pub credential_account: String,
    pub last_checked_at: Option<String>,
    pub message: Option<String>,
}

/// Minimal Oracle "Answer LLM" settings: ONE remote provider, model, base URL,
/// and (separately, in the vault) an API key. There is no LLM-to-LLM fallback and
/// no ZDR/GDPR gate — the only fallback when the remote LLM cannot answer is the
/// extractive/LanceDB retrieval answer. Old stored JSON may still carry the
/// removed fields; serde ignores unknown fields on deserialize (no
/// `deny_unknown_fields`), so historical data still reads and new saves drop them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OracleLlmSettings {
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub remote_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OracleLlmSettingsStatus {
    pub settings: OracleLlmSettings,
    pub api_key_configured: bool,
    pub status: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OracleIndexPreferences {
    pub auto_watch_on_unlock: bool,
    #[serde(default)]
    pub index_root: Option<String>,
    /// "watch" | "commit". Absent means watch (default). Serialized only when
    /// present so older persisted blobs without the key round-trip cleanly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMetadata {
    pub id: String,
    pub title: String,
    pub status: String,
    pub updated_at: String,
    #[serde(default)]
    pub root_path: Option<String>,
    /// BLOCKER B (untrusted-repo tool-config RCE): whether the user has explicitly
    /// trusted this project to RUN the Censor engine. Selecting a project runs the
    /// repo's OWN tool configs (eslint plugins, cargo build scripts via
    /// clippy/check, custom semgrep rules) from the project root, i.e. it executes
    /// repo-controlled code. Censor stays inert (no deterministic runner + no Gemma
    /// spawn) until the user opts in. Default `false`. NO-CHURN: omitted from the
    /// on-disk frontmatter when false so trusting/serializing a pre-existing
    /// project never injects `censor_trusted: false`.
    #[serde(default)]
    pub censor_trusted: bool,
    /// SANDBOX phase 2: whether the user has UNBLOCKED network for this project's sandboxed
    /// agentic commands, after a network-blocked failure surfaced the HINT (see
    /// `agentic_tools::looks_network_blocked`). Default `false` (net denied). NO-CHURN: omitted
    /// from the on-disk frontmatter when false, like `censor_trusted`.
    #[serde(default)]
    pub net_enabled: bool,
    /// SANDBOX broker Slice 1: per-project autonomy mode governing whether the broker prompts
    /// the user on blocked requests.  Default `Ask` (always prompt).  NO-CHURN: omitted from
    /// the on-disk frontmatter when equal to `Ask` so pre-existing project files stay byte-stable.
    #[serde(default, skip_serializing_if = "crate::backend::broker::is_default_sandbox_mode")]
    pub sandbox_mode: crate::backend::broker::SandboxMode,
    /// SANDBOX broker Slice 2: per-project working set — extra folders OUTSIDE the project
    /// root that the user has explicitly granted persistent write access to.  Each entry is
    /// an absolute, canonicalized path.  Default empty.  NO-CHURN: omitted from the on-disk
    /// frontmatter when empty so pre-existing project files stay byte-stable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub working_set: Vec<String>,
    /// Slice 5c: per-project default agent capability/cost controls (effort, system-prompt,
    /// turn/budget caps) for the cloud coders. NO-CHURN: the whole object is omitted from the
    /// on-disk frontmatter when every field is unset, so pre-existing project files stay
    /// byte-stable.
    #[serde(default, skip_serializing_if = "AgentControls::is_default")]
    pub agent_controls: AgentControls,
    /// P6b (role untangle): per-project OVERRIDE of the Main-coder engine. The global
    /// default lives in `RolesConfig.mainCoder` (config.json); this per-project field lets
    /// project A run `codex` while project B runs `claude` or a local backend. The value is
    /// an opaque engine/client id (the same shape as the old hand-off `coderId`), validated
    /// at the launch/consumption layer. Resolution at launch = this override, else the global
    /// RolesConfig default. `None` = use the global default. NO-CHURN: omitted from the
    /// on-disk frontmatter when None so pre-existing project files stay byte-stable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_coder: Option<String>,
}

/// Slice 5c: per-project (or per-launch) capability/cost controls for the cloud coding CLIs.
/// All optional — `None` means "leave the CLI default". The PERMISSION axis (sandbox_mode)
/// is deliberately NOT here; it stays on `ProjectMetadata` and drives the broker.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentControls {
    /// Reasoning effort. Claude `--effort` (low/medium/high/xhigh/max); Codex
    /// `model_reasoning_effort` (none/minimal/low/medium/high/xhigh/ultra). Free string —
    /// validated/clamped per-backend at emit time, not here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Extra system-prompt text. Claude `--append-system-prompt`; Codex `developer_instructions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Max agent turns (Claude `--max-turns`, print mode). No direct Codex equivalent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    /// Max spend in USD (Claude `--max-budget-usd`, print mode). No direct Codex equivalent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_budget_usd: Option<f64>,
    /// Verifier work-ethic (recommended, opt-in). `verifier_per_task`: auto-spawn one verifier when
    /// a task enters `review`. `max_recall_per_project`: spawn a max-recall verifier fan-out when
    /// every task is in {review, done}. Both default OFF (the UI nudges).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub verifier_per_task: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub max_recall_per_project: bool,
}

impl AgentControls {
    /// True when every control is unset — drives the NO-CHURN `skip_serializing_if`.
    pub fn is_default(&self) -> bool {
        self.effort.is_none()
            && self.system_prompt.is_none()
            && self.max_turns.is_none()
            && self.max_budget_usd.is_none()
            && !self.verifier_per_task
            && !self.max_recall_per_project
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTaskCounts {
    pub todo: usize,
    pub wip: usize,
    pub review: usize,
    pub blocked: usize,
    pub done: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLinkedResource {
    pub provider: ProviderId,
    pub resource_id: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTask {
    pub id: String,
    pub title: String,
    pub status: String,
    pub priority: Option<String>,
    pub assignee: Option<String>,
    pub due: Option<String>,
    #[serde(default)]
    pub linked_resources: Vec<ProjectLinkedResource>,
    pub updated_at: String,
    /// Card category: feature | hardening | bug | other. Optional + serde-default
    /// so project markdown written before categories existed loads unchanged.
    #[serde(default)]
    pub category: Option<String>,
    /// Free-form bug/work description. Persisted on the card; P2 uses it as the
    /// Oracle localization query. Serde-default for backward compatibility.
    #[serde(default)]
    pub description: Option<String>,
    /// Oracle-localized suspect files (P2 fills this). Empty for now; serde-default
    /// so old markdown blocks load with [].
    #[serde(default)]
    pub suspect_file_ids: Vec<String>,
    /// Phase 11.5-B (Piece 1a): prerequisite task ids forming the plan DAG. Empty
    /// for a manual task with no dependencies. Serde-default + camelCase (`dependsOn`)
    /// so a `.md` state block written before this field existed loads UNCHANGED;
    /// `skip_serializing_if` keeps the on-disk JSON byte-stable for manual tasks
    /// (re-serializing a pre-1a task must NOT inject `"dependsOn":[]` and churn the
    /// content hash / git-dirty state / Oracle re-index — same discipline as
    /// `milestones`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Phase 11.5-B (Piece 1a): files this task may MODIFY — the mini's write
    /// allowlist when the runner executes a plan task. Empty for a manual task.
    /// Serde-default + camelCase (`scope`) + no-churn skip → backward-compatible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope: Vec<String>,
    /// Phase 11.5-B (Piece 1a): the deterministic acceptance check (free text).
    /// Empty for a manual task. Serde-default + camelCase (`acceptance`); omitted
    /// from on-disk JSON when empty so a manual task never injects `"acceptance":""`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub acceptance: String,
    /// Phase 11.5-B (Piece 1a): the approved plan id this task was created from.
    /// `Some` ONLY for tasks created via `project_create_plan_tasks`; `None` for
    /// manual tasks (this is how the runner knows which tasks to auto-execute).
    /// Serde-default + camelCase (`planId`); omitted from on-disk JSON when `None`
    /// so a manual task never injects `"planId":null` and churns the content hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    /// ROLE UNTANGLE Phase 4: the execution TIER the runner dispatches this plan
    /// task to — `"main"` routes it to the first-class Main coder
    /// (`spawn_main_coder`, the always-agentic sandboxed engine); empty/absent =
    /// the mini path (the status quo). Written by `project_create_plan_tasks`
    /// (Python co-writer, NO-CHURN: only `"main"` is ever stored) and read by the
    /// devboule-coder runner. Serde-default + no-churn skip so every pre-Phase-4
    /// task round-trips byte-identically.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub weight: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectNote {
    pub id: String,
    pub text: String,
    pub source: String,
    pub created_at: String,
}

/// A calendar/organizer entry (deadline / milestone) attached to a project.
/// Lives in the project's `aspis-project` JSON state block alongside tasks/notes
/// so it stays in the same Oracle-indexable plain-text file (no separate store).
/// `date` is an ISO calendar date (`YYYY-MM-DD`). `note` is optional free text.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMilestone {
    pub id: String,
    pub title: String,
    /// ISO calendar date, `YYYY-MM-DD`. Validated on write.
    pub date: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStateBlock {
    pub version: u32,
    #[serde(default)]
    pub tasks: Vec<ProjectTask>,
    #[serde(default)]
    pub notes: Vec<ProjectNote>,
    /// Calendar milestones. `#[serde(default)]` so an OLD project file written
    /// before milestones existed loads as an empty list with no error (forward-
    /// compat), and a NEW file's milestones survive a write/read cycle losslessly.
    /// `skip_serializing_if = "Vec::is_empty"` keeps the on-disk JSON byte-stable
    /// for projects that have no milestones: mutating a pre-milestone project must
    /// NOT inject `"milestones":[]`, which would churn the content hash/revision,
    /// mark the file git-dirty and trigger a no-op Oracle re-index.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub milestones: Vec<ProjectMilestone>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub id: String,
    pub title: String,
    pub status: String,
    pub updated_at: String,
    pub root_path: Option<String>,
    pub revision: String,
    pub path: String,
    pub task_counts: ProjectTaskCounts,
    pub git_status: ProjectGitStatus,
    /// Calendar milestones for this project, surfaced on the summary so the Board
    /// calendar can aggregate across all projects from the cheap `list_projects`
    /// result without loading each project's full detail. `#[serde(default)]` keeps
    /// any cached/older summary JSON parseable.
    #[serde(default)]
    pub milestones: Vec<ProjectMilestone>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLiveResourceStatus {
    pub provider: ProviderId,
    pub resource_id: String,
    pub label: String,
    pub status: String,
    pub resource_type: String,
    pub region: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLiveStatus {
    pub resources: Vec<ProjectLiveResourceStatus>,
    pub checked_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGitRepoCandidate {
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub origin: Option<String>,
    pub dirty_count: u32,
    pub clone_command: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGitStatus {
    pub root_path: Option<String>,
    pub repo_root: Option<String>,
    pub repo_name: Option<String>,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub origin: Option<String>,
    pub github_url: Option<String>,
    pub clone_command: Option<String>,
    pub pull_request_url: Option<String>,
    pub commit: Option<String>,
    pub dirty_count: u32,
    pub staged_count: u32,
    pub unstaged_count: u32,
    pub untracked_count: u32,
    pub ahead_count: u32,
    pub behind_count: u32,
    pub is_git_repo: bool,
    pub is_github: bool,
    pub policy_status: String,
    pub warnings: Vec<String>,
    pub required_actions: Vec<String>,
    pub suggested_repos: Vec<ProjectGitRepoCandidate>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubConnectionStatus {
    pub configured: bool,
    pub status: String,
    pub source: String,
    pub cli_available: bool,
    pub login: Option<String>,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub profile_url: Option<String>,
    pub scopes: Vec<String>,
    pub rate_limit_remaining: Option<u32>,
    pub last_checked_at: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepoAccessStatus {
    pub url: String,
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub description: Option<String>,
    pub accessible: bool,
    pub private: Option<bool>,
    pub default_branch: Option<String>,
    pub open_issues_count: Option<u32>,
    pub stargazers_count: Option<u32>,
    pub forks_count: Option<u32>,
    pub pushed_at: Option<String>,
    pub updated_at: Option<String>,
    pub permissions: Vec<String>,
    pub status: String,
    pub checked_at: String,
    pub message: Option<String>,
}

/// Access role bound to a device identity. Extensible: add a variant here and a
/// row in `roles::role_permissions` to introduce a new collaborator type. The
/// default is the LEAST-privileged role, so any ambiguity fails safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Role {
    Admin,
    #[default]
    Collaborator,
}


/// A role assignment the admin issues to a collaborator's device identity. The
/// admin signs the canonical digest of this with their Ed25519 device signing
/// key; the collaborator's app verifies it against the bundled trust anchor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleGrant {
    pub role: Role,
    /// X25519 key-exchange public key of the subject device (hex).
    pub subject_public_key: String,
    /// Ed25519 signing public key of the subject device (hex).
    pub subject_signing_public_key: String,
    /// Key-exchange fingerprint of the subject device.
    pub subject_fingerprint: String,
    pub issued_at: String,
    /// RFC3339 expiry; `None` means the grant never expires (discouraged —
    /// prefer device revocation + cloud-token rotation for offboarding).
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// `RoleGrant` plus the admin's Ed25519 signature over its canonical digest.
/// Mirrors the package signature block shape (`PackageSignatureBlock`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedRoleGrant {
    pub grant: RoleGrant,
    pub scheme: String,
    pub issuer_signing_public_key: String,
    pub issuer_fingerprint: String,
    pub signature: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceVaultStatus {
    pub configured: bool,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub platform: String,
    pub vault_backend: String,
    pub biometric_label: String,
    pub public_key: Option<String>,
    pub public_key_fingerprint: Option<String>,
    pub private_key_configured: bool,
    // Ed25519 signing identity (additive; separate from the X25519 key-exchange
    // key above). Older device records without a signing key serialize these as
    // absent / false and regenerate the signing key on the next ensure.
    #[serde(default)]
    pub signing_public_key: Option<String>,
    #[serde(default)]
    pub signing_fingerprint: Option<String>,
    #[serde(default)]
    pub signing_key_configured: bool,
    pub created_at: Option<String>,
    pub last_checked_at: String,
    pub security_level: String,
    pub join_request: Option<String>,
    pub message: Option<String>,
    /// The verified role this install runs as: `Admin` when this device's signing
    /// key is the trust anchor, otherwise the role from a verified grant, else
    /// `None` (unprovisioned). Derived — never trusted from the wire.
    #[serde(default)]
    pub role: Option<Role>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInviteRecord {
    pub id: String,
    pub collaborator_name: String,
    pub device_name: String,
    pub platform: String,
    pub public_key: String,
    pub public_key_fingerprint: String,
    // Ed25519 signing identity of the invited device, when the join request
    // carried it. Additive: older approved invites have these as None and are
    // simply treated as "signing key unknown" for signer identity pinning.
    #[serde(default)]
    pub signing_public_key: Option<String>,
    #[serde(default)]
    pub signing_fingerprint: Option<String>,
    pub status: String,
    pub created_at: String,
    pub approved_at: Option<String>,
    pub revoked_at: Option<String>,
    pub notes: Option<String>,
    /// Role the admin assigned to this device when approving it. Additive: older
    /// approved invites have `None` and are treated as the default (Collaborator).
    #[serde(default)]
    pub role: Option<Role>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInviteInput {
    pub collaborator_name: String,
    pub join_request: String,
    pub notes: Option<String>,
    /// Role the admin picks at approval time; `None` defaults to Collaborator.
    #[serde(default)]
    pub role: Option<Role>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicesInvitesSnapshot {
    pub local_device: DeviceVaultStatus,
    pub invites: Vec<DeviceInviteRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetail {
    pub metadata: ProjectMetadata,
    pub state: ProjectStateBlock,
    pub markdown: String,
    pub revision: String,
    pub path: String,
    pub modified_at: Option<String>,
    pub live_status: ProjectLiveStatus,
    pub git_status: ProjectGitStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCreateInput {
    pub id: Option<String>,
    pub title: String,
    pub status: Option<String>,
    pub root_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMetadataPatch {
    pub title: Option<String>,
    pub status: Option<String>,
    pub root_path: Option<String>,
    pub expected_revision: String,
}

/// Phase D — the design "Save & hand off to agents" payload. Carries the design
/// working folder (a `.devboule-design/<project>` bundle: design.md, manifest.json,
/// components/, tokens.json, exports, preview.png) the launched coder must implement.
/// `working_folder_path` is the ONLY field and is validated server-side
/// (canonicalized, must exist, be a dir, contain `project.json`, and live UNDER the
/// target project's root_path) before any of it reaches the launch prompt — so no
/// caller-controlled free text ever enters the prompt addendum. camelCase over IPC
/// like its siblings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignHandoffInput {
    pub working_folder_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectWorkflowRunInput {
    pub name: String,
    #[serde(default)]
    pub args: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAgentLaunchInput {
    pub project_id: String,
    pub role: String,
    pub client: String,
    pub agent_id: Option<String>,
    pub task_id: Option<String>,
    /// Where the agent terminal runs: "app" (hosted inside Aspis Management via a
    /// PTY) or "external" (a detached OS console window). Optional and lenient:
    /// `None`/unknown values normalize to "external" so existing callers (and the
    /// current TS invoke, which sends no `host`) keep the legacy external behavior
    /// with zero regression. camelCase over IPC like its siblings.
    #[serde(default)]
    pub host: Option<String>,
    /// Advisory model hint chosen in the Spawn panel (opus/sonnet/haiku/custom).
    /// Threaded into the launch prompt's agent_register model= placeholder so the
    /// operator's intended model seeds fleet counts before the agent self-reports.
    /// Optional and lenient: `None` keeps the "<your model>" self-report
    /// placeholder. camelCase over IPC like its siblings.
    #[serde(default)]
    pub model: Option<String>,
    /// Phase H: when `Some(true)`, this verifier launch is a Censor "final
    /// review" — the launch prompt gains the residual-adjudication addendum
    /// (call `censor_findings(project_id)` for the residual ledger, adjudicate,
    /// `censor_dispose`). Optional and lenient: `None`/`Some(false)` leaves the
    /// verifier prompt UNCHANGED, so every existing launch caller (SpawnPanel,
    /// the current TS invoke that sends no `censorReview`) keeps the legacy
    /// behavior with zero regression. Ignored for the coder role (the coder's
    /// per-step Censor addendum is unconditional). camelCase over IPC.
    #[serde(default)]
    pub censor_review: Option<bool>,
    /// Per-launch LANGUAGE-PERSONA override (the Spawn panel's language selector). When
    /// `Some(non-empty)` it bypasses the project's auto-detected primary language for the
    /// (role × language) persona-skill layer; `None`/empty ⇒ auto-detect. camelCase over IPC;
    /// the lenient default keeps every existing caller byte-identical.
    #[serde(default)]
    pub language_override: Option<String>,
    /// 3b: when `Some(true)` AND `client == "orchestrator"`, the local Devboule coder is
    /// launched in PLAN-FIRST mode — the launch adds `DEVBOULE_PLAN_FIRST=1` to the
    /// orchestrator binary's env so its system prompt biases toward producing a plan
    /// (and submitting it for approval via `plan_submit`) before any other action.
    /// Optional and lenient: `None`/`Some(false)` omits the env entirely, so the default
    /// launch is byte-identical and every existing caller (codex/claude, the current TS
    /// invokes that send no `planFirst`) is unaffected. Ignored for non-orchestrator
    /// clients (they have no planner / read no such env). camelCase over IPC.
    #[serde(default)]
    pub plan_first: Option<bool>,
    /// Orchestrator composer "Plan it": the typed GOAL. When `Some(non-empty)` AND
    /// `client == "orchestrator"`, the launch sets `DEVBOULE_GOAL` so the orchestrator runs
    /// headless on that goal (plan-first) instead of waiting for interactive TUI input. Optional +
    /// lenient: `None`/empty omits the env (byte-identical default launch). camelCase over IPC.
    #[serde(default)]
    pub initial_goal: Option<String>,
    /// Orchestrator composer auto-create toggle. `Some(false)` sets `DEVBOULE_AUTO_CREATE=0` so the
    /// planner drafts + submits the plan but does NOT create its tasks on approval (you create them).
    /// `None`/`Some(true)` omits the env ⇒ the existing behavior (tasks created on approval). camelCase.
    #[serde(default)]
    pub auto_create: Option<bool>,
    /// Phase D: when `Some`, this is a design "Save & hand off" dispatch — the coder's
    /// launch prompt gains a FIXED-WORDING addendum that points it at the validated
    /// design bundle and instructs it to implement that design (respecting design.md as
    /// the contract). Optional and lenient: `None` leaves every existing launch's prompt
    /// byte-for-byte unchanged, so SpawnPanel and the current TS invokes (which send no
    /// `designHandoff`) keep their behavior with zero regression. camelCase over IPC.
    #[serde(default)]
    pub design_handoff: Option<DesignHandoffInput>,
    /// Saved Claude Code workflow launch. Optional and lenient: absent keeps every
    /// existing launch unchanged. When present, the backend validates `name` against
    /// list_saved_workflows(projectId) and builds a fixed addendum; callers never
    /// provide arbitrary prompt text.
    #[serde(default)]
    pub workflow_run: Option<ProjectWorkflowRunInput>,
    /// Phase D: when `Some(true)` AND `client` is `"claude"`/`"codex"`, launch the cloud CLI as a
    /// DUPLEX orchestrator — a piped (non-PTY) child in its structured-streaming mode whose events
    /// are normalized into the activity bridge so it drives the SAME planner Stage as the local
    /// orchestrator (instead of an opaque terminal). Optional + lenient: `None`/`Some(false)` keeps
    /// the existing PTY/terminal launch byte-identical. Ignored for non-cloud clients. camelCase.
    #[serde(default)]
    pub cloud_duplex: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAgentLaunchResult {
    pub project_id: String,
    pub role: String,
    pub client: String,
    pub agent_id: String,
    pub root_path: String,
    pub prompt: String,
    pub launched: bool,
    pub message: String,
}

/// Result of a mutating git command (commit/push) run for a project's repo. The
/// refreshed git status lets the Work-mode top bar update in place; the human
/// message summarizes the operation. Surfaced verbatim from the backend; never
/// carries raw subprocess stdout that could leak a secret — only the trimmed
/// branch/summary already in `git_status`. On FAILURE the command returns
/// `Err(stderr)` instead of this struct, so the UI can show the git error.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGitCommandResult {
    pub project_id: String,
    pub branch: String,
    pub message: String,
    pub git_status: ProjectGitStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTaskInput {
    pub title: String,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub assignee: Option<String>,
    pub due: Option<String>,
    pub linked_resources: Option<Vec<ProjectLinkedResource>>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub expected_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectNoteInput {
    pub text: String,
    pub source: Option<String>,
    pub expected_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRoleRule {
    pub role: String,
    pub summary: String,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub forbidden: Vec<String>,
    // Practical "what every agent of this role must DO" mandate strings. Mirrors
    // the `contract` lists in oracle/server/aspis_mcp.py ROLE_RULES — the two
    // copies MUST stay verbatim-identical (anti-drift). skip-if-empty so older
    // JSON without this field still deserializes and Rust never injects an empty
    // array into a file Python owns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contract: Vec<String>,
    // PHASE E: the role's Censor-consumption mandate (coder per-step, verifier
    // residual adjudication). Mirrors the `censor` list in
    // oracle/server/aspis_mcp.py ROLE_RULES — kept SEPARATE from `contract`
    // because, unlike the shared 3-line contract, this mandate differs per role.
    // INTENTIONAL BILINGUAL SPLIT: English here (feeds the fleet UI), Italian in
    // Python (agents read it) — same as summary/forbidden. skip-if-empty so older
    // JSON without the field still deserializes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub censor: Vec<String>,
    // GH-P5: the role's cooperative git-push mandate (coder only). Mirrors the
    // `push` list in oracle/server/aspis_mcp.py ROLE_RULES — agents may commit
    // freely but must NEVER raw `git push` (the agent launch env carries no
    // credentials, so a raw push fails); to publish they call the
    // `request_git_push` MCP tool and a human approves. Kept SEPARATE from
    // `contract` because, like `censor`, this mandate is per-role (verifier has
    // no push capability and gets NO push field). INTENTIONAL BILINGUAL SPLIT:
    // English here (feeds the fleet UI), Italian in Python (agents read it) —
    // same as summary/forbidden/censor. skip-if-empty so older JSON without the
    // field still deserializes and Rust never injects an empty array into a file
    // Python owns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub push: Vec<String>,
    // Phase 1: the role's plan mandate (coder only). Mirrors the `plan` list in
    // oracle/server/aspis_mcp.py ROLE_RULES — the coder submits a plan via plan_submit
    // before multi-file work, waits for approval, revises on reject per the note, and
    // uses ask_user for blocking questions. Kept SEPARATE from `contract` because,
    // like `censor`/`push`, this mandate is per-role. INTENTIONAL BILINGUAL SPLIT:
    // English here (feeds the fleet UI), Italian in Python (agents read it). skip-if-
    // empty so older JSON without the field still deserializes and Rust never injects
    // an empty array into a file Python owns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan: Vec<String>,
}

// One subagent breakdown entry an orchestrator/coder/verifier reports via the
// MCP agent_heartbeat `subagents` payload, surfaced in the fleet UI. Mirrors the
// shape produced by oracle/server/aspis_mcp.py. Additive: see AgentSession.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSubagent {
    pub label: String,
    pub model: String,
    pub count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

// Set when an agent reports it is blocked waiting on the human (a question, an
// allow/deny permission prompt, or a hard block). Mirrors the Python MCP shape.
// Every field is `#[serde(default)]` so a partial object (e.g. `{"reason":"x"}`)
// from a hand-edited or half-written file still deserializes instead of bricking
// the whole agent-state read; missing fields become "". See `lenient_needs_user`
// for the session-level guard that maps a wrong-typed or all-empty value to None.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentNeedsUser {
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub since: String,
}

impl AgentNeedsUser {
    // An all-empty needsUser ({} -> "","","") carries no signal: the UI must not
    // ring a ghost "needs you" bell for it. A partial object with ANY non-empty
    // field has signal and stays Some.
    fn is_empty(&self) -> bool {
        self.reason.is_empty() && self.message.is_empty() && self.since.is_empty()
    }
}

// Per-entry-lenient subagents deserializer. The MCP/agent-written file is the
// source of truth and Python owns clamping, but a hand-edited or half-written
// entry (`count: -1`, `count: 9999999999`, wrong-typed `label`) used to hard-fail
// `serde_json::from_str::<AgentLiveState>` and brick `get_agent_live_state`
// forever. Here we read the field as raw `Value`s and DROP any entry that fails
// to deserialize into `AgentSubagent`, keeping the valid ones. A non-array value
// (e.g. `"garbage"`) degrades to an empty list rather than erroring.
fn lenient_subagents<'de, D>(deserializer: D) -> Result<Vec<AgentSubagent>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    let entries = match raw {
        serde_json::Value::Array(items) => items,
        // null/non-array -> empty fleet contribution, never an error.
        _ => return Ok(Vec::new()),
    };
    Ok(entries
        .into_iter()
        .filter_map(|entry| serde_json::from_value::<AgentSubagent>(entry).ok())
        .collect())
}

// Session-level-lenient needsUser deserializer. A wrong-typed value (e.g. `42`)
// maps to None instead of erroring; an all-empty object collapses to None so the
// UI never sees a ghost empty "needs you". A partial object with content stays
// Some (fields default to "" via AgentNeedsUser's serde defaults).
fn lenient_needs_user<'de, D>(deserializer: D) -> Result<Option<AgentNeedsUser>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    if raw.is_null() {
        return Ok(None);
    }
    match serde_json::from_value::<AgentNeedsUser>(raw) {
        Ok(needs) if needs.is_empty() => Ok(None),
        Ok(needs) => Ok(Some(needs)),
        // Wrong type entirely -> treat as "no signal" rather than bricking.
        Err(_) => Ok(None),
    }
}

// Per-entry-lenient mini-coder directives deserializer. The directive array in
// `.aspis-agents.json` is written by the coder's MCP `spawn_mini_coder` tool and
// round-tripped by the Rust executor; a hand-edited or half-written directive
// (wrong-typed `task`, garbage `status`, missing `resultPath`) must DROP only
// that entry rather than brick the whole `AgentLiveState` read (which would
// freeze `get_agent_live_state` and the fleet UI). Mirrors `lenient_subagents`:
// read raw `Value`s, drop any that fail to deserialize into `MiniCoderDirective`,
// and degrade a non-array value to an empty queue instead of erroring.
fn lenient_mini_coder_directives<'de, D>(
    deserializer: D,
) -> Result<Vec<crate::backend::mini_coder::MiniCoderDirective>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    let entries = match raw {
        serde_json::Value::Array(items) => items,
        // null/non-array -> empty queue, never an error.
        _ => return Ok(Vec::new()),
    };
    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            serde_json::from_value::<crate::backend::mini_coder::MiniCoderDirective>(entry).ok()
        })
        .collect())
}

fn lenient_visual_check_directives<'de, D>(
    deserializer: D,
) -> Result<Vec<crate::backend::visual_check::VisualCheckDirective>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    let entries = match raw {
        serde_json::Value::Array(items) => items,
        _ => return Ok(Vec::new()),
    };
    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            serde_json::from_value::<crate::backend::visual_check::VisualCheckDirective>(entry)
                .ok()
        })
        .collect())
}

// Per-entry-lenient design-request deserializer (Phase B). The `designRequestDirectives`
// array is written by the orchestrator's MCP `design_request` tool; a malformed/half-written
// entry is dropped rather than failing the whole state load (mirrors the visual_check one).
fn lenient_design_request_directives<'de, D>(
    deserializer: D,
) -> Result<Vec<crate::backend::design_request::DesignRequestDirective>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    let entries = match raw {
        serde_json::Value::Array(items) => items,
        _ => return Ok(Vec::new()),
    };
    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            serde_json::from_value::<crate::backend::design_request::DesignRequestDirective>(entry)
                .ok()
        })
        .collect())
}

// Per-entry-lenient git push-request deserializer (GH-P4). The `gitPushRequests`
// array in `.aspis-agents.json` is written by the agent's MCP `request_git_push`
// tool and round-tripped by the Rust approve/deny commands; a hand-edited or
// half-written request (wrong-typed `projectId`, garbage `status`, missing `id`)
// must DROP only that entry rather than brick the whole `AgentLiveState` read
// (which would freeze `get_agent_live_state` and the fleet UI). Mirrors
// `lenient_mini_coder_directives`: read raw `Value`s, drop any that fail to
// deserialize into `GitPushRequest`, degrade a non-array value to an empty queue.
fn lenient_git_push_requests<'de, D>(
    deserializer: D,
) -> Result<Vec<crate::backend::git_push::GitPushRequest>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    let entries = match raw {
        serde_json::Value::Array(items) => items,
        _ => return Ok(Vec::new()),
    };
    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            serde_json::from_value::<crate::backend::git_push::GitPushRequest>(entry).ok()
        })
        .collect())
}

// Per-entry-lenient consent-request deserializer (Slice 5b). The `consentRequests`
// array in `.aspis-agents.json` is written by the Claude PreToolUse hook bin and
// round-tripped by the Rust `respond_cloud_consent` command + the Python MCP; a
// hand-edited or half-written request (garbage `status`, missing `kind`) must DROP
// only that entry rather than brick the whole `AgentLiveState` read. Mirrors
// `lenient_git_push_requests`: read raw `Value`s, drop any that fail to deserialize
// into `ConsentBridgeRequest`, degrade a non-array value to an empty queue.
fn lenient_consent_requests<'de, D>(
    deserializer: D,
) -> Result<Vec<crate::backend::consent_bridge::ConsentBridgeRequest>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    let entries = match raw {
        serde_json::Value::Array(items) => items,
        _ => return Ok(Vec::new()),
    };
    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            serde_json::from_value::<crate::backend::consent_bridge::ConsentBridgeRequest>(entry)
                .ok()
        })
        .collect())
}

// Per-entry-lenient plan-approval-request deserializer (Phase 1). The
// `planApprovalRequests` array in `.aspis-agents.json` is written by the agent's MCP
// `plan_submit` tool and round-tripped by the Rust approve/deny commands; a
// hand-edited or half-written request (wrong-typed `projectId`, an UNKNOWN status
// string, missing `id`) must DROP only that entry rather than brick the whole
// `AgentLiveState` read. Mirrors `lenient_git_push_requests`: read raw `Value`s, drop
// any that fail to deserialize into `PlanApprovalRequest`, degrade a non-array value
// to an empty queue.
fn lenient_plan_approval_requests<'de, D>(
    deserializer: D,
) -> Result<Vec<PlanApprovalRequest>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    let entries = match raw {
        serde_json::Value::Array(items) => items,
        _ => return Ok(Vec::new()),
    };
    Ok(entries
        .into_iter()
        .filter_map(|entry| serde_json::from_value::<PlanApprovalRequest>(entry).ok())
        .collect())
}

/// One plan-approval request in the `.aspis-agents.json` `planApprovalRequests` queue
/// (Phase 1). The agent's MCP `plan_submit` tool appends it as
/// `status:"pending_approval"`; the human's approve/deny Tauri command drives the
/// rest and stamps `decidedAt` + `note`. Also the shape of the on-disk plan SIDECAR
/// (`<plan_id>.json`).
///
/// camelCase over the wire; EVERY field `#[serde(default)]` so a partial / hand-edited
/// / older-writer request still deserializes (the state-level
/// `lenient_plan_approval_requests` then drops only an entry that fails entirely).
/// `status` defaults to `pending_approval`; an unknown status string fails THIS entry
/// (dropped by the lenient list reader) without bricking the whole state. Optional
/// `decidedAt`/`note` are additive + skip-if-none so a still-pending request and the
/// Python round-trip never gain churn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlanApprovalRequest {
    #[serde(default)]
    pub id: String,
    /// The requesting agent session id (the coder submitting the plan). Used to clear
    /// its `needsUser` bell and attribute the request in the card.
    #[serde(default)]
    pub agent_id: String,
    /// The project the plan is for.
    #[serde(default)]
    pub project_id: String,
    /// Short human-readable plan title (display in the card).
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: crate::backend::plan_approval::PlanApprovalStatus,
    #[serde(default)]
    pub created_at: String,
    /// RFC3339 timestamp the human decided (set by the approve/deny command). Absent
    /// until decided. NO-CHURN: skip-if-none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<String>,
    /// Optional human note: revise-instructions on reject, an approval remark on
    /// approve. NO-CHURN: skip-if-none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Per-session `pendingQuestion` (Phase 1): written by the agent's MCP `ask_user`
/// tool when it blocks on a human answer; read by the human reply-box and the fleet
/// UI. Rust treats it as a PASSTHROUGH co-owned value (Python writes it, Rust reads
/// the `id` to stamp the reply). camelCase; additive defaults so a partial value still
/// loads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentPendingQuestion {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub question: String,
    #[serde(default)]
    pub created_at: String,
}

/// Per-session `userReply` (Phase 1): written by the human reply-box (`reply_to_agent`
/// Tauri command) in answer to a `pendingQuestion`. Python's bounded poll consumes it
/// and clears both fields. camelCase; additive defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentUserReply {
    /// The `pendingQuestion.id` this reply answers (so a stale reply for a prior
    /// question can be detected by the poll).
    #[serde(default)]
    pub question_id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub agent_id: String,
    // BLOCKER C: a hand-edited / partially-written `.aspis-agents.json` session
    // missing the `role` field must NOT fail the whole `AgentLiveState`
    // deserialize (that would brick the entire fleet read, not just one session).
    // Default to "" on absence; the Python MCP normalizes "" -> "coder" on load.
    #[serde(default)]
    pub role: String,
    pub model: Option<String>,
    pub status: String,
    pub message: Option<String>,
    // Launch CLI for this agent (codex/claude/powershell). The app knows it at
    // launch time. Additive + skip-if-none so older JSON (and the Python MCP
    // server round-tripping the file without this field) still deserialize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    pub current_project_id: Option<String>,
    pub current_task_id: Option<String>,
    // File the agent is currently working on. Polis uses it to land the agent on
    // the real building for that file. Additive + skip-if-none so older JSON (and
    // the Python MCP round-trip) still deserialize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_file_path: Option<String>,
    pub first_seen_at: Option<String>,
    pub last_seen_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_token_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_token_issued_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_token_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_token_issued_at: Option<String>,
    // Subagent breakdown reported by the agent (orchestrator fan-out, coder
    // helpers, etc). Additive + skip-if-empty so older JSON and the Python MCP
    // round-trip (which owns this file) still deserialize and Rust never injects
    // an empty array. Written by Python; read by the Rust fleet aggregation.
    #[serde(
        default,
        deserialize_with = "lenient_subagents",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub subagents: Vec<AgentSubagent>,
    // Present when the agent is blocked on the human. Additive + skip-if-none so
    // the round-trip never drops nor injects it. camelCase rename -> needsUser.
    // Lenient on read: a wrong-typed or all-empty value collapses to None instead
    // of bricking the whole state parse (see `lenient_needs_user`).
    #[serde(
        default,
        deserialize_with = "lenient_needs_user",
        skip_serializing_if = "Option::is_none"
    )]
    pub needs_user: Option<AgentNeedsUser>,
    // Terminal host for this agent: "app" (PTY hosted inside Aspis Management),
    // "external" (detached OS console), or None when the session was not launched
    // by the app. For a LIVE agent this is a READ-TIME stamp set in
    // `get_agent_live_state` from the Rust-owned ledger (authoritative). For a
    // CLOSED app agent it is also PERSISTED once, by `mark_agent_session_closed`
    // (host="app"), because the ledger entry is pruned when the PTY dies — that
    // durable value is what lets the UI show a "Terminal exited — relaunch" hint
    // instead of a dead Open CLI button. Launch-time constructors set `host: None`
    // (no premature persistence). Additive + skip-if-none so the Python MCP
    // round-trip never drops nor injects it. The UI uses it to decide whether
    // "Open CLI" (external only) is meaningful for the row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    // Set when this session is a MINI-CODER spawned by another agent (its parent
    // coder). "is mini" == `parent_agent_id.is_some()`. The mini is a real
    // app-hosted PTY session nested under its parent in the work-mode rail; the
    // parent coder is the only human-contact point (the mini never escalates to
    // the human). Additive + skip-if-none so older JSON, the Python MCP
    // round-trip, and every NON-mini session stay byte-identical (no churn — the
    // key is simply absent for ordinary agents). See backend/mini_coder.rs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    // Phase 1 reply-box: present when the agent's MCP `ask_user` tool blocked on a
    // human answer. Python OWNS this field (it writes/clears it); Rust reads its `id`
    // to stamp `user_reply`. Additive + skip-if-none + camelCase so the Python MCP
    // round-trip and every non-asking session stay byte-identical (no churn — the key
    // is simply absent otherwise). See backend/plan_approval.rs `reply_to_agent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_question: Option<AgentPendingQuestion>,
    // Phase 1 reply-box: the human's answer to `pending_question`, written by the
    // `reply_to_agent` Tauri command. Python's bounded poll consumes it and clears
    // both fields. Additive + skip-if-none + camelCase (passthrough co-ownership — it
    // must round-trip Python's value untouched).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_reply: Option<AgentUserReply>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentClaim {
    pub project_id: String,
    pub project_title: Option<String>,
    pub task_id: String,
    pub task_title: Option<String>,
    pub agent_id: String,
    pub role: String,
    pub status: String,
    pub claimed_at: String,
    pub updated_at: String,
    pub lease_until: Option<String>,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub id: String,
    pub timestamp: String,
    pub agent_id: String,
    pub role: String,
    pub event_type: String,
    pub project_id: Option<String>,
    pub task_id: Option<String>,
    pub status: Option<String>,
    pub message: String,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLiveState {
    pub version: u32,
    pub updated_at: String,
    #[serde(default)]
    pub sessions: Vec<AgentSession>,
    #[serde(default)]
    pub claims: Vec<AgentClaim>,
    #[serde(default)]
    pub events: Vec<AgentEvent>,
    #[serde(default)]
    pub rules: Vec<AgentRoleRule>,
    #[serde(default)]
    pub state_path: String,
    #[serde(default)]
    pub mcp_command: String,
    #[serde(default)]
    pub mcp_client_config: String,
    // Mini-coder spawn directives (coder -> app), the queue the headless executor
    // (backend/mini_coder.rs, wired in P2) drains. Additive + skip-if-empty so an
    // existing `.aspis-agents.json` with no mini-coder activity is NOT rewritten
    // with an injected empty `miniCoderDirectives` key (no churn), and older
    // builds / the Python MCP round-trip still deserialize. Per-entry lenient: one
    // malformed directive is dropped, never bricks the whole state read.
    #[serde(
        default,
        deserialize_with = "lenient_mini_coder_directives",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub mini_coder_directives: Vec<crate::backend::mini_coder::MiniCoderDirective>,
    // Visual-check directives (agent -> app): the Python MCP appends pending
    // requests; the Rust executor renders/captures/critiques and stamps result.
    #[serde(
        default,
        deserialize_with = "lenient_visual_check_directives",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub visual_check_directives: Vec<crate::backend::visual_check::VisualCheckDirective>,
    // Design-request directives (orchestrator -> app, Phase B): the Python MCP appends
    // pending design generations; the executor claims one + emits a Tauri event the
    // frontend runs (reusing the design pipeline), then stamps the result back.
    #[serde(
        default,
        deserialize_with = "lenient_design_request_directives",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub design_request_directives:
        Vec<crate::backend::design_request::DesignRequestDirective>,
    // GH-P4: agent→human git push-approval requests. The agent's MCP
    // `request_git_push` tool appends a `pending_approval` entry; the human's
    // approve/deny Tauri command drives the rest. Additive + skip-if-empty so an
    // existing `.aspis-agents.json` with no push activity is NOT rewritten with an
    // injected empty `gitPushRequests` key (no churn), and older builds / the Python
    // MCP round-trip still deserialize. Per-entry lenient: one malformed request is
    // dropped, never bricks the whole state read.
    #[serde(
        default,
        deserialize_with = "lenient_git_push_requests",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub git_push_requests: Vec<crate::backend::git_push::GitPushRequest>,
    // Phase 1: agent→human plan-approval requests. The agent's MCP `plan_submit` tool
    // appends a `pending_approval` entry; the human's approve/deny Tauri command
    // drives the rest. Additive + skip-if-empty so an existing `.aspis-agents.json`
    // with no plan activity is NOT rewritten with an injected empty
    // `planApprovalRequests` key (no churn), and older builds / the Python MCP
    // round-trip still deserialize. Per-entry lenient: one malformed request (incl. an
    // unknown status string) is dropped, never bricks the whole state read.
    #[serde(
        default,
        deserialize_with = "lenient_plan_approval_requests",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub plan_approval_requests: Vec<PlanApprovalRequest>,
    // Slice 5b: Claude→human consent requests. The Claude PreToolUse hook bin appends a
    // `pending_approval` entry; the human's `respond_cloud_consent` Tauri command claims
    // it terminal (allowed/denied). Additive + skip-if-empty so an existing
    // `.aspis-agents.json` with no consent activity is NOT rewritten with an injected
    // empty `consentRequests` key (no churn), and older builds / the Python MCP round-trip
    // still deserialize. Per-entry lenient: one malformed request is dropped, never bricks
    // the whole state read.
    #[serde(
        default,
        deserialize_with = "lenient_consent_requests",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub consent_requests: Vec<crate::backend::consent_bridge::ConsentBridgeRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSizeEntry {
    pub name: String,
    pub entry_type: String,
    pub path: String,
    pub size_gb: f64,
    pub file_count: u64,
    pub last_write: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLargeFile {
    pub relative_path: String,
    pub path: String,
    pub size_gb: f64,
    pub size_mb: f64,
    pub last_write: Option<String>,
    pub class_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceGitRepoStatus {
    pub name: String,
    pub path: String,
    pub relative_path: String,
    pub branch: String,
    pub origin: Option<String>,
    pub dirty_count: u32,
    pub git_size: Option<String>,
    pub clone_command: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceClassificationEntry {
    pub path: String,
    pub class_label: String,
    pub git: String,
    pub oracle: String,
    pub storage: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePolicyFile {
    pub name: String,
    pub path: String,
    pub exists: bool,
    pub line_count: u32,
    pub active_rules: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceHygieneSnapshot {
    pub root: String,
    pub workspace_dir: String,
    pub scanned_at: String,
    pub needs_scan: bool,
    pub total_size_gb: f64,
    pub total_files: u64,
    pub oracle_candidate_files: u64,
    pub top_level: Vec<WorkspaceSizeEntry>,
    pub large_files: Vec<WorkspaceLargeFile>,
    pub git_repos: Vec<WorkspaceGitRepoStatus>,
    pub classifications: Vec<WorkspaceClassificationEntry>,
    pub policy_files: Vec<WorkspacePolicyFile>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePackageRecipient {
    pub fingerprint: String,
    pub collaborator_name: String,
    pub device_name: String,
    pub platform: String,
    pub source: String,
    pub public_key: String,
    // Ed25519 signing public key + fingerprint of the device, when known.
    // Additive: recipients/devices without a signing key leave these as None.
    #[serde(default)]
    pub signing_public_key: Option<String>,
    #[serde(default)]
    pub signing_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePackageInfo {
    pub path: String,
    pub file_name: String,
    pub size_mb: f64,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePackageSnapshot {
    pub root: String,
    pub package_dir: String,
    pub import_dir: String,
    pub approved_recipients: Vec<WorkspacePackageRecipient>,
    pub latest_packages: Vec<WorkspacePackageInfo>,
    pub max_package_size_mb: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePackageResult {
    pub package_id: String,
    pub path: String,
    pub file_name: String,
    pub file_count: u64,
    pub total_bytes: u64,
    pub package_bytes: u64,
    pub recipient_count: u64,
    pub skipped_files: u64,
    pub skipped_bytes: u64,
    pub readme_path: String,
    pub created_at: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDecryptResult {
    pub package_id: String,
    pub output_dir: String,
    pub files_restored: u64,
    pub bytes_restored: u64,
    pub recipient_fingerprint: String,
    // Package signature provenance. `signature_valid` is always true here because
    // decrypt fails closed before extraction if the Ed25519 signature does not
    // verify; it is surfaced so the UI can render a positive "verified" state.
    pub signature_valid: bool,
    pub signer_public_key: String,
    pub signer_fingerprint: String,
    /// True when the signer's Ed25519 key matches the local device or an approved
    /// device; false means the signature is valid but the signer is unrecognized.
    pub signer_known: bool,
    pub signer_name: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderScopeSelection {
    pub provider: ProviderId,
    pub id: String,
    pub name: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConnectionAudit {
    pub provider: ProviderId,
    pub status: String,
    pub token_health: String,
    pub selected_scope: Option<ProviderScopeSelection>,
    pub resource_count: usize,
    pub message: Option<String>,
    pub risks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHealth {
    pub id: ProviderId,
    pub name: String,
    pub status: String,
    pub last_sync: Option<String>,
    pub token_health: String,
    pub credential_kind: Option<String>,
    pub resource_count: usize,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardKpi {
    pub id: String,
    pub label: String,
    pub value: String,
    pub subtext: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderServiceSummary {
    pub id: String,
    pub provider: ProviderId,
    pub category: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub coverage: String,
    pub live_count: usize,
    pub permission: String,
    pub docs_url: String,
    pub actions: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConsoleResourceSummary {
    pub id: String,
    pub provider: ProviderId,
    pub service_id: String,
    pub resource_type: String,
    pub name: String,
    pub region: Option<String>,
    pub status: String,
    pub description: String,
    pub metadata: Vec<String>,
    pub docs_url: String,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareWorkerSummary {
    pub id: String,
    pub account_id: String,
    pub account_name: Option<String>,
    pub name: String,
    pub status: String,
    pub purpose: String,
    pub purpose_source: String,
    pub routes: Vec<String>,
    pub last_deploy: Option<String>,
    pub usage_model: Option<String>,
    pub compatibility_date: Option<String>,
    pub compatibility_flags: Vec<String>,
    pub handlers: Vec<String>,
    pub tags: Vec<String>,
    pub oracle_query: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareWorkerBinding {
    pub name: String,
    pub binding_type: String,
    pub text: Option<String>,
    pub reference: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareWorkerSettings {
    pub account_id: String,
    pub worker_name: String,
    pub plain_text: Vec<CloudflareWorkerBinding>,
    pub secrets: Vec<CloudflareWorkerBinding>,
    pub other_bindings: Vec<CloudflareWorkerBinding>,
    pub compatibility_date: Option<String>,
    pub readable: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareEnvBindingChange {
    pub name: String,
    pub before: Option<String>,
    pub after: String,
    /// "update" when a `plain_text` binding with this name already exists,
    /// "create" when the variable would be appended as a new binding.
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareEnvDryRunResult {
    pub worker_name: String,
    pub var_name: String,
    pub changes: Vec<CloudflareEnvBindingChange>,
    pub preserved_secrets: Vec<String>,
    pub preserved_other: Vec<String>,
    pub api_equivalent: Vec<String>,
    pub risks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareEnvWriteResult {
    pub worker_name: String,
    pub var_name: String,
    pub applied: bool,
    pub message: String,
    pub written_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretRotationResult {
    pub provider: ProviderId,
    pub account_id: String,
    pub worker_name: String,
    pub secret_name: String,
    pub rotated_at: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareSmokeDryRunResult {
    pub provider: ProviderId,
    pub status: String,
    pub action: String,
    pub dry_run: bool,
    pub api_equivalent: Vec<String>,
    pub selected_scope: Option<ProviderScopeSelection>,
    pub credential_kind: Option<String>,
    pub token_health: String,
    pub resource_count: usize,
    pub can_rotate_worker_secret: bool,
    pub blocked_reason: Option<String>,
    pub message: String,
    pub risks: Vec<String>,
    pub audited_at: String,
}

/// Account-level subscription/plan as read from `/accounts/{id}/subscriptions`.
/// All fields are optional because the Cloudflare API omits unset rate-plan
/// attributes and we never fabricate values. Per-worker € cost is NOT available
/// from any Cloudflare API; only the account plan is.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareBillingPlan {
    pub id: Option<String>,
    pub name: Option<String>,
    pub currency: Option<String>,
    pub frequency: Option<String>,
    pub price: Option<f64>,
    pub component_summary: Option<String>,
}

/// One invoice/charge from `/user/billing/history`. Optional for the same reason
/// as the plan: the API may omit fields and we surface only what it returns.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareInvoiceSummary {
    pub id: Option<String>,
    pub occurred_at: Option<String>,
    pub amount: Option<f64>,
    pub currency: Option<String>,
    pub status: Option<String>,
    pub kind: Option<String>,
}

/// Lazily-loaded account billing view. `readable` is `true` when at least the
/// plans were read; `message` carries a human note about partial/no access.
/// Loaded on Billing-tab select, NOT part of the sync snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareBilling {
    pub plans: Vec<CloudflareBillingPlan>,
    pub invoices: Vec<CloudflareInvoiceSummary>,
    pub readable: bool,
    pub message: Option<String>,
}

/// One line of Scaleway consumption from `GET /billing/v2beta1/consumptions`.
/// `value_untaxed`/`currency` are flattened from the API's `value` `Money`
/// object (`currency_code`/`units`/`nanos`); `billing_period` is NOT on the line
/// itself and is back-filled from the request's `billing_period` param. All
/// fields optional because the API may omit them and we never fabricate values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalewayConsumptionLine {
    pub category: Option<String>,
    pub project_id: Option<String>,
    pub value_untaxed: Option<f64>,
    pub currency: Option<String>,
    pub billing_period: Option<String>,
}

/// One invoice from `GET /billing/v2alpha1/invoices`. `total_untaxed`/
/// `total_taxed`/`currency` are flattened from the API's `total_untaxed`/
/// `total_taxed` `Money` objects. The v2alpha1 invoice payload has no `state`
/// field, so it stays `None`; `stop_date` is sourced from the invoice `due_date`
/// (the closest end-of-period marker the payload exposes). All fields optional.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalewayInvoiceLine {
    pub id: Option<String>,
    pub issued_at: Option<String>,
    pub start_date: Option<String>,
    pub stop_date: Option<String>,
    pub total_untaxed: Option<f64>,
    pub total_taxed: Option<f64>,
    pub currency: Option<String>,
    pub state: Option<String>,
}

/// Lazily-loaded Scaleway organization billing view (NOT part of the sync
/// snapshot). Consumptions are the floor: `readable` is `true` when they were
/// read. Invoices are best-effort. Unlike Cloudflare, real per-category € cost
/// IS exposed. `message` carries a human note about partial/no access.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalewayBilling {
    pub consumptions: Vec<ScalewayConsumptionLine>,
    pub total_untaxed: Option<f64>,
    pub total_discount: Option<f64>,
    pub invoices: Vec<ScalewayInvoiceLine>,
    pub updated_at: Option<String>,
    pub readable: bool,
    pub message: Option<String>,
}

/// Editable AI Gateway settings surfaced to the UI. Read via
/// `GET /accounts/{id}/ai-gateway/gateways/{id}` and written back via a LOSSLESS
/// `PUT` (re-fetch, change only the edited fields, re-send the full object). All
/// fields are `Option` because the Cloudflare API omits unset attributes and we
/// never fabricate values. `readable: false` carries a degraded read.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareAiGatewaySettings {
    pub account_id: String,
    pub gateway_id: String,
    pub cache_ttl: Option<u64>,
    pub cache_invalidate_on_update: Option<bool>,
    pub collect_logs: Option<bool>,
    pub logpush: Option<bool>,
    pub rate_limiting_interval: Option<u64>,
    pub rate_limiting_limit: Option<u64>,
    pub rate_limiting_technique: Option<String>,
    pub readable: bool,
    pub message: Option<String>,
}

/// The subset of AI Gateway fields a caller may change. Every field is optional;
/// a `None` leaves the live value untouched on the lossless re-fetch/PUT.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareAiGatewaySettingsPatch {
    pub cache_ttl: Option<u64>,
    pub cache_invalidate_on_update: Option<bool>,
    pub collect_logs: Option<bool>,
    pub logpush: Option<bool>,
    pub rate_limiting_interval: Option<u64>,
    pub rate_limiting_limit: Option<u64>,
    pub rate_limiting_technique: Option<String>,
}

/// Result of triggering an AI Search (AutoRAG) sync/reindex job.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareAutoragReindexResult {
    pub instance_id: String,
    pub job_id: Option<String>,
    pub triggered_at: String,
    pub message: String,
}

/// A single KV key from `GET .../keys`. `metadata` is surfaced as a compact JSON
/// string when present so the UI can show it without a typed schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareKvKey {
    pub name: String,
    pub expiration: Option<i64>,
    pub metadata: Option<String>,
}

/// A page of KV keys. `cursor` is the opaque continuation token (`None` when the
/// listing is complete or the page cap was hit).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareKvKeysPage {
    pub namespace_id: String,
    pub keys: Vec<CloudflareKvKey>,
    pub cursor: Option<String>,
    pub list_complete: bool,
}

/// A single KV value read. The value is capped in size; oversized values are
/// reported as truncated rather than streamed wholesale into the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareKvValue {
    pub namespace_id: String,
    pub key: String,
    pub value: String,
    pub truncated: bool,
}

/// Result of a KV write or delete.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareKvWriteResult {
    pub namespace_id: String,
    pub key: String,
    pub action: String,
    pub applied: bool,
    pub message: String,
    pub written_at: String,
}

/// Result of a D1 query. `isWrite` reflects the PURE classification of the SQL;
/// `requiresConfirmation` is `true` when the statement is a write and the caller
/// did not pass `confirm: true` (no execution happened in that case). `columns`
/// and `rows` are the flattened first result set; `rows` is capped.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareD1QueryResult {
    pub database_id: String,
    pub is_write: bool,
    pub requires_confirmation: bool,
    pub executed: bool,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub row_count: usize,
    pub truncated: bool,
    pub rows_read: Option<u64>,
    pub rows_written: Option<u64>,
    pub message: String,
}

/// R2 bucket lifecycle + CORS configuration. Both are surfaced as opaque JSON
/// values (`serde_json::Value`) so the UI can display them and round-trip them on
/// a set without us re-modelling Cloudflare's evolving rule schema. `readable`
/// distinguishes a degraded read from a genuinely empty config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareR2Config {
    pub bucket: String,
    pub lifecycle_rules: serde_json::Value,
    pub cors_rules: serde_json::Value,
    pub lifecycle_readable: bool,
    pub cors_readable: bool,
    pub message: Option<String>,
}

/// Result of an R2 lifecycle or CORS write.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareR2WriteResult {
    pub bucket: String,
    pub target: String,
    pub applied: bool,
    pub message: String,
    pub written_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalewayActionResult {
    pub provider: ProviderId,
    pub resource_id: String,
    pub resource_name: String,
    pub resource_type: String,
    pub action: String,
    pub triggered_at: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalewayResourceSummary {
    pub id: String,
    pub name: String,
    pub resource_type: String,
    pub region: String,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub state: String,
    pub commercial_type: Option<String>,
    pub runtime: Option<String>,
    pub min_scale: Option<u32>,
    pub max_scale: Option<u32>,
    pub domain_name: Option<String>,
    /// Connection endpoint (e.g. Serverless SQL psql DSN). `None` for resources
    /// that do not expose a direct endpoint.
    pub endpoint: Option<String>,
    pub privacy: Option<String>,
    pub purpose: String,
    pub purpose_source: String,
    pub tags: Vec<String>,
    pub image: Option<String>,
    pub public_ip: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub oracle_query: String,
    pub available_actions: Vec<String>,
    pub idle_cost_risk: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalewayStorageSummary {
    pub id: String,
    pub name: String,
    pub storage_type: String,
    pub region: String,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub state: String,
    pub size_gb: f64,
    pub price_eur_per_gb_hour: Option<f64>,
    pub estimated_eur_month: Option<f64>,
    pub pricing_label: String,
    pub pricing_note: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub tags: Vec<String>,
    pub billable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalewayOfferSummary {
    pub id: String,
    pub name: String,
    pub zone: String,
    pub category: String,
    pub architecture: String,
    pub vcpus: u32,
    pub memory_gb: f64,
    pub gpu_count: u32,
    pub gpu_label: Option<String>,
    pub hourly_price_eur: Option<f64>,
    pub monthly_price_eur: Option<f64>,
    pub availability: String,
    pub tags: Vec<String>,
}

/// Request to CREATE a Scaleway Instance (compute server). `image` is an image
/// UUID (validated as a UUID on the create/dry-run path); `commercial_type` is the
/// offer name (e.g. "GP1-S"); `project_id` MUST equal the pinned Aspis Bio project.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalewayInstanceCreateRequest {
    pub name: String,
    pub zone: String,
    pub commercial_type: String,
    pub image: String,
    pub project_id: String,
    pub dynamic_ip_required: bool,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Read-only preview of what `create_scaleway_instance` WOULD send, plus an
/// estimated cost looked up from the synced offer catalog. `estimated_*_eur` are
/// `None` (with a matching risk note) when the offer is not in the catalog — never
/// a fabricated zero. `body_preview` is the exact POST JSON, pretty-printed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalewayInstanceDryRunResult {
    pub zone: String,
    pub commercial_type: String,
    pub image: String,
    pub project_id: String,
    pub dynamic_ip_required: bool,
    pub estimated_hourly_eur: Option<f64>,
    pub estimated_monthly_eur: Option<f64>,
    pub body_preview: String,
    pub risks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskFlag {
    pub id: String,
    pub severity: String,
    pub title: String,
    pub description: String,
    pub source: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    pub id: String,
    pub message: String,
    pub timestamp: String,
    pub event_type: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudDashboardSnapshot {
    pub auth: AuthState,
    pub provider_health: Vec<ProviderHealth>,
    pub selected_scopes: Vec<ProviderScopeSelection>,
    pub kpis: Vec<DashboardKpi>,
    pub provider_services: Vec<ProviderServiceSummary>,
    pub console_resources: Vec<ProviderConsoleResourceSummary>,
    pub workers: Vec<CloudflareWorkerSummary>,
    pub compute: Vec<ScalewayResourceSummary>,
    pub storage: Vec<ScalewayStorageSummary>,
    pub scaleway_offers: Vec<ScalewayOfferSummary>,
    pub risks: Vec<RiskFlag>,
    pub activity: Vec<ActivityEvent>,
    pub last_sync_at: Option<String>,
}

impl CloudDashboardSnapshot {
    pub fn locked(auth: AuthState) -> Self {
        Self {
            auth,
            provider_health: Vec::new(),
            selected_scopes: Vec::new(),
            kpis: Vec::new(),
            provider_services: Vec::new(),
            console_resources: Vec::new(),
            workers: Vec::new(),
            compute: Vec::new(),
            storage: Vec::new(),
            scaleway_offers: Vec::new(),
            risks: Vec::new(),
            activity: Vec::new(),
            last_sync_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_input_without_host_deserializes_to_none() {
        // The current TS invoke sends NO `host`. It must deserialize (host = None),
        // which the launch path normalizes to "external" = zero behavior change.
        let json = r#"{
            "projectId": "p1",
            "role": "coder",
            "client": "codex",
            "agentId": null,
            "taskId": null
        }"#;
        let input: ProjectAgentLaunchInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.host, None);
    }

    #[test]
    fn launch_input_accepts_explicit_host() {
        let json = r#"{
            "projectId": "p1",
            "role": "coder",
            "client": "codex",
            "host": "app"
        }"#;
        let input: ProjectAgentLaunchInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.host.as_deref(), Some("app"));
        assert_eq!(input.agent_id, None);
        assert_eq!(input.task_id, None);
    }

    #[test]
    fn secret_status_serializes_without_token_value() {
        let status = SecretStatus {
            provider: ProviderId::Cloudflare,
            configured: true,
            status: "configured".into(),
            last_checked_at: Some("2026-05-27T00:00:00Z".into()),
            message: None,
        };

        let raw = serde_json::to_string(&status).unwrap();
        assert!(raw.contains("cloudflare"));
        assert!(!raw.contains("Bearer"));
        assert!(!raw.contains("tokenValue"));
    }

    #[test]
    fn oracle_llm_status_serializes_without_api_key() {
        let status = OracleLlmSettingsStatus {
            settings: OracleLlmSettings {
                provider: "scaleway".into(),
                model: "mistral-small-3.2-24b-instruct-2506".into(),
                base_url: Some("https://api.scaleway.ai/v1/chat/completions".into()),
                remote_enabled: true,
            },
            api_key_configured: true,
            status: "configured".into(),
            message: None,
        };

        let raw = serde_json::to_string(&status).unwrap();

        assert!(raw.contains("scaleway"));
        assert!(!raw.contains("sk-"));
        assert!(!raw.contains("apiKeyValue"));
        assert!(!raw.contains("secret"));
    }

    #[test]
    fn lenient_subagents_drops_malformed_entries_keeps_valid() {
        // A hand-edited / corrupt subagents list: count -1 (negative -> not u32),
        // count past u32 (9999999999), wrong-typed label, and two valid entries.
        // The bad ones must be dropped per-entry; the valid ones survive. One bad
        // entry must NOT brick the whole session/state read.
        let json = r#"{
            "agentId": "orch-1",
            "role": "orchestrator",
            "model": "opus",
            "status": "online",
            "message": null,
            "currentProjectId": null,
            "currentTaskId": null,
            "firstSeenAt": null,
            "lastSeenAt": null,
            "subagents": [
                {"label": "good-a", "model": "sonnet", "count": 2, "role": "coder"},
                {"label": "bad-neg", "model": "sonnet", "count": -1, "role": "coder"},
                {"label": "bad-huge", "model": "sonnet", "count": 9999999999, "role": "coder"},
                {"label": 42, "model": "haiku", "count": 1},
                {"label": "good-b", "model": "haiku", "count": 1}
            ]
        }"#;
        let session: AgentSession = serde_json::from_str(json).expect("session loads");
        assert_eq!(session.subagents.len(), 2);
        assert_eq!(session.subagents[0].label, "good-a");
        assert_eq!(session.subagents[0].count, 2);
        assert_eq!(session.subagents[1].label, "good-b");
    }

    #[test]
    fn lenient_subagents_non_array_becomes_empty() {
        let json = r#"{
            "agentId": "orch-1",
            "role": "orchestrator",
            "model": "opus",
            "status": "online",
            "message": null,
            "currentProjectId": null,
            "currentTaskId": null,
            "firstSeenAt": null,
            "lastSeenAt": null,
            "subagents": "garbage"
        }"#;
        let session: AgentSession = serde_json::from_str(json).expect("session loads");
        assert!(session.subagents.is_empty());
    }

    #[test]
    fn lenient_needs_user_empty_object_is_none() {
        // {} deserializes to all-empty strings; an all-empty needsUser carries no
        // signal and must collapse to None so the UI never rings a ghost bell.
        let json = r#"{
            "agentId": "c-1",
            "role": "coder",
            "model": "sonnet",
            "status": "online",
            "message": null,
            "currentProjectId": null,
            "currentTaskId": null,
            "firstSeenAt": null,
            "lastSeenAt": null,
            "needsUser": {}
        }"#;
        let session: AgentSession = serde_json::from_str(json).expect("session loads");
        assert_eq!(session.needs_user, None);
    }

    #[test]
    fn lenient_needs_user_partial_object_is_kept() {
        // A partial needsUser with content (reason only) has signal -> stays Some,
        // missing fields default to "".
        let json = r#"{
            "agentId": "c-1",
            "role": "coder",
            "model": "sonnet",
            "status": "online",
            "message": null,
            "currentProjectId": null,
            "currentTaskId": null,
            "firstSeenAt": null,
            "lastSeenAt": null,
            "needsUser": {"reason": "x"}
        }"#;
        let session: AgentSession = serde_json::from_str(json).expect("session loads");
        let needs = session.needs_user.expect("partial needsUser kept");
        assert_eq!(needs.reason, "x");
        assert_eq!(needs.message, "");
        assert_eq!(needs.since, "");
    }

    #[test]
    fn lenient_needs_user_wrong_type_is_none() {
        // needsUser: 42 is a wrong type entirely -> None instead of a hard error.
        let json = r#"{
            "agentId": "c-1",
            "role": "coder",
            "model": "sonnet",
            "status": "online",
            "message": null,
            "currentProjectId": null,
            "currentTaskId": null,
            "firstSeenAt": null,
            "lastSeenAt": null,
            "needsUser": 42
        }"#;
        let session: AgentSession = serde_json::from_str(json).expect("session loads");
        assert_eq!(session.needs_user, None);
    }

    #[test]
    fn lenient_fields_do_not_brick_whole_state_read() {
        // One bad subagent entry among valid sessions must not fail the whole
        // AgentLiveState read (the previous hard-fail bricked get_agent_live_state).
        let json = r#"{
            "version": 2,
            "updatedAt": "2026-06-04T10:00:00+00:00",
            "sessions": [
                {
                    "agentId": "orch-1",
                    "role": "orchestrator",
                    "model": "opus",
                    "status": "online",
                    "message": null,
                    "currentProjectId": null,
                    "currentTaskId": null,
                    "firstSeenAt": null,
                    "lastSeenAt": null,
                    "subagents": [
                        {"label": "good", "model": "sonnet", "count": 1, "role": "coder"},
                        {"label": "bad", "model": "sonnet", "count": -1, "role": "coder"}
                    ],
                    "needsUser": {}
                }
            ]
        }"#;
        let state: AgentLiveState = serde_json::from_str(json).expect("state loads");
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.sessions[0].subagents.len(), 1);
        assert_eq!(state.sessions[0].needs_user, None);
    }

    #[test]
    fn oracle_index_preferences_serializes_camel_case() {
        let preferences = OracleIndexPreferences {
            auto_watch_on_unlock: true,
            index_root: Some("C:\\Users\\gualt\\Desktop\\aspis bio".into()),
            index_mode: None,
        };

        let raw = serde_json::to_string(&preferences).unwrap();

        assert!(raw.contains("autoWatchOnUnlock"));
        assert!(raw.contains("indexRoot"));
        assert!(!raw.contains("auto_watch_on_unlock"));
        assert!(!raw.contains("index_root"));
    }

    // -- mini-coder additions (parentAgentId + miniCoderDirectives) ---------

    #[test]
    fn session_without_parent_agent_id_loads_and_is_none() {
        // Every ordinary (non-mini) session omits parentAgentId. It must load and
        // default to None, and re-serialize WITHOUT the key (no churn into a file
        // Python owns).
        let json = r#"{
            "agentId": "coder-1",
            "role": "coder",
            "model": "sonnet",
            "status": "online"
        }"#;
        let session: AgentSession = serde_json::from_str(json).expect("session loads");
        assert_eq!(session.parent_agent_id, None);
        let back = serde_json::to_string(&session).unwrap();
        assert!(!back.contains("parentAgentId"), "no-churn violated: {back}");
    }

    #[test]
    fn session_with_parent_agent_id_round_trips_camel_case() {
        let json = r#"{
            "agentId": "mini-coder1-abcd1234",
            "role": "coder",
            "model": "haiku",
            "status": "online",
            "parentAgentId": "coder-1"
        }"#;
        let session: AgentSession = serde_json::from_str(json).expect("session loads");
        assert_eq!(session.parent_agent_id.as_deref(), Some("coder-1"));
        let back = serde_json::to_string(&session).unwrap();
        assert!(
            back.contains("\"parentAgentId\":\"coder-1\""),
            "json: {back}"
        );
        assert!(!back.contains("parent_agent_id"), "snake leaked: {back}");
    }

    #[test]
    fn live_state_without_mini_coder_directives_loads_and_no_churn() {
        // An existing .aspis-agents.json with NO mini-coder activity must load
        // (empty queue) and must NOT get an injected miniCoderDirectives key on
        // re-serialize.
        let json = r#"{
            "version": 2,
            "updatedAt": "2026-06-06T10:00:00+00:00",
            "sessions": []
        }"#;
        let state: AgentLiveState = serde_json::from_str(json).expect("state loads");
        assert!(state.mini_coder_directives.is_empty());
        let back = serde_json::to_string(&state).unwrap();
        assert!(
            !back.contains("miniCoderDirectives"),
            "no-churn violated: {back}"
        );
    }

    #[test]
    fn live_state_mini_coder_directives_round_trip_camel_case() {
        let json = r#"{
            "version": 2,
            "updatedAt": "2026-06-06T10:00:00+00:00",
            "sessions": [],
            "miniCoderDirectives": [
                {
                    "id": "d1",
                    "parentAgentId": "coder-1",
                    "status": "pending",
                    "task": "docstring foo()",
                    "files": ["src/a.rs"],
                    "resultPath": "mini/d1.json",
                    "createdAt": "2026-06-06T10:00:00Z"
                }
            ]
        }"#;
        let state: AgentLiveState = serde_json::from_str(json).expect("state loads");
        assert_eq!(state.mini_coder_directives.len(), 1);
        assert_eq!(state.mini_coder_directives[0].id, "d1");
        assert_eq!(
            state.mini_coder_directives[0].status,
            crate::backend::mini_coder::MiniCoderStatus::Pending
        );
        let back = serde_json::to_string(&state).unwrap();
        assert!(back.contains("\"miniCoderDirectives\""), "json: {back}");
        assert!(
            back.contains("\"parentAgentId\":\"coder-1\""),
            "json: {back}"
        );
    }

    #[test]
    fn one_malformed_directive_does_not_brick_live_state() {
        // Mirror the lenient-subagents guard for the new array: a half-written /
        // wrong-typed directive among valid ones must be DROPPED, not fail the
        // whole AgentLiveState deserialize (which would freeze get_agent_live_state).
        let json = r#"{
            "version": 2,
            "updatedAt": "2026-06-06T10:00:00+00:00",
            "sessions": [],
            "miniCoderDirectives": [
                {
                    "id": "good",
                    "parentAgentId": "coder-1",
                    "status": "pending",
                    "task": "ok",
                    "resultPath": "mini/good.json",
                    "createdAt": "2026-06-06T10:00:00Z"
                },
                { "id": 42, "task": ["wrong", "type"], "status": "pending" },
                "totally-not-an-object"
            ]
        }"#;
        let state: AgentLiveState = serde_json::from_str(json).expect("state loads");
        assert_eq!(state.mini_coder_directives.len(), 1);
        assert_eq!(state.mini_coder_directives[0].id, "good");
    }

    #[test]
    fn live_state_visual_check_directives_round_trip_camel_case_and_lenient() {
        let json = r#"{
            "version": 2,
            "updatedAt": "2026-06-06T10:00:00+00:00",
            "sessions": [],
            "visualCheckDirectives": [
                {
                    "id": "v1",
                    "parentAgentId": "coder-1",
                    "status": "pending",
                    "htmlPath": "dist/page.html",
                    "focus": "header",
                    "resultPath": "v1.json",
                    "createdAt": "2026-06-06T10:00:00Z"
                },
                { "id": 42, "htmlPath": ["wrong"], "status": "pending" },
                "totally-not-an-object"
            ]
        }"#;
        let state: AgentLiveState = serde_json::from_str(json).expect("state loads");
        assert_eq!(state.visual_check_directives.len(), 1);
        assert_eq!(state.visual_check_directives[0].html_path, "dist/page.html");
        let back = serde_json::to_string(&state).unwrap();
        assert!(back.contains("\"visualCheckDirectives\""), "json: {back}");
        assert!(back.contains("\"parentAgentId\":\"coder-1\""), "json: {back}");
        assert!(!back.contains("html_path"), "snake leaked: {back}");
    }

    #[test]
    fn live_state_without_visual_check_directives_loads_and_no_churn() {
        let json = r#"{
            "version": 2,
            "updatedAt": "2026-06-06T10:00:00+00:00",
            "sessions": []
        }"#;
        let state: AgentLiveState = serde_json::from_str(json).expect("state loads");
        assert!(state.visual_check_directives.is_empty());
        let back = serde_json::to_string(&state).unwrap();
        assert!(
            !back.contains("visualCheckDirectives"),
            "no-churn violated: {back}"
        );
    }

    #[test]
    fn mini_coder_directives_non_array_becomes_empty() {
        let json = r#"{
            "version": 2,
            "updatedAt": "2026-06-06T10:00:00+00:00",
            "sessions": [],
            "miniCoderDirectives": "garbage"
        }"#;
        let state: AgentLiveState = serde_json::from_str(json).expect("state loads");
        assert!(state.mini_coder_directives.is_empty());
    }

    // -- Phase 1: plan-approval requests + reply-box (co-ownership) ----------

    #[test]
    fn live_state_without_plan_approval_requests_loads_and_no_churn() {
        // Backward compat: an existing .aspis-agents.json with NO plan activity must
        // load (empty queue) and must NOT get an injected planApprovalRequests key on
        // re-serialize (no churn into a file Python owns).
        let json = r#"{
            "version": 2,
            "updatedAt": "2026-06-09T10:00:00+00:00",
            "sessions": []
        }"#;
        let state: AgentLiveState = serde_json::from_str(json).expect("state loads");
        assert!(state.plan_approval_requests.is_empty());
        let back = serde_json::to_string(&state).unwrap();
        assert!(
            !back.contains("planApprovalRequests"),
            "no-churn violated: {back}"
        );
    }

    #[test]
    fn live_state_plan_approval_requests_round_trip_camel_case() {
        let json = r#"{
            "version": 2,
            "updatedAt": "2026-06-09T10:00:00+00:00",
            "sessions": [],
            "planApprovalRequests": [
                {
                    "id": "0123456789abcdef0123456789abcdef",
                    "agentId": "coder-1",
                    "projectId": "proj-1",
                    "title": "Refactor parser",
                    "status": "pending_approval",
                    "createdAt": "2026-06-09T10:00:00Z"
                }
            ]
        }"#;
        let state: AgentLiveState = serde_json::from_str(json).expect("state loads");
        assert_eq!(state.plan_approval_requests.len(), 1);
        assert_eq!(
            state.plan_approval_requests[0].id,
            "0123456789abcdef0123456789abcdef"
        );
        assert_eq!(
            state.plan_approval_requests[0].status,
            crate::backend::plan_approval::PlanApprovalStatus::PendingApproval
        );
        let back = serde_json::to_string(&state).unwrap();
        assert!(back.contains("\"planApprovalRequests\""), "json: {back}");
        assert!(back.contains("\"projectId\":\"proj-1\""), "json: {back}");
    }

    #[test]
    fn one_malformed_plan_request_does_not_brick_live_state() {
        // A half-written / unknown-status entry among valid ones must be DROPPED, not
        // fail the whole AgentLiveState deserialize.
        let json = r#"{
            "version": 2,
            "updatedAt": "2026-06-09T10:00:00+00:00",
            "sessions": [],
            "planApprovalRequests": [
                {
                    "id": "0123456789abcdef0123456789abcdef",
                    "agentId": "coder-1",
                    "projectId": "proj-1",
                    "title": "good",
                    "status": "pending_approval",
                    "createdAt": "2026-06-09T10:00:00Z"
                },
                { "id": 42, "status": "from-the-future-unknown" },
                "totally-not-an-object"
            ]
        }"#;
        let state: AgentLiveState = serde_json::from_str(json).expect("state loads");
        assert_eq!(state.plan_approval_requests.len(), 1);
        assert_eq!(state.plan_approval_requests[0].title, "good");
    }

    #[test]
    fn plan_approval_requests_non_array_becomes_empty() {
        let json = r#"{
            "version": 2,
            "updatedAt": "2026-06-09T10:00:00+00:00",
            "sessions": [],
            "planApprovalRequests": "garbage"
        }"#;
        let state: AgentLiveState = serde_json::from_str(json).expect("state loads");
        assert!(state.plan_approval_requests.is_empty());
    }

    // -- Slice 5b: Claude consent requests (file-bridge co-ownership) --------

    #[test]
    fn live_state_without_consent_requests_loads_and_no_churn() {
        // CRITICAL NO-CHURN: an existing .aspis-agents.json with NO consent activity
        // must load (empty queue) AND re-serialize byte-identically (no injected
        // consentRequests key) — the file is co-owned by the Python MCP.
        let json = r#"{
            "version": 2,
            "updatedAt": "2026-06-26T10:00:00+00:00",
            "sessions": []
        }"#;
        let state: AgentLiveState = serde_json::from_str(json).expect("state loads");
        assert!(state.consent_requests.is_empty());
        let back = serde_json::to_string(&state).unwrap();
        assert!(
            !back.contains("consentRequests"),
            "no-churn violated: {back}"
        );
    }

    #[test]
    fn live_state_consent_requests_round_trip_camel_case() {
        let json = r#"{
            "version": 2,
            "updatedAt": "2026-06-26T10:00:00+00:00",
            "sessions": [],
            "consentRequests": [
                {
                    "id": "0123456789abcdef0123456789abcdef",
                    "agentId": "claude-1",
                    "projectId": "proj-1",
                    "kind": "exec",
                    "detail": "cargo build",
                    "status": "pending_approval",
                    "createdAt": "2026-06-26T10:00:00Z"
                }
            ]
        }"#;
        let state: AgentLiveState = serde_json::from_str(json).expect("state loads");
        assert_eq!(state.consent_requests.len(), 1);
        assert_eq!(
            state.consent_requests[0].id,
            "0123456789abcdef0123456789abcdef"
        );
        assert_eq!(
            state.consent_requests[0].status,
            crate::backend::consent_bridge::ConsentBridgeStatus::PendingApproval
        );
        let back = serde_json::to_string(&state).unwrap();
        assert!(back.contains("\"consentRequests\""), "json: {back}");
        assert!(back.contains("\"projectId\":\"proj-1\""), "json: {back}");
    }

    #[test]
    fn one_malformed_consent_request_does_not_brick_live_state() {
        // A half-written / unknown-status entry among valid ones must be DROPPED, not
        // fail the whole AgentLiveState deserialize.
        let json = r#"{
            "version": 2,
            "updatedAt": "2026-06-26T10:00:00+00:00",
            "sessions": [],
            "consentRequests": [
                {
                    "id": "0123456789abcdef0123456789abcdef",
                    "agentId": "claude-1",
                    "projectId": "proj-1",
                    "kind": "patch",
                    "detail": "edit a.rs",
                    "status": "pending_approval",
                    "createdAt": "2026-06-26T10:00:00Z"
                },
                { "id": 42, "kind": "not-a-kind" },
                "totally-not-an-object"
            ]
        }"#;
        let state: AgentLiveState = serde_json::from_str(json).expect("state loads");
        assert_eq!(state.consent_requests.len(), 1);
        assert_eq!(state.consent_requests[0].detail, "edit a.rs");
    }

    #[test]
    fn consent_requests_non_array_becomes_empty() {
        let json = r#"{
            "version": 2,
            "updatedAt": "2026-06-26T10:00:00+00:00",
            "sessions": [],
            "consentRequests": "garbage"
        }"#;
        let state: AgentLiveState = serde_json::from_str(json).expect("state loads");
        assert!(state.consent_requests.is_empty());
    }

    #[test]
    fn session_pending_question_and_user_reply_passthrough_round_trip() {
        // Python writes pendingQuestion; Rust writes userReply. A session carrying both
        // must round-trip with camelCase keys, untouched (passthrough co-ownership).
        let json = r#"{
            "agentId": "coder-1",
            "role": "coder",
            "model": "sonnet",
            "status": "needs_user",
            "pendingQuestion": {
                "id": "q-1",
                "question": "Which schema?",
                "createdAt": "2026-06-09T09:59:00Z"
            },
            "userReply": {
                "questionId": "q-1",
                "text": "Use v2",
                "createdAt": "2026-06-09T10:00:00Z"
            }
        }"#;
        let session: AgentSession = serde_json::from_str(json).expect("session loads");
        let pq = session.pending_question.as_ref().expect("pendingQuestion kept");
        assert_eq!(pq.id, "q-1");
        assert_eq!(pq.question, "Which schema?");
        let ur = session.user_reply.as_ref().expect("userReply kept");
        assert_eq!(ur.question_id, "q-1");
        assert_eq!(ur.text, "Use v2");
        let back = serde_json::to_string(&session).unwrap();
        assert!(back.contains("\"pendingQuestion\""), "json: {back}");
        assert!(back.contains("\"questionId\":\"q-1\""), "json: {back}");
        assert!(!back.contains("question_id"), "snake leaked: {back}");
    }

    #[test]
    fn session_without_reply_box_fields_loads_and_no_churn() {
        let json = r#"{
            "agentId": "coder-1",
            "role": "coder",
            "model": "sonnet",
            "status": "online"
        }"#;
        let session: AgentSession = serde_json::from_str(json).expect("session loads");
        assert_eq!(session.pending_question, None);
        assert_eq!(session.user_reply, None);
        let back = serde_json::to_string(&session).unwrap();
        assert!(!back.contains("pendingQuestion"), "no-churn: {back}");
        assert!(!back.contains("userReply"), "no-churn: {back}");
    }

    // ── OracleIndexPreferences ────────────────────────────────────────────────

    /// Backward compat: a blob WITHOUT `indexMode` (old format) must still
    /// deserialize cleanly and produce `index_mode = None`.
    #[test]
    fn oracle_index_preferences_round_trips_without_index_mode() {
        let json = r#"{"autoWatchOnUnlock":true,"indexRoot":null}"#;
        let prefs: OracleIndexPreferences = serde_json::from_str(json).unwrap();
        assert!(prefs.auto_watch_on_unlock);
        assert_eq!(prefs.index_root, None);
        assert_eq!(prefs.index_mode, None);
        // Serializing back must NOT emit the key (skip_serializing_if).
        let back = serde_json::to_string(&prefs).unwrap();
        assert!(
            !back.contains("indexMode"),
            "absent index_mode must not be emitted: {back}"
        );
    }

    /// Full round-trip WITH `indexMode` present.
    #[test]
    fn oracle_index_preferences_round_trips_with_index_mode_commit() {
        let json = r#"{"autoWatchOnUnlock":false,"indexRoot":null,"indexMode":"commit"}"#;
        let prefs: OracleIndexPreferences = serde_json::from_str(json).unwrap();
        assert!(!prefs.auto_watch_on_unlock);
        assert_eq!(prefs.index_mode.as_deref(), Some("commit"));
        let back = serde_json::to_string(&prefs).unwrap();
        assert!(back.contains(r#""indexMode":"commit""#), "key must survive: {back}");
    }

    #[test]
    fn oracle_index_preferences_round_trips_with_index_mode_watch() {
        let json = r#"{"autoWatchOnUnlock":true,"indexRoot":null,"indexMode":"watch"}"#;
        let prefs: OracleIndexPreferences = serde_json::from_str(json).unwrap();
        assert_eq!(prefs.index_mode.as_deref(), Some("watch"));
        let back = serde_json::to_string(&prefs).unwrap();
        assert!(back.contains(r#""indexMode":"watch""#), "key must survive: {back}");
    }
}
