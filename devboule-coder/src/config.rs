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
use crate::model_client::{CloudModel, OmlxModel};
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
/// CLOUD model env set (the OPT-IN, consent-gated backend). When `DEVBOULE_CLOUD_BASE_URL`
/// is present, `build_model` builds the [`CloudModel`] (https + bearer) INSTEAD of the
/// loopback oMLX model — the host UI only sets these when the operator picked Cloud mode and
/// consented to prompts leaving the machine. The base URL + model are non-secret; the API key
/// is a SECRET injected via the per-launch process env (off argv) and is NEVER logged.
const ENV_CLOUD_BASE_URL: &str = "DEVBOULE_CLOUD_BASE_URL";
const ENV_CLOUD_MODEL: &str = "DEVBOULE_CLOUD_MODEL";
const ENV_CLOUD_API_KEY: &str = "DEVBOULE_CLOUD_API_KEY";
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
/// Phase B — the merged, ENABLED user MCP servers the launch wiring injects for the
/// devboule-coder ORCHESTRATOR (a JSON array of `{name,command,args,env}`). Read ONLY
/// here (the local MAIN coder). The MINI coder is a separate binary that NEVER sets or
/// reads this var (design §6 mini-exclusion). Absent/empty ⇒ the plain Oracle path.
/// NOT a secret (no key), but bounded in size by the launch wiring + parser.
const ENV_USER_MCP_SERVERS: &str = "DEVBOULE_USER_MCP_SERVERS";

/// Build the runtime from the environment. Async because the real MCP backend
/// connects to (spawns) the Oracle server — and any user MCP servers — during
/// construction. Never panics: every real-path failure falls back to the safe
/// default with a stderr note.
///
/// ORDER (Phase B): the MCP backend is connected FIRST, because connecting the user
/// MCP servers yields their tool catalog, which the MODEL needs for its system-prompt
/// external-tools section (B.3). So: connect MCP → get the catalog → build the model
/// (with the catalog) → finish the executor (which needs the model for the planner).
/// When no user servers are configured the catalog is empty and the model's prompt is
/// byte-identical to the pre-B build.
pub async fn build_runtime() -> Runtime {
    // 1. Connect the MCP backend (Oracle + any user servers). `mcp` is `None` when the
    //    server connection details are absent / the connection failed (the Stub path).
    let (mcp, user_mcp_tools) = connect_mcp().await;
    // 2. Build the model with the user-MCP tool catalog (empty ⇒ byte-identical prompt).
    let model = build_model(user_mcp_tools);
    // 3. Finish the executor (it needs the SAME model for the local planner).
    let (executor, allow_egress) = build_executor(Arc::clone(&model), mcp).await;
    Runtime {
        model,
        executor,
        allow_egress,
    }
}

/// The model. PRECEDENCE (a discriminated-backend extension):
///   1. CLOUD (opt-in) — when `DEVBOULE_CLOUD_BASE_URL` is present, build the
///      [`CloudModel`] (https + bearer). This is the ONLY path that sends the prompt
///      off-machine; the host UI sets these vars only after the operator chose Cloud
///      mode and consented.
///   2. LOOPBACK oMLX — else when `DEVBOULE_OMLX_BASE_URL` is present, build the
///      private loopback [`OmlxModel`] (UNCHANGED — byte-identical to before).
///   3. Mock — else the GPU-free / server-free default.
///
/// 3b — plan-first bias. Any non-empty `DEVBOULE_PLAN_FIRST` (the launch sets "1")
/// turns it on; absent/blank keeps the standing prompt unchanged. It applies to BOTH
/// real models (each POSTs the system prompt); the Mock never plans.
fn build_model(user_mcp_tools: Vec<crate::prompt::UserMcpServerTools>) -> Arc<dyn CoderModel> {
    let plan_first = env_nonempty(ENV_PLAN_FIRST).is_some();

    // 1. CLOUD first (opt-in). The PRESENCE of the cloud base URL selects this path;
    // a misconfiguration (bad https/host, empty model, missing key) fails LOUD to the
    // Mock rather than silently routing an unauthenticated request off-machine.
    if let Some(cloud_base) = env_nonempty(ENV_CLOUD_BASE_URL) {
        let cloud_model = std::env::var(ENV_CLOUD_MODEL).unwrap_or_default();
        // The key comes ONLY from env and is NEVER logged (errors below print no key).
        let cloud_key = std::env::var(ENV_CLOUD_API_KEY).unwrap_or_default();
        match CloudModel::new(&cloud_base, cloud_model, cloud_key, plan_first) {
            // B.3: attach the user-MCP tool catalog (empty ⇒ prompt byte-identical).
            Ok(m) => return Arc::new(m.with_user_mcp_tools(user_mcp_tools)),
            Err(e) => {
                let plan_note = if plan_first {
                    " (\"Plan first\" was requested but is now INACTIVE — the Mock does not plan)"
                } else {
                    ""
                };
                // `e` is a validation message (scheme/host/empty-field); it NEVER
                // contains the key value.
                eprintln!("devboule: Cloud model disabled ({e}); using MockModel{plan_note}");
                return Arc::new(MockModel::new());
            }
        }
    }

    // 2. LOOPBACK oMLX (the private default) — UNCHANGED.
    let base_url = std::env::var(ENV_OMLX_BASE_URL).ok();
    let Some(base_url) = base_url.filter(|s| !s.trim().is_empty()) else {
        return Arc::new(MockModel::new());
    };
    let model_id = std::env::var(ENV_OMLX_MODEL).unwrap_or_default();
    match OmlxModel::new(&base_url, model_id, plan_first) {
        // B.3: attach the user-MCP tool catalog (empty ⇒ prompt byte-identical).
        Ok(m) => Arc::new(m.with_user_mcp_tools(user_mcp_tools)),
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

/// Connect the MCP backend(s) (Phase B). When the Oracle connection details are
/// present and the handshake succeeds, returns the backend; when ALSO user MCP
/// servers are configured (`DEVBOULE_USER_MCP_SERVERS`), the Oracle is wrapped in a
/// [`MultiMcpBackend`] (Oracle FIRST + the connected user servers) and the user
/// servers' tool catalog is returned for the system prompt (B.3). Returns
/// `(None, [])` when the server details are absent or the Oracle handshake fails —
/// the executor then falls back to the Stub. NEVER panics.
///
/// MINI-EXCLUSION (design §6): this is the `devboule-coder` (local MAIN coder)
/// runtime. `DEVBOULE_USER_MCP_SERVERS` is read ONLY here; the mini coder is a
/// separate binary path (`src-tauri`) that never sets nor reads this var.
async fn connect_mcp() -> (
    Option<Arc<dyn crate::executor::McpBackend>>,
    Vec<crate::prompt::UserMcpServerTools>,
) {
    // The real backend needs the Oracle server connection details. Without them,
    // there is no backend (the executor stays on the Stub) — say so loudly: a real
    // model against a StubExecutor returns FAKE oracle/spawn output with no signal,
    // which silently looks like a working agent. This is the diagnostic for a
    // mis-launch (DEVBOULE_MCP_ROOT / DEVBOULE_MCP_PROJECTS_DIR not set).
    let (Some(root), Some(projects_dir)) = (
        env_nonempty(ENV_MCP_ROOT),
        env_nonempty(ENV_MCP_PROJECTS_DIR),
    ) else {
        eprintln!(
            "devboule: MCP backend disabled (DEVBOULE_MCP_ROOT / \
             DEVBOULE_MCP_PROJECTS_DIR not set); oracle/spawn tools will return \
             STUB results — the local coder is NOT connected"
        );
        return (None, Vec::new());
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
        model: resolve_register_model(),
        // The app-issued launch token the managed launch requires for
        // agent_register. Empty on the no-server / unmanaged dev path (the server
        // either has the compat kill switch on or no session hash to match).
        launch_token: std::env::var(ENV_MCP_LAUNCH_TOKEN).unwrap_or_default(),
        env: child_env,
    };

    let oracle: Arc<dyn crate::executor::McpBackend> = match RmcpBackend::connect(config).await {
        Ok(backend) => Arc::new(backend),
        Err(e) => {
            eprintln!(
                "devboule: MCP backend disabled ({e}); oracle/spawn tools will \
                 return STUB results — the local coder is NOT connected"
            );
            return (None, Vec::new());
        }
    };

    // Phase B: the merged ENABLED user MCP servers, passed by the launch wiring as a
    // JSON array. Absent / empty / unparseable ⇒ the plain Oracle path, UNCHANGED
    // (zero regression). When present + non-empty, wrap the Oracle in a
    // MultiMcpBackend (Oracle FIRST + the connected user servers).
    let specs = parse_user_mcp_servers();
    if specs.is_empty() {
        return (Some(oracle), Vec::new());
    }

    let (multi, catalog) = crate::multi_mcp::MultiMcpBackend::connect(oracle, specs).await;
    // Map the connect catalog (plain tuples) into the prompt's tool-section type.
    let tools = catalog
        .into_iter()
        .map(|(name, tools)| crate::prompt::UserMcpServerTools { name, tools })
        .collect();
    (Some(Arc::new(multi)), tools)
}

/// The executor: the real MCP+FS+Exa executor when an MCP backend is present, else
/// the Stub. Takes the PRE-CONNECTED `mcp` (from [`connect_mcp`], so the user-server
/// tool catalog could be fetched before the model was built). Returns `(executor,
/// allow_egress)`; egress is on ONLY for the real executor with an Exa key.
async fn build_executor(
    model: Arc<dyn CoderModel>,
    mcp: Option<Arc<dyn crate::executor::McpBackend>>,
) -> (Arc<dyn ToolExecutor>, bool) {
    // The FS backend is rooted at DEVBOULE_PROJECT_ROOT, defaulting to the cwd.
    let project_root = std::env::var(ENV_PROJECT_ROOT)
        .ok()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));

    // No MCP backend (server details absent / handshake failed) ⇒ the Stub, as before.
    let Some(mcp) = mcp else {
        return (Arc::new(StubExecutor), false);
    };

    let fs = match FsBackend::new(&project_root) {
        Ok(fs) => fs,
        Err(e) => {
            eprintln!("devboule: FS backend disabled ({e}); using StubExecutor");
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

/// Max user MCP servers the local coder will wire (design defensive bound). The
/// launch wiring already injects only the enabled, merged set; this caps a runaway /
/// hand-edited list so a hostile config cannot spawn an unbounded number of children.
/// MUST stay in lock-step with the src-tauri serializer cap `MAX_ORCHESTRATOR_SERVERS`
/// in `user_mcp_config.rs` (the two crates cannot share a const — keep both at 20).
const MAX_USER_MCP_SERVERS: usize = 20;

/// Reserved name PREFIXES a user server may not start with (case-insensitive). MIRRORS
/// the src-tauri `RESERVED_NAME_PREFIXES` guard (design §5.3). The binary RE-CHECKS this
/// because it must NOT trust `DEVBOULE_USER_MCP_SERVERS` blindly: a crafted/hand-edited
/// env could carry an entry the src-tauri config layer would have rejected, and a user
/// server registered under an Oracle/Devboule name could shadow Oracle routing. `aspis`
/// covers the `aspis-management` Oracle server key.
const RESERVED_NAME_PREFIXES: &[&str] = &["oracle", "devboule", "aspis"];

/// Exact Oracle MCP tool names a user server may not take (MIRRORS the src-tauri
/// `ORACLE_TOOL_NAMES`, kept in sync with `oracle/server/aspis_mcp.py` `TOOLS`). The
/// `oracle_*`/`project_*` entries are also covered by [`RESERVED_NAME_PREFIXES`], but the
/// full list makes the binary's guard explicit so a user server can never shadow an Oracle
/// tool name even if the env was hand-crafted to bypass the src-tauri-side check.
const ORACLE_TOOL_NAMES: &[&str] = &[
    "agent_rules",
    "agent_state",
    "agent_register",
    "agent_heartbeat",
    "spawn_mini_coder",
    "steer_mini_coder",
    "mini_coder_result",
    "visual_check",
    "request_git_push",
    "plan_submit",
    "plan_status",
    "ask_user",
    "project_list",
    "project_get",
    "project_next_task",
    "project_claim_task",
    "project_update_status",
    "project_append_note",
    "project_create_followup",
    "project_create_plan_tasks",
    "provider_credentials_status",
    "cloudflare_list_workers",
    "cloudflare_rotate_worker_secret",
    "scaleway_list_resources",
    "scaleway_resource_action",
    "oracle_ask",
    "oracle_context",
    "project_structure",
    "censor_findings",
    "censor_dispose",
];

/// Re-validate a TRIMMED user-server name from the env (defense in depth, mirrors the
/// src-tauri `validate_name`). Rejects: empty, the safe-charset violation
/// (`[A-Za-z0-9_-]` only), any [`RESERVED_NAME_PREFIXES`], and any exact
/// [`ORACLE_TOOL_NAMES`]. A rejected entry is SKIPPED by the caller (fail-open, logged) —
/// the binary must not trust its env, so a crafted Oracle/reserved name never registers a
/// user backend that could shadow Oracle dispatch.
fn user_server_name_ok(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("empty name".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("name has characters outside [A-Za-z0-9_-]".to_string());
    }
    let lower = name.to_ascii_lowercase();
    if let Some(prefix) = RESERVED_NAME_PREFIXES
        .iter()
        .find(|p| lower.starts_with(**p))
    {
        return Err(format!("reserved name prefix '{prefix}'"));
    }
    if ORACLE_TOOL_NAMES.iter().any(|t| t.eq_ignore_ascii_case(name)) {
        return Err("collides with a reserved Oracle tool name".to_string());
    }
    Ok(())
}

/// Parse `DEVBOULE_USER_MCP_SERVERS` (Phase B): a JSON ARRAY of
/// `{name, command, args?, env?}` objects (the merged, ENABLED user servers the
/// launch wiring injected). Returns the launch specs for [`connect_mcp`]. FAIL-OPEN:
/// absent / blank / non-array / unparseable ⇒ EMPTY (the plain Oracle path), so a bad
/// value never blocks a launch. Bounded by [`MAX_USER_MCP_SERVERS`] (servers beyond the
/// cap are dropped with a warning, never silently). `name` and `command` are TRIMMED at
/// storage (consistent exact-match routing). A server whose `command` is empty after trim,
/// or whose `name` fails [`user_server_name_ok`] (charset / reserved / Oracle-tool name),
/// is SKIPPED with a warning — defense in depth: the src-tauri config layer already
/// validates, but this binary must not trust its env blindly.
fn parse_user_mcp_servers() -> Vec<crate::multi_mcp::UserServerSpec> {
    #[derive(serde::Deserialize)]
    struct Entry {
        name: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
        // The on-the-wire shape is a JSON object (camelCase `env`); accept it as a
        // map and flatten to the ordered pairs the spawn path wants. Unknown fields
        // (e.g. `transport`/`enabled` from the src-tauri record) are IGNORED.
        #[serde(default)]
        env: std::collections::BTreeMap<String, String>,
    }

    let Some(raw) = env_nonempty(ENV_USER_MCP_SERVERS) else {
        return Vec::new();
    };
    let entries: Vec<Entry> = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            // Non-secret (server names/commands), but the value can be large; log only
            // the parse error, not the payload.
            eprintln!("devboule: DEVBOULE_USER_MCP_SERVERS is not a valid JSON array ({e}); ignoring");
            return Vec::new();
        }
    };

    // Pre-size to the CAP at most: a hand-edited env with thousands of entries must not
    // pre-allocate a huge Vec (we only ever keep MAX_USER_MCP_SERVERS).
    let mut specs: Vec<crate::multi_mcp::UserServerSpec> =
        Vec::with_capacity(entries.len().min(MAX_USER_MCP_SERVERS));
    for e in entries {
        // Trim at storage so a name like `" my-db "` routes by its exact inner value.
        let name = e.name.trim().to_string();
        let command = e.command.trim().to_string();
        if command.is_empty() {
            eprintln!("devboule: user MCP server '{name}' has an empty command; skipping");
            continue;
        }
        // RE-GUARD the name (charset / reserved prefix / Oracle tool name). The binary must
        // not trust its env: a crafted entry naming e.g. `oracle_ask` would otherwise
        // register a user backend under an Oracle name. Skip + log a rejected entry.
        if let Err(reason) = user_server_name_ok(&name) {
            eprintln!("devboule: user MCP server name '{name}' rejected ({reason}); skipping");
            continue;
        }
        if specs.len() == MAX_USER_MCP_SERVERS {
            // Beyond the cap: log ONCE and stop (the launch wiring already bounded this; a
            // hand-edited/oversized env is capped here, never silently spawning unbounded
            // children). Remaining entries are dropped.
            eprintln!(
                "devboule: more than {MAX_USER_MCP_SERVERS} user MCP servers configured; \
                 wiring only the first {MAX_USER_MCP_SERVERS} and dropping the rest"
            );
            break;
        }
        specs.push(crate::multi_mcp::UserServerSpec {
            name,
            command,
            args: e.args,
            env: e.env.into_iter().collect(),
        });
    }
    specs
}

/// Read an env var, treating empty/whitespace as absent.
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The model id declared to the Oracle's `agent_register` (for the dashboard). Prefer the
/// loopback oMLX model, but FALL BACK to the cloud model id: in Cloud mode `DEVBOULE_OMLX_MODEL`
/// is unset, so without this fallback an EMPTY model string would be registered. Empty when
/// neither is set (the Mock/no-config path), matching the prior behavior for non-cloud.
fn resolve_register_model() -> String {
    env_nonempty(ENV_OMLX_MODEL)
        .or_else(|| env_nonempty(ENV_CLOUD_MODEL))
        .unwrap_or_default()
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

    #[test]
    fn user_server_name_ok_rejects_reserved_and_oracle_names() {
        // FIX 4: the binary must NOT trust its env — re-reject reserved prefixes, exact
        // Oracle tool names, and the unsafe charset, mirroring the src-tauri guard.
        for bad in [
            "oracle", "Oracle", "devboule", "devboule-x", "aspis", "aspis-management",
            "oracle_ask", "spawn_mini_coder", "plan_submit", "project_get", "censor_dispose",
        ] {
            assert!(
                user_server_name_ok(bad).is_err(),
                "reserved/Oracle name '{bad}' must be rejected"
            );
        }
        // Charset + empty.
        for bad in ["", "my.db", "my db", "a=b", "weird\"name", "tab\tname"] {
            assert!(user_server_name_ok(bad).is_err(), "name '{bad}' must be rejected");
        }
        // Conventional safe names are accepted (and a name only PARTIALLY matching a
        // reserved word but not as a PREFIX is fine).
        for ok in ["my-db", "my_db_2", "ci", "store-oracle"] {
            assert!(user_server_name_ok(ok).is_ok(), "name '{ok}' must be accepted");
        }
    }

    #[test]
    fn parse_user_mcp_servers_trims_skips_reserved_and_caps() {
        // FIX 4 + FIX 5 via the env entry point. Env is process-global; this test OWNS and
        // restores the exact var it asserts on. A crafted array carries an Oracle-named
        // entry (must be skipped), an untrimmed good entry (must be trimmed), an
        // empty-command entry (skipped), and MORE than the cap of valid entries (capped).
        let prev = std::env::var(ENV_USER_MCP_SERVERS).ok();

        // 1) Oracle-named + untrimmed + empty-command entries.
        let crafted = serde_json::json!([
            {"name": "oracle_ask", "command": "python"},     // reserved Oracle name -> skip
            {"name": "  my-db  ", "command": "  python3  "},  // untrimmed -> trimmed
            {"name": "blank", "command": "   "},              // empty command -> skip
        ])
        .to_string();
        std::env::set_var(ENV_USER_MCP_SERVERS, &crafted);
        let specs = parse_user_mcp_servers();
        assert_eq!(specs.len(), 1, "only the one valid server survives");
        assert_eq!(specs[0].name, "my-db", "name is trimmed at storage");
        assert_eq!(specs[0].command, "python3", "command is trimmed at storage");

        // 2) Cap: more than MAX_USER_MCP_SERVERS valid entries -> capped to the limit.
        let many: Vec<serde_json::Value> = (0..(MAX_USER_MCP_SERVERS + 7))
            .map(|i| serde_json::json!({"name": format!("srv-{i}"), "command": "python"}))
            .collect();
        std::env::set_var(ENV_USER_MCP_SERVERS, serde_json::Value::from(many).to_string());
        let capped = parse_user_mcp_servers();
        assert_eq!(
            capped.len(),
            MAX_USER_MCP_SERVERS,
            "valid servers beyond the cap are dropped"
        );

        // Restore.
        match prev {
            Some(v) => std::env::set_var(ENV_USER_MCP_SERVERS, v),
            None => std::env::remove_var(ENV_USER_MCP_SERVERS),
        }
    }

    #[test]
    fn resolve_register_model_falls_back_to_cloud_model() {
        // FIX 5: in Cloud mode DEVBOULE_OMLX_MODEL is unset, so the register model must fall
        // back to DEVBOULE_CLOUD_MODEL instead of registering an empty string. Env is
        // process-global; this test owns BOTH vars and restores them, so it is serialized by
        // setting/removing the exact keys it asserts on (no shared key with other tests).
        let prev_omlx = std::env::var(ENV_OMLX_MODEL).ok();
        let prev_cloud = std::env::var(ENV_CLOUD_MODEL).ok();

        // Only the cloud model is set -> it is the registered model.
        std::env::remove_var(ENV_OMLX_MODEL);
        std::env::set_var(ENV_CLOUD_MODEL, "gpt-cloud-4");
        assert_eq!(resolve_register_model(), "gpt-cloud-4");

        // oMLX wins when both are set (loopback default takes precedence).
        std::env::set_var(ENV_OMLX_MODEL, "local-mlx");
        assert_eq!(resolve_register_model(), "local-mlx");

        // Neither set -> empty (the Mock/no-config path, unchanged for non-cloud).
        std::env::remove_var(ENV_OMLX_MODEL);
        std::env::remove_var(ENV_CLOUD_MODEL);
        assert_eq!(resolve_register_model(), "");

        // Restore.
        match prev_omlx {
            Some(v) => std::env::set_var(ENV_OMLX_MODEL, v),
            None => std::env::remove_var(ENV_OMLX_MODEL),
        }
        match prev_cloud {
            Some(v) => std::env::set_var(ENV_CLOUD_MODEL, v),
            None => std::env::remove_var(ENV_CLOUD_MODEL),
        }
    }
}
