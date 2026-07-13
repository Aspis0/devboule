//! Project-scoped pi MCP config writer — ensures `<project_root>/.pi/mcp.json`
//! contains an `aspis-management` server entry so pi sessions discover it via
//! the pi-mcp-adapter's project-local config sources (`.pi/mcp.json` and
//! `.mcp.json`).
//!
//! Mirrors the `aspis-management` MCP entry that claude/codex receive via
//! `cli_agents.rs` / `mcp_client_config_json`, but writes it into the pi JSON
//! format that `pi-mcp-adapter/config.ts:getConfigSources` reads.
//!
//! The merge is preserve-and-overwrite: existing foreign keys in `mcpServers`
//! are kept byte-equivalent; only the `aspis-management` key is set/updated.
//! Atomic write (tmp + rename) prevents corruption on crash.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Map, Value};

/// Monotonic counter for unique temp filenames in `ensure_project_pi_mcp_config`.
/// Multiple spawns (orchestrator + coder + mini) can run concurrently for the
/// same project, so a single fixed `mcp.json.tmp` would race; each write gets a
/// per-call `<pid>.<seq>` suffix and the rename stays atomic.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Build the `aspis-management` MCP server entry for the pi mcp.json format.
/// Uses the same resolution chain as the claude/codex MCP wiring — the caller
/// passes pre-resolved values so this function stays pure and testable.
fn build_aspis_entry(
    python: &str,
    management_root: &Path,
    projects_dir: &Path,
    app_bin: Option<&str>,
) -> Value {
    let mut env = Map::new();
    env.insert(
        "PYTHONPATH".into(),
        management_root.to_string_lossy().into_owned().into(),
    );
    env.insert("PYTHONIOENCODING".into(), "utf-8".into());
    env.insert("HF_HUB_OFFLINE".into(), "1".into());
    env.insert("TRANSFORMERS_OFFLINE".into(), "1".into());
    env.insert(
        "ASPIS_MCP_CLOUDFLARE_PROFILE_MODE".into(),
        "1".into(),
    );
    if let Some(bin) = app_bin.filter(|s| !s.trim().is_empty()) {
        env.insert("ASPIS_APP_BIN".into(), bin.to_string().into());
    }

    json!({
        "command": python,
        "args": [
            "-m", "oracle.server.aspis_mcp",
            "--root", management_root.to_string_lossy(),
            "--projects-dir", projects_dir.to_string_lossy(),
        ],
        "transport": "stdio",
        "lifecycle": "eager",
        "env": env,
    })
}

/// Ensure `<project_root>/.pi/mcp.json` contains an `aspis-management` MCP
/// server entry. Reads the existing file (if any), merges the entry, and writes
/// atomically (tmp + rename). Creates the `.pi` directory if missing.
///
/// Fails SOFT: returns `Ok(())` when the file is already correct or on
/// non-critical errors. Returns `Err` only for I/O failures that indicate a
/// real problem the caller should surface.
///
/// After a successful write, `<project_root>/.pi/.gitignore` is ensured to
/// contain a `mcp.json` line: the generated config embeds machine-local
/// absolute paths (python interpreter, management root under
/// `/Users/<name>/...`, app binary) that must never be committed into the
/// user's repo via `git add -A`.
///
/// # Arguments
/// * `project_root` — the target project's working folder (e.g. the repo root).
/// * `python` — resolved Python interpreter path (via `resolve_oracle_python`).
/// * `management_root` — the Aspis management package root.
/// * `projects_dir` — the managed projects directory.
/// * `app_bin` — optional running app binary path for `ASPIS_APP_BIN`.
pub(crate) fn ensure_project_pi_mcp_config(
    project_root: &Path,
    python: &str,
    management_root: &Path,
    projects_dir: &Path,
    app_bin: Option<&str>,
) -> Result<(), String> {
    let pi_dir = project_root.join(".pi");
    let config_path = pi_dir.join("mcp.json");

    // Create .pi/ if missing.
    fs::create_dir_all(&pi_dir)
        .map_err(|e| format!("Failed to create {}: {e}", pi_dir.display()))?;

    // Read existing config (if any).
    let existing: Value = if config_path.exists() {
        let raw = fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read {}: {e}", config_path.display()))?;
        serde_json::from_str(&raw)
            .map_err(|e| format!("Failed to parse {}: {e}", config_path.display()))?
    } else {
        json!({})
    };

    // Guard: mcpServers must be an object (or absent → create one).
    // Mirrors the setdefault semantics of cli_agents.rs: we refuse to overwrite
    // a non-object mcpServers value (it might be a config the user manually
    // crafted for another purpose).
    let mut root = match existing {
        Value::Object(map) => map,
        _ => {
            return Err(format!(
                "Refusing to write {}: mcpServers is not an object",
                config_path.display()
            ));
        }
    };

    let mcp_servers = match root.get_mut("mcpServers") {
        Some(Value::Object(map)) => map,
        Some(_) => {
            return Err(format!(
                "Refusing to write {}: mcpServers is not an object",
                config_path.display()
            ));
        }
        None => {
            // No mcpServers key — insert an empty object.
            root.insert("mcpServers".into(), Value::Object(Map::new()));
            root.get_mut("mcpServers")
                .unwrap()
                .as_object_mut()
                .unwrap()
        }
    };

    // Set/overwrite the aspis-management entry. Foreign keys are preserved.
    let entry = build_aspis_entry(python, management_root, projects_dir, app_bin);
    mcp_servers.insert("aspis-management".into(), entry);

    // Atomic write: tmp + rename. The tmp filename is unique per call (pid + a
    // monotonic counter) so concurrent spawns don't clobber each other.
    let seq = TMP_SEQ.fetch_add(1, Ordering::SeqCst);
    let tmp_path = pi_dir.join(format!("mcp.json.tmp.{}.{}", std::process::id(), seq));
    let serialized = serde_json::to_string_pretty(&Value::Object(root))
        .map_err(|e| format!("Failed to serialize MCP config: {e}"))?;

    fs::write(&tmp_path, format!("{serialized}\n"))
        .map_err(|e| format!("Failed to write {}: {e}", tmp_path.display()))?;
    fs::rename(&tmp_path, &config_path)
        .map_err(|e| format!("Failed to rename {} → {}: {e}", tmp_path.display(), config_path.display()))?;

    // X4: ensure `.pi/.gitignore` excludes `mcp.json`. The generated config
    // embeds machine-local absolute paths (python interpreter, management root
    // under /Users/<name>/..., app binary), which must NEVER be committed into
    // the user's repo. Fail-SOFT: any error here is logged only and must not
    // fail the config write we just completed successfully.
    ensure_pi_gitignore(&pi_dir);

    Ok(())
}

/// Ensure `<pi_dir>/.gitignore` exists and contains a `mcp.json` line, so the
/// machine-local config we write is not accidentally committed by the user's
/// `git add -A`. Fail-SOFT: any error is logged only.
fn ensure_pi_gitignore(pi_dir: &Path) {
    let gitignore = pi_dir.join(".gitignore");
    let target = "mcp.json";
    match fs::read_to_string(&gitignore) {
        Ok(contents) => {
            let already_present = contents
                .lines()
                .any(|l| l.trim() == target);
            if !already_present {
                let mut new_contents = contents;
                if !new_contents.ends_with('\n') {
                    new_contents.push('\n');
                }
                new_contents.push_str("mcp.json\n");
                if let Err(e) = fs::write(&gitignore, new_contents) {
                    eprintln!("[pi-mcp-config] failed to append mcp.json to {}: {e}", gitignore.display());
                }
            }
        }
        Err(e) => {
            // Distinguish "file missing" (NotFound → create fresh) from any other
            // read failure (permission denied, non-UTF-8 content, ...). In those
            // other cases the file likely EXISTS and may hold the user's ignore
            // rules, so we must NOT overwrite it with just "mcp.json\n" — we log
            // and return untouched (fail-soft, never guess it's missing).
            if e.kind() == std::io::ErrorKind::NotFound {
                // File missing: create it with just the mcp.json line.
                if let Err(e) = fs::write(&gitignore, format!("{target}\n")) {
                    eprintln!("[pi-mcp-config] failed to create {}: {e}", gitignore.display());
                }
            } else {
                eprintln!(
                    "[pi-mcp-config] failed to read {} (not touched): {e}",
                    gitignore.display()
                );
            }
        }
    }
}

/// Resolve the paths needed for the aspis-management MCP entry. Reuses the
/// EXISTING resolution chain from the claude/codex MCP wiring so the pi path
/// stays in lock-step.
///
/// Returns `(python, management_root, projects_dir, app_bin)` or an error
/// string. The caller should fail-SOFT on error (spawn without MCP is better
/// than no spawn at all).
pub(crate) fn resolve_mcp_paths(
    app: &tauri::AppHandle,
) -> Result<(String, PathBuf, PathBuf, Option<String>), String> {
    let python = crate::oracle::oracle_setup::resolve_oracle_python();
    let projects_path = super::projects::ensure_projects_dir(app)?;
    let management_root = super::agents::management_root_for_mcp(app, &projects_path);
    let app_bin = super::projects::resolve_app_binary()
        .map(|p| p.to_string_lossy().into_owned());
    Ok((python, management_root, projects_path, app_bin))
}

// ---- tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: create a temp project dir and return (project_root).
    /// Cleanup is best-effort (mirrors the existing test pattern in pi_sidecar.rs).
    fn setup_project() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pi-mcp-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        ));
        fs::create_dir_all(&root).expect("temp project dir");
        root
    }

    fn cleanup_project(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn merge_preserves_foreign_keys() {
        let root = setup_project();
        let pi_dir = root.join(".pi");
        fs::create_dir_all(&pi_dir).unwrap();
        let config_path = pi_dir.join("mcp.json");

        // Write a config with a foreign key.
        let existing = json!({
            "mcpServers": {
                "oracle-figlyph": {
                    "command": "/usr/bin/python3",
                    "args": ["-m", "oracle.server.mcp_handler"],
                    "transport": "stdio"
                }
            }
        });
        fs::write(&config_path, serde_json::to_string_pretty(&existing).unwrap()).unwrap();

        ensure_project_pi_mcp_config(
            &root,
            "/usr/bin/python3",
            Path::new("/opt/management"),
            Path::new("/opt/projects"),
            None,
        )
        .unwrap();

        // Verify: foreign key preserved, aspis-management added.
        let result: Value = serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        let servers = result["mcpServers"].as_object().unwrap();
        assert!(
            servers.contains_key("oracle-figlyph"),
            "foreign key must be preserved"
        );
        assert!(
            servers.contains_key("aspis-management"),
            "aspis-management must be added"
        );
        assert_eq!(
            servers["aspis-management"]["command"],
            "/usr/bin/python3"
        );
        cleanup_project(&root);
    }

    #[test]
    fn merge_overwrites_stale_aspis_entry() {
        let root = setup_project();
        let pi_dir = root.join(".pi");
        fs::create_dir_all(&pi_dir).unwrap();
        let config_path = pi_dir.join("mcp.json");

        // Write a config with a stale aspis-management entry.
        let existing = json!({
            "mcpServers": {
                "aspis-management": {
                    "command": "/old/python",
                    "args": ["-m", "old_module"],
                    "transport": "stdio"
                }
            }
        });
        fs::write(&config_path, serde_json::to_string_pretty(&existing).unwrap()).unwrap();

        ensure_project_pi_mcp_config(
            &root,
            "/usr/bin/python3",
            Path::new("/opt/management"),
            Path::new("/opt/projects"),
            None,
        )
        .unwrap();

        // Verify: stale entry overwritten.
        let result: Value = serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        let servers = result["mcpServers"].as_object().unwrap();
        assert_eq!(
            servers["aspis-management"]["command"],
            "/usr/bin/python3"
        );
        // Old command must NOT be there.
        assert_ne!(
            servers["aspis-management"]["command"],
            "/old/python"
        );
        cleanup_project(&root);
    }

    #[test]
    fn merge_refuses_non_object_mcp_servers() {
        let root = setup_project();
        let pi_dir = root.join(".pi");
        fs::create_dir_all(&pi_dir).unwrap();
        let config_path = pi_dir.join("mcp.json");

        // mcpServers is a string, not an object.
        let existing = json!({
            "mcpServers": "not-an-object"
        });
        fs::write(&config_path, serde_json::to_string_pretty(&existing).unwrap()).unwrap();

        let result = ensure_project_pi_mcp_config(
            &root,
            "/usr/bin/python3",
            Path::new("/opt/management"),
            Path::new("/opt/projects"),
            None,
        );
        assert!(result.is_err(), "must refuse non-object mcpServers");
        assert!(
            result.unwrap_err().contains("not an object"),
            "error must mention the issue"
        );
        cleanup_project(&root);
    }

    #[test]
    fn creates_file_from_scratch() {
        let root = setup_project();

        ensure_project_pi_mcp_config(
            &root,
            "/usr/bin/python3",
            Path::new("/opt/management"),
            Path::new("/opt/projects"),
            None,
        )
        .unwrap();

        // Verify: file created with correct structure.
        let config_path = root.join(".pi").join("mcp.json");
        assert!(config_path.exists(), "config file must be created");

        let result: Value = serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        let servers = result["mcpServers"].as_object().unwrap();
        assert!(servers.contains_key("aspis-management"));

        let entry = &servers["aspis-management"];
        assert_eq!(entry["command"], "/usr/bin/python3");
        assert_eq!(entry["transport"], "stdio");
        assert_eq!(entry["lifecycle"], "eager");

        // Args must include the module and both path flags.
        let args = entry["args"].as_array().unwrap();
        assert!(args.contains(&json!("-m")));
        assert!(args.contains(&json!("oracle.server.aspis_mcp")));
        assert!(args.contains(&json!("--root")));
        assert!(args.contains(&json!("--projects-dir")));
        cleanup_project(&root);
    }

    #[test]
    fn includes_app_bin_when_provided() {
        let root = setup_project();

        ensure_project_pi_mcp_config(
            &root,
            "/usr/bin/python3",
            Path::new("/opt/management"),
            Path::new("/opt/projects"),
            Some("/usr/local/bin/aspis-app"),
        )
        .unwrap();

        let config_path = root.join(".pi").join("mcp.json");
        let result: Value = serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        let env = &result["mcpServers"]["aspis-management"]["env"];
        assert_eq!(env["ASPIS_APP_BIN"], "/usr/local/bin/aspis-app");
        cleanup_project(&root);
    }

    #[test]
    fn omits_app_bin_when_none() {
        let root = setup_project();

        ensure_project_pi_mcp_config(
            &root,
            "/usr/bin/python3",
            Path::new("/opt/management"),
            Path::new("/opt/projects"),
            None,
        )
        .unwrap();

        let config_path = root.join(".pi").join("mcp.json");
        let result: Value = serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        let env = &result["mcpServers"]["aspis-management"]["env"];
        assert!(
            env.get("ASPIS_APP_BIN").is_none(),
            "ASPIS_APP_BIN must be absent when app_bin is None"
        );
        cleanup_project(&root);
    }

    // ---- X4: .pi/.gitignore guards machine-local mcp.json ------------------

    #[test]
    fn creates_gitignore_with_mcp_json_for_fresh_project() {
        let root = setup_project();

        ensure_project_pi_mcp_config(
            &root,
            "/usr/bin/python3",
            Path::new("/opt/management"),
            Path::new("/opt/projects"),
            None,
        )
        .unwrap();

        let gitignore = root.join(".pi").join(".gitignore");
        assert!(gitignore.exists(), ".pi/.gitignore must be created");
        let contents = fs::read_to_string(&gitignore).unwrap();
        let lines: Vec<&str> = contents.lines().map(|l| l.trim()).collect();
        assert!(
            lines.contains(&"mcp.json"),
            ".pi/.gitignore must contain a mcp.json line"
        );
        cleanup_project(&root);
    }

    #[test]
    fn appends_mcp_json_to_existing_gitignore_preserving_other_lines() {
        let root = setup_project();
        let pi_dir = root.join(".pi");
        fs::create_dir_all(&pi_dir).unwrap();
        let gitignore = pi_dir.join(".gitignore");
        fs::write(&gitignore, "settings.json\n*.log\n").unwrap();

        ensure_project_pi_mcp_config(
            &root,
            "/usr/bin/python3",
            Path::new("/opt/management"),
            Path::new("/opt/projects"),
            None,
        )
        .unwrap();

        let contents = fs::read_to_string(&gitignore).unwrap();
        let lines: Vec<&str> = contents.lines().map(|l| l.trim()).collect();
        assert!(
            lines.contains(&"settings.json"),
            "existing .gitignore lines must be preserved"
        );
        assert!(
            lines.contains(&"*.log"),
            "existing .gitignore lines must be preserved"
        );
        assert!(
            lines.contains(&"mcp.json"),
            "mcp.json must be appended to .gitignore"
        );
        // mcp.json appended at the end, after the original lines.
        let last = lines.last().expect("non-empty");
        assert_eq!(*last, "mcp.json", "mcp.json must be the final line");
        cleanup_project(&root);
    }

    #[test]
    fn leaves_existing_gitignore_with_mcp_json_unchanged() {
        let root = setup_project();
        let pi_dir = root.join(".pi");
        fs::create_dir_all(&pi_dir).unwrap();
        let gitignore = pi_dir.join(".gitignore");
        let original = "settings.json\nmcp.json\n";
        fs::write(&gitignore, original).unwrap();

        ensure_project_pi_mcp_config(
            &root,
            "/usr/bin/python3",
            Path::new("/opt/management"),
            Path::new("/opt/projects"),
            None,
        )
        .unwrap();

        let contents = fs::read_to_string(&gitignore).unwrap();
        assert_eq!(
            contents, original,
            "gitignore already containing mcp.json must be unchanged"
        );
        cleanup_project(&root);
    }

    #[test]
    fn leaves_gitignore_untouched_on_invalid_utf8() {
        let root = setup_project();
        let pi_dir = root.join(".pi");
        fs::create_dir_all(&pi_dir).unwrap();
        let gitignore = pi_dir.join(".gitignore");
        // Invalid UTF-8 bytes: read_to_string returns InvalidData, NOT NotFound.
        // The file likely exists with the user's rules, so we must not overwrite it.
        let original: Vec<u8> = vec![0xFF, 0xFE, 0x0A];
        fs::write(&gitignore, &original).unwrap();

        ensure_project_pi_mcp_config(
            &root,
            "/usr/bin/python3",
            Path::new("/opt/management"),
            Path::new("/opt/projects"),
            None,
        )
        .unwrap();

        let bytes = fs::read(&gitignore).unwrap();
        assert_eq!(
            bytes, original,
            "gitignore with invalid UTF-8 must be left untouched (fail-soft)"
        );
        cleanup_project(&root);
    }
}
