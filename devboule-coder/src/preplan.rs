//! `.devboule/preplan.md` — the LOCAL planner's on-disk EXTERNAL MEMORY (harness-owned).
//!
//! A small local model has no memory of its own beyond the current call's prompt: if the
//! planner process dies mid-run (say, after STRUCTURE + a few EXPLORE calls but before a
//! PLAN attempt lands), every finding it gathered is gone, and a restarted run re-explores
//! the same files from scratch. This module gives the planner a durable, human-readable
//! scratchpad it re-reads on every run:
//!
//! - [`Preplan::load_or_init`] resumes the existing file when it belongs to the SAME goal
//!   (mirrors `main.rs`'s `session_persist` goal-prefix staleness rule: a persisted memory
//!   that does not match the CURRENT goal is a stale leftover from a different run and must
//!   never leak in), or starts a fresh scaffold otherwise.
//! - [`Preplan::append`] adds one line to a named section, idempotently (the exact line is
//!   never duplicated) — the planner calls this after each accepted EXPLORE note, after
//!   STRUCTURE, and after each rejected PLAN attempt.
//! - [`Preplan::render_for_prompt`] returns the whole file, TAIL-preserving-truncated to a
//!   char budget, with the `## Goal` section always kept intact — recent findings matter
//!   most to a model that forgot, but it must never lose WHAT it is planning.
//! - [`Preplan::clear`] removes the file once the plan reaches a TERMINAL human verdict
//!   (approved or rejected) — a live planning session's crumbs must not survive it.
//!
//! Reuse, not reinvention: [`crate::planner::truncate_chars`] (this module's own
//! [`tail_chars_with_marker`] is its mirror-image, keeping the END instead of the head) and
//! the atomic tmp+rename write pattern from `src-tauri`'s `write_agent_live_state`
//! (replicated here, not imported — see [`atomic_write`] — so this module stays a
//! self-contained, small unit per the house rule of one capability per module). Unlike
//! `session_persist.rs`'s own fixed-name `save`, this module's tmp name is PER-CALL unique
//! ([`tmp_path_for`]): a project root's preplan can be written by two orchestrator
//! processes concurrently, and a fixed sibling tmp name would let one clobber the other's
//! in-flight write.

use std::path::{Path, PathBuf};

use crate::planner::truncate_chars;

/// The section headers, IN ORDER, every `preplan.md` scaffold carries. A section name
/// passed to [`Preplan::append`] that is not one of these is silently ignored (best-effort:
/// this is an internal harness aid, never something a caller's typo should panic on).
const SECTIONS: [&str; 6] = [
    "Goal",
    "Constraints",
    "Findings",
    "Decisions",
    "Open questions",
    "Draft outline",
];

/// The planner's on-disk external memory, rooted at `<root>/.devboule/preplan.md`.
pub struct Preplan {
    path: PathBuf,
    /// The (trimmed) goal THIS instance was loaded for — re-checked against the
    /// on-disk `## Goal` header by [`Preplan::append`] and [`Preplan::clear`]
    /// before every mutation, so a stale writer (another session already
    /// re-scaffolded the file for a DIFFERENT goal) can never corrupt it.
    goal: String,
}

impl Preplan {
    /// Load the preplan at `<root>/.devboule/preplan.md`, resuming it ONLY when its `## Goal`
    /// section matches `goal` (trimmed, exact match) — otherwise (no file, unreadable file, or
    /// a DIFFERENT goal) a fresh scaffold is written. This is the crash-resume contract: a
    /// planner that dies mid-run leaves its findings for the NEXT run with the SAME goal to
    /// re-read; a new goal must never inherit a stale prior run's memory.
    pub fn load_or_init(root: &Path, goal: &str) -> Self {
        let path = root.join(".devboule").join("preplan.md");
        let goal = goal.trim().to_string();
        let resumable = std::fs::read_to_string(&path)
            .ok()
            .map(|body| extract_goal_section(&body) == goal)
            .unwrap_or(false);
        if !resumable {
            let _ = atomic_write(&path, &render_scaffold(&goal));
        }
        Self { path, goal }
    }

    /// Append `line` (trimmed) to the named section (one of [`SECTIONS`], sans the `##`
    /// prefix, e.g. `"Findings"`), IDEMPOTENTLY: if the exact (trimmed) line already appears
    /// under that section, this is a no-op. Best-effort: an unreadable file, a missing
    /// section, or a write failure silently no-ops — this is observability-adjacent scratch
    /// state, never something a caller's disk hiccup should turn into a hard error.
    pub fn append(&self, section: &str, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        let Ok(body) = std::fs::read_to_string(&self.path) else {
            return;
        };
        if extract_goal_section(&body) != self.goal {
            // Another session re-scaffolded this file for a DIFFERENT goal since we
            // loaded it (this instance is now stale): never write this session's
            // findings under a foreign `## Goal` header.
            return;
        }
        let lines: Vec<&str> = body.lines().collect();
        let header = format!("## {section}");
        let Some(header_idx) = lines.iter().position(|l| *l == header) else {
            return; // unknown section name: best-effort no-op, never a panic.
        };
        let content_start = header_idx + 1;
        let content_end = next_header_after(&lines, content_start);

        // Idempotent: the exact (trimmed) line already present under this section.
        if lines[content_start..content_end]
            .iter()
            .any(|l| l.trim() == trimmed)
        {
            return;
        }

        let mut new_lines: Vec<&str> = Vec::with_capacity(lines.len() + 1);
        new_lines.extend_from_slice(&lines[..content_end]);
        new_lines.push(trimmed);
        new_lines.extend_from_slice(&lines[content_end..]);
        // `str::lines` drops the trailing newline; restore it so the file always ends
        // in one, matching the scaffold's own convention.
        let mut out = new_lines.join("\n");
        out.push('\n');

        let _ = atomic_write(&self.path, &out);
    }

    /// Render the whole preplan for the PLAN prompt, hard-bounded to `max_chars`. When the
    /// file already fits, it is returned verbatim. Otherwise: the `## Goal` section is ALWAYS
    /// kept (truncated as a last resort if it alone exceeds the budget — a pathological goal
    /// must never blow the ceiling), and the remaining budget is filled with the TAIL of
    /// everything after it (the MOST RECENT findings/decisions — what the model most needs to
    /// re-read after forgetting), prefixed by a marker when earlier content was dropped.
    pub fn render_for_prompt(&self, max_chars: usize) -> String {
        let body = std::fs::read_to_string(&self.path).unwrap_or_default();
        if body.chars().count() <= max_chars {
            return body;
        }
        let lines: Vec<&str> = body.lines().collect();
        let goal_end = next_header_after(&lines, 1);
        let goal_part = lines[..goal_end].join("\n");
        let goal_capped = truncate_chars(&goal_part, max_chars);
        let goal_chars = goal_capped.chars().count();
        // +1 reserves the joining newline between the (capped) Goal and the tail.
        let remaining = max_chars.saturating_sub(goal_chars + 1);

        let rest = lines[goal_end..].join("\n");
        if remaining == 0 {
            return goal_capped;
        }
        let rest_tail = tail_chars_with_marker(&rest, remaining);
        if rest_tail.is_empty() {
            goal_capped
        } else {
            format!("{goal_capped}\n{rest_tail}")
        }
    }

    /// Remove the preplan file — called on a TERMINAL human verdict (approved or rejected):
    /// the planning session that produced it is over, and its scratch memory must not survive
    /// to contaminate a LATER, unrelated run. Best-effort: a missing file is not an error.
    ///
    /// Re-validates the on-disk `## Goal` header against this instance's own goal first: if
    /// another session already re-scaffolded the file for a DIFFERENT goal (this instance is
    /// stale), clearing is a no-op — a stale session reaching its terminal verdict must never
    /// delete a LATER, unrelated session's live preplan.
    pub fn clear(&self) {
        let Ok(body) = std::fs::read_to_string(&self.path) else {
            return; // already gone: nothing to do.
        };
        if extract_goal_section(&body) != self.goal {
            return;
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Index (into `lines`) of the next line starting with `"## "` at or after `start`, or
/// `lines.len()` if none — the END boundary of whatever section begins at `start`.
fn next_header_after(lines: &[&str], start: usize) -> usize {
    if start >= lines.len() {
        return lines.len();
    }
    lines[start..]
        .iter()
        .position(|l| l.starts_with("## "))
        .map(|i| start + i)
        .unwrap_or(lines.len())
}

/// Extract the `## Goal` section's body (trimmed), or `""` if the file does not start with a
/// `## Goal` header — a corrupted/foreign file is treated as non-resumable (never a panic).
fn extract_goal_section(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    if lines.first() != Some(&"## Goal") {
        return String::new();
    }
    let end = next_header_after(&lines, 1);
    lines[1..end].join("\n").trim().to_string()
}

/// The fresh scaffold for a new (or re-initialized) preplan: every section in
/// [`SECTIONS`] order, `## Goal` populated with `goal`, everything else empty.
fn render_scaffold(goal: &str) -> String {
    let mut out = String::new();
    for section in SECTIONS {
        out.push_str("## ");
        out.push_str(section);
        out.push('\n');
        if section == "Goal" {
            out.push_str(goal);
            out.push('\n');
        }
        out.push('\n');
    }
    // Drop the final blank line the loop above always appends, keeping a single
    // trailing newline (matches `append`'s own convention).
    out.pop();
    out
}

/// The tail of `s` that fits within `cap` chars, prefixed by a truncation marker when
/// content was dropped. The mirror-image of [`crate::planner::truncate_chars`]: that helper
/// keeps the HEAD (bounding a prompt whose trailing schema text must survive); this one keeps
/// the END, because in a findings log the MOST RECENT lines are what a model that forgot most
/// needs back.
fn tail_chars_with_marker(s: &str, cap: usize) -> String {
    let total = s.chars().count();
    if total <= cap {
        return s.to_string();
    }
    const MARKER: &str = "[…earlier notes truncated]\n";
    let marker_len = MARKER.chars().count();
    if cap <= marker_len {
        return MARKER.chars().take(cap).collect();
    }
    let keep = cap - marker_len;
    let skip = total - keep;
    let tail: String = s.chars().skip(skip).collect();
    format!("{MARKER}{tail}")
}

/// Atomic write via the same tmp+rename pattern as `src-tauri`'s
/// `write_agent_live_state` (replicated here rather than imported — this module is
/// a self-contained unit; see `backend/agents.rs` for the sibling implementation).
/// Writes to a PER-CALL uniquely-named sibling tmp ([`tmp_path_for`]) then renames
/// over the target, so a crash mid-write never leaves a truncated `preplan.md`.
///
/// The tmp name is unique per call (process id + monotonic-ish nanos), NOT a fixed
/// `"<name>.tmp"` sibling: two orchestrator processes writing the SAME project
/// root's preplan concurrently would otherwise race on the identical tmp path —
/// writer A could write its tmp, writer B overwrite that same tmp with ITS content,
/// A rename (getting B's content under A's identity), then B's own rename ENOENT
/// (silently swallowed by every caller here, which is `let _ = atomic_write(...)`)
/// — losing B's write entirely. A unique-per-call tmp name means each writer only
/// ever renames its OWN file; the last rename to complete simply wins, which is the
/// expected last-writer-wins semantics for this best-effort scratch file. Creates
/// parent dirs.
fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = tmp_path_for(path);
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

/// The per-call unique tmp sibling path for `path`: `"<file_name>.<pid>-<nanos>.tmp"`
/// — same pattern as `write_agent_live_state` in `src-tauri/src/backend/agents.rs`
/// (pid disambiguates across PROCESSES, the nanosecond timestamp disambiguates
/// across CALLS within the same process). See [`atomic_write`] for why a fixed
/// sibling name is unsafe under concurrent writers.
fn tmp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("preplan");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut tmp = path.to_path_buf();
    tmp.set_file_name(format!("{file_name}.{}-{nanos}.tmp", std::process::id()));
    tmp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn load_or_init_creates_scaffold_with_goal() {
        let dir = tmp_root();
        let p = Preplan::load_or_init(dir.path(), "build the thing");
        let body = std::fs::read_to_string(&p.path).unwrap();
        assert!(body.starts_with("## Goal\nbuild the thing\n"));
        for section in ["## Constraints", "## Findings", "## Decisions", "## Open questions", "## Draft outline"] {
            assert!(body.contains(section), "scaffold missing {section}: {body}");
        }
    }

    #[test]
    fn load_or_init_same_goal_resumes_and_preserves_findings() {
        let dir = tmp_root();
        let p1 = Preplan::load_or_init(dir.path(), "g");
        p1.append("Findings", "- learned X");
        let p2 = Preplan::load_or_init(dir.path(), "g");
        let body = std::fs::read_to_string(&p2.path).unwrap();
        assert!(body.contains("- learned X"), "same-goal resume keeps prior findings: {body}");
    }

    #[test]
    fn load_or_init_different_goal_reinits_and_drops_stale_findings() {
        let dir = tmp_root();
        let p1 = Preplan::load_or_init(dir.path(), "goal A");
        p1.append("Findings", "- learned about A");
        let p2 = Preplan::load_or_init(dir.path(), "goal B");
        let body = std::fs::read_to_string(&p2.path).unwrap();
        assert!(!body.contains("learned about A"), "a different goal must not leak stale findings: {body}");
        assert!(body.starts_with("## Goal\ngoal B\n"));
    }

    #[test]
    fn load_or_init_trims_the_goal_before_comparing() {
        let dir = tmp_root();
        let p1 = Preplan::load_or_init(dir.path(), "  g  ");
        p1.append("Findings", "- kept");
        // A later call with the untrimmed-equivalent goal must still resume.
        let p2 = Preplan::load_or_init(dir.path(), "g");
        let body = std::fs::read_to_string(&p2.path).unwrap();
        assert!(body.contains("- kept"), "whitespace-only goal differences must still resume: {body}");
    }

    #[test]
    fn append_adds_a_line_under_the_named_section_only() {
        let dir = tmp_root();
        let p = Preplan::load_or_init(dir.path(), "g");
        p.append("Decisions", "attempt 1 rejected: bad scope");
        let body = std::fs::read_to_string(&p.path).unwrap();
        let decisions_idx = body.find("## Decisions").unwrap();
        let next_idx = body.find("## Open questions").unwrap();
        assert!(body[decisions_idx..next_idx].contains("attempt 1 rejected: bad scope"));
        // It must NOT have leaked into a different section.
        let findings_idx = body.find("## Findings").unwrap();
        assert!(!body[findings_idx..decisions_idx].contains("attempt 1 rejected"));
    }

    #[test]
    fn append_is_idempotent_no_duplicate_line() {
        let dir = tmp_root();
        let p = Preplan::load_or_init(dir.path(), "g");
        p.append("Findings", "- same line");
        p.append("Findings", "- same line");
        p.append("Findings", "  - same line  "); // whitespace-different but same trimmed content
        let body = std::fs::read_to_string(&p.path).unwrap();
        assert_eq!(body.matches("- same line").count(), 1, "duplicate append is a no-op: {body}");
    }

    #[test]
    fn append_to_unknown_section_is_a_silent_noop() {
        let dir = tmp_root();
        let p = Preplan::load_or_init(dir.path(), "g");
        let before = std::fs::read_to_string(&p.path).unwrap();
        p.append("NotASection", "- whatever");
        let after = std::fs::read_to_string(&p.path).unwrap();
        assert_eq!(before, after, "an unknown section name must not modify the file");
    }

    #[test]
    fn render_for_prompt_returns_whole_file_when_it_already_fits() {
        let dir = tmp_root();
        let p = Preplan::load_or_init(dir.path(), "g");
        p.append("Findings", "- a small finding");
        let rendered = p.render_for_prompt(10_000);
        let body = std::fs::read_to_string(&p.path).unwrap();
        assert_eq!(rendered, body);
    }

    #[test]
    fn render_for_prompt_truncates_tail_preserving_and_always_keeps_goal() {
        let dir = tmp_root();
        let p = Preplan::load_or_init(dir.path(), "the important goal");
        for i in 0..300 {
            p.append("Findings", &format!("- finding number {i}"));
        }
        let cap = 400;
        let rendered = p.render_for_prompt(cap);
        assert!(
            rendered.chars().count() <= cap,
            "hard-bounded to {cap}, got {}",
            rendered.chars().count()
        );
        assert!(rendered.contains("the important goal"), "Goal is always present: {rendered}");
        assert!(rendered.contains("finding number 299"), "the TAIL (most recent) survives: {rendered}");
        assert!(!rendered.contains("finding number 0\n"), "the head is dropped: {rendered}");
    }

    #[test]
    fn render_for_prompt_never_exceeds_cap_even_for_a_giant_goal() {
        let dir = tmp_root();
        let giant_goal = "g".repeat(50_000);
        let p = Preplan::load_or_init(dir.path(), &giant_goal);
        p.append("Findings", "- a finding");
        let rendered = p.render_for_prompt(500);
        assert!(rendered.chars().count() <= 500, "even a pathological Goal must be hard-bounded");
    }

    #[test]
    fn clear_removes_the_file() {
        let dir = tmp_root();
        let p = Preplan::load_or_init(dir.path(), "g");
        assert!(p.path.exists());
        p.clear();
        assert!(!p.path.exists());
        // Clearing twice (already gone) must not panic.
        p.clear();
    }

    #[test]
    fn append_is_a_noop_when_the_on_disk_file_now_belongs_to_a_different_goal() {
        // Simulates two concurrent orchestrator sessions on the SAME project root:
        // `p1` loaded first for "goal A"; a second, later session re-scaffolds the
        // same file for a DIFFERENT "goal B" (e.g. after `p1`'s process crashed and
        // the project root was reused for unrelated work). `p1` is now STALE: it
        // must re-validate the on-disk header before mutating and skip the append
        // rather than write goal-A findings under goal-B's header.
        let dir = tmp_root();
        let p1 = Preplan::load_or_init(dir.path(), "goal A");
        let _p2 = Preplan::load_or_init(dir.path(), "goal B");
        p1.append("Findings", "- p1's stale finding");
        let body = std::fs::read_to_string(&p1.path).unwrap();
        assert!(
            !body.contains("p1's stale finding"),
            "a goal-mismatched writer must never mutate another session's file: {body}"
        );
        assert!(
            body.starts_with("## Goal\ngoal B\n"),
            "goal B's file must be untouched by the stale writer: {body}"
        );
    }

    #[test]
    fn clear_is_a_noop_when_the_on_disk_file_now_belongs_to_a_different_goal() {
        let dir = tmp_root();
        let p1 = Preplan::load_or_init(dir.path(), "goal A");
        let _p2 = Preplan::load_or_init(dir.path(), "goal B");
        p1.clear();
        assert!(
            p1.path.exists(),
            "clearing a stale-goal Preplan must not delete another session's live file"
        );
        let body = std::fs::read_to_string(&p1.path).unwrap();
        assert!(body.starts_with("## Goal\ngoal B\n"));
    }

    #[test]
    fn tmp_path_for_is_unique_per_call_not_a_fixed_sibling_name() {
        // Two concurrent writers (two orchestrator processes on the SAME project
        // root) must never race on the SAME tmp path: a fixed "<name>.tmp" sibling
        // lets writer A's rename clobber writer B's in-flight tmp (or vice versa),
        // silently corrupting/losing whichever content loses the race. The tmp name
        // must be unique PER CALL (pid + monotonic nanos), not a fixed name.
        let target = Path::new("/tmp/does-not-need-to-exist/preplan.md");
        let a = tmp_path_for(target);
        let b = tmp_path_for(target);
        assert_ne!(a, b, "two calls must never produce the same tmp path: {a:?}");
        for p in [&a, &b] {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            assert!(name.starts_with("preplan.md."), "unexpected tmp name: {name}");
            assert!(name.ends_with(".tmp"), "unexpected tmp name: {name}");
            assert!(
                name.contains(&std::process::id().to_string()),
                "tmp name must embed this process's pid: {name}"
            );
        }
    }

    #[test]
    fn atomic_write_leaves_no_tmp_sibling_behind_on_success() {
        let dir = tmp_root();
        let target = dir.path().join("preplan.md");
        atomic_write(&target, "first").unwrap();
        atomic_write(&target, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "second");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "no tmp sibling must survive a successful atomic_write: {leftovers:?}"
        );
    }
}
