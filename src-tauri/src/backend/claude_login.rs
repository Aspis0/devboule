//! F51: in-app "Login with Claude" — run `claude setup-token` on a PTY, capture the
//! printed OAuth token, save via vault. Never returns or logs the token.
//!
//! Reap invariant: `claude setup-token` ignores soft kill (SIGTERM / ChildKiller);
//! never use a bare blocking `child.wait()` on the hot path — only bounded poll +
//! SIGKILL escalation, then a detached reaper as last resort.

use crate::backend::projects::command_exists;
use crate::backend::vault;
use portable_pty::{Child, CommandBuilder, PtySize};
use regex::Regex;
use serde::Serialize;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const KILL_GRACE: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const READ_BUF: usize = 4096;
const TAIL_CHARS: usize = 300;
const CODE_MAX_LEN: usize = 512;

static LOGIN_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static LOGIN_KILLER: Mutex<Option<Box<dyn portable_pty::ChildKiller + Send + Sync>>> =
    Mutex::new(None);
static LOGIN_WRITER: Mutex<Option<Box<dyn Write + Send>>> = Mutex::new(None);
static LOGIN_PHASE: Mutex<LoginPhase> = Mutex::new(LoginPhase::Idle);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginPhase {
    Idle,
    Running,
    AwaitingCode,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeLoginState {
    pub phase: LoginPhase,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeLoginResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Last ~300 chars of output with any token-like substrings redacted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr_tail: Option<String>,
}

/// Pure escalation stages after soft kill fails to reap (for unit tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillEscalation {
    /// Soft kill already tried; next is hard kill (SIGKILL / hard ChildKiller).
    HardKill,
    /// Hard kill already tried; next is detach a background reaper and return.
    DetachReaper,
}

/// Pure: after `already_hard` kill attempt still didn't reap, what next?
pub fn next_kill_escalation(already_hard: bool) -> KillEscalation {
    if already_hard {
        KillEscalation::DetachReaper
    } else {
        KillEscalation::HardKill
    }
}

/// STRICT — only OAuth setup-token shapes, used to EXTRACT the token to save.
///
/// Current `claude setup-token` (v2.1+) prints the OAuth access_token as `sk-ant-si-…`
/// (the old code only accepted `sk-ant-oat…`, so nothing was captured → login "did
/// nothing"). We require a KNOWN OAuth marker (`oat`|`si`) right after `sk-ant-` — NOT a
/// bare `sk-ant-` — so we never capture a STATIC API key (`sk-ant-api03-…`) that the CLI
/// may print in a startup warning ("ANTHROPIC_API_KEY is set…") BEFORE the real token:
/// `extract_setup_token` takes the FIRST match and the read loop saves+kills on it, so a
/// foreign `sk-ant-api…` in a preamble would be saved as the OAuth token and the login
/// would falsely report success (the Rust regex crate has no lookahead, hence an explicit
/// marker allowlist rather than a negative `(?!api)`). 12+ body chars + optional dotted
/// (JWT-like) segments. Add new OAuth markers here if a future CLI introduces them.
fn token_extract_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"sk-ant-(?:oat|si)[A-Za-z0-9_-]{12,}(?:\.[A-Za-z0-9_-]+)*")
            .expect("token extract regex")
    })
}

/// BROAD — any `sk-ant-*` credential shape (incl. static API keys), used ONLY to REDACT
/// tokens from log tails. Over-matching is strictly SAFER for redaction (redacts more).
fn token_redact_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"sk-ant-[A-Za-z0-9_-]{16,}(?:\.[A-Za-z0-9_-]+)*").expect("token redact regex")
    })
}

fn ansi_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[()][0-9A-Za-z]",
        )
        .expect("ansi regex")
    })
}

/// Strip common ANSI escape sequences. Pure + total.
pub fn strip_ansi(s: &str) -> String {
    ansi_regex().replace_all(s, "").into_owned()
}

/// Extract the first Claude setup-token from (possibly ANSI-laden) CLI output. Pure.
pub fn extract_setup_token(output: &str) -> Option<String> {
    let clean = strip_ansi(output);
    token_extract_regex()
        .find(&clean)
        .map(|m| m.as_str().to_string())
}

/// Redact any token-shaped substrings from a log tail. Pure.
pub fn redact_token_patterns(s: &str) -> String {
    token_redact_regex()
        .replace_all(s, "[redacted-token]")
        .into_owned()
}

fn paste_code_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)paste\s*code\s*here").expect("paste-code regex")
    })
}

/// Pure: true when CLI output is asking the user to paste a browser OAuth code.
pub fn detects_paste_code_prompt(output: &str) -> bool {
    let clean = strip_ansi(output);
    paste_code_regex().is_match(&clean)
}

/// Pure: validate a manual OAuth code before writing it to the PTY.
pub fn validate_login_code(code: &str) -> Result<String, String> {
    let trimmed = code.trim();
    if trimmed.is_empty() {
        return Err("Code is empty.".into());
    }
    if trimmed.len() > CODE_MAX_LEN {
        return Err(format!("Code is too long (max {CODE_MAX_LEN} characters)."));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("Code must not contain control characters.".into());
    }
    Ok(trimmed.to_string())
}

fn tail_redacted(buf: &str) -> String {
    let clean = strip_ansi(buf);
    let slice = if clean.chars().count() > TAIL_CHARS {
        clean
            .chars()
            .rev()
            .take(TAIL_CHARS)
            .collect::<String>()
            .chars()
            .rev()
            .collect()
    } else {
        clean
    };
    redact_token_patterns(&slice)
}

fn set_login_phase(phase: LoginPhase) {
    if let Ok(mut g) = LOGIN_PHASE.lock() {
        *g = phase;
    }
}

fn clear_login_slot() {
    LOGIN_IN_PROGRESS.store(false, Ordering::Release);
    set_login_phase(LoginPhase::Idle);
    if let Ok(mut g) = LOGIN_KILLER.lock() {
        *g = None;
    }
    if let Ok(mut g) = LOGIN_WRITER.lock() {
        *g = None;
    }
}

fn store_killer(killer: Box<dyn portable_pty::ChildKiller + Send + Sync>) {
    if let Ok(mut g) = LOGIN_KILLER.lock() {
        *g = Some(killer);
    }
}

fn store_writer(writer: Box<dyn Write + Send>) {
    if let Ok(mut g) = LOGIN_WRITER.lock() {
        *g = Some(writer);
    }
}

/// Soft-kill via the stored ChildKiller (if any). Does not wait.
fn kill_stored_child_soft() {
    if let Ok(mut g) = LOGIN_KILLER.lock() {
        if let Some(mut k) = g.take() {
            let _ = k.kill();
        }
    }
}

/// Cheap poll of the shared login phase (idle / running / awaiting_code).
pub fn login_state() -> ClaudeLoginState {
    let phase = LOGIN_PHASE
        .lock()
        .map(|g| *g)
        .unwrap_or(LoginPhase::Idle);
    ClaudeLoginState { phase }
}

/// Write a browser OAuth code into the live PTY (code + `\r`), then flush.
pub fn submit_login_code(code: String) -> Result<(), String> {
    let code = validate_login_code(&code)?;
    let mut guard = LOGIN_WRITER
        .lock()
        .map_err(|_| "Claude login writer lock is poisoned.".to_string())?;
    let writer = guard
        .as_mut()
        .ok_or_else(|| "No Claude login session is waiting for a code.".to_string())?;
    writer
        .write_all(format!("{code}\r").as_bytes())
        .map_err(|e| format!("Could not write code to Claude login: {e}"))?;
    writer
        .flush()
        .map_err(|e| format!("Could not flush code to Claude login: {e}"))?;
    Ok(())
}

/// Poll `try_wait` every 100ms until reaped or `grace` elapses. Never blocks on `wait()`.
fn poll_reaped(child: &mut Box<dyn Child + Send + Sync>, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => {
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(_) => {
                // Unknown status — treat as not reaped so caller can escalate.
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

/// Soft kill → poll → SIGKILL → poll → detach reaper. Never a bare blocking `wait()`
/// on the calling thread. Outcome of the login flow must never hang on reap.
fn kill_and_reap_bounded(mut child: Box<dyn Child + Send + Sync>, grace: Duration) {
    // Soft: ChildKiller + Child::kill (often SIGTERM — may be ignored by setup-token).
    kill_stored_child_soft();
    let _ = child.kill();

    if poll_reaped(&mut child, grace) {
        return;
    }

    // Hard kill (unix SIGKILL by pid; Windows ChildKiller kill is already hard).
    debug_assert_eq!(next_kill_escalation(false), KillEscalation::HardKill);
    if let Some(pid) = child.process_id() {
        #[cfg(unix)]
        {
            // SIGKILL cannot be ignored — the only signal that worked live on setup-token.
            unsafe {
                let _ = libc::kill(pid as i32, libc::SIGKILL);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = child.kill();
            let _ = pid;
        }
    } else {
        let _ = child.kill();
    }

    if poll_reaped(&mut child, grace) {
        return;
    }

    // Still alive: detach a reaper so we never block the command return path.
    debug_assert_eq!(next_kill_escalation(true), KillEscalation::DetachReaper);
    std::thread::spawn(move || {
        // Detached last-resort wait — only allowed off the request path.
        let _ = child.wait();
    });
}

fn try_save_token(token: &str) -> ClaudeLoginResult {
    match vault::save_claude_oauth_token(token) {
        Ok(status) if status.configured => ClaudeLoginResult {
            ok: true,
            reason: None,
            stderr_tail: None,
        },
        Ok(status) => ClaudeLoginResult {
            ok: false,
            reason: Some(
                status
                    .message
                    .unwrap_or_else(|| "Token was rejected by the vault.".into()),
            ),
            stderr_tail: None,
        },
        Err(e) => ClaudeLoginResult {
            ok: false,
            reason: Some(format!("Could not save token: {e}")),
            stderr_tail: None,
        },
    }
}

/// Cancel an in-flight `claude setup-token` child if the login guard is held.
pub fn cancel_claude_login() -> ClaudeLoginResult {
    if !LOGIN_IN_PROGRESS.load(Ordering::Acquire) {
        return ClaudeLoginResult {
            ok: false,
            reason: Some("No Claude login is in progress.".into()),
            stderr_tail: None,
        };
    }
    // Soft poke only — the start thread owns the Child and will escalate via
    // kill_and_reap_bounded when it observes EOF / next loop iteration.
    kill_stored_child_soft();
    ClaudeLoginResult {
        ok: true,
        reason: Some("cancel requested".into()),
        stderr_tail: None,
    }
}

/// Run `claude setup-token` on a PTY, capture the token, save to vault.
/// Blocks the calling thread (invoke via spawn_blocking from an async command).
pub fn run_claude_setup_token() -> ClaudeLoginResult {
    if !command_exists("claude") {
        return ClaudeLoginResult {
            ok: false,
            reason: Some(
                "The `claude` CLI is not installed or not on PATH. Install Claude Code, then retry."
                    .into(),
            ),
            stderr_tail: None,
        };
    }
    if LOGIN_IN_PROGRESS
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return ClaudeLoginResult {
            ok: false,
            reason: Some("already in progress".into()),
            stderr_tail: None,
        };
    }
    set_login_phase(LoginPhase::Running);

    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(run_claude_setup_token_inner));
    // Reset phase + killer + writer on EVERY exit (success/failure/panic).
    clear_login_slot();
    match result {
        Ok(r) => r,
        Err(_) => ClaudeLoginResult {
            ok: false,
            reason: Some("Claude login task panicked.".into()),
            stderr_tail: None,
        },
    }
}

/// Watchdog that soft-kills on deadline; always stopped+joined via Drop.
struct LoginWatchdog {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl LoginWatchdog {
    fn start(deadline: Instant) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_w = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stop_w.load(Ordering::Relaxed) {
                if Instant::now() >= deadline {
                    kill_stored_child_soft();
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for LoginWatchdog {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run_claude_setup_token_inner() -> ClaudeLoginResult {
    // Owner's normal env — do NOT set CLAUDE_CONFIG_DIR (mirrors terminal run).
    let mut cmd = CommandBuilder::new("claude");
    cmd.arg("setup-token");

    let pty_system = portable_pty::native_pty_system();
    let pair = match pty_system.openpty(PtySize {
        rows: 24,
        cols: 100,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(e) => {
            return ClaudeLoginResult {
                ok: false,
                reason: Some(format!("Could not open a terminal for Claude login: {e}")),
                stderr_tail: None,
            };
        }
    };

    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => {
            return ClaudeLoginResult {
                ok: false,
                reason: Some(format!("Could not start `claude setup-token`: {e}")),
                stderr_tail: None,
            };
        }
    };

    store_killer(child.clone_killer());

    // Writer for interactive "Paste code here" submission (shared with submit_login_code).
    match pair.master.take_writer() {
        Ok(w) => store_writer(w),
        Err(e) => {
            kill_and_reap_bounded(child, KILL_GRACE);
            return ClaudeLoginResult {
                ok: false,
                reason: Some(format!("Could not attach Claude login writer: {e}")),
                stderr_tail: None,
            };
        }
    }

    let mut reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            kill_and_reap_bounded(child, KILL_GRACE);
            return ClaudeLoginResult {
                ok: false,
                reason: Some(format!("Could not read Claude login output: {e}")),
                stderr_tail: None,
            };
        }
    };

    // Drop slave so we get EOF when child exits. Keep master for the PTY lifetime.
    drop(pair.slave);
    let _master = pair.master;

    let deadline = Instant::now() + LOGIN_TIMEOUT;
    let watchdog = LoginWatchdog::start(deadline);

    let mut accumulated = String::new();
    let mut buf = [0u8; READ_BUF];

    let outcome = loop {
        if Instant::now() >= deadline {
            kill_and_reap_bounded(child, KILL_GRACE);
            break ClaudeLoginResult {
                ok: false,
                reason: Some("timeout".into()),
                stderr_tail: Some(tail_redacted(&accumulated)),
            };
        }

        match reader.read(&mut buf) {
            Ok(0) => {
                // EOF — PTY closed. Child may still ignore soft kill; reap bounded.
                // Prefer save-if-token first so hang on kill never loses a good login.
                let result = if let Some(token) = extract_setup_token(&accumulated) {
                    try_save_token(&token)
                } else {
                    ClaudeLoginResult {
                        ok: false,
                        reason: Some("no token in output".into()),
                        stderr_tail: Some(tail_redacted(&accumulated)),
                    }
                };
                kill_and_reap_bounded(child, KILL_GRACE);
                break result;
            }
            Ok(n) => {
                accumulated.push_str(&String::from_utf8_lossy(&buf[..n]));
                if accumulated.len() > 65_536 {
                    accumulated = accumulated[accumulated.len() - 32_768..].to_string();
                }
                if let Some(token) = extract_setup_token(&accumulated) {
                    // Success path: save FIRST, then bounded kill — return save result even
                    // if kill escalates / detaches.
                    let saved = try_save_token(&token);
                    kill_and_reap_bounded(child, KILL_GRACE);
                    break saved;
                }
                // Manual-code OAuth variant: prompt for browser code on stdin.
                if detects_paste_code_prompt(&accumulated) {
                    set_login_phase(LoginPhase::AwaitingCode);
                }
                // Child may have exited without us noticing yet.
                if let Ok(Some(_)) = child.try_wait() {
                    // Already reaped via try_wait — drain a bit more from the PTY.
                    for _ in 0..10 {
                        match reader.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                accumulated.push_str(&String::from_utf8_lossy(&buf[..n]));
                            }
                        }
                    }
                    // Drop reaped child (no wait).
                    drop(child);
                    if let Some(token) = extract_setup_token(&accumulated) {
                        break try_save_token(&token);
                    }
                    break ClaudeLoginResult {
                        ok: false,
                        reason: Some("no token in output".into()),
                        stderr_tail: Some(tail_redacted(&accumulated)),
                    };
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let timed_out = Instant::now() >= deadline;
                kill_and_reap_bounded(child, KILL_GRACE);
                break if timed_out {
                    ClaudeLoginResult {
                        ok: false,
                        reason: Some("timeout".into()),
                        stderr_tail: Some(tail_redacted(&accumulated)),
                    }
                } else {
                    ClaudeLoginResult {
                        ok: false,
                        reason: Some(format!("Error reading Claude login output: {e}")),
                        stderr_tail: Some(tail_redacted(&accumulated)),
                    }
                };
            }
        }
    };

    // Explicit stop so join is done before return (Drop is backup).
    watchdog.stop();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_token_from_plain_output() {
        let out = "You're all set up!\n\nsk-ant-oat01AbCdEfGhIjKlMnOp\n\nDone.\n";
        assert_eq!(
            extract_setup_token(out).as_deref(),
            Some("sk-ant-oat01AbCdEfGhIjKlMnOp")
        );
    }

    #[test]
    fn extract_token_strips_ansi() {
        let out =
            "\x1b[32mHere's your token:\x1b[0m \x1b[1msk-ant-oat01XxYyZz_12345678\x1b[0m\n";
        assert_eq!(
            extract_setup_token(out).as_deref(),
            Some("sk-ant-oat01XxYyZz_12345678")
        );
    }

    #[test]
    fn extract_token_missing() {
        assert!(extract_setup_token("no token here, just words").is_none());
        assert!(extract_setup_token("sk-ant-oatSHORT").is_none());
        // A bare prefix or too-short body must NOT be captured.
        assert!(extract_setup_token("sk-ant-").is_none());
        assert!(extract_setup_token("sk-ant-si-abc").is_none());
    }

    #[test]
    fn extract_token_new_setup_token_format_sk_ant_si() {
        // Regression for the stale `-oat`-only regex: current `claude setup-token` prints
        // an OAuth access_token as `sk-ant-si-…`, which the old regex dropped entirely.
        let out = "Long-lived authentication token created successfully\n\n\
                   Your OAuth token (valid for 1 year):\n\
                   sk-ant-si-01AbCdEfGhIjKlMnOpQrStUvWx_1234567890\n";
        assert_eq!(
            extract_setup_token(out).as_deref(),
            Some("sk-ant-si-01AbCdEfGhIjKlMnOpQrStUvWx_1234567890")
        );
    }

    #[test]
    fn extract_token_jwt_like_dotted_segments() {
        // Some token values carry `.`-separated segments (JWT-like); capture the whole run.
        let tok = "sk-ant-si-01AbCdEfGhIjKlMnOpQr.eyJhbGciOiJIUzI1NiJ9.SflKxwRJSMeKKF2QT4";
        let out = format!("Here is your token:\n{tok}\ndone");
        assert_eq!(extract_setup_token(&out).as_deref(), Some(tok));
    }

    #[test]
    fn extract_token_si_strips_ansi() {
        let out = "\x1b[1msk-ant-si-01ZzYyXxWwVvUuTtSsRrQq_0987654321\x1b[0m\n";
        assert_eq!(
            extract_setup_token(out).as_deref(),
            Some("sk-ant-si-01ZzYyXxWwVvUuTtSsRrQq_0987654321")
        );
    }

    #[test]
    fn redact_covers_new_token_format() {
        let s = "log tail sk-ant-si-01AbCdEfGhIjKlMnOpQrStUv_XYZ end";
        let red = redact_token_patterns(s);
        assert!(!red.contains("sk-ant-si-01AbCdEfGhIjKlMnOpQrStUv_XYZ"), "{red}");
        assert!(red.contains("[redacted-token]"), "{red}");
    }

    #[test]
    fn extract_ignores_api_key_preamble_and_takes_real_oauth_token() {
        // SECURITY (audit BLOCKER): the CLI may print a startup warning that echoes a static
        // ANTHROPIC_API_KEY (`sk-ant-api03-…`) BEFORE the real OAuth token. extract takes the
        // FIRST match + saves/kills on it, so a bare `sk-ant-` regex would save the WRONG
        // credential. The strict extract regex must SKIP the api key and capture the si token.
        let out = "Warning: ANTHROPIC_API_KEY is set: sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUV\n\n\
                   Your OAuth token (valid for 1 year):\n\
                   sk-ant-si-01RealTokenHereXYZ1234567890\n";
        assert_eq!(
            extract_setup_token(out).as_deref(),
            Some("sk-ant-si-01RealTokenHereXYZ1234567890"),
            "must skip the sk-ant-api03 key and capture the sk-ant-si OAuth token"
        );
    }

    #[test]
    fn extract_never_captures_a_static_api_key() {
        // An api key alone (no OAuth token) must NOT be captured as a setup-token.
        let out = "ANTHROPIC_API_KEY=sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ012345";
        assert!(
            extract_setup_token(out).is_none(),
            "sk-ant-api03 (static API key) must never be saved as the Claude OAuth token"
        );
    }

    #[test]
    fn redact_still_covers_api_key_shape() {
        // Redaction stays BROAD (safer): an api key in a log tail must still be redacted,
        // even though extract deliberately ignores it.
        let s = "leak sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ012345 tail";
        let red = redact_token_patterns(s);
        assert!(!red.contains("sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ012345"), "{red}");
        assert!(red.contains("[redacted-token]"), "{red}");
    }

    #[test]
    fn redact_replaces_token_in_tail() {
        let s = "prefix sk-ant-oat01SECRETTOKEN99 suffix";
        let r = redact_token_patterns(s);
        assert!(!r.contains("SECRETTOKEN"));
        assert!(r.contains("[redacted-token]"));
    }

    #[test]
    fn strip_ansi_removes_csi() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
    }

    #[test]
    fn tail_redacted_does_not_leak_token() {
        let mut big = "x".repeat(500);
        big.push_str("sk-ant-oat01LEAKEDTOKENVALUE99");
        let t = tail_redacted(&big);
        assert!(!t.contains("LEAKEDTOKEN"));
        assert!(t.contains("[redacted-token]") || !t.contains("sk-ant-oat"));
    }

    #[test]
    fn kill_escalation_soft_then_hard_then_detach() {
        assert_eq!(next_kill_escalation(false), KillEscalation::HardKill);
        assert_eq!(next_kill_escalation(true), KillEscalation::DetachReaper);
    }

    #[test]
    fn detects_paste_code_prompt_plain_and_ansi() {
        assert!(detects_paste_code_prompt(
            "Open the URL…\nPaste code here if prompted > "
        ));
        assert!(detects_paste_code_prompt(
            "\x1b[1mPaste code here\x1b[0m if prompted >"
        ));
        assert!(detects_paste_code_prompt("PASTE   CODE   HERE:"));
        assert!(!detects_paste_code_prompt("You're all set up — close this window"));
        assert!(!detects_paste_code_prompt("sk-ant-oat01AbCdEfGhIjKlMnOp"));
    }

    #[test]
    fn validate_login_code_accepts_and_rejects() {
        assert_eq!(validate_login_code("  abcd-1234  ").unwrap(), "abcd-1234");
        assert!(validate_login_code("").is_err());
        assert!(validate_login_code("   ").is_err());
        assert!(validate_login_code("has\nnewline").is_err());
        assert!(validate_login_code(&"x".repeat(CODE_MAX_LEN + 1)).is_err());
    }
}
