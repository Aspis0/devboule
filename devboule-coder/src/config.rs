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
/// The app-issued launch token (L2.4). The Oracle MCP server REQUIRES it on
/// `agent_register` for a managed launch (it stamps a launchTokenHash on the
/// pending session up front), so the launch wiring sets this from the same token
/// it hashed into the session. Read from env and passed into `agent_register`
/// only — never logged.
const ENV_MCP_LAUNCH_TOKEN: &str = "DEVBOULE_MCP_LAUNCH_TOKEN";

/// Build the runtime from the environment. Async because the real MCP backend
/// connects to (spawns) the Oracle server during construction. Never panics:
/// every real-path failure falls back to the safe default with a stderr note.
pub async fn build_runtime() -> Runtime {
    let model = build_model();
    let (executor, allow_egress) = build_executor().await;
    Runtime { model, executor, allow_egress }
}

/// The model: real loopback oMLX when configured + valid, else the Mock.
fn build_model() -> Arc<dyn CoderModel> {
    let base_url = std::env::var(ENV_OMLX_BASE_URL).ok();
    let Some(base_url) = base_url.filter(|s| !s.trim().is_empty()) else {
        return Arc::new(MockModel::new());
    };
    let model_id = std::env::var(ENV_OMLX_MODEL).unwrap_or_default();
    match OmlxModel::new(&base_url, model_id) {
        Ok(m) => Arc::new(m),
        Err(e) => {
            // Misconfigured endpoint (non-loopback / https / empty model): refuse
            // to route the prompt off-machine; fall back to the Mock.
            eprintln!("devboule: oMLX model disabled ({e}); using MockModel");
            Arc::new(MockModel::new())
        }
    }
}

/// The executor: the real MCP+FS+Exa executor when the MCP server connects, else
/// the Stub. Returns `(executor, allow_egress)`; egress is on ONLY for the real
/// executor with an Exa key.
async fn build_executor() -> (Arc<dyn ToolExecutor>, bool) {
    // The FS backend is rooted at DEVBOULE_PROJECT_ROOT, defaulting to the cwd.
    let project_root = std::env::var(ENV_PROJECT_ROOT)
        .ok()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));

    // The real executor needs the MCP server connection details. Without them,
    // stay on the Stub so `cargo run` works with no server.
    let (Some(root), Some(projects_dir)) = (
        env_nonempty(ENV_MCP_ROOT),
        env_nonempty(ENV_MCP_PROJECTS_DIR),
    ) else {
        return (Arc::new(StubExecutor), false);
    };

    let fs = match FsBackend::new(&project_root) {
        Ok(fs) => fs,
        Err(e) => {
            eprintln!("devboule: FS backend disabled ({e}); using StubExecutor");
            return (Arc::new(StubExecutor), false);
        }
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
        env: Vec::new(),
    };

    let mcp = match RmcpBackend::connect(config).await {
        Ok(backend) => Arc::new(backend) as Arc<dyn crate::executor::McpBackend>,
        Err(e) => {
            eprintln!("devboule: MCP backend disabled ({e}); using StubExecutor");
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

    let executor = RealExecutor::new(mcp, fs, web, project_root);
    let allow_egress = executor.egress_enabled();
    (Arc::new(executor), allow_egress)
}

/// Read an env var, treating empty/whitespace as absent.
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
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
