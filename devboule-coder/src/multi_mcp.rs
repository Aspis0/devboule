//! The multi-server MCP backend (Phase B): fans the executor's [`McpBackend`]
//! seam across the PRIVATE Oracle plus the user-configured MCP servers.
//!
//! See `docs/design-user-mcp-servers-2026-06.md` §2.5. The trust model (§5):
//! * the Oracle ([`crate::rmcp_backend::RmcpBackend`] via the Oracle `connect`) is
//!   ALWAYS present and ALWAYS first; the fixed Oracle tool names
//!   (`oracle_ask` / `oracle_context` / `spawn_mini_coder` / `plan_submit` /
//!   `project_*` / …) route to it through [`McpBackend::call_tool`], UNCHANGED.
//! * user servers (each its own [`crate::rmcp_backend::RmcpBackend`] via
//!   `connect_generic`) are appended after it and reached ONLY through the explicit
//!   [`McpBackend::call_user_tool`] path — an [`crate::action::AgentAction::McpTool`]
//!   the executor routes here by server NAME. A user server can never shadow an
//!   Oracle tool: the two dispatch surfaces are disjoint (Oracle = `call_tool`;
//!   user = `call_user_tool`), and the config layer (`user_mcp_config` in
//!   `src-tauri`) already forbids a user server from taking an Oracle name.
//!
//! Robustness: an unknown user-server name is a recoverable `Err` (NEVER a panic —
//! the burst recovers). A user server that fails to CONNECT at startup is logged
//! and SKIPPED (the Oracle still connects; the burst proceeds with the servers
//! that did connect) — see [`MultiMcpBackend::connect`].
//!
//! HARD invariant (design §6): this type lives ONLY in `devboule-coder` (the local
//! MAIN coder). It is never constructed for, or reachable from, the MINI coder.

use std::sync::Arc;

use async_trait::async_trait;

use crate::executor::McpBackend;
use crate::rmcp_backend::RmcpBackend;

/// The advertised tools of one connected user server: `(server name, [(tool,
/// description)])`. Returned by [`MultiMcpBackend::connect`] for the system-prompt
/// catalog (B.3) so the listing can be built while the concrete backend (which
/// exposes `list_tools`) is in hand. Plain tuples (not the `prompt::UserMcpServerTools`
/// type) so this module stays independent of the prompt module.
pub type UserServerCatalog = (String, Vec<(String, Option<String>)>);

/// One user MCP server's launch spec (the ENABLED, merged subset the launch wiring
/// passes in via `DEVBOULE_USER_MCP_SERVERS`). Mirrors the validated
/// `user_mcp_config::UserMcpServer` fields the local coder needs to spawn the child.
#[derive(Debug, Clone)]
pub struct UserServerSpec {
    /// The routing key — the configured server name (matches `McpTool { server }`).
    pub name: String,
    /// The child-process command (e.g. `python`, `node`, an absolute path).
    pub command: String,
    /// Arguments passed to `command`.
    pub args: Vec<String>,
    /// Environment variables for the child.
    pub env: Vec<(String, String)>,
}

/// The Oracle-plus-user-servers [`McpBackend`]. The Oracle is held FIRST and always;
/// user backends are a `(name, backend)` list reached only by [`call_user_tool`].
///
/// [`call_user_tool`]: McpBackend::call_user_tool
pub struct MultiMcpBackend {
    /// The PRIVATE Oracle backend — always present, always first. Fixed Oracle tool
    /// names route here through [`McpBackend::call_tool`].
    oracle: Arc<dyn McpBackend>,
    /// The connected USER servers, in launch order. Reached only via
    /// [`McpBackend::call_user_tool`], matched by `name`.
    user: Vec<(String, Arc<dyn McpBackend>)>,
    /// The connected user-server NAMES (the `user` keys), precomputed so
    /// [`McpBackend::user_server_names`] can hand back a `&[String]` slice — the burst
    /// validates an `mcp_tool` server name against this at parse time. A server that
    /// failed to connect is NOT here, so the model cannot call one that is not live.
    user_names: Vec<String>,
}

impl MultiMcpBackend {
    /// Build from a connected Oracle backend and the ALREADY-CONNECTED user backends.
    /// Pure (no I/O) so it is unit-testable with mock backends; the real connect path
    /// is [`MultiMcpBackend::connect`].
    pub fn new(oracle: Arc<dyn McpBackend>, user: Vec<(String, Arc<dyn McpBackend>)>) -> Self {
        let user_names = user.iter().map(|(n, _)| n.clone()).collect();
        Self {
            oracle,
            user,
            user_names,
        }
    }

    /// Overall wall-clock deadline for the WHOLE user-server connect phase. Each
    /// per-server `connect_generic` + `list_tools` is already individually bounded (by
    /// `rmcp_backend::CONNECT_TIMEOUT`, ~30s each), and the servers connect CONCURRENTLY
    /// (below), so the phase normally finishes in ~one such window. This is a belt-and-
    /// suspenders cap on the aggregate: even if many servers each hang right up to their
    /// own timeout, the startup (and thus the Oracle/burst that waits on it) is released
    /// after at most this long. A server not connected by the deadline is SKIPPED, exactly
    /// like a per-server failure. Sized to comfortably exceed a single per-server window.
    const CONNECT_PHASE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(45);

    /// Connect the user servers (each its own `connect_generic` stdio child) and wrap
    /// them with the already-connected Oracle into a [`MultiMcpBackend`]. The servers
    /// connect CONCURRENTLY (a hung/slow server no longer serializes behind the others),
    /// so the total wait is bounded by ~one per-server timeout window — not N× — plus an
    /// overall [`CONNECT_PHASE_DEADLINE`] safety cap. A user server that FAILS to connect
    /// (or is not connected by the deadline) is logged to stderr and SKIPPED — the Oracle
    /// is already up and the burst proceeds with the servers that did connect (design
    /// §2.5). The returned backend's `user_server_names()` lists ONLY the live servers, in
    /// the ORIGINAL spec order (concurrency does not reorder the catalog / routing list).
    ///
    /// Also returns each LIVE server's tool catalog (`(name, [(tool, desc)])`), fetched
    /// via `list_tools` on connect, for the system-prompt external-tools section (B.3).
    /// A server whose `list_tools` fails is still WIRED (its tools can be called by
    /// name) but contributes an EMPTY catalog (a warning is logged); the burst proceeds.
    ///
    /// SECURITY: each `spec` came from the user's own validated MCP config (charset-
    /// guarded names / env keys, control-char-free args/values). We spawn exactly the
    /// declared command with exactly the declared env — no orchestrator secrets are
    /// forwarded to a user child.
    pub async fn connect(
        oracle: Arc<dyn McpBackend>,
        specs: Vec<UserServerSpec>,
    ) -> (Self, Vec<UserServerCatalog>) {
        // Connect every server CONCURRENTLY. Each future returns
        // `Option<(name, backend, catalog)>` (None ⇒ this server failed and is skipped),
        // and the futures are joined IN ORDER so the surviving `Some`s preserve the
        // original spec order for deterministic routing + prompt-catalog output.
        let connects = specs.into_iter().map(|spec| async move {
            match RmcpBackend::connect_generic(&spec.command, &spec.args, &spec.env).await {
                Ok(backend) => {
                    // Fetch the tool catalog WHILE we still hold the concrete backend
                    // (the trait object does not expose list_tools). A failure here is
                    // non-fatal: the server is still wired; it just lists no tools.
                    let tools = match backend.list_tools().await {
                        Ok(t) => t,
                        Err(e) => {
                            eprintln!(
                                "devboule: user MCP server '{}' list_tools failed ({e}); \
                                 wired with no advertised tools",
                                spec.name
                            );
                            Vec::new()
                        }
                    };
                    Some((spec.name, Arc::new(backend) as Arc<dyn McpBackend>, tools))
                }
                Err(e) => {
                    // Skip, never abort: the Oracle is up and the burst must proceed.
                    // The server name is non-secret (charset-guarded); the env is NOT
                    // logged (it may carry the user's own credentials).
                    eprintln!(
                        "devboule: user MCP server '{}' failed to connect ({e}); skipping",
                        spec.name
                    );
                    None
                }
            }
        });

        // Bound the WHOLE phase: on the overall deadline, drop the still-pending connects
        // (each child is torn down on its transport drop) and proceed with NONE of the
        // user servers rather than wedging startup. Per-server timeouts make this rare; it
        // exists so a pathological set of hangs can never block the Oracle/burst forever.
        let results = match tokio::time::timeout(
            Self::CONNECT_PHASE_DEADLINE,
            futures::future::join_all(connects),
        )
        .await
        {
            Ok(results) => results,
            Err(_) => {
                eprintln!(
                    "devboule: user MCP servers did not all connect within {}s; \
                     proceeding without the user servers (Oracle still serves)",
                    Self::CONNECT_PHASE_DEADLINE.as_secs()
                );
                Vec::new()
            }
        };

        let mut user: Vec<(String, Arc<dyn McpBackend>)> = Vec::with_capacity(results.len());
        let mut catalog: Vec<UserServerCatalog> = Vec::with_capacity(results.len());
        for (name, backend, tools) in results.into_iter().flatten() {
            catalog.push((name.clone(), tools));
            user.push((name, backend));
        }
        (Self::new(oracle, user), catalog)
    }

}

#[async_trait]
impl McpBackend for MultiMcpBackend {
    /// Fixed Oracle tool names route to the Oracle backend, UNCHANGED. User servers
    /// are NEVER reachable here — they have a separate explicit path
    /// ([`McpBackend::call_user_tool`]) — so an Oracle dispatch can never be hijacked
    /// by a user server.
    async fn call_tool(&self, name: &str, params: serde_json::Value) -> Result<String, String> {
        self.oracle.call_tool(name, params).await
    }

    /// Route a user-MCP call to the named backend. An unknown server is a recoverable
    /// `Err` (the model gets a precise message; the burst continues) — NEVER a panic.
    async fn call_user_tool(
        &self,
        server: &str,
        tool: &str,
        params: serde_json::Value,
    ) -> Result<String, String> {
        match self.user.iter().find(|(name, _)| name == server) {
            // A user server's tool is called by its OWN tool name through `call_tool`
            // (the generic backend injects no identity, so params pass through verbatim).
            Some((_, backend)) => backend.call_tool(tool, params).await,
            None => Err(format!(
                "unknown user MCP server `{server}` (not connected / not configured)"
            )),
        }
    }

    fn user_server_names(&self) -> &[String] {
        &self.user_names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A mock backend that records every `call_tool(name, params)` and returns a
    /// labelled canned result so a test can tell WHICH backend served a call.
    struct LabelMock {
        label: &'static str,
        calls: Mutex<Vec<(String, serde_json::Value)>>,
    }
    impl LabelMock {
        fn new(label: &'static str) -> Arc<Self> {
            Arc::new(Self {
                label,
                calls: Mutex::new(Vec::new()),
            })
        }
        fn calls(&self) -> Vec<(String, serde_json::Value)> {
            self.calls.lock().unwrap().clone()
        }
    }
    #[async_trait]
    impl McpBackend for LabelMock {
        async fn call_tool(
            &self,
            name: &str,
            params: serde_json::Value,
        ) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push((name.to_string(), params));
            Ok(format!("[{} served {name}]", self.label))
        }
    }

    #[tokio::test]
    async fn oracle_tool_routes_to_oracle_not_user() {
        let oracle = LabelMock::new("oracle");
        let db = LabelMock::new("my-db");
        let multi = MultiMcpBackend::new(
            oracle.clone(),
            vec![("my-db".to_string(), db.clone() as Arc<dyn McpBackend>)],
        );

        let out = multi
            .call_tool("oracle_ask", serde_json::json!({"query": "x"}))
            .await
            .unwrap();
        assert!(out.contains("oracle served oracle_ask"), "got: {out}");
        // The user backend saw NOTHING.
        assert!(db.calls().is_empty(), "the user backend must not be touched");
        assert_eq!(oracle.calls().len(), 1, "the oracle served the call");
    }

    #[tokio::test]
    async fn mcp_tool_routes_to_the_named_user_backend() {
        let oracle = LabelMock::new("oracle");
        let db = LabelMock::new("my-db");
        let other = LabelMock::new("other");
        let multi = MultiMcpBackend::new(
            oracle.clone(),
            vec![
                ("my-db".to_string(), db.clone() as Arc<dyn McpBackend>),
                ("other".to_string(), other.clone() as Arc<dyn McpBackend>),
            ],
        );

        let out = multi
            .call_user_tool("my-db", "query", serde_json::json!({"sql": "SELECT 1"}))
            .await
            .unwrap();
        assert!(out.contains("my-db served query"), "routed to my-db: {out}");
        // ONLY my-db saw the call (by its own tool name `query`); oracle + other did not.
        assert_eq!(db.calls().len(), 1);
        assert_eq!(db.calls()[0].0, "query", "the user tool name is used, not mcp_tool");
        assert!(oracle.calls().is_empty(), "oracle untouched");
        assert!(other.calls().is_empty(), "the other user server untouched");
    }

    #[tokio::test]
    async fn unknown_user_server_is_recoverable_err_not_panic() {
        let oracle = LabelMock::new("oracle");
        let db = LabelMock::new("my-db");
        let multi = MultiMcpBackend::new(
            oracle.clone(),
            vec![("my-db".to_string(), db.clone() as Arc<dyn McpBackend>)],
        );

        let err = multi
            .call_user_tool("ghost", "query", serde_json::json!({}))
            .await
            .expect_err("an unknown server must be an Err, not a panic");
        assert!(err.contains("unknown user MCP server"), "{err}");
        assert!(err.contains("ghost"), "names the bad server: {err}");
        // No backend was invoked for the unknown server.
        assert!(db.calls().is_empty());
        assert!(oracle.calls().is_empty());
    }

    #[tokio::test]
    async fn connect_skips_servers_that_fail_to_spawn_and_keeps_oracle() {
        // FIX 1 (concurrent connect): a user server that cannot even spawn (a bogus
        // command) is SKIPPED — the connect phase still returns, the Oracle is wrapped,
        // and the surviving user set is empty. Two bogus specs prove the concurrent path
        // joins all of them and drops the failures without wedging (each spawn-failure is
        // fast). The deadline cap is not exercised here (spawn fails immediately).
        let oracle = LabelMock::new("oracle");
        let bogus = |n: &str| UserServerSpec {
            name: n.to_string(),
            // A command path that cannot exist ⇒ connect_generic's TokioChildProcess::new
            // returns Err immediately (no hang), so this is a fast, deterministic skip.
            command: "/nonexistent/devboule-test-bogus-mcp-binary".to_string(),
            args: vec![],
            env: vec![],
        };
        let (multi, catalog) = MultiMcpBackend::connect(
            oracle.clone(),
            vec![bogus("alpha"), bogus("beta")],
        )
        .await;
        // Both failed to connect ⇒ no live user servers, empty catalog.
        assert!(
            multi.user_server_names().is_empty(),
            "a server that fails to spawn must be skipped, leaving no live user servers"
        );
        assert!(catalog.is_empty(), "a skipped server contributes no catalog entry");
        // The Oracle is still wired and serves its tools.
        let out = multi
            .call_tool("oracle_ask", serde_json::json!({"query": "x"}))
            .await
            .unwrap();
        assert!(out.contains("oracle served oracle_ask"), "oracle still serves: {out}");
    }

    #[test]
    fn user_server_names_lists_only_configured_servers() {
        let oracle = LabelMock::new("oracle");
        let db = LabelMock::new("my-db");
        let api = LabelMock::new("api");
        let multi = MultiMcpBackend::new(
            oracle,
            vec![
                ("my-db".to_string(), db as Arc<dyn McpBackend>),
                ("api".to_string(), api as Arc<dyn McpBackend>),
            ],
        );
        assert_eq!(
            multi.user_server_names(),
            &["my-db".to_string(), "api".to_string()]
        );
    }
}
