//! Deterministic linter runners for the Censor engine (sub-phase A2).
//!
//! Each tool gets its own submodule with two halves, split so the bulk of logic
//! is testable WITHOUT the tool installed (CI has no linters):
//!   1. A PURE `parse_<tool>(stdout, ...) -> Vec<RawFinding>` — no IO. Fed captured
//!      sample output in tests.
//!   2. A thin `run(root, target) -> Vec<RawFinding>` that presence-detects the
//!      tool (absent → empty, never an error), spawns it FROM the project root
//!      (so it picks up the project's OWN config), pipes stdout, and parses.
//!
//! Every spawned `Command` is built via `build_command`, which centralizes the
//! `apply_no_window` call so no tool ever flashes a console window on Windows.
//!
//! SECURITY: raw tool stdout/stderr is NEVER persisted or logged. gitleaks/semgrep
//! output carries the actual secret/matched source; their parsers extract ONLY
//! structured fields and redact any secret literal. On a spawn failure we log the
//! tool name + path only, never the output.
//!
//! DEAD-CODE NOTE: A2 ships "dark" — the orchestrator that calls `run`/parses/
//! `applicable_runners`/`into_finding` lands in A3. The pure parsers are fully
//! exercised by this module's tests, but `cargo check` doesn't compile test code,
//! so the runner scaffolding reads as unused until A3 wires it. The allow is
//! file-scoped (not crate-wide) and removed when A3 consumes these APIs.
#![allow(dead_code)]

pub mod bandit;
pub mod cargo_audit;
pub mod cargo_check;
pub mod cargo_deny;
pub mod cargo_fmt;
pub mod clippy;
pub mod eslint;
pub mod gitleaks;
pub mod go_vet;
pub mod gofmt;
pub mod jscpd;
pub mod knip;
pub mod lizard;
pub mod npm_audit;
pub mod oxlint;
pub mod pip_audit;
pub mod pyright;
pub mod prettier;
pub mod ruff;
pub mod ruff_format;
pub mod semgrep;
pub mod tsc;
pub mod vulture;
pub mod zizmor;

use super::detect::{FileLang, ProjectKind};
use super::schema::{Category, Disposition, Finding, ProvenanceEntry, Severity, Verdict};
use crate::oracle::python_oracle::apply_no_window;
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// A tool finding before it becomes a persisted `Finding`. Lightweight, IO-free,
/// produced by every `parse_<tool>`. The A3 orchestrator converts each into a
/// `Finding` (stamping content_hash, the stable id, created_at, verdict, etc.) via
/// [`RawFinding::into_finding`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFinding {
    /// Project-relative path, forward-slash normalized.
    pub file: String,
    /// 1-based line, or `None` for a file-level finding.
    pub line: Option<u32>,
    pub severity: Severity,
    pub category: Category,
    /// Tool name (e.g. "clippy", "gitleaks").
    pub source: String,
    pub title: String,
    /// English summary. NEVER raw tool stdout that could carry a secret value.
    pub body: String,
}

impl RawFinding {
    /// Convert into a persisted `Finding`, stamping the fields A2 cannot know:
    /// `content_hash` (the file's current hash, supplied by A3), `created_at`/the
    /// initial provenance timestamp (`now`), a deterministic `id`, and the machine
    /// defaults `verdict = Suspected` / `disposition = Open`. The file path is
    /// re-normalized to forward slashes defensively (the parsers already normalize,
    /// but `into_finding` is the contract boundary A3 relies on).
    pub fn into_finding(self, content_hash: &str, now: &str) -> Finding {
        let file = self.file.replace('\\', "/");
        let id = Finding::compute_id(&file, self.line, self.category, &self.source, &self.title);
        Finding {
            id,
            file,
            content_hash: content_hash.to_string(),
            line: self.line,
            severity: self.severity,
            category: self.category,
            source: self.source,
            title: self.title,
            body: self.body,
            verdict: Verdict::Suspected,
            disposition: Disposition::Open,
            provenance: vec![ProvenanceEntry {
                actor: "censor".to_string(),
                action: "created".to_string(),
                role: String::new(),
                at: now.to_string(),
            }],
            created_at: now.to_string(),
            commit: None,
        }
    }
}

/// How often a runner should fire. The A3 orchestrator reads this to bucket
/// runners onto the fine (per-file) vs coarse (crate/project) debounce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granularity {
    /// Per-file: cheap, runs on every settled file with the changed file path
    /// (eslint/ruff/bandit/vulture/lizard/semgrep).
    Fine,
    /// Crate/project-level: slow, whole-project/tree scan; runs on the coarse
    /// debounce and ignores the changed file path (clippy/cargo-check/cargo-audit/
    /// tsc/knip/jscpd/gitleaks).
    Coarse,
}

/// Stable identifier for each runner, returned by `applicable_runners` and used by
/// A3 to look up and dispatch the matching `run` fn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunnerId {
    Clippy,
    CargoCheck,
    CargoAudit,
    CargoDeny,
    CargoFmt,
    Tsc,
    Eslint,
    Knip,
    Prettier,
    NpmAudit,
    Oxlint,
    Ruff,
    RuffFormat,
    PipAudit,
    Pyright,
    Bandit,
    Vulture,
    Gofmt,
    GoVet,
    Gitleaks,
    Jscpd,
    Lizard,
    Semgrep,
    Zizmor,
}

impl RunnerId {
    /// The trigger granularity of this runner.
    ///
    /// COARSE = project-wide: the tool inspects the whole project (or the whole
    /// crate / dependency tree) in one invocation and ignores any single changed
    /// file, so A3 runs it on the slow debounce ONCE per settle batch rather than
    /// per file. FINE = per-file: the tool is invoked with the changed file path
    /// and is cheap enough to run on every settled file.
    pub fn granularity(self) -> Granularity {
        match self {
            // Project-wide / crate-level / whole-tree scans.
            RunnerId::Clippy
            | RunnerId::CargoCheck
            | RunnerId::CargoAudit
            | RunnerId::CargoDeny
            | RunnerId::CargoFmt
            | RunnerId::Tsc
            | RunnerId::Knip
            | RunnerId::Jscpd
            | RunnerId::Gitleaks
            | RunnerId::NpmAudit
            | RunnerId::PipAudit
            // go vet COMPILES the module's packages (type-checked AST) → heavy /
            // thermal, project-wide; it is COARSE (debounced) and must never run in
            // the tight interactive loop (see runners/go_vet.rs header).
            | RunnerId::GoVet
            | RunnerId::Zizmor => Granularity::Coarse,
            // Per-file linters/scanners (invoked with the changed file path).
            RunnerId::Eslint
            | RunnerId::Prettier
            | RunnerId::Ruff
            | RunnerId::RuffFormat
            | RunnerId::Bandit
            | RunnerId::Vulture
            // gofmt only parses + reformats in memory (INSTANT, no compile) → Fine.
            | RunnerId::Gofmt
            | RunnerId::Lizard
            | RunnerId::Semgrep
            | RunnerId::Oxlint
            | RunnerId::Pyright => Granularity::Fine,
        }
    }

    /// The tool's executable name, used for presence detection.
    pub fn program(self) -> &'static str {
        match self {
            RunnerId::Clippy | RunnerId::CargoCheck | RunnerId::CargoAudit | RunnerId::CargoFmt => "cargo",
            // cargo-deny ships as its own binary (a cargo subcommand shim).
            RunnerId::CargoDeny => "cargo-deny",
            RunnerId::Tsc => "tsc",
            RunnerId::Eslint => "eslint",
            RunnerId::Knip => "knip",
            RunnerId::Prettier => "prettier",
            RunnerId::NpmAudit => "npm",
            RunnerId::Oxlint => "oxlint",
            RunnerId::Ruff => "ruff",
            RunnerId::PipAudit => "pip-audit",
            RunnerId::Pyright => "pyright",
            RunnerId::RuffFormat => "ruff",
            RunnerId::Bandit => "bandit",
            RunnerId::Vulture => "vulture",
            RunnerId::Gofmt => "gofmt",
            RunnerId::GoVet => "go",
            RunnerId::Gitleaks => "gitleaks",
            RunnerId::Jscpd => "jscpd",
            RunnerId::Lizard => "lizard",
            RunnerId::Semgrep => "semgrep",
            RunnerId::Zizmor => "zizmor",
        }
    }
}

/// What a runner is asked to inspect. Fine runners receive the changed file's
/// project-relative path; coarse runners ignore it and inspect the whole project.
#[derive(Debug, Clone)]
pub struct RunTarget {
    /// Project-relative path of the changed file (forward-slash normalized).
    /// Coarse (crate/project) runners ignore this.
    pub file_rel_path: String,
}

/// The cross-cutting runners that apply to EVERY file regardless of project kind
/// or language: secret scanning, copy/paste, complexity, semgrep, and zizmor.
const CROSS_CUTTING: [RunnerId; 5] = [
    RunnerId::Gitleaks,
    RunnerId::Jscpd,
    RunnerId::Lizard,
    RunnerId::Semgrep,
    RunnerId::Zizmor,
];

/// Decide which runners apply to a changed file, given the project's detected
/// kinds and the file's language. Deterministic and order-stable (kind-specific
/// runners first, then the cross-cutting set). The result never contains
/// duplicates.
///
/// Mapping:
///   - `.rs` file in a Rust project → clippy, cargo-check, cargo-audit (coarse)
///   - `.ts`/`.js` file in a Node project → tsc, eslint, knip
///   - `.py` file in a Python project → ruff, bandit, vulture
///   - `.go` file in a Go project → gofmt (fine), go vet (coarse, compile-based)
///   - ANY file → gitleaks, jscpd, lizard, semgrep (cross-cutting)
///
/// A language-specific runner is only added when BOTH the file lang AND the
/// matching project kind are present (a stray `.py` in a Rust-only repo gets only
/// the cross-cutting set — no point running ruff where there's no Python config).
pub fn applicable_runners(kinds: &HashSet<ProjectKind>, lang: FileLang) -> Vec<RunnerId> {
    let mut out: Vec<RunnerId> = Vec::new();
    match lang {
        FileLang::Rust if kinds.contains(&ProjectKind::Rust) => {
            out.push(RunnerId::Clippy);
            out.push(RunnerId::CargoCheck);
            out.push(RunnerId::CargoAudit);
            out.push(RunnerId::CargoDeny);
            out.push(RunnerId::CargoFmt);
        }
        FileLang::Ts if kinds.contains(&ProjectKind::Node) => {
            out.push(RunnerId::Tsc);
            out.push(RunnerId::Eslint);
            out.push(RunnerId::Knip);
            out.push(RunnerId::Prettier);
            out.push(RunnerId::NpmAudit);
            out.push(RunnerId::Oxlint);
        }
        FileLang::Py if kinds.contains(&ProjectKind::Python) => {
            out.push(RunnerId::Ruff);
            out.push(RunnerId::RuffFormat);
            out.push(RunnerId::PipAudit);
            out.push(RunnerId::Pyright);
            out.push(RunnerId::Bandit);
            out.push(RunnerId::Vulture);
        }
        FileLang::Go if kinds.contains(&ProjectKind::Go) => {
            // gofmt is Fine (instant, no compile); go vet is Coarse (compile-based,
            // debounced — never in the hot loop). Both advisory.
            out.push(RunnerId::Gofmt);
            out.push(RunnerId::GoVet);
        }
        _ => {}
    }
    out.extend_from_slice(&CROSS_CUTTING);
    out
}

/// Build a `Command` with `apply_no_window` already applied. EVERY runner spawns
/// through this helper so the CREATE_NO_WINDOW flag is set in exactly one place
/// (no per-runner re-inlining of `0x0800_0000`, no console flash on Windows).
pub fn build_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    apply_no_window(&mut cmd);
    cmd
}

/// Default hard timeout for a single runner invocation. Compile/dependency-scan
/// tools (clippy/cargo-check/cargo-audit/semgrep) are slow; A3 passes a longer
/// budget for those via [`run_capture_with_timeout`]. The plain [`run_capture`]
/// uses this default.
pub const DEFAULT_RUNNER_TIMEOUT: Duration = Duration::from_secs(120);

/// Hard cap on the stdout we will read from a runner (16 MiB). A linter that
/// floods stdout (a pathological repo, a tool bug, or a hostile config) must not
/// be able to OOM the process: once the cap is hit we stop reading, kill the
/// child, and return nothing. Real linter JSON for a single file is kilobytes;
/// 16 MiB is generous headroom for an honest project-wide run.
pub const MAX_STDOUT_BYTES: usize = 16 * 1024 * 1024;

/// Shared single-line/truncation cap used by every parser to bound a title/body.
/// `max` is a CHAR count (not bytes), so multibyte text is never split mid-char
/// and the ellipsis is appended on overflow. Consolidated here so the ~12 runner
/// copies don't drift.
pub(super) fn cap(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{truncated}…")
    }
}

/// Redact secret-looking tokens from a tool's human-readable message BEFORE it is
/// placed into a finding's title/body.
///
/// PRIVACY: several tools interpolate the MATCHED value into their `message` /
/// `Description` (semgrep expands `$METAVAR`; gitleaks can embed the match). Those
/// strings flow into the finding title/body, which is persisted to a shard — so a
/// raw secret would be written to disk. This pass replaces high-entropy / secret-
/// shaped runs with `[redacted]` while leaving ordinary prose (rule names, English
/// words, short identifiers, file paths) intact.
///
/// Heuristics (conservative — redact when in doubt, never panic):
///   - any run of 12+ chars drawn from the secret alphabet
///     (`A-Za-z0-9` plus the symbols common in tokens: `+/=_\-.`) that ALSO
///     contains at least one digit OR mixes upper+lower OR contains a token
///     symbol — i.e. it does not look like an ordinary lowercase English word; and
///   - AWS-access-key-shaped tokens (`AKIA`/`ASIA` + 16 base32 chars) regardless
///     of length classification.
///
/// A long all-lowercase word ("authentication") is NOT redacted; a base64/hex blob
/// or `AKIAIOSFODNN7EXAMPLE` IS. The scan is byte-cheap and allocation-light.
pub(super) fn redact_secrets(s: &str) -> String {
    const REDACTED: &str = "[redacted]";
    // A char is part of a candidate token if it's in the secret alphabet.
    fn is_token_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-' | '.')
    }
    // Does a token look like a secret rather than ordinary prose / a path / a slug?
    fn looks_secret(tok: &str) -> bool {
        let len = tok.chars().count();
        if len < 12 {
            return false;
        }
        // AWS access key id: AKIA/ASIA + 16 uppercase base32 chars.
        if (tok.starts_with("AKIA") || tok.starts_with("ASIA"))
            && tok.len() == 20
            && tok[4..]
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            return true;
        }
        let has_digit = tok.chars().any(|c| c.is_ascii_digit());
        let has_upper = tok.chars().any(|c| c.is_ascii_uppercase());
        let has_lower = tok.chars().any(|c| c.is_ascii_lowercase());
        let has_symbol = tok
            .chars()
            .any(|c| matches!(c, '+' | '/' | '=' | '_' | '-' | '.'));
        // Dotted/dashed/underscored identifiers (rule ids, file paths like
        // `python.lang.security.audit`) are prose-ish: only flag them when they
        // also carry digits AND mixed case (i.e. a real token, not a slug).
        let mostly_separators = has_symbol && !has_digit && !(has_upper && has_lower);
        if mostly_separators {
            return false;
        }
        // High-entropy if it mixes classes: digits+letters, or upper+lower, or a
        // token symbol alongside alphanumerics.
        (has_digit && (has_upper || has_lower)) || (has_upper && has_lower) || has_symbol
    }

    let mut out = String::with_capacity(s.len());
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut String| {
        if token.is_empty() {
            return;
        }
        if looks_secret(token) {
            out.push_str(REDACTED);
        } else {
            out.push_str(token);
        }
        token.clear();
    };
    for c in s.chars() {
        if is_token_char(c) {
            token.push(c);
        } else {
            flush(&mut token, &mut out);
            out.push(c);
        }
    }
    flush(&mut token, &mut out);
    out
}

/// Run a single piped command from `root` with the [`DEFAULT_RUNNER_TIMEOUT`].
/// See [`run_capture_with_timeout`] for the full contract.
pub fn run_capture(program: &str, args: &[&str], root: &Path) -> Option<String> {
    run_capture_with_timeout(program, args, root, DEFAULT_RUNNER_TIMEOUT)
}

/// Run a single piped command from `root`, returning its stdout as a String when
/// the process produced output (even on a non-zero exit, which is NORMAL for a
/// linter that found issues). Returns `None` ONLY on a true spawn failure, a
/// timeout, a stdout overrun, or when the process crashed with no output. On any
/// error, logs the program name + root path ONLY — never stdout/stderr (which may
/// carry secrets).
///
/// HARD LIMITS (a linter is untrusted, third-party code):
///   - `timeout`: the child is KILLED if it runs longer than this (a hung tool
///     must never block the watcher forever). Defaults via [`run_capture`].
///   - [`MAX_STDOUT_BYTES`]: stdout is read into a bounded buffer; on overrun the
///     child is killed and we return `None` (never accumulate unbounded → OOM).
///   - stdin is `null` so a tool that reads stdin can't block waiting for input.
///
/// Implementation: we spawn ONCE, read stdout on a worker thread into a capped
/// buffer, and poll for exit / timeout / overrun on this thread. On timeout or
/// overrun we kill the child, then join the reader (the kill closes the pipe so
/// the reader unblocks). stderr is drained on its own thread and discarded so a
/// chatty tool can't deadlock by filling the stderr pipe — but its CONTENTS are
/// never surfaced (only the exit code is logged, per WARNING 2).
///
/// The caller (each `run`) is responsible for `command_exists` gating BEFORE
/// calling this; an absent tool should short-circuit to an empty result without
/// even attempting a spawn.
pub fn run_capture_with_timeout(
    program: &str,
    args: &[&str],
    root: &Path,
    timeout: Duration,
) -> Option<String> {
    run_capture_stream_with_timeout(program, args, root, timeout, false)
}

/// Like [`run_capture_with_timeout`] but captures STDERR (draining stdout):
/// some tools (cargo-deny) emit their line-delimited JSON diagnostics there.
/// OPT-IN per runner that needs it — every cap, kill and privacy rule of the
/// stdout path applies unchanged (captured bytes go to the caller's PURE
/// parser; they are never logged).
pub fn run_capture_stderr_with_timeout(
    program: &str,
    args: &[&str],
    root: &Path,
    timeout: Duration,
) -> Option<String> {
    run_capture_stream_with_timeout(program, args, root, timeout, true)
}

/// Stream-parametric core: `capture_stderr` selects which pipe feeds the
/// capped reader; the OTHER pipe is drained to a sink (full-pipe deadlock
/// guard). Variable names below keep the historical stdout-centric names —
/// "stdout_handle" is "the captured stream's handle".
fn run_capture_stream_with_timeout(
    program: &str,
    args: &[&str],
    root: &Path,
    timeout: Duration,
    capture_stderr: bool,
) -> Option<String> {
    let mut cmd = build_command(program);
    cmd.args(args)
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => {
            // Spawn failure (tool vanished between detect and spawn, perms, etc.).
            // NEVER log the underlying error if it could echo arguments; the
            // program name + path is enough for diagnosis.
            eprintln!(
                "censor: runner '{program}' failed to spawn at {}",
                root.display()
            );
            return None;
        }
    };

    // Drain stdout on a worker into a CAPPED buffer; on overrun the worker flips
    // `overran_flag` and stops reading. The poll loop watches that flag so it can
    // KILL the child promptly on overrun (rather than waiting for the timeout while
    // the child blocks on a full pipe). Reading on a thread (rather than
    // `output()`) is what lets the poll loop enforce the timeout / overrun kill.
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let overran_flag = Arc::new(AtomicBool::new(false));
    let capture_src: Option<Box<dyn Read + Send>> = if capture_stderr {
        child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>)
    } else {
        child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>)
    };
    let drain_src: Option<Box<dyn Read + Send>> = if capture_stderr {
        child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>)
    } else {
        child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>)
    };
    let stdout_handle = capture_src.map(|mut pipe| {
        let flag = Arc::clone(&overran_flag);
        std::thread::spawn(move || read_capped(&mut pipe, MAX_STDOUT_BYTES, &flag))
    });
    // Drain the OTHER stream to /dev/null so a tool that floods it can't
    // deadlock on a full pipe. Contents are discarded (privacy: either stream
    // can echo matched values).
    let stderr_handle = drain_src.map(|mut pipe| {
        std::thread::spawn(move || {
            let mut sink = std::io::sink();
            let _ = std::io::copy(&mut pipe, &mut sink);
        })
    });

    let started = Instant::now();
    let mut timed_out = false;
    let mut overran = false;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {}
            Err(_) => {
                // try_wait itself failed: kill, drain, give up.
                let _ = child.kill();
                let _ = child.wait();
                eprintln!(
                    "censor: runner '{program}' wait failed at {}",
                    root.display()
                );
                join_quiet(stdout_handle);
                join_stderr(stderr_handle);
                return None;
            }
        }
        if overran_flag.load(Ordering::Relaxed) {
            // The reader hit the cap; kill the child so it can't keep producing
            // (and so it unblocks if it's stalled on a now-unread, full pipe).
            overran = true;
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    // Reap the final status (after a kill `wait` was already called; a second is a
    // harmless no-op error we ignore). Capture the exit code for diagnostics.
    let status = child.wait().ok();
    // Kill closed the pipe (on timeout/overrun); on normal exit the pipe is at EOF
    // — either way the reader thread can now finish.
    join_stderr(stderr_handle);
    let (stdout_bytes, reader_overran) = match join_quiet(stdout_handle) {
        Some(r) => r,
        None => (Vec::new(), false),
    };
    // Overrun is true if EITHER the poll loop observed the flag OR the reader
    // reported it on join (a race where the child exited before the loop polled).
    let overran = overran || reader_overran;

    if timed_out {
        // Identity only — never the (possibly secret-bearing) output.
        eprintln!("censor: runner '{program}' timeout at {}", root.display());
        return None;
    }
    if overran {
        eprintln!(
            "censor: runner '{program}' {} exceeded {} bytes at {} (overrun)",
            if capture_stderr { "stderr" } else { "stdout" },
            MAX_STDOUT_BYTES,
            root.display()
        );
        return None;
    }

    // "stdout" here = the CAPTURED stream (stderr when capture_stderr).
    let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let success = status.map(|s| s.success()).unwrap_or(false);
    if stdout.is_empty() && !success {
        // Crash / hard failure with no parseable output. Log identity + exit CODE
        // (WARNING 2) — never stderr/stdout content.
        let code = status.and_then(|s| s.code());
        match code {
            Some(c) => eprintln!(
                "censor: runner '{program}' produced no output at {} (exit {c})",
                root.display()
            ),
            None => eprintln!(
                "censor: runner '{program}' produced no output at {} (terminated by signal)",
                root.display()
            ),
        }
        None
    } else {
        Some(stdout)
    }
}

/// Read from `pipe` into a buffer capped at `max` bytes. Returns the bytes read and
/// a flag set when the source had MORE than `max` bytes (overrun). On overrun we
/// set `overran_flag` (so the poll loop can kill the child) AND stop reading, so an
/// unbounded source can't grow the buffer past the cap. Read errors are treated as
/// EOF (the bytes gathered so far are returned).
fn read_capped<R: Read>(
    pipe: &mut R,
    max: usize,
    overran_flag: &std::sync::atomic::AtomicBool,
) -> (Vec<u8>, bool) {
    use std::sync::atomic::Ordering;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => return (buf, false),
            Ok(n) => {
                if buf.len() + n > max {
                    // Take only up to the cap, signal overrun, and stop reading.
                    let take = max.saturating_sub(buf.len());
                    buf.extend_from_slice(&chunk[..take]);
                    overran_flag.store(true, Ordering::Relaxed);
                    return (buf, true);
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return (buf, false),
        }
    }
}

/// Join the stdout reader, tolerating a panicked thread (returns `None`).
fn join_quiet(handle: Option<std::thread::JoinHandle<(Vec<u8>, bool)>>) -> Option<(Vec<u8>, bool)> {
    handle.and_then(|h| h.join().ok())
}

/// Join the stderr drainer; its result is intentionally discarded.
fn join_stderr(handle: Option<std::thread::JoinHandle<()>>) {
    if let Some(h) = handle {
        let _ = h.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn kinds(list: &[ProjectKind]) -> HashSet<ProjectKind> {
        list.iter().copied().collect()
    }

    #[test]
    fn build_command_sets_program() {
        // apply_no_window is not observable cross-platform, but the program is.
        let cmd = build_command("some-tool");
        assert_eq!(cmd.get_program(), std::ffi::OsStr::new("some-tool"));
    }

    #[test]
    fn rust_file_in_rust_project_gets_cargo_tools_plus_cross_cutting() {
        let r = applicable_runners(&kinds(&[ProjectKind::Rust]), FileLang::Rust);
        assert!(r.contains(&RunnerId::Clippy));
        assert!(r.contains(&RunnerId::CargoCheck));
        assert!(r.contains(&RunnerId::CargoAudit));
        // Cross-cutting always present.
        assert!(r.contains(&RunnerId::Gitleaks));
        assert!(r.contains(&RunnerId::Jscpd));
        assert!(r.contains(&RunnerId::Lizard));
        assert!(r.contains(&RunnerId::Semgrep));
        // No JS/Python tools for a .rs file.
        assert!(!r.contains(&RunnerId::Eslint));
        assert!(!r.contains(&RunnerId::Ruff));
    }

    #[test]
    fn ts_file_in_node_project_gets_js_tools_plus_cross_cutting() {
        let r = applicable_runners(&kinds(&[ProjectKind::Node]), FileLang::Ts);
        assert!(r.contains(&RunnerId::Tsc));
        assert!(r.contains(&RunnerId::Eslint));
        assert!(r.contains(&RunnerId::Knip));
        assert!(r.contains(&RunnerId::Gitleaks));
        assert!(!r.contains(&RunnerId::Clippy));
    }

    #[test]
    fn py_file_in_python_project_gets_py_tools_plus_cross_cutting() {
        let r = applicable_runners(&kinds(&[ProjectKind::Python]), FileLang::Py);
        assert!(r.contains(&RunnerId::Ruff));
        assert!(r.contains(&RunnerId::Bandit));
        assert!(r.contains(&RunnerId::Vulture));
        assert!(r.contains(&RunnerId::Semgrep));
        assert!(!r.contains(&RunnerId::Tsc));
    }

    #[test]
    fn go_file_in_go_project_gets_gofmt_govet_plus_cross_cutting() {
        let r = applicable_runners(&kinds(&[ProjectKind::Go]), FileLang::Go);
        assert!(r.contains(&RunnerId::Gofmt));
        assert!(r.contains(&RunnerId::GoVet));
        // Cross-cutting always present.
        assert!(r.contains(&RunnerId::Gitleaks));
        assert!(r.contains(&RunnerId::Semgrep));
        // No other-language tools for a .go file.
        assert!(!r.contains(&RunnerId::Clippy));
        assert!(!r.contains(&RunnerId::Ruff));
        assert!(!r.contains(&RunnerId::Eslint));
    }

    #[test]
    fn go_file_without_go_project_kind_gets_only_cross_cutting() {
        // A stray .go file in a Rust-only repo: no gofmt/go-vet, just cross-cutting.
        let r = applicable_runners(&kinds(&[ProjectKind::Rust]), FileLang::Go);
        assert!(!r.contains(&RunnerId::Gofmt));
        assert!(!r.contains(&RunnerId::GoVet));
        assert_eq!(r.len(), CROSS_CUTTING.len());
    }

    #[test]
    fn unknown_file_lang_gets_only_cross_cutting() {
        let r = applicable_runners(
            &kinds(&[ProjectKind::Rust, ProjectKind::Node]),
            FileLang::Other,
        );
        assert_eq!(r.len(), CROSS_CUTTING.len());
        for c in CROSS_CUTTING {
            assert!(r.contains(&c));
        }
    }

    #[test]
    fn lang_specific_runner_skipped_when_project_kind_absent() {
        // A .py file in a Rust-only repo: no ruff/bandit/vulture, just cross-cutting.
        let r = applicable_runners(&kinds(&[ProjectKind::Rust]), FileLang::Py);
        assert!(!r.contains(&RunnerId::Ruff));
        assert_eq!(r.len(), CROSS_CUTTING.len());
    }

    #[test]
    fn applicable_runners_has_no_duplicates() {
        let r = applicable_runners(&kinds(&[ProjectKind::Rust]), FileLang::Rust);
        let set: HashSet<RunnerId> = r.iter().copied().collect();
        assert_eq!(set.len(), r.len());
    }

    #[test]
    fn granularity_buckets() {
        // Project-wide / crate-level scans are Coarse.
        for c in [
            RunnerId::Clippy,
            RunnerId::CargoCheck,
            RunnerId::CargoAudit,
            RunnerId::CargoDeny,
            RunnerId::CargoFmt,
            RunnerId::NpmAudit,
            RunnerId::PipAudit,
            RunnerId::Zizmor,
            RunnerId::Tsc,
            RunnerId::Knip,
            RunnerId::Jscpd,
            RunnerId::Gitleaks,
            // go vet is compile-based / project-wide → Coarse.
            RunnerId::GoVet,
        ] {
            assert_eq!(
                c.granularity(),
                Granularity::Coarse,
                "{c:?} should be Coarse"
            );
        }
        // Per-file linters are Fine.
        for f in [
            RunnerId::Eslint,
            RunnerId::Oxlint,
            RunnerId::Prettier,
            RunnerId::Pyright,
            RunnerId::RuffFormat,
            RunnerId::Ruff,
            RunnerId::Bandit,
            RunnerId::Vulture,
            // gofmt is instant (no compile) → Fine.
            RunnerId::Gofmt,
            RunnerId::Lizard,
            RunnerId::Semgrep,
        ] {
            assert_eq!(f.granularity(), Granularity::Fine, "{f:?} should be Fine");
        }
    }

    #[test]
    fn into_finding_stamps_machine_defaults_and_normalizes_path() {
        let raw = RawFinding {
            file: "src\\a.rs".into(),
            line: Some(10),
            severity: Severity::High,
            category: Category::Security,
            source: "gitleaks".into(),
            title: "Secret detected: aws-key".into(),
            body: "aws-key at src/a.rs:10".into(),
        };
        let f = raw.into_finding("hash123", "2026-06-05T00:00:00Z");
        // Path normalized to forward slashes.
        assert_eq!(f.file, "src/a.rs");
        assert_eq!(f.content_hash, "hash123");
        assert_eq!(f.created_at, "2026-06-05T00:00:00Z");
        assert_eq!(f.verdict, Verdict::Suspected);
        assert_eq!(f.disposition, Disposition::Open);
        assert_eq!(f.line, Some(10));
        assert_eq!(f.severity, Severity::High);
        assert_eq!(f.category, Category::Security);
        // Provenance seeded with the creation entry.
        assert_eq!(f.provenance.len(), 1);
        assert_eq!(f.provenance[0].actor, "censor");
        assert_eq!(f.provenance[0].action, "created");
        assert_eq!(f.provenance[0].at, "2026-06-05T00:00:00Z");
        // Deterministic id matches compute_id over the normalized path.
        let expected = Finding::compute_id(
            "src/a.rs",
            Some(10),
            Category::Security,
            "gitleaks",
            "Secret detected: aws-key",
        );
        assert_eq!(f.id, expected);
    }

    #[test]
    fn redact_secrets_removes_aws_key_and_blobs_keeps_prose() {
        // AWS access-key id is redacted.
        let r = redact_secrets("Key found: AKIAIOSFODNN7EXAMPLE in config");
        assert!(!r.contains("AKIAIOSFODNN7EXAMPLE"), "aws key leaked: {r}");
        assert!(r.contains("[redacted]"));
        assert!(r.contains("Key found:"));
        assert!(r.contains("in config"));

        // A long base64/hex blob is redacted.
        let blob = "aGVsbG8td29ybGQtc2VjcmV0LTEyMzQ1Njc4OQ==";
        let r = redact_secrets(&format!("token={blob}"));
        assert!(!r.contains(blob), "blob leaked: {r}");
        assert!(r.contains("[redacted]"));

        // Ordinary prose / English words are NOT redacted.
        let prose = "Hardcoded password detected in authentication module";
        assert_eq!(redact_secrets(prose), prose);

        // A dotted rule id (slug) is preserved.
        let rule = "python.lang.security.audit.hardcoded-password";
        assert_eq!(redact_secrets(rule), rule);

        // Empty / no-secret strings round-trip unchanged.
        assert_eq!(redact_secrets(""), "");
        assert_eq!(redact_secrets("short id x1"), "short id x1");
    }

    #[test]
    fn read_capped_returns_bytes_under_cap() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let data = b"hello world".to_vec();
        let flag = AtomicBool::new(false);
        let (out, overran) = read_capped(&mut data.as_slice(), 1024, &flag);
        assert_eq!(out, data);
        assert!(!overran);
        assert!(!flag.load(Ordering::Relaxed));
    }

    #[test]
    fn read_capped_flags_overrun_and_stops_at_cap() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let data = vec![b'x'; 5000];
        let flag = AtomicBool::new(false);
        let (out, overran) = read_capped(&mut data.as_slice(), 100, &flag);
        assert!(overran);
        assert_eq!(out.len(), 100);
        assert!(flag.load(Ordering::Relaxed));
    }

    #[test]
    fn run_capture_kills_on_timeout() {
        // A guaranteed-long-running child invoked DIRECTLY (no shell wrapper), so
        // killing the child closes its stdout pipe and the reader unblocks. On
        // Windows `ping -n 30 -w 1000 127.0.0.1` runs ~30s; on unix `sleep 30`.
        #[cfg(windows)]
        let (prog, args): (&str, Vec<&str>) = ("ping", vec!["-n", "30", "-w", "1000", "127.0.0.1"]);
        #[cfg(not(windows))]
        let (prog, args): (&str, Vec<&str>) = ("sleep", vec!["30"]);

        if !crate::backend::projects::command_exists(prog) {
            return; // environment without the helper tool; nothing to assert.
        }
        let start = std::time::Instant::now();
        let out = run_capture_with_timeout(
            prog,
            &args,
            std::env::temp_dir().as_path(),
            Duration::from_millis(300),
        );
        // Timed out → None, and we did NOT wait the full 30s (killed promptly).
        assert!(out.is_none());
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "did not kill promptly"
        );
    }
}
