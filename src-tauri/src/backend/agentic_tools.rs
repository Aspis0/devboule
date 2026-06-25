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
const RUN_TIMEOUT_SECS: u64 = 600; // generous: real test/build runs are slow; bounds a hang

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
}

impl ScopedAgentTools {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            touched: Vec::new(),
            write_allowlist: Vec::new(),
            net: crate::backend::sandbox::NetPolicy::None,
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

    /// The (deduped) relative paths written/edited so far.
    pub fn touched(&self) -> &[String] {
        &self.touched
    }

    /// Whether writing `safe` (a normalized rel path) is permitted by the write allowlist.
    fn write_allowed(&self, safe: &str) -> bool {
        self.write_allowlist.is_empty() || self.write_allowlist.iter().any(|f| f == safe)
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
        let safe = safe_rel_path(path)?;
        if !self.write_allowed(&safe) {
            return Err(format!("'{safe}' is outside this task's write scope"));
        }
        let p = self.write_resolve(path)?;
        fs::write(&p, content).map_err(|e| e.to_string())?;
        self.record_touched(safe);
        Ok(format!("wrote {} bytes to {}", content.len(), p.display()))
    }

    fn edit_file(&mut self, path: &str, old: &str, new: &str) -> Result<String, String> {
        if old.is_empty() {
            return Err("old_string cannot be empty".to_string());
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
        fs::write(&p, content.replacen(old, new, 1)).map_err(|e| e.to_string())?;
        self.record_touched(safe);
        Ok(format!("edited {}", p.display()))
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
    fn run(&self, command: &str) -> Result<String, String> {
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
        let policy = agentic_run_policy(&self.root, self.net.clone());
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
            body.push_str(NETWORK_BLOCKED_HINT);
        }
        Ok(body)
    }
}

/// Build the sandbox policy for a `run` command: the scope root is the SINGLE writable root
/// (build/test tools write under it: target/, node_modules/.cache, …); reads are broad; network
/// is DENY by default (a hostile test cannot exfiltrate). Per-project network unlock to Enabled
/// is a later slice (1b: detect a network-blocked failure → let the user re-run with net). rlimits
/// are NOT yet enforced by wrap (TODO in wrap).
fn agentic_run_policy(
    root: &std::path::Path,
    net: crate::backend::sandbox::NetPolicy,
) -> crate::backend::sandbox::SandboxPolicy {
    crate::backend::sandbox::SandboxPolicy::deny(root.to_path_buf())
        .writable(root.to_path_buf())
        .net(net)
}

const NETWORK_BLOCKED_HINT: &str = "\n--- HINT: this command likely needs NETWORK access, which is \
DISABLED for this project's sandbox. If you trust it, enable network for this project and re-run. ---";

/// Heuristic: does `body` look like a command that failed because the sandbox denied network?
/// Used only when net == None to append a HINT (no false-positive risk: it only adds advice).
fn looks_network_blocked(body: &str) -> bool {
    let b = body.to_ascii_lowercase();
    const NEEDLES: &[&str] = &[
        "could not resolve host", "couldn't resolve host", "name resolution", "getaddrinfo",
        "temporary failure in name resolution", "network is unreachable", "no route to host",
        "connection refused", "operation not permitted", "failed to download", "failed to get",
        "spurious network error", "etimedout", "enotfound", "econnrefused",
        "connection timed out", "could not connect", "network error", "tls handshake",
    ];
    NEEDLES.iter().any(|n| b.contains(n))
}

/// Read a child stream, keeping at most `MAX_RUN_OUTPUT` bytes but CONTINUING to read+discard
/// past the cap so the child's pipe never blocks (bounded memory, no writer deadlock).
fn drain_capped(r: &mut impl std::io::Read) -> Vec<u8> {
    use std::io::Read;
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
}
