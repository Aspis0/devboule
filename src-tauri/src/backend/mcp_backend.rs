//! Which app-tools MCP backend agents should use.
//!
//! - `python` (default until cutover): `python -m oracle.server.aspis_mcp`
//! - `rust`: the native `devboule-mcp` binary
//!
//! See `docs/devboule-mcp-port-plan.md`.
//!
//! **Packaging (P0 honesty):** the Rust backend is **not** bundled in Tauri
//! resources until P7. Selecting `rust` requires `DEVBOULE_MCP_BIN` or a local
//! `devboule-mcp` tree build / PATH install.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

/// Env var selecting the MCP implementation.
pub const ENV_BACKEND: &str = "DEVBOULE_MCP_BACKEND";

/// Optional absolute path to the Rust MCP binary.
pub const ENV_BIN: &str = "DEVBOULE_MCP_BIN";

pub const ENV_CF_PROFILE_MODE: &str = "DEVBOULE_MCP_CLOUDFLARE_PROFILE_MODE";
pub const LEGACY_ENV_CF_PROFILE_MODE: &str = "ASPIS_MCP_CLOUDFLARE_PROFILE_MODE";
pub const ENV_APP_BIN: &str = "DEVBOULE_APP_BIN";
pub const LEGACY_ENV_APP_BIN: &str = "ASPIS_APP_BIN";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpBackend {
    Python,
    Rust,
}

impl McpBackend {
    /// Parse a backend string (testable without global env).
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "rust" | "devboule-mcp" | "native" => Self::Rust,
            _ => Self::Python,
        }
    }

    /// Parse from env. Default **Python** until P7 cutover.
    pub fn from_env() -> Self {
        Self::parse(&std::env::var(ENV_BACKEND).unwrap_or_default())
    }
}

/// Resolve `devboule-mcp` absolute path when backend is Rust.
///
/// Order: `DEVBOULE_MCP_BIN` (error if set but missing/not executable), compile-time
/// crate target paths, `current_exe` siblings, then PATH via `resolve_program`.
pub fn resolve_devboule_mcp_bin() -> Result<PathBuf, String> {
    resolve_devboule_mcp_bin_with(std::env::var(ENV_BIN).ok().as_deref())
}

/// Pure-ish resolver: optional `DEVBOULE_MCP_BIN` override for tests (avoids env races).
///
/// `env_bin` mirrors the env var: `None` = unset, `Some("")` = set-but-empty (error),
/// `Some(path)` = that path must be an executable file.
pub fn resolve_devboule_mcp_bin_with(env_bin: Option<&str>) -> Result<PathBuf, String> {
    if let Some(p) = env_bin {
        let trimmed = p.trim();
        if trimmed.is_empty() {
            return Err(format!("{ENV_BIN} is set but empty"));
        }
        let pb = PathBuf::from(trimmed);
        if !crate::backend::provider_detect::is_executable_file(&pb) {
            return Err(format!(
                "{ENV_BIN} is set to {} but that path is not an executable file",
                pb.display()
            ));
        }
        // Store absolute path so Claude/Codex/pi configs do not depend on CWD.
        return std::fs::canonicalize(&pb).map_err(|e| {
            format!(
                "{ENV_BIN} path {} is executable but could not be canonicalized: {e}",
                pb.display()
            )
        });
    }

    // Prefer the profile matching this binary, then the other.
    // debug_assertions → debug then release; release builds → release then debug.
    let profiles: &[&str] = if cfg!(debug_assertions) {
        &["debug", "release"]
    } else {
        &["release", "debug"]
    };

    // Compile-time path to this crate's sibling `devboule-mcp/target/...`
    // (works for `cargo test` / `cargo run` from src-tauri without runtime env).
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for profile in profiles {
        let cand = manifest
            .join("..")
            .join("devboule-mcp")
            .join("target")
            .join(profile)
            .join(bin_name());
        if let Some(abs) = absolute_if_executable(&cand) {
            return Ok(abs);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join(bin_name());
            if let Some(abs) = absolute_if_executable(&cand) {
                return Ok(abs);
            }
            let cand = dir.join("resources").join(bin_name());
            if let Some(abs) = absolute_if_executable(&cand) {
                return Ok(abs);
            }
        }
    }

    crate::backend::provider_detect::resolve_program("devboule-mcp").ok_or_else(|| {
        format!(
            "devboule-mcp binary not found (set {ENV_BIN} or build crate devboule-mcp; \
             not bundled in Tauri resources until P7)"
        )
    })
}

/// Return a canonical absolute path if `path` is an executable file.
fn absolute_if_executable(path: &Path) -> Option<PathBuf> {
    if !crate::backend::provider_detect::is_executable_file(path) {
        return None;
    }
    std::fs::canonicalize(path).ok()
}

fn bin_name() -> &'static str {
    #[cfg(windows)]
    {
        "devboule-mcp.exe"
    }
    #[cfg(not(windows))]
    {
        "devboule-mcp"
    }
}

/// Insert Devboule + legacy Aspis env keys for MCP children (one release dual-write).
pub fn insert_dual_mcp_env(env: &mut Map<String, Value>, app_bin: Option<&str>) {
    env.insert(ENV_CF_PROFILE_MODE.into(), json!("1"));
    env.insert(LEGACY_ENV_CF_PROFILE_MODE.into(), json!("1"));
    if let Some(bin) = app_bin.map(str::trim).filter(|s| !s.is_empty()) {
        env.insert(ENV_APP_BIN.into(), json!(bin));
        env.insert(LEGACY_ENV_APP_BIN.into(), json!(bin));
    }
}

/// Build the MCP server entry for Claude/Codex/pi configs.
///
/// `python_interpreter` is required for the Python backend; ignored for Rust.
/// Fail-closed: when backend is Rust and the binary cannot be resolved, returns `Err`
/// (never silently falls back to Python).
pub fn build_devboule_mcp_server_entry(
    backend: McpBackend,
    python_interpreter: &str,
    management_root: &Path,
    projects_dir: &Path,
    app_bin: Option<&str>,
) -> Result<Value, String> {
    build_devboule_mcp_server_entry_with(
        backend,
        python_interpreter,
        management_root,
        projects_dir,
        app_bin,
        None,
    )
}

/// Like [`build_devboule_mcp_server_entry`], but accepts an optional bin override for tests.
///
/// When `rust_bin_override` is `Some`, it is used instead of env/path resolution
/// (`Some(None)` means "env unset"; `Some(Some(path))` forces that path).
/// When `None`, uses normal [`resolve_devboule_mcp_bin`] resolution.
pub fn build_devboule_mcp_server_entry_with(
    backend: McpBackend,
    python_interpreter: &str,
    management_root: &Path,
    projects_dir: &Path,
    app_bin: Option<&str>,
    rust_bin_override: Option<Option<&str>>,
) -> Result<Value, String> {
    let mut env = Map::new();
    insert_dual_mcp_env(&mut env, app_bin);

    match backend {
        McpBackend::Python => {
            env.insert(
                "PYTHONPATH".into(),
                json!(management_root.to_string_lossy()),
            );
            env.insert("PYTHONIOENCODING".into(), json!("utf-8"));
            env.insert("HF_HUB_OFFLINE".into(), json!("1"));
            env.insert("TRANSFORMERS_OFFLINE".into(), json!("1"));
            env.insert("ORACLE_REQUIRE_REAL_EMBEDDER".into(), json!("1"));
            Ok(json!({
                "command": python_interpreter,
                "args": [
                    "-m",
                    "oracle.server.aspis_mcp",
                    "--root",
                    management_root.to_string_lossy(),
                    "--projects-dir",
                    projects_dir.to_string_lossy(),
                ],
                "env": env,
            }))
        }
        McpBackend::Rust => {
            let bin = match rust_bin_override {
                Some(override_val) => resolve_devboule_mcp_bin_with(override_val)?,
                None => resolve_devboule_mcp_bin()?,
            };
            // Pass roots via env until the Rust server grows CLI flags (P1+).
            env.insert(
                "DEVBOULE_MCP_ROOT".into(),
                json!(management_root.to_string_lossy()),
            );
            env.insert(
                "DEVBOULE_MCP_PROJECTS_DIR".into(),
                json!(projects_dir.to_string_lossy()),
            );
            // Legacy names for any code still reading ASPIS_*.
            env.insert(
                "ASPIS_MCP_ROOT".into(),
                json!(management_root.to_string_lossy()),
            );
            env.insert(
                "ASPIS_MCP_PROJECTS_DIR".into(),
                json!(projects_dir.to_string_lossy()),
            );
            Ok(json!({
                "command": bin.to_string_lossy(),
                "args": [],
                "env": env,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_exec(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "devboule-mcp-bin-{}-{}-{:?}",
            tag,
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("devboule-mcp-fake");
        fs::write(&path, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    #[test]
    fn parse_backend_strings() {
        assert_eq!(McpBackend::parse(""), McpBackend::Python);
        assert_eq!(McpBackend::parse("python"), McpBackend::Python);
        assert_eq!(McpBackend::parse("rust"), McpBackend::Rust);
        assert_eq!(McpBackend::parse("NATIVE"), McpBackend::Rust);
        assert_eq!(McpBackend::parse("devboule-mcp"), McpBackend::Rust);
    }

    #[test]
    fn dual_env_writes_both_keys() {
        let mut env = Map::new();
        insert_dual_mcp_env(&mut env, Some("/tmp/app"));
        assert_eq!(env.get(ENV_CF_PROFILE_MODE), Some(&json!("1")));
        assert_eq!(env.get(LEGACY_ENV_CF_PROFILE_MODE), Some(&json!("1")));
        assert_eq!(env.get(ENV_APP_BIN), Some(&json!("/tmp/app")));
        assert_eq!(env.get(LEGACY_ENV_APP_BIN), Some(&json!("/tmp/app")));
    }

    #[test]
    fn python_entry_still_uses_aspis_module() {
        let entry = build_devboule_mcp_server_entry(
            McpBackend::Python,
            "/venv/bin/python",
            Path::new("/mgmt"),
            Path::new("/projects"),
            None,
        )
        .unwrap();
        assert_eq!(entry["command"], "/venv/bin/python");
        assert!(entry["args"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "oracle.server.aspis_mcp"));
        assert!(entry["env"].get(ENV_CF_PROFILE_MODE).is_some());
        assert!(entry["env"].get(LEGACY_ENV_CF_PROFILE_MODE).is_some());
        // Dual-write keys present; app bin omitted when None.
        assert!(entry["env"].get(ENV_APP_BIN).is_none());
        assert!(entry["env"].get(LEGACY_ENV_APP_BIN).is_none());
    }

    #[test]
    fn rust_entry_uses_override_bin_empty_args_and_root_dual_keys() {
        let bin = temp_exec("ok");
        let canon = fs::canonicalize(&bin).unwrap();
        let entry = build_devboule_mcp_server_entry_with(
            McpBackend::Rust,
            "/ignored/python",
            Path::new("/mgmt"),
            Path::new("/projects"),
            Some("/tmp/app"),
            Some(Some(bin.to_str().unwrap())),
        )
        .expect("rust entry should build");

        assert_eq!(
            entry["command"].as_str().unwrap(),
            canon.to_str().unwrap(),
            "command must be the canonicalized binary path"
        );
        assert_eq!(
            entry["args"].as_array().map(|a| a.len()).unwrap_or(99),
            0,
            "rust backend uses empty args (roots via env)"
        );

        let env = entry["env"].as_object().expect("env object");
        assert_eq!(env.get("DEVBOULE_MCP_ROOT"), Some(&json!("/mgmt")));
        assert_eq!(env.get("ASPIS_MCP_ROOT"), Some(&json!("/mgmt")));
        assert_eq!(
            env.get("DEVBOULE_MCP_PROJECTS_DIR"),
            Some(&json!("/projects"))
        );
        assert_eq!(env.get("ASPIS_MCP_PROJECTS_DIR"), Some(&json!("/projects")));
        assert_eq!(env.get(ENV_APP_BIN), Some(&json!("/tmp/app")));
        assert_eq!(env.get(LEGACY_ENV_APP_BIN), Some(&json!("/tmp/app")));
        assert_eq!(env.get(ENV_CF_PROFILE_MODE), Some(&json!("1")));
        assert_eq!(env.get(LEGACY_ENV_CF_PROFILE_MODE), Some(&json!("1")));

        // Must not look like the Python module launch.
        let args = entry["args"].as_array().unwrap();
        assert!(!args.iter().any(|a| a == "oracle.server.aspis_mcp"));

        let _ = fs::remove_dir_all(bin.parent().unwrap());
    }

    #[test]
    fn rust_entry_fails_closed_when_bin_missing() {
        let missing = std::env::temp_dir().join(format!(
            "devboule-mcp-missing-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        // Ensure it does not exist.
        let _ = fs::remove_file(&missing);
        let err = build_devboule_mcp_server_entry_with(
            McpBackend::Rust,
            "/ignored/python",
            Path::new("/mgmt"),
            Path::new("/projects"),
            None,
            Some(Some(missing.to_str().unwrap())),
        )
        .expect_err("missing bin must Err");
        assert!(
            err.contains(ENV_BIN) || err.contains("not an executable"),
            "error should mention bin/executable: {err}"
        );
    }

    #[test]
    fn resolve_with_empty_env_bin_errors() {
        let err = resolve_devboule_mcp_bin_with(Some("  ")).expect_err("empty");
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn resolve_with_existing_executable() {
        let bin = temp_exec("resolve");
        let canon = fs::canonicalize(&bin).unwrap();
        let got = resolve_devboule_mcp_bin_with(Some(bin.to_str().unwrap())).unwrap();
        assert_eq!(got, canon, "resolved path must be absolute/canonical");
        let _ = fs::remove_dir_all(bin.parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_rejects_non_executable_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "devboule-mcp-nonexec-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("not-exec");
        fs::write(&path, b"nope").unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&path, perms).unwrap();

        let err = resolve_devboule_mcp_bin_with(Some(path.to_str().unwrap())).expect_err("no exec");
        assert!(
            err.contains("not an executable"),
            "expected executable check: {err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
