//! The real MCP transport (L2.3): an [`McpBackend`] backed by the `rmcp` client
//! over a stdio CHILD-PROCESS transport.
//!
//! ALL rmcp specifics are isolated in this module so the rest of the executor is
//! transport-agnostic and unit-testable against `MockMcpBackend`. This module
//! itself needs a LIVE Python Oracle server, so it is INTEGRATION-tested later
//! (not in the unit run) — but it MUST compile, and the action->params mapping
//! it serves is unit-tested via the mock in [`crate::executor`].
//!
//! Lifecycle:
//! 1. spawn `python -m oracle.server.aspis_mcp --root <root> --projects-dir
//!    <dir>` as a child (config-supplied; nothing hardcoded — the L2.4 launch
//!    wiring provides it),
//! 2. run the MCP `initialize` handshake (the `().serve(transport)` client
//!    handler does this),
//! 3. call `agent_register` with `{role, agent_id, model, ...}` to obtain the
//!    `sessionToken`, store it,
//! 4. inject `role` / `agent_id` / `session_token` into EVERY subsequent
//!    `call_tool`,
//! 5. on drop, cancel the running service (which terminates the child).

use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{RoleClient, RunningService, RunningServiceCancellationToken};
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::transport::ConfigureCommandExt;
use rmcp::ServiceExt;
use serde_json::{json, Map, Value};
use tokio::process::Command;

use crate::executor::McpBackend;

/// Config to launch + register against the Oracle MCP server. Nothing is
/// hardcoded: the L2.4 Devboule launch wiring supplies the interpreter, the
/// server root, the projects dir, and the agent identity.
#[derive(Debug, Clone)]
pub struct RmcpConfig {
    /// The Python interpreter (e.g. an absolute venv path). Defaults to `python`
    /// only if the caller passes that explicitly.
    pub python: String,
    /// The Oracle server root passed as `--root`.
    pub root: String,
    /// The projects dir passed as `--projects-dir`.
    pub projects_dir: String,
    /// This agent's id (registered + injected into every call).
    pub agent_id: String,
    /// This agent's role. L2.3 registers as `orchestrator` (the role the
    /// parallel server change adds); the server aliases/normalizes as needed.
    pub role: String,
    /// The model id declared to `agent_register` (for the dashboard). May be
    /// empty.
    pub model: String,
    /// Extra env vars for the child (e.g. an Oracle index path). Never secrets in
    /// logs — these are passed to the child env only.
    pub env: Vec<(String, String)>,
}

/// The live rmcp-over-stdio backend. Holds the running client service, the
/// session identity, and the cancellation token used to terminate the child on
/// drop.
pub struct RmcpBackend {
    /// The running client service. Sole owner (never cloned): `peer()` borrows it
    /// for each call, and `Drop` cancels it to terminate the child.
    service: RunningService<RoleClient, ()>,
    agent_id: String,
    role: String,
    session_token: String,
    cancel: Option<RunningServiceCancellationToken>,
}

impl RmcpBackend {
    /// Spawn the Oracle server as a child, run the MCP handshake, register as the
    /// configured role, and store the returned `sessionToken`. INTEGRATION path:
    /// requires a live Python server, so it is not exercised in the unit run.
    pub async fn connect(config: RmcpConfig) -> Result<Self, String> {
        // Build the child command from config — NOTHING hardcoded. `-m
        // oracle.server.aspis_mcp` with `--root` / `--projects-dir`, cwd at the
        // root, and the supplied env. stdin/stdout are the MCP transport; stderr
        // is inherited so server diagnostics are visible (and never parsed).
        let command = Command::new(&config.python).configure(|cmd| {
            cmd.arg("-m")
                .arg("oracle.server.aspis_mcp")
                .arg("--root")
                .arg(&config.root)
                .arg("--projects-dir")
                .arg(&config.projects_dir)
                .current_dir(&config.root);
            for (k, v) in &config.env {
                cmd.env(k, v);
            }
        });

        let transport = TokioChildProcess::new(command)
            .map_err(|e| format!("failed to spawn Oracle MCP child: {e}"))?;

        // `()` is the default client handler; `.serve` runs the initialize
        // handshake and returns the running service.
        let service = ().serve(transport).await.map_err(|e| {
            format!("MCP initialize handshake failed: {e}")
        })?;

        // From here the child is RUNNING but `Self` is not yet constructed, so a
        // bare `?`/early `return Err` on the registration below would drop neither
        // `service` nor a `Self` whose `Drop` cancels — the child would be
        // ORPHANED. Take the cancellation token now: on ANY post-spawn error,
        // `cleanup.cancel()` closes the transport (terminating the child) before
        // returning; on success the SAME token is stored in `self.cancel`, so
        // `Drop` later cancels it to tear the child down on shutdown.
        let cleanup = service.cancellation_token();

        // Register to obtain the session token. This call carries identity but no
        // token yet (registration is what mints it).
        let mut reg_args = Map::new();
        reg_args.insert("agent_id".into(), json!(config.agent_id));
        reg_args.insert("role".into(), json!(config.role));
        reg_args.insert("model".into(), json!(config.model));
        reg_args.insert("message".into(), json!("devboule orchestrator online"));

        let result = match service
            .peer()
            .call_tool(CallToolRequestParams::new("agent_register").with_arguments(reg_args))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                cleanup.cancel();
                return Err(format!("agent_register failed: {e}"));
            }
        };

        let text = match first_text(&result) {
            Some(t) => t,
            None => {
                cleanup.cancel();
                return Err("agent_register returned no text content".to_string());
            }
        };
        let session_token = match extract_session_token(&text) {
            Some(t) => t,
            None => {
                cleanup.cancel();
                return Err("agent_register did not return a sessionToken".to_string());
            }
        };

        Ok(Self {
            service,
            agent_id: config.agent_id,
            role: config.role,
            session_token,
            cancel: Some(cleanup),
        })
    }
}

#[async_trait]
impl McpBackend for RmcpBackend {
    async fn call_tool(&self, name: &str, params: Value) -> Result<String, String> {
        // Inject the session identity into EVERY call: role / agent_id /
        // session_token. The action-specific params are merged on top.
        //
        // The executor ALWAYS passes a JSON object. A non-object is a programming
        // error, and the old `_ => Map::new()` fallback silently dropped the
        // caller's params and sent an ARGUMENT-LESS tool call — a future refactor
        // could then ship a tool call with no arguments and never notice. Hard-fail
        // instead so the mistake surfaces immediately.
        let mut args = match params {
            Value::Object(map) => map,
            other => {
                return Err(format!(
                    "call_tool {name}: params is not a JSON object: {other}"
                ));
            }
        };
        args.insert("role".into(), json!(self.role));
        args.insert("agent_id".into(), json!(self.agent_id));
        args.insert("session_token".into(), json!(self.session_token));

        // `name` is a borrowed &str but CallToolRequestParams wants a
        // Cow<'static, str>, so own it.
        let result = self
            .service
            .peer()
            .call_tool(CallToolRequestParams::new(name.to_string()).with_arguments(args))
            .await
            .map_err(|e| format!("call_tool {name} failed: {e}"))?;

        // A tool-level error (`is_error == Some(true)`) is surfaced as an Err so
        // the executor turns it into a failed ToolResult the model can recover
        // from.
        let text = first_text(&result).unwrap_or_default();
        if result.is_error.unwrap_or(false) {
            return Err(if text.is_empty() {
                format!("tool {name} reported an error")
            } else {
                text
            });
        }
        Ok(text)
    }
}

impl Drop for RmcpBackend {
    fn drop(&mut self) {
        // Cancel the running service synchronously; this drives the transport to
        // close, which terminates the child process. `cancellation_token().cancel()`
        // is a non-async fire-and-forget, safe in Drop.
        if let Some(token) = self.cancel.take() {
            token.cancel();
        }
    }
}

/// Extract the first text-content block from a tool result. The Oracle returns
/// its JSON payload as a text content block (the MCP convention).
fn first_text(result: &CallToolResult) -> Option<String> {
    result
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
}

/// Pull the `sessionToken` out of an `agent_register` text result. The server
/// returns a JSON object carrying `sessionToken` (camelCase — matches the
/// Python eval harness). Total + pure so it is unit-testable without a server.
fn extract_session_token(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    value
        .get("sessionToken")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    // These unit tests cover the PURE helpers (token extraction). The live
    // connect()/call_tool() path needs a Python server and is integration-
    // deferred; the action->params mapping it serves is unit-tested via
    // MockMcpBackend in crate::executor.

    #[test]
    fn extract_session_token_reads_camelcase_field() {
        let body = r#"{"sessionToken": "tok-abc123", "role": "orchestrator"}"#;
        assert_eq!(extract_session_token(body).as_deref(), Some("tok-abc123"));
    }

    #[test]
    fn extract_session_token_rejects_missing_or_empty() {
        assert_eq!(extract_session_token(r#"{"role":"x"}"#), None);
        assert_eq!(extract_session_token(r#"{"sessionToken":""}"#), None);
        assert_eq!(extract_session_token("not json"), None);
    }
}
