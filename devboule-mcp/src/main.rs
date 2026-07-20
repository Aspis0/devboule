//! `devboule-mcp` — native app-tools MCP server (stdio).
//!
//! Replaces `python -m oracle.server.aspis_mcp` as tools are ported.
//! Phase P1: agent_rules + agent_register + agent_heartbeat + agent_state.
//! See `docs/devboule-mcp-port-plan.md`.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    devboule_mcp::serve_stdio().await
}
