//! Slice 5b: generate a per-project Claude Code `settings.json` (as serializable structs)
//! from the project's sandbox knobs. Claude's PROCESS is OS-sandboxed by the caller's
//! broker (cloud_duplex launches it via spawn_sandboxed_with_stdin — AppContainer on
//! Windows, is_enforced()==true since C6); on top of that we drive its native permission
//! system + a PreToolUse hook that bridges every tool call to OUR consent UI over the
//! `.aspis-agents.json` file-bridge.
//!
//! ⚠️ Claude settings.json has NO `sandbox.filesystem`/`sandbox.network` block (verified —
//! not in the official schema). Filesystem/network confinement is enforced by the hook +
//! `permissions.deny` rules only. The keys here are the LITERAL Claude settings keys
//! (`permissions`/`deny`/`hooks`/`PreToolUse`/`type`/`command`/`timeout`) — not camelCase.
//!
//! F36: product launches set `CLAUDE_CONFIG_DIR` to an app-owned directory so the CLI never
//! inherits the owner's personal `~/.claude` (CLAUDE.md, skills, allowlists).
//! F33 residual: product MCP tools (`mcp__devboule__*`) are always on `permissions.allow`
//! because headless `acceptEdits` does not cover MCP, and personal config can still deny.

use crate::backend::broker::SandboxMode;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClaudeSettings {
    #[serde(skip_serializing_if = "ClaudePermissions::is_empty")]
    pub permissions: ClaudePermissions,
    #[serde(skip_serializing_if = "ClaudeHooks::is_empty")]
    pub hooks: ClaudeHooks,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ClaudePermissions {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ask: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
}

impl ClaudePermissions {
    fn is_empty(&self) -> bool {
        self.deny.is_empty() && self.ask.is_empty() && self.allow.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ClaudeHooks {
    #[serde(rename = "PreToolUse", skip_serializing_if = "Vec::is_empty")]
    pub pre_tool_use: Vec<HookMatcher>,
}

impl ClaudeHooks {
    fn is_empty(&self) -> bool {
        self.pre_tool_use.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HookMatcher {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    pub hooks: Vec<HookCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HookCommand {
    #[serde(rename = "type")]
    pub kind: String,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

/// Product MCP server name used in `.mcp.json` / `--mcp-config` (see `cli_agents::MCP_KEY`).
pub const PRODUCT_MCP_SERVER: &str = "devboule";

/// Permission allow rules for product-owned MCP tools (F33 residual).
/// Headless `acceptEdits` does **not** cover MCP; without these rules every
/// `mcp__devboule__*` call is denied with "you haven't granted it yet".
pub fn product_mcp_allow_rules() -> Vec<String> {
    vec![format!("mcp__{PRODUCT_MCP_SERVER}__*")]
}

/// When the PreToolUse consent hook is **not** registered, headless duplex still
/// needs Read/Edit tools for write-capable product agents. Bash stays out so the
/// model cannot silently run arbitrary shell without a prompt/hook.
pub fn headless_file_tool_allow_rules() -> Vec<String> {
    [
        "Read",
        "Write",
        "Edit",
        "MultiEdit",
        "Glob",
        "Grep",
        "LS",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// F36: app-owned Claude config directory for one agent launch.
/// Never under `$HOME/.claude` — product agents must not inherit owner CLAUDE.md/skills.
pub fn claude_product_config_dir(base: &Path, agent_id: &str) -> PathBuf {
    let safe: String = agent_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.is_empty() {
        "agent".to_string()
    } else {
        safe
    };
    base.join("claude-agent-config").join(safe)
}

/// Env pair that isolates the Claude CLI from the owner's personal config (F36).
pub fn claude_config_dir_env(config_dir: &Path) -> (String, String) {
    (
        "CLAUDE_CONFIG_DIR".to_string(),
        config_dir.to_string_lossy().into_owned(),
    )
}

/// Pure: env pair for a Claude setup-token (never logs the value).
pub fn claude_oauth_token_env_pair(token: &str) -> (String, String) {
    (
        "CLAUDE_CODE_OAUTH_TOKEN".to_string(),
        token.to_string(),
    )
}

/// If a setup-token is in the vault, return the env pair to inject into a Claude child.
/// Empty/missing/error → None (no injection; CLI may still use credentials.json seed).
pub fn claude_oauth_token_env_from_vault() -> Option<(String, String)> {
    match crate::backend::vault::read_claude_oauth_token() {
        Ok(Some(t)) if !t.trim().is_empty() => Some(claude_oauth_token_env_pair(&t)),
        _ => None,
    }
}

/// Home directory via env (same idiom as `saved_workflows` / `pi_extensions` — no extra crate).
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Auth env vars the Claude CLI understands (`CLAUDE_CODE_OAUTH_TOKEN`, `ANTHROPIC_API_KEY`).
///
/// Child processes already inherit the app's process env, so call sites need not wire
/// this unless they clear/rebuild the child env map. Exported for that explicit future
/// use; not required for the F46 credential-file seed path.
pub(crate) fn claude_auth_env_passthrough() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for key in ["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY"] {
        if let Ok(v) = std::env::var(key) {
            if !v.is_empty() {
                out.push((key.to_string(), v));
            }
        }
    }
    out
}

/// Seed `$HOME/.claude/.credentials.json` into an isolated product config dir when missing
/// or stale. Never logs file contents. No-op when source is absent or home is unknown.
fn seed_owner_credentials(config_dir: &Path) -> std::io::Result<()> {
    let Some(home) = home_dir() else {
        return Ok(());
    };
    let src = home.join(".claude").join(".credentials.json");
    if !src.is_file() {
        return Ok(());
    }
    let dest = config_dir.join(".credentials.json");
    if dest.is_file() {
        let src_m = match src.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => return Ok(()),
        };
        let dest_m = match dest.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => return Ok(()),
        };
        // Dest present and not older than source → leave it alone.
        if dest_m >= src_m {
            return Ok(());
        }
    }
    std::fs::copy(&src, &dest)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Write JSON atomically to `path` with mode 0600 (unix). Never logs contents.
fn write_json_atomic_0600(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(
        ".{}.tmp.{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("claude.json"),
        std::process::id()
    ));
    std::fs::write(&tmp, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    {
        // rename may not preserve mode across filesystems — re-assert 0600 on dest.
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// F46-auto: reuse the machine's Claude CLI login *state* in the isolated config dir.
///
/// The keychain credential item is already shared at the OS level; only the login STATE
/// (who is logged in) was missing from empty isolated dirs — that lives in
/// `$HOME/.claude.json` (home root, **not** inside `~/.claude/`). When source has
/// `oauthAccount` and dest lacks it, merge `oauthAccount` (+ `userID` if present) into
/// `<config_dir>/.claude.json` without overwriting an existing logged-in dest or other
/// dest fields. Silent no-op if source is missing/unparseable. NEVER logs field values.
///
/// The setup-token env path (`CLAUDE_CODE_OAUTH_TOKEN` via vault) remains the explicit
/// override for machines without a local interactive login.
fn seed_owner_login_state(config_dir: &Path) -> std::io::Result<()> {
    let Some(home) = home_dir() else {
        return Ok(());
    };
    // Owner login state: ~/.claude.json at HOME ROOT (not ~/.claude/.claude.json).
    let src = home.join(".claude.json");
    if !src.is_file() {
        return Ok(());
    }
    let src_raw = match std::fs::read_to_string(&src) {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };
    let src_json: serde_json::Value = match serde_json::from_str(&src_raw) {
        Ok(v) => v,
        Err(_) => return Ok(()), // unparseable → silent no-op
    };
    let Some(oauth_account) = src_json.get("oauthAccount").cloned() else {
        return Ok(());
    };
    // Require a real object (or at least present non-null) — null/empty skips.
    if oauth_account.is_null() {
        return Ok(());
    }

    let dest = config_dir.join(".claude.json");
    let mut dest_json = if dest.is_file() {
        match std::fs::read_to_string(&dest)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        {
            Some(serde_json::Value::Object(map)) => serde_json::Value::Object(map),
            Some(_) | None => {
                // Unparseable dest: do not clobber — leave alone.
                return Ok(());
            }
        }
    } else {
        serde_json::json!({})
    };

    let dest_obj = match dest_json.as_object_mut() {
        Some(o) => o,
        None => return Ok(()),
    };
    // Never overwrite an existing logged-in state.
    if dest_obj.contains_key("oauthAccount") {
        return Ok(());
    }
    dest_obj.insert("oauthAccount".to_string(), oauth_account);
    if let Some(user_id) = src_json.get("userID").cloned() {
        if !user_id.is_null() && !dest_obj.contains_key("userID") {
            dest_obj.insert("userID".to_string(), user_id);
        }
    }
    write_json_atomic_0600(&dest, &dest_json)
}

/// Create the isolated config dir (idempotent). Returns the path for `CLAUDE_CONFIG_DIR`.
///
/// F46: isolation targets CLAUDE.md / skills / permissions — **not** authentication.
/// Credentials are deliberately shared from the owner install by seeding
/// `~/.claude/.credentials.json` into the isolated dir when missing or stale —
/// **unless** a vault setup-token is present (see `vault_token_present`).
///
/// F46-auto: also reuses the machine login *state* from `$HOME/.claude.json`
/// (`oauthAccount` / `userID`) into the isolated dir's `.claude.json` when dest is not
/// already logged in — the keychain credential item is already shared; only the login
/// state was missing. The setup-token env path remains the explicit override.
///
/// F65: when `vault_token_present` is true, the env token (`CLAUDE_CODE_OAUTH_TOKEN`) is
/// the single auth source. Stale per-agent `.credentials.json` files (seeded while auth
/// was broken) shadow the env token and cause `401 Invalid bearer token` — so delete that
/// file and skip `seed_owner_credentials`. Login-state seed still runs (display only).
///
/// macOS keychain-stored credentials cannot be fully synthesized if neither file nor
/// token is available; if still logged-out the CLI will report that and the app surface
/// should show the auth error (out of scope here).
pub fn ensure_claude_product_config_dir(
    base: &Path,
    agent_id: &str,
    vault_token_present: bool,
) -> std::io::Result<PathBuf> {
    let dir = claude_product_config_dir(base, agent_id);
    std::fs::create_dir_all(&dir)?;
    if vault_token_present {
        // Env token is sole auth — drop any stale credentials seed so the CLI cannot
        // prefer a broken file over CLAUDE_CODE_OAUTH_TOKEN. Best-effort; ignore NotFound.
        // F65/A-3: surface non-NotFound errors (EACCES/EPERM) so stuck stale-creds is
        // diagnosable — never log file contents or token values, only the config dir.
        let creds = dir.join(".credentials.json");
        match std::fs::remove_file(&creds) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!(
                    "cloud claude: could not remove stale .credentials.json under {} ({e}); \
                     vault token may be shadowed (F65 best-effort)",
                    dir.display()
                );
            }
        }
    } else {
        seed_owner_credentials(&dir)?;
    }
    seed_owner_login_state(&dir)?;
    Ok(dir)
}

/// Build the per-project Claude settings from the sandbox knobs.
/// - `helper_path`: absolute path to our compiled PreToolUse hook binary.
/// - `hook_timeout_secs`: the hook `timeout` (must exceed our consent poll cap so the CLI
///   doesn't kill the hook before the human answers).
///
/// `mode` does not change the settings STRUCTURE (the hook is registered for every mode,
/// including Unattended where the helper answers `deny` without prompting — a runtime
/// decision, not a config one). It is part of the signature for future per-mode deny tuning.
///
/// F33 residual: always emits `permissions.allow` for product MCP tools so headless
/// register/plan/task/spawn is not stuck on interactive MCP grants.
pub fn build_claude_settings(
    mode: SandboxMode,
    net_enabled: bool,
    helper_path: Option<&str>,
    hook_timeout_secs: u64,
) -> ClaudeSettings {
    let _ = mode;

    let deny = if !net_enabled {
        vec![
            "Bash(curl *)".to_string(),
            "Bash(wget *)".to_string(),
            "Bash(nc *)".to_string(),
            "WebFetch".to_string(),
            "WebSearch".to_string(),
        ]
    } else {
        Vec::new()
    };

    // 5b max-recall F11: the PreToolUse hook is registered ONLY when the helper binary is
    // available. When it is missing we still emit the `permissions.deny` net rules (deny-only
    // settings) so a net-disabled project never loses its network gating just because the hook
    // could not be located.
    let hooks = match helper_path {
        Some(helper_path) => ClaudeHooks {
            pre_tool_use: vec![HookMatcher {
                matcher: None,
                hooks: vec![HookCommand {
                    kind: "command".to_string(),
                    command: helper_path.to_string(),
                    timeout: Some(hook_timeout_secs),
                }],
            }],
        },
        None => ClaudeHooks::default(),
    };

    // F33 residual: product MCP always allowed (role-gated server-side). Without a
    // consent hook, also allow headless file tools so Write is not stuck on interactive
    // prompts that stream-json cannot answer. Bash stays denied/prompted.
    let mut allow = product_mcp_allow_rules();
    if helper_path.is_none() {
        allow.extend(headless_file_tool_allow_rules());
    }

    ClaudeSettings {
        permissions: ClaudePermissions {
            deny,
            ask: Vec::new(),
            allow,
        },
        hooks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_settings_registers_pretooluse_hook() {
        let settings =
            build_claude_settings(SandboxMode::Unattended, true, Some("/usr/local/bin/claude-hook"), 30);
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"PreToolUse\""));
        assert!(json.contains("\"command\""));
        assert!(json.contains("/usr/local/bin/claude-hook"));
        assert!(json.contains("\"timeout\""));
    }

    #[test]
    fn net_disabled_emits_net_deny_rules() {
        let settings =
            build_claude_settings(SandboxMode::Unattended, false, Some("/usr/local/bin/claude-hook"), 30);
        let json = serde_json::to_string(&settings).unwrap();
        assert!(settings.permissions.deny.contains(&"Bash(curl *)".to_string()));
        assert!(settings.permissions.deny.contains(&"WebFetch".to_string()));
        assert!(json.contains("\"deny\""));
    }

    #[test]
    fn net_enabled_omits_net_deny_rules() {
        let settings =
            build_claude_settings(SandboxMode::Ask, true, Some("/usr/local/bin/claude-hook"), 30);
        assert!(settings.permissions.deny.is_empty());
        let json = serde_json::to_string(&settings).unwrap();
        assert!(!json.contains("\"deny\""));
    }

    #[test]
    fn settings_serializes_pretooluse_and_type_keys() {
        let settings =
            build_claude_settings(SandboxMode::Ask, true, Some("/usr/local/bin/claude-hook"), 30);
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"PreToolUse\""));
        assert!(json.contains("\"type\":\"command\""));
    }

    #[test]
    fn product_mcp_allow_always_present_even_with_hook() {
        // F33 residual: acceptEdits does not cover MCP; allow list must ship with settings.
        let settings =
            build_claude_settings(SandboxMode::Ask, true, Some("/usr/local/bin/claude-hook"), 30);
        let allow = &settings.permissions.allow;
        assert!(
            allow.iter().any(|r| r.contains("mcp__devboule__")),
            "product MCP allow missing: {allow:?}"
        );
        // With hook present, do NOT blanket-allow Write (hook bridges consent).
        assert!(
            !allow.iter().any(|r| r == "Write"),
            "Write must not be pre-allowed when PreToolUse hook is active: {allow:?}"
        );
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"allow\""));
        assert!(json.contains("mcp__devboule__"));
    }

    #[test]
    fn headless_without_hook_allows_file_tools_not_bash() {
        // F33 residual: no interactive human for stream-json → allow Read/Write/Edit.
        let settings = build_claude_settings(SandboxMode::Ask, true, None, 30);
        assert!(settings.hooks.pre_tool_use.is_empty());
        for tool in ["Read", "Write", "Edit", "MultiEdit"] {
            assert!(
                settings.permissions.allow.iter().any(|r| r == tool),
                "missing headless file allow {tool}: {:?}",
                settings.permissions.allow
            );
        }
        assert!(
            !settings.permissions.allow.iter().any(|r| r.starts_with("Bash")),
            "Bash must not be on the headless allow list"
        );
        assert!(settings
            .permissions
            .allow
            .iter()
            .any(|r| r.contains("mcp__devboule__")));
        // Settings must not collapse to {} so --settings is always written.
        let json = serde_json::to_string(&settings).unwrap();
        assert_ne!(json, "{}");
        assert!(json.contains("\"allow\""));
    }

    #[test]
    fn hook_registered_for_unattended_mode_too() {
        let settings =
            build_claude_settings(SandboxMode::Unattended, true, Some("/usr/local/bin/claude-hook"), 30);
        assert!(!settings.hooks.pre_tool_use.is_empty());
    }

    #[test]
    fn deny_only_settings_when_helper_missing_still_keeps_net_rules() {
        // F11: no hook binary → no PreToolUse hook, but the net deny rules MUST remain.
        let settings = build_claude_settings(SandboxMode::Ask, false, None, 30);
        assert!(settings.hooks.pre_tool_use.is_empty(), "no hook when helper is None");
        assert!(settings.permissions.deny.contains(&"Bash(curl *)".to_string()));
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"deny\""));
        assert!(!json.contains("\"PreToolUse\""));
        // F33: product MCP allow still present alongside deny.
        assert!(json.contains("mcp__devboule__"));
    }

    #[test]
    fn f46_claude_oauth_token_env_pair_is_pure_and_never_logs() {
        let (k, v) = claude_oauth_token_env_pair("sk-ant-oat-test-token");
        assert_eq!(k, "CLAUDE_CODE_OAUTH_TOKEN");
        assert_eq!(v, "sk-ant-oat-test-token");
        // Pure helper: empty string still yields a pair (callers filter empties).
        let (k2, v2) = claude_oauth_token_env_pair("");
        assert_eq!(k2, "CLAUDE_CODE_OAUTH_TOKEN");
        assert_eq!(v2, "");
    }

    #[test]
    fn f36_product_config_dir_is_not_home_dot_claude() {
        let base = PathBuf::from("/tmp/devboule-app-state");
        let dir = claude_product_config_dir(&base, "orch/../evil");
        let s = dir.to_string_lossy();
        assert!(s.contains("claude-agent-config"));
        // path traversal in agent_id: / and . stay as non-alphanum → `_`
        assert_eq!(
            dir.file_name().and_then(|n| n.to_str()),
            Some("orch____evil")
        );
        assert!(!s.contains("/.claude/"));
        assert!(!s.ends_with("/.claude"));
        // must stay under base, not escape via agent_id
        assert!(dir.starts_with(&base));
        let (k, v) = claude_config_dir_env(&dir);
        assert_eq!(k, "CLAUDE_CONFIG_DIR");
        assert_eq!(v, dir.to_string_lossy());
    }

    #[test]
    fn f36_ensure_creates_isolated_dir() {
        let base = std::env::temp_dir().join(format!(
            "devboule-claude-cfg-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let dir = ensure_claude_product_config_dir(&base, "agent-1", false).unwrap();
        assert!(dir.is_dir());
        assert!(dir.starts_with(&base));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Hold the crate-wide env lock, point HOME at `fake_home`, run `f`, restore HOME/USERPROFILE.
    fn with_fake_home<R>(fake_home: &Path, f: impl FnOnce() -> R) -> R {
        let _g = crate::backend::state::DEV_UNLOCK_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev_home = std::env::var_os("HOME");
        let prev_userprofile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", fake_home);
        std::env::set_var("USERPROFILE", fake_home);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_userprofile {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
        match result {
            Ok(v) => v,
            Err(e) => std::panic::resume_unwind(e),
        }
    }

    fn f46_temp(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "devboule-f46-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn f46_seeds_credentials_when_dest_missing() {
        let home = f46_temp("home-seed");
        let base = f46_temp("base-seed");
        let claude = home.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        let src = claude.join(".credentials.json");
        std::fs::write(&src, br#"{"token":"fake-seed-only"}"#).unwrap();

        with_fake_home(&home, || {
            let dir = ensure_claude_product_config_dir(&base, "agent-seed", false).unwrap();
            let dest = dir.join(".credentials.json");
            assert!(dest.is_file(), "credentials should be seeded into isolated dir");
            assert_eq!(
                std::fs::read_to_string(&dest).unwrap(),
                r#"{"token":"fake-seed-only"}"#
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = dest.metadata().unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "seeded credentials must be 0600");
            }
        });

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn f46_no_source_credentials_is_ok() {
        let home = f46_temp("home-absent");
        let base = f46_temp("base-absent");
        // No ~/.claude/.credentials.json under fake home.
        std::fs::create_dir_all(home.join(".claude")).unwrap();

        with_fake_home(&home, || {
            let dir = ensure_claude_product_config_dir(&base, "agent-absent", false).unwrap();
            assert!(dir.is_dir());
            assert!(
                !dir.join(".credentials.json").exists(),
                "must not invent credentials when source is absent"
            );
        });

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn f46_does_not_overwrite_newer_dest() {
        let home = f46_temp("home-newer");
        let base = f46_temp("base-newer");
        let claude = home.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        let src = claude.join(".credentials.json");
        std::fs::write(&src, br#"{"token":"older-source"}"#).unwrap();

        // Create dest first under the future isolated path, after source, so dest is newer.
        let dir = claude_product_config_dir(&base, "agent-newer");
        std::fs::create_dir_all(&dir).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let dest = dir.join(".credentials.json");
        std::fs::write(&dest, br#"{"token":"newer-dest"}"#).unwrap();

        with_fake_home(&home, || {
            let out = ensure_claude_product_config_dir(&base, "agent-newer", false).unwrap();
            assert_eq!(out, dir);
            assert_eq!(
                std::fs::read_to_string(&dest).unwrap(),
                r#"{"token":"newer-dest"}"#,
                "newer dest must not be overwritten by older source"
            );
        });

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&base);
    }

    // --- F46-auto: seed oauthAccount login state from $HOME/.claude.json ---------

    #[test]
    fn f46_auto_seeds_login_state_when_dest_missing() {
        let home = f46_temp("home-login-seed");
        let base = f46_temp("base-login-seed");
        // Owner login state at HOME ROOT (not inside ~/.claude/).
        std::fs::write(
            home.join(".claude.json"),
            r#"{"oauthAccount":{"email":"owner@example.com"},"userID":"user-abc","theme":"dark"}"#,
        )
        .unwrap();

        with_fake_home(&home, || {
            let dir = ensure_claude_product_config_dir(&base, "agent-login", false).unwrap();
            let dest = dir.join(".claude.json");
            assert!(dest.is_file(), "login state should be seeded");
            let v: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&dest).unwrap()).unwrap();
            assert!(v.get("oauthAccount").is_some());
            assert_eq!(v["userID"], "user-abc");
            // Must not copy unrelated owner fields (theme).
            assert!(v.get("theme").is_none());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = dest.metadata().unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "seeded .claude.json must be 0600");
            }
        });

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn f46_auto_leaves_existing_oauth_account_untouched() {
        let home = f46_temp("home-login-keep");
        let base = f46_temp("base-login-keep");
        std::fs::write(
            home.join(".claude.json"),
            r#"{"oauthAccount":{"email":"owner@example.com"},"userID":"owner-id"}"#,
        )
        .unwrap();
        let dir = claude_product_config_dir(&base, "agent-keep");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join(".claude.json");
        std::fs::write(
            &dest,
            r#"{"oauthAccount":{"email":"already@isolated.local"},"userID":"dest-id","keepMe":true}"#,
        )
        .unwrap();

        with_fake_home(&home, || {
            ensure_claude_product_config_dir(&base, "agent-keep", false).unwrap();
            let v: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&dest).unwrap()).unwrap();
            assert_eq!(v["oauthAccount"]["email"], "already@isolated.local");
            assert_eq!(v["userID"], "dest-id");
            assert_eq!(v["keepMe"], true);
        });

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn f46_auto_source_absent_or_garbage_is_noop() {
        let home = f46_temp("home-login-absent");
        let base = f46_temp("base-login-absent");
        // No .claude.json under home.
        with_fake_home(&home, || {
            let dir = ensure_claude_product_config_dir(&base, "agent-absent-login", false).unwrap();
            assert!(!dir.join(".claude.json").exists());
        });
        // Garbage JSON → still no-op.
        std::fs::write(home.join(".claude.json"), b"not-json{{{{").unwrap();
        with_fake_home(&home, || {
            let dir =
                ensure_claude_product_config_dir(&base, "agent-garbage-login", false).unwrap();
            assert!(!dir.join(".claude.json").exists());
        });

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn f46_auto_merge_preserves_other_dest_fields() {
        let home = f46_temp("home-login-merge");
        let base = f46_temp("base-login-merge");
        std::fs::write(
            home.join(".claude.json"),
            r#"{"oauthAccount":{"email":"owner@example.com"},"userID":"uid-1"}"#,
        )
        .unwrap();
        let dir = claude_product_config_dir(&base, "agent-merge");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join(".claude.json");
        // Dest has other fields but NO oauthAccount → merge in login, keep fields.
        std::fs::write(&dest, r#"{"theme":"dark","numStartups":3}"#).unwrap();

        with_fake_home(&home, || {
            ensure_claude_product_config_dir(&base, "agent-merge", false).unwrap();
            let v: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&dest).unwrap()).unwrap();
            assert!(v.get("oauthAccount").is_some());
            assert_eq!(v["userID"], "uid-1");
            assert_eq!(v["theme"], "dark");
            assert_eq!(v["numStartups"], 3);
        });

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&base);
    }

    // --- F65: vault token present → drop stale .credentials.json, skip seed -----

    #[test]
    fn f65_vault_token_present_removes_stale_credentials() {
        let base = f46_temp("base-f65-drop");
        let dir = claude_product_config_dir(&base, "agent-stale");
        std::fs::create_dir_all(&dir).unwrap();
        let creds = dir.join(".credentials.json");
        std::fs::write(&creds, br#"{"token":"stale-broken-seed"}"#).unwrap();
        assert!(creds.is_file());

        let out = ensure_claude_product_config_dir(&base, "agent-stale", true).unwrap();
        assert_eq!(out, dir);
        assert!(
            !creds.exists(),
            "F65: vault token present must delete isolated .credentials.json"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn f65_vault_token_absent_keeps_credentials() {
        let base = f46_temp("base-f65-keep");
        let dir = claude_product_config_dir(&base, "agent-keep-creds");
        std::fs::create_dir_all(&dir).unwrap();
        let creds = dir.join(".credentials.json");
        std::fs::write(&creds, br#"{"token":"keep-me"}"#).unwrap();

        // No owner source under a fake empty home → seed is no-op; dest must stay.
        let home = f46_temp("home-f65-keep");
        with_fake_home(&home, || {
            let out = ensure_claude_product_config_dir(&base, "agent-keep-creds", false).unwrap();
            assert_eq!(out, dir);
            assert!(
                creds.is_file(),
                "F65: vault token absent must keep existing .credentials.json"
            );
            assert_eq!(
                std::fs::read_to_string(&creds).unwrap(),
                r#"{"token":"keep-me"}"#
            );
        });

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&base);
    }
}
