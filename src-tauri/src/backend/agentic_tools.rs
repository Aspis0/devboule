//! Sandboxed tool executor for the agentic loop (Phase 6c, read-only half).
//!
//! Implements `AgentTools` confined to a scope ROOT. SECURITY: the LLM is semi-trusted and
//! its tool arguments are hostile input. Two layers keep every read inside the scope:
//!  1. `safe_rel_path` — pure: rejects absolute paths + `..` traversal, drops `.` components.
//!  2. canonicalize-based checks — a resolved path (and every grep-walked entry) must stay
//!     under the canonical root; SYMLINKS are skipped so a symlinked dir/file inside the
//!     repo can never be followed out of scope (the classic exfiltration vector).
//! Read-only here (read_file / list_dir / grep); edit/write/run are a later chunk that must
//! NOT reuse `resolve` verbatim for writes (see the non-existent-path note on `resolve`).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::backend::agentic_loop::AgentTools;

const MAX_READ_BYTES: usize = 256 * 1024;
const MAX_GREP_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_GREP_DEPTH: usize = 50;
const MAX_GREP_FILES: usize = 2000;
const MAX_GREP_MATCHES: usize = 100;
const MAX_RUN_OUTPUT: usize = 64 * 1024;
/// Wall-clock cap for a single `run` tool invocation (F42).
/// Builds/tests need headroom, but multi-minute server hangs (e.g. `python -m http.server`)
/// must not block the agentic round indefinitely. Process group is killed on expiry.
const RUN_TIMEOUT_SECS: u64 = 180;

/// PURE security core: normalize a model-supplied relative path and reject anything that
/// could escape the scope (absolute, drive/scheme `:`, `..`). `.` and empty components are
/// dropped. Returns `"."` for the scope root. NO filesystem access.
pub fn safe_rel_path(rel: &str) -> Result<String, String> {
    let s = rel.trim();
    if s.is_empty() {
        return Err("empty path".to_string());
    }
    if s.starts_with('/') || s.contains(':') {
        return Err("absolute paths not allowed".to_string());
    }
    let mut normalized = s.replace('\\', "/");
    while normalized.contains("//") {
        normalized = normalized.replace("//", "/");
    }
    // Re-check for a leading slash AFTER backslash→slash normalization (e.g. "\foo").
    if normalized.starts_with('/') {
        return Err("absolute paths not allowed".to_string());
    }
    let mut parts: Vec<&str> = Vec::new();
    for comp in normalized.split('/') {
        match comp {
            "" | "." => continue,                              // drop empty + current-dir
            ".." => return Err("path escapes scope".to_string()),
            c => parts.push(c),
        }
    }
    if parts.is_empty() {
        return Ok(".".to_string()); // resolved to the scope root
    }
    Ok(parts.join("/"))
}

/// LANGUAGE-AGNOSTIC dev/build/test programs the `run` tool may execute. Running the
/// PROJECT's own build/test via these (cargo build.rs, npm/make/gradle scripts, pytest, …)
/// runs project code by design — that's the agent's purpose, accepted uniformly across
/// languages. The security boundary is NOT "which tool" but: no shell-chaining, no scope
/// escape, env hygiene, resource bounds (see `parse_run_command` + `ScopedAgentTools::run`).
const RUN_PROGRAMS: &[&str] = &[
    // Rust
    "cargo", "rustc", "rustfmt", "clippy-driver",
    // Go
    "go", "gofmt", "golangci-lint",
    // C / C++ / native
    "make", "cmake", "ninja", "ctest", "meson",
    // JVM
    "gradle", "./gradlew", "mvn", "./mvnw",
    // .NET
    "dotnet",
    // JS / TS
    "npm", "npx", "yarn", "pnpm", "node", "deno", "bun", "tsc", "eslint", "vitest", "jest",
    "biome",
    // Python
    "python", "python3", "pytest", "tox", "ruff", "mypy", "pip", "pip3", "poetry", "uv",
    "black", "flake8",
    // Ruby
    "ruby", "rake", "rspec", "bundle", "rubocop",
    // PHP
    "php", "composer", "phpunit",
    // Swift / others
    "swift", "zig", "dart", "flutter", "elixir", "mix",
];

/// Package-manager script names that typically start a listen-forever / watch loop.
const BLOCKING_PM_SCRIPTS: &[&str] = &["dev", "start", "serve", "watch"];

/// True if `name` is a blocking PM script (exact, or prefix + `:`/`-`, case-insensitive).
/// `dev:api` / `serve-docs` block; `build:watch` does not (prefix only on the blocking base).
fn is_blocking_script_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    for base in BLOCKING_PM_SCRIPTS {
        if lower == *base {
            return true;
        }
        if lower.starts_with(&format!("{base}:")) || lower.starts_with(&format!("{base}-")) {
            return true;
        }
    }
    false
}

/// True if any token is a watch-mode flag. `-w` only counts when `prog` is `tsc`
/// (for npm, `-w` means workspace). `--watchAll=false` is allowed; bare/`=true` block.
fn has_watch_flag(prog: &str, tokens: &[String]) -> bool {
    tokens.iter().any(|t| {
        if t == "--watch" || t == "--watch-all" || t.starts_with("--watch=") {
            return true;
        }
        if t == "--watchAll" || t == "--watchAll=true" {
            return true;
        }
        // Jest: `--watchAll=false` stays allowed (not matched above).
        if t == "-w" && prog == "tsc" {
            return true;
        }
        false
    })
}

/// First npm subcommand token, skipping flags and value-taking options (`-w pkg`, `--prefix x`).
fn first_npm_subcommand(tokens: &[String]) -> Option<&str> {
    let mut i = 1usize;
    while i < tokens.len() {
        let s = tokens[i].as_str();
        // Flags that consume the next token as their value.
        if s == "-w" || s == "--workspace" || s == "--prefix" || s == "-C" {
            i += 2;
            continue;
        }
        if s.starts_with('-') {
            i += 1;
            continue;
        }
        return Some(s);
    }
    None
}

/// True when a package-manager invocation names a blocking forever script.
/// Locates the `run` token by position (so `npm -w pkg run dev` is caught).
fn is_blocking_pm_script(prog: &str, tokens: &[String]) -> bool {
    match prog {
        // `npm start` | `npm run dev|…` | `npm -w pkg run dev` | `npm --prefix x run dev`
        "npm" => {
            // Bare lifecycle `start` only as the subcommand (first non-flag after npm,
            // skipping `-w pkg` etc.). Positional `start` after `run build` stays allowed.
            if first_npm_subcommand(tokens).is_some_and(|c| c.eq_ignore_ascii_case("start")) {
                return true;
            }
            if let Some(i) = tokens.iter().position(|t| t.eq_ignore_ascii_case("run")) {
                if let Some(script) = tokens.get(i + 1) {
                    return is_blocking_script_name(script);
                }
            }
            false
        }
        // `yarn|pnpm|bun dev` | `yarn|pnpm|bun run start` (bare scripts + run form)
        "yarn" | "pnpm" | "bun" => {
            if let Some(i) = tokens.iter().position(|t| t.eq_ignore_ascii_case("run")) {
                if let Some(script) = tokens.get(i + 1) {
                    if is_blocking_script_name(script) {
                        return true;
                    }
                }
            }
            // Bare script as first non-flag after prog (`bun dev`, `yarn start`).
            if let Some(cmd) = tokens.iter().skip(1).find(|t| !t.starts_with('-')) {
                if !cmd.eq_ignore_ascii_case("run") && is_blocking_script_name(cmd) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// Bare `vitest` defaults to watch mode; only an explicit `run` subcommand is finite.
fn is_blocking_vitest_invocation(tokens: &[String], vitest_idx: usize) -> bool {
    let rest = &tokens[vitest_idx + 1..];
    match rest.iter().find(|t| !t.starts_with('-')) {
        None => true, // bare / only flags → watch default
        Some(sub) if sub.eq_ignore_ascii_case("run") => false,
        Some(sub) if sub.eq_ignore_ascii_case("watch") => true,
        Some(_) => false,
    }
}

/// F42: long-running server / listen-forever patterns the agent must not start via `run`.
/// These hang the tool until timeout and can squat reserved ports (e.g. 8000 = oMLX).
pub fn is_blocking_server_run(tokens: &[String]) -> bool {
    if tokens.is_empty() {
        return false;
    }
    let prog = tokens[0].as_str();

    // Runner-prefix bypass: `uv|poetry run <cmd…>` → re-check the inner command.
    if matches!(prog, "uv" | "poetry")
        && tokens
            .get(1)
            .is_some_and(|t| t.eq_ignore_ascii_case("run"))
        && tokens.len() > 2
    {
        return is_blocking_server_run(&tokens[2..]);
    }

    // Watch-mode flags on any allowlisted program (`tsc --watch`, `jest --watchAll`, …).
    if has_watch_flag(prog, tokens) {
        return true;
    }

    // Package-manager forever scripts (`npm run dev`, `bun dev`, `npm run dev:api`, …).
    if is_blocking_pm_script(prog, tokens) {
        return true;
    }

    // Bare forever-servers as the program itself (e.g. after `poetry run uvicorn`).
    if matches!(prog, "uvicorn" | "gunicorn" | "waitress") {
        return true;
    }

    // `python -m http.server` / `python3 -m http.server` / versioned `python3.12`, SimpleHTTPServer
    // plus ASGI/WSGI forever servers and `flask run`.
    let is_python = prog == "python"
        || prog == "python3"
        || (prog.starts_with("python3.") && prog.len() > "python3.".len());
    if is_python {
        let joined = tokens.join(" ");
        if joined.contains("http.server") || joined.contains("SimpleHTTPServer") {
            return true;
        }
        if let Some(m_idx) = tokens.iter().position(|t| t == "-m") {
            if let Some(mod_name) = tokens.get(m_idx + 1).map(|s| s.to_ascii_lowercase()) {
                match mod_name.as_str() {
                    "uvicorn" | "gunicorn" | "waitress" => return true,
                    "flask"
                        if tokens
                            .get(m_idx + 2)
                            .is_some_and(|t| t.eq_ignore_ascii_case("run")) =>
                    {
                        return true;
                    }
                    _ => {}
                }
            }
        }
    }

    // npx forever-servers.
    if prog == "npx" {
        if tokens.iter().any(|t| {
            t.eq_ignore_ascii_case("serve")
                || t.eq_ignore_ascii_case("http-server")
                || t.eq_ignore_ascii_case("nodemon")
                || t.eq_ignore_ascii_case("json-server")
        }) {
            return true;
        }
        if tokens.iter().any(|t| t.eq_ignore_ascii_case("vite"))
            && !tokens.iter().any(|t| t.eq_ignore_ascii_case("build"))
        {
            return true;
        }
        if tokens.windows(2).any(|w| {
            w[0].eq_ignore_ascii_case("next")
                && (w[1].eq_ignore_ascii_case("dev") || w[1].eq_ignore_ascii_case("start"))
        }) {
            return true;
        }
        if tokens
            .windows(2)
            .any(|w| w[0].eq_ignore_ascii_case("webpack") && w[1].eq_ignore_ascii_case("serve"))
        {
            return true;
        }
        if let Some(i) = tokens.iter().position(|t| t.eq_ignore_ascii_case("vitest")) {
            if is_blocking_vitest_invocation(tokens, i) {
                return true;
            }
        }
    }

    // Common forever-servers: `php -S`, `ruby -run`, `flutter run`.
    if prog == "php" && tokens.iter().any(|t| t == "-S") {
        return true;
    }
    if prog == "ruby" && tokens.iter().any(|t| t == "-run") {
        return true;
    }
    if prog == "flutter" && tokens.iter().any(|t| t.eq_ignore_ascii_case("run")) {
        return true;
    }
    // `deno serve` as the subcommand only (`deno task serve` stays allowed).
    if prog == "deno"
        && tokens
            .get(1)
            .is_some_and(|t| t.eq_ignore_ascii_case("serve"))
    {
        return true;
    }
    // Bare `vitest` / `vitest watch` block; `vitest run` allowed.
    if prog == "vitest" && is_blocking_vitest_invocation(tokens, 0) {
        return true;
    }

    false
}

/// PURE RCE gate for the `run` tool: validate a command into an argv vector for a NO-SHELL
/// exec. (1) rejects shell metacharacters (no chaining/substitution/redirection — also defangs
/// `python -c "…"` etc. since quotes/parens are blocked); (2) requires the program (token 0) to
/// be a known dev/build/test tool — LANGUAGE-AGNOSTIC, not a Rust/JS-only pair list; (3) safe
/// charset on every token; (4) blocks scope-escape in args (parent-`..` segments, absolute
/// paths, `--flag=/abs`). Running the project's own build/test is accepted; this stops shell
/// injection + escaping the scope root, not the project running its own code.
pub fn parse_run_command(input: &str) -> Result<Vec<String>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Input is empty".to_string());
    }

    const META: &[char] = &[
        ';', '|', '&', '$', '`', '>', '<', '(', ')', '{', '}', '[', ']', '!', '*', '?', '~',
        '\\', '"', '\'', '\n', '\r', '\t',
    ];
    if let Some(c) = input.chars().find(|c| META.contains(c)) {
        return Err(format!("Forbidden shell metacharacter: {c:?}"));
    }

    let tokens: Vec<String> = trimmed.split_whitespace().map(|s| s.to_string()).collect();
    if tokens.is_empty() {
        return Err("No command".to_string());
    }
    if !RUN_PROGRAMS.contains(&tokens[0].as_str()) {
        return Err(format!("Program not in the dev-tool allowlist: {}", tokens[0]));
    }
    // F42: reject listen-forever servers before spawn (timeout alone still burns minutes).
    if is_blocking_server_run(&tokens) {
        return Err(
            "Blocking/server command is not allowed via run (use a finite test/build command; \
             long-lived servers hang the agent and may occupy reserved ports)"
                .to_string(),
        );
    }

    for (i, token) in tokens.iter().enumerate() {
        if !token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '=' | '-'))
        {
            return Err(format!("Token has an invalid character: {token}"));
        }
        if i > 0 {
            // Scope-escape: a `..` PATH SEGMENT (not the literal substring — keeps Go's
            // `./...`), an absolute path, or a `--flag=/abs` value.
            let as_path = token.replace('=', "/");
            if as_path.split('/').any(|seg| seg == "..") {
                return Err(format!("Parent-dir traversal not allowed in arg: {token}"));
            }
            if token.starts_with('/') || token.contains("=/") {
                return Err(format!("Absolute path not allowed in arg: {token}"));
            }
            // Absolute path ATTACHED to a flag with no '=' (e.g. `make -C/etc`,
            // `go build -o/tmp/x`, `rustc -o/abs`): strip the leading flag chars, and if the
            // remainder is an absolute path, reject (it escapes the scope cwd).
            if token.starts_with('-') {
                let rest = token.trim_start_matches(|c: char| c != '/');
                if rest.starts_with('/') {
                    return Err(format!("Absolute path embedded in flag not allowed: {token}"));
                }
            }
        }
    }

    Ok(tokens)
}

pub struct ScopedAgentTools {
    root: PathBuf,
    /// Relative paths the loop wrote/edited — feeds the result file's `filesTouched`.
    touched: Vec<String>,
    /// If non-empty, WRITES (write_file/edit_file) are restricted to these NORMALIZED relative
    /// paths (the directive's file scope). Reads stay project-wide for context. Empty = no
    /// extra restriction beyond the scope root.
    write_allowlist: Vec<String>,
    /// Network policy for the OS sandbox around `run` commands. Default `None` (deny). The caller
    /// sets it to `Enabled` for a project the user unblocked after a network-blocked failure
    /// (per-project flag). `Loopback` is unused here for now.
    net: crate::backend::sandbox::NetPolicy,
    /// Set to `true` when `looks_network_blocked` fired during a `run` call while
    /// `net == NetPolicy::None`. Surfaced as `net_blocked()` so `run_agentic_coder` can
    /// propagate the signal up to `claim_and_launch` (which has the `AppHandle` needed
    /// to emit the consent-request event). Never set when `net == Enabled` (no false
    /// positives on already-unblocked projects).
    net_blocked: bool,
    /// Broker Slice 2: extra writable folders OUTSIDE the project root that the user has
    /// persistently or transiently granted. Stored as CANONICALIZED absolute paths so
    /// the "is this path in the working set?" check is a simple `starts_with` after
    /// canonicalizing the target.  Empty = only the project root is writable.
    working_set: Vec<PathBuf>,
    /// Broker Slice 2: when a write tool targets a path that canonicalizes OUTSIDE BOTH
    /// `root` AND `working_set`, this field is set to the CANONICALIZED parent folder of
    /// the denied target so `claim_and_launch` can emit a `FolderWrite` consent-request.
    ///
    /// CRITICAL DISTINCTION: a write inside the project root but outside the per-task
    /// `write_allowlist` is a task-scope error — it does NOT set this signal.  Only paths
    /// genuinely outside (root + working_set) trigger the folder-consent flow.
    out_of_scope_write: Option<String>,
    /// A3: when `Some`, the read-only `oracle_ask` tool is enabled, scoped to these allowed
    /// file_ids (the mini's project's indexed files). `None` = the tool is unavailable.
    oracle_scope: Option<Vec<String>>,
}

impl ScopedAgentTools {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            touched: Vec::new(),
            write_allowlist: Vec::new(),
            net: crate::backend::sandbox::NetPolicy::None,
            net_blocked: false,
            working_set: Vec::new(),
            out_of_scope_write: None,
            oracle_scope: None,
        }
    }

    /// Restrict WRITES to `files` (the directive's allowlist). Empty leaves writes confined
    /// only by the scope root.
    pub fn with_write_allowlist(mut self, files: Vec<String>) -> Self {
        // Normalize through safe_rel_path so the allowlist matches what write_file/edit_file
        // compare against (e.g. a directive's "./src/a.rs" → "src/a.rs"). Drops invalid entries.
        self.write_allowlist = files.into_iter().filter_map(|f| safe_rel_path(&f).ok()).collect();
        self
    }

    /// Set the network policy for sandboxed `run` commands (default `None`). Used to unblock a
    /// project to `Enabled` after the user approves a network-blocked failure.
    pub fn with_net(mut self, net: crate::backend::sandbox::NetPolicy) -> Self {
        self.net = net;
        self
    }

    /// Broker Slice 2: set the effective working set — extra ABSOLUTE folders (canonicalized by
    /// the caller) outside the project root that the user has explicitly granted write access to.
    /// Only absolute paths that actually exist on disk at call time are accepted; others are
    /// silently dropped (a stale grant for a deleted folder must not error the spawn).
    pub fn with_working_set(mut self, folders: Vec<PathBuf>) -> Self {
        self.working_set = folders
            .into_iter()
            .filter_map(|p| p.canonicalize().ok())
            .collect();
        self
    }

    /// A3: enable the read-only `oracle_ask` tool, scoped to these allowed file_ids (the mini's
    /// project's indexed files, resolved by `oracle_agent_scope_file_ids`). An empty Vec still
    /// enables the tool but the bounded endpoint returns a grounded-empty answer (no documents).
    pub fn with_oracle(mut self, allowed_file_ids: Vec<String>) -> Self {
        self.oracle_scope = Some(allowed_file_ids);
        self
    }

    /// The (deduped) relative paths written/edited so far.
    pub fn touched(&self) -> &[String] {
        &self.touched
    }

    /// True if any `run` call detected a network-blocked failure while `net == None`.
    /// Used by `run_agentic_coder` to surface the signal for the consent-request event.
    pub fn net_blocked(&self) -> bool {
        self.net_blocked
    }

    /// Broker Slice 2: the canonicalized parent folder of the FIRST write attempt that
    /// targeted a path outside (root + working_set).  `None` if no such attempt happened.
    /// Used by `run_agentic_coder` to surface the FolderWrite consent-request signal.
    pub fn out_of_scope_write(&self) -> Option<&str> {
        self.out_of_scope_write.as_deref()
    }

    /// Whether writing `safe` (a normalized rel path) is permitted by the write allowlist.
    /// This is the TASK-SCOPE check (directive's file list), NOT the folder-consent check.
    fn write_allowed(&self, safe: &str) -> bool {
        self.write_allowlist.is_empty() || self.write_allowlist.iter().any(|f| f == safe)
    }

    /// Broker Slice 2 (app-level): write an ABSOLUTE path that must live under either `root`
    /// or one of the `working_set` folders.  Returns `Err` if out of scope, setting the
    /// `out_of_scope_write` signal to the canonicalized parent folder on the first violation.
    ///
    /// This method is called ONLY when the write target is expressed as an absolute path
    /// (i.e. from the working_set layer, not the relative-path tool layer).  The relative-path
    /// `write_file` / `edit_file` tools continue to use the existing `write_resolve` path.
    pub fn write_file_abs(&mut self, target: &Path, content: &str) -> Result<String, String> {
        if content.len() > MAX_WRITE_BYTES {
            return Err("content too large".to_string());
        }
        let canon_root = self.root.canonicalize().map_err(|_| "scope root not accessible".to_string())?;
        // The target must live under `root` OR under one of the working_set folders.
        let canon_target_parent = target
            .parent()
            .ok_or_else(|| "invalid path (no parent)".to_string())?;
        if !canon_target_parent.exists() {
            return Err("parent directory does not exist".to_string());
        }
        let canon_parent = canon_target_parent.canonicalize().map_err(|e| e.to_string())?;
        let in_root = canon_parent.starts_with(&canon_root);
        let in_working_set = self.working_set.iter().any(|ws| canon_parent.starts_with(ws));
        if !in_root && !in_working_set {
            // First violation: record the parent folder for the consent-request signal.
            if self.out_of_scope_write.is_none() {
                self.out_of_scope_write = Some(canon_parent.to_string_lossy().into_owned());
            }
            return Err(format!(
                "'{}' is outside the project root and the working set — folder consent required",
                target.display()
            ));
        }
        // FIX 1: for in-root paths, enforce the per-task write_allowlist (the same gate that
        // the relative write_file path uses).  Working-set paths are a distinct granted layer
        // and intentionally bypass the task allowlist — do NOT gate them here.
        //
        // We derive the relative path from the ALREADY-CANONICALIZED parent (which is verified
        // under canon_root above) + the raw filename.  We do NOT canonicalize the target itself
        // here because it may not exist yet (new file write) and we need the allowlist check
        // before any filesystem mutation.
        if in_root && !in_working_set {
            let fname = target
                .file_name()
                .ok_or_else(|| "invalid path (no filename)".to_string())?;
            let rel_dir = canon_parent
                .strip_prefix(&canon_root)
                .map_err(|_| "could not derive relative directory".to_string())?;
            let rel = if rel_dir == std::path::Path::new("") {
                std::path::PathBuf::from(fname)
            } else {
                rel_dir.join(fname)
            };
            let rel_str = rel.to_string_lossy();
            if !self.write_allowed(&rel_str) {
                return Err(format!("'{rel_str}' is outside this task's write scope"));
            }
        }
        // BLOCKER 3: unconditional symlink check — must fire even for DANGLING symlinks
        // (a dangling symlink has exists()==false so an exists()-guarded check misses it).
        // Strategy: inspect the symlink_metadata of the target itself:
        //   - is_symlink() → always reject (live or dangling)
        //   - NotFound      → ok (genuinely new file)
        //   - other Err     → reject (fail-safe)
        // TODO(security): residual TOCTOU — a setsid grandchild could swap the leaf for a
        // symlink between this check and fs::write below.  Structural fix: O_NOFOLLOW/cap-std.
        // Acceptable on a single-user desktop; must be addressed before any multi-tenant use.
        match std::fs::symlink_metadata(target) {
            Ok(m) if m.is_symlink() => return Err("refusing to write through a symlink".to_string()),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("cannot stat target: {e}"));
            }
            _ => {} // NotFound (new file) or not a symlink — proceed
        }
        // BLOCKER 4: hardlink check (parity with write_resolve's nlink() > 1 guard).
        // A hardlink shares an inode with a possibly out-of-scope file; canonicalize
        // cannot detect inode aliasing, so we check nlink() directly.
        #[cfg(unix)]
        if target.exists() {
            use std::os::unix::fs::MetadataExt;
            if fs::metadata(target).map(|m| m.nlink() > 1).unwrap_or(true) {
                return Err("refusing to write to a hardlinked file".to_string());
            }
        }
        sanity_check_written(target, content)?;
        std::fs::write(target, content).map_err(|e| e.to_string())?;
        // FIX 2: record the write so the Censor audit (files_touched / verdict_fn) sees it.
        // Use the relative path for in-root writes (consistent with the relative branch);
        // use the absolute path string for working-set writes so the audit at least records it.
        // We reuse canon_parent (already computed) to build the relative path safely.
        let touch_key = if in_root && !in_working_set {
            if let (Some(fname), Ok(rel_dir)) = (
                target.file_name(),
                canon_parent.strip_prefix(&canon_root),
            ) {
                let rel = if rel_dir == std::path::Path::new("") {
                    std::path::PathBuf::from(fname)
                } else {
                    rel_dir.join(fname)
                };
                rel.to_string_lossy().into_owned()
            } else {
                target.to_string_lossy().into_owned()
            }
        } else {
            target.to_string_lossy().into_owned()
        };
        self.record_touched(touch_key);
        Ok(format!("wrote {} bytes to {}", content.len(), target.display()))
    }

    fn canon_root(&self) -> Result<PathBuf, String> {
        self.root
            .canonicalize()
            .map_err(|_| "scope root is not accessible".to_string())
    }

    /// Resolve a scoped relative path to an absolute path under `root`, verifying (when the
    /// path or its nearest existing ancestor exists) that it canonicalizes UNDER the root.
    /// NOTE: for a fully non-existent path this returns the lexical join WITHOUT a canonical
    /// check — safe for reads (a missing file just fails to open), but the WRITE chunk must
    /// NOT trust this case (refuse, or descend with cap-std).
    fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
        let safe = safe_rel_path(rel)?;
        let full = self.root.join(&safe);
        let canon_root = self.canon_root()?;
        let to_check: Option<PathBuf> = if full.exists() {
            Some(full.clone())
        } else {
            full.parent().filter(|p| p.exists()).map(Path::to_path_buf)
        };
        if let Some(p) = to_check {
            let canon = p.canonicalize().map_err(|e| format!("path could not be resolved: {e}"))?;
            if !canon.starts_with(&canon_root) {
                return Err("path escapes scope".to_string());
            }
        }
        Ok(full)
    }

    fn read_file(&self, path: &str) -> Result<String, String> {
        let p = self.resolve(path)?;
        let content = fs::read_to_string(&p).map_err(|e| e.to_string())?;
        if content.len() <= MAX_READ_BYTES {
            return Ok(content);
        }
        let mut end = MAX_READ_BYTES;
        while end > 0 && !content.is_char_boundary(end) {
            end -= 1;
        }
        Ok(format!("{}\n…[truncated]", &content[..end]))
    }

    fn list_dir(&self, path: &str) -> Result<String, String> {
        let p = self.resolve(path)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(&p).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            // file_type() on a DirEntry is lstat — reports the symlink itself, not its target.
            let ft = entry.file_type().map_err(|e| e.to_string())?;
            let suffix = if ft.is_symlink() {
                "@" // mark symlinks; they are not followed
            } else if ft.is_dir() {
                "/"
            } else {
                ""
            };
            entries.push(format!("{}{}", entry.file_name().to_string_lossy(), suffix));
            if entries.len() >= 500 {
                break;
            }
        }
        entries.sort();
        Ok(entries.join("\n"))
    }

    fn grep(&self, pattern: &str, path: &str) -> Result<String, String> {
        let start = self.resolve(path)?;
        let canon_root = self.canon_root()?;
        let mut matches = Vec::new();
        let mut files = 0usize;
        walk_grep(&start, &self.root, &canon_root, pattern, 0, &mut matches, &mut files);
        Ok(matches.join("\n"))
    }
}

const MAX_WRITE_BYTES: usize = 1024 * 1024;

/// Detect content that is a single logical line stuffed with uninterpreted `\n` escape
/// sequences instead of real newlines (LLM double-escape corruption). Safe against normal
/// multi-line files and one-liners that contain the two-char string `\n` inside literals:
/// only fires when there are zero real newlines AND at least two literal `\n` sequences.
/// Intentionally narrow: a single literal `\n`, or corruption plus a trailing real newline,
/// is left to the per-language syntax gate.
fn has_unescaped_literal_newlines(content: &str) -> bool {
    let real_nl = content.bytes().filter(|&b| b == b'\n').count();
    let lit_nl = content.matches("\\n").count();
    real_nl == 0 && lit_nl >= 2
}

/// Source-code extensions where the language-agnostic escape guard may fire.
/// Data/minified formats (json, css, map, txt, md, …) legitimately embed `\\n` on one line.
/// Caller should pass a lowercased extension (no leading dot).
fn is_code_ext(ext: &str) -> bool {
    matches!(
        ext,
        "py" | "rs" | "js" | "ts" | "jsx" | "tsx" | "mjs" | "cjs" | "go" | "java"
            | "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "rb" | "php" | "swift" | "kt"
            | "kts" | "scala" | "sh" | "bash" | "zsh" | "lua" | "pl" | "pm" | "r" | "dart"
    )
}

/// Pre-write sanity gate. Called on the FINAL bytes about to hit the disk, before `fs::write`.
/// Rejects (1) double-escaped newline corruption on source-code extensions only, and
/// (2) invalid Python syntax for `.py`/`.pyi`/`.pyw` (best-effort via system `python3`;
/// skipped if unavailable or if the checker hangs past a short deadline).
fn sanity_check_written(path: &Path, content: &str) -> Result<(), String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    // Escape guard: code extensions only (skip json/css/map/txt/md/… and unknown).
    if is_code_ext(&ext) && has_unescaped_literal_newlines(content) {
        let lit_nl = content.matches("\\n").count();
        return Err(format!(
            "refused: content has {lit_nl} literal \\n escape sequences and no real newlines — emit real newlines, not escaped ones"
        ));
    }

    // Best-effort Python syntax gate. Skip if python3 is missing or hangs (~3s budget).
    if matches!(ext.as_str(), "py" | "pyi" | "pyw") {
        use std::io::{Read, Write};
        use std::time::{Duration, Instant};

        let mut child = match std::process::Command::new("python3")
            .args(["-c", "import ast,sys; ast.parse(sys.stdin.read())"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return Ok(()),
        };

        // Feed content, then drop stdin so python's sys.stdin.read() gets EOF.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(content.as_bytes());
        }
        // Take stderr before waiting; read only after the child has exited.
        let mut stderr_pipe = match child.stderr.take() {
            Some(s) => s,
            None => return Ok(()),
        };

        let deadline = Instant::now() + Duration::from_secs(3);
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        // Hung/slow checker must not block writes — skip like python-missing.
                        let _ = child.kill();
                        let _ = child.wait();
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(_) => return Ok(()),
            }
        };

        let mut stderr_buf = String::new();
        let _ = stderr_pipe.read_to_string(&mut stderr_buf);

        if !status.success() {
            let trimmed = stderr_buf.trim();
            let mut end = trimmed.len().min(400);
            while end > 0 && !trimmed.is_char_boundary(end) {
                end -= 1;
            }
            return Err(format!(
                "refused: Python syntax error in {}: {}",
                path.display(),
                &trimmed[..end]
            ));
        }
    }
    Ok(())
}

impl ScopedAgentTools {
    /// WRITE-safe resolver (stricter than `resolve`): the parent dir MUST exist and
    /// canonicalize UNDER the root (blocks writing through a symlinked-out ancestor), and
    /// an existing target must NOT be a symlink (blocks overwriting through a symlink-out).
    fn write_resolve(&self, rel: &str) -> Result<PathBuf, String> {
        let safe = safe_rel_path(rel)?;
        if safe == "." {
            return Err("cannot write to the scope root".to_string());
        }
        let full = self.root.join(&safe);
        let parent = full.parent().ok_or_else(|| "invalid path".to_string())?;
        if !parent.exists() {
            return Err("parent directory does not exist".to_string());
        }
        let canon_root = self.canon_root()?;
        let canon_parent = parent.canonicalize().map_err(|e| e.to_string())?;
        if !canon_parent.starts_with(&canon_root) {
            return Err("path escapes scope".to_string());
        }
        if full.exists() {
            if fs::symlink_metadata(&full).map(|m| m.is_symlink()).unwrap_or(true) {
                return Err("refusing to write through a symlink".to_string());
            }
            let canon_full = full.canonicalize().map_err(|e| e.to_string())?;
            if !canon_full.starts_with(&canon_root) {
                return Err("path escapes scope".to_string());
            }
            // A hardlink (nlink > 1) shares an inode with a possibly out-of-scope file;
            // canonicalize resolves dir symlinks, NOT inode aliases, so it can't detect it.
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if fs::metadata(&full).map(|m| m.nlink() > 1).unwrap_or(true) {
                    return Err("refusing to write to a hardlinked file".to_string());
                }
            }
        }
        // Residual TOCTOU: a concurrent rename of the verified parent between this check and
        // the actual fs::write could defeat the scope (low risk in a single-user desktop app;
        // not the semi-trusted-LLM threat model). cap-std / openat(O_NOFOLLOW) is the
        // structural fix — a documented follow-up before multi-tenant use.
        Ok(full)
    }

    fn write_file(&mut self, path: &str, content: &str) -> Result<String, String> {
        if content.len() > MAX_WRITE_BYTES {
            return Err("content too large".to_string());
        }
        // BLOCKER 1: if the LLM supplies an absolute path, route through the abs layer which
        // checks (root ∪ working_set) and sets `out_of_scope_write` on a violation.
        if path.starts_with('/') {
            return self.write_file_abs(std::path::Path::new(path), content);
        }
        let safe = safe_rel_path(path)?;
        if !self.write_allowed(&safe) {
            return Err(format!("'{safe}' is outside this task's write scope"));
        }
        let p = self.write_resolve(path)?;
        sanity_check_written(&p, content)?;
        fs::write(&p, content).map_err(|e| e.to_string())?;
        self.record_touched(safe);
        Ok(format!("wrote {} bytes to {}", content.len(), p.display()))
    }

    fn edit_file(&mut self, path: &str, old: &str, new: &str) -> Result<String, String> {
        if old.is_empty() {
            return Err("old_string cannot be empty".to_string());
        }
        // BLOCKER 1: if the LLM supplies an absolute path, route through the abs layer.
        if path.starts_with('/') {
            return self.edit_file_abs(std::path::Path::new(path), old, new);
        }
        let safe = safe_rel_path(path)?;
        if !self.write_allowed(&safe) {
            return Err(format!("'{safe}' is outside this task's write scope"));
        }
        let p = self.write_resolve(path)?;
        if !p.exists() {
            return Err("file not found".to_string());
        }
        // OOM guard: don't read an arbitrarily large in-scope file into memory.
        if fs::metadata(&p).map(|m| m.len() > MAX_WRITE_BYTES as u64).unwrap_or(true) {
            return Err("file too large to edit".to_string());
        }
        let content = fs::read_to_string(&p).map_err(|e| e.to_string())?;
        let n = content.matches(old).count();
        if n == 0 {
            return Err("old_string not found".to_string());
        }
        if n > 1 {
            return Err(format!("old_string is not unique ({n} matches)"));
        }
        let final_content = content.replacen(old, new, 1);
        sanity_check_written(&p, &final_content)?;
        fs::write(&p, &final_content).map_err(|e| e.to_string())?;
        self.record_touched(safe);
        Ok(format!("edited {}", p.display()))
    }

    /// BLOCKER 1 helper: edit (old→new string replacement) at an absolute path that must
    /// live under (root ∪ working_set). Mirrors `write_file_abs` scope/symlink/hardlink
    /// checks then applies the replacement.
    fn edit_file_abs(&mut self, target: &std::path::Path, old: &str, new: &str) -> Result<String, String> {
        let canon_root = self.root.canonicalize().map_err(|_| "scope root not accessible".to_string())?;
        // Scope check: parent must be under root or a working_set folder.
        let parent = target.parent().ok_or_else(|| "invalid path (no parent)".to_string())?;
        if !parent.exists() {
            return Err("parent directory does not exist".to_string());
        }
        let canon_parent = parent.canonicalize().map_err(|e| e.to_string())?;
        let in_root = canon_parent.starts_with(&canon_root);
        let in_working_set = self.working_set.iter().any(|ws| canon_parent.starts_with(ws));
        if !in_root && !in_working_set {
            if self.out_of_scope_write.is_none() {
                self.out_of_scope_write = Some(canon_parent.to_string_lossy().into_owned());
            }
            return Err(format!(
                "'{}' is outside this project's writable scope — folder consent required",
                target.display()
            ));
        }
        // FIX 1: enforce the per-task write_allowlist for in-root absolute paths.
        // Working-set paths intentionally bypass the task allowlist — do NOT gate them.
        // Derive the relative path from the already-canonicalized parent + raw filename
        // (same technique as write_file_abs: avoids needing to canonicalize the target,
        // which may not exist yet when called from a new-file write path).
        if in_root && !in_working_set {
            let fname = target
                .file_name()
                .ok_or_else(|| "invalid path (no filename)".to_string())?;
            let rel_dir = canon_parent
                .strip_prefix(&canon_root)
                .map_err(|_| "could not derive relative directory".to_string())?;
            let rel = if rel_dir == std::path::Path::new("") {
                std::path::PathBuf::from(fname)
            } else {
                rel_dir.join(fname)
            };
            let rel_str = rel.to_string_lossy();
            if !self.write_allowed(&rel_str) {
                return Err(format!("'{rel_str}' is outside this task's write scope"));
            }
        }
        // Unconditional symlink check (BLOCKER 3 parity).
        // TODO(security): residual TOCTOU — a setsid grandchild could swap the leaf for a
        // symlink between this check and fs::write below.  Structural fix: O_NOFOLLOW/cap-std.
        // Acceptable on a single-user desktop; must be addressed before any multi-tenant use.
        match std::fs::symlink_metadata(target) {
            Ok(m) if m.is_symlink() => return Err("refusing to write through a symlink".to_string()),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("cannot stat target: {e}"));
            }
            _ => {} // NotFound (new file) or not a symlink — ok
        }
        if !target.exists() {
            return Err("file not found".to_string());
        }
        // Hardlink check (BLOCKER 4 parity).
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if fs::metadata(target).map(|m| m.nlink() > 1).unwrap_or(true) {
                return Err("refusing to write to a hardlinked file".to_string());
            }
        }
        // OOM guard.
        if fs::metadata(target).map(|m| m.len() > MAX_WRITE_BYTES as u64).unwrap_or(true) {
            return Err("file too large to edit".to_string());
        }
        let content = fs::read_to_string(target).map_err(|e| e.to_string())?;
        let n = content.matches(old).count();
        if n == 0 {
            return Err("old_string not found".to_string());
        }
        if n > 1 {
            return Err(format!("old_string is not unique ({n} matches)"));
        }
        let final_content = content.replacen(old, new, 1);
        sanity_check_written(target, &final_content)?;
        fs::write(target, &final_content).map_err(|e| e.to_string())?;
        // FIX 2: record the edit so the Censor audit (files_touched / verdict_fn) sees it.
        // Use the same canon_parent-based relative path derivation as FIX 1 above.
        let touch_key = if in_root && !in_working_set {
            if let (Some(fname), Ok(rel_dir)) = (
                target.file_name(),
                canon_parent.strip_prefix(&canon_root),
            ) {
                let rel = if rel_dir == std::path::Path::new("") {
                    std::path::PathBuf::from(fname)
                } else {
                    rel_dir.join(fname)
                };
                rel.to_string_lossy().into_owned()
            } else {
                target.to_string_lossy().into_owned()
            }
        } else {
            target.to_string_lossy().into_owned()
        };
        self.record_touched(touch_key);
        Ok(format!("edited {}", target.display()))
    }

    fn record_touched(&mut self, safe: String) {
        if !self.touched.contains(&safe) {
            self.touched.push(safe);
        }
    }

    /// Execute an allowlisted command (validated by `parse_run_command`) as an argv vector with
    /// NO shell, in the scope root. Drains stdout/stderr on threads (so a full pipe can't
    /// deadlock), kills on timeout, and returns capped output. The argv-exec + allowlist + safe
    /// charset are the RCE gate; the scope-root cwd confines side effects.
    fn run(&mut self, command: &str) -> Result<String, String> {
        let mut argv = parse_run_command(command)?;
        // F6: npx must not fetch+exec a REMOTE package — only locally-installed tools.
        // `--prefer-offline` is honored on npm 6 AND 7+ (uses cache only); combined with the
        // null stdin (no interactive install prompt) it won't pull a package from the network.
        if argv[0] == "npx" {
            argv.insert(1, "--prefer-offline".to_string());
        }

        // OS sandbox (macOS Seatbelt): confine writes to the scope root + deny network. The
        // app-level RCE gate (parse_run_command allowlist), env_clear, and process_group below
        // stay as defense-in-depth ON TOP of the OS sandbox. On non-macOS, wrap is a passthrough
        // (Windows lands in phase 3) and logs a warning.
        let policy = agentic_run_policy_with_working_set(&self.root, self.net.clone(), &self.working_set);
        let wrapped = crate::backend::sandbox::wrap(&policy, &argv[0], &argv[1..], &self.root);
        let mut cmd = std::process::Command::new(&wrapped.program);
        cmd.args(&wrapped.args)
            .current_dir(&self.root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        // F4: don't leak the app's env (API keys/tokens) to a project build script of ANY
        // language — clear it, pass only what build tools genuinely need.
        cmd.env_clear();
        for key in [
            "PATH", "HOME", "LANG", "LC_ALL", "TMPDIR", "USER", "SHELL", "CARGO_HOME",
            "RUSTUP_HOME", "GOPATH", "GOCACHE", "GOMODCACHE", "NODE_PATH", "JAVA_HOME",
            "ANDROID_HOME", "PYENV_ROOT", "VIRTUAL_ENV",
            // native-toolchain vars (else cargo crates like openssl-sys/libpq-sys fail to build)
            "CC", "CXX", "PKG_CONFIG_PATH", "OPENSSL_DIR", "LIBRARY_PATH", "LD_LIBRARY_PATH",
        ] {
            if let Ok(v) = std::env::var(key) {
                cmd.env(key, v);
            }
        }
        // F2: own process group so a timeout SIGKILLs the WHOLE tree (cargo→rustc→test bin,
        // npm→node, make→…). Killing only the direct child orphans descendants AND can deadlock
        // the output drain (a surviving child keeps the pipe write end open).
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        // C3 (rlimits): Seatbelt has no native rlimit — enforce CPU/addr-space/max-procs on the
        // spawned process (inherited by the child). Bounds a runaway/fork-bomb in a sandboxed run.
        crate::backend::sandbox::apply_rlimits(&mut cmd, &policy.rlimits);

        let mut child = cmd.spawn().map_err(|e| format!("failed to start '{}': {e}", argv[0]))?;
        let pid = child.id() as i32;

        // F1: drain each stream CAPPED (never read an unbounded firehose into memory → OOM).
        let mut out = child.stdout.take().expect("stdout piped");
        let mut err = child.stderr.take().expect("stderr piped");
        let (tx_o, rx_o) = std::sync::mpsc::channel();
        let (tx_e, rx_e) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx_o.send(drain_capped(&mut out));
        });
        std::thread::spawn(move || {
            let _ = tx_e.send(drain_capped(&mut err));
        });

        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(RUN_TIMEOUT_SECS);
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        kill_process_group(pid, &mut child);
                        break None;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => return Err(format!("run wait failed: {e}")),
            }
        };

        // Normally the exit/group-kill closes the pipe writers → the drains finish at once. But
        // a daemonized grandchild could setsid() out of the killed group and hold a pipe open;
        // bound the wait so run() can NEVER hang (a stuck drain thread is then abandoned).
        let jt = std::time::Duration::from_secs(5);
        let stdout = rx_o.recv_timeout(jt).unwrap_or_default();
        let stderr = rx_e.recv_timeout(jt).unwrap_or_default();
        let mut body = match status {
            Some(s) => format!("exit: {}\n", s.code().unwrap_or(-1)),
            None => format!("TIMEOUT after {RUN_TIMEOUT_SECS}s (process group killed)\n"),
        };
        body.push_str(&String::from_utf8_lossy(&stdout));
        if !stderr.is_empty() {
            body.push_str("\n--- stderr ---\n");
            body.push_str(&String::from_utf8_lossy(&stderr));
        }
        if body.len() > MAX_RUN_OUTPUT {
            let mut end = MAX_RUN_OUTPUT;
            while end > 0 && !body.is_char_boundary(end) {
                end -= 1;
            }
            body.truncate(end);
            body.push_str("\n…[truncated]");
        }
        if matches!(self.net, crate::backend::sandbox::NetPolicy::None) && looks_network_blocked(&body) {
            self.net_blocked = true;
            body.push_str(NETWORK_BLOCKED_HINT);
        }
        Ok(body)
    }
}

/// Build the sandbox policy for a `run` command (convenience wrapper used only in tests;
/// production code calls `agentic_run_policy_with_working_set` directly).
#[cfg(test)]
fn agentic_run_policy(
    root: &std::path::Path,
    net: crate::backend::sandbox::NetPolicy,
) -> crate::backend::sandbox::SandboxPolicy {
    agentic_run_policy_with_working_set(root, net, &[])
}

/// Broker Slice 2: build the sandbox policy including the project's effective working set.
/// Each `working_set` folder is added as an additional writable path (it is already
/// canonicalized by the caller — `ScopedAgentTools::with_working_set` canonicalizes on
/// insert, and `claim_and_launch` canonicalizes the resolved set before threading it in).
pub fn agentic_run_policy_with_working_set(
    root: &std::path::Path,
    net: crate::backend::sandbox::NetPolicy,
    working_set: &[PathBuf],
) -> crate::backend::sandbox::SandboxPolicy {
    let mut policy = crate::backend::sandbox::SandboxPolicy::deny(root.to_path_buf())
        .writable(root.to_path_buf())
        .net(net);
    for folder in working_set {
        policy = policy.writable(folder.clone());
    }
    policy
}

const NETWORK_BLOCKED_HINT: &str = "\n--- HINT: this command likely needs NETWORK access, which is \
DISABLED for this project's sandbox. If you trust it, enable network for this project and re-run. ---";

/// Heuristic: does `body` look like a command that failed because the sandbox denied network?
/// Used only when net == None to append a HINT (no false-positive risk: it only adds advice).
fn looks_network_blocked(body: &str) -> bool {
    let b = body.to_ascii_lowercase();
    // Only clearly network-specific strings. Deliberately EXCLUDES "operation not permitted"
    // (Seatbelt also returns it for WRITE denials → would wrongly suggest enabling network when
    // the real issue is a write restriction), "failed to get" (cargo lock contention), and
    // "network error"/"failed to download" (too ambiguous). (review F2)
    const NEEDLES: &[&str] = &[
        "could not resolve host", "couldn't resolve host", "name resolution", "getaddrinfo",
        "temporary failure in name resolution", "network is unreachable", "no route to host",
        "connection refused", "spurious network error", "etimedout", "enotfound", "econnrefused",
        "connection timed out", "could not connect", "tls handshake",
    ];
    NEEDLES.iter().any(|n| b.contains(n))
}

/// Read a child stream, keeping at most `MAX_RUN_OUTPUT` bytes but CONTINUING to read+discard
/// past the cap so the child's pipe never blocks (bounded memory, no writer deadlock).
fn drain_capped(r: &mut impl std::io::Read) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match r.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if buf.len() < MAX_RUN_OUTPUT {
                    let take = n.min(MAX_RUN_OUTPUT - buf.len());
                    buf.extend_from_slice(&chunk[..take]);
                }
            }
        }
    }
    buf
}

/// SIGKILL the child's whole process group (negative pid) on timeout, so descendants
/// (rustc/test-bin/node/…) die too — they can't orphan or hold the output pipes open. Also
/// kills + reaps the direct child (covers non-unix + belt-and-suspenders).
fn kill_process_group(pid: i32, child: &mut std::process::Child) {
    #[cfg(unix)]
    // SAFETY: kill(2) with a negative pid targets the process group; SIGKILL is async-safe.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[allow(clippy::too_many_arguments)]
fn walk_grep(
    dir: &Path,
    root: &Path,
    canon_root: &Path,
    pattern: &str,
    depth: usize,
    matches: &mut Vec<String>,
    files: &mut usize,
) {
    if depth > MAX_GREP_DEPTH || *files >= MAX_GREP_FILES || matches.len() >= MAX_GREP_MATCHES {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        if matches.len() >= MAX_GREP_MATCHES || *files >= MAX_GREP_FILES {
            return;
        }
        // SECURITY: never follow symlinks (skip on any file_type error too — fail safe).
        if entry.file_type().map(|t| t.is_symlink()).unwrap_or(true) {
            continue;
        }
        let path = entry.path();
        // Defense-in-depth: the real path must stay under the canonical root.
        match path.canonicalize() {
            Ok(c) if c.starts_with(canon_root) => {}
            _ => continue,
        }
        if path.is_dir() {
            walk_grep(&path, root, canon_root, pattern, depth + 1, matches, files);
        } else if path.is_file() {
            // OOM guard: skip files above the size cap rather than loading them.
            if fs::metadata(&path).map(|m| m.len() > MAX_GREP_FILE_BYTES).unwrap_or(true) {
                continue;
            }
            *files += 1;
            if let Ok(content) = fs::read_to_string(&path) {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
                for (i, line) in content.lines().enumerate() {
                    if line.contains(pattern) {
                        matches.push(format!("{}:{}: {}", rel, i + 1, line.trim_end()));
                        if matches.len() >= MAX_GREP_MATCHES {
                            return;
                        }
                    }
                }
            }
        }
    }
}

impl AgentTools for ScopedAgentTools {
    fn call(&mut self, name: &str, arguments: &str) -> Result<String, String> {
        let args: Value =
            serde_json::from_str(arguments).map_err(|e| format!("invalid tool arguments JSON: {e}"))?;
        match name {
            "read_file" => {
                let path = args["path"].as_str().ok_or_else(|| "missing 'path'".to_string())?;
                self.read_file(path)
            }
            "list_dir" => self.list_dir(args["path"].as_str().unwrap_or(".")),
            "grep" => {
                let pattern =
                    args["pattern"].as_str().ok_or_else(|| "missing 'pattern'".to_string())?;
                self.grep(pattern, args["path"].as_str().unwrap_or("."))
            }
            "write_file" => {
                let path = args["path"].as_str().ok_or_else(|| "missing 'path'".to_string())?;
                let content =
                    args["content"].as_str().ok_or_else(|| "missing 'content'".to_string())?;
                self.write_file(path, content)
            }
            "edit_file" => {
                let path = args["path"].as_str().ok_or_else(|| "missing 'path'".to_string())?;
                let old =
                    args["oldString"].as_str().ok_or_else(|| "missing 'oldString'".to_string())?;
                let new =
                    args["newString"].as_str().ok_or_else(|| "missing 'newString'".to_string())?;
                self.edit_file(path, old, new)
            }
            "run" => {
                let command =
                    args["command"].as_str().ok_or_else(|| "missing 'command'".to_string())?;
                self.run(command)
            }
            "oracle_ask" => match &self.oracle_scope {
                Some(scope) => {
                    let query = args["query"].as_str().unwrap_or("");
                    if query.trim().is_empty() {
                        return Err("oracle_ask requires a non-empty 'query'".to_string());
                    }
                    crate::oracle::python_oracle::oracle_agent_ask(query, scope)
                }
                None => Err("oracle_ask is not available for this run".to_string()),
            },
            other => Err(format!("tool not available: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agentic_run_policy_respects_net() {
        use crate::backend::sandbox::NetPolicy;
        let root = std::path::PathBuf::from("/some/project/root");
        let p_none = super::agentic_run_policy(&root, NetPolicy::None);
        assert_eq!(p_none.net, NetPolicy::None);
        assert_eq!(p_none.readonly_root, root);
        assert!(p_none.writable_paths.contains(&root), "scope root must be writable");
        let p_en = super::agentic_run_policy(&root, NetPolicy::Enabled);
        assert_eq!(p_en.net, NetPolicy::Enabled);
    }

    #[test]
    fn looks_network_blocked_detects_common_failures() {
        assert!(super::looks_network_blocked("error: could not resolve host: crates.io"));
        assert!(super::looks_network_blocked("npm ERR! network ETIMEDOUT"));
        assert!(super::looks_network_blocked("curl: (7) Connection refused"));
        assert!(!super::looks_network_blocked("test result: ok. 5 passed"));
        assert!(!super::looks_network_blocked("compile error: missing semicolon"));
    }

    #[test]
    fn has_unescaped_literal_newlines_blocks_corrupted_one_liner() {
        // Model double-escaped: literal backslash-n, no real newlines. Mirrors the real
        // llama-3.1-8b corruption (three defs collapsed onto one line = two literal `\n`).
        // The guard requires >= 2 literal `\n` with zero real newlines to avoid tripping on
        // a legit no-trailing-newline one-liner like `x = "a\nb"` (a single literal `\n`).
        assert!(has_unescaped_literal_newlines(
            "def a():pass\\ndef b():pass\\ndef c():pass"
        ));
    }

    #[test]
    fn has_unescaped_literal_newlines_allows_legit_multiline() {
        assert!(!has_unescaped_literal_newlines("def a():\n    pass\n"));
    }

    #[test]
    fn has_unescaped_literal_newlines_allows_string_literal_escape() {
        // One literal `\n` inside a string plus a real trailing newline — not the corruption pattern.
        assert!(!has_unescaped_literal_newlines("x = \"a\\nb\"\n"));
    }

    #[test]
    fn has_unescaped_literal_newlines_allows_empty() {
        assert!(!has_unescaped_literal_newlines(""));
    }

    #[test]
    fn has_unescaped_literal_newlines_allows_js_regex_with_real_newline() {
        assert!(!has_unescaped_literal_newlines("const re = /\\n/g;\n"));
    }

    #[test]
    fn is_code_ext_accepts_source() {
        assert!(is_code_ext("py"));
        assert!(is_code_ext("rs"));
        assert!(is_code_ext("js"));
    }

    #[test]
    fn is_code_ext_rejects_data_and_empty() {
        assert!(!is_code_ext("json"));
        assert!(!is_code_ext("css"));
        assert!(!is_code_ext("txt"));
        assert!(!is_code_ext(""));
    }

    #[test]
    fn sanity_check_written_blocks_corrupted_py_via_escape_guard() {
        // Corrupted one-liner trips the escape guard before python3 is spawned.
        let content = "def a():pass\\ndef b():pass\\ndef c():pass";
        let err = sanity_check_written(Path::new("x.py"), content).unwrap_err();
        assert!(err.contains("literal \\n"), "{err}");
    }

    #[test]
    fn sanity_check_written_allows_json_minified_escapes() {
        // Legitimate minified JSON with literal `\n` inside strings — escape guard skipped.
        let content = "{\"a\":\"x\\ny\",\"b\":\"u\\nv\"}";
        assert!(sanity_check_written(Path::new("x.json"), content).is_ok());
    }

    #[test]
    fn sanity_check_written_skips_escape_guard_for_txt() {
        let content = "def a():pass\\ndef b():pass\\ndef c():pass";
        assert!(sanity_check_written(Path::new("x.txt"), content).is_ok());
    }

    #[test]
    fn sanity_check_written_code_ext_case_insensitive() {
        // Extension lowercased before is_code_ext / python gate matching.
        let content = "def a():pass\\ndef b():pass\\ndef c():pass";
        let err = sanity_check_written(Path::new("x.PY"), content).unwrap_err();
        assert!(err.contains("literal \\n"), "{err}");
    }

    #[test]
    fn safe_rel_path_rejects_escapes() {
        for bad in ["../etc/passwd", "/abs/path", "a/../b", "", "C:\\Windows", "dir/..", "\\foo"] {
            assert!(safe_rel_path(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn safe_rel_path_normalizes_accepted() {
        assert_eq!(safe_rel_path("src/main.rs"), Ok("src/main.rs".to_string()));
        assert_eq!(safe_rel_path("./src/x"), Ok("src/x".to_string()));
        assert_eq!(safe_rel_path("././src/x"), Ok("src/x".to_string()));
        assert_eq!(safe_rel_path("dir//sub"), Ok("dir/sub".to_string()));
        assert_eq!(safe_rel_path("."), Ok(".".to_string()));
        assert_eq!(safe_rel_path("./"), Ok(".".to_string()));
    }

    #[test]
    fn parse_run_command_multilang_safe_and_escapes_blocked() {
        assert_eq!(parse_run_command("cargo test").unwrap(), vec!["cargo", "test"]);
        // allowed: dev/build/test tools ACROSS languages (running the project's own build/test)
        for ok in [
            "cargo build --release",
            "go test ./...", // the `..` check must NOT trip on `./...`
            "pytest",
            "make",
            "npm run build",
            "npx tsc --noEmit",
            "gradle test",
            "python -m pytest",
            "ruff check .",
        ] {
            assert!(parse_run_command(ok).is_ok(), "{ok} should be allowed");
        }
        // rejected: shell injection / chaining / substitution / redirection
        for bad in [
            "cargo test; rm -rf /",
            "cargo test && curl evil",
            "cargo test | sh",
            "cargo test `whoami`",
            "cargo test $(whoami)",
            "npx tsc > out",
            "cargo test\ncargo build",
        ] {
            assert!(parse_run_command(bad).is_err(), "{bad} must be rejected (injection)");
        }
        // rejected: program not a known dev tool
        for bad in ["rm -rf /", "git push", "bash script.sh", "curl x", "", "   "] {
            assert!(parse_run_command(bad).is_err(), "{bad} must be rejected (program)");
        }
        // rejected: scope escape via args (absolute paths, parent traversal, --flag=/abs)
        for bad in [
            "cargo test --manifest-path=/etc/x",
            "cargo build --target-dir=/tmp/e",
            "make -C ../../other",
            "go test ../../../etc",
            "make -C/etc",            // abs path attached to a flag (no '=')
            "go build -o/tmp/evil",   // -o<abs> output escape
        ] {
            assert!(parse_run_command(bad).is_err(), "{bad} must be rejected (escape)");
        }
        // F42: long-running servers rejected (even when program is allowlisted)
        for bad in [
            "python -m http.server 8000",
            "python3 -m http.server",
            "npx serve",
            "npx http-server",
            "php -S 127.0.0.1:8000",
        ] {
            let err = parse_run_command(bad).expect_err(&format!("{bad} must be rejected (server)"));
            assert!(
                err.contains("Blocking/server") || err.contains("server"),
                "{bad} err should mention server: {err}"
            );
        }
        // finite python still allowed
        assert!(parse_run_command("python -m pytest").is_ok());
    }

    fn toks(cmd: &str) -> Vec<String> {
        cmd.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn f42_is_blocking_server_run_detects_http_server() {
        assert!(is_blocking_server_run(&toks("python -m http.server 8000")));
        assert!(!is_blocking_server_run(&toks("python -m pytest")));
        // Package-manager forever scripts vs finite build/test.
        assert!(is_blocking_server_run(&toks("npm run dev")));
        assert!(!is_blocking_server_run(&toks("npm run build")));
        // Watch flags / vitest subcommands.
        assert!(is_blocking_server_run(&toks("tsc --watch")));
        assert!(!is_blocking_server_run(&toks("vitest run")));
        assert!(is_blocking_server_run(&toks("vitest watch")));
        // Python ASGI forever vs finite pytest module.
        assert!(is_blocking_server_run(&toks("python -m uvicorn")));
        assert!(!is_blocking_server_run(&toks("python -m pytest")));
        // Review-round gaps.
        assert!(is_blocking_server_run(&toks("bun dev")));
        assert!(is_blocking_server_run(&toks("npm RUN dev")));
        assert!(is_blocking_server_run(&toks("npx next start")));
        assert!(is_blocking_server_run(&toks("jest --watchAll")));
        assert!(!is_blocking_server_run(&toks("jest --watchAll=false")));
        assert!(is_blocking_server_run(&toks("tsc -w")));
        assert!(is_blocking_server_run(&toks("npm -w pkg run dev")));
        assert!(!is_blocking_server_run(&toks("npm -w pkg run build")));
        assert!(is_blocking_server_run(&toks("vitest")));
        assert!(!is_blocking_server_run(&toks("npx vitest run")));
        assert!(is_blocking_server_run(&toks("uv run python -m http.server")));
        assert!(is_blocking_server_run(&toks("poetry run uvicorn app")));
        assert!(!is_blocking_server_run(&toks("uv run pytest")));
        assert!(is_blocking_server_run(&toks("npm run dev:api")));
        assert!(!is_blocking_server_run(&toks("npm run build:watch")));
        assert!(!is_blocking_server_run(&toks("deno task serve")));
        assert!(is_blocking_server_run(&toks("deno serve mod.ts")));
        // `start` is only the npm subcommand, not a later positional arg.
        assert!(!is_blocking_server_run(&toks("npm run build start")));
        assert!(is_blocking_server_run(&toks("npm -w pkg start")));
    }

    // FS tests: build a temp scope with an INSIDE file and an OUTSIDE secret reachable only
    // via a symlink, then assert the sandbox never follows the symlink out (the F1 blocker).
    #[cfg(unix)]
    #[test]
    fn grep_does_not_follow_symlinks_out_of_scope() {
        use std::os::unix::fs::symlink;
        let base = std::env::temp_dir().join(format!("aspis_at_{}_{}", std::process::id(), line!()));
        let root = base.join("scope");
        let outside = base.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(root.join("inside.txt"), "INSIDE_MARKER here").unwrap();
        fs::write(outside.join("secret.txt"), "OUTSIDE_SECRET here").unwrap();
        symlink(&outside, root.join("link")).unwrap();

        let mut tools = ScopedAgentTools::new(root.clone());
        let inside = tools.call("grep", r#"{"pattern":"INSIDE_MARKER"}"#).unwrap();
        assert!(inside.contains("INSIDE_MARKER"), "should find in-scope content");
        let leaked = tools.call("grep", r#"{"pattern":"OUTSIDE_SECRET"}"#).unwrap();
        assert!(!leaked.contains("OUTSIDE_SECRET"), "must NOT follow the symlink out of scope");

        // read_file through the symlink must also be blocked.
        let via_link = tools.call("read_file", r#"{"path":"link/secret.txt"}"#);
        assert!(via_link.is_err(), "read via symlink-out must be refused");

        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn read_file_truncates_large_content() {
        let base = std::env::temp_dir().join(format!("aspis_at_{}_{}", std::process::id(), line!()));
        fs::create_dir_all(&base).unwrap();
        let big = "x".repeat(MAX_READ_BYTES + 5000);
        fs::write(base.join("big.txt"), &big).unwrap();
        let mut tools = ScopedAgentTools::new(base.clone());
        let out = tools.call("read_file", r#"{"path":"big.txt"}"#).unwrap();
        assert!(out.contains("…[truncated]"));
        assert!(out.len() < big.len());
        let _ = fs::remove_dir_all(&base);
    }
}

#[cfg(all(test, unix))]
mod write_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    // Unique temp dir per call (atomic counter, NOT line!() — tests run in parallel).
    fn unique_root() -> (PathBuf, PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("aspis_aw_{}_{}", std::process::id(), n));
        let root = tmp.join("root");
        let outside = tmp.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        (root, tmp)
    }

    #[test]
    fn write_then_read_roundtrip() {
        let (root, tmp) = unique_root();
        let mut tools = ScopedAgentTools::new(root);
        tools.call("write_file", r#"{"path":"test.txt","content":"hello world"}"#).unwrap();
        let back = tools.call("read_file", r#"{"path":"test.txt"}"#).unwrap();
        assert_eq!(back, "hello world");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn edit_replaces_unique_occurrence() {
        let (root, tmp) = unique_root();
        fs::write(root.join("edit.txt"), "original content").unwrap();
        let mut tools = ScopedAgentTools::new(root.clone());
        tools
            .call("edit_file", r#"{"path":"edit.txt","oldString":"original","newString":"updated"}"#)
            .unwrap();
        assert_eq!(fs::read_to_string(root.join("edit.txt")).unwrap(), "updated content");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn edit_rejects_non_unique() {
        let (root, tmp) = unique_root();
        fs::write(root.join("multi.txt"), "a a a").unwrap();
        let mut tools = ScopedAgentTools::new(root);
        let res = tools.call("edit_file", r#"{"path":"multi.txt","oldString":"a","newString":"b"}"#);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("not unique"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_through_symlink_out_is_refused() {
        let (root, tmp) = unique_root();
        std::os::unix::fs::symlink(tmp.join("outside"), root.join("link")).unwrap();
        let mut tools = ScopedAgentTools::new(root);
        let res = tools.call("write_file", r#"{"path":"link/escaped.txt","content":"data"}"#);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("escapes scope"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_to_hardlinked_file_is_refused() {
        let (root, tmp) = unique_root();
        let secret = tmp.join("outside").join("secret.txt");
        fs::write(&secret, "SECRET").unwrap();
        // A hardlink inside the scope sharing the outside file's inode.
        std::fs::hard_link(&secret, root.join("inside_link.txt")).unwrap();
        let mut tools = ScopedAgentTools::new(root);
        let res = tools.call("write_file", r#"{"path":"inside_link.txt","content":"clobber"}"#);
        assert!(res.is_err(), "writing a hardlinked file must be refused");
        assert!(res.unwrap_err().contains("hardlink"));
        // The outside inode must be untouched.
        assert_eq!(fs::read_to_string(&secret).unwrap(), "SECRET");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn touched_records_deduped_paths() {
        let (root, tmp) = unique_root();
        let mut tools = ScopedAgentTools::new(root);
        tools.call("write_file", r#"{"path":"test.txt","content":"hello"}"#).unwrap();
        tools.call("edit_file", r#"{"path":"test.txt","oldString":"hello","newString":"world"}"#).unwrap();
        assert_eq!(tools.touched(), &["test.txt".to_string()]); // written + edited → one entry
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_allowlist_blocks_out_of_scope_writes() {
        let (root, tmp) = unique_root();
        // "./allowed.txt" exercises normalization: it must match a write to "allowed.txt".
        let mut tools =
            ScopedAgentTools::new(root).with_write_allowlist(vec!["./allowed.txt".to_string()]);
        // A path on the allowlist writes fine (despite the "./" prefix in the allowlist).
        assert!(tools.call("write_file", r#"{"path":"allowed.txt","content":"x"}"#).is_ok());
        // A path INSIDE the root but NOT on the allowlist is refused.
        let res = tools.call("write_file", r#"{"path":"other.txt","content":"x"}"#);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("write scope"));
        let _ = fs::remove_dir_all(&tmp);
    }

    // ── net_blocked signal ──────────────────────────────────────────────────

    /// `net_blocked()` starts false and stays false when net is already Enabled.
    #[test]
    fn net_blocked_is_false_by_default() {
        let (root, tmp) = unique_root();
        let tools = ScopedAgentTools::new(root);
        assert!(!tools.net_blocked());
        let _ = fs::remove_dir_all(&tmp);
    }

    /// `net_blocked()` is set true when `looks_network_blocked` fires while `net == None`.
    /// We simulate this by writing a fake run-output file whose content matches a NEEDLE,
    /// then manually calling `looks_network_blocked` to confirm the detection — the real
    /// `run()` path would need an actual binary; we test the flag logic here directly via
    /// the public `net_blocked()` getter after manually setting via a mock scenario.
    ///
    /// Since `looks_network_blocked` is private we test the end-to-end contract through
    /// the `run()` call path: a command that writes a matching error to its stderr triggers
    /// the flag. We use a shell `echo` piped to stderr as a minimal test fixture. On
    /// platforms where `echo` behaves differently we skip gracefully.
    #[test]
    fn net_blocked_is_set_when_network_error_detected_with_net_none() {
        let (root, tmp) = unique_root();
        // Craft a `run` invocation whose combined output contains a NEEDLE.
        // `python3 -c "import sys; sys.stderr.write('could not resolve host: x.io\n')"` is
        // cross-platform but requires python3. Use `sh -c` which is available on macOS/Linux.
        // On Windows this test will not run (`sh` unavailable), so accept a parse/exec error.
        let mut tools = ScopedAgentTools::new(root.clone())
            .with_net(crate::backend::sandbox::NetPolicy::None);
        let result = tools.call(
            "run",
            r#"{"command":"sh -c \"echo 'could not resolve host: crates.io' >&2; exit 1\""}"#,
        );
        // Whether the command succeeds or not (sandbox may deny sh), the important thing is
        // that IF it ran and produced the NEEDLE, net_blocked is set.  If the sandbox blocked
        // `sh` entirely the output will mention "not in allowlist" (no NEEDLE) → flag stays false.
        // Both outcomes are valid; we just assert the flag is consistent with the output.
        match result {
            Ok(output) | Err(output) => {
                let blocked = output.to_ascii_lowercase().contains("could not resolve host");
                assert_eq!(tools.net_blocked(), blocked,
                    "net_blocked flag must match whether NEEDLE appeared in output; output={output:?}");
            }
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    // ── working_set + out_of_scope_write signal ───────────────────────────────

    /// A write targeting a path IN the working_set (outside root) succeeds and does
    /// NOT set out_of_scope_write.
    #[test]
    fn write_to_working_set_folder_succeeds_and_no_signal() {
        // Build two sibling dirs: root (project root) and extra (working_set member).
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir()
            .join(format!("aspis_wset_{}_{}", std::process::id(), n));
        let root = base.join("root");
        let extra = base.join("extra");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&extra).unwrap();

        let mut tools = ScopedAgentTools::new(root.clone())
            .with_working_set(vec![extra.clone()]);

        // Write a file inside `extra` via a relative path from root.
        // We use an absolute path here (which write_file won't accept as a rel-path),
        // so test via the absolute path in the working_set layer's write_file_absolute
        // path. Actually: the working_set write is through write_file_abs.
        // For the app-level write check we call write_file_abs directly.
        let target = extra.join("out.txt");
        let result = tools.write_file_abs(&target, "hello");
        assert!(result.is_ok(), "write to working_set folder must succeed");
        assert!(
            tools.out_of_scope_write().is_none(),
            "no out_of_scope_write signal when path is in working_set"
        );
        let _ = fs::remove_dir_all(&base);
    }

    /// A write targeting a path OUTSIDE root AND outside the working_set sets
    /// out_of_scope_write to the parent folder.
    #[test]
    fn write_outside_root_and_working_set_sets_signal() {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir()
            .join(format!("aspis_wset2_{}_{}", std::process::id(), n));
        let root = base.join("root");
        let extra = base.join("extra"); // working_set
        let elsewhere = base.join("elsewhere"); // NOT in working_set
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&extra).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();

        let mut tools = ScopedAgentTools::new(root.clone())
            .with_working_set(vec![extra.clone()]);

        let target = elsewhere.join("secret.txt");
        let result = tools.write_file_abs(&target, "x");
        // Must error (not in scope).
        assert!(result.is_err(), "write outside working_set must be rejected");
        // Signal must point to the parent folder (elsewhere, canonicalized).
        let signal = tools.out_of_scope_write();
        assert!(signal.is_some(), "out_of_scope_write must be set");
        // The signal folder must be `elsewhere` (canonicalized).
        let canon_elsewhere = elsewhere.canonicalize().unwrap_or(elsewhere.clone());
        assert_eq!(
            signal.as_deref().unwrap(),
            canon_elsewhere.to_string_lossy().as_ref(),
            "signal must be the parent folder"
        );
        let _ = fs::remove_dir_all(&base);
    }

    /// A write inside the project root but OUTSIDE the per-task write_allowlist does NOT
    /// set out_of_scope_write — it keeps the existing "outside this task's write scope" error.
    #[test]
    fn in_root_but_outside_allowlist_does_not_set_out_of_scope_write_signal() {
        let (root, tmp) = unique_root();
        // allowlist restricts to allowed.txt only
        let mut tools =
            ScopedAgentTools::new(root.clone())
                .with_write_allowlist(vec!["allowed.txt".to_string()]);

        let res = tools.call("write_file", r#"{"path":"other.txt","content":"x"}"#);
        assert!(res.is_err(), "must be rejected by allowlist");
        // The error must mention the task-level scope, NOT set the consent signal.
        assert!(
            res.unwrap_err().contains("write scope"),
            "error should mention write scope"
        );
        assert!(
            tools.out_of_scope_write().is_none(),
            "in-root but out-of-allowlist must NOT set out_of_scope_write (that is a task-scope error, not a consent case)"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    // ── BLOCKER 1: LLM write_file / edit_file with absolute paths ────────────

    /// LLM write_file tool with an absolute path inside a working_set folder MUST succeed
    /// and must NOT set out_of_scope_write.
    #[test]
    fn llm_write_file_abs_in_working_set_succeeds() {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir()
            .join(format!("aspis_b1a_{}_{}", std::process::id(), n));
        let root = base.join("root");
        let extra = base.join("extra");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&extra).unwrap();

        let mut tools = ScopedAgentTools::new(root.clone())
            .with_working_set(vec![extra.clone()]);

        let target = extra.join("out.txt");
        let path_str = target.to_string_lossy();
        let args = format!(r#"{{"path":"{path_str}","content":"hello working_set"}}"#);
        let result = tools.call("write_file", &args);
        assert!(result.is_ok(), "write_file to working_set via abs path must succeed: {:?}", result);
        assert!(tools.out_of_scope_write().is_none(), "must not set signal for in-working_set write");
        // File must actually exist with correct content.
        assert_eq!(fs::read_to_string(&target).unwrap(), "hello working_set");
        let _ = fs::remove_dir_all(&base);
    }

    /// LLM write_file tool with an absolute path OUTSIDE root AND outside the working_set
    /// MUST set out_of_scope_write to the parent folder, return a consent-required error,
    /// and must NOT write anything to disk.
    #[test]
    fn llm_write_file_abs_outside_scope_sets_signal_and_writes_nothing() {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir()
            .join(format!("aspis_b1b_{}_{}", std::process::id(), n));
        let root = base.join("root");
        let extra = base.join("extra");   // working_set
        let elsewhere = base.join("elsewhere"); // NOT in working_set
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&extra).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();

        let mut tools = ScopedAgentTools::new(root.clone())
            .with_working_set(vec![extra.clone()]);

        let target = elsewhere.join("secret.txt");
        let path_str = target.to_string_lossy();
        let args = format!(r#"{{"path":"{path_str}","content":"evil"}}"#);
        let result = tools.call("write_file", &args);
        assert!(result.is_err(), "write outside scope must be rejected");
        let err = result.unwrap_err();
        assert!(
            err.contains("consent required") || err.contains("outside"),
            "error must mention consent: {err}"
        );
        // Signal must be set to the parent folder.
        let signal = tools.out_of_scope_write();
        assert!(signal.is_some(), "out_of_scope_write must be set");
        let canon_elsewhere = elsewhere.canonicalize().unwrap_or(elsewhere.clone());
        assert_eq!(
            signal.unwrap(),
            canon_elsewhere.to_string_lossy().as_ref(),
            "signal must be the parent folder"
        );
        // File must NOT have been written.
        assert!(!target.exists(), "file must not be written on scope violation");
        let _ = fs::remove_dir_all(&base);
    }

    /// LLM edit_file tool with an absolute path inside a working_set folder MUST succeed.
    #[test]
    fn llm_edit_file_abs_in_working_set_succeeds() {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir()
            .join(format!("aspis_b1c_{}_{}", std::process::id(), n));
        let root = base.join("root");
        let extra = base.join("extra");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&extra).unwrap();
        let target = extra.join("edit.txt");
        fs::write(&target, "original content in working_set").unwrap();

        let mut tools = ScopedAgentTools::new(root.clone())
            .with_working_set(vec![extra.clone()]);

        let path_str = target.to_string_lossy();
        let args = format!(
            r#"{{"path":"{path_str}","oldString":"original","newString":"updated"}}"#
        );
        let result = tools.call("edit_file", &args);
        assert!(result.is_ok(), "edit_file via abs path in working_set must succeed: {:?}", result);
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "updated content in working_set"
        );
        let _ = fs::remove_dir_all(&base);
    }

    /// LLM edit_file tool with an absolute path OUTSIDE root AND outside the working_set
    /// MUST set out_of_scope_write and return a consent-required error.
    #[test]
    fn llm_edit_file_abs_outside_scope_sets_signal() {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir()
            .join(format!("aspis_b1d_{}_{}", std::process::id(), n));
        let root = base.join("root");
        let extra = base.join("extra");
        let elsewhere = base.join("elsewhere");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&extra).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        let target = elsewhere.join("secret.txt");
        fs::write(&target, "secret content").unwrap();

        let mut tools = ScopedAgentTools::new(root.clone())
            .with_working_set(vec![extra.clone()]);

        let path_str = target.to_string_lossy();
        let args = format!(
            r#"{{"path":"{path_str}","oldString":"secret","newString":"evil"}}"#
        );
        let result = tools.call("edit_file", &args);
        assert!(result.is_err(), "edit outside scope must be rejected");
        let signal = tools.out_of_scope_write();
        assert!(signal.is_some(), "out_of_scope_write must be set for abs-path edit outside scope");
        // File must be unmodified.
        assert_eq!(fs::read_to_string(&target).unwrap(), "secret content", "file must be unmodified");
        let _ = fs::remove_dir_all(&base);
    }

    /// Relative paths inside root continue to work unchanged after the blocker-1 fix.
    #[test]
    fn relative_write_in_root_still_works_after_blocker1() {
        let (root, tmp) = unique_root();
        let mut tools = ScopedAgentTools::new(root.clone());
        let result = tools.call("write_file", r#"{"path":"rel.txt","content":"relative"}"#);
        assert!(result.is_ok(), "relative write inside root must still work: {:?}", result);
        assert_eq!(fs::read_to_string(root.join("rel.txt")).unwrap(), "relative");
        assert!(tools.out_of_scope_write().is_none(), "relative in-root write must not set signal");
        let _ = fs::remove_dir_all(&tmp);
    }

    // ── BLOCKER 3: write_file_abs must reject DANGLING symlinks ──────────────

    /// A dangling symlink inside a working_set folder pointing OUTSIDE must be rejected by
    /// write_file_abs — even though target.exists() is false for a dangling symlink.
    #[test]
    fn write_file_abs_rejects_dangling_symlink_in_working_set() {
        use std::os::unix::fs::symlink;
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir()
            .join(format!("aspis_b3_{}_{}", std::process::id(), n));
        let root = base.join("root");
        let extra = base.join("extra");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&extra).unwrap();

        // Dangling symlink: points to a path outside the scope that does NOT exist.
        let dangling_link = extra.join("dangle.txt");
        let nonexistent_outside = base.join("nonexistent_outside").join("secret.txt");
        symlink(&nonexistent_outside, &dangling_link).unwrap();

        let mut tools = ScopedAgentTools::new(root.clone())
            .with_working_set(vec![extra.clone()]);

        // The link is in the working_set, so scope check passes — but symlink check must fire.
        let result = tools.write_file_abs(&dangling_link, "evil");
        assert!(result.is_err(), "dangling symlink must be rejected");
        let err = result.unwrap_err();
        assert!(
            err.contains("symlink"),
            "error must mention symlink: {err}"
        );
        // Nothing written at the symlink target (doesn't exist anyway, but be explicit).
        assert!(!nonexistent_outside.exists(), "dangling target must not be created");
        let _ = fs::remove_dir_all(&base);
    }

    // ── BLOCKER 4: write_file_abs must check hardlinks ───────────────────────

    /// A hardlinked file inside a working_set folder must be rejected by write_file_abs
    /// (parity with write_resolve's unix nlink() > 1 guard).
    #[test]
    fn write_file_abs_rejects_hardlinked_file_in_working_set() {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir()
            .join(format!("aspis_b4_{}_{}", std::process::id(), n));
        let root = base.join("root");
        let extra = base.join("extra");
        let outside_dir = base.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&extra).unwrap();
        fs::create_dir_all(&outside_dir).unwrap();
        // Create the inode outside, hardlink inside working_set.
        let outside_file = outside_dir.join("secret.txt");
        fs::write(&outside_file, "SECRET").unwrap();
        let link_inside = extra.join("hard.txt");
        std::fs::hard_link(&outside_file, &link_inside).unwrap();

        let mut tools = ScopedAgentTools::new(root.clone())
            .with_working_set(vec![extra.clone()]);

        let result = tools.write_file_abs(&link_inside, "clobber");
        assert!(result.is_err(), "hardlinked file in working_set must be rejected");
        assert!(result.unwrap_err().contains("hardlink"), "error must mention hardlink");
        // Outside inode must be untouched.
        assert_eq!(fs::read_to_string(&outside_file).unwrap(), "SECRET");
        let _ = fs::remove_dir_all(&base);
    }

    // ── FIX 1: write_file_abs / edit_file_abs must enforce the task write_allowlist
    //           for in-root absolute paths ─────────────────────────────────────────

    /// Abs write to an in-root file NOT in the write_allowlist must be REJECTED with the
    /// task-scope error message. The file must NOT be written.
    #[test]
    fn write_file_abs_in_root_outside_allowlist_is_rejected() {
        let (root, tmp) = unique_root();
        // "allowed.txt" is the only allowed path; we try to write "secret.txt" via abs path.
        let mut tools = ScopedAgentTools::new(root.clone())
            .with_write_allowlist(vec!["allowed.txt".to_string()]);

        let target = root.join("secret.txt");
        let path_str = target.to_string_lossy();
        let args = format!(r#"{{"path":"{path_str}","content":"escape"}}"#);
        let result = tools.call("write_file", &args);
        assert!(result.is_err(), "abs in-root write outside allowlist must be rejected");
        assert!(
            result.unwrap_err().contains("write scope"),
            "error must mention write scope (task-level gate)"
        );
        assert!(!target.exists(), "file must NOT be written on allowlist violation");
        // Must NOT set out_of_scope_write (this is a task-scope error, not a consent case).
        assert!(
            tools.out_of_scope_write().is_none(),
            "in-root allowlist violation must NOT set the folder-consent signal"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Abs write to an in-root file that IS in the write_allowlist must SUCCEED.
    #[test]
    fn write_file_abs_in_root_on_allowlist_succeeds() {
        let (root, tmp) = unique_root();
        let mut tools = ScopedAgentTools::new(root.clone())
            .with_write_allowlist(vec!["allowed.txt".to_string()]);

        let target = root.join("allowed.txt");
        let path_str = target.to_string_lossy();
        let args = format!(r#"{{"path":"{path_str}","content":"ok"}}"#);
        let result = tools.call("write_file", &args);
        assert!(result.is_ok(), "abs in-root write for allowlisted path must succeed: {:?}", result);
        assert_eq!(fs::read_to_string(&target).unwrap(), "ok");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Abs write to a file UNDER a working_set folder must SUCCEED regardless of the
    /// write_allowlist (working_set is a distinct granted layer, not subject to allowlist).
    #[test]
    fn write_file_abs_in_working_set_bypasses_allowlist() {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir()
            .join(format!("aspis_fix1c_{}_{}", std::process::id(), n));
        let root = base.join("root");
        let extra = base.join("extra");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&extra).unwrap();

        // allowlist names "allowed.txt" (relative) — but we write to extra/other.txt (abs, ws).
        let mut tools = ScopedAgentTools::new(root.clone())
            .with_write_allowlist(vec!["allowed.txt".to_string()])
            .with_working_set(vec![extra.clone()]);

        let target = extra.join("other.txt");
        let path_str = target.to_string_lossy();
        let args = format!(r#"{{"path":"{path_str}","content":"ws ok"}}"#);
        let result = tools.call("write_file", &args);
        assert!(result.is_ok(), "abs write to working_set must bypass allowlist: {:?}", result);
        assert_eq!(fs::read_to_string(&target).unwrap(), "ws ok");
        let _ = fs::remove_dir_all(&base);
    }

    /// Abs edit_file to an in-root file NOT in the write_allowlist must be REJECTED.
    #[test]
    fn edit_file_abs_in_root_outside_allowlist_is_rejected() {
        let (root, tmp) = unique_root();
        let target = root.join("secret.txt");
        fs::write(&target, "original secret").unwrap();

        let mut tools = ScopedAgentTools::new(root.clone())
            .with_write_allowlist(vec!["allowed.txt".to_string()]);

        let path_str = target.to_string_lossy();
        let args = format!(
            r#"{{"path":"{path_str}","oldString":"original","newString":"evil"}}"#
        );
        let result = tools.call("edit_file", &args);
        assert!(result.is_err(), "abs in-root edit outside allowlist must be rejected");
        assert!(
            result.unwrap_err().contains("write scope"),
            "error must mention write scope"
        );
        // File must be unmodified.
        assert_eq!(fs::read_to_string(&target).unwrap(), "original secret");
        assert!(
            tools.out_of_scope_write().is_none(),
            "in-root allowlist violation must NOT set the consent signal"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Abs edit_file to an in-root file that IS in the write_allowlist must SUCCEED.
    #[test]
    fn edit_file_abs_in_root_on_allowlist_succeeds() {
        let (root, tmp) = unique_root();
        let target = root.join("allowed.txt");
        fs::write(&target, "original content").unwrap();

        let mut tools = ScopedAgentTools::new(root.clone())
            .with_write_allowlist(vec!["allowed.txt".to_string()]);

        let path_str = target.to_string_lossy();
        let args = format!(
            r#"{{"path":"{path_str}","oldString":"original","newString":"updated"}}"#
        );
        let result = tools.call("edit_file", &args);
        assert!(result.is_ok(), "abs in-root edit for allowlisted path must succeed: {:?}", result);
        assert_eq!(fs::read_to_string(&target).unwrap(), "updated content");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Abs edit_file to a file under a working_set folder must SUCCEED regardless of allowlist.
    #[test]
    fn edit_file_abs_in_working_set_bypasses_allowlist() {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir()
            .join(format!("aspis_fix1f_{}_{}", std::process::id(), n));
        let root = base.join("root");
        let extra = base.join("extra");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&extra).unwrap();
        let target = extra.join("ws_file.txt");
        fs::write(&target, "ws original").unwrap();

        let mut tools = ScopedAgentTools::new(root.clone())
            .with_write_allowlist(vec!["allowed.txt".to_string()])
            .with_working_set(vec![extra.clone()]);

        let path_str = target.to_string_lossy();
        let args = format!(
            r#"{{"path":"{path_str}","oldString":"ws original","newString":"ws updated"}}"#
        );
        let result = tools.call("edit_file", &args);
        assert!(result.is_ok(), "abs edit in working_set must bypass allowlist: {:?}", result);
        assert_eq!(fs::read_to_string(&target).unwrap(), "ws updated");
        let _ = fs::remove_dir_all(&base);
    }

    // ── FIX 2: abs writes must call record_touched so the Censor audit sees them ──────────

    /// After an abs write to an in-root file (allowlisted), touched() must contain
    /// the RELATIVE path (consistent with the relative write_file branch).
    #[test]
    fn write_file_abs_in_root_is_recorded_in_touched() {
        let (root, tmp) = unique_root();
        let target = root.join("allowed.txt");
        let mut tools = ScopedAgentTools::new(root.clone())
            .with_write_allowlist(vec!["allowed.txt".to_string()]);

        let path_str = target.to_string_lossy();
        let args = format!(r#"{{"path":"{path_str}","content":"data"}}"#);
        tools.call("write_file", &args).unwrap();
        assert!(
            tools.touched().contains(&"allowed.txt".to_string()),
            "touched() must contain the relative path after an in-root abs write; got: {:?}",
            tools.touched()
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    /// After an abs write to a working_set file, touched() must contain the absolute
    /// path string (so the audit at least records it).
    #[test]
    fn write_file_abs_in_working_set_is_recorded_in_touched() {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir()
            .join(format!("aspis_fix2b_{}_{}", std::process::id(), n));
        let root = base.join("root");
        let extra = base.join("extra");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&extra).unwrap();

        let mut tools = ScopedAgentTools::new(root.clone())
            .with_working_set(vec![extra.clone()]);

        let target = extra.join("ws_out.txt");
        let path_str = target.to_string_lossy().to_string();
        let args = format!(r#"{{"path":"{path_str}","content":"ws"}}"#);
        tools.call("write_file", &args).unwrap();
        assert!(
            tools.touched().iter().any(|e| e.contains("ws_out.txt")),
            "touched() must record working_set abs write; got: {:?}",
            tools.touched()
        );
        let _ = fs::remove_dir_all(&base);
    }

    /// After an abs edit to an in-root file (allowlisted), touched() must contain the
    /// relative path.
    #[test]
    fn edit_file_abs_in_root_is_recorded_in_touched() {
        let (root, tmp) = unique_root();
        let target = root.join("allowed.txt");
        fs::write(&target, "original").unwrap();

        let mut tools = ScopedAgentTools::new(root.clone())
            .with_write_allowlist(vec!["allowed.txt".to_string()]);

        let path_str = target.to_string_lossy();
        let args = format!(
            r#"{{"path":"{path_str}","oldString":"original","newString":"edited"}}"#
        );
        tools.call("edit_file", &args).unwrap();
        assert!(
            tools.touched().contains(&"allowed.txt".to_string()),
            "touched() must contain relative path after in-root abs edit; got: {:?}",
            tools.touched()
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    /// After an abs edit to a working_set file, touched() must record the file.
    #[test]
    fn edit_file_abs_in_working_set_is_recorded_in_touched() {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir()
            .join(format!("aspis_fix2d_{}_{}", std::process::id(), n));
        let root = base.join("root");
        let extra = base.join("extra");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&extra).unwrap();
        let target = extra.join("ws_edit.txt");
        fs::write(&target, "ws original").unwrap();

        let mut tools = ScopedAgentTools::new(root.clone())
            .with_working_set(vec![extra.clone()]);

        let path_str = target.to_string_lossy().to_string();
        let args = format!(
            r#"{{"path":"{path_str}","oldString":"ws original","newString":"ws edited"}}"#
        );
        tools.call("edit_file", &args).unwrap();
        assert!(
            tools.touched().iter().any(|e| e.contains("ws_edit.txt")),
            "touched() must record working_set abs edit; got: {:?}",
            tools.touched()
        );
        let _ = fs::remove_dir_all(&base);
    }

    // ── agentic_run_policy with a working_set includes those folders in writable_paths ──

    /// agentic_run_policy with a working_set includes those folders in writable_paths.
    #[test]
    fn agentic_run_policy_includes_working_set_in_writable() {
        use crate::backend::sandbox::NetPolicy;
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir()
            .join(format!("aspis_wset3_{}_{}", std::process::id(), n));
        let root = base.join("root");
        let extra = base.join("extra");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&extra).unwrap();

        let policy = agentic_run_policy_with_working_set(
            &root,
            NetPolicy::None,
            &[extra.clone()],
        );
        assert!(
            policy.writable_paths.contains(&root),
            "root must be writable"
        );
        assert!(
            policy.writable_paths.contains(&extra),
            "working_set folder must also be writable in the sandbox policy"
        );
        let _ = fs::remove_dir_all(&base);
    }
}

// ── Slice 3: broker gate tests (design-doc acceptance criteria) ──────────────

#[cfg(test)]
mod broker_gates {
    use super::*;
    use crate::backend::broker::SandboxMode;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static GATE_COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Create a unique temp root + containing dir for parallel test isolation.
    fn gate_root(tag: &str) -> (PathBuf, PathBuf) {
        let n = GATE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir()
            .join(format!("aspis_gate_{}_{}_{}", tag, std::process::id(), n));
        let root = tmp.join("root");
        fs::create_dir_all(&root).unwrap();
        (root, tmp)
    }

    // ── gate_out_of_scope_write_triggers_prompt ────────��──────────────────────

    /// CONTRACT: a write targeting a path OUTSIDE (root + working_set) sets the
    /// `out_of_scope_write` signal to the parent folder AND, under a prompting mode
    /// (Ask), `SandboxMode::Ask.prompts_for_folder_write() == true`, so the consent
    /// event WOULD be emitted. This test locks the full signal→prompt chain without an
    /// AppHandle by exercising the pure halves on both ends.
    ///
    /// Additionally asserts the CRITICAL DISTINCTION: a write inside the project root
    /// but outside the per-task allowlist does NOT set the signal (that is a
    /// task-scope error, not a consent case).
    #[test]
    fn gate_out_of_scope_write_triggers_prompt() {
        let n = GATE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir()
            .join(format!("aspis_gate1_{}_{}", std::process::id(), n));
        let root = base.join("root");
        let working_set_dir = base.join("wset");
        let outside = base.join("outside"); // neither root nor working_set
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&working_set_dir).unwrap();
        fs::create_dir_all(&outside).unwrap();

        // ── Part A: write OUTSIDE root + working_set sets the signal ─────────
        let mut tools =
            ScopedAgentTools::new(root.clone()).with_working_set(vec![working_set_dir.clone()]);

        let target = outside.join("secret.txt");
        let result = tools.write_file_abs(&target, "evil");
        assert!(result.is_err(), "write outside scope must be rejected");
        let signal = tools.out_of_scope_write();
        assert!(signal.is_some(), "out_of_scope_write must be set on out-of-scope write");
        let canon_outside = outside.canonicalize().unwrap_or(outside.clone());
        assert_eq!(
            signal.unwrap(),
            canon_outside.to_string_lossy().as_ref(),
            "signal must be the parent folder (outside)"
        );

        // ── Part B: under Ask mode, prompts_for_folder_write() == true ───────
        // This means the consent-request event WOULD be emitted.
        assert!(
            SandboxMode::Ask.prompts_for_folder_write(),
            "Ask mode must prompt for folder write — consent event would be emitted"
        );

        // ── Part C: in-root-but-outside-allowlist does NOT set the signal ────
        let (in_root, in_root_tmp) = gate_root("gate1c");
        let mut tools_in_root =
            ScopedAgentTools::new(in_root).with_write_allowlist(vec!["allowed.txt".to_string()]);
        let res = tools_in_root.call("write_file", r#"{"path":"other.txt","content":"x"}"#);
        assert!(res.is_err(), "allowlist must block the write");
        assert!(
            tools_in_root.out_of_scope_write().is_none(),
            "in-root but out-of-allowlist must NOT set out_of_scope_write (task-scope error, not consent case)"
        );

        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&in_root_tmp);
    }

    // ── gate_respawn_includes_widened_scope ────────────────��──────────────────

    /// CONTRACT: a folder in `working_set` is included in BOTH the Seatbelt writable
    /// policy (`agentic_run_policy_with_working_set`) AND the app-level write check
    /// (`write_file_abs`). I.e. the widened scope takes effect on the next spawn.
    #[test]
    fn gate_respawn_includes_widened_scope() {
        use crate::backend::sandbox::NetPolicy;

        let n = GATE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir()
            .join(format!("aspis_gate3_{}_{}", std::process::id(), n));
        let root = base.join("root");
        let extra = base.join("extra"); // folder being added to working_set
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&extra).unwrap();

        // ── Part A: sandbox policy includes the working_set folder as writable ─
        let policy = agentic_run_policy_with_working_set(
            &root,
            NetPolicy::None,
            &[extra.clone()],
        );
        assert!(
            policy.writable_paths.contains(&root),
            "project root must be in writable_paths"
        );
        assert!(
            policy.writable_paths.contains(&extra),
            "working_set folder must be in writable_paths (Seatbelt side)"
        );

        // ── Part B: app-level write to the working_set folder succeeds ────────
        let mut tools =
            ScopedAgentTools::new(root.clone()).with_working_set(vec![extra.clone()]);
        let target = extra.join("widened.txt");
        let result = tools.write_file_abs(&target, "widened content");
        assert!(
            result.is_ok(),
            "write to working_set folder must succeed (app-level check): {:?}",
            result
        );
        assert!(
            tools.out_of_scope_write().is_none(),
            "working_set write must NOT set out_of_scope_write"
        );
        assert!(
            target.exists(),
            "file must actually have been written on disk"
        );

        let _ = fs::remove_dir_all(&base);
    }
}
