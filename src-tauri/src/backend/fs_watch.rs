//! Shared, PURE filesystem-watch primitives reused by BOTH the Polis live-map
//! watcher (`polis/watcher.rs`) and the Censor code-review watcher
//! (`backend/censor/watch.rs`).
//!
//! Two pieces live here, extracted (BLOCKER 8 of the Censor plan) so the two
//! watchers cannot drift on the parts they genuinely share:
//!
//!   1. [`DebounceState`] — the tiny pure burst-coalescing state machine (no IO,
//!      no real clock; the caller supplies `now`). Both watchers debounce an
//!      editor's multi-event save into a single settled pass; Censor additionally
//!      runs TWO of these (a FINE per-file window and a COARSE crate/project
//!      window).
//!   2. [`is_excluded_path`] — the GENERIC "skip this path" screen: any path
//!      component equal to an excluded dir, OR a file whose name matches an
//!      ignore set, is excluded. This is the shared half of every watcher's
//!      relevance predicate; the LANGUAGE/extension keep-decision stays
//!      caller-specific (Polis delegates to the scanner's keep filter; Censor
//!      uses [`crate::backend::censor::detect::FileLang`]).
//!
//! The watcher THREAD + `notify` wiring + the signal-then-detached-reaper
//! teardown are intentionally NOT shared: each subsystem owns a distinct event
//! payload, ignore set, and dispatch core, and a single shared watcher serving
//! two subscribers would couple their lifecycles (see the plan's risk #1 — two
//! independent watchers on the same root is acceptable and verified). Only the
//! pure pieces are deduped here.

use std::path::Path;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Generic path-exclusion screen (shared half of every relevance predicate).
// ---------------------------------------------------------------------------

/// `true` if a change at `path` (under `root`) should be IGNORED purely on the
/// basis of its location/name — i.e. it lives inside an excluded directory, or
/// its file name matches one of `ignore_names`.
///
/// This is the location/name half ONLY: the caller still applies its own
/// extension/keep decision to a path that survives this screen. Comparisons:
///   - directory components are matched case-SENSITIVELY against `excluded_dirs`
///     (dir names like `node_modules`/`target`/`.git` are case-stable on the
///     platforms we target, and the Polis filter has always matched exactly);
///   - the file name is matched case-INSENSITIVELY against `ignore_names`
///     (so `.aspis-meta.json`/`.ASPIS-META.JSON` both self-guard).
///
/// A DELETED file no longer exists on disk, so the screen judges purely by the
/// path string (never by `stat`) — deleting a real tracked file is correctly
/// NOT excluded by this function.
pub fn is_excluded_path(
    path: &Path,
    root: &Path,
    excluded_dirs: &[&str],
    ignore_names: &[&str],
) -> bool {
    // Judge by the path relative to root when possible; otherwise fall back to
    // the raw components (a notify event path is normally absolute under root,
    // but be defensive).
    let rel = path.strip_prefix(root).unwrap_or(path);

    // Excluded-dir screen on the relative components.
    for comp in rel.components() {
        if let std::path::Component::Normal(os) = comp {
            let seg = os.to_string_lossy();
            if excluded_dirs.contains(&seg.as_ref()) {
                return true;
            }
        }
    }

    // Ignore-name screen on the file name (case-insensitive).
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if ignore_names.iter().any(|ig| name.eq_ignore_ascii_case(ig)) {
            return true;
        }
    }

    false
}

// ---------------------------------------------------------------------------
// Pure debounce/coalescing state (unit-testable without real timers).
// ---------------------------------------------------------------------------

/// Tracks whether a settled pass is pending and when the last relevant event
/// arrived, so the run loop can decide when a burst has gone QUIET long enough
/// to dispatch. Deliberately a tiny pure state machine (no IO, no real clock —
/// the caller supplies the `now`/instant) so the coalescing logic is unit-tested
/// and shared by both watchers.
///
/// Generic over the instant type `I` so tests can drive it with a fake
/// monotonic clock; production uses `std::time::Instant`.
#[derive(Debug, Clone)]
pub struct DebounceState<I = std::time::Instant> {
    /// Instant of the most recent relevant event, if a pass is pending.
    last_event: Option<I>,
    /// Debounce window.
    window: Duration,
}

impl<I: Copy + std::ops::Sub<I, Output = Duration>> DebounceState<I> {
    pub fn new(window: Duration) -> Self {
        Self {
            last_event: None,
            window,
        }
    }

    /// Record a relevant event at `now`. Marks a pass pending and (re)starts the
    /// quiet timer. Multiple events in a burst just push the deadline forward —
    /// the burst coalesces to a single eventual dispatch.
    pub fn record(&mut self, now: I) {
        self.last_event = Some(now);
    }

    /// `true` if a pass is currently pending (at least one event since the last
    /// `take`/reset). Part of the debounce API; exercised by the watcher tests
    /// (the production loops drive the window via `take_if_quiet`/`time_until_quiet`).
    #[allow(dead_code)]
    pub fn pending(&self) -> bool {
        self.last_event.is_some()
    }

    /// If a pass is pending AND `now` is at least `window` past the last event
    /// (the burst has gone quiet), clear the pending flag and return `true` — the
    /// caller should dispatch exactly once. Otherwise return `false`.
    pub fn take_if_quiet(&mut self, now: I) -> bool {
        match self.last_event {
            Some(last) if now - last >= self.window => {
                self.last_event = None;
                true
            }
            _ => false,
        }
    }

    /// Remaining time until the pending burst is considered quiet, or `None` if
    /// nothing is pending. Used to size the receive timeout so the loop wakes
    /// right when the debounce elapses (no busy-poll).
    pub fn time_until_quiet(&self, now: I) -> Option<Duration> {
        self.last_event.map(|last| {
            let elapsed = now - last;
            if elapsed >= self.window {
                Duration::ZERO
            } else {
                self.window - elapsed
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from(if cfg!(windows) { r"C:\proj" } else { "/proj" })
    }

    fn under(root: &Path, rel: &str) -> PathBuf {
        let mut p = root.to_path_buf();
        for seg in rel.split('/') {
            p.push(seg);
        }
        p
    }

    // ---- is_excluded_path ----

    #[test]
    fn excludes_dirs_and_ignore_names() {
        let r = root();
        let excluded = ["node_modules", "target", ".git", ".aspis-censor"];
        let ignore = [".aspis-meta.json"];

        // Excluded dir anywhere in the path → excluded.
        assert!(is_excluded_path(
            &under(&r, "node_modules/react/index.ts"),
            &r,
            &excluded,
            &ignore
        ));
        assert!(is_excluded_path(
            &under(&r, "src-tauri/target/debug/foo.rs"),
            &r,
            &excluded,
            &ignore
        ));
        assert!(is_excluded_path(
            &under(&r, ".git/HEAD"),
            &r,
            &excluded,
            &ignore
        ));
        // The censor shard dir itself (self-trigger guard).
        assert!(is_excluded_path(
            &under(&r, ".aspis-censor/abc.json"),
            &r,
            &excluded,
            &ignore
        ));

        // Ignore-name screen (case-insensitive).
        assert!(is_excluded_path(
            &under(&r, ".aspis-meta.json"),
            &r,
            &excluded,
            &ignore
        ));
        assert!(is_excluded_path(
            &under(&r, ".ASPIS-META.JSON"),
            &r,
            &excluded,
            &ignore
        ));

        // A normal source file under root survives the screen.
        assert!(!is_excluded_path(
            &under(&r, "src/main.rs"),
            &r,
            &excluded,
            &ignore
        ));
        assert!(!is_excluded_path(
            &under(&r, "src/app.ts"),
            &r,
            &excluded,
            &ignore
        ));
    }

    #[test]
    fn deletion_of_real_file_is_not_excluded_by_path() {
        // A deleted file is gone from disk; the screen judges by path string only,
        // so a deleted real source file is NOT excluded (drives a re-review).
        let r = root();
        assert!(!is_excluded_path(
            &under(&r, "src/deleted/old.rs"),
            &r,
            &["target"],
            &[]
        ));
    }

    // ---- DebounceState ----

    /// A fake monotonic clock so the debounce logic is tested without sleeping.
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct FakeInstant(u64);
    impl std::ops::Sub for FakeInstant {
        type Output = Duration;
        fn sub(self, rhs: Self) -> Duration {
            Duration::from_millis(self.0.saturating_sub(rhs.0))
        }
    }

    #[test]
    fn debounce_coalesces_a_burst_into_one_pass() {
        let mut d = DebounceState::<FakeInstant>::new(Duration::from_millis(400));
        assert!(!d.pending());

        d.record(FakeInstant(0));
        d.record(FakeInstant(100));
        d.record(FakeInstant(200));
        assert!(d.pending());

        // Still inside the window after the last event: not quiet yet.
        assert!(!d.take_if_quiet(FakeInstant(500))); // 500-200=300 < 400
        assert!(d.pending());

        // Window elapsed since the LAST event: dispatch once.
        assert!(d.take_if_quiet(FakeInstant(600)));
        assert!(!d.pending());

        // A second quiet check does nothing (already taken): no double dispatch.
        assert!(!d.take_if_quiet(FakeInstant(2000)));
    }

    #[test]
    fn debounce_time_until_quiet_shrinks_then_zeroes() {
        let mut d = DebounceState::<FakeInstant>::new(Duration::from_millis(400));
        assert_eq!(d.time_until_quiet(FakeInstant(0)), None);

        d.record(FakeInstant(0));
        assert_eq!(
            d.time_until_quiet(FakeInstant(100)),
            Some(Duration::from_millis(300))
        );
        assert_eq!(d.time_until_quiet(FakeInstant(400)), Some(Duration::ZERO));
        // Past the window clamps to zero (never negative).
        assert_eq!(d.time_until_quiet(FakeInstant(9999)), Some(Duration::ZERO));
    }
}
