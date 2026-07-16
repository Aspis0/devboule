//! `oracle-mcp` binary entry point.
//!
//! Thin `#[tokio::main]` wrapper around the library server in
//! `oracle_core::mcp` (kept in the lib so integration tests can construct
//! `OracleMcp` directly). All tool/handler logic lives in `crate::mcp`.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    oracle_core::mcp::serve_stdio().await
}
