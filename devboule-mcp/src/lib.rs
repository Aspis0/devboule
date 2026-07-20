//! Devboule app-tools MCP (Rust).
//!
//! Port of `oracle/server/aspis_mcp.py` by phase — see `docs/devboule-mcp-port-plan.md`.
//! Branding: server process is **devboule-mcp** (not aspis_mcp).

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
use tools::agent_lifecycle;

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

#[tool_router]
impl DevbouleMcp {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    /// Practical roles for Devboule agents (P1: lifecycle tools).
    ///
    /// Each role keeps only `role`, filtered `allowedTools`, and a short summary.
    #[tool(
        description = "Return Devboule agent role rules for this Rust MCP (P1: agent_rules + register/heartbeat/state). Roles are slim: role, allowedTools, summary. Use DEVBOULE_MCP_BACKEND=python for full tools and full role prose."
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
            "portPhase": "P1",
            "note": "Partial Rust MCP (P1). Lifecycle tools implemented; project/cloud/oracle still require DEVBOULE_MCP_BACKEND=python until cutover.",
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
}

#[tool_handler]
impl ServerHandler for DevbouleMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Devboule app-tools MCP (Rust). P1: agent_rules + agent_register + agent_heartbeat + agent_state. Prefer DEVBOULE_MCP_BACKEND=python until cutover for full tool set."
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
const P1_ROLE_SUMMARY: &str =
    "P1 Rust MCP: agent_rules, agent_register, agent_heartbeat, agent_state (role-filtered). Use DEVBOULE_MCP_BACKEND=python for project/cloud/oracle tools.";

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
            "summary": P1_ROLE_SUMMARY,
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
            assert_eq!(role["summary"], P1_ROLE_SUMMARY);
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
    fn coder_gets_lifecycle_tools() {
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
        ] {
            assert!(tools.contains(&need), "coder missing {need}");
        }
    }

    #[test]
    fn client_roles_payload_does_not_name_unimplemented_tools() {
        let roles = load_role_rules_for_client().expect("parse");
        let dumped = roles.to_string();
        for banned in [
            "plan_submit",
            "spawn_mini_coder",
            "cloudflare_rotate",
            "request_git_push",
            "censor_dispose",
            "project_set_status",
            "oracle_context",
        ] {
            assert!(
                !dumped.contains(banned),
                "P1 role payload must not mention unimplemented tool {banned}: {dumped}"
            );
        }
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
            n > IMPLEMENTED_TOOLS.len(),
            "expected full role_rules to list more tools than P1 implements (got {n})"
        );
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
