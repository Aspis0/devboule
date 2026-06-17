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

use std::time::Duration;

use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{RoleClient, RunningService, RunningServiceCancellationToken};
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::transport::ConfigureCommandExt;
use rmcp::ServiceExt;
use serde_json::{json, Map, Value};
use tokio::process::Command;
use tokio::time::timeout;

use crate::executor::McpBackend;

/// Wall-clock cap on the connect path (MCP `initialize` handshake + the
/// `agent_register` round-trip). A hung Oracle server must NOT block
/// `build_runtime` forever: without this the `.await` never returns and the
/// burst's wall-clock guard (which only runs BETWEEN awaits) can never fire. On
/// elapse we tear the child down and return `Err`, which `config::build_executor`
/// turns into the safe Stub fallback.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default wall-clock cap on a single `call_tool` for the FAST, non-blocking tools
/// (oracle_*, project_*, agent_*): generous for grounded retrieval / a Kanban op, but
/// FINITE so a hung server turns one call into a recoverable `ToolResult::err` instead
/// of wedging the whole burst on a pending `.await`.
const DEFAULT_CALL_TOOL_TIMEOUT: Duration = Duration::from_secs(120);

/// Per-tool wall-clock cap on a single `call_tool`.
///
/// CRITICAL INVARIANT: a tool whose SERVER handler BLOCKS on a bounded poll (it waits
/// for a spawned mini, a human approval gate, or a render) MUST get a CLIENT timeout
/// that EXCEEDS that server poll — otherwise the client gives up while the server is
/// still working, surfacing a misleading transport timeout and (for the 11.3 runner) a
/// FALSE `blocked` task while the mini is still running. A real mini-coder burst easily
/// takes minutes (owner: "5 minuti, facile"); the old flat 120s was far too short.
///
/// These mirror the server's poll constants in `oracle/server/aspis_mcp.py` (+ margin
/// for the server's final result-stamp + transport). Keep each ≥ its server counterpart
/// if those change:
///   spawn_mini_coder : MINI_CODER_POLL_TIMEOUT_SECS  = 1800 -> 1920 (32 min)
///   plan_submit      : PLAN_POLL_TIMEOUT_SECS        = 600  -> 720
///   request_git_push : GIT_PUSH_POLL_TIMEOUT_SECS    = 600  -> 720
///   ask_user         : ASK_USER_POLL_TIMEOUT_SECS    = 600  -> 720
///   visual_check     : VISUAL_CHECK_POLL_TIMEOUT_SECS = 120 -> 240
/// Everything else falls back to [`DEFAULT_CALL_TOOL_TIMEOUT`].
fn call_tool_timeout(tool: &str) -> Duration {
    let secs = match tool {
        "spawn_mini_coder" => 1920,
        "plan_submit" | "request_git_push" | "ask_user" => 720,
        "visual_check" => 240,
        _ => return DEFAULT_CALL_TOOL_TIMEOUT,
    };
    Duration::from_secs(secs)
}

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
    /// The app-issued launch token (L2.4). The Oracle server stamps a
    /// `launchTokenHash` on this agent's pending session BEFORE the launch, so
    /// `agent_register` REQUIRES the matching raw token (see
    /// `validate_launch_token_for_registration` in `oracle/server/aspis_mcp.py`):
    /// without it the server raises and registration fails. Empty only for the
    /// unmanaged / no-token dev path (a server with the compat kill switch on, or
    /// a session with no hash). NEVER logged — it is sent in the register params
    /// only.
    pub launch_token: String,
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
    /// The per-call session token. Non-empty on the MANAGED path (the server minted
    /// it and enforces it on every call). EMPTY on the UNMANAGED dev/CI path, where
    /// the server issues no token and accepts tokenless calls; `call_tool` then omits
    /// the `session_token` key entirely.
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
        // handshake and returns the running service. A hung server must not block
        // `build_runtime` forever, so the handshake is bounded: on elapse the
        // transport is dropped (terminating the child) and we return `Err`.
        let service = match timeout(CONNECT_TIMEOUT, ().serve(transport)).await {
            Ok(Ok(service)) => service,
            Ok(Err(e)) => return Err(format!("MCP initialize handshake failed: {e}")),
            Err(_) => {
                return Err(format!(
                    "MCP initialize handshake timed out after {}s",
                    CONNECT_TIMEOUT.as_secs()
                ))
            }
        };

        // From here the child is RUNNING but `Self` is not yet constructed, so a
        // bare `?`/early `return Err` on the registration below would drop neither
        // `service` nor a `Self` whose `Drop` cancels — the child would be
        // ORPHANED. Take the cancellation token now: on ANY post-spawn error,
        // `cleanup.cancel()` closes the transport (terminating the child) before
        // returning; on success the SAME token is stored in `self.cancel`, so
        // `Drop` later cancels it to tear the child down on shutdown.
        let cleanup = service.cancellation_token();

        // Register to obtain the session token. This call carries identity plus the
        // app-issued LAUNCH token (the one-shot credential the server requires to
        // mint the session token); registration consumes the launch token and
        // returns the session token. The field name `launch_token` matches the
        // server's `agent_register` schema (oracle/server/aspis_mcp.py). An empty
        // launch token is only valid on the unmanaged/no-hash dev path; against a
        // managed launch the server rejects a blank or wrong token.
        let reg_args = build_register_args(&config);

        // Bound the registration round-trip too: a server that completed the
        // handshake but then hangs on `agent_register` would otherwise wedge
        // startup just the same. On timeout, tear the child down before returning.
        let register = service
            .peer()
            .call_tool(CallToolRequestParams::new("agent_register").with_arguments(reg_args));
        let result = match timeout(CONNECT_TIMEOUT, register).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                cleanup.cancel();
                return Err(format!("agent_register failed: {e}"));
            }
            Err(_) => {
                cleanup.cancel();
                return Err(format!(
                    "agent_register timed out after {}s",
                    CONNECT_TIMEOUT.as_secs()
                ));
            }
        };

        let text = match first_text(&result) {
            Some(t) => t,
            None => {
                cleanup.cancel();
                return Err("agent_register returned no text content".to_string());
            }
        };
        // MANAGED vs UNMANAGED token handling. A MANAGED launch supplies a launch
        // token; the server mints a sessionToken and stamps a per-agent hash, then
        // REQUIRES that exact token on every subsequent call — so a managed register
        // that returns no token is a hard error (we must never proceed tokenless
        // against a server that will reject every call). The UNMANAGED dev/CI path
        // (no launch token; server has ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS=1)
        // registers WITHOUT a session hash and returns an empty/absent sessionToken;
        // `require_session_token` in aspis_mcp.py then ACCEPTS a tokenless call for
        // exactly that case. So tolerate an empty token there and proceed with an
        // empty one, which `call_tool` omits from the args (matching the server's
        // tokenless-call contract). See oracle/server/aspis_mcp.py:require_session_token.
        let managed = !config.launch_token.trim().is_empty();
        let session_token = match resolve_session_token(&text, managed) {
            Ok(t) => t,
            Err(e) => {
                cleanup.cancel();
                return Err(e);
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
        // Inject the session token only when we HAVE one (the managed path). On the
        // unmanaged path the token is empty: leave the key ABSENT so the server sees
        // a tokenless call, which `require_session_token` accepts for an unmanaged
        // (no-hash) session. Sending `""` would also be accepted there, but omitting
        // the key is the honest representation of "no session token".
        if !self.session_token.is_empty() {
            args.insert("session_token".into(), json!(self.session_token));
        }

        // `name` is a borrowed &str but CallToolRequestParams wants a
        // Cow<'static, str>, so own it. The call is bounded by the PER-TOOL timeout:
        // a hung server turns a pending `.await` into a recoverable error the burst
        // can feed back to the model, instead of wedging the whole burst (the
        // wall-clock guard only runs BETWEEN awaits, never during a pending one). The
        // per-tool value EXCEEDS the server's blocking poll for slow tools (a mini burst,
        // a human gate) so a genuinely-working call is never cut short into a false error.
        let call_timeout = call_tool_timeout(name);
        let call = self
            .service
            .peer()
            .call_tool(CallToolRequestParams::new(name.to_string()).with_arguments(args));
        let result = match timeout(call_timeout, call).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(format!("call_tool {name} failed: {e}")),
            Err(_) => {
                return Err(format!(
                    "call_tool {name} timed out after {}s",
                    call_timeout.as_secs()
                ))
            }
        };

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

/// Build the `agent_register` argument map from the config. PURE (no transport) so
/// the identity + launch-token wiring is unit-testable without a live server. The
/// `launch_token` field name matches the server's `agent_register` schema
/// (`oracle/server/aspis_mcp.py`), which REQUIRES it for a managed launch.
fn build_register_args(config: &RmcpConfig) -> Map<String, Value> {
    let mut reg_args = Map::new();
    reg_args.insert("agent_id".into(), json!(config.agent_id));
    reg_args.insert("role".into(), json!(config.role));
    reg_args.insert("model".into(), json!(config.model));
    reg_args.insert("message".into(), json!("devboule orchestrator online"));
    reg_args.insert("launch_token".into(), json!(config.launch_token));
    reg_args
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

/// Decide the session token to store from an `agent_register` result, given
/// whether this was a MANAGED registration (a launch token was supplied).
///
/// * MANAGED: the server mints a sessionToken and stamps a per-agent hash it then
///   enforces on every call (`require_session_token` in `oracle/server/aspis_mcp.py`).
///   A missing token here means every subsequent call would be rejected, so it is a
///   HARD ERROR — never proceed tokenless against a server that will refuse us.
/// * UNMANAGED (dev/CI, `ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS=1`): the server
///   registers WITHOUT a session hash and returns an empty/absent sessionToken;
///   `require_session_token` then ACCEPTS a tokenless call for exactly that case. So
///   an empty token is TOLERATED and returned as `""` — `call_tool` omits the key,
///   yielding the tokenless call the server expects. If the unmanaged server DID
///   return a token, honor it.
///
/// Pure + total so it is unit-testable without a live server.
fn resolve_session_token(register_text: &str, managed: bool) -> Result<String, String> {
    match extract_session_token(register_text) {
        Some(token) => Ok(token),
        None if managed => Err("agent_register did not return a sessionToken".to_string()),
        // Unmanaged: no token is expected and the server accepts tokenless calls.
        None => Ok(String::new()),
    }
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
    fn call_tool_timeout_exceeds_server_poll_for_blocking_tools() {
        // The load-bearing invariant: the CLIENT timeout must exceed the SERVER blocking
        // poll for each slow tool, so the server's bounded poll returns a real terminal
        // outcome before the client gives up (no false transport timeout / false block).
        // Server polls (aspis_mcp.py): mini=1800, plan/push/ask=600, visual=120.
        assert!(
            call_tool_timeout("spawn_mini_coder").as_secs() > 1800,
            "mini > 1800s"
        );
        assert!(
            call_tool_timeout("plan_submit").as_secs() > 600,
            "plan gate > 600s"
        );
        assert!(
            call_tool_timeout("request_git_push").as_secs() > 600,
            "push gate > 600s"
        );
        assert!(
            call_tool_timeout("ask_user").as_secs() > 600,
            "ask_user gate > 600s"
        );
        assert!(
            call_tool_timeout("visual_check").as_secs() > 120,
            "visual > 120s"
        );
        // Fast / unknown tools fall back to the (shorter) default.
        assert_eq!(call_tool_timeout("oracle_ask"), DEFAULT_CALL_TOOL_TIMEOUT);
        assert_eq!(
            call_tool_timeout("project_structure"),
            DEFAULT_CALL_TOOL_TIMEOUT
        );
        // A mini burst of several minutes (owner: "5 minuti, facile") is comfortably under.
        assert!(call_tool_timeout("spawn_mini_coder").as_secs() >= 300 * 2);
    }

    #[test]
    fn extract_session_token_rejects_missing_or_empty() {
        assert_eq!(extract_session_token(r#"{"role":"x"}"#), None);
        assert_eq!(extract_session_token(r#"{"sessionToken":""}"#), None);
        assert_eq!(extract_session_token("not json"), None);
    }

    #[test]
    fn resolve_session_token_managed_requires_a_token() {
        // MANAGED launch (a launch token was supplied): the server mints a
        // sessionToken and enforces it on every call, so it MUST be present.
        let body = r#"{"sessionToken": "tok-managed", "role": "orchestrator"}"#;
        assert_eq!(
            resolve_session_token(body, true).as_deref(),
            Ok("tok-managed")
        );
    }

    #[test]
    fn resolve_session_token_managed_errors_when_token_absent() {
        // A managed register that returns no token must HARD-ERROR: proceeding
        // tokenless against a server that enforces the hash would fail every call.
        assert!(resolve_session_token(r#"{"role":"orchestrator"}"#, true).is_err());
        assert!(resolve_session_token(r#"{"sessionToken":""}"#, true).is_err());
    }

    #[test]
    fn resolve_session_token_unmanaged_tolerates_absent_token() {
        // UNMANAGED dev/CI path: the server returns no sessionToken (no hash stored)
        // and accepts tokenless calls. Tolerate it — store an empty token so
        // subsequent calls omit the key, matching require_session_token's contract.
        assert_eq!(
            resolve_session_token(r#"{"role":"orchestrator"}"#, false).as_deref(),
            Ok("")
        );
        assert_eq!(
            resolve_session_token(r#"{"sessionToken":""}"#, false).as_deref(),
            Ok("")
        );
    }

    #[test]
    fn resolve_session_token_unmanaged_still_honors_a_returned_token() {
        // If an unmanaged server DID return a token, use it (don't discard it).
        let body = r#"{"sessionToken": "tok-unmanaged"}"#;
        assert_eq!(
            resolve_session_token(body, false).as_deref(),
            Ok("tok-unmanaged")
        );
    }

    fn test_config(launch_token: &str) -> RmcpConfig {
        RmcpConfig {
            python: "python3".into(),
            root: "/srv/root".into(),
            projects_dir: "/srv/root/projects".into(),
            agent_id: "orchestrator-1".into(),
            role: "orchestrator".into(),
            model: "qwen".into(),
            launch_token: launch_token.into(),
            env: Vec::new(),
        }
    }

    #[test]
    fn register_args_carry_launch_token_under_server_field_name() {
        // The server's agent_register REQUIRES `launch_token` for a managed launch
        // (validate_launch_token_for_registration), so it must ride in the register
        // params under that exact field name with the configured value.
        let args = build_register_args(&test_config("tok-xyz-789"));
        assert_eq!(
            args.get("launch_token").and_then(|v| v.as_str()),
            Some("tok-xyz-789")
        );
        assert_eq!(
            args.get("agent_id").and_then(|v| v.as_str()),
            Some("orchestrator-1")
        );
        assert_eq!(
            args.get("role").and_then(|v| v.as_str()),
            Some("orchestrator")
        );
    }

    #[test]
    fn register_args_emit_blank_launch_token_for_unmanaged_path() {
        // The dev / unmanaged path passes no token; the field is still present (the
        // server only enforces it when a session hash exists).
        let args = build_register_args(&test_config(""));
        assert_eq!(args.get("launch_token").and_then(|v| v.as_str()), Some(""));
    }
}
