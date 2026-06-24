//! Backend commands for the "Changes" tab.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Fixed allowlist of external editors that may be spawned.
pub const ALLOWED_EDITORS: [&str; 4] = ["code", "cursor", "zed", "idea"];

/// Maximum size (in bytes) of a returned diff string before truncation.
const DIFF_MAX_BYTES: usize = 200_000;

/// Truncation suffix appended when a diff exceeds the byte cap.
const DIFF_TRUNCATION_NOTE: &str = "\n… (diff truncated)";

/// BLOCKER 3: hard deadline for a single git invocation. A hung git (fsmonitor,
/// `diff.external`, a network filesystem, a credential prompt) must NEVER block the
/// Tauri worker forever. Mirrors `projects::git_output_timeout`'s spawn+poll+kill
/// pattern with a short local-only budget (`git diff`/`ls-files` do no network IO).
const GIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Returns true iff `name` is in the fixed editor allowlist.
fn is_allowed_editor(name: &str) -> bool {
    ALLOWED_EDITORS.contains(&name)
}

/// Truncate `s` to at most `max_bytes` bytes on a UTF-8 char boundary,
/// appending `note` when truncation occurs. Never splits a multibyte char.
fn truncate_diff(s: &str, max_bytes: usize, note: &str) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    // Walk back to a char boundary.
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + note.len());
    out.push_str(&s[..end]);
    out.push_str(note);
    out
}

/// Resolve `git` via the augmented PATH detector, falling back to a bare
/// `git` (relying on the spawn environment) if not found.
fn resolve_git() -> PathBuf {
    super::provider_detect::resolve_program("git")
        .unwrap_or_else(|| PathBuf::from("git"))
}

/// True if a git failure stderr indicates a missing/unknown HEAD revision.
fn is_missing_head_error(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("unknown revision")
        || lower.contains("bad revision")
        || lower.contains("ambiguous argument 'head'")
}

/// Resolve a project's CANONICAL working root from its id, SERVER-SIDE.
///
/// BLOCKER 2 (project confinement): the frontend never supplies a raw filesystem
/// path. We resolve the project's REGISTERED `root_path` from the project store
/// (`projects::resolve_project_root_by_id`, the same resolution the agent/mini
/// launchers use) and canonicalize it, so these commands can only ever operate
/// inside a declared project tree — a caller cannot point them at `/etc` or any
/// arbitrary directory.
fn resolve_project_root(app: &tauri::AppHandle, project_id: &str) -> Result<PathBuf, String> {
    let root = crate::backend::projects::resolve_project_root_by_id(app, project_id)?;
    let canonical = std::fs::canonicalize(&root)
        .map_err(|e| format!("invalid project root: {e}"))?;
    if !canonical.is_dir() {
        return Err("project root is not a directory".to_string());
    }
    Ok(canonical)
}

/// Run `<git> -C <root> <args..>` capturing stdout/stderr under a hard deadline.
///
/// BLOCKER 3: uses a spawn+poll+kill loop (not blocking `Command::output()`) so a
/// wedged git cannot pin the Tauri worker thread past [`GIT_TIMEOUT`]. Also fully
/// neutralizes credential prompting:
///   - `GIT_TERMINAL_PROMPT=0` → git never blocks on an interactive prompt;
///   - `GIT_ASKPASS=""` → git cannot fall back to an askpass helper either.
/// (The Changes tab is read-only — `diff`/`ls-files` — so auth is never expected;
/// these guards ensure a misconfigured repo can't wedge us waiting for a password.)
///
/// DEADLOCK-SAFE DRAIN: a `git diff` can emit far more than the OS pipe buffer
/// (~64 KiB). If we polled `try_wait()` without reading the pipes, git would block
/// on a full stdout pipe, `try_wait()` would never report exit, and we would only
/// kill it at the timeout — silently truncating any large-but-fast diff. So we drain
/// stdout AND stderr on dedicated reader threads (concurrently with the child), then
/// bound the WAIT on those readers by the deadline: on overrun we kill the child,
/// which closes the pipes and lets the readers reach EOF and join.
fn run_git(git: &Path, root: &Path, args: &[&str]) -> Result<String, String> {
    use std::io::Read;

    let mut command = Command::new(git);
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: avoid a conhost flash for each spawn in the release GUI.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command
        .spawn()
        .map_err(|e| format!("failed to spawn git: {e}"))?;

    // Take the pipe handles and drain each on its own thread so the child can never
    // block on a full pipe while we wait (the FIX-1 no-deadlock invariant).
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut p) = stdout_pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut p) = stderr_pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    // Poll for exit under the deadline. On overrun, kill the child: that closes the
    // pipes, the reader threads hit EOF, and the joins below return promptly.
    let started_at = Instant::now();
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) => {
                if started_at.elapsed() >= GIT_TIMEOUT {
                    let _ = child.kill();
                    break true;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("failed to wait on git: {e}"));
            }
        }
    };

    // Reap the child (after a kill this is non-blocking) and join the drained output.
    let status = child.wait().ok();
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();

    if timed_out {
        return Err("git timed out".to_string());
    }
    match status {
        Some(s) if s.success() => Ok(String::from_utf8_lossy(&stdout).into_owned()),
        _ => Err(String::from_utf8_lossy(&stderr).trim().to_string()),
    }
}

/// WARNING 4: list the repo's untracked, non-ignored files. `git diff HEAD` omits
/// these entirely, so a repo whose only changes are brand-new (un-added) files would
/// otherwise look clean. Rendered as a labeled section appended to the diff.
fn untracked_section(git: &Path, root: &Path) -> Option<String> {
    let out = run_git(git, root, &["ls-files", "--others", "--exclude-standard"]).ok()?;
    let trimmed = out.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut section = String::from("# Untracked files (not staged):\n");
    for file in trimmed.lines() {
        section.push_str("?? ");
        section.push_str(file);
        section.push('\n');
    }
    Some(section)
}

/// B13: filename of the per-repo diff baseline marker. Stored INSIDE the git dir so
/// it travels with the repo and never shows up as an untracked work-tree file.
const ASPIS_DIFF_BASELINE: &str = "aspis-diff-baseline";

/// B13: resolve the baseline marker path via `--absolute-git-dir` (handles worktrees /
/// submodules where `.git` is a file, not a directory). `None` when not a git repo.
fn baseline_path(git: &Path, root: &Path) -> Option<PathBuf> {
    let git_dir = run_git(git, root, &["rev-parse", "--absolute-git-dir"]).ok()?;
    let git_dir = git_dir.trim();
    if git_dir.is_empty() {
        return None;
    }
    Some(Path::new(git_dir).join(ASPIS_DIFF_BASELINE))
}

/// B13: capture a diff baseline the FIRST time an agent launches for this repo, so the
/// project's "Changes" view shows what the AGENTS changed — not pre-existing dirty edits
/// already in the work tree (e.g. a developer's own unrelated edits to the SAME repo,
/// the reported bug). Idempotent: once a baseline exists it is NOT reset, so it keeps
/// representing the state before the project's agents first touched the tree. Best-effort:
/// any git failure is a silent no-op (the diff then falls back to `git diff HEAD`).
pub fn ensure_diff_baseline(root: &Path) {
    ensure_diff_baseline_with_git(&resolve_git(), root);
}

fn ensure_diff_baseline_with_git(git: &Path, root: &Path) {
    let Some(path) = baseline_path(git, root) else {
        return;
    };
    if path.exists() {
        return;
    }
    // Snapshot the current dirty work tree as a commit object WITHOUT touching the tree
    // (`stash create` prints a commit sha and leaves the tree untouched). Empty output ⇒
    // a clean tree ⇒ baseline is HEAD. No HEAD (fresh repo) ⇒ skip (nothing to baseline).
    let snapshot = match run_git(git, root, &["stash", "create"]) {
        Ok(out) if !out.trim().is_empty() => out.trim().to_string(),
        _ => match run_git(git, root, &["rev-parse", "HEAD"]) {
            Ok(head) if !head.trim().is_empty() => head.trim().to_string(),
            _ => return,
        },
    };
    // Reviewer max-recall: atomic create_new so two concurrent agent launches can't both
    // pass the exists() check and clobber each other's baseline — the FIRST writer wins
    // (it captured the earliest "before agents" state), the loser is a silent no-op.
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        let _ = file.write_all(snapshot.as_bytes());
    }
}

/// B13: read a STILL-VALID baseline commit for `root`, or `None`. Validated with
/// `cat-file -e` so a stale/garbage-collected snapshot can never break the diff (we
/// fall back to HEAD).
fn read_diff_baseline(git: &Path, root: &Path) -> Option<String> {
    let path = baseline_path(git, root)?;
    let sha = std::fs::read_to_string(&path).ok()?.trim().to_string();
    if sha.is_empty() {
        // Reviewer max-recall: an empty marker (e.g. a write that was interrupted after
        // create_new) would otherwise stick forever (ensure_diff_baseline early-returns on
        // exists()). Remove it so the NEXT launch re-captures a real baseline.
        let _ = std::fs::remove_file(&path);
        return None;
    }
    run_git(git, root, &["cat-file", "-e", &format!("{sha}^{{commit}}")]).ok()?;
    Some(sha)
}

/// Tracked-changes diff against HEAD, falling back to the unstaged diff when the repo
/// has no HEAD yet (fresh repo). The original `working_diff_for_root` behavior, factored
/// out so the B13 baseline path can reuse it as a fallback.
fn diff_against_head(git: &Path, root: &Path) -> Result<String, String> {
    match run_git(git, root, &["diff", "HEAD"]) {
        Ok(stdout) => Ok(stdout),
        Err(stderr) => {
            if is_missing_head_error(&stderr) {
                run_git(git, root, &["diff"])
            } else {
                Err(stderr)
            }
        }
    }
}

/// Compose the full diff body for a canonical `root`: tracked changes vs the captured
/// baseline (B13 — falls back to HEAD, or the unstaged diff when there is no HEAD yet)
/// followed by a labeled untracked-files section, then truncated to the byte cap.
/// Factored out of the `#[tauri::command]` so the git logic is unit-testable against a
/// real temp repo without a Tauri `AppHandle`/`State`.
fn working_diff_for_root(git: &Path, root: &Path) -> Result<String, String> {
    // B13: prefer a diff against the captured baseline (changes since the agents started),
    // so unrelated pre-existing dirty edits in the same repo don't pollute the view.
    let tracked = if let Some(base) = read_diff_baseline(git, root) {
        match run_git(git, root, &["diff", &base]) {
            Ok(stdout) => stdout,
            // A baseline that no longer diffs cleanly (e.g. object gone) ⇒ fall back.
            Err(_) => diff_against_head(git, root)?,
        }
    } else {
        diff_against_head(git, root)?
    };

    let mut body = tracked;
    if let Some(untracked) = untracked_section(git, root) {
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str(&untracked);
    }

    Ok(truncate_diff(&body, DIFF_MAX_BYTES, DIFF_TRUNCATION_NOTE))
}

/// Working-tree diff against HEAD for a project (falls back to the unstaged diff when
/// there is no HEAD), plus an untracked-files section. The project root is resolved
/// SERVER-SIDE from `project_id` (never a caller-supplied path).
#[tauri::command]
pub fn git_working_diff(
    project_id: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::backend::state::BackendState>,
) -> Result<String, String> {
    state.ensure_unlocked()?;
    let root = resolve_project_root(&app, &project_id)?;
    let git = resolve_git();
    working_diff_for_root(&git, &root)
}

/// Open the project root in an external editor (detached). The project root is
/// resolved SERVER-SIDE from `project_id` (never a caller-supplied path).
#[tauri::command]
pub fn open_in_editor(
    project_id: String,
    editor: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::backend::state::BackendState>,
) -> Result<(), String> {
    state.ensure_unlocked()?;

    // Editor allowlist BEFORE any path resolution or spawn (unchanged ordering).
    if !is_allowed_editor(&editor) {
        return Err(format!("editor not allowed: {editor}"));
    }

    let binary = super::provider_detect::resolve_program(&editor)
        .ok_or_else(|| format!("editor not installed: {editor}"))?;

    let root = resolve_project_root(&app, &project_id)?;

    Command::new(binary)
        .arg(&root)
        .spawn()
        .map_err(|e| format!("failed to launch editor: {e}"))?;

    Ok(())
}

/// List installed external editors from the fixed allowlist (in order).
///
/// Gated on the app lock for consistency with the other Changes commands: it only
/// probes PATH (low-risk), but keeping every command behind `ensure_unlocked()`
/// avoids a surface that responds while the app is locked.
#[tauri::command]
pub fn list_external_editors(
    state: tauri::State<'_, crate::backend::state::BackendState>,
) -> Result<Vec<String>, String> {
    state.ensure_unlocked()?;
    Ok(ALLOWED_EDITORS
        .iter()
        .filter_map(|name| {
            if super::provider_detect::resolve_program(name).is_some() {
                Some((*name).to_string())
            } else {
                None
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_rejects_unknown_editors() {
        assert!(is_allowed_editor("code"));
        assert!(is_allowed_editor("cursor"));
        assert!(is_allowed_editor("zed"));
        assert!(is_allowed_editor("idea"));

        // Dangerous / unknown strings must be rejected.
        assert!(!is_allowed_editor("rm"));
        assert!(!is_allowed_editor("sh"));
        assert!(!is_allowed_editor("code; rm -rf /"));
        assert!(!is_allowed_editor(""));
        assert!(!is_allowed_editor("CODE")); // case-sensitive
        assert!(!is_allowed_editor("vim"));
    }

    #[test]
    fn truncate_diff_keeps_short_strings_unchanged() {
        let s = "hello world";
        assert_eq!(truncate_diff(s, 100, "..."), s);
        assert_eq!(truncate_diff("", 100, "..."), "");
    }

    #[test]
    fn truncate_diff_caps_and_appends_note() {
        let s = "abcdefghij".repeat(1000); // 10000 bytes
        let out = truncate_diff(&s, 100, "[cut]");
        assert!(out.len() <= 100 + "[cut]".len());
        assert!(out.ends_with("[cut]"));
        // The non-suffix portion is a prefix of the input.
        let prefix = &out[..out.len() - "[cut]".len()];
        assert!(s.starts_with(prefix));
    }

    #[test]
    fn truncate_diff_never_splits_multibyte_char() {
        // 'é' is 2 bytes in UTF-8; fill with é then truncate at an odd byte.
        let s = "é".repeat(500); // 1000 bytes
        let out = truncate_diff(&s, 101, "…");
        // Must be valid UTF-8 by construction (it's a String), but verify
        // the boundary logic explicitly: the prefix length is even.
        let note = "…";
        let prefix_len = out.len() - note.len();
        assert_eq!(prefix_len % 2, 0, "split a multibyte char");
        assert!(out.ends_with(note));
        // And the prefix is a whole number of é chars.
        let prefix = &out[..prefix_len];
        assert_eq!(prefix.chars().count(), prefix_len / 2);
    }

    #[test]
    fn truncate_diff_exact_boundary() {
        let s = "abc".to_string();
        // Exactly at limit -> no truncation.
        assert_eq!(truncate_diff(&s, 3, "X"), "abc");
        // One byte over -> truncate.
        let out = truncate_diff(&s, 2, "X");
        assert_eq!(out, "abX");
    }

    #[test]
    fn is_missing_head_error_recognizes_known_phrasings() {
        assert!(is_missing_head_error("fatal: bad revision 'HEAD'"));
        assert!(is_missing_head_error(
            "fatal: ambiguous argument 'HEAD': unknown revision or path not in the working tree."
        ));
        assert!(is_missing_head_error("unknown revision: HEAD"));
        assert!(!is_missing_head_error("fatal: not a git repository"));
        assert!(!is_missing_head_error(""));
    }

    // ---- BLOCKER 2 / 3 / WARNING 4: the git diff helper against a real repo ----

    /// A unique temp dir for a test repo (kept off the project tree).
    fn temp_repo_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aspis-changes-{}-{tag}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    /// Run a git command in `dir`, failing the test on a non-zero exit. Used only to
    /// set up the fixture repo (git is assumed present in the test environment).
    fn git_in(git: &Path, dir: &Path, args: &[&str]) {
        let status = Command::new(git)
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            // Deterministic identity so `git commit` works in CI with no global config.
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("spawn git for fixture");
        assert!(
            status.status.success(),
            "fixture git {args:?} failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    /// Skip the repo-backed tests when git is not on PATH (resolve_git falls back to
    /// a bare "git" that won't spawn). Returns the resolved git path when usable.
    fn git_or_skip() -> Option<PathBuf> {
        super::super::provider_detect::resolve_program("git")
    }

    #[test]
    fn working_diff_includes_tracked_changes() {
        let Some(git) = git_or_skip() else { return };
        let repo = temp_repo_dir("tracked");
        git_in(&git, &repo, &["init", "-q"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git_in(&git, &repo, &["add", "a.txt"]);
        git_in(&git, &repo, &["commit", "-q", "-m", "init"]);
        // Modify the tracked file -> shows up in `git diff HEAD`.
        std::fs::write(repo.join("a.txt"), "two\n").unwrap();

        let out = working_diff_for_root(&git, &repo).unwrap();
        assert!(out.contains("a.txt"), "diff should mention a.txt: {out}");
        assert!(out.contains("-one") && out.contains("+two"), "got: {out}");

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn working_diff_surfaces_untracked_files() {
        let Some(git) = git_or_skip() else { return };
        let repo = temp_repo_dir("untracked");
        git_in(&git, &repo, &["init", "-q"]);
        std::fs::write(repo.join("seed.txt"), "x\n").unwrap();
        git_in(&git, &repo, &["add", "seed.txt"]);
        git_in(&git, &repo, &["commit", "-q", "-m", "init"]);
        // A brand-new, un-added file: invisible to `git diff HEAD` but must surface.
        std::fs::write(repo.join("brand_new.txt"), "hello\n").unwrap();

        let out = working_diff_for_root(&git, &repo).unwrap();
        assert!(
            out.contains("Untracked files"),
            "missing untracked section: {out}"
        );
        assert!(
            out.contains("?? brand_new.txt"),
            "missing untracked file line: {out}"
        );

        let _ = std::fs::remove_dir_all(&repo);
    }

    /// B13: with a captured baseline, the diff shows ONLY changes made AFTER the
    /// baseline (the agents' work) — a pre-existing dirty edit (a developer's own
    /// edit to the same repo) is in the baseline snapshot and must NOT appear.
    #[test]
    fn baseline_scopes_diff_to_post_baseline_changes() {
        let Some(git) = git_or_skip() else { return };
        let repo = temp_repo_dir("baseline");
        git_in(&git, &repo, &["init", "-q"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git_in(&git, &repo, &["add", "a.txt"]);
        git_in(&git, &repo, &["commit", "-q", "-m", "init"]);

        // PRE-LAUNCH developer dirty edit (unrelated to the project's agents).
        std::fs::write(repo.join("a.txt"), "one\nDEV_EDIT\n").unwrap();
        // Baseline captured at launch — snapshots the dirty tree (incl. DEV_EDIT).
        ensure_diff_baseline_with_git(&git, &repo);
        assert!(
            baseline_path(&git, &repo).unwrap().exists(),
            "baseline marker must be written"
        );

        // POST-LAUNCH agent edit.
        std::fs::write(repo.join("a.txt"), "one\nDEV_EDIT\nAGENT_EDIT\n").unwrap();

        let out = working_diff_for_root(&git, &repo).unwrap();
        assert!(out.contains("AGENT_EDIT"), "agent change must show: {out}");
        assert!(
            !out.contains("+DEV_EDIT"),
            "the pre-existing dirty edit is in the baseline and must NOT show: {out}"
        );

        // Idempotent: a second ensure does not reset the baseline.
        let first = std::fs::read_to_string(baseline_path(&git, &repo).unwrap()).unwrap();
        ensure_diff_baseline_with_git(&git, &repo);
        let second = std::fs::read_to_string(baseline_path(&git, &repo).unwrap()).unwrap();
        assert_eq!(first, second, "baseline must not be reset once captured");

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn working_diff_drains_large_output_without_deadlock() {
        // A diff far larger than the OS pipe buffer (~64 KiB) must NOT deadlock the
        // poll loop: the reader threads drain the pipe concurrently. We add ~512 KiB
        // of new tracked content (well past the cap so it also exercises truncation).
        let Some(git) = git_or_skip() else { return };
        let repo = temp_repo_dir("large");
        git_in(&git, &repo, &["init", "-q"]);
        std::fs::write(repo.join("big.txt"), "seed\n").unwrap();
        git_in(&git, &repo, &["add", "big.txt"]);
        git_in(&git, &repo, &["commit", "-q", "-m", "init"]);
        // Replace with many lines so `git diff HEAD` emits a large stream.
        let big: String = (0..40_000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(repo.join("big.txt"), big).unwrap();

        let out = working_diff_for_root(&git, &repo).unwrap();
        // It returned (no hang) and was truncated to the byte cap + note.
        assert!(out.len() <= DIFF_MAX_BYTES + DIFF_TRUNCATION_NOTE.len());
        assert!(out.ends_with(DIFF_TRUNCATION_NOTE), "expected truncation note");

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn working_diff_no_head_falls_back_to_unstaged() {
        let Some(git) = git_or_skip() else { return };
        let repo = temp_repo_dir("nohead");
        git_in(&git, &repo, &["init", "-q"]);
        // Stage a file but NEVER commit -> no HEAD; `git diff HEAD` errors and we fall
        // back to `git diff` (which here is empty for staged-only) while still listing
        // the staged file as... actually staged files are tracked-but-uncommitted, so
        // they appear via the no-HEAD fallback path. Add an untracked file too.
        std::fs::write(repo.join("staged.txt"), "s\n").unwrap();
        git_in(&git, &repo, &["add", "staged.txt"]);
        std::fs::write(repo.join("loose.txt"), "l\n").unwrap();

        // Must NOT error despite the missing HEAD, and must surface the untracked file.
        let out = working_diff_for_root(&git, &repo).unwrap();
        assert!(out.contains("?? loose.txt"), "got: {out}");

        let _ = std::fs::remove_dir_all(&repo);
    }
}
