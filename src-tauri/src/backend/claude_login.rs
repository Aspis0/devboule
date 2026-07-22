//! F51: in-app "Login with Claude" — run `claude setup-token` on a PTY, capture the
//! printed OAuth token, save via vault. Never returns or logs the token.

use crate::backend::projects::command_exists;
use crate::backend::vault;
use portable_pty::{CommandBuilder, PtySize};
use regex::Regex;
use serde::Serialize;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const READ_BUF: usize = 4096;
const TAIL_CHARS: usize = 300;

static LOGIN_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static LOGIN_KILLER: Mutex<Option<Box<dyn portable_pty::ChildKiller + Send + Sync>>> =
    Mutex::new(None);

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

fn token_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"sk-ant-oat[A-Za-z0-9\-_]{8,}").expect("token regex"))
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
    token_regex()
        .find(&clean)
        .map(|m| m.as_str().to_string())
}

/// Redact any token-shaped substrings from a log tail. Pure.
pub fn redact_token_patterns(s: &str) -> String {
    token_regex()
        .replace_all(s, "[redacted-token]")
        .into_owned()
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

fn clear_login_slot() {
    LOGIN_IN_PROGRESS.store(false, Ordering::Release);
    if let Ok(mut g) = LOGIN_KILLER.lock() {
        *g = None;
    }
}

fn store_killer(killer: Box<dyn portable_pty::ChildKiller + Send + Sync>) {
    if let Ok(mut g) = LOGIN_KILLER.lock() {
        *g = Some(killer);
    }
}

fn kill_stored_child() {
    if let Ok(mut g) = LOGIN_KILLER.lock() {
        if let Some(mut k) = g.take() {
            let _ = k.kill();
        }
    }
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
    kill_stored_child();
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

    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(run_claude_setup_token_inner));
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

    let mut reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            kill_stored_child();
            let _ = child.wait();
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

    // Watchdog: kill child after LOGIN_TIMEOUT so a blocking read unblocks.
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    let stop_watchdog = Arc::new(AtomicBool::new(false));
    let stop_w = Arc::clone(&stop_watchdog);
    let watchdog = std::thread::spawn(move || {
        while !stop_w.load(Ordering::Relaxed) {
            if Instant::now() >= deadline {
                kill_stored_child();
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });

    let mut accumulated = String::new();
    let mut buf = [0u8; READ_BUF];
    let outcome = loop {
        if Instant::now() >= deadline {
            kill_stored_child();
            let _ = child.wait();
            break ClaudeLoginResult {
                ok: false,
                reason: Some("timeout".into()),
                stderr_tail: Some(tail_redacted(&accumulated)),
            };
        }

        match reader.read(&mut buf) {
            Ok(0) => {
                // EOF — child closed PTY.
                let _ = child.wait();
                if let Some(token) = extract_setup_token(&accumulated) {
                    break try_save_token(&token);
                }
                break ClaudeLoginResult {
                    ok: false,
                    reason: Some("no token in output".into()),
                    stderr_tail: Some(tail_redacted(&accumulated)),
                };
            }
            Ok(n) => {
                accumulated.push_str(&String::from_utf8_lossy(&buf[..n]));
                if accumulated.len() > 65_536 {
                    accumulated = accumulated[accumulated.len() - 32_768..].to_string();
                }
                if let Some(token) = extract_setup_token(&accumulated) {
                    let saved = try_save_token(&token);
                    kill_stored_child();
                    let _ = child.wait();
                    break saved;
                }
                // Child may have exited without EOF yet.
                if let Ok(Some(_)) = child.try_wait() {
                    // Drain a bit more.
                    for _ in 0..10 {
                        match reader.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                accumulated.push_str(&String::from_utf8_lossy(&buf[..n]));
                            }
                        }
                    }
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
                kill_stored_child();
                let _ = child.wait();
                if Instant::now() >= deadline {
                    break ClaudeLoginResult {
                        ok: false,
                        reason: Some("timeout".into()),
                        stderr_tail: Some(tail_redacted(&accumulated)),
                    };
                }
                break ClaudeLoginResult {
                    ok: false,
                    reason: Some(format!("Error reading Claude login output: {e}")),
                    stderr_tail: Some(tail_redacted(&accumulated)),
                };
            }
        }
    };

    stop_watchdog.store(true, Ordering::Relaxed);
    let _ = watchdog.join();
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
}
