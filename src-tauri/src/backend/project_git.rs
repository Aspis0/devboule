//! Git operations extracted from `projects.rs` (S9 Pass 1).
//!
//! All functions here were moved verbatim — same logic, same doc-comments —
//! to keep `projects.rs` focused on project CRUD while git plumbing lives here.

use super::git_push::{self, GitPushRequest, GitPushResult};
use super::model::{
    ProjectCreateInput, ProjectDetail, ProjectGitCommandResult, ProjectGitRepoCandidate,
    ProjectGitStatus, ProjectMetadata,
};
use super::projects::{
    create_project, create_restricted_temp_file, normalize_project_root, read_project_by_id,
    reject_broad_project_root, remove_restricted_temp_file, resolve_project_agent_root,
    ParsedProject,
};
use super::state::BackendState;
use super::vault;
use chrono::Utc;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tauri::State;

pub(crate) fn project_git_status(root_value: Option<&str>) -> ProjectGitStatus {
    let mut status = ProjectGitStatus {
        policy_status: "blocked".into(),
        ..ProjectGitStatus::default()
    };
    let Some(root_raw) = normalize_project_root(root_value) else {
        status
            .warnings
            .push("Project has no agent working root.".into());
        status.required_actions.push(
            "Set the project root to the exact GitHub repository before collaborator handoff."
                .into(),
        );
        return status;
    };
    let root = PathBuf::from(&root_raw);
    status.root_path = Some(root_raw.clone());
    if !root.is_dir() {
        status
            .warnings
            .push("Project root path does not exist on this workstation.".into());
        status
            .required_actions
            .push("Fix the root path before launching agents or cloning collaborators.".into());
        return status;
    }
    let resolved_root = root.canonicalize().unwrap_or(root);
    status.root_path = Some(resolved_root.to_string_lossy().into_owned());

    let Some(repo_root_raw) = git_output_timeout(&resolved_root, &["rev-parse", "--show-toplevel"])
    else {
        status
            .warnings
            .push("Project root is not inside a Git repository.".into());
        status
            .required_actions
            .push("Use a specific code repo root, not the whole Devboule workspace.".into());
        status.suggested_repos = suggested_git_repos_for_root(&resolved_root);
        return status;
    };

    let repo_root = PathBuf::from(repo_root_raw.trim());
    let repo_root = repo_root.canonicalize().unwrap_or(repo_root);
    status.is_git_repo = true;
    status.repo_root = Some(repo_root.to_string_lossy().into_owned());
    status.repo_name = repo_root
        .file_name()
        .and_then(|value| value.to_str())
        .map(ToString::to_string);
    status.branch = git_output_timeout(&repo_root, &["branch", "--show-current"])
        .filter(|value| !value.trim().is_empty())
        .or_else(|| git_output_timeout(&repo_root, &["rev-parse", "--short", "HEAD"]));
    status.commit = git_output_timeout(&repo_root, &["rev-parse", "--short", "HEAD"]);
    status.upstream = git_output_timeout(
        &repo_root,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .filter(|value| !value.trim().is_empty());
    status.origin = git_output_timeout(&repo_root, &["config", "--get", "remote.origin.url"])
        .filter(|value| !value.trim().is_empty())
        .map(|value| sanitize_git_remote(&value));
    status.github_url = status
        .origin
        .as_deref()
        .and_then(github_web_url_from_origin);
    status.is_github = status.github_url.is_some();
    status.clone_command = status
        .origin
        .as_deref()
        .map(|remote| format!("git clone {}", remote.trim_end_matches(".git")));
    status.pull_request_url = status.github_url.as_ref().and_then(|url| {
        let branch = status.branch.as_deref()?;
        if matches!(branch, "main" | "master") {
            None
        } else {
            Some(format!(
                "{url}/compare/{}?expand=1",
                urlencoding::encode(branch)
            ))
        }
    });

    let porcelain =
        git_output_timeout(&repo_root, &["status", "--porcelain=v1"]).unwrap_or_default();
    for line in porcelain.lines().filter(|line| !line.trim().is_empty()) {
        status.dirty_count += 1;
        let bytes = line.as_bytes();
        if line.starts_with("??") {
            status.untracked_count += 1;
            continue;
        }
        if bytes.first().is_some_and(|value| *value != b' ') {
            status.staged_count += 1;
        }
        if bytes.get(1).is_some_and(|value| *value != b' ') {
            status.unstaged_count += 1;
        }
    }

    if status.upstream.is_some() {
        if let Some(raw) = git_output_timeout(
            &repo_root,
            &["rev-list", "--left-right", "--count", "HEAD...@{u}"],
        ) {
            let mut parts = raw.split_whitespace();
            status.ahead_count = parts
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            status.behind_count = parts
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
        }
    }

    status.policy_status = "ready".into();
    if !status.is_github {
        status.policy_status = "warning".into();
        status
            .warnings
            .push("Remote origin is not a recognized GitHub repository.".into());
        status
            .required_actions
            .push("Set a GitHub origin before collaborator onboarding or PR workflow.".into());
    }
    if status.upstream.is_none() {
        status.policy_status = "warning".into();
        status.warnings.push("Branch has no upstream.".into());
        status
            .required_actions
            .push("Push this branch with upstream tracking before handoff.".into());
    }
    if status
        .branch
        .as_deref()
        .is_some_and(|branch| matches!(branch, "main" | "master"))
    {
        status.policy_status = "warning".into();
        status.warnings.push(
            "Current branch is main/master; collaborators should work on feature branches.".into(),
        );
        status
            .required_actions
            .push("Create a feature branch before assigning coder work.".into());
    }
    if status.dirty_count > 0 {
        status.policy_status = "warning".into();
        status.warnings.push(format!(
            "{} uncommitted Git change(s) in this project repo.",
            status.dirty_count
        ));
        status.required_actions.push(
            "Commit or intentionally shelve local changes before marking the project ready.".into(),
        );
    }
    if status.ahead_count > 0 {
        status.policy_status = "warning".into();
        status.warnings.push(format!(
            "Branch is {} commit(s) ahead of upstream.",
            status.ahead_count
        ));
        status
            .required_actions
            .push("Push the branch or open a PR before closing collaborator work.".into());
    }
    if status.behind_count > 0 {
        status.policy_status = "warning".into();
        status.warnings.push(format!(
            "Branch is {} commit(s) behind upstream.",
            status.behind_count
        ));
        status
            .required_actions
            .push("Pull/rebase before launching more coder work.".into());
    }
    status
}

fn git_output_timeout(path: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: in the release GUI exe this git probe runs on every
        // project-status refresh; without it each spawn flashes a conhost window.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().ok()?;
    let started_at = Instant::now();
    loop {
        if child.try_wait().ok()?.is_some() {
            let output = child.wait_with_output().ok()?;
            if !output.status.success() {
                return None;
            }
            return Some(String::from_utf8_lossy(&output.stdout).trim().to_string());
        }
        if started_at.elapsed() >= Duration::from_secs(3) {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

/// Outcome of a mutating git subprocess: exit code + captured stdout/stderr. The
/// caller decides whether a non-zero exit is an error and what to surface; raw
/// stderr is returned ONLY to be threaded into a user-facing error string, never
/// persisted or logged.
#[derive(Debug)]
struct GitRunOutcome {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

/// WARNING E: maximum stderr (in CHARS) carried out of a mutating git op for the
/// user-facing error. Real git errors are short; a repo's commit/pre-push hook is
/// untrusted and could dump large/secret output, so we bound it.
const GIT_STDERR_MAX_CHARS: usize = 500;

/// Trim + cap git stderr to [`GIT_STDERR_MAX_CHARS`] characters (not bytes, so a
/// multibyte message is never split mid-char), appending an ellipsis on overflow.
fn cap_git_stderr(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.chars().count() <= GIT_STDERR_MAX_CHARS {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(GIT_STDERR_MAX_CHARS).collect();
    format!("{head}… [git output truncated]")
}

/// Run a MUTATING git subprocess (commit/push) in `path` and capture its output.
/// Bounded wait for a local git op (add/commit): no network, so 30s is ample.
const GIT_LOCAL_TIMEOUT: Duration = Duration::from_secs(30);
/// Bounded wait for `git push`: hits the network, so a slow upload/handshake must
/// not be killed prematurely. 60s gives a real push room while still capping a hang.
const GIT_PUSH_TIMEOUT: Duration = Duration::from_secs(60);
/// Bounded wait for `git pull --ff-only`: a network fetch + fast-forward checkout.
/// Same order of magnitude as a push, so it shares the 60s budget.
const GIT_PULL_TIMEOUT: Duration = Duration::from_secs(60);
/// Bounded wait for `git clone`: a full history download for a possibly large repo
/// can take far longer than an incremental push/pull, so it gets a much larger
/// budget. Still capped so a wedged clone cannot hang the app forever.
const GIT_CLONE_TIMEOUT: Duration = Duration::from_secs(600);

/// What a drained git child produced: exit code (None if the process was killed
/// for exceeding the timeout) plus the FULLY drained stdout/stderr bytes.
#[derive(Debug)]
struct DrainedChild {
    /// `Some(code)` when git exited on its own; `None` when we killed it because the
    /// timeout elapsed. `-1` is used by callers when a real exit reported no code.
    exit_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// FIX 7: hard cap on how many bytes of EACH stream we STORE in memory while
/// draining a git child. We keep reading past this (so the pipe never fills and
/// deadlocks the child) but discard the excess. 1 MiB is orders of magnitude
/// larger than any real git error or progress stream and larger than the
/// user-facing `cap_git_stderr` (500-char) bound, so normal output is stored in
/// full and the happy path is unchanged — it only bounds a pathological/hostile
/// stream that would otherwise grow for the whole (up to 600s) timeout.
const DRAIN_STORE_CAP_BYTES: usize = 1024 * 1024;

/// FIX 7: drain `reader` to EOF, STORING at most [`DRAIN_STORE_CAP_BYTES`] bytes
/// and DISCARDING the rest. Reading continues to EOF regardless of the cap so the
/// child never blocks on a full pipe (preserving the FIX-1 no-deadlock invariant);
/// only the in-memory buffer is bounded. Generic over `Read` so it works for both
/// `ChildStdout` and `ChildStderr` and is unit-testable with an in-memory reader.
/// Reads in chunks until EOF; appends to `buf` only until the cap, then keeps
/// reading (to drain the source) but discards the bytes.
fn drain_capped<R: std::io::Read>(reader: Option<&mut R>) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    let Some(reader) = reader else {
        return buf;
    };
    let mut chunk = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break, // EOF
            Ok(n) => {
                if buf.len() < DRAIN_STORE_CAP_BYTES {
                    let room = DRAIN_STORE_CAP_BYTES - buf.len();
                    let take = n.min(room);
                    buf.extend_from_slice(&chunk[..take]);
                }
                // else: at cap — keep looping to drain the source, store nothing.
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break, // pipe broke / closed — stop, like read_to_end would.
        }
    }
    buf
}

/// FIX 1 (pipe-buffer deadlock): wait for a spawned git child while CONCURRENTLY
/// draining both pipes, and enforce `timeout`.
///
/// The previous busy-poll (`try_wait` + sleep, then `wait_with_output`) deadlocks
/// when git writes more than the OS pipe buffer (~64KB) to stdout/stderr: git
/// blocks on the pipe write waiting for a reader that never runs until after the
/// process exits, so the process never exits, `try_wait` never returns `Some`, and
/// a perfectly healthy verbose push is killed at the timeout.
///
/// Here a reader thread per pipe drains stdout/stderr to a `Vec<u8>` as git writes,
/// so git never blocks on a full pipe. The timeout is enforced by a watcher loop in
/// THIS thread that polls `try_wait`; if it elapses we `kill` the child. Either way
/// the child is reaped (`wait`) and both reader threads are joined so no FD or
/// thread leaks. Closing the pipes (on child exit/kill) makes the reader threads
/// hit EOF and return, so the joins cannot hang.
fn wait_with_drained_output(
    mut child: std::process::Child,
    timeout: Duration,
) -> Result<DrainedChild, String> {
    // Take the pipe handles so the reader threads own them; dropping them on EOF
    // is what lets `wait` below reap the child cleanly.
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    // FIX 7: cap how much of each stream we STORE. A verbose/malicious git server
    // (or a repo hook) could stream output for the entire (up to 600s clone)
    // timeout, growing the buffer unbounded. We keep READING the pipe to EOF so it
    // never fills and deadlocks the child (the FIX-1 invariant), but stop APPENDING
    // once `DRAIN_STORE_CAP_BYTES` is reached and discard the rest. The cap is far
    // larger than any real git error and larger than the user-facing
    // `cap_git_stderr` bound, so the small-output happy path is byte-identical.
    let stdout_handle = thread::spawn(move || drain_capped(stdout_pipe.as_mut()));
    let stderr_handle = thread::spawn(move || drain_capped(stderr_pipe.as_mut()));

    // Watch for exit or timeout. Draining happens on the reader threads, so this
    // loop can never deadlock on a full pipe — it only decides when to kill.
    let started_at = Instant::now();
    let mut timed_out = false;
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(e) => {
                // Could not poll: kill, reap, and surface the error after draining.
                let _ = child.kill();
                let _ = child.wait();
                // Join readers so the threads/FDs do not leak even on this path.
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                return Err(format!("git could not be polled: {e}"));
            }
        }
        if started_at.elapsed() >= timeout {
            let _ = child.kill();
            timed_out = true;
            break None;
        }
        thread::sleep(Duration::from_millis(25));
    };

    // Reap the child (no zombie). On the timeout path we just killed it; on the
    // happy path it already exited — `wait` returns immediately either way.
    let _ = child.wait();

    if timed_out {
        // FIX 5 (Windows join-hang): on the timeout-kill path the joins CANNOT be
        // assumed safe. `TerminateProcess` kills only git itself, not its children
        // (git-remote-https / the askpass helper); a surviving grandchild keeps the
        // pipe write-end open, so the reader threads never hit EOF and a plain
        // `join()` would block this thread FOREVER — hanging the Tauri command
        // (e.g. approve_git_push_request) and leaving the needs_user bell lit.
        //
        // So we BOUND the join: give the reader threads a short grace period to
        // drain whatever is still buffered, then ABANDON (detach) any that have not
        // finished. Abandoning is safe — each thread owns only a capped buffer
        // (DRAIN_STORE_CAP_BYTES) and will exit on its own once the grandchild
        // finally closes the pipe. We return the timeout error regardless.
        const ABANDON_JOIN_GRACE: Duration = Duration::from_secs(3);
        let _ = join_with_deadline(stdout_handle, ABANDON_JOIN_GRACE);
        let _ = join_with_deadline(stderr_handle, ABANDON_JOIN_GRACE);
        return Err("git command timed out.".into());
    }

    // Happy path: the child exited on its own, so its pipe write-ends are closed,
    // the reader threads have hit EOF and returned, and these joins return at once.
    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();

    Ok(DrainedChild {
        exit_code: exit_status.and_then(|s| s.code()),
        stdout,
        stderr,
    })
}

/// Join a drain thread but give up after `deadline`, returning `None` if it has not
/// finished (the thread is then detached and left to exit on its own). Used ONLY on
/// the timeout-kill path of `wait_with_drained_output`, where a killed git's
/// surviving grandchild can hold a pipe open and make a plain `join()` block forever
/// (see FIX 5). Polls `is_finished()` so we never block past the deadline.
fn join_with_deadline(handle: thread::JoinHandle<Vec<u8>>, deadline: Duration) -> Option<Vec<u8>> {
    let started_at = Instant::now();
    loop {
        if handle.is_finished() {
            return handle.join().ok();
        }
        if started_at.elapsed() >= deadline {
            // Abandon: drop the handle without joining. The thread owns a capped
            // buffer and exits when the lingering pipe write-end is finally closed.
            return None;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

/// Mirrors `git_output_timeout`'s CREATE_NO_WINDOW + bounded-wait pattern (no
/// console flash, no hang) but, unlike the read-only probe, returns the exit code
/// and stderr so a failed commit/push can surface git's message to the UI.
///
/// Args are passed verbatim via `.arg()` (never a shell), so a commit message with
/// spaces/quotes is a single argv entry — no shell injection is possible.
///
/// `timeout` bounds the wait before the hung process is killed. It is per-call so a
/// network push (slow) can be given a longer budget than a local commit/add.
fn git_run(path: &Path, args: &[&str], timeout: Duration) -> Result<GitRunOutcome, String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: this runs from the release GUI exe; without it the
        // spawn flashes a conhost window (the regression fixed app-wide).
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let child = command
        .spawn()
        .map_err(|e| format!("git could not be started: {e}"))?;
    // FIX 1: drain both pipes concurrently while bounding the wait, so a verbose
    // commit/push that writes > the OS pipe buffer cannot deadlock and get killed.
    let drained = wait_with_drained_output(child, timeout)?;
    Ok(GitRunOutcome {
        exit_code: drained.exit_code.unwrap_or(-1),
        stdout: String::from_utf8_lossy(&drained.stdout).trim().to_string(),
        // WARNING E: cap stderr before it is surfaced to the UI. A real git
        // error is short (a line or two); an arbitrary repo's commit/pre-push
        // HOOK can write anything to stderr — potentially echoing a secret it
        // read. Truncating to a small bound keeps the message informative
        // while preventing a hook from exfiltrating large/secret output
        // through the error string.
        stderr: cap_git_stderr(&String::from_utf8_lossy(&drained.stderr)),
    })
}

// ===========================================================================
// Authenticated git (GitHub PAT injected OFF argv/disk-plaintext/logs)
// ===========================================================================
//
// The keystone of the GitHub-push security model. A `git push` to a private
// GitHub remote needs the PAT, but the token must NEVER appear on:
//   - argv (visible in `ps`/Task Manager and to any process on the box),
//   - `.git/config` (a credentialed remote URL persists the token on disk),
//   - the PTY scrollback / any log,
//   - an HTTP header passed via `-c http.extraHeader=` (that is argv).
//
// Mechanism: git's `GIT_ASKPASS`. git invokes the askpass PROGRAM twice — once
// for the "Username" prompt and once for the "Password" prompt — passing the
// prompt text as the program's first argument. Our askpass program is a tiny
// generated script that holds NO secret: it inspects its first argument and,
// for the username, prints the fixed literal `x-access-token`; for anything else
// (the password) it prints the value of the `ASPIS_GIT_ASKPASS_TOKEN` env var.
// That env var is set ONLY on the child git process's environment — never global,
// never on argv. The token therefore lives only in the child process's env block
// (not world-readable on a multi-user box) and in the parent's memory.
//
// Hardening flags (all NON-secret, argv-safe):
//   - GIT_TERMINAL_PROMPT=0  → git never blocks on an interactive prompt, so a
//     missing/invalid token fails fast instead of hanging.
//   - GIT_CONFIG_NOSYSTEM=1  → ignore the system git config.
//   - `-c credential.helper=` (empty) → neutralize any ambient credential helper
//     (Windows Git Credential Manager, `gh`, `~/.git-credentials`) so the box's
//     global creds cannot silently override the token we are injecting.

/// Name of the env var the askpass script reads the token from. Set ONLY on the
/// child git process environment (never global, never argv). The script contains
/// only this NAME, never the token value.
const ASPIS_GIT_ASKPASS_TOKEN: &str = "ASPIS_GIT_ASKPASS_TOKEN";

/// The GitHub username used for token (PAT/installation) auth over HTTPS. GitHub
/// accepts the literal `x-access-token` (or any non-empty username) paired with
/// the token as the password. This value is NON-secret and fixed.
const GIT_TOKEN_USERNAME: &str = "x-access-token";

/// Build the GIT_ASKPASS script body. The script holds NO secret: it branches on
/// the prompt text git passes as the first argument — if it mentions "Username"
/// it prints the fixed `x-access-token`, otherwise it prints the token read from
/// the `ASPIS_GIT_ASKPASS_TOKEN` env var (set only on the child git process).
///
/// cfg-gated per platform: Windows emits a `.cmd` batch script (git on Windows
/// honors GIT_ASKPASS pointing at a `.cmd`); unix emits a POSIX `sh` script with
/// a shebang. The pure string output is unit-tested on both arms.
#[cfg(windows)]
fn build_askpass_script() -> String {
    // FIX 3 (cmd-metacharacter injection): git passes the PROMPT text as the first
    // argument (`%~1`). It originates from the REMOTE and can contain cmd
    // metacharacters (`| & > < ^ %`). The previous `echo %~1 | findstr ...` EXPANDED
    // `%~1` straight onto the command line, so a hostile prompt could inject
    // commands. Instead we:
    //   - `@echo off` so the commands themselves never reach stdout (git captures
    //     this script's stdout; only the intended single line must appear),
    //   - `setlocal enabledelayedexpansion` + `set "PROMPT=%~1"` to capture the
    //     untrusted prompt into a variable WITHOUT re-parsing it,
    //   - compare via DELAYED expansion `!PROMPT!` (resolved at run time, after the
    //     line is parsed, so metacharacters in the value are inert),
    //   - emit the token via DELAYED expansion `!ASPIS_GIT_ASKPASS_TOKEN!` (never
    //     `%...%`, which would expand at parse time).
    // The token VALUE is never written into this file — only the env-var name is.
    format!(
        "@echo off\r\n\
         setlocal enabledelayedexpansion\r\n\
         set \"PROMPT=%~1\"\r\n\
         echo !PROMPT! | findstr /C:\"Username\" >nul\r\n\
         if !errorlevel!==0 (\r\n\
         echo {GIT_TOKEN_USERNAME}\r\n\
         ) else (\r\n\
         echo !{ASPIS_GIT_ASKPASS_TOKEN}!\r\n\
         )\r\n"
    )
}

/// Unix variant of [`build_askpass_script`] — a POSIX `sh` script. Same contract:
/// no secret in the file, branch on the prompt argument, read the token from the
/// `ASPIS_GIT_ASKPASS_TOKEN` env var. Made executable (0700) by the caller.
// UNVERIFIED on macOS — exercised by string-level tests on this Windows host;
// needs a real run on a Mac/Linux box (mirrors the mini-coder macOS-script gap).
#[cfg(not(windows))]
fn build_askpass_script() -> String {
    // `case "$1" in *Username*)` matches git's "Username for '...': " prompt; the
    // password branch echoes the env var (unset → empty line, which git treats as
    // an empty password and fails fast — never an interactive hang).
    format!(
        "#!/bin/sh\n\
         case \"$1\" in\n\
         *Username*) echo \"{GIT_TOKEN_USERNAME}\" ;;\n\
         *) echo \"${ASPIS_GIT_ASKPASS_TOKEN}\" ;;\n\
         esac\n"
    )
}

/// File suffix for the generated askpass script. Windows needs `.cmd` so git
/// (and the OS) execute it as a batch file; unix uses `.sh`.
#[cfg(windows)]
const ASKPASS_SUFFIX: &str = ".cmd";
#[cfg(not(windows))]
const ASKPASS_SUFFIX: &str = ".sh";

/// RAII guard that removes the generated askpass script (and its locked-down
/// per-call parent directory) on EVERY exit path — success, early return, or
/// panic. Mirrors the mini-coder's restricted-temp-file lifecycle. The script
/// holds no secret, but leaving it (and the env-var reference) behind is sloppy
/// and the parent restricted directory must not leak.
struct AskpassScriptGuard {
    path: PathBuf,
}

impl Drop for AskpassScriptGuard {
    fn drop(&mut self) {
        remove_restricted_temp_file(&self.path);
    }
}

/// Create the 0600/owner-only askpass script in a fresh restricted temp directory
/// and (on unix) mark it executable (0700) so git can exec it. Returns a guard
/// that deletes the script + its directory on drop, plus the script path.
///
/// ACCEPTED RESIDUAL RISK (Finding 7 — Windows icacls dir-ACL race, documented not
/// fixed): on Windows `create_restricted_temp_file` does `create_dir` and only then
/// applies the `icacls` owner-only ACL, leaving a narrow window where the directory
/// is readable by other local users. We ACCEPT this here because the askpass script
/// holds NO secret — its body is only the `@echo off` branch logic plus the NAME of
/// the env var (`ASPIS_GIT_ASKPASS_TOKEN`), never the token value. The token lives
/// solely in the child git process's environment block. Disclosure of this file
/// therefore reveals nothing sensitive, so hardening the shared restricted-temp-file
/// infra against this race is out of scope for this code path.
fn create_askpass_script() -> Result<AskpassScriptGuard, String> {
    let path = create_restricted_temp_file(
        &build_askpass_script(),
        "aspis-git-askpass-",
        ASKPASS_SUFFIX,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // git execs GIT_ASKPASS directly, so the script must be executable. The
        // restricted helper created it 0600 inside an owner-only (0700) directory;
        // raise it to 0700 (owner rwx, no group/other) — still owner-only.
        if let Err(e) = fs::set_permissions(&path, fs::Permissions::from_mode(0o700)) {
            remove_restricted_temp_file(&path);
            return Err(format!("Could not make the askpass script executable: {e}"));
        }
    }
    Ok(AskpassScriptGuard { path })
}

/// Build the non-secret `-c credential.helper=` prefix that neutralizes any
/// ambient credential helper, prepended to the caller's git args. Kept as a
/// helper so the invariant (empty value, argv-safe, no secret) is unit-tested.
fn credential_neutralizing_args() -> Vec<String> {
    // Empty value disables credential helpers for THIS invocation only; the value
    // is the empty string, so nothing secret is ever on argv.
    vec!["-c".into(), "credential.helper=".into()]
}

/// FIX 6 (MAX_PATH): the INTERNAL, non-secret `-c` config we prepend to every
/// authenticated git op (clone/pull/push). `core.longpaths=true` lets git on
/// Windows write paths longer than the legacy 259-char MAX_PATH limit — after we
/// strip the `\\?\` verbatim prefix from the canonicalized destination, a deep
/// repo path can exceed it and git would otherwise fail with a cryptic "Filename
/// too long". This is set by US (never via caller `args`), carries no secret, and
/// is a no-op on platforms without the limit. It is NOT passed through
/// `reject_unsafe_git_args` (that guards only caller-supplied args); a stray
/// caller `-c` is still rejected there.
fn internal_git_config_args() -> Vec<String> {
    vec!["-c".into(), "core.longpaths=true".into()]
}

/// FIX 4 (defense-in-depth identity redaction): replace every literal occurrence
/// of the live token in `text` with a fixed placeholder. `github::sanitize_error`
/// only catches the documented token PREFIXES, but git can surface the token in a
/// form with no recognizable prefix — base64 inside an `Authorization: Basic ...`
/// header, a GIT_TRACE dump, a credentialed URL. Because we hold the exact token
/// value here, a literal `replace` removes it regardless of encoding. A no-op when
/// the token is empty (so we never replace the empty string everywhere).
fn redact_token(token: &str, text: &str) -> String {
    if token.is_empty() {
        return text.to_string();
    }
    text.replace(token, "[redacted-github-token]")
}

/// FIX 6 (argv smuggling guard): reject any caller `args` that could re-introduce
/// the token onto argv or override our credential neutralization. `git_run_authenticated`
/// injects auth via GIT_ASKPASS only; a future caller must NOT be able to smuggle a
/// credential onto the command line. We refuse (no spawn) if any arg:
///   - mentions `http.extraHeader` (an Authorization header on argv),
///   - mentions `credential.helper` (could re-enable an ambient helper; ours is the
///     ONLY credential.helper, and it is prepended internally, never via `args`),
///   - looks like a credentialed URL (`://` together with a later `@`, e.g.
///     `https://x-access-token:TOKEN@github.com/...`),
///   - is a stray `-c` (only OUR internal `-c credential.helper=` is allowed; a
///     caller-supplied `-c` could set an arbitrary config override).
///
/// Returns the offending reason so the caller surfaces a clean error.
fn reject_unsafe_git_args(args: &[&str]) -> Result<(), String> {
    for arg in args {
        let lowered = arg.to_ascii_lowercase();
        if lowered.contains("http.extraheader") {
            return Err(
                "Refusing to run authenticated git: http.extraHeader is not allowed.".into(),
            );
        }
        if lowered.contains("credential.helper") {
            return Err(
                "Refusing to run authenticated git: credential.helper is not allowed.".into(),
            );
        }
        if *arg == "-c" {
            return Err("Refusing to run authenticated git: a -c override is not allowed.".into());
        }
        // A credentialed URL embeds the userinfo before an `@` that follows the
        // scheme separator `://` (e.g. `https://user:tok@host/...`).
        if let Some(idx) = arg.find("://") {
            if arg[idx + 3..].contains('@') {
                return Err(
                    "Refusing to run authenticated git: a credentialed URL is not allowed.".into(),
                );
            }
        }
    }
    Ok(())
}

/// Run a git subcommand in `path` authenticated with the stored GitHub PAT,
/// injected via GIT_ASKPASS so the token never touches argv, `.git/config`, the
/// PTY, or any log. Returns the same [`GitRunOutcome`] shape as [`git_run`];
/// stderr is capped AND run through [`github::sanitize_error`] so a token echoed
/// by git in its error output is redacted before it can surface to the UI.
///
/// `args` is the git subcommand argv WITHOUT the leading `git` (e.g.
/// `["push", "origin", "HEAD"]`). The credential-neutralizing `-c credential.helper=`
/// is prepended automatically. `timeout` bounds the wait before a hung child is
/// killed (a network push gets a longer budget than a local op).
///
/// Fails closed: if no GitHub token is configured we return a clean error and do
/// NOT fall back to ambient credentials for an authenticated operation.
fn git_run_authenticated(
    path: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<GitRunOutcome, String> {
    // 0) FIX 6: reject any caller args that could smuggle a credential onto argv or
    //    override our credential neutralization — BEFORE we touch the vault or spawn.
    reject_unsafe_git_args(args)?;

    // 1) Token from the vault. No token → fail closed (never silently use ambient
    //    creds for an op the caller explicitly asked to authenticate). No git is run.
    let token = vault::read_github_token()?
        .ok_or_else(|| "No GitHub token configured. Connect GitHub in Settings.".to_string())?;

    // 2) Write the (secret-free) askpass script to a locked-down temp file. The
    //    guard removes it + its directory on every exit path (incl. panic).
    let guard = create_askpass_script()?;
    let askpass_path = guard.path.clone();

    // 3) Assemble argv: our INTERNAL `-c` config (credential-helper neutralizer +
    //    FIX 6 `core.longpaths=true`) then the caller's args. The token is NEVER on
    //    argv. These internal `-c` entries are prepended by US and are deliberately
    //    NOT run through reject_unsafe_git_args (which guards only caller `args`).
    let mut full_args: Vec<String> = credential_neutralizing_args();
    full_args.extend(internal_git_config_args());
    full_args.extend(args.iter().map(|a| a.to_string()));

    let mut command = Command::new("git");
    command
        .args(&full_args)
        .current_dir(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // GIT_ASKPASS → our secret-free script; git invokes it for username/password.
        .env("GIT_ASKPASS", &askpass_path)
        // The token, on the CHILD env only — never global, never argv.
        .env(ASPIS_GIT_ASKPASS_TOKEN, &token)
        // Never block on an interactive prompt: a bad/missing token fails fast.
        .env("GIT_TERMINAL_PROMPT", "0")
        // Ignore the system git config (ambient system-wide credential helper).
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // FIX 4: explicitly disable every git trace channel so git never dumps the
        // Authorization header (which carries the base64-encoded token) into stderr
        // via an inherited GIT_TRACE*/GIT_CURL_VERBOSE from the ambient environment.
        .env("GIT_TRACE", "0")
        .env("GIT_TRACE_CURL", "0")
        .env("GIT_TRACE_PACKET", "0")
        .env("GIT_CURL_VERBOSE", "0")
        // FIX 2 (defense-in-depth): also neutralize the GIT_TRACE2 family. These are
        // distinct channels from the classic GIT_TRACE* set and can each emit the
        // Authorization header / credentialed URL into their own sink. Zero them so
        // no inherited GIT_TRACE2*/event/perf channel can dump the auth header.
        .env("GIT_TRACE2", "0")
        .env("GIT_TRACE2_EVENT", "0")
        .env("GIT_TRACE2_PERF", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no conhost flash from the release GUI exe.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            // guard drops here, removing the script. FIX 5: redact the token from
            // the spawn error too (cheap; the OS error is unlikely to carry it).
            return Err(redact_token(
                &token,
                &format!("git could not be started: {e}"),
            ));
        }
    };

    // FIX 1: drain both pipes concurrently while bounding the wait. A verbose
    // authenticated push (lots of progress on stderr) cannot deadlock the pipe and
    // get falsely timed out. FIX 5: the poll/timeout error strings from the helper
    // are routed through redact_token before surfacing.
    let drained = wait_with_drained_output(child, timeout).map_err(|e| redact_token(&token, &e))?;

    // FIX 8: cap stdout as well as stderr (untrusted hook output / large progress),
    // then run BOTH through the prefix sanitizer AND the identity redactor so a
    // token echoed by git — in any encoding — is scrubbed before it reaches the UI.
    let stderr = redact_token(
        &token,
        &super::github::sanitize_error(&cap_git_stderr(&String::from_utf8_lossy(&drained.stderr))),
    );
    let stdout = redact_token(
        &token,
        &super::github::sanitize_error(&cap_git_stderr(&String::from_utf8_lossy(&drained.stdout))),
    );
    Ok(GitRunOutcome {
        exit_code: drained.exit_code.unwrap_or(-1),
        stdout,
        stderr,
    })
    // guard drops here → script + restricted dir removed on every return path.
}

/// Validate + trim a commit message. Empty (after trim) is rejected so the UI
/// cannot create an empty-message commit; a cap keeps a pathological paste from
/// becoming the whole commit body.
fn validate_commit_message(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("Commit message must not be empty.".into());
    }
    if trimmed.chars().count() > 2000 {
        return Err("Commit message is too long (max 2000 characters).".into());
    }
    Ok(trimmed.to_string())
}

/// Build the argv for staging the tracked changes of the CURRENT branch. We add
/// only TRACKED, modified/deleted files (`git add -u`) — never untracked files
/// and never `git add -A` — so a commit from the UI cannot sweep in stray files.
fn git_add_tracked_args() -> Vec<String> {
    vec!["add".into(), "-u".into()]
}

/// Build the argv for committing the staged changes with `message`. The message
/// is a single argv entry (`-m <message>`), never shell-interpolated. No `--all`,
/// no `--amend` — a plain commit of what was just staged.
fn git_commit_args(message: &str) -> Vec<String> {
    vec!["commit".into(), "-m".into(), message.to_string()]
}

/// Build the argv for pushing the CURRENT branch to its remote. `HEAD` pushes
/// only the checked-out branch; `--set-upstream origin HEAD` is intentionally NOT
/// used here (we never invent a remote). NEVER contains `--force`/`-f`: a push
/// from the UI can only fast-forward, never rewrite remote history.
fn git_push_args() -> Vec<String> {
    vec!["push".into(), "origin".into(), "HEAD".into()]
}

/// GH-P4: build the argv for an AGENT-requested, human-APPROVED push. Like
/// `git_push_args` it pushes the repo's current `HEAD` (we never invent a branch
/// from agent-supplied text — the agent's `branch` is display-only on the card),
/// but it honors a validated `remote` (default `origin`) and, when the human
/// approved a FORCE push, appends `--force-with-lease` (the safest force: refuses
/// to clobber refs the local doesn't know about). A plain `--force` is deliberately
/// NOT used. The remote is validated by `validate_push_remote` BEFORE this is
/// called so it can never smuggle a flag or a credentialed URL onto argv.
fn git_push_request_args(remote: &str, force: bool) -> Vec<String> {
    let mut args = vec!["push".to_string(), remote.to_string(), "HEAD".to_string()];
    if force {
        args.push("--force-with-lease".to_string());
    }
    args
}

/// GH-P4: validate an agent-supplied remote NAME (e.g. `origin`, `upstream`). It is
/// placed on the `git push <remote> HEAD` argv, so it must be a bare token — letters,
/// digits, `.`, `_`, `-`, `/` — never a flag (`-`-leading), a URL, whitespace, or a
/// path-traversal/metachar. An empty/None remote defaults to `origin`. Mirrors the
/// bare-token discipline of `validate_mini_coder_backend`'s model-tag check.
fn validate_push_remote(remote: Option<&str>) -> Result<String, String> {
    let raw = remote.map(str::trim).unwrap_or("");
    if raw.is_empty() {
        return Ok("origin".to_string());
    }
    if raw.len() > 100 {
        return Err("Remote name is too long.".into());
    }
    let mut chars = raw.chars();
    let first = chars.next().unwrap(); // non-empty checked above
                                       // A leading '-' would be parsed as a flag by git; reject it outright.
    if !first.is_ascii_alphanumeric() {
        return Err("Remote name must start with a letter or digit.".into());
    }
    if !raw
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '/'))
    {
        return Err("Remote name may only contain letters, digits, . _ - /".into());
    }
    Ok(raw.to_string())
}

/// True when an argv vector contains any force-push flag. Used by the no-force
/// invariant test and as a defensive runtime guard before a push spawn.
fn args_contain_force(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg == "--force"
            || arg == "-f"
            || arg == "--force-with-lease"
            // `--force-with-lease=<ref>` is the attached-value form (e.g.
            // `--force-with-lease=main`); it still force-pushes, so reject it too.
            || arg.starts_with("--force-with-lease=")
    })
}

/// Resolve the git repo root for a project's configured agent root. Returns an
/// error when the root is not inside a git repository, so commit/push fail loudly
/// instead of operating on the wrong directory.
fn resolve_project_repo_root(project: &ParsedProject) -> Result<PathBuf, String> {
    let agent_root = resolve_project_agent_root(project)?;
    let repo_root_raw = git_output_timeout(&agent_root, &["rev-parse", "--show-toplevel"])
        .ok_or_else(|| "Project root is not inside a Git repository.".to_string())?;
    let repo_root = PathBuf::from(repo_root_raw.trim());
    Ok(repo_root.canonicalize().unwrap_or(repo_root))
}

/// Commit the staged + tracked changes of the project repo's CURRENT branch with
/// the given message. Stages tracked changes (`git add -u`), then commits. On a
/// git failure the trimmed stderr is surfaced so the UI shows the real reason.
#[tauri::command]
pub fn project_git_commit(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    project_id: String,
    message: String,
) -> Result<ProjectGitCommandResult, String> {
    state.ensure_unlocked()?;
    let commit_message = validate_commit_message(&message)?;
    let project = read_project_by_id(&app, &project_id)?;
    let repo_root = resolve_project_repo_root(&project)?;

    // Stage tracked changes of the current branch only (never untracked).
    let add_args = git_add_tracked_args();
    let add_argv: Vec<&str> = add_args.iter().map(String::as_str).collect();
    let add = git_run(&repo_root, &add_argv, GIT_LOCAL_TIMEOUT)?;
    if add.exit_code != 0 {
        return Err(if add.stderr.is_empty() {
            "git add failed.".into()
        } else {
            add.stderr
        });
    }

    let commit_args = git_commit_args(&commit_message);
    let commit_argv: Vec<&str> = commit_args.iter().map(String::as_str).collect();
    let commit = git_run(&repo_root, &commit_argv, GIT_LOCAL_TIMEOUT)?;
    if commit.exit_code != 0 {
        // "nothing to commit" is a non-zero exit; surface git's own message.
        return Err(if commit.stderr.is_empty() {
            if commit.stdout.is_empty() {
                "git commit failed.".into()
            } else {
                commit.stdout
            }
        } else {
            commit.stderr
        });
    }

    let git_status = project_git_status(project.metadata.root_path.as_deref());
    let branch = git_status.branch.clone().unwrap_or_default();
    // Best-effort: kick an incremental Oracle index if the index_mode pref is
    // "commit" AND this committed repo is within the Oracle index root. The call
    // is fire-and-forget (returns immediately) and must not fail the git command.
    crate::backend::oracle_service::notify_local_commit(&repo_root);
    Ok(ProjectGitCommandResult {
        project_id,
        branch,
        message: "Committed staged changes on the current branch.".into(),
        git_status,
    })
}

/// Push the project repo's CURRENT branch to origin. NEVER force-pushes. On a git
/// failure the trimmed stderr is surfaced so the UI shows the real reason (e.g.
/// no upstream, rejected non-fast-forward).
///
/// F47: async + spawn_blocking — authenticated push reads the GitHub PAT from the
/// keyring via `git_run_authenticated`.
#[tauri::command]
pub async fn project_git_push(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    project_id: String,
) -> Result<ProjectGitCommandResult, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(move || {
        let project = read_project_by_id(&app, &project_id)?;
        let repo_root = resolve_project_repo_root(&project)?;

        let push_args = git_push_args();
        // Defense in depth: refuse to run a push whose argv somehow carries a force
        // flag. The argv is built by git_push_args() (asserted force-free in tests),
        // so this can only trip if that helper regresses.
        if args_contain_force(&push_args) {
            return Err("Refusing to force-push from the app.".into());
        }
        let push_argv: Vec<&str> = push_args.iter().map(String::as_str).collect();
        // Authenticated push: the GitHub PAT is injected via GIT_ASKPASS (off argv,
        // off .git/config, off the PTY/logs). Fails closed if no token is configured.
        let push = git_run_authenticated(&repo_root, &push_argv, GIT_PUSH_TIMEOUT)?;
        if push.exit_code != 0 {
            return Err(if push.stderr.is_empty() {
                "git push failed.".into()
            } else {
                push.stderr
            });
        }

        let git_status = project_git_status(project.metadata.root_path.as_deref());
        let branch = git_status.branch.clone().unwrap_or_default();
        Ok(ProjectGitCommandResult {
            project_id,
            branch,
            message: "Pushed the current branch to origin.".into(),
            git_status,
        })
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
}

// ---------------------------------------------------------------------------
// GH-P4: agent push-approval gate (human-resolved)
// ---------------------------------------------------------------------------
//
// Agents COMMIT freely but every PUSH must be approved by the human. The agent's
// MCP `request_git_push` tool appends a `pending_approval` GitPushRequest to
// `.aspis-agents.json` and BOUNDED-polls its verdict; the human, via the
// PushApprovalCard, calls these commands. There is NO background executor for this
// gate — the human IS the resolver, and the APPROVE command itself runs the push.
//
// LOCK DISCIPLINE (mirrors mini_coder_executor): the agent-state file lock is NEVER
// held across the network push. Approve = (locked: claim pending_approval ->
// approved, re-checking status so a double-approve / approve-after-timeout no-ops)
// -> RELEASE the lock -> run `git_run_authenticated` -> (locked: stamp the result +
// transition pushed/push_failed + clear needs_user). See the GitPushStatus module
// doc for the TIMEOUT/STALE decision.

/// GH-P4: list the current git push-approval requests for the PushApprovalCard.
/// Returns the whole queue (the UI filters to `pending_approval` for the card and
/// may surface recent terminal results). Gated on the app being unlocked.
///
/// FIX F2 — list-time reconciliation: this is the safety net for a push-approve whose
/// step-3 finalize NEVER landed (the lock could not be re-acquired even with the
/// retried budget), which would otherwise leave a request stuck `approved`/`pushing`
/// with the requesting agent's `needs_user` bell lit FOREVER. Each card refresh (and
/// app startup, since the Work-mode shell mounts the card) sweeps such STUCK requests
/// (older than the grace window so a live in-flight push is never touched), stamps
/// them terminal `push_failed`, and clears the bell. The sweep runs under the state
/// lock; on a normal queue with nothing stuck it writes nothing extra of substance
/// (the closure still rewrites the file, matching every other mutate path).
#[tauri::command]
pub fn git_push_requests_list(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
) -> Result<Vec<GitPushRequest>, String> {
    state.ensure_unlocked()?;
    let now = Utc::now().to_rfc3339();
    // Read-only fast path: the card polls every ~5s, so the COMMON case (nothing
    // stuck) must NOT take the write lock + rewrite the state file on every tick.
    // We snapshot first and only escalate to a locked mutate when a stuck request is
    // actually present (`reconcile_stuck_requests` would return at least one agent to
    // clear). The mutate re-runs reconciliation under the lock against the live state
    // (the snapshot may be stale), so the decision is re-validated before any write.
    let snapshot = super::agents::read_agent_live_state_snapshot(&app)?;
    let mut probe = snapshot.git_push_requests.clone();
    if git_push::reconcile_stuck_requests(&mut probe, &now).is_empty() {
        return Ok(snapshot.git_push_requests);
    }
    super::agents::mutate_agent_live_state(&app, |live| {
        let cleared = git_push::reconcile_stuck_requests(&mut live.git_push_requests, &now);
        for agent_id in &cleared {
            clear_request_needs_user(live, agent_id);
        }
        live.git_push_requests.clone()
    })
}

/// GH-P4: clear the requesting agent session's `needs_user` bell. Called on EVERY
/// terminal path of a push request (approved-pushed, approved-push-failed, denied)
/// so the bell never lingers after the human acted. A missing session is a no-op.
/// Operates on the live state INSIDE the caller's locked mutation closure.
fn clear_request_needs_user(state: &mut super::model::AgentLiveState, agent_id: &str) {
    if agent_id.is_empty() {
        return;
    }
    if let Some(session) = state.sessions.iter_mut().find(|s| s.agent_id == agent_id) {
        session.needs_user = None;
    }
}

/// GH-P4: approve a pending push request and PERFORM the push.
///
/// Concurrency (the reviewer WILL attack these):
///   * DOUBLE-APPROVE: two clicks -> only one push. The claim transition
///     (`pending_approval -> approved`) is done UNDER THE LOCK re-reading the LIVE
///     status; the second click sees not-`pending_approval` and no-ops (returns the
///     already-resolved/in-flight request).
///   * APPROVE-AFTER-TIMEOUT / -DENY: the request already went terminal (the agent's
///     poll swept it, or it was denied) -> the claim is refused (idempotent), NO push.
///   * LOCK NOT HELD ACROSS THE NETWORK PUSH: claim under lock -> release -> push ->
///     re-lock to record. (mirrors mini_coder claim_and_launch.)
///   * PROJECT AUTHORIZATION: the request's `projectId` must resolve to a real
///     project repo root; otherwise the request fails (`push_failed`) and nothing is
///     pushed.
///   * TOKEN never surfaced: the push runs via `git_run_authenticated`, which redacts
///     the token from stdout/stderr before we ever store it on the request.
///   * needs_user cleared on the pushed AND push_failed paths.
/// F47: async + spawn_blocking — approve path calls `git_run_authenticated` (keyring).
#[tauri::command]
pub async fn approve_git_push_request(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    request_id: String,
) -> Result<GitPushRequest, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(move || {
        approve_git_push_request_blocking(app, request_id)
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
}

fn approve_git_push_request_blocking(
    app: tauri::AppHandle,
    request_id: String,
) -> Result<GitPushRequest, String> {
    let request_id = request_id.trim().to_string();
    if request_id.is_empty() {
        return Err("Missing push request id.".into());
    }

    // 1) CLAIM under the lock: pending_approval -> approved. Re-reads the LIVE status
    //    so a double-approve / approve-after-terminal is a no-op. Returns the claimed
    //    request (a clone) on success, or None if it was not claimable.
    let claimed: Option<GitPushRequest> = super::agents::mutate_agent_live_state(&app, |live| {
        let result = {
            let Some(req) = live
                .git_push_requests
                .iter_mut()
                .find(|r| r.id == request_id)
            else {
                return None;
            };
            // FIX F2: stamp the approval time so list-time reconciliation can
            // tell a live in-flight push from a stuck one.
            match git_push::apply_approve(req, Utc::now().to_rfc3339()) {
                Ok(next) => {
                    *req = next.clone();
                    Some(next)
                }
                Err(_) => None, // not pending_approval (double-approve / terminal).
            }
        };
        git_push::cap_push_requests(&mut live.git_push_requests, git_push::MAX_PUSH_REQUESTS);
        result
    })?;

    let Some(claimed) = claimed else {
        // Did not win the claim: surface the current (terminal/in-flight) request so
        // the UI updates, without pushing. A vanished request is an error.
        let snapshot = super::agents::read_agent_live_state_snapshot(&app)?;
        return snapshot
            .git_push_requests
            .into_iter()
            .find(|r| r.id == request_id)
            .ok_or_else(|| "Push request not found (it may have been evicted).".to_string());
    };

    // 2) Resolve + validate the target repo + push argv OUTSIDE the lock. A bad
    //    project / remote fails the request cleanly (approved -> pushing -> push_failed)
    //    and pushes nothing.
    let push_outcome = run_approved_push(&app, &claimed);

    // 3) Re-lock and FINALIZE: record the real outcome of the push that ALREADY RAN
    //    and CLEAR needs_user. This step is CRITICAL — the push already happened, so a
    //    failure here would leave the bell stuck and the outcome unrecorded. Two
    //    hardenings (FIX F2 + F6):
    //
    //    * FIX F2 — robust re-acquire: use `mutate_agent_live_state_retrying` so a
    //      contended lock gets a multiplied budget instead of the single ~5s spin.
    //    * FIX F6 — outcome wins over a speculative timeout: while the push ran
    //      OUTSIDE the lock, the Python poll may have stamped the request `timeout`
    //      (it gave up). The strict `apply_push_result` (pushing-only) would then
    //      swallow the real outcome and leave a misleading `timeout` though the push
    //      physically landed. We use `apply_push_result_override`, which reconciles
    //      `approved | pushing | timeout -> pushed | push_failed`, so the REAL,
    //      human-approved outcome is recorded (correct audit, no double-push risk).
    //
    //    The finalize closure is idempotent (it re-checks the live status and only
    //    records a not-yet-recorded request), so re-running it across retries is safe.
    // FIX 6: keep a captured agent_id ONLY for the out-of-closure best-effort clear
    // in the Err branch below (the closure may never run / the req may be gone there).
    // Inside the closure we read agent_id from the LIVE req under the finalize lock
    // rather than from the stale `claimed` clone — robust even though agent_id is
    // immutable per session.
    let agent_id = claimed.agent_id.clone();
    let finalize_result = super::agents::mutate_agent_live_state_retrying(&app, 4, |live| {
        let (result, live_agent_id) = {
            let Some(req) = live
                .git_push_requests
                .iter_mut()
                .find(|r| r.id == request_id)
            else {
                return None;
            };
            let live_agent_id = req.agent_id.clone();
            // Reconcile to the real outcome from approved/pushing/timeout. A refusal
            // (already a REAL terminal — pushed/push_failed/denied — e.g. a racing
            // duplicate finalize) is swallowed so a late result never clobbers a
            // recorded real outcome or a human denial.
            let resolved =
                if let Ok(done) = git_push::apply_push_result_override(req, push_outcome.clone()) {
                    *req = done.clone();
                    Some(done)
                } else {
                    Some(req.clone())
                };
            (resolved, live_agent_id)
        };
        // needs_user cleared on every terminal path (pushed AND push_failed), using
        // the agent_id read from the live request above (FIX 6).
        clear_request_needs_user(live, &live_agent_id);
        git_push::cap_push_requests(&mut live.git_push_requests, git_push::MAX_PUSH_REQUESTS);
        result
    });

    match finalize_result {
        Ok(Some(finalized)) => Ok(finalized),
        Ok(None) => Err("Push request not found after push.".to_string()),
        // FIX F2: the push completed but recording the result kept failing (the lock
        // could not be re-acquired even with the multiplied budget). Make a separate
        // best-effort attempt to at least CLEAR the bell so it does not stay lit
        // forever, then surface a clear, actionable error. The persisted request is
        // left in its drifted state (approved/timeout); the list-time reconciliation
        // in `git_push_requests_list` will stamp it terminal on the next refresh.
        Err(e) => {
            let _ = super::agents::mutate_agent_live_state(&app, |live| {
                clear_request_needs_user(live, &agent_id);
            });
            Err(format!(
                "Push completed but recording the result failed ({e}). The push DID land; \
                 the approval bell may need a manual refresh of the push-approval list."
            ))
        }
    }
}

/// GH-P4: deny a pending push request. pending_approval -> denied, CLEAR needs_user,
/// NO push. Idempotent: a non-pending request (already approved / pushing / terminal)
/// is a no-op that returns the current request.
#[tauri::command]
pub fn deny_git_push_request(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    request_id: String,
) -> Result<GitPushRequest, String> {
    state.ensure_unlocked()?;
    let request_id = request_id.trim().to_string();
    if request_id.is_empty() {
        return Err("Missing push request id.".into());
    }
    let result: Option<GitPushRequest> = super::agents::mutate_agent_live_state(&app, |live| {
        // FIX F10: track whether the deny ACTUALLY transitioned the request, so we
        // only clear the bell when it did. The no-op path (request not
        // pending_approval — e.g. it is `pushing`) must NOT clear `needs_user`:
        // clearing it while a push is in flight would drop the bell prematurely.
        let (resolved, agent_id, transitioned) = {
            let Some(req) = live
                .git_push_requests
                .iter_mut()
                .find(|r| r.id == request_id)
            else {
                return None;
            };
            let agent_id = req.agent_id.clone();
            match git_push::apply_deny(req) {
                Ok(next) => {
                    *req = next.clone();
                    (Some(next), agent_id, true)
                }
                // Not pending (already approved/pushing/terminal): no-op, return
                // current WITHOUT clearing the bell.
                Err(_) => (Some(req.clone()), agent_id, false),
            }
        };
        // needs_user cleared ONLY on the real denied terminal transition.
        if transitioned {
            clear_request_needs_user(live, &agent_id);
        }
        git_push::cap_push_requests(&mut live.git_push_requests, git_push::MAX_PUSH_REQUESTS);
        resolved
    })?;
    result.ok_or_else(|| "Push request not found (it may have been evicted).".to_string())
}

/// GH-P4: run the actual authenticated push for an APPROVED request, OUTSIDE the
/// state lock. Resolves + validates the project repo root and the remote, refuses a
/// forced argv that somehow lacks the approved flag's safety, and runs
/// `git_run_authenticated` (token off argv/logs, stderr redacted). Returns the
/// terminal `GitPushResult` (pushed | push_failed) — NEVER carries a raw token (the
/// error string is the already-sanitized git stderr / app message).
fn run_approved_push(app: &tauri::AppHandle, request: &GitPushRequest) -> GitPushResult {
    // Project authorization: the request's projectId must resolve to a real repo.
    let project = match read_project_by_id(app, &request.project_id) {
        Ok(p) => p,
        Err(e) => return GitPushResult::push_failed(None, e),
    };
    let repo_root = match resolve_project_repo_root(&project) {
        Ok(r) => r,
        Err(e) => return GitPushResult::push_failed(None, e),
    };
    let remote = match validate_push_remote(request.remote.as_deref()) {
        Ok(r) => r,
        Err(e) => return GitPushResult::push_failed(None, e),
    };

    let push_args = git_push_request_args(&remote, request.force);
    // Defense in depth: if the human did NOT approve a force, the argv must be
    // force-free. (For an approved force, --force-with-lease IS expected.)
    if !request.force && args_contain_force(&push_args) {
        return GitPushResult::push_failed(
            None,
            "Refusing to force-push a non-force request.".to_string(),
        );
    }
    let push_argv: Vec<&str> = push_args.iter().map(String::as_str).collect();
    match git_run_authenticated(&repo_root, &push_argv, GIT_PUSH_TIMEOUT) {
        Ok(outcome) if outcome.exit_code == 0 => {
            let msg = if outcome.stdout.is_empty() {
                if outcome.stderr.is_empty() {
                    "Pushed.".to_string()
                } else {
                    outcome.stderr
                }
            } else {
                outcome.stdout
            };
            GitPushResult::pushed(msg)
        }
        Ok(outcome) => {
            let err = if outcome.stderr.is_empty() {
                "git push failed.".to_string()
            } else {
                outcome.stderr
            };
            GitPushResult::push_failed(Some(outcome.exit_code), err)
        }
        // `git_run_authenticated` already redacts the token from its error string.
        Err(e) => GitPushResult::push_failed(None, e),
    }
}

/// PURE: strip the Windows extended-length verbatim prefix (`\\?\`, incl. the UNC
/// form `\\?\UNC\`) from a canonicalized path string. `std::fs::canonicalize` on
/// Windows returns a `\\?\C:\...` verbatim path, which `git clone <dest>` can choke
/// on as a destination argument. Removing the prefix yields a plain `C:\...` path
/// git accepts. A no-op on non-verbatim paths and on every non-Windows platform
/// (their canonical paths never carry the prefix), so it is safe to call always.
fn strip_verbatim_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        // `\\?\UNC\server\share` → `\\server\share`
        return format!(r"\\{rest}");
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    path.to_string()
}

/// PURE: derive a SAFE on-disk directory name for a clone from a validated
/// `(owner, repo)` pair. `parse_github_repo` already runs both segments through
/// `clean_github_path_segment` (ascii alnum / `-` / `_` / `.` only, no separators,
/// length-capped), so the repo name is already free of path separators, `..`, and
/// absolute-path markers. We defensively re-assert here so a future change to the
/// parser cannot silently let a traversal name through: a name that is empty, `.`,
/// `..`, contains a path separator, a drive-letter `:`, or a leading separator is
/// rejected. Returns the bare directory NAME (never a path) on success.
fn clone_dir_name(repo: &str) -> Result<String, String> {
    let name = repo.trim();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains(':')
        || name.contains('\0')
    {
        return Err("Repository name is not a safe directory name.".into());
    }
    // FIX 3: reject Windows reserved DEVICE names (case-insensitive), including
    // when used as the stem before the first dot (`NUL.txt` is still the NUL
    // device on Windows). GitHub itself rejects these so a real URL cannot reach
    // here, but this validator is the documented authority — assert it anyway. The
    // guard is platform-independent so a clone made on macOS that is later opened
    // on Windows can never carry a name Windows refuses to create.
    if is_windows_reserved_device_name(name) {
        return Err(
            "Repository name is a reserved device name and is not a safe directory name.".into(),
        );
    }
    Ok(name.to_string())
}

/// PURE: true when `name`'s stem (the part before the first `.`) is a Windows
/// reserved device name (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`),
/// matched case-insensitively. On Windows these names are devices regardless of
/// any extension, so `NUL`, `nul.txt`, and `Com1.tar.gz` are all rejected.
fn is_windows_reserved_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    let upper = stem.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => {
            // COM1–COM9 / LPT1–LPT9: a 3-char prefix + a single 1–9 digit.
            (upper
                .strip_prefix("COM")
                .or_else(|| upper.strip_prefix("LPT")))
            .map(|d| d.len() == 1 && matches!(d.as_bytes()[0], b'1'..=b'9'))
            .unwrap_or(false)
        }
    }
}

/// PURE: build the CREDENTIAL-FREE plain https URL handed to `git clone`. The PAT
/// is injected by `git_run_authenticated` via GIT_ASKPASS, so the URL must NEVER
/// carry userinfo (no `user:token@`). We construct it from the already-validated
/// `(owner, repo)` segments — not from the raw pasted string — so nothing the user
/// typed (a smuggled `user:pass@`, a query, a fragment) can ride along.
fn plain_clone_url(owner: &str, repo: &str) -> String {
    format!("https://github.com/{owner}/{repo}.git")
}

/// PURE predicate: true when `dir` exists AND contains at least one entry. Used to
/// REFUSE cloning into an existing non-empty directory (never clobber user files).
/// A missing dir or an empty dir is fine (`false`). An unreadable existing dir is
/// treated as non-empty (conservative: refuse rather than risk clobbering).
fn dir_is_non_empty(dir: &Path) -> bool {
    match fs::read_dir(dir) {
        Ok(mut entries) => entries.next().is_some(),
        // Does not exist → not a blocker. Any other read error → treat as occupied.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

/// Resolve the BASE directory clones land in. An explicit, validated `dest_parent`
/// wins; otherwise we fall back to the same Desktop base the default agent root
/// lives under (so a clone sits next to the user's existing projects), and finally
/// to `USERPROFILE`/`HOME`. Returns a real, existing directory.
fn clone_base_dir(dest_parent: Option<&str>) -> Result<PathBuf, String> {
    if let Some(parent) = normalize_project_root(dest_parent) {
        let path = PathBuf::from(&parent);
        if !path.is_dir() {
            return Err(format!("Destination folder does not exist: {parent}"));
        }
        let resolved = path
            .canonicalize()
            .map_err(|e| format!("Destination folder could not be resolved: {e}"))?;
        reject_broad_project_root(&resolved)?;
        return Ok(resolved);
    }
    // No explicit parent: clone next to the user's other projects (Desktop), then
    // fall back to the home directory. Never the broad roots reject_broad_* guards.
    // FIX 10: pick the home var per-platform (mirrors real_global_gitconfig_paths).
    // USERPROFILE is the Windows home; on macOS/Linux it is unset and HOME is the
    // correct one, so it must not be consulted first there.
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let home =
        home.ok_or_else(|| "Could not determine a home directory for the clone.".to_string())?;
    let desktop = home.join("Desktop");
    let base = if desktop.is_dir() { desktop } else { home };
    base.canonicalize()
        .map_err(|e| format!("Clone base folder could not be resolved: {e}"))
}

/// Clone a GitHub repository into a safe destination and REGISTER it as a project.
///
/// The PAT is injected via GIT_ASKPASS by `git_run_authenticated` — it is NEVER on
/// argv, in the clone URL, in `.git/config`, or in any log. `url` is validated with
/// the SAME `parse_github_repo` the rest of the app uses (https/github.com only);
/// the URL actually handed to git is rebuilt from the validated owner/repo, so a
/// smuggled credentialed URL cannot reach git. `--` precedes the URL so a URL
/// starting with `-` can never be read as a flag. We refuse to clone into an
/// existing non-empty directory (never clobber). On success the cloned working
/// tree is registered as a project rooted at the new directory.
/// F47: async + spawn_blocking for the authenticated clone (keyring + network);
/// project registration stays on the command after the blocking section.
#[tauri::command]
pub async fn project_git_clone(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    url: String,
    dest_parent: Option<String>,
) -> Result<ProjectDetail, String> {
    state.ensure_unlocked()?;

    // Pure validation + destination claim + authenticated clone off the main thread.
    let clone_result = tauri::async_runtime::spawn_blocking(move || {
        // 1) Validate the remote with the canonical parser (https/github.com only).
        let (owner, repo) = super::github::parse_github_repo(&url).ok_or_else(|| {
            "Enter a valid GitHub repository URL (https://github.com/owner/repo).".to_string()
        })?;

        // 2) Safe destination: <base>/<safe repo name>. Both pieces validated.
        let dir_name = clone_dir_name(&repo)?;
        let base = clone_base_dir(dest_parent.as_deref())?;
        let dest = base.join(&dir_name);

        // 3) Never clobber: cheap pre-check so an OBVIOUSLY occupied destination gives a
        //    clear error before we attempt anything. This is advisory only — the real
        //    guard is the atomic exclusive create in step 4 (this read→create gap is a
        //    TOCTOU window the exclusive create closes).
        if dir_is_non_empty(&dest) {
            return Err(format!(
                "A non-empty folder named \"{dir_name}\" already exists here. Move or remove it first."
            ));
        }

        // 4) FIX 5 (TOCTOU): atomically claim the destination. `fs::create_dir`
        //    (NON-recursive) is exclusive — it fails with AlreadyExists if anything is
        //    already there (an existing empty dir, a symlink, or a racing concurrent
        //    clone that won the create). This closes the check→create race and the
        //    empty-symlink write-through window: we proceed ONLY when WE created the dir,
        //    which also makes the FIX-2 cleanup below safe (we never remove a pre-existing
        //    directory we did not create). git clones cleanly into a pre-made empty dir.
        match fs::create_dir(&dest) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(format!(
                    "A folder named \"{dir_name}\" already exists here. Move or remove it first."
                ));
            }
            Err(e) => {
                return Err(format!("Could not create the clone destination: {e}"));
            }
        }

        // 5) Credential-free plain URL, rebuilt from validated segments (never the raw
        //    input). `--` guards against a URL being parsed as a flag. The dest is the
        //    verbatim-prefix-stripped path (git clone chokes on `\\?\C:\...`).
        let clone_url = plain_clone_url(&owner, &repo);
        let dest_str = strip_verbatim_prefix(&dest.to_string_lossy());
        let clone = match git_run_authenticated(
            &base,
            &["clone", "--", &clone_url, &dest_str],
            GIT_CLONE_TIMEOUT,
        ) {
            Ok(outcome) => outcome,
            Err(e) => {
                // We created the (empty) dest; git may have left a partial tree. Remove
                // the dir WE own so a retry is not blocked by the clobber guard.
                let _ = fs::remove_dir_all(&dest);
                return Err(e);
            }
        };
        if clone.exit_code != 0 {
            // git failed (bad URL/auth/network). Tear down the dir WE created (and any
            // partial clone in it) so the user can retry without hitting the guard.
            let _ = fs::remove_dir_all(&dest);
            return Err(if clone.stderr.is_empty() {
                "git clone failed.".into()
            } else {
                clone.stderr
            });
        }
        Ok((repo, dir_name, dest, dest_str))
    })
    .await
    .map_err(|e| format!("task join error: {e}"))??;

    let (repo, dir_name, dest, dest_str) = clone_result;

    // 6) Register the cloned working tree as a project rooted at the new dir. Reuse
    //    the canonical create_project path so the new project is identical in shape
    //    to a hand-created one (id slug, censor-untrusted default, status active).
    //    FIX 2: if registration fails (e.g. a duplicate project id), the clone is
    //    already on disk — remove the dir WE created so the user is not left with an
    //    orphaned, un-re-clonable folder, and explain what happened.
    //    create_project is sync disk I/O (no keyring); keep it outside spawn_blocking
    //    so State<'_, BackendState> does not need to cross the 'static boundary.
    match create_project(
        app,
        state,
        ProjectCreateInput {
            id: None,
            title: repo.clone(),
            status: Some("active".into()),
            root_path: Some(dest_str),
        },
    ) {
        Ok(detail) => Ok(detail),
        Err(reason) => match fs::remove_dir_all(&dest) {
            Ok(()) => Err(format!(
                "Clone succeeded but registering the project failed: {reason}. The cloned folder was removed."
            )),
            Err(cleanup_err) => Err(format!(
                "Clone succeeded but registering the project failed: {reason}. \
                 The cloned folder could not be removed automatically: {cleanup_err}. \
                 Remove \"{dir_name}\" manually before retrying."
            )),
        },
    }
}

/// Build the argv for `git pull --ff-only` on the current branch. `--ff-only`
/// guarantees the pull either fast-forwards cleanly or fails loudly — it can NEVER
/// create a merge commit or touch the working tree on a divergence (v1 surfaces
/// the conflict for the user to resolve manually, it does not auto-merge).
fn git_pull_args() -> Vec<String> {
    vec!["pull".into(), "--ff-only".into()]
}

/// Pull (fast-forward only) the project repo's current branch from its remote.
/// Authenticated via GIT_ASKPASS (token off argv/config/logs). On a non-fast-
/// forward / divergence git fails and `--ff-only` leaves the working tree CLEAN;
/// we surface git's (already sanitized) message telling the user to resolve it
/// manually, never swallowing the error.
/// F47: async + spawn_blocking — pull reads the GitHub PAT via keyring.
#[tauri::command]
pub async fn project_git_pull(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    project_id: String,
) -> Result<ProjectGitCommandResult, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(move || {
        let project = read_project_by_id(&app, &project_id)?;
        let repo_root = resolve_project_repo_root(&project)?;

        let pull_args = git_pull_args();
        let pull_argv: Vec<&str> = pull_args.iter().map(String::as_str).collect();
        let pull = git_run_authenticated(&repo_root, &pull_argv, GIT_PULL_TIMEOUT)?;
        if pull.exit_code != 0 {
            return Err(if pull.stderr.is_empty() {
                if pull.stdout.is_empty() {
                    "git pull failed.".into()
                } else {
                    pull.stdout
                }
            } else {
                pull.stderr
            });
        }

        let git_status = project_git_status(project.metadata.root_path.as_deref());
        let branch = git_status.branch.clone().unwrap_or_default();
        // Best-effort: kick an incremental Oracle index if the index_mode pref is
        // "commit" AND this pulled repo is within the Oracle index root. The call is
        // fire-and-forget (returns immediately) and must not fail the git command.
        crate::backend::oracle_service::notify_local_commit(&repo_root);
        Ok(ProjectGitCommandResult {
            project_id,
            branch,
            message: "Pulled the latest changes (fast-forward) from origin.".into(),
            git_status,
        })
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
}

fn sanitize_git_remote(value: &str) -> String {
    if let Some((scheme, rest)) = value.trim().split_once("://") {
        if let Some(at) = rest.find('@') {
            return format!("{scheme}://{}", &rest[at + 1..]);
        }
    }
    value.trim().to_string()
}

fn github_web_url_from_origin(origin: &str) -> Option<String> {
    let mut remote = origin.trim().trim_end_matches(".git").to_string();
    if let Some(path) = remote.strip_prefix("git@github.com:") {
        remote = format!("https://github.com/{path}");
    } else if let Some(path) = remote.strip_prefix("ssh://git@github.com/") {
        remote = format!("https://github.com/{path}");
    } else if let Some(path) = remote.strip_prefix("http://github.com/") {
        remote = format!("https://github.com/{path}");
    }
    let path = remote.strip_prefix("https://github.com/")?;
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    let owner = parts.next()?;
    let repo = parts.next()?;
    Some(format!("https://github.com/{owner}/{repo}"))
}

fn suggested_git_repos_for_root(root: &Path) -> Vec<ProjectGitRepoCandidate> {
    let Some(csv_path) = find_workspace_git_repos_csv(root) else {
        return Vec::new();
    };
    let Ok(content) = fs::read_to_string(csv_path) else {
        return Vec::new();
    };
    let mut lines = content.lines();
    let Some(header_line) = lines.next() else {
        return Vec::new();
    };
    let headers = parse_project_csv_line(header_line);
    let root_canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    lines
        .filter_map(|line| {
            let row = parse_project_csv_line(line);
            let field = |name: &str| -> Option<String> {
                headers
                    .iter()
                    .position(|header| header.eq_ignore_ascii_case(name))
                    .and_then(|index| row.get(index))
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            };
            let path = PathBuf::from(field("Path")?);
            let path_canonical = path.canonicalize().unwrap_or(path.clone());
            if !path_canonical.starts_with(&root_canonical) {
                return None;
            }
            let origin = field("Origin").map(|value| sanitize_git_remote(&value));
            if !origin
                .as_deref()
                .is_some_and(|value| value.contains("github.com"))
            {
                return None;
            }
            let name = field("Name").unwrap_or_else(|| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("repo")
                    .to_string()
            });
            let clone_command = origin
                .as_deref()
                .map(|remote| format!("git clone {}", remote.trim_end_matches(".git")));
            Some(ProjectGitRepoCandidate {
                name,
                path: path.to_string_lossy().into_owned(),
                branch: field("Branch"),
                origin,
                dirty_count: field("DirtyCount")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0),
                clone_command,
            })
        })
        .take(8)
        .collect()
}

fn find_workspace_git_repos_csv(root: &Path) -> Option<PathBuf> {
    for candidate in root.ancestors() {
        let path = candidate
            .join("_workspace")
            .join("inventory")
            .join("git-repos.csv");
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn parse_project_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut in_quotes = false;
    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                current.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(current.clone());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    fields.push(current);
    fields
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::super::projects::{forbidden_ancestor_dirs, path_is_under_forbidden_ancestor};

    // --- Work-mode commit/push (Phase D) argument-safety + validation ---------

    #[test]
    fn commit_message_rejects_empty_and_whitespace() {
        assert!(validate_commit_message("").is_err());
        assert!(validate_commit_message("   \t\n").is_err());
        assert_eq!(validate_commit_message("  fix bug ").unwrap(), "fix bug");
    }

    #[test]
    fn commit_message_rejects_overlong() {
        let long = "x".repeat(2001);
        assert!(validate_commit_message(&long).is_err());
        let ok = "x".repeat(2000);
        assert_eq!(validate_commit_message(&ok).unwrap().chars().count(), 2000);
    }

    #[test]
    fn git_add_stages_tracked_only_never_all() {
        let args = git_add_tracked_args();
        assert_eq!(args, vec!["add".to_string(), "-u".to_string()]);
        // Never `-A`/`--all`: a UI commit must not sweep in untracked files.
        assert!(!args.iter().any(|a| a == "-A" || a == "--all"));
    }

    #[test]
    fn git_commit_args_use_dash_m_single_message_argv() {
        let args = git_commit_args("a tricky \"message\" with spaces");
        assert_eq!(
            args,
            vec![
                "commit".to_string(),
                "-m".to_string(),
                "a tricky \"message\" with spaces".to_string()
            ]
        );
        // No --all / --amend: a plain commit of what was staged.
        assert!(!args
            .iter()
            .any(|a| a == "--all" || a == "--amend" || a == "-a"));
    }

    #[test]
    fn git_push_targets_current_branch_and_never_forces() {
        let args = git_push_args();
        // Pushes only the checked-out branch (HEAD) to origin.
        assert_eq!(
            args,
            vec!["push".to_string(), "origin".to_string(), "HEAD".to_string()]
        );
        // The no-force invariant: no force flag in any form.
        assert!(!args_contain_force(&args));
        assert!(!args.iter().any(|a| a == "--force" || a == "-f"));
    }

    #[test]
    fn args_contain_force_detects_every_force_variant() {
        assert!(args_contain_force(&["push".into(), "--force".into()]));
        assert!(args_contain_force(&["push".into(), "-f".into()]));
        assert!(args_contain_force(&[
            "push".into(),
            "--force-with-lease".into()
        ]));
        // The attached-value form `--force-with-lease=<ref>` must also be rejected.
        assert!(args_contain_force(&[
            "push".into(),
            "--force-with-lease=main".into()
        ]));
        assert!(args_contain_force(&[
            "push".into(),
            "--force-with-lease=origin/main".into()
        ]));
        assert!(!args_contain_force(&["push".into(), "origin".into()]));
        // A flag merely *containing* the substring but not a force flag is allowed.
        assert!(!args_contain_force(&[
            "push".into(),
            "--no-force-with-lease".into()
        ]));
    }

    // --- GH-P4: approved push argv + remote validation -------------------------

    #[test]
    fn git_push_request_args_default_remote_no_force() {
        let args = git_push_request_args("origin", false);
        assert_eq!(
            args,
            vec!["push".to_string(), "origin".to_string(), "HEAD".to_string()]
        );
        assert!(!args_contain_force(&args));
    }

    #[test]
    fn git_push_request_args_honors_custom_remote_and_force() {
        let args = git_push_request_args("upstream", true);
        assert_eq!(
            args,
            vec![
                "push".to_string(),
                "upstream".to_string(),
                "HEAD".to_string(),
                "--force-with-lease".to_string(),
            ]
        );
        // An APPROVED force IS detected as a force (the card warns; the human OK'd it).
        assert!(args_contain_force(&args));
    }

    #[test]
    fn validate_push_remote_defaults_and_accepts_bare_tokens() {
        assert_eq!(validate_push_remote(None).unwrap(), "origin");
        assert_eq!(validate_push_remote(Some("   ")).unwrap(), "origin");
        assert_eq!(validate_push_remote(Some("origin")).unwrap(), "origin");
        assert_eq!(validate_push_remote(Some("upstream")).unwrap(), "upstream");
        assert_eq!(validate_push_remote(Some("fork-2.0")).unwrap(), "fork-2.0");
    }

    #[test]
    fn validate_push_remote_rejects_flags_urls_and_metachars() {
        // A leading '-' (would be a git flag), a URL, whitespace, and a credentialed
        // form must all be rejected so a remote can never smuggle a flag onto argv.
        assert!(validate_push_remote(Some("--force")).is_err());
        assert!(validate_push_remote(Some("-f")).is_err());
        assert!(validate_push_remote(Some("https://github.com/a/b.git")).is_err());
        assert!(validate_push_remote(Some("origin extra")).is_err());
        assert!(validate_push_remote(Some("a;rm -rf b")).is_err());
        assert!(validate_push_remote(Some("user:tok@host")).is_err());
        assert!(validate_push_remote(Some(&"x".repeat(200))).is_err());
    }

    // --- P2: secure GIT_ASKPASS token injection (token OFF argv/disk/logs) ------

    #[test]
    fn askpass_script_branches_username_vs_password_and_holds_no_secret() {
        let script = build_askpass_script();
        // Username branch prints the fixed, NON-secret token username.
        assert!(
            script.contains(GIT_TOKEN_USERNAME),
            "askpass script must echo the fixed username for the Username prompt"
        );
        assert!(
            script.contains("x-access-token"),
            "username literal must be x-access-token"
        );
        // Password branch reads the token from the env var by NAME — the script
        // file itself must reference only the variable, never embed a token value.
        assert!(
            script.contains(ASPIS_GIT_ASKPASS_TOKEN),
            "askpass script must reference the token env var by name"
        );
        // It must branch on the prompt: the Username prompt is matched explicitly.
        assert!(
            script.contains("Username"),
            "askpass script must distinguish the Username prompt from the password"
        );
        // No GitHub token prefix may appear literally in the generated script —
        // the script is non-secret by construction; this guards against a regression
        // that accidentally interpolates a token into the file body.
        for prefix in ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"] {
            assert!(
                !script.contains(prefix),
                "askpass script must NOT contain any literal token (found {prefix})"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn askpass_script_is_a_batch_file_on_windows() {
        let script = build_askpass_script();
        assert!(
            script.starts_with("@echo off"),
            "Windows askpass must be a .cmd batch script"
        );
        // FIX 3: the script must use the injection-safe delayed-expansion form.
        assert!(
            script.contains("setlocal enabledelayedexpansion"),
            "must enable delayed expansion so the untrusted prompt is inert"
        );
        assert!(
            script.contains("set \"PROMPT=%~1\""),
            "must capture the untrusted prompt into a variable, not expand it inline"
        );
        // The prompt is compared via DELAYED expansion `!PROMPT!`, never bare `%~1`.
        assert!(
            script.contains("echo !PROMPT! | findstr"),
            "must compare via delayed expansion, not pipe the raw arg"
        );
        assert!(
            !script.contains("echo %~1"),
            "must NOT echo the raw %~1 (cmd-metacharacter injection)"
        );
        // The token is emitted via DELAYED expansion `!VAR!`, never `%VAR%` (which
        // would expand at parse time). Either way it is the NAME, not the value.
        assert!(
            script.contains(&format!("!{ASPIS_GIT_ASKPASS_TOKEN}!")),
            "token env var must be read via delayed expansion"
        );
        assert!(
            !script.contains(&format!("%{ASPIS_GIT_ASKPASS_TOKEN}%")),
            "must not use parse-time %VAR% expansion for the token"
        );
        assert_eq!(ASKPASS_SUFFIX, ".cmd");
    }

    #[cfg(not(windows))]
    #[test]
    fn askpass_script_is_a_posix_sh_script_on_unix() {
        let script = build_askpass_script();
        assert!(
            script.starts_with("#!/bin/sh"),
            "unix askpass must start with a sh shebang"
        );
        // Reads the token via shell env-var expansion $VAR, never a literal value.
        assert!(script.contains(&format!("${ASPIS_GIT_ASKPASS_TOKEN}")));
        assert!(
            script.contains("case"),
            "unix askpass must branch on the prompt with case"
        );
        assert_eq!(ASKPASS_SUFFIX, ".sh");
    }

    #[test]
    fn credential_neutralizing_args_disable_ambient_helper_with_empty_value() {
        let args = credential_neutralizing_args();
        // `-c credential.helper=` (empty value) neutralizes any ambient helper for
        // this invocation only. The value is empty, so nothing secret is on argv.
        assert_eq!(
            args,
            vec!["-c".to_string(), "credential.helper=".to_string()]
        );
        // Must NOT use the insecure alternatives the plan explicitly rejects.
        assert!(
            !args.iter().any(|a| a.contains("http.extraHeader")),
            "must not inject the token via an HTTP header on argv"
        );
        assert!(
            !args
                .iter()
                .any(|a| a.contains("x-access-token:") || a.contains('@')),
            "must not build a credentialed remote URL on argv"
        );
    }

    #[test]
    fn askpass_env_var_name_is_the_off_global_child_only_name() {
        // The token env var name is a fixed, app-specific identifier so it never
        // collides with a real git env var and is obviously app-scoped.
        assert_eq!(ASPIS_GIT_ASKPASS_TOKEN, "ASPIS_GIT_ASKPASS_TOKEN");
    }

    #[test]
    fn create_askpass_script_writes_then_cleans_up_on_drop() {
        // The guard creates a restricted temp script and removes it (and its
        // per-call directory) on drop — the cleanup-on-every-exit-path invariant.
        let path;
        {
            let guard = create_askpass_script().expect("askpass script should be creatable");
            path = guard.path.clone();
            assert!(
                path.exists(),
                "askpass script must exist while the guard is alive"
            );
            let body = fs::read_to_string(&path).expect("script readable");
            assert!(body.contains(ASPIS_GIT_ASKPASS_TOKEN));
            // No literal token in the on-disk file.
            assert!(!body.contains("ghp_"));
        }
        // Guard dropped: script AND its parent restricted directory are gone.
        assert!(
            !path.exists(),
            "askpass script must be removed on guard drop"
        );
        if let Some(parent) = path.parent() {
            assert!(
                !parent.exists(),
                "the per-call restricted directory must be removed too"
            );
        }
    }

    #[test]
    fn git_run_authenticated_surfaces_sanitized_errors() {
        // Sanity that the same sanitizer used by the HTTP path scrubs a token from
        // any surfaced text (the authenticated git path runs stderr through it).
        let dirty = "remote: error pushing with ghp_AbCdEf0123456789secrettoken value";
        let clean = super::super::github::sanitize_error(dirty);
        assert!(!clean.contains("ghp_AbCdEf0123456789secrettoken"));
        assert!(clean.contains("[redacted-github-token]"));
    }

    #[test]
    fn redact_token_strips_literal_token_with_no_recognizable_prefix() {
        // FIX 4: a token can surface base64-encoded / mid-string with NO documented
        // prefix (e.g. inside an Authorization: Basic header or a GIT_TRACE dump).
        // The prefix sanitizer would miss it; redact_token removes the literal value.
        let token = "AbCdEf0123_no_prefix_here_456";
        let dirty = format!("Authorization: Basic eA=={token} more text and {token}again");
        let clean = redact_token(token, &dirty);
        assert!(
            !clean.contains(token),
            "literal token must be removed: {clean}"
        );
        assert!(clean.contains("[redacted-github-token]"));
        // An empty token must be a no-op (never replace the empty string everywhere).
        assert_eq!(redact_token("", "anything stays"), "anything stays");
        // A non-matching token leaves the text untouched.
        assert_eq!(redact_token("zzz", "no token here"), "no token here");
    }

    #[test]
    fn reject_unsafe_git_args_blocks_credential_smuggling() {
        // FIX 6: a future caller must NOT be able to put a credential back on argv.
        // http.extraHeader (Authorization header on argv).
        assert!(
            reject_unsafe_git_args(&["-c", "http.extraHeader=Authorization: Basic x"]).is_err()
        );
        assert!(reject_unsafe_git_args(&["-c", "HTTP.ExtraHeader=foo"]).is_err());
        // credential.helper override.
        assert!(reject_unsafe_git_args(&["-c", "credential.helper=store"]).is_err());
        // A stray -c (could set any config override) is rejected.
        assert!(reject_unsafe_git_args(&["-c", "core.pager=less"]).is_err());
        // A credentialed URL (userinfo before @ after ://).
        assert!(
            reject_unsafe_git_args(&["push", "https://x-access-token:tok@github.com/o/r"]).is_err()
        );
        // The legitimate push argv is accepted (no -c here; ours is prepended
        // internally, and a plain github URL with no userinfo is fine).
        assert!(reject_unsafe_git_args(&["push", "origin", "HEAD"]).is_ok());
        assert!(reject_unsafe_git_args(&["push", "https://github.com/o/r"]).is_ok());
    }

    // --- P3 clone/pull pure-helper guards -------------------------------------

    #[test]
    fn clone_dir_name_rejects_traversal_separators_and_absolute() {
        // A traversal / separator / drive-letter / empty name must never become a
        // clone destination directory NAME.
        assert!(clone_dir_name("..").is_err());
        assert!(clone_dir_name(".").is_err());
        assert!(clone_dir_name("").is_err());
        assert!(clone_dir_name("   ").is_err());
        assert!(clone_dir_name("a/b").is_err());
        assert!(clone_dir_name("a\\b").is_err());
        assert!(clone_dir_name("C:evil").is_err());
        assert!(clone_dir_name("nul\0byte").is_err());
        // A normal repo name passes through verbatim (already segment-sanitized).
        assert_eq!(clone_dir_name("Aspis-bio").unwrap(), "Aspis-bio");
        assert_eq!(clone_dir_name(" my_repo.git-x ").unwrap(), "my_repo.git-x");
    }

    #[test]
    fn clone_dir_name_rejects_windows_reserved_device_names() {
        // FIX 3: Windows reserved device names must never become a directory name,
        // case-insensitively, including as the stem before a dot (`NUL.txt` is the
        // NUL device). GitHub rejects these but this validator is the authority.
        for name in [
            "CON",
            "con",
            "PRN",
            "AUX",
            "NUL",
            "nul",
            "COM1",
            "com9",
            "LPT1",
            "lpt9",
            "NUL.txt",
            "Com1.tar.gz",
            "aux.md",
        ] {
            assert!(
                clone_dir_name(name).is_err(),
                "reserved device name must be rejected: {name}"
            );
        }
        // Near-misses that are NOT reserved must still pass.
        for name in [
            "COM0",
            "COM10",
            "LPT0",
            "CONSOLE",
            "container",
            "comet",
            "lptest",
        ] {
            assert!(
                clone_dir_name(name).is_ok(),
                "non-reserved name must be accepted: {name}"
            );
        }
    }

    #[test]
    fn is_windows_reserved_device_name_matches_only_real_devices() {
        // PURE predicate: exact device names + COM1-9/LPT1-9, any case, stem-only.
        assert!(is_windows_reserved_device_name("CON"));
        assert!(is_windows_reserved_device_name("nul"));
        assert!(is_windows_reserved_device_name("COM3"));
        assert!(is_windows_reserved_device_name("lpt7.log"));
        // Not devices: digit out of range, no digit, longer word, embedded.
        assert!(!is_windows_reserved_device_name("COM0"));
        assert!(!is_windows_reserved_device_name("COM12"));
        assert!(!is_windows_reserved_device_name("COM"));
        assert!(!is_windows_reserved_device_name("CONSOLE"));
        assert!(!is_windows_reserved_device_name("my-con"));
    }

    #[test]
    fn path_is_under_forbidden_ancestor_confines_dest_parent() {
        // FIX 4: a candidate that IS or is nested under a forbidden ancestor is
        // rejected; a sibling whose name merely shares a prefix is NOT.
        let sep = std::path::MAIN_SEPARATOR;
        let appdata = PathBuf::from(format!("C:{sep}Users{sep}me{sep}AppData{sep}Roaming"));
        let temp = PathBuf::from(format!("C:{sep}Temp"));
        let forbidden = vec![appdata.clone(), temp.clone()];

        // Exact match → forbidden.
        assert!(path_is_under_forbidden_ancestor(&appdata, &forbidden));
        // Nested (e.g. the Startup folder) → forbidden.
        let startup = appdata
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Startup");
        assert!(path_is_under_forbidden_ancestor(&startup, &forbidden));
        // Case-insensitive (Windows fs) → still forbidden.
        let lowercased = PathBuf::from(appdata.to_string_lossy().to_lowercase());
        assert!(path_is_under_forbidden_ancestor(&lowercased, &forbidden));
        // Prefix-sibling (C:\Tempest) is NOT under C:\Temp.
        let tempest = PathBuf::from(format!("C:{sep}Tempest{sep}repo"));
        assert!(!path_is_under_forbidden_ancestor(&tempest, &forbidden));
        // A normal Desktop project is not under any forbidden ancestor.
        let desktop = PathBuf::from(format!("C:{sep}Users{sep}me{sep}Desktop{sep}repo"));
        assert!(!path_is_under_forbidden_ancestor(&desktop, &forbidden));
        // An empty forbidden entry never matches (guards against a blank env var).
        assert!(!path_is_under_forbidden_ancestor(
            &desktop,
            &[PathBuf::new()]
        ));
    }

    #[test]
    fn internal_git_config_enables_longpaths_and_is_argv_safe() {
        // FIX 6: we prepend `-c core.longpaths=true` ourselves (never via caller
        // args) so a deep clone path past MAX_PATH on Windows does not fail. It is
        // non-secret and must NOT trip the caller-arg smuggling guard.
        assert_eq!(
            internal_git_config_args(),
            vec!["-c".to_string(), "core.longpaths=true".to_string()]
        );
        // The smuggling guard validates only CALLER args; our internal `-c` config
        // is prepended after this check, so a clone/pull caller arg-set is clean.
        assert!(
            reject_unsafe_git_args(&["clone", "--", "https://github.com/o/r.git", "dest"]).is_ok()
        );
        assert!(reject_unsafe_git_args(&["pull", "--ff-only"]).is_ok());
    }

    #[test]
    fn drain_capped_caps_storage_but_consumes_whole_source() {
        // FIX 7: a source larger than the store cap is bounded in memory, yet fully
        // CONSUMED (drained) so a real pipe would never block the child.
        let big = vec![b'A'; DRAIN_STORE_CAP_BYTES + 500_000];
        let mut reader = std::io::Cursor::new(big.clone());
        let stored = drain_capped(Some(&mut reader));
        assert_eq!(
            stored.len(),
            DRAIN_STORE_CAP_BYTES,
            "stored buffer must be capped at the byte budget"
        );
        // The cursor was read to EOF (position advanced past the whole source).
        assert_eq!(
            reader.position() as usize,
            big.len(),
            "the entire source must be consumed, not just the stored prefix"
        );
        // A small source is stored in full (happy path unchanged).
        let mut small = std::io::Cursor::new(b"short git error".to_vec());
        assert_eq!(drain_capped(Some(&mut small)), b"short git error");
        // A None reader yields an empty buffer (the take()-d pipe was absent).
        let none: Option<&mut std::io::Cursor<Vec<u8>>> = None;
        assert!(drain_capped(none).is_empty());
    }

    #[test]
    fn strip_verbatim_prefix_removes_windows_extended_length_markers() {
        // `git clone <dest>` chokes on a `\\?\` verbatim path from canonicalize().
        assert_eq!(
            strip_verbatim_prefix(r"\\?\C:\Users\me\Desktop\repo"),
            r"C:\Users\me\Desktop\repo"
        );
        assert_eq!(
            strip_verbatim_prefix(r"\\?\UNC\server\share\repo"),
            r"\\server\share\repo"
        );
        // A plain path (and any POSIX path) is returned unchanged.
        assert_eq!(strip_verbatim_prefix(r"C:\plain\path"), r"C:\plain\path");
        assert_eq!(strip_verbatim_prefix("/home/me/repo"), "/home/me/repo");
    }

    #[test]
    fn parse_github_repo_rejects_non_github_urls() {
        // The clone command validates the pasted URL through this canonical parser;
        // a non-github / non-https URL must be rejected so we never clone elsewhere.
        assert!(super::super::github::parse_github_repo("https://evil.example/o/r").is_none());
        assert!(super::super::github::parse_github_repo("ftp://github.com/o/r").is_none());
        assert!(super::super::github::parse_github_repo("not a url").is_none());
        assert_eq!(
            super::super::github::parse_github_repo("https://github.com/Saurias92/Aspis-bio.git"),
            Some(("Saurias92".into(), "Aspis-bio".into()))
        );
    }

    #[test]
    fn plain_clone_url_carries_no_credentials() {
        // The URL handed to `git clone` is rebuilt from validated segments and must
        // NEVER contain userinfo (no `user:token@`) — the PAT goes via GIT_ASKPASS.
        let url = plain_clone_url("Saurias92", "Aspis-bio");
        assert_eq!(url, "https://github.com/Saurias92/Aspis-bio.git");
        assert!(
            !url.contains('@'),
            "clone URL must not embed userinfo: {url}"
        );
        // No documented GitHub token prefix may appear in the URL we build.
        for prefix in ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"] {
            assert!(
                !url.contains(prefix),
                "token prefix {prefix} leaked into URL"
            );
        }
        // Defense in depth: the URL we build is accepted by the argv-smuggling guard
        // (no credentialed-URL pattern), so it can be passed to git_run_authenticated.
        assert!(reject_unsafe_git_args(&["clone", "--", &url, "dest"]).is_ok());
    }

    #[test]
    fn dir_is_non_empty_refuses_existing_occupied_destination() {
        // Missing dir → not a blocker.
        let missing =
            std::env::temp_dir().join(format!("aspis-clone-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&missing);
        assert!(!dir_is_non_empty(&missing));

        // Empty dir → not a blocker (a clone may target a freshly-made empty dir).
        let empty = std::env::temp_dir().join(format!("aspis-clone-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&empty);
        fs::create_dir_all(&empty).unwrap();
        assert!(!dir_is_non_empty(&empty));

        // Non-empty dir → BLOCKER: a clone here would clobber user files.
        let occupied =
            std::env::temp_dir().join(format!("aspis-clone-occupied-{}", std::process::id()));
        let _ = fs::remove_dir_all(&occupied);
        fs::create_dir_all(&occupied).unwrap();
        fs::write(occupied.join("keep.txt"), b"data").unwrap();
        assert!(dir_is_non_empty(&occupied));

        let _ = fs::remove_dir_all(&empty);
        let _ = fs::remove_dir_all(&occupied);
    }

    #[test]
    fn git_pull_args_are_ff_only() {
        // The pull command must ALWAYS run `pull --ff-only` (never a plain pull that
        // could create a merge commit / dirty the tree on a divergence).
        let args = git_pull_args();
        assert_eq!(args, vec!["pull".to_string(), "--ff-only".to_string()]);
        assert!(args.iter().any(|a| a == "--ff-only"));
        // No force / no rebase / no merge-strategy flags sneak in.
        assert!(!args
            .iter()
            .any(|a| a == "--rebase" || a == "-f" || a == "--force"));
    }

    #[test]
    fn git_run_authenticated_fails_closed_without_a_token() {
        // FIX 2: exercise the SECURITY function itself. When no GitHub token is
        // configured, git_run_authenticated must return the clean no-token error
        // WITHOUT spawning git. We only assert when the vault genuinely has no token
        // (the normal state on a dev/CI box); if a token IS configured we skip so
        // the test never depends on machine keyring state.
        match vault::read_github_token() {
            Ok(None) => {
                let res = git_run_authenticated(
                    Path::new("."),
                    &["push", "origin", "HEAD"],
                    GIT_PUSH_TIMEOUT,
                );
                let err = res.expect_err("must fail closed when no token is configured");
                assert!(
                    err.contains("No GitHub token configured"),
                    "fail-closed error should name the missing token: {err}"
                );
            }
            _ => {
                // A token is present (or the keyring errored) — skip rather than
                // run a real authenticated push from the test suite.
            }
        }
    }

    #[test]
    fn git_run_authenticated_rejects_unsafe_args_before_touching_the_vault() {
        // FIX 6 end-to-end: an unsafe arg is refused before any vault read / spawn,
        // regardless of whether a token is configured on this machine.
        let res = git_run_authenticated(
            Path::new("."),
            &["-c", "http.extraHeader=Authorization: Basic x", "push"],
            GIT_PUSH_TIMEOUT,
        );
        let err = res.expect_err("unsafe args must be rejected");
        assert!(
            err.contains("Refusing to run authenticated git"),
            "must reject the smuggled credential arg: {err}"
        );
    }

    #[test]
    fn wait_with_drained_output_handles_large_output_without_deadlock() {
        // FIX 1: a child that writes MORE than the OS pipe buffer (~64KB) to both
        // stdout and stderr must complete and be fully drained — the old busy-poll
        // would deadlock (git blocks on the full pipe, never exits) and time out.
        if !git_available() {
            return;
        }
        // Emit a large blob to stdout. `git` is guaranteed present here; use a git
        // subcommand whose output we can size up: `git --help -a` is large but not
        // huge, so instead drive a portable large write via the platform shell.
        #[cfg(windows)]
        let mut command = {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            // ~200KB to stdout via a cmd FOR loop, well over the 64KB pipe buffer.
            let mut c = Command::new("cmd");
            c.args([
                "/C",
                "for /L %i in (1,1,4000) do @echo AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            ]);
            c.creation_flags(CREATE_NO_WINDOW);
            c
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut c = Command::new("sh");
            // ~200KB to stdout, well over the 64KB pipe buffer.
            c.args(["-c", "i=0; while [ $i -lt 4000 ]; do echo AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA; i=$((i+1)); done"]);
            c
        };
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let child = command.spawn().expect("spawn large-output child");
        let drained = wait_with_drained_output(child, Duration::from_secs(30))
            .expect("large output must drain and complete, not time out");
        assert_eq!(drained.exit_code, Some(0), "child should exit cleanly");
        assert!(
            drained.stdout.len() > 100_000,
            "stdout should be fully drained (>100KB), got {} bytes",
            drained.stdout.len()
        );
    }

    #[test]
    fn wait_with_drained_output_times_out_and_kills_a_hung_child() {
        // FIX 1: a genuinely hung child must still be killed at the timeout and the
        // reader threads must not hang the join (the kill closes the pipes → EOF).
        // Spawn a process that DIRECTLY holds the piped stdout (no shell grandchild),
        // so killing it closes the pipe write-end → the reader hits EOF and the join
        // returns promptly. This mirrors `git` itself (git owns its stdout/stderr).
        #[cfg(windows)]
        let mut command = {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let mut c = Command::new("ping");
            // ~30s of pings to stdout; we time out at 1s and must kill it.
            c.args(["-n", "30", "127.0.0.1"]);
            c.creation_flags(CREATE_NO_WINDOW);
            c
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut c = Command::new("sh");
            c.args(["-c", "sleep 30"]);
            c
        };
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let child = command.spawn().expect("spawn hung child");
        let started = Instant::now();
        let res = wait_with_drained_output(child, Duration::from_secs(1));
        assert!(res.is_err(), "a hung child must time out");
        assert!(
            res.unwrap_err().contains("timed out"),
            "timeout error message expected"
        );
        // The kill+join must return promptly, not wait out the full 30s sleep.
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timeout path must not block on the child's full runtime"
        );
    }

    #[test]
    fn wait_with_drained_output_abandons_join_when_grandchild_holds_pipe() {
        // FIX 5: simulate the Windows hazard where killing the parent does NOT close
        // the inherited pipe write-end because a grandchild still holds it open. We
        // spawn a shell whose own stdout is the pipe AND which forks a long-lived
        // grandchild that inherits and holds that same stdout. Killing the shell
        // leaves the grandchild writing/holding the pipe, so the reader thread never
        // hits EOF. The function MUST still return the timeout error within the
        // bounded grace window (a few seconds) instead of blocking forever.
        //
        // Cross-platform note: `Child::kill` on unix kills only the immediate child
        // (no process-group kill), so the grandchild survives there too — this models
        // the same hazard on both platforms. The Windows arm uses a detached `start`
        // grandchild; the unix arm uses a backgrounded `sleep` that inherits stdout.
        #[cfg(windows)]
        let mut command = {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let mut c = Command::new("cmd");
            // Launch a detached child that inherits this stdout and lingers ~30s,
            // then the parent cmd exits — but the pipe stays open via the grandchild.
            c.args([
                "/C",
                "start /b cmd /C ping -n 30 127.0.0.1 & ping -n 30 127.0.0.1",
            ]);
            c.creation_flags(CREATE_NO_WINDOW);
            c
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut c = Command::new("sh");
            // Background a grandchild `sleep` that inherits stdout, then the parent
            // shell also sleeps. Killing the shell leaves the grandchild holding the
            // pipe write-end open, so the reader thread cannot hit EOF.
            c.args(["-c", "sleep 30 & sleep 30"]);
            c
        };
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let child = command.spawn().expect("spawn pipe-holding child");
        let started = Instant::now();
        let res = wait_with_drained_output(child, Duration::from_secs(1));
        assert!(res.is_err(), "a hung child must time out");
        assert!(
            res.unwrap_err().contains("timed out"),
            "timeout error message expected"
        );
        // Must return within timeout(1s) + 2*grace(3s) + slack, NOT block forever on
        // the grandchild that keeps the pipe open.
        assert!(
            started.elapsed() < Duration::from_secs(12),
            "bounded-abandon path must not block on a grandchild-held pipe, took {:?}",
            started.elapsed()
        );
    }

    // Real-repo integration test: REQUIRES a `git` binary on PATH. When git is
    // absent (CI without git) it self-skips via `git_available()` rather than
    // failing, so the suite stays green on a minimal host.
    #[test]
    fn project_git_commit_push_real_repo_current_branch() {
        if !git_available() {
            return;
        }
        let root = temp_project_root("git-commit-push");
        // Init a repo with an identity + a committed baseline.
        assert!(Command::new("git")
            .arg("init")
            .current_dir(&root)
            .status()
            .unwrap()
            .success());
        for cfg in [
            ["config", "user.email", "test@example.com"],
            ["config", "user.name", "Test"],
        ] {
            let _ = Command::new("git").args(cfg).current_dir(&root).status();
        }
        fs::write(root.join("tracked.txt"), "v1\n").unwrap();
        let _ = Command::new("git")
            .args(["add", "tracked.txt"])
            .current_dir(&root)
            .status();
        let _ = Command::new("git")
            .args(["commit", "-m", "baseline"])
            .current_dir(&root)
            .status();

        // Modify the tracked file + drop an untracked file that must NOT be swept.
        fs::write(root.join("tracked.txt"), "v2\n").unwrap();
        fs::write(root.join("untracked.txt"), "scratch\n").unwrap();

        let repo_root = root.canonicalize().unwrap_or(root.clone());
        // Stage tracked-only, then commit, exactly as project_git_commit does.
        let add = git_run(
            &repo_root,
            &git_add_tracked_args()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            GIT_LOCAL_TIMEOUT,
        )
        .unwrap();
        assert_eq!(add.exit_code, 0);
        let commit_args = git_commit_args("work-mode commit");
        let commit = git_run(
            &repo_root,
            &commit_args.iter().map(String::as_str).collect::<Vec<_>>(),
            GIT_LOCAL_TIMEOUT,
        )
        .unwrap();
        assert_eq!(commit.exit_code, 0, "stderr: {}", commit.stderr);

        // The untracked file is still uncommitted (was never staged).
        let porcelain =
            git_output_timeout(&repo_root, &["status", "--porcelain=v1"]).unwrap_or_default();
        assert!(
            porcelain.contains("untracked.txt"),
            "untracked file must remain uncommitted: {porcelain}"
        );

        // A second commit with nothing staged surfaces git's non-zero exit.
        let nothing = git_run(
            &repo_root,
            &git_commit_args("noop")
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            GIT_LOCAL_TIMEOUT,
        )
        .unwrap();
        assert_ne!(nothing.exit_code, 0);

        // FIX 2: exercise the AUTHENTICATED push path (askpass injection, credential
        // neutralization, sanitize/redact), not the bare git_run that has no auth.
        match vault::read_github_token() {
            Ok(None) => {
                // No token configured: git_run_authenticated must fail CLOSED with the
                // clean no-token error and NOT spawn git. This is the security gate.
                let push = git_run_authenticated(
                    &repo_root,
                    &git_push_args()
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                    GIT_PUSH_TIMEOUT,
                );
                let err = push.expect_err("must fail closed without a token");
                assert!(
                    err.contains("No GitHub token configured"),
                    "fail-closed error expected: {err}"
                );
            }
            _ => {
                // A token IS available: drive the FULL askpass chain against a local
                // `file://` bare remote so no network/secret is involved. The bare
                // remote uses no auth, but git_run_authenticated still injects the
                // askpass script + neutralizes ambient creds, exercising that code.
                let bare = temp_project_root("git-bare-remote");
                let bare_repo = bare.canonicalize().unwrap_or(bare.clone());
                assert!(Command::new("git")
                    .args(["init", "--bare"])
                    .current_dir(&bare_repo)
                    .status()
                    .unwrap()
                    .success());
                // Commit so there is something to push, then add the file:// remote.
                let commit2 = git_run(
                    &repo_root,
                    &git_commit_args("push-me")
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                    GIT_LOCAL_TIMEOUT,
                );
                let _ = commit2; // may be "nothing to commit"; either is fine here.
                let remote_url = format!("file://{}", bare_repo.to_string_lossy());
                let _ = Command::new("git")
                    .args(["remote", "add", "origin", &remote_url])
                    .current_dir(&repo_root)
                    .status();
                let push = git_run_authenticated(
                    &repo_root,
                    &git_push_args()
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                    GIT_PUSH_TIMEOUT,
                )
                .expect("authenticated push to a file:// remote should run");
                // No token value (or any prefix) may leak into the surfaced output.
                for prefix in ["ghp_", "github_pat_"] {
                    assert!(
                        !push.stdout.contains(prefix) && !push.stderr.contains(prefix),
                        "no token may surface in push output"
                    );
                }
                let _ = fs::remove_dir_all(&bare_repo);
            }
        }

        let _ = fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn cap_git_stderr_truncates_long_output_keeps_short() {
        // A short, real-git-shaped error is returned verbatim (trimmed).
        let short = "  fatal: not a git repository  ";
        assert_eq!(cap_git_stderr(short), "fatal: not a git repository");
        // A hostile hook dumping a long blob (e.g. echoing a secret) is bounded.
        let long = "A".repeat(GIT_STDERR_MAX_CHARS + 5000);
        let capped = cap_git_stderr(&long);
        assert!(
            capped.chars().count() <= GIT_STDERR_MAX_CHARS + 32,
            "len: {}",
            capped.chars().count()
        );
        assert!(capped.ends_with("[git output truncated]"));
        // The cap is on CHARS and never panics on multibyte input.
        let multi = "é".repeat(GIT_STDERR_MAX_CHARS + 10);
        let capped_multi = cap_git_stderr(&multi);
        assert!(capped_multi.contains('é'));
    }


    #[test]
    fn github_origin_parser_accepts_common_remote_shapes() {
        assert_eq!(
            github_web_url_from_origin("https://github.com/Saurias92/Aspis-bio.git"),
            Some("https://github.com/Saurias92/Aspis-bio".into())
        );
        assert_eq!(
            github_web_url_from_origin("git@github.com:Saurias92/Aspis-bio.git"),
            Some("https://github.com/Saurias92/Aspis-bio".into())
        );
        assert_eq!(
            github_web_url_from_origin("ssh://git@github.com/Saurias92/Aspis-bio.git"),
            Some("https://github.com/Saurias92/Aspis-bio".into())
        );
    }

    #[test]
    fn non_git_workspace_suggests_github_repo_roots() {
        let root = temp_project_root("suggested-repos");
        let inventory = root.join("_workspace").join("inventory");
        fs::create_dir_all(&inventory).unwrap();
        let repo = root.join("aspis-lab");
        fs::create_dir_all(&repo).unwrap();
        fs::write(
            inventory.join("git-repos.csv"),
            "\"Path\",\"Name\",\"Branch\",\"Origin\",\"DirtyCount\",\"GitSize\"\n\"C:\\\\outside\",\"outside\",\"main\",\"https://github.com/Saurias92/outside.git\",\"0\",\"\"\n\"".to_string()
                + &repo.to_string_lossy().replace('"', "\"\"")
                + "\",\"aspis-lab\",\"feature/work\",\"https://github.com/Saurias92/Aspis-bio.git\",\"3\",\"\"\n",
        )
        .unwrap();

        let status = project_git_status(Some(&root.to_string_lossy()));

        let _ = fs::remove_dir_all(&root);

        assert_eq!(status.policy_status, "blocked");
        assert!(!status.is_git_repo);
        assert_eq!(status.suggested_repos.len(), 1);
        assert_eq!(status.suggested_repos[0].name, "aspis-lab");
        assert_eq!(status.suggested_repos[0].dirty_count, 3);
    }

    #[test]
    fn project_git_status_reports_dirty_github_repo() {
        if !git_available() {
            return;
        }
        let root = temp_project_root("git-policy");
        fs::create_dir_all(&root).unwrap();
        assert!(Command::new("git")
            .arg("init")
            .current_dir(&root)
            .status()
            .unwrap()
            .success());
        let _ = Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "https://github.com/Saurias92/Aspis-bio.git",
            ])
            .current_dir(&root)
            .status();
        fs::write(root.join("src.rs"), "fn main() {}\n").unwrap();

        let status = project_git_status(Some(&root.to_string_lossy()));

        let _ = fs::remove_dir_all(&root);

        assert!(status.is_git_repo);
        assert!(status.is_github);
        assert_eq!(
            status.github_url.as_deref(),
            Some("https://github.com/Saurias92/Aspis-bio")
        );
        assert_eq!(status.dirty_count, 1);
        assert_eq!(status.untracked_count, 1);
        assert_eq!(status.policy_status, "warning");
        assert!(status
            .required_actions
            .iter()
            .any(|action| action.contains("Commit")));
    }

    fn temp_project_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "aspis-projects-{name}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }


}
