//! Devboule app-tools MCP (Rust).
//!
//! Port of `oracle/server/aspis_mcp.py` by phase — see `docs/devboule-mcp-port-plan.md`.
//! Branding: server process is **devboule-mcp** (not aspis_mcp).

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

/// SSoT role rules (same file as the Tauri app and legacy Python MCP).
const ROLE_RULES_JSON: &str = include_str!("../../oracle/server/role_rules.json");

/// Server version string advertised to clients.
pub const DEVBOULE_MCP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Tools actually registered on this server's tool router.
/// `agent_rules` must NEVER advertise tools that are not callable (P0 audit).
const IMPLEMENTED_TOOLS: &[&str] = &["agent_rules"];

#[derive(Clone)]
pub struct DevbouleMcp {
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EmptyArgs {}

#[tool_router]
impl DevbouleMcp {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    /// Practical roles for Devboule agents (P0 slim payload).
    ///
    /// Each role keeps only `role`, filtered `allowedTools`, and a short summary.
    /// Full SSoT prose (forbidden, contracts, launchPrompt, etc.) lives in
    /// `role_rules.json` for the Python backend — advertising it here would name
    /// tools this binary does not implement.
    #[tool(
        description = "Return Devboule agent role rules for this Rust MCP (P0: agent_rules only). Roles are slim: role, allowedTools, summary. Use DEVBOULE_MCP_BACKEND=python for full tools and full role prose."
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
            "portPhase": "P0",
            "note": "Partial Rust MCP (P0). Each role is slim (role + allowedTools + summary). Full role SSoT and unimplemented tools require DEVBOULE_MCP_BACKEND=python until cutover.",
        });
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
                "Devboule app-tools MCP (Rust). P0: agent_rules only. Prefer DEVBOULE_MCP_BACKEND=python until cutover for full tool set."
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

/// Short per-role summary for the P0 slim payload (no tool names beyond agent_rules).
const P0_ROLE_SUMMARY: &str =
    "P0 Rust MCP: only agent_rules is available on this server. Use DEVBOULE_MCP_BACKEND=python for full tools.";

/// Load role rules, filter `allowedTools` to `IMPLEMENTED_TOOLS`, and strip prose
/// that would advertise unimplemented tools (forbidden lists, contracts, etc.).
fn load_role_rules_for_client() -> Result<Value> {
    let json = ROLE_RULES_JSON.trim_start_matches('\u{feff}');
    let v: Value = serde_json::from_str(json).context("role_rules.json parse")?;
    let mut roles = v
        .get("roles")
        .cloned()
        .context("role_rules.json missing roles[]")?;
    slim_roles_for_p0(&mut roles);
    Ok(roles)
}

/// Keep only `role`, filtered `allowedTools`, and a short P0 `summary` per role.
///
/// Note: `agent_rules` is **not** listed in role_rules.json `allowedTools` (it is a
/// meta/bootstrap tool always available). After filtering we still surface it so
/// agents know the Rust server exposes it.
fn slim_roles_for_p0(roles: &mut Value) {
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

        // Filter allowedTools to implemented only, always include agent_rules.
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
        for &name in IMPLEMENTED_TOOLS {
            if !tools.iter().any(|t| t.as_str() == Some(name)) {
                tools.push(json!(name));
            }
        }

        *role = json!({
            "role": role_name,
            "allowedTools": tools,
            "summary": P0_ROLE_SUMMARY,
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
            // agent_rules itself must remain for every role that had any tools.
            assert!(
                tools.iter().any(|t| t == "agent_rules"),
                "role {:?} lost agent_rules after filter",
                role.get("role")
            );
            // Slim payload: only role / allowedTools / summary keys.
            let obj = role.as_object().expect("role object");
            for key in obj.keys() {
                assert!(
                    matches!(key.as_str(), "role" | "allowedTools" | "summary"),
                    "role {:?} kept unexpected key {key}",
                    role.get("role")
                );
            }
            assert_eq!(role["summary"], P0_ROLE_SUMMARY);
        }
    }

    #[test]
    fn client_roles_payload_does_not_name_unimplemented_tools() {
        // Hostile: filtered JSON must not contain strings of tools we don't ship.
        let roles = load_role_rules_for_client().expect("parse");
        let dumped = roles.to_string();
        for banned in [
            "plan_submit",
            "spawn_mini_coder",
            "cloudflare_rotate",
            "request_git_push",
            "censor_dispose",
            "project_set_status",
            "agent_register",
            "oracle_context",
        ] {
            assert!(
                !dumped.contains(banned),
                "P0 role payload must not mention unimplemented tool {banned}: {dumped}"
            );
        }
    }

    #[test]
    fn raw_rules_have_more_tools_than_implemented() {
        // Guard: if someone empties role_rules, the filter test becomes a no-op.
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
            "expected full role_rules to list more tools than P0 implements (got {n})"
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
}
