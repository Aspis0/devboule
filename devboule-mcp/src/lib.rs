//! Devboule app-tools MCP (Rust).
//!
//! Port of `oracle/server/aspis_mcp.py` by phase — see `docs/devboule-mcp-port-plan.md`.
//! Branding: server process is **devboule-mcp** (not aspis_mcp).

pub mod project_file;
pub mod state;
pub mod tools;

use anyhow::{Context as _, Result};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, serde::Deserialize,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde_json::{json, Value};
use state::resolve_projects_dir;
use tools::{agent_lifecycle, cloud, human_gates, mini_coder, project};

/// SSoT role rules (same file as the Tauri app and legacy Python MCP).
const ROLE_RULES_JSON: &str = include_str!("../../oracle/server/role_rules.json");

/// Server version string advertised to clients.
pub const DEVBOULE_MCP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Tools actually registered on this server's tool router.
/// `agent_rules` must NEVER advertise tools that are not callable.
const IMPLEMENTED_TOOLS: &[&str] = &[
    "agent_rules",
    "agent_register",
    "agent_heartbeat",
    "agent_state",
    "project_list",
    "project_get",
    "project_next_task",
    "project_claim_task",
    "project_update_status",
    "project_append_note",
    "project_set_title",
    "project_create_followup",
    "project_create_plan_tasks",
    "plan_submit",
    "plan_status",
    "request_git_push",
    "ask_user",
    "spawn_mini_coder",
    "steer_mini_coder",
    "mini_coder_result",
    "spawn_main_coder",
    "provider_credentials_status",
    "cloudflare_list_workers",
    "scaleway_list_resources",
    "cloudflare_rotate_worker_secret",
    "scaleway_resource_action",
];

#[derive(Clone)]
pub struct DevbouleMcp {
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EmptyArgs {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AgentRegisterArgs {
    pub agent_id: String,
    pub role: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub client: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub launch_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AgentHeartbeatArgs {
    pub agent_id: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub session_token: Option<String>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub current_file_path: Option<String>,
    /// Optional subagent breakdown. Absent/null = leave stored; `[]` clears.
    #[serde(default)]
    pub subagents: Option<Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AgentStateArgs {
    pub agent_id: String,
    pub role: String,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProjectListArgs {
    pub agent_id: String,
    pub role: String,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProjectIdArgs {
    pub project_id: String,
    pub agent_id: String,
    pub role: String,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProjectClaimArgs {
    pub project_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub role: String,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProjectUpdateStatusArgs {
    pub project_id: String,
    pub task_id: String,
    pub status: String,
    pub agent_id: String,
    pub role: String,
    #[serde(default)]
    pub evidence: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProjectAppendNoteArgs {
    pub project_id: String,
    pub text: String,
    pub agent_id: String,
    pub role: String,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProjectSetTitleArgs {
    pub project_id: String,
    pub title: String,
    pub agent_id: String,
    pub role: String,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProjectCreateFollowupArgs {
    pub project_id: String,
    pub title: String,
    pub reason: String,
    pub agent_id: String,
    pub role: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PlanSubmitArgs {
    pub agent_id: String,
    pub role: String,
    pub project_id: String,
    pub title: String,
    pub plan_markdown: String,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PlanStatusArgs {
    pub agent_id: String,
    pub role: String,
    pub plan_id: String,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RequestGitPushArgs {
    pub agent_id: String,
    pub role: String,
    pub project_id: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub force: Option<bool>,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AskUserArgs {
    pub agent_id: String,
    pub role: String,
    pub question: String,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProjectCreatePlanTasksArgs {
    pub project_id: String,
    pub plan_id: String,
    /// Each task: {id, title, scope?, acceptance?, dependsOn?, weight?}
    pub tasks: Vec<Value>,
    pub agent_id: String,
    pub role: String,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SpawnMiniCoderArgs {
    pub agent_id: String,
    pub role: String,
    pub task: String,
    pub files: Vec<String>,
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub allow_oracle: Option<bool>,
    #[serde(default)]
    pub write: Option<bool>,
    #[serde(default)]
    pub wait: Option<bool>,
    #[serde(default)]
    pub write_mode: Option<String>,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SpawnMainCoderArgs {
    pub agent_id: String,
    pub role: String,
    pub task: String,
    pub files: Vec<String>,
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub allow_oracle: Option<bool>,
    #[serde(default)]
    pub wait: Option<bool>,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SteerMiniCoderArgs {
    pub agent_id: String,
    pub role: String,
    pub directive_id: String,
    pub message: String,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MiniCoderResultArgs {
    pub agent_id: String,
    pub role: String,
    pub directive_id: String,
    #[serde(default)]
    pub wait: Option<bool>,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProviderCredentialsStatusArgs {
    pub agent_id: String,
    pub role: String,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CloudflareListWorkersArgs {
    pub agent_id: String,
    pub role: String,
    #[serde(default)]
    pub session_token: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CloudflareRotateWorkerSecretArgs {
    pub agent_id: String,
    pub role: String,
    pub worker_name: String,
    pub secret_name: String,
    pub secret_value: String,
    #[serde(default)]
    pub session_token: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub management_project_id: Option<String>,
    #[serde(default)]
    pub aspis_project_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub evidence: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ScalewayListResourcesArgs {
    pub agent_id: String,
    pub role: String,
    #[serde(default)]
    pub session_token: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ScalewayResourceActionArgs {
    pub agent_id: String,
    pub role: String,
    pub resource_id: String,
    pub action: String,
    #[serde(default)]
    pub session_token: Option<String>,
    #[serde(default)]
    pub confirm_resource_name: Option<String>,
    /// Scaleway cloud project id pin (not the management Kanban project).
    #[serde(default)]
    pub scaleway_project_id: Option<String>,
    #[serde(default)]
    pub provider_project_id: Option<String>,
    /// Ambiguous: preferred as SCW pin when scaleway_project_id unset; management
    /// Kanban id is `management_project_id`.
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub management_project_id: Option<String>,
    #[serde(default)]
    pub aspis_project_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub evidence: Option<String>,
}

#[tool_router]
impl DevbouleMcp {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    /// Practical roles for Devboule agents (P5: + cloud list/mutate).
    ///
    /// Each role keeps only `role`, filtered `allowedTools`, and a short summary.
    #[tool(
        description = "Return Devboule agent role rules for this Rust MCP (P5: lifecycle + project + human gates + mini/main coder + cloud). Roles are slim: role, allowedTools, summary. Use DEVBOULE_MCP_BACKEND=python for oracle/CKG/censor/design tools and full role prose."
    )]
    pub async fn agent_rules(
        &self,
        Parameters(_args): Parameters<EmptyArgs>,
    ) -> Result<CallToolResult, McpError> {
        let rules = load_role_rules_for_client().map_err(internal)?;
        let body = json!({
            "server": "devboule-mcp",
            "version": DEVBOULE_MCP_VERSION,
            "backend": "rust",
            "roles": rules,
            "implementedTools": IMPLEMENTED_TOOLS,
            "portPhase": "P5",
            "note": "Partial Rust MCP (P5). Lifecycle + project/Kanban + human gates + mini/main coder + cloud (CF/SCW list + coder-only mutate with claim/evidence/pin/confirm). Oracle/CKG/censor/design still require DEVBOULE_MCP_BACKEND=python until cutover. Provider tokens from env only (app injects). MCP never self-approves plans/pushes or logs secrets.",
        });
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(description = "Registers a CLI agent before reading or updating projects.")]
    pub async fn agent_register(
        &self,
        Parameters(args): Parameters<AgentRegisterArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = agent_lifecycle::agent_register(
            &projects,
            &args.agent_id,
            &args.role,
            args.model.as_deref(),
            args.client.as_deref(),
            args.message.as_deref(),
            args.launch_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(description = "Updates the agent's live presence in the dashboard.")]
    pub async fn agent_heartbeat(
        &self,
        Parameters(args): Parameters<AgentHeartbeatArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let file_path = args
            .file_path
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(args.current_file_path.as_deref().filter(|s| !s.is_empty()));
        let body = agent_lifecycle::agent_heartbeat(
            &projects,
            &args.agent_id,
            args.status.as_deref(),
            args.message.as_deref(),
            args.session_token.as_deref(),
            file_path,
            args.subagents,
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(
        description = "Reads the live state of agent sessions, claims, and latest events after registration."
    )]
    pub async fn agent_state(
        &self,
        Parameters(args): Parameters<AgentStateArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = agent_lifecycle::agent_state(
            &projects,
            &args.agent_id,
            &args.role,
            args.session_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(description = "List local Markdown projects.")]
    pub async fn project_list(
        &self,
        Parameters(args): Parameters<ProjectListArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = project::project_list(
            &projects,
            &args.agent_id,
            &args.role,
            args.session_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(description = "Read a project with its tasks, notes, revision and path.")]
    pub async fn project_get(
        &self,
        Parameters(args): Parameters<ProjectIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = project::project_get(
            &projects,
            &args.project_id,
            &args.agent_id,
            &args.role,
            args.session_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(description = "Suggest the next incomplete task for a role.")]
    pub async fn project_next_task(
        &self,
        Parameters(args): Parameters<ProjectIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = project::project_next_task(
            &projects,
            &args.project_id,
            &args.agent_id,
            &args.role,
            args.session_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(description = "Create a lease claim on the task.")]
    pub async fn project_claim_task(
        &self,
        Parameters(args): Parameters<ProjectClaimArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = project::project_claim_task(
            &projects,
            &args.project_id,
            &args.task_id,
            &args.agent_id,
            &args.role,
            args.session_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(description = "Update task/project status with notes and an auditable event.")]
    pub async fn project_update_status(
        &self,
        Parameters(args): Parameters<ProjectUpdateStatusArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = project::project_update_status(
            &projects,
            &args.project_id,
            &args.task_id,
            &args.status,
            &args.agent_id,
            &args.role,
            args.evidence.as_deref(),
            args.confidence,
            args.session_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(description = "Append a structured note to the project.")]
    pub async fn project_append_note(
        &self,
        Parameters(args): Parameters<ProjectAppendNoteArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = project::project_append_note(
            &projects,
            &args.project_id,
            &args.text,
            &args.agent_id,
            &args.role,
            args.session_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(description = "Rename a project title (durable frontmatter write).")]
    pub async fn project_set_title(
        &self,
        Parameters(args): Parameters<ProjectSetTitleArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = project::project_set_title(
            &projects,
            &args.project_id,
            &args.title,
            &args.agent_id,
            &args.role,
            args.session_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(
        description = "Create a follow-up TODO task. category: feature|hardening|bug|other (default other)."
    )]
    pub async fn project_create_followup(
        &self,
        Parameters(args): Parameters<ProjectCreateFollowupArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = project::project_create_followup(
            &projects,
            &args.project_id,
            &args.title,
            &args.reason,
            &args.agent_id,
            &args.role,
            args.category.as_deref(),
            args.description.as_deref(),
            args.session_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(
        description = "Coder-only: SUBMIT an implementation plan for human approval and BLOCK on the verdict (approved/rejected/timeout). Does not self-approve."
    )]
    pub async fn plan_submit(
        &self,
        Parameters(args): Parameters<PlanSubmitArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = human_gates::plan_submit(
            &projects,
            &args.agent_id,
            &args.role,
            &args.project_id,
            &args.title,
            &args.plan_markdown,
            args.session_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(
        description = "Coder or verifier: read the current status of a previously submitted plan (pending_approval/approved/rejected/timeout/not_found)."
    )]
    pub async fn plan_status(
        &self,
        Parameters(args): Parameters<PlanStatusArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = human_gates::plan_status(
            &projects,
            &args.agent_id,
            &args.role,
            &args.plan_id,
            args.session_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(
        description = "Coder-only: REQUEST human approval to git push (you may COMMIT freely, but every PUSH is approved by the human). BLOCKS until pushed/push_failed/denied/timeout."
    )]
    pub async fn request_git_push(
        &self,
        Parameters(args): Parameters<RequestGitPushArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = human_gates::request_git_push(
            &projects,
            &args.agent_id,
            &args.role,
            &args.project_id,
            args.branch.as_deref(),
            args.remote.as_deref(),
            args.force.unwrap_or(false),
            args.session_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(
        description = "Coder or verifier: ask the HUMAN a blocking question and wait for the reply (or timeout)."
    )]
    pub async fn ask_user(
        &self,
        Parameters(args): Parameters<AskUserArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = human_gates::ask_user(
            &projects,
            &args.agent_id,
            &args.role,
            &args.question,
            args.session_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(
        description = "Bulk-create an approved plan's tasks on the project Kanban as todo, tagged with planId. Requires plan status=approved (human gate)."
    )]
    pub async fn project_create_plan_tasks(
        &self,
        Parameters(args): Parameters<ProjectCreatePlanTasksArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = human_gates::project_create_plan_tasks(
            &projects,
            &args.project_id,
            &args.plan_id,
            &args.tasks,
            &args.agent_id,
            &args.role,
            args.session_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(
        description = "Coder/orchestrator: delegate a sub-task to a one-shot mini-coder hosted by the app (writes a pending directive the Tauri mini_coder_executor claims). Default wait=true blocks until terminal result (poll capped at 1800s, runs on a blocking pool); wait=false returns directiveId for steer_mini_coder / mini_coder_result. Fail-closed if the app executor is offline."
    )]
    pub async fn spawn_mini_coder(
        &self,
        Parameters(args): Parameters<SpawnMiniCoderArgs>,
    ) -> Result<CallToolResult, McpError> {
        // wait=true polls with thread::sleep — offload so the async runtime stays free.
        let body = tokio::task::spawn_blocking(move || {
            let projects = resolve_projects_dir();
            mini_coder::spawn_mini_coder(
                &projects,
                &args.agent_id,
                &args.role,
                &args.task,
                &args.files,
                args.backend.as_deref(),
                args.allow_oracle.unwrap_or(false),
                args.write,
                args.write_mode.as_deref(),
                args.wait,
                args.session_token.as_deref(),
            )
        })
        .await
        .map_err(|e| internal(format!("spawn_mini_coder join: {e}")))?
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(
        description = "Orchestrator only: dispatch a substantial task to the local MAIN coder (tier=main, always agentic write). Same supervision as spawn_mini_coder (wait=false + steer_mini_coder + mini_coder_result). wait=true poll is capped at 1800s and runs on a blocking pool."
    )]
    pub async fn spawn_main_coder(
        &self,
        Parameters(args): Parameters<SpawnMainCoderArgs>,
    ) -> Result<CallToolResult, McpError> {
        let body = tokio::task::spawn_blocking(move || {
            let projects = resolve_projects_dir();
            mini_coder::spawn_main_coder(
                &projects,
                &args.agent_id,
                &args.role,
                &args.task,
                &args.files,
                args.backend.as_deref(),
                args.allow_oracle.unwrap_or(false),
                args.wait,
                args.session_token.as_deref(),
            )
        })
        .await
        .map_err(|e| internal(format!("spawn_main_coder join: {e}")))?
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(
        description = "Coder/orchestrator: steer a RUNNING mini you spawned (append mid-flight correction, or message 'stop' to abort via kill path). Pass directiveId from spawn_mini_coder / spawn_main_coder."
    )]
    pub async fn steer_mini_coder(
        &self,
        Parameters(args): Parameters<SteerMiniCoderArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = mini_coder::steer_mini_coder(
            &projects,
            &args.agent_id,
            &args.role,
            &args.directive_id,
            &args.message,
            args.session_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(
        description = "Coder/orchestrator: collect the outcome of a mini delegated with spawn_mini_coder(wait=false). wait=true (default) blocks until terminal (poll capped at 1800s, runs on a blocking pool); wait=false is a single non-blocking read returning the real directive status."
    )]
    pub async fn mini_coder_result(
        &self,
        Parameters(args): Parameters<MiniCoderResultArgs>,
    ) -> Result<CallToolResult, McpError> {
        let body = tokio::task::spawn_blocking(move || {
            let projects = resolve_projects_dir();
            mini_coder::mini_coder_result(
                &projects,
                &args.agent_id,
                &args.role,
                &args.directive_id,
                args.wait,
                args.session_token.as_deref(),
            )
        })
        .await
        .map_err(|e| internal(format!("mini_coder_result join: {e}")))?
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(
        description = "Read provider credential readiness (Cloudflare/Scaleway/GitHub/Oracle LLM) without ever returning secret values. Sources are env-injected tokens (and vault targets for docs)."
    )]
    pub async fn provider_credentials_status(
        &self,
        Parameters(args): Parameters<ProviderCredentialsStatusArgs>,
    ) -> Result<CallToolResult, McpError> {
        let projects = resolve_projects_dir();
        let body = cloud::provider_credentials_status(
            &projects,
            &args.agent_id,
            &args.role,
            args.session_token.as_deref(),
        )
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(
        description = "List Cloudflare Workers in the pinned Aspis Bio / Devboule account scope (sibling workers are hidden). Tokens from env only."
    )]
    pub async fn cloudflare_list_workers(
        &self,
        Parameters(args): Parameters<CloudflareListWorkersArgs>,
    ) -> Result<CallToolResult, McpError> {
        let body = tokio::task::spawn_blocking(move || {
            let projects = resolve_projects_dir();
            cloud::cloudflare_list_workers(
                &projects,
                &args.agent_id,
                &args.role,
                args.session_token.as_deref(),
                args.account_id.as_deref(),
            )
        })
        .await
        .map_err(|e| internal(format!("cloudflare_list_workers join: {e}")))?
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(
        description = "Coder-only: rotate a Cloudflare Worker secret binding. Requires session token, active claim, live wip/blocked task with non-coder approvedBy, and evidence. Worker must be in Aspis Bio inventory/scope. Never logs the secret value."
    )]
    pub async fn cloudflare_rotate_worker_secret(
        &self,
        Parameters(args): Parameters<CloudflareRotateWorkerSecretArgs>,
    ) -> Result<CallToolResult, McpError> {
        let body = tokio::task::spawn_blocking(move || {
            let projects = resolve_projects_dir();
            cloud::cloudflare_rotate_worker_secret(
                &projects,
                &args.agent_id,
                &args.role,
                args.session_token.as_deref(),
                &args.worker_name,
                &args.secret_name,
                &args.secret_value,
                args.account_id.as_deref(),
                args.management_project_id.as_deref(),
                args.aspis_project_id.as_deref(),
                args.task_id.as_deref(),
                args.evidence.as_deref(),
            )
        })
        .await
        .map_err(|e| internal(format!("cloudflare_rotate_worker_secret join: {e}")))?
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(
        description = "List Scaleway resources in the pinned aspis-bio Devboule project (instances, serverless, block, file, SQL). Tokens from env only."
    )]
    pub async fn scaleway_list_resources(
        &self,
        Parameters(args): Parameters<ScalewayListResourcesArgs>,
    ) -> Result<CallToolResult, McpError> {
        let body = tokio::task::spawn_blocking(move || {
            let projects = resolve_projects_dir();
            cloud::scaleway_list_resources(
                &projects,
                &args.agent_id,
                &args.role,
                args.session_token.as_deref(),
                args.project_id.as_deref(),
            )
        })
        .await
        .map_err(|e| internal(format!("scaleway_list_resources join: {e}")))?
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }

    #[tool(
        description = "Coder-only: Scaleway resource action (start/stop/reboot/deploy/delete). Requires session token, claim, live task + approvedBy, evidence. delete requires confirm_resource_name exact match. terminate is rejected — use delete. Project pin must be aspis-bio."
    )]
    pub async fn scaleway_resource_action(
        &self,
        Parameters(args): Parameters<ScalewayResourceActionArgs>,
    ) -> Result<CallToolResult, McpError> {
        let body = tokio::task::spawn_blocking(move || {
            let projects = resolve_projects_dir();
            // SCW cloud project pin: explicit scaleway/provider field, else project_id
            // when it is not the management Kanban id.
            let scw_pin = args
                .scaleway_project_id
                .as_deref()
                .or(args.provider_project_id.as_deref())
                .or(args.project_id.as_deref());
            cloud::scaleway_resource_action(
                &projects,
                &args.agent_id,
                &args.role,
                args.session_token.as_deref(),
                &args.resource_id,
                &args.action,
                args.confirm_resource_name.as_deref(),
                scw_pin,
                args.management_project_id.as_deref(),
                args.aspis_project_id.as_deref(),
                args.task_id.as_deref(),
                args.evidence.as_deref(),
            )
        })
        .await
        .map_err(|e| internal(format!("scaleway_resource_action join: {e}")))?
        .map_err(tool_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }
}

#[tool_handler]
impl ServerHandler for DevbouleMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Devboule app-tools MCP (Rust). P5: lifecycle + project/Kanban + human gates + mini/main coder + cloud (provider_credentials_status, cloudflare_list_workers, cloudflare_rotate_worker_secret, scaleway_list_resources, scaleway_resource_action). Prefer DEVBOULE_MCP_BACKEND=python until cutover for oracle/CKG/censor/design. Cloud mutate is coder-only with claim+evidence+pin/confirm. Tokens from env only; never log secrets. Agents never self-approve plans/pushes."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "devboule-mcp".into(),
                title: Some("Devboule MCP".into()),
                version: DEVBOULE_MCP_VERSION.into(),
                icons: None,
                website_url: None,
            },
            ..Default::default()
        }
    }
}

/// Short per-role summary for the slim payload.
const P5_ROLE_SUMMARY: &str =
    "P5 Rust MCP: lifecycle + project/Kanban + human gates + mini/main coder + cloud list/mutate (role-filtered). Use DEVBOULE_MCP_BACKEND=python for oracle/CKG/censor/design.";

/// Load role rules, filter `allowedTools` to `IMPLEMENTED_TOOLS`, and strip prose.
fn load_role_rules_for_client() -> Result<Value> {
    let json = ROLE_RULES_JSON.trim_start_matches('\u{feff}');
    let v: Value = serde_json::from_str(json).context("role_rules.json parse")?;
    let mut roles = v
        .get("roles")
        .cloned()
        .context("role_rules.json missing roles[]")?;
    slim_roles_for_client(&mut roles);
    Ok(roles)
}

/// Keep only `role`, filtered `allowedTools`, and a short `summary` per role.
///
/// `agent_rules` is a meta tool always available (not listed in role_rules.json).
/// Other tools are the intersection of the role allowlist and IMPLEMENTED_TOOLS —
/// so mini does not see heartbeat/state.
fn slim_roles_for_client(roles: &mut Value) {
    let Some(arr) = roles.as_array_mut() else {
        return;
    };
    for role in arr.iter_mut() {
        let Some(obj) = role.as_object() else {
            continue;
        };
        let role_name = obj
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut tools: Vec<Value> = obj
            .get("allowedTools")
            .and_then(|v| v.as_array())
            .map(|list| {
                list.iter()
                    .filter(|t| {
                        t.as_str()
                            .map(|name| IMPLEMENTED_TOOLS.contains(&name))
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        // Meta tool always available for every role.
        if !tools.iter().any(|t| t.as_str() == Some("agent_rules")) {
            tools.insert(0, json!("agent_rules"));
        }

        *role = json!({
            "role": role_name,
            "allowedTools": tools,
            "summary": P5_ROLE_SUMMARY,
        });
    }
}

#[cfg(test)]
fn load_role_rules_raw() -> Result<Value> {
    let json = ROLE_RULES_JSON.trim_start_matches('\u{feff}');
    let v: Value = serde_json::from_str(json).context("role_rules.json parse")?;
    let roles = v
        .get("roles")
        .cloned()
        .context("role_rules.json missing roles[]")?;
    Ok(roles)
}

fn internal(err: impl std::fmt::Display) -> McpError {
    McpError::internal_error(err.to_string(), None)
}

fn tool_err(err: state::ToolError) -> McpError {
    // Client-facing tool failures (authz, missing register, bad token).
    McpError::invalid_params(err.message, None)
}

/// Run the stdio MCP server to completion.
pub async fn serve_stdio() -> Result<()> {
    let service = DevbouleMcp::new()
        .serve(stdio())
        .await
        .context("failed to start devboule-mcp stdio server")?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{hash_launch_token, seed_launch_pending};
    use crate::tools::agent_lifecycle::{agent_heartbeat, agent_register, agent_state};
    use std::fs;
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    /// Serialize env-mutating tests (unmanaged kill switch).
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn clear_unmanaged_env() {
        std::env::remove_var("DEVBOULE_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS");
        std::env::remove_var("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS");
    }

    fn set_unmanaged(on: bool) {
        if on {
            std::env::set_var("DEVBOULE_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", "1");
        } else {
            clear_unmanaged_env();
            std::env::set_var("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", "");
            std::env::set_var("DEVBOULE_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", "");
        }
    }

    fn temp_projects() -> (TempDir, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        let projects = tmp.path().join("projects");
        fs::create_dir_all(&projects).unwrap();
        (tmp, projects)
    }

    #[test]
    fn role_rules_json_loads_four_roles() {
        let roles = load_role_rules_raw().expect("parse");
        let arr = roles.as_array().expect("array");
        assert_eq!(arr.len(), 4);
        let names: Vec<&str> = arr
            .iter()
            .filter_map(|r| r.get("role").and_then(|x| x.as_str()))
            .collect();
        assert_eq!(names, vec!["coder", "orchestrator", "verifier", "mini"]);
    }

    #[test]
    fn client_roles_only_advertise_implemented_tools() {
        let roles = load_role_rules_for_client().expect("parse");
        let arr = roles.as_array().expect("array");
        for role in arr {
            let tools = role["allowedTools"].as_array().expect("allowedTools");
            for t in tools {
                let name = t.as_str().expect("tool name string");
                assert!(
                    IMPLEMENTED_TOOLS.contains(&name),
                    "role {:?} advertised unimplemented tool {name}",
                    role.get("role")
                );
            }
            assert!(
                tools.iter().any(|t| t == "agent_rules"),
                "role {:?} lost agent_rules after filter",
                role.get("role")
            );
            let obj = role.as_object().expect("role object");
            for key in obj.keys() {
                assert!(
                    matches!(key.as_str(), "role" | "allowedTools" | "summary"),
                    "role {:?} kept unexpected key {key}",
                    role.get("role")
                );
            }
            assert_eq!(role["summary"], P5_ROLE_SUMMARY);
        }
    }

    #[test]
    fn mini_does_not_get_heartbeat_or_state_in_filtered_list() {
        let roles = load_role_rules_for_client().expect("parse");
        let mini = roles
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["role"] == "mini")
            .expect("mini");
        let tools: Vec<&str> = mini["allowedTools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.as_str())
            .collect();
        assert!(tools.contains(&"agent_rules"));
        assert!(tools.contains(&"agent_register"));
        assert!(!tools.contains(&"agent_heartbeat"));
        assert!(!tools.contains(&"agent_state"));
    }

    #[test]
    fn coder_gets_lifecycle_project_human_gate_and_mini_tools() {
        let roles = load_role_rules_for_client().expect("parse");
        let coder = roles
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["role"] == "coder")
            .expect("coder");
        let tools: Vec<&str> = coder["allowedTools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.as_str())
            .collect();
        for need in [
            "agent_rules",
            "agent_register",
            "agent_heartbeat",
            "agent_state",
            "project_list",
            "project_get",
            "project_claim_task",
            "project_update_status",
            "project_set_title",
            "project_create_followup",
            "project_create_plan_tasks",
            "plan_submit",
            "plan_status",
            "request_git_push",
            "ask_user",
            "spawn_mini_coder",
            "steer_mini_coder",
            "mini_coder_result",
            "provider_credentials_status",
            "cloudflare_list_workers",
            "cloudflare_rotate_worker_secret",
            "scaleway_list_resources",
            "scaleway_resource_action",
        ] {
            assert!(tools.contains(&need), "coder missing {need}");
        }
        assert!(
            !tools.contains(&"spawn_main_coder"),
            "coder must not get spawn_main_coder"
        );
    }

    #[test]
    fn verifier_lacks_set_title_followup_and_plan_submit() {
        let roles = load_role_rules_for_client().expect("parse");
        let verifier = roles
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["role"] == "verifier")
            .expect("verifier");
        let tools: Vec<&str> = verifier["allowedTools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.as_str())
            .collect();
        assert!(tools.contains(&"project_claim_task"));
        assert!(tools.contains(&"project_update_status"));
        assert!(tools.contains(&"plan_status"));
        assert!(tools.contains(&"ask_user"));
        assert!(!tools.contains(&"project_set_title"));
        assert!(!tools.contains(&"project_create_followup"));
        assert!(!tools.contains(&"plan_submit"));
        assert!(!tools.contains(&"request_git_push"));
        assert!(!tools.contains(&"project_create_plan_tasks"));
    }

    #[test]
    fn mini_gets_no_project_tools() {
        let roles = load_role_rules_for_client().expect("parse");
        let mini = roles
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["role"] == "mini")
            .expect("mini");
        let tools: Vec<&str> = mini["allowedTools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.as_str())
            .collect();
        assert!(!tools.iter().any(|t| t.starts_with("project_")));
    }

    #[test]
    fn client_roles_payload_does_not_name_unimplemented_tools() {
        let roles = load_role_rules_for_client().expect("parse");
        let dumped = roles.to_string();
        for banned in [
            "censor_dispose",
            "oracle_context",
            "oracle_ask",
            "visual_check",
            "design_request",
            "project_structure",
            "get_neighborhood",
            "find_imports",
            "censor_findings",
        ] {
            assert!(
                !dumped.contains(banned),
                "P5 role payload must not mention unimplemented tool {banned}: {dumped}"
            );
        }
        // P5: human gates + mini coder + cloud must appear for coder.
        assert!(dumped.contains("plan_submit"));
        assert!(dumped.contains("request_git_push"));
        assert!(dumped.contains("ask_user"));
        assert!(dumped.contains("project_create_plan_tasks"));
        assert!(dumped.contains("spawn_mini_coder"));
        assert!(dumped.contains("steer_mini_coder"));
        assert!(dumped.contains("mini_coder_result"));
        assert!(dumped.contains("provider_credentials_status"));
        assert!(dumped.contains("cloudflare_list_workers"));
        assert!(dumped.contains("cloudflare_rotate_worker_secret"));
        assert!(dumped.contains("scaleway_list_resources"));
        assert!(dumped.contains("scaleway_resource_action"));
    }

    #[test]
    fn orchestrator_gets_spawn_main_coder() {
        let roles = load_role_rules_for_client().expect("parse");
        let orch = roles
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["role"] == "orchestrator")
            .expect("orchestrator");
        let tools: Vec<&str> = orch["allowedTools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.as_str())
            .collect();
        assert!(tools.contains(&"spawn_main_coder"));
        assert!(tools.contains(&"spawn_mini_coder"));
        assert!(tools.contains(&"steer_mini_coder"));
        assert!(tools.contains(&"mini_coder_result"));
    }

    #[test]
    fn raw_rules_have_more_tools_than_implemented() {
        let raw = load_role_rules_raw().expect("parse");
        let coder = raw
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["role"] == "coder")
            .expect("coder");
        let n = coder["allowedTools"].as_array().unwrap().len();
        assert!(
            n > IMPLEMENTED_TOOLS.len() - 1, // agent_rules not in role_rules
            "expected full role_rules to list more tools than P5 implements (got {n})"
        );
    }

    #[test]
    fn orchestrator_gets_cloud_read_not_mutate() {
        let roles = load_role_rules_for_client().expect("parse");
        let orch = roles
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["role"] == "orchestrator")
            .expect("orchestrator");
        let tools: Vec<&str> = orch["allowedTools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.as_str())
            .collect();
        assert!(tools.contains(&"provider_credentials_status"));
        assert!(tools.contains(&"cloudflare_list_workers"));
        assert!(tools.contains(&"scaleway_list_resources"));
        assert!(!tools.contains(&"cloudflare_rotate_worker_secret"));
        assert!(!tools.contains(&"scaleway_resource_action"));
    }

    #[test]
    fn version_is_nonempty() {
        assert!(!DEVBOULE_MCP_VERSION.is_empty());
    }

    #[test]
    fn server_info_names_devboule_mcp() {
        let info = DevbouleMcp::new().get_info();
        assert_eq!(info.server_info.name, "devboule-mcp");
        assert_eq!(info.server_info.version, DEVBOULE_MCP_VERSION);
    }

    // ── security parity (Python test_aspis_mcp) ─────────────────────────────

    #[test]
    fn privileged_register_without_token_fails_when_unmanaged_off() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let err = agent_register(
            &projects,
            "self-attested-coder",
            "coder",
            Some("codex"),
            None,
            Some("try direct privileged register"),
            None,
        )
        .unwrap_err();
        assert!(
            err.message.contains("app-issued launch token"),
            "{}",
            err.message
        );
    }

    #[test]
    fn good_launch_token_register_active_session_token_secrets_scrubbed() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let token = "test-launch-token";
        seed_launch_pending(&projects, "pending-coder", "coder", token).unwrap();

        let ack = agent_register(
            &projects,
            "pending-coder",
            "coder",
            Some("codex"),
            None,
            Some("registered through app launch"),
            Some(token),
        )
        .unwrap();

        assert_eq!(ack["session"]["status"], "active");
        assert!(ack["session"].get("launchTokenHash").is_none());
        assert!(ack["session"].get("sessionTokenHash").is_none());
        assert!(ack["session"].get("launchConsumedAt").is_none());
        let session_token = ack["sessionToken"].as_str().expect("sessionToken");
        assert!(!session_token.is_empty());

        // On-disk must retain hashes + launchConsumedAt, not raw session token.
        let raw = fs::read_to_string(projects.join(".aspis-agents.json")).unwrap();
        assert!(!raw.contains(session_token));
        assert!(raw.contains("sessionTokenHash"));
        assert!(raw.contains("launchConsumedAt"));
        assert!(!raw.contains(&format!("\"launchTokenHash\":\"{}\"", hash_launch_token(token))));
    }

    #[test]
    fn wrong_launch_token_fails() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        seed_launch_pending(&projects, "pending-verifier", "verifier", "right-token").unwrap();
        let err = agent_register(
            &projects,
            "pending-verifier",
            "verifier",
            Some("audit"),
            None,
            Some("wrong token"),
            Some("wrong-token"),
        )
        .unwrap_err();
        assert!(
            err.message.contains("launch token is invalid"),
            "{}",
            err.message
        );
    }

    #[test]
    fn re_register_after_consume_fails_sec7() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let token = "mini-once-token";
        seed_launch_pending(&projects, "mini-7", "mini", token).unwrap();
        agent_register(
            &projects,
            "mini-7",
            "mini",
            Some("qwen"),
            None,
            Some("x"),
            Some(token),
        )
        .unwrap();
        let err = agent_register(
            &projects,
            "mini-7",
            "mini",
            Some("qwen"),
            None,
            Some("x"),
            None,
        )
        .unwrap_err();
        assert!(
            err.message.contains("already consumed") || err.message.contains("launch"),
            "{}",
            err.message
        );
    }

    #[test]
    fn heartbeat_without_register_fails() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let err = agent_heartbeat(
            &projects,
            "ghost-agent",
            Some("active"),
            Some("try implicit register"),
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(err.message.contains("agent_register"), "{}", err.message);
    }

    #[test]
    fn launch_pending_blocks_heartbeat() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        // Pending without hash still blocks heartbeat.
        let state = json!({
            "version": 2,
            "updatedAt": "2026-05-29T00:00:00Z",
            "sessions": [{
                "agentId": "pending-coder",
                "role": "coder",
                "status": "launch_pending",
                "lastSeenAt": "2026-05-29T00:00:00Z",
            }],
            "claims": [],
            "events": [],
        });
        fs::write(
            projects.join(".aspis-agents.json"),
            serde_json::to_string_pretty(&state).unwrap(),
        )
        .unwrap();
        let err = agent_heartbeat(
            &projects,
            "pending-coder",
            Some("active"),
            Some("try"),
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(err.message.contains("launch is pending"), "{}", err.message);
    }

    #[test]
    fn session_token_required_for_heartbeat_wrong_token_fails() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let token = "hb-launch-token";
        seed_launch_pending(&projects, "hb-coder", "coder", token).unwrap();
        let ack = agent_register(
            &projects,
            "hb-coder",
            "coder",
            Some("opus"),
            None,
            Some("reg"),
            Some(token),
        )
        .unwrap();
        let session_token = ack["sessionToken"].as_str().unwrap().to_string();

        let missing = agent_heartbeat(
            &projects,
            "hb-coder",
            Some("active"),
            Some("alive"),
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(
            missing.message.contains("session_token"),
            "{}",
            missing.message
        );

        let wrong = agent_heartbeat(
            &projects,
            "hb-coder",
            Some("active"),
            Some("alive"),
            Some("wrong-token"),
            None,
            None,
        )
        .unwrap_err();
        assert!(
            wrong.message.contains("session token is invalid"),
            "{}",
            wrong.message
        );

        let ok = agent_heartbeat(
            &projects,
            "hb-coder",
            Some("active"),
            Some("alive"),
            Some(&session_token),
            None,
            None,
        )
        .unwrap();
        // Heartbeat never echoes sessionToken.
        assert!(ok.get("sessionToken").is_none());
        assert_eq!(ok["session"]["agentId"], "hb-coder");
    }

    #[test]
    fn mini_cannot_heartbeat() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let token = "mini-hb-token";
        seed_launch_pending(&projects, "mini-hb", "mini", token).unwrap();
        let ack = agent_register(
            &projects,
            "mini-hb",
            "mini",
            Some("qwen"),
            None,
            Some("reg"),
            Some(token),
        )
        .unwrap();
        let session_token = ack["sessionToken"].as_str().unwrap().to_string();
        let err = agent_heartbeat(
            &projects,
            "mini-hb",
            Some("active"),
            Some("nope"),
            Some(&session_token),
            None,
            None,
        )
        .unwrap_err();
        assert!(
            err.message.contains("mini") && err.message.contains("agent_heartbeat"),
            "{}",
            err.message
        );
    }

    #[test]
    fn heartbeat_rejects_reserved_status() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let token = "reserved-status-launch";
        seed_launch_pending(&projects, "rsv-coder", "coder", token).unwrap();
        let ack = agent_register(
            &projects,
            "rsv-coder",
            "coder",
            Some("opus"),
            None,
            Some("reg"),
            Some(token),
        )
        .unwrap();
        let session_token = ack["sessionToken"].as_str().unwrap().to_string();

        for status in ["launch_pending", "closed"] {
            let err = agent_heartbeat(
                &projects,
                "rsv-coder",
                Some(status),
                Some("spoof"),
                Some(&session_token),
                None,
                None,
            )
            .unwrap_err();
            assert!(
                err.message.contains("reserved"),
                "status={status}: {}",
                err.message
            );
        }

        // Control-char file path rejected.
        let err = agent_heartbeat(
            &projects,
            "rsv-coder",
            Some("active"),
            Some("alive"),
            Some(&session_token),
            Some("src/\nmain.rs"),
            None,
        )
        .unwrap_err();
        assert!(
            err.message.contains("control characters"),
            "{}",
            err.message
        );
    }

    #[test]
    fn agent_state_requires_register() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let err = agent_state(&projects, "nobody", "coder", None).unwrap_err();
        assert!(
            err.message.contains("agent_register") || err.message.contains("Agent"),
            "{}",
            err.message
        );
    }

    #[test]
    fn agent_state_returns_public_scrubbed() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let token = "state-launch";
        seed_launch_pending(&projects, "state-coder", "coder", token).unwrap();
        let ack = agent_register(
            &projects,
            "state-coder",
            "coder",
            Some("sonnet"),
            None,
            Some("reg"),
            Some(token),
        )
        .unwrap();
        let session_token = ack["sessionToken"].as_str().unwrap().to_string();
        let state = agent_state(&projects, "state-coder", "coder", Some(&session_token)).unwrap();
        assert!(state.get("sessions").is_some());
        let session = &state["sessions"][0];
        assert_eq!(session["agentId"], "state-coder");
        assert!(session.get("launchTokenHash").is_none());
        assert!(session.get("sessionTokenHash").is_none());
        assert!(session.get("launchConsumedAt").is_none());
        assert!(session.get("sessionTokenIssuedAt").is_none());
    }

    #[test]
    fn compact_ack_shape() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let token = "compact-launch-token-0123456789";
        seed_launch_pending(&projects, "compact-coder", "coder", token).unwrap();
        let registered = agent_register(
            &projects,
            "compact-coder",
            "coder",
            Some("opus"),
            None,
            Some("reg"),
            Some(token),
        )
        .unwrap();
        assert!(registered.get("sessionToken").is_some());
        assert!(registered.get("version").is_some());
        assert!(registered.get("updatedAt").is_some());
        assert_eq!(registered["fleet"]["sessions"], 1);
        assert_eq!(registered["fleet"]["active"], 1);
        assert!(registered.get("note").is_some());
        assert_eq!(registered["session"]["agentId"], "compact-coder");
        assert!(registered["session"].get("launchTokenHash").is_none());
        assert!(registered["session"].get("sessionTokenHash").is_none());
        assert!(registered["session"].get("launchConsumedAt").is_none());
        assert!(registered.get("sessions").is_none());
        assert!(registered.get("claims").is_none());
        assert!(registered.get("events").is_none());
        assert!(registered.get("rules").is_none());

        let session_token = registered["sessionToken"].as_str().unwrap().to_string();
        let heartbeat = agent_heartbeat(
            &projects,
            "compact-coder",
            Some("active"),
            Some("alive"),
            Some(&session_token),
            None,
            None,
        )
        .unwrap();
        assert_eq!(heartbeat["session"]["agentId"], "compact-coder");
        assert_eq!(heartbeat["fleet"]["sessions"], 1);
        assert!(heartbeat.get("sessionToken").is_none());
    }

    #[test]
    fn architect_alias_registers_as_coder() {
        let _g = env_lock();
        set_unmanaged(true);
        let (_tmp, projects) = temp_projects();
        let ack = agent_register(
            &projects,
            "architect-1",
            "architect",
            Some("planner"),
            None,
            Some("planning"),
            None,
        )
        .unwrap();
        // Stored role: normalize_role maps architect->coder on inbound; upsert
        // stores the normalized role when no prior session.
        assert_eq!(ack["session"]["role"], "coder");
        clear_unmanaged_env();
    }
}
