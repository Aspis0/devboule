//! Which app-tools MCP backend agents should use.
//!
//! - `rust`: the native `devboule-mcp` binary
//! - `python`: `python -m oracle.server.aspis_mcp` (explicit soak / packaged fallback)
//!
//! See `docs/devboule-mcp-port-plan.md`.
//!
//! **P7 dual-stack default (not silent fallback when rust is explicit):**
//! - `DEVBOULE_MCP_BACKEND` **set** → honor strictly (`rust` / `python` aliases).
//!   Rust chosen but binary missing → fail-closed at [`build_devboule_mcp_server_entry`]
//!   (never silently switches to Python).
//! - **unset** → prefer Rust **only if** [`resolve_devboule_mcp_bin`] succeeds;
//!   else default Python so packaged apps without a sidecar keep working.
//!
//! **Packaging:** `scripts/stage-devboule-mcp.sh` builds the release binary into
//! `src-tauri/binaries/devboule-mcp-<triple>` for Tauri `bundle.externalBin`.
//! At runtime the sidecar lands next to the app executable; setup also records
//! a resolved path via [`set_bundled_mcp_bin`].

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{json, Map, Value};

/// Absolute path to the bundled/staged MCP binary, recorded once at app startup.
static BUNDLED_MCP_BIN: OnceLock<PathBuf> = OnceLock::new();

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
    /// Parse a backend string (testable without global env / binary probe).
    ///
    /// **Target default:** empty / unknown → [`Self::Rust`] (prefer rust when the
    /// caller already decided a value). Only explicit `python` aliases select
    /// Python. Prefer [`Self::from_env`] at runtime: it applies the dual-stack
    /// binary probe when the env var is unset.
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "python" | "py" | "aspis" | "aspis_mcp" => Self::Python,
            // empty, "rust", "native", "devboule-mcp", or anything else → Rust
            _ => Self::Rust,
        }
    }

    /// Resolve backend from env with P7 dual-stack default.
    ///
    /// - `DEVBOULE_MCP_BACKEND` set (non-empty) → [`Self::parse`] strictly.
    /// - unset / empty → Rust if `devboule-mcp` resolves, else Python.
    ///
    /// In unit tests, prefer [`with_backend_override`] over mutating process env
    /// (thread-local, race-free under `cargo test` parallelism).
    pub fn from_env() -> Self {
        #[cfg(test)]
        {
            if let Some(overridden) = test_backend_override() {
                return overridden;
            }
        }
        match std::env::var(ENV_BACKEND) {
            Ok(v) if !v.trim().is_empty() => Self::parse(&v),
            _ => {
                // Unset: rust if binary available, else python (soak / packaged
                // without sidecar). Not a silent fallback when rust is explicit.
                if resolve_devboule_mcp_bin().is_ok() {
                    Self::Rust
                } else {
                    Self::Python
                }
            }
        }
    }
}

// ---- test-only backend override (thread-local; no process-env races) --------

#[cfg(test)]
thread_local! {
    static BACKEND_OVERRIDE: std::cell::Cell<Option<McpBackend>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn test_backend_override() -> Option<McpBackend> {
    BACKEND_OVERRIDE.with(|c| c.get())
}

/// Run `f` with [`McpBackend::from_env`] forced to `backend` on **this thread**.
///
/// Restores the previous override (if any) even on panic. Prefer this over
/// `std::env::set_var(DEVBOULE_MCP_BACKEND, …)` in tests.
#[cfg(test)]
pub fn with_backend_override<R>(backend: McpBackend, f: impl FnOnce() -> R) -> R {
    BACKEND_OVERRIDE.with(|c| {
        let prev = c.get();
        c.set(Some(backend));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        c.set(prev);
        match result {
            Ok(v) => v,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    })
}

/// Record the absolute path of the bundled/staged `devboule-mcp` binary.
///
/// Call once from the Tauri setup hook after probing `current_exe` siblings and
/// `resource_dir`. Subsequent calls are ignored (`OnceLock`).
pub fn set_bundled_mcp_bin(path: &Path) {
    if !crate::backend::provider_detect::is_executable_file(path) {
        return;
    }
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let _ = BUNDLED_MCP_BIN.set(abs);
}

/// Best-effort discovery at app startup: siblings of the main exe (Tauri
/// `externalBin` lands here) and common resource_dir layouts.
pub fn discover_and_record_bundled_mcp_bin(resource_dir: Option<&Path>) {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Bare name first (Tauri externalBin strips the triple at package time).
            // Also accept triple-suffixed names if a packager left them in place.
            let mut candidates: Vec<PathBuf> = vec![dir.join(bin_name())];
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            candidates.push(dir.join("devboule-mcp-aarch64-apple-darwin"));
            #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
            candidates.push(dir.join("devboule-mcp-x86_64-apple-darwin"));
            #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
            candidates.push(dir.join("devboule-mcp-x86_64-unknown-linux-gnu"));
            #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
            candidates.push(dir.join("devboule-mcp-aarch64-unknown-linux-gnu"));
            #[cfg(windows)]
            candidates.push(dir.join("devboule-mcp.exe"));
            for cand in candidates {
                if crate::backend::provider_detect::is_executable_file(&cand) {
                    set_bundled_mcp_bin(&cand);
                    return;
                }
            }
            // macOS app: Resources/ next to MacOS/
            if let Some(resources) = dir.parent().map(|p| p.join("Resources")) {
                for name in [bin_name(), &format!("bin/{}", bin_name())] {
                    let cand = resources.join(name);
                    if crate::backend::provider_detect::is_executable_file(&cand) {
                        set_bundled_mcp_bin(&cand);
                        return;
                    }
                }
            }
        }
    }
    if let Some(dir) = resource_dir {
        for name in [
            bin_name(),
            &format!("bin/{}", bin_name()),
            &format!("binaries/{}", bin_name()),
            &format!("resources/bin/{}", bin_name()),
        ] {
            let cand = dir.join(name);
            if crate::backend::provider_detect::is_executable_file(&cand) {
                set_bundled_mcp_bin(&cand);
                return;
            }
        }
    }
}

/// Resolve `devboule-mcp` absolute path when backend is Rust.
///
/// Order: `DEVBOULE_MCP_BIN` (error if set but missing/not executable),
/// [`set_bundled_mcp_bin`] path, debug-only cargo target tree, `current_exe`
/// siblings / Resources, staged `src-tauri/binaries/` (dev), then PATH.
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

    if let Some(bundled) = BUNDLED_MCP_BIN.get() {
        if crate::backend::provider_detect::is_executable_file(bundled) {
            return Ok(bundled.clone());
        }
    }

    // Compile-time sibling paths only in debug builds.
    // Release must not bake developer absolute paths from the build machine.
    // Prefer staged externalBin (fresh release stage) over stale target/debug.
    if cfg!(debug_assertions) {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let staged = manifest.join("binaries").join(bin_name());
        if let Some(abs) = absolute_if_executable(&staged) {
            return Ok(abs);
        }
        for profile in ["release", "debug"] {
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
            if let Some(resources) = dir.parent().map(|p| p.join("Resources")) {
                let cand = resources.join(bin_name());
                if let Some(abs) = absolute_if_executable(&cand) {
                    return Ok(abs);
                }
            }
        }
    }

    crate::backend::provider_detect::resolve_program("devboule-mcp").ok_or_else(|| {
        format!(
            "devboule-mcp binary not found (set {ENV_BIN}, run scripts/stage-devboule-mcp.sh, \
             or package with Tauri externalBin; use DEVBOULE_MCP_BACKEND=python for soak)"
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
        // Pure parse target default: empty / unknown → Rust; only python aliases → Python.
        // Runtime dual-stack probe lives in `from_env` (unset → rust if bin, else python).
        assert_eq!(McpBackend::parse(""), McpBackend::Rust);
        assert_eq!(McpBackend::parse("   "), McpBackend::Rust);
        assert_eq!(McpBackend::parse("rust"), McpBackend::Rust);
        assert_eq!(McpBackend::parse("NATIVE"), McpBackend::Rust);
        assert_eq!(McpBackend::parse("devboule-mcp"), McpBackend::Rust);
        assert_eq!(McpBackend::parse("garbage"), McpBackend::Rust);
        assert_eq!(McpBackend::parse("python"), McpBackend::Python);
        assert_eq!(McpBackend::parse("PY"), McpBackend::Python);
        assert_eq!(McpBackend::parse("aspis_mcp"), McpBackend::Python);
        assert_eq!(McpBackend::parse("aspis"), McpBackend::Python);
    }

    #[test]
    fn from_env_honors_thread_local_override() {
        // Prefer override over process env so parallel cargo tests stay race-free.
        assert_eq!(
            with_backend_override(McpBackend::Python, || McpBackend::from_env()),
            McpBackend::Python
        );
        assert_eq!(
            with_backend_override(McpBackend::Rust, || McpBackend::from_env()),
            McpBackend::Rust
        );
        // Nested override restores outer.
        let nested = with_backend_override(McpBackend::Python, || {
            assert_eq!(McpBackend::from_env(), McpBackend::Python);
            with_backend_override(McpBackend::Rust, || {
                assert_eq!(McpBackend::from_env(), McpBackend::Rust);
            });
            McpBackend::from_env()
        });
        assert_eq!(nested, McpBackend::Python);
    }

    #[test]
    fn from_env_unset_prefers_rust_when_bin_resolves() {
        // Under cargo test (debug), sibling devboule-mcp/target usually exists after
        // a workspace build; if not, dual-stack falls back to Python — both valid.
        // This asserts the probe is wired: result matches resolve_devboule_mcp_bin.
        let expected = if resolve_devboule_mcp_bin().is_ok() {
            McpBackend::Rust
        } else {
            McpBackend::Python
        };
        // No thread-local override; process ENV_BACKEND may be set by the operator —
        // only assert the dual-stack probe contract when the var is unset/empty.
        match std::env::var(ENV_BACKEND) {
            Ok(v) if !v.trim().is_empty() => {
                assert_eq!(McpBackend::from_env(), McpBackend::parse(&v));
            }
            _ => {
                assert_eq!(McpBackend::from_env(), expected);
            }
        }
    }

    #[test]
    fn parse_explicit_rust_is_strict() {
        // Explicit rust must not be rewritten to python by parse (fail-closed later
        // at build_entry if the binary is missing).
        assert_eq!(McpBackend::parse("rust"), McpBackend::Rust);
        assert_eq!(McpBackend::parse("RUST"), McpBackend::Rust);
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
