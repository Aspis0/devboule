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

    /// Practical roles, responsibilities and tool allowlists for Devboule agents.
    #[tool(
        description = "Return Devboule agent role rules (summaries, forbidden, contracts, allowedTools). English SSoT from role_rules.json."
    )]
    pub async fn agent_rules(
        &self,
        Parameters(_args): Parameters<EmptyArgs>,
    ) -> Result<CallToolResult, McpError> {
        let rules = load_role_rules().map_err(internal)?;
        let body = json!({
            "server": "devboule-mcp",
            "version": DEVBOULE_MCP_VERSION,
            "backend": "rust",
            "roles": rules,
            // Port progress: only tools listed in the live router are callable.
            "portPhase": "P0",
            "note": "Partial Rust MCP. Unimplemented tools still require DEVBOULE_MCP_BACKEND=python until cutover.",
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
                "Devboule app-tools MCP (Rust). Prefer this over oracle.server.aspis_mcp once port is complete."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

fn load_role_rules() -> Result<Value> {
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
        let roles = load_role_rules().expect("parse");
        let arr = roles.as_array().expect("array");
        assert_eq!(arr.len(), 4);
        let names: Vec<&str> = arr
            .iter()
            .filter_map(|r| r.get("role").and_then(|x| x.as_str()))
            .collect();
        assert_eq!(names, vec!["coder", "orchestrator", "verifier", "mini"]);
    }

    #[test]
    fn version_is_nonempty() {
        assert!(!DEVBOULE_MCP_VERSION.is_empty());
    }
}
