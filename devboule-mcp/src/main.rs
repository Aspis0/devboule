//! `devboule-mcp` — native app-tools MCP server (stdio).
//!
//! Replaces `python -m oracle.server.aspis_mcp` as tools are ported.
//! Phase P0: scaffold + `agent_rules` only. See `docs/devboule-mcp-port-plan.md`.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    devboule_mcp::serve_stdio().await
}
