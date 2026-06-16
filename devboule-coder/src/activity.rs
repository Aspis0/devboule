//! Orchestrator → Console FILE BRIDGE (the writer half).
//!
//! The orchestrator runs as a SEPARATE process with a ratatui TUI on its PTY, so it
//! CANNOT print activity markers to stdout — that would corrupt the TUI. Instead it
//! APPENDS one tiny JSON event per line to the path in `DEVBOULE_ACTIVITY_FILE`. The
//! Tauri host (`src-tauri/src/backend/mini_activity.rs` + the launch tail task in
//! `projects.rs`) tails that file, parses each line, and turns it into a coder-tier
//! `CoderEntry` milestone in the live Activity Console for this launch's `agent_id`.
//!
//! BEST-EFFORT BY DESIGN: this is pure observability. If `DEVBOULE_ACTIVITY_FILE` is
//! unset, or the file is unwritable, or a write fails for ANY reason, we SILENTLY
//! no-op. Emitting a milestone must NEVER break, slow, or fail the orchestrator run.
//! There is no buffering and no background thread: each `milestone` opens the file in
//! append mode, writes one line, and closes it (append is atomic for the small,
//! single-line writes we do on a local fs — the host tail reads only whole lines).
//!
//! PRIVACY: an event carries only a short, redacted LABEL (`text`) and a node style —
//! NEVER a raw transcript, file body, token, or secret. The planner / burst pick the
//! label; the host surfaces it verbatim. Keep every label a path basename + a verb.

use std::io::Write;
use std::path::PathBuf;

/// The env var the Tauri host sets at orchestrator launch to the per-agent activity
/// file path. Unset (the headless / standalone case) ⇒ every milestone no-ops.
const ENV_ACTIVITY_FILE: &str = "DEVBOULE_ACTIVITY_FILE";

/// Hard cap on a single milestone's `text` (chars, not bytes). The host also bounds
/// the line it reads back; capping here keeps the on-disk line small and the wire
/// payload tiny. A label longer than this is char-truncated (never split mid-codepoint).
const MAX_TEXT_CHARS: usize = 200;

/// The timeline node style for a milestone — mirrors the host's `NodeStyle` /
/// the frontend `ConsoleEntry["node"]` union (`"" | "dot" | "sage" | "terra"`).
/// `Hollow` serializes to the empty string. There is NO "coral" node in the wire
/// contract; the terracotta (`Terra`) ring is the warm/warning color, so a
/// rejection milestone uses `Terra` and lets its TEXT carry the "rejected" meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    /// `""` — hollow teal (a neutral step, e.g. one EXPLORE).
    Hollow,
    /// `"dot"` — filled teal (a completed planner phase).
    Dot,
    /// `"sage"` — sage ring (a positive terminal, e.g. plan approved).
    Sage,
    /// `"terra"` — terracotta ring (a submit/awaiting or a rejection — warm/warning).
    Terra,
}

impl Node {
    /// The exact wire string the host parses into its `NodeStyle` (and the frontend
    /// renders). MUST stay in lockstep with `mini_activity::NodeStyle`'s serde.
    fn as_wire(self) -> &'static str {
        match self {
            Node::Hollow => "",
            Node::Dot => "dot",
            Node::Sage => "sage",
            Node::Terra => "terra",
        }
    }
}

/// The orchestrator-side activity emitter. Cheap to clone (just an `Option<PathBuf>`),
/// `Send + Sync` so it threads through the async planner / burst without ceremony.
/// A `None` path (env unset or blank) makes every `milestone` a no-op.
#[derive(Debug, Clone, Default)]
pub struct Activity {
    /// The resolved activity-file path, or `None` to disable (the no-op case).
    path: Option<PathBuf>,
}

impl Activity {
    /// Build from the process env. Reads `DEVBOULE_ACTIVITY_FILE`; a missing or
    /// blank value yields a disabled (no-op) emitter. Does NOT touch the disk here
    /// (no create / probe) — the first `milestone` is what opens the file, so a host
    /// that sets the var but never created the file still works (the open creates it).
    pub fn from_env() -> Self {
        let path = std::env::var(ENV_ACTIVITY_FILE)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        Self { path }
    }

    /// An explicitly-disabled emitter (no path). Used by call sites / tests that
    /// never want to touch a file.
    pub fn disabled() -> Self {
        Self { path: None }
    }

    /// Build pointed at an explicit path (for tests + any non-env caller).
    #[cfg(test)]
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            path: Some(path.into()),
        }
    }

    /// Append ONE `milestone` event line for `text` with node style `node`.
    /// BEST-EFFORT: a `None` path or ANY I/O error silently no-ops (never panics,
    /// never returns an error to the caller — observability must not break the run).
    /// The line is a compact JSON object: `{"kind":"milestone","text":"…","node":"…"}`
    /// plus a trailing `\n` so the host tail reads it as one whole line.
    pub fn milestone(&self, text: &str, node: Node) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let line = encode_milestone(text, node);
        // Open in append mode (create if absent) and write the single line. We
        // deliberately DROP the handle each call: appends are independent and the
        // cost is negligible at planner-phase frequency (a handful per run). A
        // failure at any step is swallowed.
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            // One write_all of the whole line (incl. the newline) so a partial write
            // cannot leave a half-line the host would skip; still best-effort.
            let _ = file.write_all(line.as_bytes());
        }
    }
}

/// Serialize ONE milestone event to its single-line JSON form, `text` char-capped to
/// [`MAX_TEXT_CHARS`]. Factored out so it is directly unit-testable and so the wire
/// shape lives in one place. Always ends with a single `\n`.
fn encode_milestone(text: &str, node: Node) -> String {
    let capped: String = if text.chars().count() > MAX_TEXT_CHARS {
        text.chars().take(MAX_TEXT_CHARS).collect()
    } else {
        text.to_string()
    };
    // serde_json escapes control chars / quotes / newlines, guaranteeing the value
    // stays on ONE physical line even if a label contained a stray newline — the
    // host tail splits on '\n', so the payload must never embed a raw one.
    let value = serde_json::json!({
        "kind": "milestone",
        "text": capped,
        "node": node.as_wire(),
    });
    let mut line = value.to_string();
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn encode_milestone_is_one_well_formed_json_line() {
        let line = encode_milestone("Planning: 3 spine files", Node::Dot);
        // Exactly one trailing newline; no embedded newline.
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);

        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["kind"], "milestone");
        assert_eq!(v["text"], "Planning: 3 spine files");
        assert_eq!(v["node"], "dot");
    }

    #[test]
    fn node_wire_strings_match_the_contract() {
        assert_eq!(Node::Hollow.as_wire(), "");
        assert_eq!(Node::Dot.as_wire(), "dot");
        assert_eq!(Node::Sage.as_wire(), "sage");
        assert_eq!(Node::Terra.as_wire(), "terra");
    }

    #[test]
    fn text_is_char_capped_without_splitting_a_codepoint() {
        // A multi-byte label longer than the cap is truncated to MAX_TEXT_CHARS chars.
        let long = "é".repeat(MAX_TEXT_CHARS + 50);
        let line = encode_milestone(&long, Node::Hollow);
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        let got = v["text"].as_str().unwrap();
        assert_eq!(got.chars().count(), MAX_TEXT_CHARS, "capped to MAX_TEXT_CHARS chars");
        // Every char is intact 'é' (no replacement char from a split codepoint).
        assert!(got.chars().all(|c| c == 'é'));
    }

    #[test]
    fn embedded_newline_in_label_stays_on_one_line() {
        // A label with a stray newline must not break the one-event-per-line contract.
        let line = encode_milestone("explor\ning src/a.rs", Node::Hollow);
        assert_eq!(line.matches('\n').count(), 1, "the only newline is the terminator");
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["text"], "explor\ning src/a.rs");
    }

    #[test]
    fn disabled_emitter_never_touches_disk() {
        // A disabled (no-path) emitter is a silent no-op — nothing to assert beyond
        // "it does not panic and writes nothing" (there is no path to write to).
        let a = Activity::disabled();
        a.milestone("anything", Node::Dot);
        // from_env with the var unset is also disabled.
        std::env::remove_var(ENV_ACTIVITY_FILE);
        let b = Activity::from_env();
        b.milestone("anything", Node::Dot);
        // No file path exists on either; both are no-ops by construction.
        assert!(a.path.is_none());
        assert!(b.path.is_none());
    }

    #[test]
    fn milestone_appends_well_formed_jsonl_to_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("activity.jsonl");
        let a = Activity::with_path(&file);

        a.milestone("Planning: 2 spine files", Node::Dot);
        a.milestone("exploring src/a.rs", Node::Hollow);
        a.milestone("plan approved", Node::Sage);

        let body = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 3, "one line per milestone, appended in order");

        let l0: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(l0["kind"], "milestone");
        assert_eq!(l0["text"], "Planning: 2 spine files");
        assert_eq!(l0["node"], "dot");

        let l1: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(l1["text"], "exploring src/a.rs");
        assert_eq!(l1["node"], "");

        let l2: Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(l2["text"], "plan approved");
        assert_eq!(l2["node"], "sage");
    }

    #[test]
    fn milestone_to_unwritable_path_silently_no_ops() {
        // Point at a path whose PARENT does not exist → open fails → swallowed.
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("does-not-exist").join("activity.jsonl");
        let a = Activity::with_path(&bad);
        // Must not panic; nothing is created.
        a.milestone("should be dropped", Node::Dot);
        assert!(!bad.exists());
    }
}
