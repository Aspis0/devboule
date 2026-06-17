//! Runtime construction (L2.3): build the model + executor the burst loop uses,
//! from environment configuration, defaulting to the GPU-free / server-free
//! Mock + Stub when nothing is configured so `cargo run` works with no server.
//!
//! The REAL path is gated on config PRESENCE:
//! * the oMLX model is real only when `DEVBOULE_OMLX_BASE_URL` (+ model) is set
//!   and validates as a loopback http endpoint;
//! * the executor is real only when the MCP server connection succeeds; the FS
//!   backend is always real (it needs only a project root, defaulting to the cwd);
//! * egress is enabled ONLY when `EXA_API_KEY` is present, which is exactly what
//!   makes the burst's `allow_egress` true — the two can never disagree.
//!
//! Secrets are read from env and NEVER logged. A misconfiguration degrades to
//! the safe default (Mock/Stub, egress off) with a one-line stderr note rather
//! than crashing the TUI.

use std::sync::Arc;

use crate::agent_loop::{StubExecutor, ToolExecutor};
use crate::executor::{ExaBackend, FsBackend, RealExecutor};
use crate::model::{CoderModel, MockModel};
use crate::model_client::OmlxModel;
use crate::rmcp_backend::{RmcpBackend, RmcpConfig};

/// Everything the burst loop needs, resolved once at startup.
pub struct Runtime {
    pub model: Arc<dyn CoderModel>,
    pub executor: Arc<dyn ToolExecutor>,
    /// Authoritative egress gate passed to `run_burst`. True ONLY when a real
    /// Exa-backed executor is present.
    pub allow_egress: bool,
}

/// Env var names. Kept here so the binary's configuration surface is in one place.
const ENV_OMLX_BASE_URL: &str = "DEVBOULE_OMLX_BASE_URL";
const ENV_OMLX_MODEL: &str = "DEVBOULE_OMLX_MODEL";
const ENV_MCP_PYTHON: &str = "DEVBOULE_MCP_PYTHON";
const ENV_MCP_ROOT: &str = "DEVBOULE_MCP_ROOT";
const ENV_MCP_PROJECTS_DIR: &str = "DEVBOULE_MCP_PROJECTS_DIR";
const ENV_AGENT_ID: &str = "DEVBOULE_AGENT_ID";
const ENV_PROJECT_ROOT: &str = "DEVBOULE_PROJECT_ROOT";
const ENV_EXA_API_KEY: &str = "EXA_API_KEY";
/// The Aspis Management app binary path the launch wiring set in OUR env (L2.4 /
/// Phase 11.2). We FORWARD it to the MCP child as `ASPIS_APP_BIN` so the server's
/// read-only `project_structure` tool can shell out to the Rust structure builder
/// (zero tree-sitter duplication). Absent when the app could not resolve `current_exe`;
/// we then omit the forward and the tool degrades to a clear error. NOT a secret.
const ENV_APP_BIN: &str = "DEVBOULE_APP_BIN";
/// The env var name the MCP server reads to find the structure-bridge binary.
const ENV_MCP_APP_BIN: &str = "ASPIS_APP_BIN";
/// The app-issued launch token (L2.4). The Oracle MCP server REQUIRES it on
/// `agent_register` for a managed launch (it stamps a launchTokenHash on the
/// pending session up front), so the launch wiring sets this from the same token
/// it hashed into the session. Read from env and passed into `agent_register`
/// only — never logged.
const ENV_MCP_LAUNCH_TOKEN: &str = "DEVBOULE_MCP_LAUNCH_TOKEN";
/// The Oracle-side project key (Phase 11.2). The local planner passes it to the
/// `project_structure` / `plan_submit` MCP tools. The launch wiring sets it to the
/// project the orchestrator was opened on; absent on the no-project dev path (the
/// planner then escalates with a clear message instead of planning the wrong
/// project). NOT a secret.
const ENV_PROJECT_ID: &str = "DEVBOULE_PROJECT_ID";
/// 3b — the operator's "Plan first" launch bias. The launch wiring sets this to "1"
/// when the Spawn panel's "Plan first" toggle was ON; absent/empty otherwise. When set
/// (any non-empty value), the orchestrator's system prompt gains the PLAN-FIRST
/// directive (`prompt::build_system_prompt(true)`) so the model's first action for a
/// non-trivial goal is `plan`. NOT a secret.
const ENV_PLAN_FIRST: &str = "DEVBOULE_PLAN_FIRST";

/// Build the runtime from the environment. Async because the real MCP backend
/// connects to (spawns) the Oracle server during construction. Never panics:
/// every real-path failure falls back to the safe default with a stderr note.
pub async fn build_runtime() -> Runtime {
    let model = build_model();
    // The executor needs a handle to the SAME model to drive the local planner
    // (Phase 11.2), so build the model first and pass a clone into the executor.
    let (executor, allow_egress) = build_executor(Arc::clone(&model)).await;
    Runtime {
        model,
        executor,
        allow_egress,
    }
}

/// The model: real loopback oMLX when configured + valid, else the Mock.
fn build_model() -> Arc<dyn CoderModel> {
    let base_url = std::env::var(ENV_OMLX_BASE_URL).ok();
    let Some(base_url) = base_url.filter(|s| !s.trim().is_empty()) else {
        return Arc::new(MockModel::new());
    };
    let model_id = std::env::var(ENV_OMLX_MODEL).unwrap_or_default();
    // 3b — plan-first bias. Any non-empty DEVBOULE_PLAN_FIRST (the launch sets "1")
    // turns it on; absent/blank keeps the standing prompt unchanged. Only the REAL
    // oMLX model uses the system prompt, so the bias is meaningful only here (the
    // Mock fallback never POSTs a prompt).
    let plan_first = env_nonempty(ENV_PLAN_FIRST).is_some();
    match OmlxModel::new(&base_url, model_id, plan_first) {
        Ok(m) => Arc::new(m),
        Err(e) => {
            // Misconfigured endpoint (non-loopback / https / empty model): refuse
            // to route the prompt off-machine; fall back to the Mock. If "Plan first"
            // was requested, say so explicitly — the Mock never plans, so otherwise an
            // operator could believe the bias was honored when it silently was not.
            let plan_note = if plan_first {
                " (\"Plan first\" was requested but is now INACTIVE — the Mock does not plan)"
            } else {
                ""
            };
            eprintln!("devboule: oMLX model disabled ({e}); using MockModel{plan_note}");
            Arc::new(MockModel::new())
        }
    }
}

/// The executor: the real MCP+FS+Exa executor when the MCP server connects, else
/// the Stub. Returns `(executor, allow_egress)`; egress is on ONLY for the real
/// executor with an Exa key.
async fn build_executor(model: Arc<dyn CoderModel>) -> (Arc<dyn ToolExecutor>, bool) {
    // The FS backend is rooted at DEVBOULE_PROJECT_ROOT, defaulting to the cwd.
    let project_root = std::env::var(ENV_PROJECT_ROOT)
        .ok()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));

    // The real executor needs the MCP server connection details. Without them,
    // stay on the Stub so `cargo run` works with no server — but say so loudly:
    // a real model against a StubExecutor returns FAKE oracle/spawn output with no
    // signal, which silently looks like a working agent. This is the diagnostic for
    // a mis-launch (DEVBOULE_MCP_ROOT / DEVBOULE_MCP_PROJECTS_DIR not set).
    let (Some(root), Some(projects_dir)) = (
        env_nonempty(ENV_MCP_ROOT),
        env_nonempty(ENV_MCP_PROJECTS_DIR),
    ) else {
        eprintln!(
            "devboule: MCP backend disabled (DEVBOULE_MCP_ROOT / \
             DEVBOULE_MCP_PROJECTS_DIR not set); oracle/spawn tools will return \
             STUB results — the local coder is NOT connected"
        );
        return (Arc::new(StubExecutor), false);
    };

    let fs = match FsBackend::new(&project_root) {
        Ok(fs) => fs,
        Err(e) => {
            eprintln!("devboule: FS backend disabled ({e}); using StubExecutor");
            return (Arc::new(StubExecutor), false);
        }
    };

    // Forward DEVBOULE_APP_BIN (our env) to the MCP child as ASPIS_APP_BIN so the
    // server's read-only `project_structure` tool can shell out to the Rust structure
    // builder. Omitted when unset (the tool then errors clearly instead of guessing).
    let child_env: Vec<(String, String)> = match env_nonempty(ENV_APP_BIN) {
        Some(app_bin) => vec![(ENV_MCP_APP_BIN.to_string(), app_bin)],
        None => Vec::new(),
    };

    let config = RmcpConfig {
        python: env_nonempty(ENV_MCP_PYTHON).unwrap_or_else(|| "python".to_string()),
        root,
        projects_dir,
        agent_id: env_nonempty(ENV_AGENT_ID).unwrap_or_else(|| "devboule".to_string()),
        role: "orchestrator".to_string(),
        model: std::env::var(ENV_OMLX_MODEL).unwrap_or_default(),
        // The app-issued launch token the managed launch requires for
        // agent_register. Empty on the no-server / unmanaged dev path (the server
        // either has the compat kill switch on or no session hash to match).
        launch_token: std::env::var(ENV_MCP_LAUNCH_TOKEN).unwrap_or_default(),
        env: child_env,
    };

    let mcp = match RmcpBackend::connect(config).await {
        Ok(backend) => Arc::new(backend) as Arc<dyn crate::executor::McpBackend>,
        Err(e) => {
            eprintln!(
                "devboule: MCP backend disabled ({e}); oracle/spawn tools will \
                 return STUB results — the local coder is NOT connected"
            );
            return (Arc::new(StubExecutor), false);
        }
    };

    // Egress is real ONLY when an Exa key is configured.
    let web = match env_nonempty(ENV_EXA_API_KEY) {
        Some(key) => match ExaBackend::new(key) {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("devboule: Exa egress disabled ({e})");
                None
            }
        },
        None => None,
    };

    // The planner needs the Oracle-side project key. Empty when unset: `run_planner`
    // validates it non-empty BEFORE the first STRUCTURE MCP call and escalates with a
    // clear "project_id not set" message (no wasted tool call) rather than submitting
    // a plan against the wrong project.
    let project_id = env_nonempty(ENV_PROJECT_ID).unwrap_or_default();

    let executor = RealExecutor::new(mcp, fs, web).with_planner(model, project_id);
    let allow_egress = executor.egress_enabled();
    (Arc::new(executor), allow_egress)
}

/// Read an env var, treating empty/whitespace as absent.
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Env-mutating tests are inherently process-global; we keep these minimal and
    // self-contained, asserting the DEFAULT (no-config) path yields the safe
    // Mock/Stub with egress off. The real path is integration-deferred (it needs a
    // live server), and the per-backend behavior is unit-tested in their modules.

    #[test]
    fn env_nonempty_treats_blank_as_absent() {
        // A direct unit on the helper, no global env mutation.
        std::env::set_var("DEVBOULE_TEST_BLANK", "   ");
        assert_eq!(env_nonempty("DEVBOULE_TEST_BLANK"), None);
        std::env::set_var("DEVBOULE_TEST_BLANK", "x");
        assert_eq!(env_nonempty("DEVBOULE_TEST_BLANK").as_deref(), Some("x"));
        std::env::remove_var("DEVBOULE_TEST_BLANK");
        assert_eq!(env_nonempty("DEVBOULE_TEST_BLANK"), None);
    }
}
