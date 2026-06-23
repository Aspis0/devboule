//! Steer INBOX (the reader half) — live messages to a RUNNING orchestrator.
//!
//! The orchestrator runs as a SEPARATE headless process; the user can message it
//! mid-plan from the app. The host APPENDS one message per line to the path in
//! `DEVBOULE_STEER_FILE`; the burst loop calls [`Steer::drain`] BETWEEN rounds and
//! injects each NEW line into the transcript as a human turn, so the model sees the
//! steer on its next call. Mirror of the `activity.rs` file-bridge, other direction.
//!
//! BEST-EFFORT BY DESIGN: an unset path, a missing file, or any I/O error yields no
//! messages (never panics, never errors to the caller). `drain` returns only WHOLE
//! new lines since the previous drain (it tracks a byte offset), so a half-written
//! line is never delivered torn.

use std::path::PathBuf;
use std::sync::Mutex;

/// The env var the host sets at orchestrator launch to the per-agent steer inbox path.
/// Unset (headless / standalone) ⇒ [`Steer::drain`] always returns empty.
const ENV_STEER_FILE: &str = "DEVBOULE_STEER_FILE";

/// Hard cap on bytes read in ONE `drain` (the rest is read on the next drain). Bounds
/// memory + the per-iteration message count so a flooded/huge inbox can't stall the
/// burst. Messages are command-capped to 2000 chars, so this window always holds many
/// WHOLE lines.
const MAX_STEER_READ: u64 = 64 * 1024;

/// Reads newly-appended lines from the steer inbox file. Interior-mutable (the
/// executor holds it behind a shared ref): `drain` advances a byte offset under a
/// `Mutex` so each whole line is delivered exactly once.
pub struct Steer {
    path: Option<PathBuf>,
    offset: Mutex<u64>,
}

impl Steer {
    /// A disabled inbox: `drain` always returns empty.
    pub fn disabled() -> Self {
        Self {
            path: None,
            offset: Mutex::new(0),
        }
    }

    /// Bind to an explicit inbox path (used by tests + the executor).
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            path: Some(path.into()),
            offset: Mutex::new(0),
        }
    }

    /// Resolve from `DEVBOULE_STEER_FILE`: a non-empty value ⇒ a bound inbox at that
    /// path; unset/empty ⇒ disabled.
    pub fn from_env() -> Self {
        match std::env::var(ENV_STEER_FILE) {
            Ok(p) if !p.trim().is_empty() => Self::with_path(p),
            _ => Self::disabled(),
        }
    }

    /// Return the WHOLE new lines appended since the previous call (advancing the
    /// offset past them), oldest-first. Empty when disabled, the file is absent, no
    /// new content, or on any I/O error. A trailing partial (no `\n` yet) line is NOT
    /// returned until its newline arrives.
    pub fn drain(&self) -> Vec<String> {
        let path = match &self.path {
            Some(p) => p,
            None => return Vec::new(),
        };

        let mut off = match self.offset.lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };

        let start = *off;
        let mut file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };

        let mut buf = Vec::new();
        if std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(start)).is_err() {
            return Vec::new();
        }
        // Bound the per-drain read (see MAX_STEER_READ): read at most a fixed window from
        // the offset; any tail beyond it is consumed on the next drain. Keeps the held
        // lock + the allocation small even if the inbox file is huge.
        let mut limited = std::io::Read::take(file, MAX_STEER_READ);
        if std::io::Read::read_to_end(&mut limited, &mut buf).is_err() {
            return Vec::new();
        }

        let last_nl = match buf.iter().rposition(|&b| b == b'\n') {
            Some(i) => i,
            None => return Vec::new(),
        };

        let consume_len = last_nl + 1;
        // Lossy decode (never fails) so stray non-UTF8 bytes degrade to U+FFFD rather
        // than dropping the whole window; advance the offset only AFTER decoding, so a
        // decode issue can never silently consume messages.
        let text = String::from_utf8_lossy(&buf[..consume_len]).into_owned();
        *off += consume_len as u64;
        text.split('\n')
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::Write;

    fn append(path: &std::path::Path, s: &str) {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(s.as_bytes()).unwrap();
    }

    #[test]
    fn disabled_inbox_drains_nothing() {
        assert!(Steer::disabled().drain().is_empty());
    }

    #[test]
    fn drains_only_new_whole_lines_each_call() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("steer.inbox");
        let steer = Steer::with_path(&path);

        // No file yet ⇒ empty, no panic.
        assert!(steer.drain().is_empty());

        append(&path, "make it idempotent\nuse axum\n");
        assert_eq!(
            steer.drain(),
            vec![
                "make it idempotent".to_string(),
                "use axum".to_string()
            ],
            "both complete lines delivered, oldest-first"
        );

        // No new content ⇒ empty (offset advanced).
        assert!(steer.drain().is_empty(), "already-consumed lines not re-delivered");

        // A partial line (no newline) is NOT delivered yet.
        append(&path, "partial without newline");
        assert!(steer.drain().is_empty(), "a half-written line is held back");

        // Once its newline arrives, the whole line is delivered.
        append(&path, "\nand more\n");
        assert_eq!(
            steer.drain(),
            vec![
                "partial without newline".to_string(),
                "and more".to_string()
            ],
        );
    }

    #[test]
    fn drain_caps_the_read_window_but_loses_no_messages() {
        // Write more than one read window of whole lines; one drain returns only what fits
        // in the window, subsequent drains return the rest — total preserved, none lost.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("steer.inbox");
        let steer = Steer::with_path(&path);

        let line = "x".repeat(1000); // ~1001 bytes per line incl '\n'
        let n = 200; // ~200 KB total, > a couple of 64 KiB windows
        for i in 0..n {
            append(&path, &format!("{line}{i}\n"));
        }

        let first = steer.drain();
        assert!(!first.is_empty(), "first drain returns something");
        assert!(first.len() < n, "first drain is bounded by the read window, not the whole file");

        let mut total = first.len();
        loop {
            let more = steer.drain();
            if more.is_empty() {
                break;
            }
            total += more.len();
        }
        assert_eq!(total, n, "every message is delivered exactly once across drains");
    }

    #[test]
    fn blank_lines_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("steer.inbox");
        let steer = Steer::with_path(&path);
        append(&path, "\n   \nreal message\n\n");
        assert_eq!(steer.drain(), vec!["real message".to_string()]);
    }
}
